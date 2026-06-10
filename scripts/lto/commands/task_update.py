"""lto task-update — 更新一个 task 的状态/证据，不 spawn subprocess。

pi 实测反馈（2026-06-10）：`lto runner` 会真实执行命令，agent 想标记"我已经
做完这步"只能 `runner --command true` 滥用语义——结果 runner log 里全是
`PASS(rc=0)`，无法区分真执行和假记录。

task-update 补上这个缺口：它只改 state.json 里 task 的 status / phase /
evidence / touched_files，绝不起子进程。语义清晰——"记录一个已完成事实"和
"执行并验证一条命令"（runner 的职责）从此分开。底层 state.update_task() 早已
存在，这里只补 CLI 入口。
"""

from __future__ import annotations

import argparse
from pathlib import Path

from .. import state as st
from .. import safe_emit


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    task = next((t for t in state.get("tasks", []) if t.get("id") == args.task_id), None)
    if task is None:
        raise SystemExit(f"no such task: {args.task_id}")

    # 必须至少改一个字段，否则是 no-op（防止 agent 以为更新了其实没有）
    if not any([args.status, args.phase, args.note, args.touch]):
        raise SystemExit(
            "task-update is a no-op: pass at least one of "
            "--status / --phase / --note / --touch"
        )

    changes: list[str] = []

    if args.status:
        if args.status not in st.VALID_TASK_STATUSES:
            raise SystemExit(
                f"invalid status: {args.status!r} "
                f"(valid: {sorted(st.VALID_TASK_STATUSES)})"
            )
        task["status"] = args.status
        changes.append(f"status={args.status}")

    if args.phase:
        if args.phase not in st.VALID_PHASES:
            raise SystemExit(
                f"invalid phase: {args.phase!r} (valid: {sorted(st.VALID_PHASES)})"
            )
        task["phase"] = args.phase
        changes.append(f"phase={args.phase}")

    if args.note:
        # Evidence entry tagged manual so it's never confused with a runner's
        # executed-command evidence (kind=manual, no rc — it's a recorded fact).
        task.setdefault("evidence", []).append({
            "kind": "manual",
            "summary": args.note,
            "recorded_at": st.iso_now(),
        })
        changes.append("note")

    if args.touch:
        touched = task.setdefault("touched_files", [])
        for f in args.touch:
            if f not in touched:
                touched.append(f)
        changes.append(f"touched+{len(args.touch)}")

    task["last_update"] = st.iso_now()
    st.save_state(state_path, state)

    # Only emit the status_changed event when status actually changed — the
    # event taxonomy has no generic "task.updated", and note/touch-only edits
    # aren't status transitions.
    if args.status:
        safe_emit(
            repo, run_id, type="task.status_changed", actor_kind="host",
            phase=task.get("phase", state.get("current_phase")),
            task_id=args.task_id, object_id=args.task_id,
            object_type="task", summary=", ".join(changes),
        )
    print(f"task {args.task_id} updated: {', '.join(changes)}")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "task-update",
        help="update a task's status/phase/evidence WITHOUT running a subprocess",
    )
    p.add_argument("--run-id")
    p.add_argument("--task-id", required=True, help="task id, e.g. T1")
    p.add_argument(
        "--status",
        help="new status: pending|in_progress|blocked|done|skipped",
    )
    p.add_argument("--phase", help="move task to a different phase")
    p.add_argument(
        "--note",
        help="record a manual evidence note (a completed fact, not an executed command)",
    )
    p.add_argument(
        "--touch",
        action="append",
        metavar="PATH",
        help="add a touched file (repeatable)",
    )
    p.set_defaults(func=run)
