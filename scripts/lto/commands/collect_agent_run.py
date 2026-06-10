"""lto collect-agent-run — 把 delegate.sh 派工的产物登记进 state.agent_runs。

pi 实测反馈（2026-06-10）：用 `delegate.sh` 成功派了 codex/pi，但 LTO 完全
不知道——`agent_runs` 为空、`recap` 不显示 token、sidecar 落盘了 LTO 不读。

根因（见 references/agent-runs-decoupling-diagnosis.md）：LTO 有两条平行派工
路径，只有 Python 侧 `agent_exec.spawn_agents(persist=True)` 写 agent_runs；
`delegate.sh` 是独立 shell 路径，不经过 agent_exec，产物（reply +
`.meta.json` token sidecar）只落文件系统，state 层一无所知。

本命令是「方案 A」的轻量桥：事后把一次 delegate 派工的产物收集成一条
AgentResult 追加进 `state["agent_runs"][task_id]`，对齐 token_rollup 读的
`cost.{tokens,tokens_in,tokens_out,elapsed_sec}`，于是 recap/closeout/
cross-run-mining 都能看见这次派工。它**不** spawn 任何进程——和 task-update
同构，只登记已发生的事实。runner 接 agent 的边界不动。
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from .. import state as st
from .. import safe_emit
from ..agent_job import AgentResult, JobStatus, KNOWN_RUNNERS


def _load_sidecar(path: Path) -> dict:
    """Best-effort read a token sidecar (.meta.json). Missing/garbage → {}."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, json.JSONDecodeError, ValueError):
        return {}


def _build_cost(meta: dict, elapsed_sec: float | None) -> dict:
    """Map sidecar fields to the cost dict token_rollup reads.

    Sidecar shape (pi/codex): {"tokens_in": int, "tokens_out": int, "tokens": int}.
    agy produces no sidecar → cost stays token-less (honestly unmetered).
    """
    cost: dict = {}
    for src, dst in (("tokens", "tokens"), ("tokens_in", "tokens_in"), ("tokens_out", "tokens_out")):
        v = meta.get(src)
        if isinstance(v, int) and not isinstance(v, bool) and v >= 0:
            cost[dst] = v
    if elapsed_sec is not None and elapsed_sec >= 0:
        cost["elapsed_sec"] = float(elapsed_sec)
    return cost


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    if args.runner not in KNOWN_RUNNERS:
        raise SystemExit(f"unknown runner: {args.runner!r} (known: {sorted(KNOWN_RUNNERS)})")

    task = next((t for t in state.get("tasks", []) if t.get("id") == args.task_id), None)
    if task is None:
        raise SystemExit(f"no such task: {args.task_id}")

    reply_path = Path(args.reply)
    if not reply_path.is_absolute():
        reply_path = (repo / reply_path).resolve()
    if not reply_path.exists():
        raise SystemExit(f"reply file not found: {reply_path}")
    reply_text = reply_path.read_text(encoding="utf-8", errors="replace")

    # sidecar: explicit --meta, else infer <reply>.meta.json next to the reply.
    meta_path = Path(args.meta) if args.meta else reply_path.with_name(reply_path.name + ".meta.json")
    if not meta_path.is_absolute():
        meta_path = (repo / meta_path).resolve()
    meta = _load_sidecar(meta_path) if meta_path.exists() else {}

    # status: explicit --status wins; else infer from empty reply.
    if args.status:
        status = args.status
    else:
        status = JobStatus.OK.value if reply_text.strip() else JobStatus.FAILED.value

    cost = _build_cost(meta, args.elapsed_sec)

    result = AgentResult(
        job_id=args.task_id,
        runner=args.runner,
        model=args.model,
        status=status,
        reply_text=reply_text,
        cost=cost,
        artifacts=[str(reply_path)],
        error="" if status == JobStatus.OK.value else (args.note or "empty or failed reply"),
    )

    agent_runs = state.setdefault("agent_runs", {})
    agent_runs.setdefault(args.task_id, []).append(result.to_dict())

    # also drop a manual evidence note on the task so the dispatch is visible
    # in the task's own evidence trail, not just the run-level agent_runs.
    tok = cost.get("tokens")
    tok_str = f", {tok} tokens" if tok else " (unmetered)"
    task.setdefault("evidence", []).append({
        "kind": "manual",
        "summary": f"collected {args.runner} dispatch{tok_str}"
        + (f": {args.note}" if args.note else ""),
        "recorded_at": st.iso_now(),
    })
    task["last_update"] = st.iso_now()

    st.save_state(state_path, state)
    safe_emit(
        repo, run_id, type="runner.finished", actor_kind="runner",
        phase=task.get("phase", state.get("current_phase")),
        task_id=args.task_id, object_id=args.task_id, object_type="task",
        summary=f"collected {args.runner} dispatch{tok_str}",
    )
    print(f"collected {args.runner} run for task {args.task_id}: status={status}{tok_str}")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "collect-agent-run",
        help="register a delegate.sh dispatch (reply + token sidecar) into state.agent_runs",
    )
    p.add_argument("--run-id")
    p.add_argument("--task-id", required=True, help="task this dispatch belongs to")
    p.add_argument("--runner", required=True, help="runner family: codex|pi|agy|claude|gemini")
    p.add_argument("--reply", required=True, help="path to the reply file delegate.sh wrote")
    p.add_argument("--meta", help="path to the token sidecar (default: <reply>.meta.json next to reply)")
    p.add_argument("--model", help="specific model id (optional, used by cross-run mining)")
    p.add_argument("--status", choices=["ok", "failed"], help="override status (default: inferred from reply)")
    p.add_argument("--elapsed-sec", type=float, help="optional dispatch wall-clock seconds")
    p.add_argument("--note", help="optional note recorded with the dispatch")
    p.set_defaults(func=run)
