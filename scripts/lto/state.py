"""LTO 状态管理：state.json 读写 + run-state.md 双向同步。

真源规则：state.json 是机器真源，run-state.md 是人类可读渲染。
每次写入 state.json 时同步更新 run-state.md 的机器字段。
人类字段（Decision Slugs、Evidence Snapshot 文本描述）只在 run-state.md 中。
"""

from __future__ import annotations

import json, re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1

VALID_PHASES = {"intake", "spec", "audit", "implementation", "deploy", "observe", "closed"}
VALID_TASK_STATUSES = {"pending", "in_progress", "blocked", "done", "skipped"}
VALID_EVIDENCE_KINDS = {"test", "lint", "build", "manual", "review", "deploy"}

# run-state.md fields that state.json owns (machine sync)
CORE_RUN_STATE_KEYS = (
    "run_id",
    "feature / goal",
    "started_at",
    "host_runtime",
    "repo",
    "initial_user_request",
    "current_phase",
    "current_git_head",
    "current_branch",
)


def iso_now() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def single_line(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip()


def validate_run_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}", value):
        raise SystemExit(f"invalid run id: {value!r}")
    if value in {".", ".."} or ".." in value:
        raise SystemExit(f"invalid run id: {value!r}")
    return value


def resolve_run_id(repo: Path, run_id: str | None) -> str:
    """解析 run id：显式 --run-id 优先，否则读 .lto/current。

    被 9 个 command（autopilot/check/judge/runner/parallel/recap/task_add/
    pipeline/audit）共用——消除原本逐字节相同的 9 处 _resolve 复制。
    """
    if run_id:
        return validate_run_id(run_id)
    current = repo / ".lto" / "current"
    if current.exists():
        value = current.read_text(encoding="utf-8").strip()
        if value:
            return validate_run_id(value)
    raise SystemExit("missing --run-id and .lto/current")


def _replace_field(content: str, field: str, value: str) -> str:
    pattern = re.compile(rf"^- {re.escape(field)}:.*$", re.MULTILINE)
    replacement = f"- {field}: {single_line(value)}"
    if pattern.search(content):
        return pattern.sub(lambda _: replacement, content, count=1)
    return content


def _read_field(content: str, field: str) -> str:
    match = re.search(rf"^- {re.escape(field)}:[ \t]*(.*)$", content, re.MULTILINE)
    return match.group(1).strip() if match else ""


def default_state(goal: str, host: str, repo: str, request: str, phase: str,
                  head: str, branch: str, auditors: str, timeout: str,
                  why: str = "", done_when: str = "",
                  max_turns: int | None = None, max_tokens: int | None = None,
                  hard_deadline: str | None = None) -> dict[str, Any]:
    return {
        "schema_version": SCHEMA_VERSION,
        "run_id": "",
        "goal": goal,
        # 面向人类回顾（lto recap）：why=为什么做这件事，done_when=判断做完的标准。
        # 长任务跨 session 后，人会忘了当初为什么开这个 run——这两个字段是答案。
        "why": why,
        "done_when": done_when,
        "started_at": iso_now(),
        "host_runtime": host,
        "workspace": {
            "repo_root": repo,
            "branch": branch,
            "head": head,
            "dirty_fingerprint": "clean",
        },
        "environment_snapshot": {
            "sandbox": "unknown",
            "network": "unknown",
            "mcp_services": [],
            "write_roots": [],
            "captured_at": iso_now(),
        },
        "current_phase": phase,
        "original_user_request": request or goal,
        "phase_transitions": [],
        "tasks": [],
        "active_task_id": None,
        "risk_points": [],
        # 运行时累积区（autopilot/decision/agent_exec 写入）。预置初值使其进入
        # schema 真源，而非靠各模块 setdefault 隐式建——读取默认行为与原先等价。
        "agent_runs": {},
        "decision_escalate_points": {},
        "gates": {
            "last_tested_head": None,
            "last_reviewed_head": None,
            "unresolved_blocks": [],
            "exit_criteria": {},
            "autopilot_last_digest": {},
            "progress_high_water": {"done": 0, "verified_risks": 0},
        },
        # Run 级预算契约（全可选，缺省 None = 无限 → 老 run 零破坏）。
        # turns_used 只数 autopilot 自动推进调用，人手动操作不计。分级刹车：
        # warn_ratio 软警告（next/recap 事实层）→ 100% 硬刹车（autopilot fail-closed）。
        "budget": {
            "max_turns": max_turns,
            "max_tokens": max_tokens,
            "hard_deadline": hard_deadline,
            "turns_used": 0,
            "warn_ratio": 0.8,
        },
        "last_failure": None,
        "user_decisions": [],
        "next_action": "",
        "blocked_by": "none",
        "artifacts": {
            "manifest": ".lto/<run-id>/artifacts.json",
        },
    }


