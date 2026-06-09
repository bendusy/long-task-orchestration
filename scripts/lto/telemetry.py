"""Derived telemetry snapshot for LTO runs (.lto/<run-id>/telemetry.json).

Telemetry is **derived signal**, rebuildable from state.json + artifacts.json +
events.jsonl + git status. It is not a source of truth. Per spec §5.2 it MUST
NOT persist ``control_recommendations`` or any route/promote/recommend advice —
``next`` may read it and produce an ephemeral host-facing brief; the host
decides. This module only computes deterministic metrics.

Spec: references/control-loop-harness.md §5.2 / §4.
"""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any

from . import state as st
from . import events as ev

SCHEMA_VERSION = 1


def _redact(value: str | None) -> str | None:
    """Run a free-text string through the events ingress redactor.

    telemetry.json can be exported/included in handoff, so its string fields
    (goal label, file paths) must be redacted just like event lines (review #4):
    secrets + absolute private paths removed, truncated.
    """
    if not value or not isinstance(value, str):
        return value
    return ev._clean(value)


def _rel_files(files: Any, repo: Path) -> list[str]:
    """Redacted, repo-relative file list. Absolute paths under the repo are
    rewritten relative; anything still absolute is _clean'd (private-path
    redaction) so no absolute private path leaks into exported telemetry.
    """
    out: list[str] = []
    for f in files or []:
        s = str(f)
        try:
            p = Path(s)
            if p.is_absolute():
                s = p.resolve().relative_to(repo).as_posix()
        except (ValueError, OSError):
            pass  # outside repo → fall through to _clean redaction
        cleaned = ev._clean(s)
        if cleaned:
            out.append(cleaned)
    return out


def _parse_iso(value: str | None) -> datetime | None:
    if not value or not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return None


def _seconds_between(start: str | None, end: str | None) -> int | None:
    a, b = _parse_iso(start), _parse_iso(end)
    if a is None or b is None:
        return None
    return int((b - a).total_seconds())


def _telemetry_path(repo: Path, run_id: str) -> Path:
    return repo / ".lto" / st.validate_run_id(run_id) / "telemetry.json"


def build(repo: Path, run_id: str) -> dict[str, Any]:
    """Derive a telemetry snapshot. Does not persist; call :func:`save` to write.

    Loads state (may be None for a bare run), the event log, and the artifact
    manifest, then computes Phase 1 deterministic metrics only.
    """
    repo = repo.resolve()
    run_id = st.validate_run_id(run_id)
    state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
    events = ev.read(repo, run_id)
    now = st.iso_now()

    run_metrics = _run_metrics(repo, run_id, state, events, now)
    task_metrics = _task_metrics(state, events, repo)
    redaction = _redaction_summary(events)
    event_log = _event_log_metrics(events, now)
    budget = _budget(state, run_metrics)

    return {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": now,
        "run_metrics": run_metrics,
        "task_metrics": task_metrics,
        # Deferred phases — present-but-empty, never fabricated.
        "worker_observations": [],
        "issue_metrics": {},
        "barrier_metrics": [],
        "budget": budget,
        "redaction_summary": redaction,
        "event_log": event_log,
    }


def save(repo: Path, run_id: str) -> Path:
    """Build + write telemetry.json. Returns the path."""
    snapshot = build(repo, run_id)
    path = _telemetry_path(repo, run_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def _run_metrics(repo: Path, run_id: str, state: dict, events: list[dict], now: str) -> dict[str, Any]:
    tasks = state.get("tasks", []) or []
    started = state.get("started_at")
    closed = None
    for e in events:
        if e.get("type") == "run.closed":
            closed = e.get("at")
    wall = _seconds_between(started, closed or now)

    runner_finished = [e for e in events if e.get("type") == "runner.finished"]
    runner_calls = len(runner_finished)
    timeout_count = sum(
        1 for e in runner_finished if (e.get("fields") or {}).get("timeout") is True
    )
    status_changes = sum(1 for e in events if e.get("type") == "task.status_changed")

    calls_per_hour = None
    if wall and wall > 0 and runner_calls:
        calls_per_hour = round(runner_calls / (wall / 3600.0), 3)

    return {
        "run_id": run_id,
        "goal_label": _redact(st.single_line(state.get("goal", ""))[:80]) or None,
        "phase": _redact(state.get("current_phase")),
        "started_at": started,
        "closed_at": closed,
        "wall_seconds": wall,
        "tasks_total": len(tasks),
        "tasks_done": sum(1 for t in tasks if t.get("status") == "done"),
        "tasks_blocked": sum(1 for t in tasks if t.get("status") == "blocked"),
        "wip_count": sum(1 for t in tasks if t.get("status") == "in_progress"),
        "runner_calls": runner_calls,
        "runner_calls_per_hour": calls_per_hour,
        "timeout_count": timeout_count,
        "status_transition_count": status_changes,
        "estimated_cost_usd": None,
    }


def _task_metrics(state: dict, events: list[dict], repo: Path) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for t in state.get("tasks", []) or []:
        tid = t.get("id")
        t_events = [e for e in events if e.get("task_id") == tid]
        created = next((e.get("at") for e in t_events if e.get("type") == "task.created"), None)
        started = next((e.get("at") for e in t_events if e.get("type") == "runner.started"), None)
        transitions = sum(1 for e in t_events if e.get("type") == "task.status_changed")
        out.append({
            "task_id": _redact(tid),
            "status": _redact(t.get("status")),
            "created_at": created,
            "started_at": started,
            "last_updated_at": t.get("last_update"),
            "retry_count": t.get("retry_count", 0),
            "status_transition_count": transitions,
            "evidence_count": len(t.get("evidence", []) or []),
            "touched_files": _rel_files(t.get("touched_files"), repo),
        })
    return out


def _redaction_summary(events: list[dict]) -> dict[str, int]:
    summary = {"passed": 0, "failed": 0, "not_required": 0}
    for e in events:
        status = (e.get("privacy") or {}).get("redaction_status")
        if status in summary:
            summary[status] += 1
    return summary


def _event_log_metrics(events: list[dict], now: str) -> dict[str, Any]:
    last_at = events[-1].get("at") if events else None
    return {
        "event_count": len(events),
        "seconds_since_last_event": _seconds_between(last_at, now) if last_at else None,
    }


def _budget(state: dict, run_metrics: dict) -> dict[str, Any]:
    return {
        "max_wall_seconds": None,
        "max_runner_calls": None,
        "max_cost_usd": None,
        "used_wall_seconds": run_metrics.get("wall_seconds"),
        "used_runner_calls": run_metrics.get("runner_calls"),
        "used_cost_usd": None,
    }
