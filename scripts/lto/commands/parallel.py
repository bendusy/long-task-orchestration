"""lto parallel — 并发执行多个 task。"""

from __future__ import annotations

import argparse, concurrent.futures, threading, time
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import evidence as ev
from .. import exec as lto_exec


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    tasks = state.get("tasks", [])
    if args.task_ids:
        targets = [t for t in tasks if t["id"] in args.task_ids]
    elif args.phase:
        targets = [t for t in tasks if t.get("phase") == args.phase and t["status"] in ("pending", "in_progress")]
    else:
        targets = [t for t in tasks if t["status"] in ("pending", "in_progress")]

    if not targets:
        print("no tasks to run")
        return 0

    concurrency = min(args.concurrency, len(targets))
    print(f"◆ LTO Parallel: {len(targets)} tasks ({concurrency} concurrent)")

    results: dict[str, dict] = {}
    started = time.time()
    state_lock = threading.Lock()

    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = {
            executor.submit(_run_one, repo, run_id, task, args, state_lock): task["id"]
            for task in targets
        }

        for future in concurrent.futures.as_completed(futures):
            task_id = futures[future]
            try:
                result = future.result()
                results[task_id] = result
                status = "✓" if result.get("rc") == 0 else "✗"
                print(f"  {status} {task_id}: {result.get('summary', '')[:80]}")
            except Exception as e:
                results[task_id] = {"rc": 1, "summary": str(e)}
                _mark_task_failed(state_path, state_lock, task_id, str(e))
                print(f"  ✗ {task_id}: {e}")

    elapsed = time.time() - started
    passed = sum(1 for r in results.values() if r.get("rc") == 0)
    print(f"◆ {passed}/{len(targets)} passed ({elapsed:.1f}s)")

    # Each _run_one already persisted its own task update; just optionally commit.
    gs.auto_commit_lto(repo, f"lto: parallel {passed}/{len(targets)} passed", enabled=args.auto_commit)

    return 0 if passed == len(targets) else 1


def _run_one(repo: Path, run_id: str, task: dict, args: argparse.Namespace, lock) -> dict:
    cwd = Path(args.cwd) if args.cwd else repo
    state_path = repo / ".lto" / run_id / "state.json"
    task_id = task["id"]

    # Build command from task or args
    if task.get("commands_run"):
        command = task["commands_run"][-1]
    else:
        command = args.command or "echo 'no command'"

    # Execute via shared kernel (handles artifact + evidence + timeout)
    rc, evidence_entry = lto_exec.run_command(
        repo, run_id, task_id,
        kind=args.kind, command=command, cwd=cwd, timeout=args.timeout,
        verified_by="parallel",
        summary="",
    )
    evidence_entry["summary"] = (
        f"{'PASS' if rc == 0 else ('TIMEOUT' if rc == 124 else 'FAIL')}: "
        f"{task.get('title', task_id)}"
    )
    ended_at = evidence_entry.get("ended_at", st.iso_now())

    # Update task in state (thread-safe via lock + reload)
    with lock:
        state = st.load_state(state_path)
        if state:
            for t in state["tasks"]:
                if t["id"] == task_id:
                    t["evidence"].append(evidence_entry)
                    t["commands_run"].append(command)
                    if args.touch:
                        for f in args.touch:
                            if f not in t["touched_files"]:
                                t["touched_files"].append(f)
                    t["status"] = "done" if rc == 0 else "blocked"
                    if rc == 0 and args.kind == "test":
                        state["gates"]["last_tested_head"] = evidence_entry.get(
                            "head_after", gs.git_head(repo)
                        )
                    if rc != 0:
                        t["blockers"].append({
                            "reason": f"parallel: command failed (rc={rc})",
                            "command": command,
                            "at": ended_at,
                        })
                        state["last_failure"] = f"{task_id}: parallel rc={rc}"
                    t["last_update"] = ended_at
                    break
            st.save_state(state_path, state)

    return {"rc": rc, "summary": evidence_entry.get("summary", "")}


def _mark_task_failed(state_path: Path, lock, task_id: str, reason: str) -> None:
    """把内部异常导致的失败落进 state.json（线程安全），避免 closeout 门卫漏放行。"""
    with lock:
        state = st.load_state(state_path)
        if not state:
            return
        for t in state.get("tasks", []):
            if t["id"] == task_id:
                t["status"] = "blocked"
                t["blockers"].append({
                    "reason": f"parallel internal error: {reason}",
                    "at": st.iso_now(),
                })
                t["last_update"] = st.iso_now()
                break
        state["last_failure"] = f"{task_id}: parallel internal error"
        st.save_state(state_path, state)


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("parallel", help="run multiple tasks concurrently")
    p.add_argument("--run-id")
    p.add_argument("--task-ids", nargs="*", help="specific task IDs")
    p.add_argument("--phase", help="run all pending tasks in phase")
    p.add_argument("--kind", default="test", choices=["test", "lint", "build", "manual", "review", "deploy"])
    p.add_argument("--command", help="default command for tasks without recorded commands")
    p.add_argument("--cwd")
    p.add_argument("--timeout", type=int, default=300)
    p.add_argument("--touch", nargs="*")
    p.add_argument("--concurrency", type=int, default=4)
    p.add_argument("--auto-commit", action="store_true",
                   help="commit .lto state changes (opt-in; default off, uses repo git identity)")
    p.set_defaults(func=run)