def load_state(state_path: Path) -> dict[str, Any] | None:
    """加载 state.json。不存在返回 None，损坏抛异常。"""
    if not state_path.exists():
        return None
    with open(state_path, encoding="utf-8") as f:
        return json.load(f)


def save_state(state_path: Path, state: dict[str, Any]) -> None:
    """写 state.json 到磁盘。"""
    state_path.parent.mkdir(parents=True, exist_ok=True)
    with open(state_path, "w", encoding="utf-8") as f:
        json.dump(state, f, indent=2, ensure_ascii=False)
        f.write("\n")


def sync_run_state_md(run_state_path: Path, state: dict[str, Any]) -> None:
    """将 state.json 的机器字段同步到 run-state.md。

    只在 run-state.md 已存在时更新；不创建新文件（由 start 命令负责）。
    """
    if not run_state_path.exists():
        return

    content = run_state_path.read_text(encoding="utf-8")

    # Sync identity fields
    ws = state.get("workspace", {})
    field_map = {
        "run_id": state.get("run_id", ""),
        "feature / goal": state.get("goal", ""),
        "started_at": state.get("started_at", ""),
        "host_runtime": state.get("host_runtime", ""),
        "repo": ws.get("repo_root", ""),
        "initial_user_request": state.get("original_user_request", ""),
        "current_phase": state.get("current_phase", ""),
        "current_git_head": ws.get("head", ""),
        "current_branch": ws.get("branch", ""),
        "next_command_or_question": state.get("next_action", ""),
        "blocked_by": state.get("blocked_by", "none"),
    }

    for field, value in field_map.items():
        if value:
            content = _replace_field(content, field, str(value))

    run_state_path.write_text(content, encoding="utf-8")


def transition_phase(state: dict[str, Any], to_phase: str, head: str) -> dict[str, Any]:
    """记录阶段切换。"""
    if to_phase not in VALID_PHASES:
        raise ValueError(f"invalid phase: {to_phase}")

    old_phase = state["current_phase"]
    state["phase_transitions"].append({
        "from": old_phase,
        "to": to_phase,
        "at": iso_now(),
        "head": head,
    })
    state["current_phase"] = to_phase
    return state


def add_task(state: dict[str, Any], task_id: str, title: str, phase: str | None = None) -> dict[str, Any]:
    """追加 task。"""
    state["tasks"].append({
        "id": task_id,
        "title": title,
        "status": "pending",
        "phase": phase or state["current_phase"],
        "depends_on": [],
        "last_update": iso_now(),
        "touched_files": [],
        "commands_run": [],
        "evidence": [],
        "blockers": [],
        "assumptions": [],
        "retry_count": 0,
    })
    return state


def update_task(state: dict[str, Any], task_id: str, **kwargs) -> dict[str, Any]:
    """更新指定 task 的字段。"""
    for task in state["tasks"]:
        if task["id"] == task_id:
            task.update(kwargs)
            task["last_update"] = iso_now()
            return state
    raise KeyError(f"task not found: {task_id}")


def set_active_task(state: dict[str, Any], task_id: str | None) -> dict[str, Any]:
    state["active_task_id"] = task_id
    return state


