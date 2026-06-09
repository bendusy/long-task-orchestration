#!/usr/bin/env python3
"""Unit tests for derived telemetry.json (scripts/lto/telemetry.py).

Covers spec references/control-loop-harness.md §5.2: telemetry is derived from
state.json + events.jsonl, computes Phase 1 deterministic metrics, and MUST NOT
contain control_recommendations or any route/promote/recommend advice.

Standalone runner (no pytest):
  cd scripts && python3 test_telemetry.py
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lto import events as ev  # noqa: E402
from lto import telemetry as tel  # noqa: E402

FAIL: list[str] = []


def ok(cond: bool, msg: str) -> None:
    print(("OK   " if cond else "FAIL ") + msg, file=sys.stdout if cond else sys.stderr)
    if not cond:
        FAIL.append(msg)


_RID = "20260101-000000-tele-deadbeef"


def _seed(repo: Path) -> None:
    run_dir = repo / ".lto" / _RID
    run_dir.mkdir(parents=True, exist_ok=True)
    state = {
        "schema_version": 1,
        "run_id": _RID,
        "goal": "build the sensor layer",
        "started_at": "2026-01-01T00:00:00+00:00",
        "current_phase": "implementation",
        "tasks": [
            {"id": "T1", "status": "done", "retry_count": 0,
             "last_update": "2026-01-01T00:10:00+00:00",
             "evidence": [{"rc": 0}], "touched_files": ["scripts/lto/events.py"]},
            {"id": "T2", "status": "blocked", "retry_count": 2,
             "last_update": "2026-01-01T00:20:00+00:00", "evidence": []},
        ],
    }
    (run_dir / "state.json").write_text(json.dumps(state), encoding="utf-8")
    # Seed an event log consistent with state.
    ev.append(repo, _RID, type="run.started", actor_kind="host")
    ev.append(repo, _RID, type="task.created", actor_kind="host", task_id="T1")
    ev.append(repo, _RID, type="runner.started", actor_kind="runner", task_id="T1")
    ev.append(repo, _RID, type="runner.finished", actor_kind="runner", task_id="T1",
              fields={"rc": 0, "elapsed_seconds": 3, "timeout": False})
    ev.append(repo, _RID, type="task.status_changed", actor_kind="runner", task_id="T1",
              fields={"from_status": "pending", "to_status": "done"})
    ev.append(repo, _RID, type="runner.finished", actor_kind="runner", task_id="T2",
              fields={"rc": 124, "timeout": True})


def test_derived_metrics(repo: Path) -> None:
    snap = tel.build(repo, _RID)
    rm = snap["run_metrics"]
    ok(rm["tasks_total"] == 2, "tasks_total derived from state")
    ok(rm["tasks_done"] == 1 and rm["tasks_blocked"] == 1, "task status counts derived")
    ok(rm["runner_calls"] == 2, "runner_calls counted from runner.finished events")
    ok(rm["timeout_count"] == 1, "timeout_count from finished event metadata")
    ok(rm["status_transition_count"] == 1, "status transitions counted")
    ok(rm["phase"] == "implementation", "phase carried from state")
    ok(snap["event_log"]["event_count"] == 6, "event_count matches events.jsonl")


def test_task_metrics(repo: Path) -> None:
    snap = tel.build(repo, _RID)
    by_id = {t["task_id"]: t for t in snap["task_metrics"]}
    ok(by_id["T1"]["created_at"] is not None, "T1 created_at derived from task.created event")
    ok(by_id["T1"]["started_at"] is not None, "T1 started_at derived from runner.started event")
    ok(by_id["T1"]["evidence_count"] == 1, "T1 evidence_count from state")
    ok(by_id["T2"]["retry_count"] == 2, "T2 retry_count from state")
    ok(by_id["T1"]["touched_files"] == ["scripts/lto/events.py"], "touched_files carried")


def test_no_recommendations(repo: Path) -> None:
    snap = tel.build(repo, _RID)
    blob = json.dumps(snap)
    for banned in ("control_recommendations", "recommend", "promote", "route"):
        ok(banned not in blob, f"telemetry contains no '{banned}' (spec §5.2 red line)")
    ok("worker_observations" in snap and snap["worker_observations"] == [],
       "deferred worker_observations present-but-empty (not fabricated)")


def test_rebuildable_and_save(repo: Path) -> None:
    p = tel.save(repo, _RID)
    ok(p.exists(), "telemetry.json written by save()")
    a = tel.build(repo, _RID)
    b = tel.build(repo, _RID)
    # generated_at differs by clock; compare the rest.
    a.pop("generated_at"); b.pop("generated_at")
    ok(a == b, "telemetry is deterministically rebuildable from sources")


def test_no_events_no_state() -> None:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        rid = "20260109-000000-empty-77777777"
        (repo / ".lto" / rid).mkdir(parents=True, exist_ok=True)
        snap = tel.build(repo, rid)
        ok(snap["run_metrics"]["tasks_total"] == 0, "bare run telemetry builds without state")
        ok(snap["event_log"]["event_count"] == 0, "empty event log handled")


def test_string_fields_redacted() -> None:
    # Review #4: telemetry string fields (goal label, touched files) are
    # export-bound and must be redacted just like event lines.
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        rid = "20260112-000000-redact-44444444"
        run_dir = repo / ".lto" / rid
        run_dir.mkdir(parents=True, exist_ok=True)
        fake_token = "sk-ant-" + "Z9y8X7w6V5u4T3s2"
        state = {
            "schema_version": 1, "run_id": rid,
            "goal": f"ship feature with {fake_token} per /Users/ben/secret/plan.md",
            "started_at": "2026-01-01T00:00:00+00:00",
            "current_phase": "implementation",
            "tasks": [{
                "id": "T1", "status": "done", "retry_count": 0,
                "last_update": "2026-01-01T00:01:00+00:00", "evidence": [],
                "touched_files": ["/Users/ben/secret/leak.py", "scripts/lto/events.py"],
            }],
        }
        (run_dir / "state.json").write_text(json.dumps(state), encoding="utf-8")
        snap = tel.build(repo, rid)
        blob = json.dumps(snap)
        ok(fake_token not in blob, "secret token redacted from telemetry goal_label")
        ok("/Users/ben/secret" not in blob, "absolute private path redacted from telemetry")
        tf = snap["task_metrics"][0]["touched_files"]
        ok(all("/Users/ben/secret" not in f for f in tf),
           "touched_files absolute private path redacted/relativized")
        ok("scripts/lto/events.py" in tf, "repo-relative touched file preserved")


def main() -> int:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        _seed(repo)
        test_derived_metrics(repo)
        test_task_metrics(repo)
        test_no_recommendations(repo)
        test_rebuildable_and_save(repo)
    test_no_events_no_state()
    test_string_fields_redacted()

    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nTELEMETRY OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
