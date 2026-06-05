#!/usr/bin/env python3
"""Unit tests for the interventions.jsonl protocol layer.

Covers the schema contract documented in
references/protocol-and-language-strategy.md: optional fact fields
(actor/gate), the actor whitelist, backwards-compatibility of old events
without the new fields, and cross-run friction aggregation rules.

Standalone runner (no pytest):
  cd scripts && python3 test_interventions.py
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from lto import interventions as iv  # noqa: E402

FAIL: list[str] = []


def ok(cond: bool, msg: str) -> None:
    print(("OK   " if cond else "FAIL ") + msg, file=sys.stdout if cond else sys.stderr)
    if not cond:
        FAIL.append(msg)


_RID = "20260101-000000-probe-deadbeef"


def _base(**kw):
    args = dict(
        type="human_intervention", category="force_closeout",
        reason="r", source="s", meaningful=True, avoidable=False, preventable=False,
    )
    args.update(kw)
    return args


def test_optional_fact_fields(repo: Path) -> None:
    # Absent when not provided — no wall of nulls.
    e = iv.append(repo, _RID, **_base())
    ok("actor" not in e and "gate" not in e,
       "actor/gate omitted from event when not provided")

    # Written when provided.
    e = iv.append(repo, _RID, **_base(
        type="intervention_candidate", category="dirty_closeout_blocked",
        actor="gate", gate="closeout", dedupe_key="probe:dc"))
    ok(e.get("actor") == "gate" and e.get("gate") == "closeout",
       f"actor/gate written as facts (got actor={e.get('actor')}, gate={e.get('gate')})")


def test_actor_whitelist(repo: Path) -> None:
    for good in ("runner", "gate", "operator"):
        try:
            iv.append(repo, _RID, **_base(actor=good, dedupe_key=f"probe:actor:{good}"))
            ok(True, f"actor '{good}' accepted")
        except ValueError:
            ok(False, f"actor '{good}' wrongly rejected")
    try:
        iv.append(repo, _RID, **_base(actor="hacker"))
        ok(False, "invalid actor not rejected")
    except ValueError as exc:
        ok("actor" in str(exc), "invalid actor rejected with clear error")


def test_old_event_still_loads(repo: Path) -> None:
    # An old event with no actor/gate must load without crashing (rule 2).
    path = repo / ".lto" / _RID / "interventions.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        '{"schema_version":1,"event_id":1,"type":"human_intervention",'
        '"category":"force_closeout","meaningful":true}\n',
        encoding="utf-8",
    )
    events = iv.read(repo, _RID)
    ok(bool(events) and events[0].get("actor") is None,
       "pre-actor event loads with actor defaulting to None")


def test_cross_run_friction_rules() -> None:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        # Two distinct runs, each one dirty_closeout_blocked candidate.
        for rid in ("20260101-000000-a-aaaaaaaa", "20260102-000000-b-bbbbbbbb"):
            iv.append(repo, rid, type="intervention_candidate",
                      category="dirty_closeout_blocked", reason="r", source="s",
                      meaningful=False, avoidable=True, preventable=True,
                      actor="gate", gate="closeout")
        friction = iv.recurring_friction(repo, min_runs=2)
        dc = next((f for f in friction if f["category"] == "dirty_closeout_blocked"), None)
        ok(dc is not None and dc["runs"] == 2,
           f"recurring across 2 runs is flagged (got {friction})")

        # avoided_intervention across many runs is help, not friction.
        for rid in ("20260103-000000-c-cccccccc", "20260104-000000-d-dddddddd"):
            iv.append(repo, rid, type="avoided_intervention",
                      category="superseded_blocker", reason="r", source="s",
                      meaningful=False, avoidable=True, preventable=True,
                      actor="gate", gate="judge")
        friction = iv.recurring_friction(repo, min_runs=2)
        ok(all(f["category"] != "superseded_blocker" for f in friction),
           "pure avoided_intervention never counts as friction")

        # min_runs threshold: one run alone does not trigger.
        with tempfile.TemporaryDirectory() as t2:
            repo2 = Path(t2)
            iv.append(repo2, "20260105-000000-e-eeeeeeee", type="intervention_candidate",
                      category="dirty_closeout_blocked", reason="r", source="s",
                      meaningful=False, avoidable=True, preventable=True)
            ok(iv.recurring_friction(repo2, min_runs=2) == [],
               "single-run friction does not trigger advisory")


def main() -> int:
    with tempfile.TemporaryDirectory() as t:
        repo = Path(t)
        test_optional_fact_fields(repo)
        test_actor_whitelist(repo)
        test_old_event_still_loads(repo)
    test_cross_run_friction_rules()

    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nINTERVENTIONS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
