"""lto pipeline — 逐个 item 通过多个阶段，item 间可并发。

借鉴 pi-dynamic-workflows 的 pipeline(items, ...stages) 模式。
"""

from __future__ import annotations

import argparse, concurrent.futures, threading, time
from pathlib import Path

from .. import state as st
from .. import git_state as gs
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
        items = [t for t in tasks if t["id"] in args.task_ids]
    elif args.phase:
        items = [t for t in tasks if t.get("phase") == args.phase]
    else:
        items = [t for t in tasks if t["status"] in ("pending", "in_progress")]

    if not items:
        print("no items to pipeline")
        return 0

    stages = args.stages
    if not stages:
        print("no stages specified; use --stages 'cmd1' 'cmd2' ...")
        return 1

    cwd = Path(args.cwd) if args.cwd else repo
    concurrency = min(args.concurrency, len(items))
    print(f"◆ LTO Pipeline: {len(items)} items × {len(stages)} stages ({concurrency} concurrent)")

    started = time.time()
    results: dict[str, list[dict]] = {}
    state_lock = threading.Lock()

    def process_item(task: dict) -> tuple[str, list[dict]]:
        task_id = task["id"]
        stage_results: list[dict] = []
        # 仅替换已过白名单校验的 {task_id}；不再替换用户自由文本 {title}（注入面）
        for si, stage_cmd in enumerate(stages):
            cmd = stage_cmd.replace("{task_id}", task_id)
            rc, evidence = lto_exec.run_command(
                repo, run_id, task_id,
                kind=args.kind, command=cmd, cwd=cwd, timeout=args.timeout,
                verified_by="pipeline",
                summary=f"stage {si}",
                artifact_suffix=f"stage{si}",
            )
            evidence["summary"] = f"stage {si} {'PASS' if rc == 0 else f'FAIL(rc={rc})'}"
            stage_results.append(evidence)
            _record_stage_evidence(state_path, state_lock, task_id, evidence)
            if rc != 0 and not args.continue_on_error:
                break
        _finalize_task_status(state_path, state_lock, task_id, stage_results, repo, args.kind)
        return task_id, stage_results

    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = {executor.submit(process_item, task): task["id"] for task in items}
        for future in concurrent.futures.as_completed(futures):
            task_id = futures[future]
            try:
                tid, stage_results = future.result()
                results[tid] = stage_results
                passed = sum(1 for s in stage_results if s["rc"] == 0)
                total = len(stage_results)
                status = "✓" if passed == total else "✗"
                print(f"  {status} {tid}: {passed}/{total} stages passed")
            except Exception as e:
                failed = [{"rc": 1, "summary": str(e), "command": "(pipeline internal error)"}]
                results[task_id] = failed
                # 把内部异常也落进 state.json，避免 closeout 门卫漏放行
                _finalize_task_status(state_path, state_lock, task_id, failed)
                print(f"  ✗ {task_id}: {e}")

    elapsed = time.time() - started
    total_stages = sum(len(r) for r in results.values())
    passed_stages = sum(sum(1 for s in r if s["rc"] == 0) for r in results.values())
    print(f"◆ {passed_stages}/{total_stages} stages passed ({elapsed:.1f}s)")

    # Optionally commit .lto state changes (opt-in; default off)
    gs.auto_commit_lto(repo, f"lto: pipeline {passed_stages}/{total_stages} stages", enabled=args.auto_commit)

    return 0 if passed_stages == total_stages else 1


def _record_stage_evidence(state_path: Path, lock, task_id: str, evidence: dict) -> None:
    """把一条 stage evidence 追加到对应 task（线程安全 reload-modify-save）。"""
    with lock:
        state = st.load_state(state_path)
        if not state:
            return
        for t in state.get("tasks", []):
            if t["id"] == task_id:
                t.setdefault("evidence", []).append(evidence)
                t["commands_run"].append(evidence.get("command", ""))
                t["last_update"] = evidence.get("ended_at", st.iso_now())
                break
        st.save_state(state_path, state)


def _finalize_task_status(
    state_path: Path, lock, task_id: str, stage_results: list[dict],
    repo: Path | None = None, kind: str = "",
) -> None:
    """所有 stage 跑完后定 task 终态：全过=done，否则 blocked。

    全过且 kind=test 时同步更新 gates.last_tested_head，让 pre-commit hook
    的 test-staleness 判定不会因 pipeline 跑过测试却没更新 head 而误报。
    """
    with lock:
        state = st.load_state(state_path)
        if not state:
            return
        all_passed = bool(stage_results) and all(s.get("rc") == 0 for s in stage_results)
        for t in state.get("tasks", []):
            if t["id"] == task_id:
                t["status"] = "done" if all_passed else "blocked"
                if all_passed and kind == "test" and repo is not None:
                    last = stage_results[-1].get("head_after") or gs.git_head(repo)
                    state["gates"]["last_tested_head"] = last
                if not all_passed:
                    failed = next((s for s in stage_results if s.get("rc") != 0), None)
                    if failed is not None:
                        t["blockers"].append({
                            "reason": f"pipeline stage failed (rc={failed.get('rc')})",
                            "command": failed.get("command", ""),
                            "at": st.iso_now(),
                        })
                    state["last_failure"] = f"{task_id}: pipeline stage rc!=0"
                t["last_update"] = st.iso_now()
                break
        st.save_state(state_path, state)


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("pipeline", help="run items through sequential stages")
    p.add_argument("--run-id")
    p.add_argument("--task-ids", nargs="*", help="specific task IDs")
    p.add_argument("--phase", help="run all items in phase")
    p.add_argument("--stages", nargs="+", required=True, help="commands per stage (use {task_id} placeholder)")
    p.add_argument("--kind", default="test", choices=["test", "lint", "build", "manual", "review", "deploy"])
    p.add_argument("--cwd")
    p.add_argument("--timeout", type=int, default=300)
    p.add_argument("--concurrency", type=int, default=4)
    p.add_argument("--continue-on-error", action="store_true", help="continue to next stage even if current fails")
    p.add_argument("--auto-commit", action="store_true",
                   help="commit .lto state changes (opt-in; default off, uses repo git identity)")
    p.set_defaults(func=run)
