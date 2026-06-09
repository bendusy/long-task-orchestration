#!/usr/bin/env python3
"""Unit tests for the Phase 1 events.jsonl sensor layer (scripts/lto/events.py).

Covers spec references/control-loop-harness.md §5.1/§5.3: the 8 Phase 1 event
types, append-only + monotonic event_id, duplicate-id rejection on read, old
runs with missing fields loading, the privacy redaction contract (secrets +
absolute private paths redacted BEFORE append, truncate pinned to 240 per spec
§5.0), nested/suffix raw-output key stripping, contains_raw_output rejection,
concurrent multi-process append (no duplicate ids / no byte interleave), and the
event-log size policy (warn / hard-stop).

Standalone runner (no pytest):
  cd scripts && python3 test_events.py
"""

from __future__ import annotations

import json
import sys
import tempfile
import warnings
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lto import events as ev  # noqa: E402

FAIL: list[str] = []


def ok(cond: bool, msg: str) -> None:
    print(("OK   " if cond else "FAIL ") + msg, file=sys.stdout if cond else sys.stderr)
    if not cond:
        FAIL.append(msg)


_RID = "20260101-000000-probe-deadbeef"


def test_all_phase1_types(repo: Path) -> None:
    fixtures = [
        dict(type="run.started", actor_kind="host", object_id="R", object_type="run"),
        dict(type="phase.changed", actor_kind="host", phase="implementation"),
        dict(type="task.created", actor_kind="host", task_id="T1", object_type="task"),
        dict(type="task.status_changed", actor_kind="runner", task_id="T1",
             fields={"from_status": "pending", "to_status": "done"}),
        dict(type="runner.started", actor_kind="runner", task_id="T1"),
        dict(type="runner.finished", actor_kind="runner", task_id="T1",
             fields={"rc": 0, "elapsed_seconds": 3, "timeout": False}),
        dict(type="artifact.registered", actor_kind="lto", object_id="af_x",
             object_type="artifact"),
        dict(type="run.closed", actor_kind="host", phase="closed"),
    ]
    for i, fx in enumerate(fixtures, 1):
        e = ev.append(repo, _RID, summary=f"event {i}", **fx)
        ok(e["type"] == fx["type"] and e["event_id"] == i,
           f"{fx['type']} appended with event_id={e['event_id']}")
    ok(len(ev.read(repo, _RID)) == 8, "all 8 Phase 1 events read back")


def test_deferred_type_rejected(repo: Path) -> None:
    for bad in ("finding.recorded", "issue.created", "gate.failed", "worker.observed"):
        try:
            ev.append(repo, _RID, type=bad, actor_kind="lto")
            ok(False, f"deferred type {bad} wrongly accepted")
        except ValueError:
            ok(True, f"deferred type {bad} rejected")


def test_bad_actor_rejected(repo: Path) -> None:
    try:
        ev.append(repo, _RID, type="run.started", actor_kind="hacker")
        ok(False, "invalid actor_kind accepted")
    except ValueError as exc:
        ok("actor" in str(exc), "invalid actor_kind rejected")


def test_monotonic_and_append_only(repo: Path) -> None:
    rid = "20260102-000000-mono-aaaaaaaa"
    ids = [ev.append(repo, rid, type="run.started", actor_kind="host")["event_id"]
           for _ in range(5)]
    ok(ids == [1, 2, 3, 4, 5], f"event_id monotonic 1..5 (got {ids})")
    # append-only: original lines never rewritten, only added.
    path = repo / ".lto" / rid / "events.jsonl"
    first_two = path.read_text(encoding="utf-8").splitlines()[:2]
    ev.append(repo, rid, type="run.closed", actor_kind="host")
    ok(path.read_text(encoding="utf-8").splitlines()[:2] == first_two,
       "earlier lines untouched after later append")


def test_duplicate_id_rejected_on_read(repo: Path) -> None:
    rid = "20260103-000000-dup-bbbbbbbb"
    path = repo / ".lto" / rid / "events.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        '{"event_id":1,"type":"run.started"}\n'
        '{"event_id":1,"type":"run.started"}\n'
        '{"event_id":2,"type":"run.closed"}\n',
        encoding="utf-8",
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        events = ev.read(repo, rid)
    ids = [e.get("event_id") for e in events]
    ok(ids == [1, 2], f"duplicate event_id dropped on read (got {ids})")


