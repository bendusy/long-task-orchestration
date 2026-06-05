"""lto judge — 只读审查 + YAML verdict 输出。"""

from __future__ import annotations

import argparse, subprocess, sys
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import artifacts as af
from .. import interventions as iv


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    args.run_id_resolved = run_id
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    head = gs.git_head(repo)
    phase = state.get("current_phase", "unknown")

    # Collect tasks to review
    tasks = _filter_tasks(state, args)
    if not tasks:
        print(f"no tasks to judge (phase={phase}, since={args.since})")
        return 0

    # Rerun tests if requested
    test_results = _rerun_tests(repo, run_id, tasks, args)

    # Build verdict
    verdict = _build_verdict(tasks, test_results, head, args)

    # Save verdict
    judge_dir = repo / ".lto" / run_id / "judge"
    judge_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    verdict_path = judge_dir / f"judge-{phase}-{ts}.yaml"
    verdict_path.write_text(verdict, encoding="utf-8")
    af.register_path(
        repo, run_id, verdict_path, kind="judge_verdict",
        producer="lto.commands.judge", state=state,
        summary=f"judge verdict for {phase}", tags=["judge", "verdict"],
    )

    # Update state gates
    state["gates"]["last_reviewed_head"] = head
    st.save_state(state_path, state)

    # Optionally commit .lto state changes (opt-in; default off)
    verdict_line = verdict.split("\n")[0] if verdict else "done"
    verdict_val = verdict_line.split(": ")[1].strip() if ": " in verdict_line else "done"
    gs.auto_commit_lto(repo, f"lto: judge {phase}: {verdict_val}", enabled=args.auto_commit)

    print(verdict)
    return 0


def _filter_tasks(state: dict, args: argparse.Namespace) -> list[dict]:
    tasks = state.get("tasks", [])
    if args.task_id:
        return [t for t in tasks if t["id"] == args.task_id]
    if args.phase:
        return [t for t in tasks if t.get("phase") == args.phase]
    return [t for t in tasks if t.get("status") in ("done", "in_progress")]


def _rerun_tests(repo: Path, run_id: str, tasks: list[dict], args: argparse.Namespace) -> list[dict]:
    results = []
    if not args.rerun_tests:
        return results

    for task in tasks:
        for ev_entry in task.get("evidence", []):
            if ev_entry.get("kind") == "test" and ev_entry.get("rc", 1) == 0:
                cmd = ev_entry.get("command", "")
                if cmd:
                    try:
                        proc = subprocess.run(cmd, shell=True, cwd=repo, capture_output=True, text=True, timeout=120)
                        results.append({
                            "command": cmd,
                            "result": "pass" if proc.returncode == 0 else "fail",
                            "rc": proc.returncode,
                        })
                    except subprocess.TimeoutExpired:
                        results.append({"command": cmd, "result": "timeout", "rc": 124})
    return results


def _build_verdict(tasks: list[dict], test_results: list[dict], head: str, args: argparse.Namespace) -> str:
    lines = ["# LTO Judge Verdict", ""]

    # Determine overall verdict
    has_failures = any(r.get("result") != "pass" for r in test_results)
    blocker_state = {t.get("id", "?"): _classify_blockers(t) for t in tasks}
    has_blockers = any(
        t.get("status") == "blocked" or blocker_state.get(t.get("id", "?"), {}).get("active")
        for t in tasks
    )
    verdict = "fail" if has_failures or has_blockers else "pass"

    lines.extend([
        f"verdict: {verdict}",
        f"reviewed_head: {head}",
        f"runner: {args.runner}",
        f"phase: {args.phase or 'auto'}",
        f"tasks_reviewed: {len(tasks)}",
        "",
        "## Test Rerun Results",
    ])

    for r in test_results:
        lines.append(f"- command: {r['command']}")
        lines.append(f"  result: {r['result']} (rc={r['rc']})")

    lines.extend(["", "## Must Fix"])
    for task in tasks:
        task_id = task.get("id", "?")
        for blocker in blocker_state.get(task_id, {}).get("active", []):
            lines.append(f"- task: {task_id}")
            lines.append(f"  reason: {blocker.get('reason', 'unknown')}")
            if task.get("touched_files"):
                lines.append(f"  files: {', '.join(task['touched_files'][:5])}")

    lines.extend(["", "## Superseded Blockers"])
    for task in tasks:
        task_id = task.get("id", "?")
        for blocker in blocker_state.get(task_id, {}).get("superseded", []):
            lines.append(f"- task: {task_id}")
            reason = blocker.get('reason', 'unknown')
            lines.append(f"  reason: {reason}")
            lines.append("  status: superseded_by_later_success")
            if getattr(args, "run_id_resolved", None):
                iv.append(
                    args.repo.resolve(), args.run_id_resolved,
                    type="avoided_intervention",
                    category="superseded_blocker",
                    reason=f"judge ignored stale blocker on done task {task_id}",
                    source="lto judge",
                    meaningful=False,
                    avoidable=True,
                    preventable=True,
                    details={"task_id": task_id, "blocker_reason": reason},
                    dedupe_key=f"judge:superseded:{task_id}:{reason}:{head}",
                )

    lines.extend(["", "## Should Fix", "", "## Scope Drift", "", "## Residual Risks", ""])
    lines.append(f"next_action: {'fix_and_rerun' if has_failures or has_blockers else 'commit_allowed'}")

    return "\n".join(lines)


def _task_has_success(task: dict) -> bool:
    return any(ev.get("rc") == 0 for ev in task.get("evidence", []) or [])


def _classify_blockers(task: dict) -> dict[str, list[dict]]:
    """Split blockers into active brakes vs superseded history.

    Judge remains read-only: runner clears blockers on new successes, but this
    classifier keeps old runs from requiring human state surgery.
    """
    blockers = list(task.get("blockers", []) or [])
    if task.get("status") == "done" and blockers and _task_has_success(task):
        return {"active": [], "superseded": blockers}
    return {"active": blockers, "superseded": []}


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("judge", help="review runner output (read-only)")
    p.add_argument("--run-id")
    p.add_argument("--task-id", help="judge single task")
    p.add_argument("--phase", help="judge all tasks in phase")
    p.add_argument("--since", help="git base for diff review")
    p.add_argument("--runner", default="codex", help="runner agent name")
    p.add_argument("--rerun-tests", action="store_true", help="rerun task tests")
    p.add_argument("--auto-commit", action="store_true",
                   help="commit .lto state changes (opt-in; default off, uses repo git identity)")
    p.set_defaults(func=run)
