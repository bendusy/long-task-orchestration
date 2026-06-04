#!/usr/bin/env python3
"""standalone 测试：progress.py（autopilot stall 闸门，load-bearing 安全逻辑）。

覆盖 4 个函数：
  - progress_digest：快照构造（done / blocked_fp / rc0_evidence / verified_risks / ledger）
  - has_progressed：推进单调量判定（blocked↓/done↑/risk verified=推进；同 rc 同 stderr 指纹=未推进）
  - update_high_water：单向棘轮（digest 回退不被接受）
  - stall：同失败指纹 → 不推进

沿用 sibling 惯例（test_worktree_sandbox.py）：standalone runner + FAIL 累加 + sys.exit。
"""
from __future__ import annotations

import sys
import tempfile
import shutil
import subprocess
from pathlib import Path

ROOT = str(Path(__file__).resolve().parent)
sys.path.insert(0, ROOT)
from lto import progress as pg

FAIL = []


def ok(c, m):
    print(("OK   " if c else "FAIL ") + m, file=sys.stderr if not c else sys.stdout)
    if not c:
        FAIL.append(m)


def mkrepo(tmp: Path) -> Path:
    """临时 git repo（与 test_worktree_sandbox.py 同形）。"""
    repo = tmp / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, capture_output=True)
    subprocess.run(["git", "config", "user.name", "T"], cwd=repo, capture_output=True)
    subprocess.run(["git", "config", "user.email", "t@x.com"], cwd=repo, capture_output=True)
    return repo


def write_stderr_artifact(repo: Path, rel: str, text: str) -> str:
    """真写一个 stderr_artifact 文件，触发 _evidence_failure_fingerprint 的 _extract 分支。"""
    p = repo / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")
    return rel


# --------------------------------------------------------------------------- #
# progress_digest
# --------------------------------------------------------------------------- #
def test_digest_basic():
    print("\n=== digest: 基本字段 ===")
    tmp = Path(tempfile.mkdtemp(prefix="prog_"))
    try:
        repo = mkrepo(tmp)
        state = {
            "tasks": [
                {"id": "T1", "status": "done", "evidence": [{"rc": 0}]},
                {"id": "T2", "status": "done", "evidence": [{"rc": 0}, {"rc": 0}]},
                {"id": "T3", "status": "blocked", "evidence": [{"rc": 1}]},
                {"id": "T4", "status": "pending", "evidence": []},
            ],
            "gates": {"ledger_blockers": 3},
            "risk_points": [
                {"disposition": "verified"},
                {"verified_by": "codex"},
                {"disposition": "open"},
            ],
        }
        d = pg.progress_digest(state, repo)
        ok(d["done"] == 2, f"done 计数（2 done task）→ {d['done']}")
        ok(d["blocked_count"] == 1, f"blocked_count（仅 T3）→ {d['blocked_count']}")
        ok(d["ledger_blockers"] == 3, f"ledger_blockers 从 gates 读 → {d['ledger_blockers']}")
        ok(d["verified_risks"] == 2, f"verified_risks（disposition 或 verified_by）→ {d['verified_risks']}")
        # rc0_evidence 覆盖全部 task，不只 blocked
        ok(d["rc0_evidence"].get("T1") == 1, "rc0_evidence 覆盖 done task T1")
        ok(d["rc0_evidence"].get("T2") == 2, "rc0_evidence 计 rc=0 数量")
        ok(d["rc0_evidence"].get("T3") == 0, "blocked task T3 无 rc=0 证据")
        ok("T3" in d["blocked_fp"], "blocked_fp 含 blocked task T3")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_digest_ledger_absent_defaults_zero():
    print("\n=== digest: ledger 缺省为 0 ===")
    tmp = Path(tempfile.mkdtemp(prefix="prog_"))
    try:
        repo = mkrepo(tmp)
        d = pg.progress_digest({"tasks": []}, repo)
        ok(d["ledger_blockers"] == 0, "无 gates.ledger_blockers → 0（不阻碍判定）")
        ok(d["done"] == 0 and d["blocked_count"] == 0, "空 state 计数全 0")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_fingerprint_reads_artifact_file():
    print("\n=== digest: stderr_artifact 真文件触发指纹 _extract ===")
    tmp = Path(tempfile.mkdtemp(prefix="prog_"))
    try:
        repo = mkrepo(tmp)
        art = write_stderr_artifact(repo, "out/T9.stderr", "line1\nline2\nFATAL: boom\n")
        state = {
            "tasks": [{
                "id": "T9", "status": "blocked",
                "evidence": [{"rc": 1, "stderr_artifact": art}],
            }],
        }
        d = pg.progress_digest(state, repo)
        fp = d["blocked_fp"]["T9"]
        ok(bool(fp), f"读到 artifact 文件 → 非空指纹 {fp}")
        # 同内容 → 同指纹（确定性）
        d2 = pg.progress_digest(state, repo)
        ok(d2["blocked_fp"]["T9"] == fp, "同 rc 同 stderr 内容 → 同指纹（确定性）")
        # 改 stderr 内容 → 指纹变（跑了新命令出新错）
        write_stderr_artifact(repo, "out/T9.stderr", "line1\nline2\nFATAL: different error\n")
        d3 = pg.progress_digest(state, repo)
        ok(d3["blocked_fp"]["T9"] != fp, "stderr 内容变 → 指纹变")
        # 缺失 artifact 文件不崩（OSError → stderr_tail 空），仍由 rc 决定指纹
        state2 = {"tasks": [{"id": "T9", "status": "blocked",
                             "evidence": [{"rc": 1, "stderr_artifact": "out/missing.stderr"}]}]}
        d4 = pg.progress_digest(state2, repo)
        ok("T9" in d4["blocked_fp"], "artifact 文件缺失也不崩，仍产出指纹")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


