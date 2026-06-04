"""lto runner — 单 task 执行 + 自动证据记录。"""

from __future__ import annotations

import argparse
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import evidence as ev
from .. import exec as lto_exec


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()

    # Resolve run
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    # Find task
    task = None
    for t in state.get("tasks", []):
        if t["id"] == args.task_id:
            task = t
            break
    if task is None:
        raise SystemExit(f"task {args.task_id} not found in state")

    cwd = Path(args.cwd) if args.cwd else repo

    # Execute via shared kernel (handles artifact + evidence + timeout)
    rc, evidence = lto_exec.run_command(
        repo, run_id, args.task_id,
        kind=args.kind, command=args.command, cwd=cwd, timeout=args.timeout,
        verified_by="runner",
        summary=args.note or "",
    )
    if not args.note:
        evidence["summary"] = f"{args.kind}: {'PASS' if rc == 0 else ('TIMEOUT' if rc == 124 else 'FAIL')}"

    # Update task
    task["evidence"].append(evidence)
    if args.touch:
        for f in args.touch:
            if f not in task["touched_files"]:
                task["touched_files"].append(f)

    ended_at = evidence.get("ended_at", st.iso_now())
    if rc == 0:
        task["status"] = "done"
        state["gates"]["last_tested_head"] = evidence.get("head_after", gs.git_head(repo))
    else:
        # G2: 失败时按 (command_fingerprint) 维度递增 retry_count，
        # 换条命令重试不会绕过失控刹车（同一坏命令反复跑才累加）。
        retries = _bump_retry(task, args.command)
        if rc == 124:
            task["status"] = "blocked"
            task["blockers"].append({"reason": f"timeout ({args.timeout}s)", "at": ended_at})
            state["last_failure"] = f"{args.task_id}: timeout (retry {retries})"
        else:
            task["status"] = args.status_on_fail or "blocked"
            task["blockers"].append({
                "reason": f"command failed (rc={rc})",
                "command": args.command,
                "evidence_kind": args.kind,
                "at": ended_at,
            })
            state["last_failure"] = f"{args.task_id}: {args.kind} rc={rc} (retry {retries})"

    task["commands_run"].append(args.command)
    task["last_update"] = ended_at
    st.save_state(state_path, state)

    # Optionally commit .lto state changes (opt-in; default off)
    gs.auto_commit_lto(
        repo,
        f"lto: {args.task_id} {args.kind} {'PASS' if rc == 0 else 'FAIL'}",
        enabled=args.auto_commit,
    )

    print(ev.evidence_summary(evidence))
    return rc


def _command_fingerprint(command: str) -> str:
    """命令指纹——同一命令归一化后 hash，换命令则指纹变。"""
    import hashlib
    normalized = " ".join(command.split())  # 折叠空白，避免格式差异误判
    return hashlib.sha1(normalized.encode("utf-8")).hexdigest()[:12]


def _bump_retry(task: dict, command: str) -> int:
    """按命令指纹递增失败计数；返回当前命令的累计失败次数。

    task['retry_by_command'][fp] 记每条命令各自的失败次数；
    task['retry_count'] 暴露当前命令的失败次数（autopilot 失控刹车读它）。
    """
    fp = _command_fingerprint(command)
    by_cmd = task.setdefault("retry_by_command", {})
    by_cmd[fp] = by_cmd.get(fp, 0) + 1
    task["retry_count"] = by_cmd[fp]
    return by_cmd[fp]


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("runner", help="execute task + record evidence")
    p.add_argument("--run-id")
    p.add_argument("--task-id", required=True)
    p.add_argument("--kind", default="test", choices=["test", "lint", "build", "manual", "review", "deploy"])
    p.add_argument("--command", required=True)
    p.add_argument("--cwd")
    p.add_argument("--timeout", type=int, default=300)
    p.add_argument("--touch", nargs="*", help="files touched by this task")
    p.add_argument("--note", help="human summary")
    p.add_argument("--status-on-fail", choices=["blocked", "in_progress"], default="blocked")
    p.add_argument("--auto-commit", action="store_true",
                   help="commit .lto state changes (opt-in; default off, uses repo git identity)")
    p.set_defaults(func=run)
