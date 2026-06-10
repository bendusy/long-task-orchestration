"""lto memory — ANIMEM/memory-flow projection helpers."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from ..memory_projection import build_projection
from ..memory_sink import (
    AmCliSink,
    LegacyMemoryFlowSink,
    MemorySink,
    MemorySinkError,
    print_resume_result,
)


def _make_sink(args: argparse.Namespace) -> MemorySink:
    """Pick the memory sink. am-cli is the recommended path since am 0.7.0;
    legacy-rest stays as a fallback for hosts still on memory-flow REST."""
    sink = getattr(args, "sink", "am-cli")
    if sink == "legacy-rest":
        return LegacyMemoryFlowSink(
            url=getattr(args, "url", None),
            token=getattr(args, "token", None),
            timeout=args.timeout,
        )
    return AmCliSink(binary=getattr(args, "am_bin", None), timeout=args.timeout)


def run(args: argparse.Namespace) -> int:
    action = getattr(args, "memory_action", "")
    if action == "export":
        return _export(args)
    if action == "publish":
        return _publish(args)
    if action == "resume":
        return _resume(args)
    raise SystemExit(f"unknown memory action: {action}")


def _export(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    projection = build_projection(repo, run_id)
    print(json.dumps(projection, ensure_ascii=False, indent=2, sort_keys=True))
    return 0


def _publish(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    projection = build_projection(repo, run_id)
    sink = _make_sink(args)
    try:
        result = sink.publish(projection)
    except MemorySinkError as exc:
        print(f"memory publish failed: {exc}", file=sys.stderr)
        return 2
    print(json.dumps({
        "ok": result.ok,
        "published": result.published,
        "written": result.written,
        "updated": result.updated,
        "skipped": result.skipped,
        "failed": result.failed,
        "detail": result.detail,
    }, ensure_ascii=False, sort_keys=True))
    return 0 if result.ok else 2


def _resume(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    project_key = args.project or repo.name
    sink = _make_sink(args)
    try:
        result = sink.resume(project_key)
        print_resume_result(result)
    except MemorySinkError as exc:
        print(
            "warning: am/ANIMEM memory unavailable; "
            f"using local .lto only. cross-project history unavailable. ({exc})",
            file=sys.stderr,
        )
    return _print_local_resume(repo, args.run_id)


def _print_local_resume(repo: Path, explicit_run_id: str | None) -> int:
    run_id = _local_run_id(repo, explicit_run_id)
    if not run_id:
        print("LTO: no local .lto state available", file=sys.stderr)
        return 1
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        print(
            f"LTO: local state missing for {run_id}; ensure the matching branch is checked out "
            f"or sync .lto/{run_id}/ before running lto resume --run-id {run_id}",
            file=sys.stderr,
        )
        return 1
    _print_capsule(repo, run_id, state)
    return 0


def _local_run_id(repo: Path, explicit_run_id: str | None) -> str | None:
    if explicit_run_id:
        return st.validate_run_id(explicit_run_id)
    current = repo / ".lto" / "current"
    if current.exists():
        value = current.read_text(encoding="utf-8").strip()
        return st.validate_run_id(value) if value else None
    return None


def _print_capsule(repo: Path, run_id: str, state: dict) -> None:
    ws = state.get("workspace", {})
    actual_head = gs.git_head(repo)
    state_hash_note = _hash_note(repo, run_id)
    tasks = state.get("tasks", []) or []
    task_summary = ", ".join(f"{t.get('id')}:{t.get('status')}" for t in tasks[-5:]) or "none"
    print("=== LTO MEMORY LOCAL CAPSULE ===")
    print(f"Run ID: {run_id}")
    print(f"Goal: {state.get('goal', '?')}")
    print(f"Phase: {state.get('current_phase', 'unknown')}")
    print(f"Recorded Head: {ws.get('head', 'unknown')[:12]}")
    print(f"Current Head: {actual_head[:12]}")
    print(f"Tasks: {task_summary}")
    print(f"Next: {(state.get('next_action') or '')[:160]}")
    print(f"Projection Drift: {state_hash_note}")
    print("Local .lto remains source of truth; memory resume did not modify files.")
    print("================================")


def _hash_note(repo: Path, run_id: str) -> str:
    projection = build_projection(repo, run_id)
    run = next((r for r in projection.get("records", []) if r.get("kind") == "lto_run_snapshot"), {})
    return f"state_hash={run.get('state_hash', '')[:12]} artifact_hash={run.get('artifact_hash', '')[:12]}"


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("memory", help="export/publish/resume ANIMEM artifact-memory projection")
    mem = p.add_subparsers(dest="memory_action", required=True)

    export = mem.add_parser("export", help="print redacted LTO memory projection JSON")
    export.add_argument("--run-id")
    export.add_argument("--dry-run", action="store_true", help="accepted for clarity; export never writes")
    export.set_defaults(func=run)

    publish = mem.add_parser(
        "publish",
        help="publish redacted projection to am (native CLI, default) or legacy memory-flow REST",
    )
    publish.add_argument("--run-id")
    publish.add_argument(
        "--sink", choices=["am-cli", "legacy-rest"], default="am-cli",
        help="am-cli = pipe envelope to `am ingest` (recommended since am 0.7.0); "
             "legacy-rest = memory-flow REST fallback",
    )
    publish.add_argument("--am-bin", help="am binary path; defaults to AM_BIN or 'am'")
    publish.add_argument("--url", help="[legacy-rest] memory-flow base URL; defaults to MEMORY_FLOW_URL")
    publish.add_argument("--token", help="[legacy-rest] memory-flow token; defaults to MEMORY_FLOW_TOKEN")
    publish.add_argument("--timeout", type=float, default=60.0,
                         help="sink timeout seconds (am ingest connects PG, slower than REST)")
    publish.set_defaults(func=run)

    resume = mem.add_parser("resume", help="discover memory projection, then print local-first capsule")
    resume.add_argument("--project", help="project key; defaults to repo directory name")
    resume.add_argument("--run-id", help="local run id for fallback capsule")
    resume.add_argument(
        "--sink", choices=["am-cli", "legacy-rest"], default="am-cli",
        help="am-cli = `am search` (recommended); legacy-rest = memory-flow REST fallback",
    )
    resume.add_argument("--am-bin", help="am binary path; defaults to AM_BIN or 'am'")
    resume.add_argument("--url", help="[legacy-rest] memory-flow base URL; defaults to MEMORY_FLOW_URL")
    resume.add_argument("--token", help="[legacy-rest] memory-flow token; defaults to MEMORY_FLOW_TOKEN")
    resume.add_argument("--timeout", type=float, default=60.0)
    resume.set_defaults(func=run)
