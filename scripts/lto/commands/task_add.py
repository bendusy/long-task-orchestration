"""lto task-add — 给当前 run 加一个 task。

三方实测（codex/pi/agy，2026-06-03）一致发现的断链：LTO 原有 15 命令却没有
"加 task"的 CLI 入口——task 是 runner/next/audit 的操作对象，但创建只能手动
调 Python state.add_task()。陌生 agent 读完 onboarding 第二步就撞墙。本命令补上。
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

    # task-id 唯一性检查（重复会让 runner/next 选错对象）
    existing = {t.get("id") for t in state.get("tasks", [])}
    if args.task_id in existing:
        raise SystemExit(f"task id already exists: {args.task_id}")

    phase = args.phase or state.get("current_phase", "implementation")
    st.add_task(state, args.task_id, args.title, phase)

    # 可选：记下该 task 计划跑的命令（runner/autopilot 会用 commands_run）
    if args.command:
        for t in state["tasks"]:
            if t["id"] == args.task_id:
                t["commands_run"].append(args.command)
                break

    st.save_state(state_path, state)
    safe_emit(
        repo, run_id, type="task.created", actor_kind="host",
        phase=phase, task_id=args.task_id, object_id=args.task_id,
        object_type="task", summary=args.title,
    )
    print(f"task {args.task_id} added to phase '{phase}': {args.title}")
    if args.command:
        print(f"  planned command: {args.command}")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("task-add", help="add a task to the current run")
    p.add_argument("--run-id")
    p.add_argument("--task-id", required=True, help="task id, e.g. T1")
    p.add_argument("--title", required=True, help="short task title")
    p.add_argument("--phase", help="phase (default: current phase)")
    p.add_argument("--command", help="optional: command this task will run (runner/autopilot use it)")
    p.set_defaults(func=run)
