"""lto runs — 列出本项目 .lto/ 下所有 LTO run 的概览。

为什么需要它（用户 2026-06-10）：am（animem）没装时，`.lto/` 目录**就是**这个
项目的本地记忆层——每个 run 一个目录，记着干过什么、到哪个阶段、留下什么证据。
但在此命令之前，没有任何入口引导一个**刚进项目的 agent**去看这段历史：`resume`
只看 current run，`recap --mine` 要 opt-in 且 agent 不知道该跑。结果新 agent
对"这项目以前用 LTO 做过什么"一无所知。

`lto runs` 补上这个入口：一眼列出所有 run（目标 / 阶段 / 任务进度 / 时间），
让 agent 进项目第一件事就能了解 LTO 的使用历史。它是 am 缺席时的本地"记忆索引"。
纯只读，零依赖。
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

from .. import state as st
from ..cross_run_mining import _iter_runs
from ..heartbeat import scan_live_heartbeats, format_watch_table


def _summarize_run(repo: Path, run_id: str) -> dict:
    """Pull a one-line summary from a run's state.json (best-effort)."""
    state_path = repo / ".lto" / run_id / "state.json"
    summary = {
        "run_id": run_id,
        "goal": "",
        "phase": "?",
        "tasks_done": 0,
        "tasks_total": 0,
        "closed": False,
    }
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, ValueError):
        summary["goal"] = "(unreadable state.json)"
        return summary
    summary["goal"] = str(state.get("goal", "") or "")
    summary["phase"] = str(state.get("current_phase", "?") or "?")
    tasks = state.get("tasks", []) or []
    summary["tasks_total"] = len(tasks)
    summary["tasks_done"] = sum(1 for t in tasks if t.get("status") == "done")
    summary["closed"] = state.get("current_phase") == "closed"
    return summary


def _render_watch(repo: Path, run_id: str | None) -> None:
    """Render one snapshot of live-job heartbeats (P0-1 层 1 汇总入口)."""
    rows = scan_live_heartbeats(repo, run_id, now=time.time())
    print(f"# LTO live jobs ({len(rows)} with heartbeats) — {time.strftime('%H:%M:%S')}")
    print(format_watch_table(rows))


def _run_watch(args: argparse.Namespace) -> int:
    """`lto runs --watch`: one-line summary of all running jobs' heartbeats.

    host 一条命令看全所有在跑 job（runner / 已跑多久 / 最后心跳距今 / reply 就绪），
    不必挨个 poll。--once 出单帧（脚本/测试友好）；否则按心跳间隔轮询刷新。
    """
    repo = args.repo.resolve()
    run_id = getattr(args, "run_id", None)
    if getattr(args, "once", False):
        _render_watch(repo, run_id)
        return 0

    interval = max(1.0, float(getattr(args, "interval", 30.0) or 30.0))
    try:
        while True:
            _render_watch(repo, run_id)
            time.sleep(interval)
    except KeyboardInterrupt:
        return 0


def run(args: argparse.Namespace) -> int:
    if getattr(args, "watch", False):
        return _run_watch(args)

    repo = args.repo.resolve()
    lto_root = repo / ".lto"
    if not lto_root.is_dir():
        print("no .lto/ directory — this project hasn't run LTO yet.")
        return 0

    # _iter_runs validates the *name* (path-traversal guard), not whether the
    # dir is a real run. A real run always has state.json — filter on that so
    # ad-hoc dirs under .lto/ (scratch reply dirs, etc.) don't show as noise.
    run_ids = [rid for rid in _iter_runs(repo)
               if (lto_root / rid / "state.json").is_file()]
    if not run_ids:
        print(".lto/ exists but has no runs yet.")
        return 0

    # current run (if any) so we can mark it
    current = ""
    current_file = lto_root / "current"
    if current_file.exists():
        try:
            current = current_file.read_text(encoding="utf-8").strip()
        except OSError:
            current = ""

    summaries = [_summarize_run(repo, rid) for rid in run_ids]

    if args.json:
        print(json.dumps({"runs": summaries, "current": current, "count": len(summaries)},
                         ensure_ascii=False, indent=2, sort_keys=True))
        return 0

    print(f"# LTO runs in this project ({len(summaries)} total)")
    print("# This .lto/ directory IS the project's local memory when am/ANIMEM")
    print("# isn't installed. Read it to understand what LTO has done here.\n")
    # newest first (run_ids are chronological → reverse)
    for s in reversed(summaries):
        mark = " ←current" if s["run_id"] == current else ""
        status = "closed" if s["closed"] else s["phase"]
        prog = f"{s['tasks_done']}/{s['tasks_total']}" if s["tasks_total"] else "-"
        goal = s["goal"][:60] or "(no goal)"
        print(f"  [{status:<14}] {prog:>5} tasks · {goal}{mark}")
        print(f"                  {s['run_id']}")
    print("\nInspect one: `lto resume` (current) or read .lto/<run-id>/{state.json,handoff.md,run-state.md}")
    print("Cross-run patterns (which model/phase works): `lto recap --mine`")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "runs",
        help="list all LTO runs in this project (.lto/ is the local memory when am isn't installed)",
    )
    p.add_argument("--json", action="store_true")
    p.add_argument(
        "--watch", action="store_true",
        help="summarize all running jobs' live heartbeats (runner / elapsed / last-hb / reply)",
    )
    p.add_argument(
        "--once", action="store_true",
        help="with --watch: render a single snapshot then exit (default: poll every --interval)",
    )
    p.add_argument(
        "--interval", type=float, default=30.0,
        help="with --watch: poll interval in seconds (default 30)",
    )
    p.add_argument(
        "--run-id", dest="run_id", default=None,
        help="with --watch: target run id (default: .lto/current)",
    )
    p.set_defaults(func=run)