def test_old_run_missing_fields_loads(repo: Path) -> None:
    rid = "20260104-000000-old-cccccccc"
    path = repo / ".lto" / rid / "events.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    # A pre-schema line missing actor/privacy/artifact_refs/summary must load.
    path.write_text('{"event_id":1,"type":"run.started"}\n', encoding="utf-8")
    events = ev.read(repo, rid)
    ok(bool(events) and events[0].get("actor") is None,
       "old event missing fields loads (defaults to None)")
    # Broken JSON line is skipped, not fatal.
    with path.open("a", encoding="utf-8") as f:
        f.write("{not json\n")
        f.write('{"event_id":2,"type":"run.closed"}\n')
    ok(len(ev.read(repo, rid)) == 2, "broken line skipped, valid lines kept")


def test_privacy_redaction(repo: Path) -> None:
    rid = "20260105-000000-priv-dddddddd"
    fake_token = "sk-ant-" + "A1b2C3d4E5f6G7h8"
    secret_summary = f"leaked {fake_token} at /Users/ben/secret/key.pem now"
    e = ev.append(
        repo, rid, type="runner.finished", actor_kind="runner",
        summary=secret_summary,
        fields={"note": "see /Users/ben/secret/out.log and token sk-test_ABCDEFGHIJKL"},
        artifact_refs=["/Users/ben/secret/evidence.txt"],
    )
    blob = (repo / ".lto" / rid / "events.jsonl").read_text(encoding="utf-8")
    ok("/Users/ben/secret" not in blob, "absolute private path redacted from event line")
    ok(fake_token not in blob, "secret token redacted from event line")
    ok("[REDACTED_SECRET]" in e["summary"], "secret replaced with marker in summary")
    ok("[REDACTED_PATH]" in e["summary"], "path replaced with marker in summary")
    ok("[REDACTED_PATH]" in e["fields"]["note"], "fields free-text redacted too")
    ok(all("/Users/ben/secret" not in r for r in e["artifact_refs"]),
       "artifact_refs absolute private path redacted")


def test_raw_output_keys_stripped(repo: Path) -> None:
    rid = "20260106-000000-raw-eeeeeeee"
    e = ev.append(
        repo, rid, type="runner.finished", actor_kind="runner",
        fields={
            "rc": 1,
            "stdout": "BIG RAW OUTPUT",
            "stderr": "trace",
            "reply_text": "x",
            # nested raw output (review #3)
            "details": {"stderr": "RAW NESTED STDERR", "kept": "ok"},
            # suffix-matched keys
            "stderr_excerpt": "EXCERPT LEAK",
            "command_output_tail": "TAIL LEAK",
        },
    )
    f = e.get("fields", {})
    ok("stdout" not in f and "stderr" not in f and "reply_text" not in f,
       "top-level raw-output keys stripped from event fields")
    ok("stderr" not in f.get("details", {}),
       "nested raw-output key stripped from event fields")
    ok(f.get("details", {}).get("kept") == "ok", "nested non-raw sibling preserved")
    ok("stderr_excerpt" not in f and "command_output_tail" not in f,
       "*_excerpt / *_output* / *_tail suffix keys stripped")
    ok(f.get("rc") == 1, "legit metadata (rc) preserved")
    blob = (repo / ".lto" / rid / "events.jsonl").read_text(encoding="utf-8")
    for leak in ("RAW NESTED STDERR", "EXCERPT LEAK", "TAIL LEAK", "BIG RAW OUTPUT"):
        ok(leak not in blob, f"raw value {leak!r} absent from event line")


def test_truncate_is_240(repo: Path) -> None:
    # Truncate width is spec §5.0 (240), not the old interventions 500.
    ok(ev._TRUNCATE == 240, f"_clean truncate width pinned at 240 (got {ev._TRUNCATE})")
    rid = "20260107-000000-trunc-ffffffff"
    e = ev.append(repo, rid, type="run.started", actor_kind="host", summary="x" * 1000)
    ok(len(e["summary"]) == 240, f"summary truncated to 240 (got {len(e['summary'])})")


