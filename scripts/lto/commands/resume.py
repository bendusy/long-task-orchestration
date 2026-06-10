"""lto resume — 跨 session 断点热启动。"""

from __future__ import annotations

import argparse, sys
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import artifacts as af


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()

    # Resolve run_id
    current_file = repo / ".lto" / "current"
    if args.run_id:
        run_id = st.validate_run_id(args.run_id)
    elif current_file.exists():
        run_id = current_file.read_text(encoding="utf-8").strip()
        if run_id:
            run_id = st.validate_run_id(run_id)
        else:
            print("LTO: no active run (empty .lto/current)", file=sys.stderr)
            return 1
    else:
        print("LTO: no active run (missing .lto/current)", file=sys.stderr)
        return 1

    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        print(f"LTO: no state.json found for {run_id}", file=sys.stderr)
        return 1
    is_closed = state.get("current_phase") == "closed"

    # Validate workspace
    ws = state.get("workspace", {})
    recorded_head = ws.get("head", "unknown")
    actual_head = gs.git_head(repo)
    actual_branch = gs.git_branch(repo)
    drift = gs.head_drift(repo, recorded_head)
    dirty = gs.git_dirty(repo)

    # Determine resume status
    revalidate_tasks = []
    warnings = []

    if drift == "none":
        pass
    elif drift == "forward":
        # HEAD advanced — check if related files changed
        file_drift = gs.task_file_drift(repo, recorded_head, actual_head, state)
        if file_drift["changed_paths"]:
            revalidate_tasks = [t["id"] for t in state.get("tasks", []) if t["status"] in ("in_progress", "done")]
            sample = ", ".join(file_drift["changed_paths"][:6])
            warnings.append(
                f"HEAD advanced ({recorded_head[:8]}→{actual_head[:8]}), "
                f"related files changed: {sample}"
            )
        elif file_drift["missing_touched_files"]:
            warnings.append(
                f"HEAD advanced ({recorded_head[:8]}→{actual_head[:8]}), "
                "no task touched_files recorded; file drift precision unavailable"
            )
        else:
            warnings.append(f"HEAD advanced ({recorded_head[:8]}→{actual_head[:8]}), no related file changes")
    elif drift == "rewrite":
        revalidate_tasks = [t["id"] for t in state.get("tasks", []) if t["status"] in ("in_progress", "done")]
        warnings.append(f"HEAD rewritten ({recorded_head[:8]} not ancestor of {actual_head[:8]})")
    elif drift == "unreachable":
        warnings.append(f"recorded HEAD {recorded_head[:8]} unreachable")
        revalidate_tasks = [t["id"] for t in state.get("tasks", []) if t["status"] != "pending"]

    if dirty:
        warnings.append("worktree has uncommitted changes outside .lto")

    if is_closed:
        if revalidate_tasks:
            warnings.append(
                "run is closed; resume is read-only and will not reopen tasks "
                "or update recorded HEAD"
            )
            revalidate_tasks = []
        _print_capsule(repo, run_id, state, warnings, revalidate_tasks)
        return 0

    # Update state
    ws["head"] = actual_head
    ws["branch"] = actual_branch
    ws["dirty_fingerprint"] = "dirty" if dirty else "clean"
    state["workspace"] = ws

    # Mark revalidate
    for task_id in revalidate_tasks:
        try:
            st.update_task(state, task_id, status="pending", blockers=[{
                "reason": "requires_revalidate",
                "recorded_head": recorded_head,
                "current_head": actual_head,
            }])
        except KeyError:
            pass

    # Save updated state
    st.save_state(state_path, state)
    st.sync_run_state_md(repo / ".lto" / run_id / "run-state.md", state)
    af.load_manifest(repo, run_id, state=state, persist_synthesized=True)

    # Print context capsule
    _print_capsule(repo, run_id, state, warnings, revalidate_tasks)

    return 0 if not revalidate_tasks else 2