# --------------------------------------------------------------------------- #
# has_progressed
# --------------------------------------------------------------------------- #
def test_progressed_first_step():
    print("\n=== has_progressed: 首步无 baseline ===")
    curr = {"done": 0, "blocked_count": 0, "blocked_fp": {}, "rc0_evidence": {},
            "ledger_blockers": 0, "verified_risks": 0}
    moved, why = pg.has_progressed({}, curr)
    ok(moved, f"prev 空 → 推进（首步）：{why}")


def test_progressed_done_up():
    print("\n=== has_progressed: done↑ ===")
    base = {"done": 1, "blocked_count": 1, "blocked_fp": {"T1": "aa"},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    curr = {**base, "done": 2}
    moved, why = pg.has_progressed(base, curr)
    ok(moved and "done 1→2" in why, f"done 1→2 = 推进：{why}")


def test_progressed_ledger_down():
    print("\n=== has_progressed: ledger blockers↓ ===")
    base = {"done": 1, "blocked_count": 0, "blocked_fp": {}, "rc0_evidence": {},
            "ledger_blockers": 5, "verified_risks": 0}
    curr = {**base, "ledger_blockers": 3}
    moved, why = pg.has_progressed(base, curr)
    ok(moved and "ledger blockers 5→3" in why, f"ledger 5→3 = 推进：{why}")


def test_progressed_risk_verified():
    print("\n=== has_progressed: risk verified↑ ===")
    base = {"done": 1, "blocked_count": 0, "blocked_fp": {}, "rc0_evidence": {},
            "ledger_blockers": 0, "verified_risks": 0}
    curr = {**base, "verified_risks": 1}
    moved, why = pg.has_progressed(base, curr)
    ok(moved and "verified risks 0→1" in why, f"verified risk 0→1 = 推进：{why}")


def test_progressed_blocked_down_with_evidence():
    print("\n=== has_progressed: blocked↓ 且有新 rc=0 证据 = 推进 ===")
    # done 不变（覆盖 done↑ 优先路径之外），blocked 减 1，离开的 T1 攒了新 rc=0 证据
    base = {"done": 1, "blocked_count": 1, "blocked_fp": {"T1": "aa"},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    curr = {"done": 1, "blocked_count": 0, "blocked_fp": {},
            "rc0_evidence": {"T1": 1}, "ledger_blockers": 0, "verified_risks": 0}
    moved, why = pg.has_progressed(base, curr)
    ok(moved and "passing evidence" in why, f"blocked↓ + 新 rc=0 = 推进：{why}")


def test_blocked_down_no_evidence_not_progress():
    print("\n=== has_progressed: blocked↓ 但无新成功证据 = 可疑翻动，不推进 ===")
    # T1 离开 blocked，但 rc0_evidence 没涨（纯字段翻动 pending/skipped），指纹也无共享变更
    base = {"done": 1, "blocked_count": 1, "blocked_fp": {"T1": "aa"},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    curr = {"done": 1, "blocked_count": 0, "blocked_fp": {},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    moved, why = pg.has_progressed(base, curr)
    ok(not moved, f"blocked↓ 无新证据 → 不认推进（防纯翻动博弈）：{why}")


def test_stall_same_fingerprint():
    print("\n=== has_progressed: 同 blocked 同失败指纹 = stall ===")
    base = {"done": 1, "blocked_count": 1, "blocked_fp": {"T1": "aa"},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    curr = {**base}  # 一模一样：同 rc 同 stderr 指纹
    moved, why = pg.has_progressed(base, curr)
    ok(not moved and "stalled" in why, f"同指纹空转 = 未推进：{why}")


def test_fingerprint_changed_is_progress():
    print("\n=== has_progressed: 同 blocked 指纹变 = 跑了新命令出新错 = 推进 ===")
    base = {"done": 1, "blocked_count": 1, "blocked_fp": {"T1": "aa"},
            "rc0_evidence": {"T1": 0}, "ledger_blockers": 0, "verified_risks": 0}
    curr = {**base, "blocked_fp": {"T1": "bb"}}
    moved, why = pg.has_progressed(base, curr)
    ok(moved and "failure changed" in why, f"指纹变 = 推进：{why}")


# --------------------------------------------------------------------------- #
# update_high_water（单向棘轮：digest 回退不被接受）
# --------------------------------------------------------------------------- #
def test_high_water_ratchet_up():
    print("\n=== update_high_water: 升高被记录 ===")
    state = {}
    hw = pg.update_high_water(state, {"done": 3, "verified_risks": 1})
    ok(hw["done"] == 3 and hw["verified_risks"] == 1, "首次写入棘轮值")
    ok(state["gates"]["progress_high_water"]["done"] == 3, "高水位写进 state.gates")


def test_high_water_ratchet_no_regress():
    print("\n=== update_high_water: digest 回退不被接受（单向棘轮）===")
    state = {}
    pg.update_high_water(state, {"done": 5, "verified_risks": 2})
    # 瞬时翻动：done 回退到 1，verified_risks 回退到 0
    hw = pg.update_high_water(state, {"done": 1, "verified_risks": 0})
    ok(hw["done"] == 5, f"done 回退 5→1 棘轮仍保 5（反复 done→undone 骗不过）→ {hw['done']}")
    ok(hw["verified_risks"] == 2, f"verified_risks 回退不被接受 → {hw['verified_risks']}")
    # 再升过历史高点才更新
    hw2 = pg.update_high_water(state, {"done": 6, "verified_risks": 2})
    ok(hw2["done"] == 6, "升过历史高点 → 棘轮更新到 6")


def test_high_water_preexisting_gate():
    print("\n=== update_high_water: 已有 gates 不被覆盖 ===")
    state = {"gates": {"ledger_blockers": 9, "progress_high_water": {"done": 4, "verified_risks": 3}}}
    hw = pg.update_high_water(state, {"done": 4, "verified_risks": 1})
    ok(hw["done"] == 4, "等于历史高点不降")
    ok(hw["verified_risks"] == 3, "verified_risks 已有更高值不被新 digest 拉低")
    ok(state["gates"]["ledger_blockers"] == 9, "update_high_water 不动 gates 其他 key")


# --------------------------------------------------------------------------- #
# 端到端：digest → has_progressed → high_water 串起来
# --------------------------------------------------------------------------- #
def test_end_to_end_stall_loop():
    print("\n=== 端到端: 同一坏命令反复失败 → 连续判 stall ===")
    tmp = Path(tempfile.mkdtemp(prefix="prog_"))
    try:
        repo = mkrepo(tmp)
        art = write_stderr_artifact(repo, "out/e2e.stderr", "Traceback\nValueError: stuck\n")
        state = {
            "tasks": [{"id": "T1", "status": "blocked",
                       "evidence": [{"rc": 1, "stderr_artifact": art}]}],
        }
        prev = pg.progress_digest(state, repo)
        # 第二步：state 没变（同 rc 同 stderr） → digest 一致 → stall
        curr = pg.progress_digest(state, repo)
        moved, why = pg.has_progressed(prev, curr)
        ok(not moved, f"重复同失败 → stall：{why}")
        # 棘轮 done 始终 0（无真进展）
        hw = pg.update_high_water(state, curr)
        ok(hw["done"] == 0, "无真进展 → 棘轮 done 仍 0")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    test_digest_basic()
    test_digest_ledger_absent_defaults_zero()
    test_fingerprint_reads_artifact_file()
    test_progressed_first_step()
    test_progressed_done_up()
    test_progressed_ledger_down()
    test_progressed_risk_verified()
    test_progressed_blocked_down_with_evidence()
    test_blocked_down_no_evidence_not_progress()
    test_stall_same_fingerprint()
    test_fingerprint_changed_is_progress()
    test_high_water_ratchet_up()
    test_high_water_ratchet_no_regress()
    test_high_water_preexisting_gate()
    test_end_to_end_stall_loop()
    print()
    if FAIL:
        print(f"{len(FAIL)} FINDINGS:", file=sys.stderr)
        for f in FAIL:
            print("  - " + f, file=sys.stderr)
        sys.exit(1)
    print("PROGRESS TESTS: progress.py 全部断言通过")
    sys.exit(0)