def add_risk_point(
    state: dict[str, Any],
    rp_id: str,
    source: str,
    claim: str,
    evidence_to_check: str,
) -> dict[str, Any]:
    """Register a risk point for adversarial audit coverage tracking.

    Each risk point records a claim about a potential issue that must be
    verified before closeout.  Unverified risk points with disposition="open"
    block closeout (see closeout.py risk-coverage gate).

    Args:
        state: state dict
        rp_id: unique risk point ID (e.g. "RP1")
        source: origin — "diff" | "static" | "risk-agent" | "prior-blocker" | "human"
        claim: natural language description of the risk
        evidence_to_check: file path or description of what to verify
    """
    state.setdefault("risk_points", []).append({
        "id": rp_id,
        "source": source,
        "claim": claim,
        "evidence_to_check": evidence_to_check,
        "verified_by": "",
        "disposition": "open",
    })
    return state


def mark_risk_verified(
    state: dict[str, Any],
    rp_id: str,
    auditor: str,
) -> dict[str, Any]:
    """Mark a risk point as verified by an auditor.

    Args:
        state: state dict
        rp_id: risk point ID to mark
        auditor: name of the auditor / audit round that verified it (e.g. "codex-R1")

    Raises:
        KeyError: if rp_id is not found in state["risk_points"]
    """
    for rp in state.get("risk_points", []):
        if rp["id"] == rp_id:
            rp["verified_by"] = auditor
            rp["disposition"] = "verified"
            return state
    raise KeyError(f"risk point not found: {rp_id!r}")


def token_rollup(state: dict[str, Any]) -> dict[str, Any]:
    """Aggregate per-run token usage across all persisted agent_runs.

    Each agent_runs[job_id] is a list of AgentResult dicts; each may carry
    cost.tokens / cost.tokens_in / cost.tokens_out (written by a runner token
    sidecar — codex/pi/claude provide it, agy does not). Returns:

        {
          "total_tokens": int, "tokens_in": int, "tokens_out": int,
          "runs_with_tokens": int,   # runner results that reported tokens
          "runs_total": int,         # total runner results seen
          "by_runner": {runner: {tokens, runs_with_tokens, runs_total}},
        }

    Token-less results (agy, or runs before sidecar support) count toward
    runs_total but not runs_with_tokens — so the consumer can honestly say
    "N of M runs reported tokens" instead of pretending coverage is complete.
    """
    total = tin = tout = 0
    with_tokens = total_runs = 0
    total_elapsed = 0.0   # 所有派工的累计执行耗时（秒），与 token 一起报"这次 run 烧了多少"
    by_runner: dict[str, dict[str, int]] = {}

    def _int(val: Any) -> int:
        return val if isinstance(val, int) and not isinstance(val, bool) and val >= 0 else 0

    for results in (state.get("agent_runs") or {}).values():
        if not isinstance(results, list):
            continue
        for r in results:
            if not isinstance(r, dict):
                continue
            total_runs += 1
            runner = str(r.get("runner", "?"))
            slot = by_runner.setdefault(runner, {"tokens": 0, "runs_with_tokens": 0, "runs_total": 0})
            slot["runs_total"] += 1
            cost = r.get("cost") or {}
            el = cost.get("elapsed_sec")
            if isinstance(el, (int, float)) and not isinstance(el, bool) and el >= 0:
                total_elapsed += float(el)
            tok = _int(cost.get("tokens"))
            ti = _int(cost.get("tokens_in"))
            to = _int(cost.get("tokens_out"))
            # tokens may be absent while in/out present → fall back to their sum
            if tok == 0 and (ti or to):
                tok = ti + to
            if tok > 0:
                total += tok
                tin += ti
                tout += to
                with_tokens += 1
                slot["tokens"] += tok
                slot["runs_with_tokens"] += 1

    return {
        "total_tokens": total,
        "tokens_in": tin,
        "tokens_out": tout,
        "runs_with_tokens": with_tokens,
        "runs_total": total_runs,
        "total_elapsed_sec": round(total_elapsed, 1),
        "by_runner": by_runner,
    }