def _print_capsule(repo: Path, run_id: str, state: dict, warnings: list[str], revalidate: list[str]) -> None:
    phase = state.get("current_phase", "unknown")
    ws = state.get("workspace", {})
    head = ws.get("head", "unknown")

    tasks = state.get("tasks", [])
    task_summary = ", ".join(
        f"{t['id']}:{t['status']}" for t in tasks[-5:]
    ) if tasks else "none"

    last_failure = state.get("last_failure", "")
    next_action = state.get("next_action", "none")
    blocked = state.get("blocked_by", "none")
    unresolved = state.get("gates", {}).get("unresolved_blocks", [])

    print("=== LTO ACTIVE SESSION ===")
    print(f"Run ID: {state.get('run_id', '?')}")
    print(f"Goal: {state.get('goal', '?')}")
    print(f"Phase: {phase}")
    print(f"Head: {head[:12]} ({ws.get('dirty_fingerprint', '?')})")
    print(f"Tasks: {task_summary}")
    if last_failure:
        print(f"Last Failure: {last_failure[:120]}")
    print(f"Next: {next_action[:120]}")
    if blocked and blocked != "none":
        print(f"Blocked: {blocked}")
    if unresolved:
        print(f"Unresolved Blocks: {len(unresolved)}")

    entries = af.recent(repo, run_id, limit=6)
    if entries:
        print("Recent Artifacts:")
        for entry in entries:
            summary = entry.get("summary") or ""
            suffix = f" — {summary[:80]}" if summary else ""
            source = " (synthesized)" if entry.get("source") == "synthesized" else ""
            print(f"  - {entry.get('kind')}: {entry.get('relative_path')}{suffix}{source}")

    if warnings:
        print(f"\nWarnings:")
        for w in warnings:
            print(f"  ⚠ {w}")

    if revalidate:
        print(f"\n⚠ {len(revalidate)} tasks require revalidation: {', '.join(revalidate[:5])}")

    # 感知面：跨 session 回来的第一个命令必须重注入 affordance 事实——
    # SKILL.md 早出 context 窗口，插件不在这里可见就是死数据。零推荐，匹配归 host。
    try:
        from .. import plugins as plg
        aff = plg.affordance_facts(repo, run_id)
        if aff["available"]:
            mounted = set(aff["mounted"])
            names = ", ".join(
                f"{p['id']}{' (mounted)' if p['id'] in mounted else ''}"
                for p in aff["available"]
            )
            print(f"Plugins: {names}")
            print("  → 任务形态先验见 references/workflow-playbook.md；"
                  "细节 `lto plugin list`；挂载 `lto plugin mount <dir> --run-id <id>`")
    except Exception:
        pass  # 感知层绝不弄崩 capsule

    # 长 gap 提醒：人类侧 goal drift。距上次活动 >24h 时，提示给用户跑 recap。
    # capsule 本身是给 AI 的（git head/dirty），但隔了很久回来的人可能忘了在做什么。
    gap_hours = _max_session_gap_hours(state)
    if gap_hours >= 24:
        d = int(gap_hours / 24)
        print(f"\n⏳ 距上次活动约 {int(gap_hours)} 小时（{d} 天）。"
              f"建议先给用户跑 `lto recap`——隔这么久，人可能忘了在做什么、为什么。")

    print("===========================")


def _max_session_gap_hours(state: dict) -> float:
    """phase_transitions 相邻时间戳的最大间隔（小时）——近似 session gap。"""
    from datetime import datetime
    transitions = state.get("phase_transitions", [])
    prev = None
    max_gap = 0.0
    for tr in transitions:
        at = tr.get("at", "")
        if not at:
            continue
        try:
            dt = datetime.fromisoformat(at)
        except ValueError:
            continue
        if prev is not None:
            max_gap = max(max_gap, (dt - prev).total_seconds() / 3600)
        prev = dt
    return max_gap


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("resume", help="resume from last checkpoint")
    p.add_argument("--run-id")
    p.set_defaults(func=run)