def test_contains_raw_output_rejected(repo: Path) -> None:
    rid = "20260110-000000-craw-66666666"
    try:
        ev.append(repo, rid, type="runner.finished", actor_kind="runner",
                  contains_raw_output=True, summary="x")
        ok(False, "contains_raw_output=True wrongly accepted")
    except ValueError as exc:
        ok("contains_raw_output" in str(exc), "contains_raw_output=True rejected")


def test_size_policy() -> None:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        rid = "20260108-000000-size-99999999"
        path = repo / ".lto" / rid / "events.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        # Warn threshold: pre-seed WARN_AT lines, next append warns.
        path.write_text("".join(
            '{"event_id":%d,"type":"run.started"}\n' % i
            for i in range(1, ev.WARN_AT + 1)
        ), encoding="utf-8")
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            ev.append(repo, rid, type="run.started", actor_kind="host")
            ok(any("warn threshold" in str(x.message) for x in w),
               "warn emitted at WARN_AT events")

        # Hard stop: pre-seed HARD_STOP_AT lines.
        rid2 = "20260108-000000-hard-88888888"
        path2 = repo / ".lto" / rid2 / "events.jsonl"
        path2.parent.mkdir(parents=True, exist_ok=True)
        path2.write_text("".join(
            '{"event_id":%d,"type":"run.started"}\n' % i
            for i in range(1, ev.HARD_STOP_AT + 1)
        ), encoding="utf-8")
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            try:
                ev.append(repo, rid2, type="run.started", actor_kind="host")
                ok(False, "hard stop not enforced")
            except ValueError as exc:
                ok("hard stop" in str(exc), "hard stop blocks non-critical event")
            # force=True overrides.
            e = ev.append(repo, rid2, type="run.started", actor_kind="host", force=True)
            ok(e is not None, "force=True overrides hard stop")


def _concurrent_worker(repo_str: str, rid: str, n: int) -> None:
    # Child process: append n events. Must run under the cross-process lock so
    # ids never collide and lines never interleave.
    import warnings as _w
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from lto import events as _ev
    _w.simplefilter("ignore")
    for _ in range(n):
        _ev.append(Path(repo_str), rid, type="run.started", actor_kind="host",
                   summary="concurrent append")


def test_concurrent_append() -> None:
    import multiprocessing as mp
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        rid = "20260111-000000-conc-55555555"
        (repo / ".lto" / rid).mkdir(parents=True, exist_ok=True)
        procs, per = 6, 40
        ctx = mp.get_context("fork")
        workers = [ctx.Process(target=_concurrent_worker, args=(str(repo), rid, per))
                   for _ in range(procs)]
        for w in workers:
            w.start()
        for w in workers:
            w.join()
            ok(w.exitcode == 0, f"concurrent worker pid exited clean (rc={w.exitcode})")

        path = repo / ".lto" / rid / "events.jsonl"
        lines = [ln for ln in path.read_text(encoding="utf-8").splitlines() if ln.strip()]
        ok(len(lines) == procs * per, f"all {procs * per} appends landed (got {len(lines)})")
        ids = []
        all_parse = True
        for ln in lines:
            try:
                ids.append(json.loads(ln)["event_id"])
            except (json.JSONDecodeError, KeyError):
                all_parse = False
        ok(all_parse, "every concurrent line is valid JSON (no byte interleave)")
        ok(len(set(ids)) == len(ids), f"no duplicate event_id under concurrency ({len(ids) - len(set(ids))} dups)")
        ok(sorted(ids) == list(range(1, procs * per + 1)),
           "event_ids are a contiguous 1..N set under concurrency")


def test_emit_is_failsafe() -> None:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        # Invalid run id would make append raise; emit must swallow → None.
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            r = ev.emit(repo, "../escape", type="run.started", actor_kind="host")
        ok(r is None, "emit swallows append failure and returns None")


def main() -> int:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        test_all_phase1_types(repo)
        test_deferred_type_rejected(repo)
        test_bad_actor_rejected(repo)
        test_monotonic_and_append_only(repo)
        test_duplicate_id_rejected_on_read(repo)
        test_old_run_missing_fields_loads(repo)
        test_privacy_redaction(repo)
        test_raw_output_keys_stripped(repo)
        test_truncate_is_240(repo)
        test_contains_raw_output_rejected(repo)
    test_size_policy()
    test_concurrent_append()
    test_emit_is_failsafe()

    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nEVENTS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
