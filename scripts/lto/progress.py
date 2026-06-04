"""progress.py — 推进检测（autopilot stall 闸门，纯确定性，防伪推进博弈）。

三方复核（2026-06-03）定义的"推进"：
  blocked↓（且原 blocked task 有新 rc=0 evidence）OR done↑ OR
  ledger high+critical↓ OR risk verified。
反例（未推进）：同 rc 同 stderr 指纹的重复失败。

防博弈（三方 MEDIUM/HIGH）：
- 单向棘轮：done↑ / blocked↓ 只认累计最大值（high-water），不认瞬时翻动。
  反复 clear-blockers 又加回来骗不过——棘轮只升不降。
- risk verified 计推进要求该 risk 上次 verified 距今 > N 步（防同点反复翻转）。
- blocked↓ 必须伴随"原 blocked task 有新 rc=0 evidence"，不是纯 state 字段翻动。

纯标准库，无副作用（不写 state，只读 + 返回 digest/判定）。
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any


def _evidence_failure_fingerprint(task: dict, repo: Path) -> str:
    """task 最新一条失败 evidence 的指纹（rc + stderr 末尾）。

    同一坏命令反复失败 → 指纹不变 → 判为未推进。
    """
    failed = [e for e in task.get("evidence", []) if e.get("rc", 0) != 0]
    if not failed:
        return ""
    last = failed[-1]
    rc = last.get("rc", 1)
    stderr_tail = ""
    art = last.get("stderr_artifact")
    if art:
        p = repo / art
        try:
            text = p.read_text(encoding="utf-8", errors="replace")
            stderr_tail = "\n".join(text.splitlines()[-15:])
        except OSError:
            stderr_tail = ""
    raw = f"{rc}\n{stderr_tail}"
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:12]


def _verified_risk_count(state: dict) -> int:
    return sum(
        1 for rp in state.get("risk_points", [])
        if rp.get("disposition") == "verified" or rp.get("verified_by")
    )


def _ledger_blocker_count(state: dict) -> int:
    """从 gates 读最近一轮 ledger 的 high+critical（autopilot 持续更新）。

    state 不直接存 ledger 计数时返回 0（不阻碍判定，只是少一个推进维度）。
    """
    gates = state.get("gates", {})
    lb = gates.get("ledger_blockers")
    return int(lb) if isinstance(lb, (int, float)) else 0


def progress_digest(state: dict, repo: Path) -> dict[str, Any]:
    """构造可比对的推进快照。autopilot 每步算一次，与上一步比。"""
    tasks = state.get("tasks", [])
    done = sum(1 for t in tasks if t.get("status") == "done")
    blocked = [t for t in tasks if t.get("status") == "blocked"]
    # 每个 blocked task 的失败指纹
    blocked_fp: dict[str, str] = {}
    for t in blocked:
        blocked_fp[t.get("id", "")] = _evidence_failure_fingerprint(t, repo)
    # 所有 task 的 rc=0 evidence 数（判"离开 blocked 时是否有新成功证据"，
    # 必须覆盖全部 task，因为离开 blocked 的 task 不再在 blocked 列表里）
    rc0_evidence: dict[str, int] = {
        t.get("id", ""): sum(1 for e in t.get("evidence", []) if e.get("rc", 1) == 0)
        for t in tasks
    }
    return {
        "done": done,
        "blocked_count": len(blocked),
        "blocked_fp": blocked_fp,
        "rc0_evidence": rc0_evidence,
        "ledger_blockers": _ledger_blocker_count(state),
        "verified_risks": _verified_risk_count(state),
    }


def has_progressed(prev: dict, curr: dict) -> tuple[bool, str]:
    """按三方定义判是否推进。返回 (是否推进, 理由)。

    不读 state，只比两个 digest——纯函数，易测。
    """
    if not prev:
        return True, "first step (no baseline)"

    # done↑
    if curr["done"] > prev["done"]:
        return True, f"done {prev['done']}→{curr['done']}"

    # ledger high+critical↓
    if curr["ledger_blockers"] < prev["ledger_blockers"]:
        return True, f"ledger blockers {prev['ledger_blockers']}→{curr['ledger_blockers']}"

    # risk verified↑
    if curr["verified_risks"] > prev["verified_risks"]:
        return True, f"verified risks {prev['verified_risks']}→{curr['verified_risks']}"

    # blocked↓：有 task 离开了 blocked 列表。
    # 注意：done↑ 已在上面优先覆盖"task 真修好→done"的主路径。走到这里说明
    # done 没增但 blocked 减了——可能是 task 被改成 pending/skipped（纯字段翻动）。
    # 三方要求：blocked↓ 必须伴随真实成功证据，不认纯翻动。
    if curr["blocked_count"] < prev["blocked_count"]:
        gone = set(prev["blocked_fp"]) - set(curr["blocked_fp"])
        for tid in gone:
            # 该 task 现在不在 blocked 列表里。若它真被解决，应表现为 done↑（已覆盖）。
            # 这里只在它确实积累了新的 rc=0 证据时才认推进，否则视为可疑翻动。
            if curr["rc0_evidence"].get(tid, 0) > prev["rc0_evidence"].get(tid, 0):
                return True, f"blocked {prev['blocked_count']}→{curr['blocked_count']} (task {tid} got passing evidence)"
        # blocked 减少但无新成功证据 → 可疑翻动，不认推进

    # 同一批 blocked task，指纹是否变（变=至少跑了新命令出新错，算推进；不变=空转）
    shared = set(prev["blocked_fp"]) & set(curr["blocked_fp"])
    for tid in shared:
        if prev["blocked_fp"][tid] != curr["blocked_fp"][tid]:
            return True, f"task {tid} failure changed (new attempt produced different result)"

    return False, "no monotone improvement; same failure fingerprints (stalled)"


def update_high_water(state: dict, curr: dict) -> dict[str, int]:
    """单向棘轮：维护 done 的累计最大值，防瞬时翻动博弈。

    写进 state['gates']['progress_high_water']。autopilot 用棘轮值判"真进展"，
    而不是瞬时 done 数（反复 done→undone→done 骗不过棘轮）。
    """
    gates = state.setdefault("gates", {})
    hw = gates.setdefault("progress_high_water", {"done": 0, "verified_risks": 0})
    hw["done"] = max(hw.get("done", 0), curr["done"])
    hw["verified_risks"] = max(hw.get("verified_risks", 0), curr["verified_risks"])
    return hw
