#!/usr/bin/env python3
"""Self-test for decision.py — tally_votes, merge_findings, injection defense, brief builder.

Run: PYTHONPATH=skills/long-task-orchestration/scripts python3 skills/long-task-orchestration/scripts/lto/test_decision.py
"""

from __future__ import annotations

import json
import tempfile
from pathlib import Path

from lto.agent_job import AgentResult, JobStatus
from lto.auditors import parse_findings_text
from lto.decision import (
    tally_votes,
    merge_findings,
    _build_escalate_key,
    _has_spawned_before,
    _record_spawn,
    _parse_direction_reply,
    _compose_result,
    run_decision,
)
from lto.decision_brief import build_decision_brief_v2
from lto.commands.next import analyze


def _run_selftest() -> int:
    tests_passed = 0
    tests_total = 0

    def ok(cond: bool, label: str) -> None:
        nonlocal tests_passed, tests_total
        tests_total += 1
        if cond:
            tests_passed += 1
            print(f"  ✅ {label}")
        else:
            print(f"  ❌ {label}")

    # ═══════════════════════════════════════════════════════════════
    # T1-T3: tally_votes basic (unchanged, still valid)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T1] tally_votes: 2/3 majority → converged")
    replies = [
        {"decision": "pick_task", "value": "T1", "reasoning": "most critical", "source": "codex"},
        {"decision": "pick_task", "value": "T1", "reasoning": "also T1", "source": "pi"},
        {"decision": "pick_task", "value": "T2", "reasoning": "T2 first", "source": "agy"},
    ]
    tally = tally_votes(replies)
    ok(tally["supermajority_met"], "T1a: supermajority met")
    ok(tally["majority_pick"] == "pick_task:T1", f"T1b: majority = pick_task:T1")
    ok(tally["majority_count"] == 2, f"T1c: count = 2")
    ok(len(tally["minority"]) == 1, f"T1d: 1 minority vote")
    ok(tally["minority"][0]["source"] == "agy", "T1e: minority is agy")
    ok(not tally["needs_info"], "T1f: needs_info=False")

    print("\n[T2] tally_votes: 3/3 unanimous")
    replies2 = [
        {"decision": "pick_pattern", "value": "linear", "reasoning": "seq", "source": "codex"},
        {"decision": "pick_pattern", "value": "linear", "reasoning": "order", "source": "pi"},
        {"decision": "pick_pattern", "value": "linear", "reasoning": "safe", "source": "agy"},
    ]
    tally2 = tally_votes(replies2)
    ok(tally2["supermajority_met"], "T2a: 3/3")
    ok(tally2["majority_count"] == 3, "T2b: count=3")
    ok(len(tally2["minority"]) == 0, "T2c: 0 minority")

    print("\n[T3] tally_votes: 1/1/1 tie → needs_info")
    r3 = [
        {"decision": "pick_task", "value": "T1", "reasoning": "...", "source": "codex"},
        {"decision": "pick_task", "value": "T2", "reasoning": "...", "source": "pi"},
        {"decision": "pick_task", "value": "T3", "reasoning": "...", "source": "agy"},
    ]
    t3 = tally_votes(r3)
    ok(not t3["supermajority_met"], "T3a: no supermajority")
    ok(t3["needs_info"], "T3b: needs_info=True")

    # ═══════════════════════════════════════════════════════════════
    # T4 (FIX-2): 2 agree + 1 needs_human → needs_info (ONE-VOTE VETO)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T4] tally_votes: 2 agree + 1 needs_human → needs_info (FIX-2: one-vote veto)")
    r4 = [
        {"decision": "pick_task", "value": "T1", "reasoning": "...", "source": "codex"},
        {"decision": "pick_task", "value": "T1", "reasoning": "...", "source": "pi"},
        {"decision": "needs_human", "value": "ambiguous", "reasoning": "...", "source": "agy"},
    ]
    t4 = tally_votes(r4)
    ok(t4["supermajority_met"], "T4a: supermajority met (2/3 for T1)")
    ok(t4["needs_info"], "T4b: needs_info=True (one needs_human = veto)")
    ok(t4["needs_human_votes"] == 1, "T4c: needs_human_votes=1")

    # ═══════════════════════════════════════════════════════════════
    # T5 (FIX-2b): ≥2 needs_human → needs_info (still works)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T5] tally_votes: ≥2 needs_human → needs_info")
    r5 = [
        {"decision": "needs_human", "value": "complex", "reasoning": "...", "source": "codex"},
        {"decision": "needs_human", "value": "unsure", "reasoning": "...", "source": "pi"},
        {"decision": "pick_task", "value": "T1", "reasoning": "...", "source": "agy"},
    ]
    t5 = tally_votes(r5)
    ok(t5["needs_info"], "T5a: needs_info=True")
    ok(t5["needs_human_votes"] == 2, "T5b: 2 needs_human")

    # ═══════════════════════════════════════════════════════════════
    # T6: tally_votes empty list
    # ═══════════════════════════════════════════════════════════════

    print("\n[T6] tally_votes: empty list")
    t6 = tally_votes([])
    ok(t6["needs_info"], "T6a: needs_info=True")
    ok(t6["total_voters"] == 0, "T6b: total_voters=0")
    ok(t6["invalid_votes_count"] == 0, "T6c: invalid_votes_count=0")

    # ═══════════════════════════════════════════════════════════════
    # T7-T9: merge_findings (unchanged)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T7] merge_findings: union merge, no dedup")
    results = [
        AgentResult(job_id="rev-codex", runner="codex", status=JobStatus.OK.value,
                    findings=[{"severity": "critical", "claim": "race", "file": "src/x.py"}]),
        AgentResult(job_id="rev-pi", runner="pi", status=JobStatus.OK.value,
                    findings=[{"severity": "high", "claim": "no tests", "file": "tests/"}]),
        AgentResult(job_id="rev-agy", runner="agy", status=JobStatus.OK.value,
                    reply_text=json.dumps([{"severity": "low", "claim": "unregistered risk", "file": "state.json"}])),
    ]
    m = merge_findings(results)
    ok(len(m) == 3, f"T7a: 3 findings")
    ok({f["source"] for f in m} == {"codex", "pi", "agy"}, "T7b: 3 sources")
    ok(any(f["claim"] == "race" for f in m), "T7c: codex preserved")
    ok(any(f["claim"] == "no tests" for f in m), "T7d: pi preserved")
    ok(any(f["claim"] == "unregistered risk" for f in m), "T7e: agy from reply_text")
    fenced_findings = parse_findings_text(
        '```json\n[{"severity":"medium","claim":"shared parser","file":"x"}]\n```'
    )
    ok(fenced_findings is not None and fenced_findings[0]["claim"] == "shared parser",
       "T7f: core parser handles fenced findings text")

    print("\n[T8] merge_findings: empty agent → others collected")
    r8 = [
        AgentResult(job_id="r1", runner="codex", status=JobStatus.OK.value,
                    findings=[{"severity": "high", "claim": "auth", "file": "src/auth.py"}]),
        AgentResult(job_id="r2", runner="pi", status=JobStatus.OK.value,
                    findings=[], reply_text="[]"),
        AgentResult(job_id="r3", runner="agy", status=JobStatus.OK.value,
                    findings=[{"severity": "medium", "claim": "log", "file": "src/log.py"}]),
    ]
    m8 = merge_findings(r8)
    ok(len(m8) == 2, f"T8a: 2 findings")
    ok(any(f["claim"] == "auth" for f in m8), "T8b: codex intact")
    ok(any(f["claim"] == "log" for f in m8), "T8c: agy intact")

    print("\n[T9] merge_findings: all empty → []")
    r9 = [
        AgentResult(job_id="r1", runner="codex", status=JobStatus.OK.value, findings=[]),
        AgentResult(job_id="r2", runner="pi", status=JobStatus.OK.value, findings=[], reply_text="[]"),
        AgentResult(job_id="r3", runner="agy", status=JobStatus.OK.value, findings=[]),
    ]
    m9 = merge_findings(r9)
    ok(len(m9) == 0, f"T9a: 0 findings")

    # ═══════════════════════════════════════════════════════════════
    # T10 (FIX-1a): injection — "T99" non-existent task_id → rejected
    # ═══════════════════════════════════════════════════════════════

    print("\n[T10] Injection: non-existent task_id T99 → invalid (FIX-1a)")
    r10 = [
        {"decision": "pick_task", "value": "T1", "reasoning": "critical", "source": "codex"},
        {"decision": "pick_task", "value": "T99", "reasoning": "bad task", "source": "pi"},
        {"decision": "pick_task", "value": "T2", "reasoning": "T2", "source": "agy"},
    ]
    t10 = tally_votes(r10, valid_task_ids={"T1", "T2"})
    ok(t10["invalid_votes_count"] == 1, "T10a: 1 invalid vote (T99)")
    ok(len(t10["invalid_votes"]) == 1, "T10b: invalid_votes list has 1 entry")
    ok(t10["invalid_votes"][0]["source"] == "pi", "T10c: pi's vote rejected")
    ok(t10["total_voters"] == 2, "T10d: only 2 valid voters")
    ok(not t10["supermajority_met"], "T10e: 1/1 tie → no supermajority")
    ok(t10["needs_info"], "T10f: tie → needs_info")

    # ═══════════════════════════════════════════════════════════════
    # T11 (FIX-1b): injection — "T1; rm -rf /" → rejected
    # ═══════════════════════════════════════════════════════════════

    print("\n[T11] Injection: command injection string → invalid (FIX-1b)")
    r11 = [
        {"decision": "pick_task", "value": "T1", "reasoning": "ok", "source": "codex"},
        {"decision": "pick_task", "value": "T1", "reasoning": "ok", "source": "pi"},
        {"decision": "pick_task", "value": "T1; rm -rf /", "reasoning": "malicious", "source": "agy"},
    ]
    t11 = tally_votes(r11, valid_task_ids={"T1", "T2"})
    ok(t11["invalid_votes_count"] == 1, "T11a: injection string rejected")
    ok(t11["total_voters"] == 2, "T11b: 2 valid voters (codex+pi)")
    ok(t11["supermajority_met"], "T11c: 2/2 for T1 → converged")
    ok(t11["majority_pick"] == "pick_task:T1", "T11d: majority = T1, NOT the injection string")

    # ═══════════════════════════════════════════════════════════════
    # T12 (FIX-1c): pick_pattern with illegal value → rejected
    # ═══════════════════════════════════════════════════════════════

    print("\n[T12] pick_pattern illegal value → invalid (FIX-1c)")
    r12 = [
        {"decision": "pick_pattern", "value": "linear", "reasoning": "safe", "source": "codex"},
        {"decision": "pick_pattern", "value": "linear", "reasoning": "safe", "source": "pi"},
        {"decision": "pick_pattern", "value": "chaos", "reasoning": "bad pattern", "source": "agy"},
    ]
    t12 = tally_votes(r12)
    ok(t12["invalid_votes_count"] == 1, "T12a: 'chaos' rejected")
    ok(t12["total_voters"] == 2, "T12b: 2 valid voters")
    ok(t12["supermajority_met"], "T12c: 2/2 for linear")

    # ═══════════════════════════════════════════════════════════════
    # T13 (FIX-1d): valid_task_ids=None → backward compat, no validation
    # ═══════════════════════════════════════════════════════════════

    print("\n[T13] valid_task_ids=None → no task ID validation (backward compat)")
    r13 = [
        {"decision": "pick_task", "value": "T99", "reasoning": "weird id", "source": "codex"},
        {"decision": "pick_task", "value": "T99", "reasoning": "same", "source": "pi"},
        {"decision": "pick_task", "value": "T1", "reasoning": "normal", "source": "agy"},
    ]
    t13 = tally_votes(r13)  # no valid_task_ids → no validation
    ok(t13["invalid_votes_count"] == 0, "T13a: no validation when valid_task_ids=None")
    ok(t13["supermajority_met"], "T13b: T99 gets 2/3")
    ok(t13["majority_pick"] == "pick_task:T99", "T13c: T99 wins (no whitelist)")

    # ═══════════════════════════════════════════════════════════════
    # T14-T16: escalate-point dedup (unchanged)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T14] Escalate-point dedup")
    state = {}
    facts = {"phase": "impl", "blocked": [{"id": "T1"}, {"id": "T2"}], "pending": []}
    key1 = _build_escalate_key(facts)
    ok(not _has_spawned_before(state, key1), "T14a: not spawned yet")
    _record_spawn(state, key1)
    ok(_has_spawned_before(state, key1), "T14b: spawned after record")
    facts2 = {"phase": "impl", "blocked": [{"id": "T2"}, {"id": "T1"}], "pending": []}
    key2 = _build_escalate_key(facts2)
    ok(key1 == key2, "T14c: same facts → same key")
    facts3 = {"phase": "audit", "blocked": [{"id": "T1"}], "pending": [{"id": "T3"}]}
    key3 = _build_escalate_key(facts3)
    ok(key1 != key3, "T14d: diff facts → diff key")

    # ═══════════════════════════════════════════════════════════════
    # T15-T17: _parse_direction_reply (unchanged)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T15] _parse_direction_reply: valid JSON")
    r = AgentResult(job_id="t", runner="codex", status=JobStatus.OK.value,
                    reply_text=json.dumps({"decision": "pick_task", "value": "T1", "reasoning": "x"}))
    p = _parse_direction_reply(r)
    ok(p is not None and p["decision"] == "pick_task", "T15")

    print("\n[T16] _parse_direction_reply: JSON fence")
    r16 = AgentResult(job_id="t", runner="pi", status=JobStatus.OK.value,
                      reply_text='```json\n{"decision": "pick_pattern", "value": "linear", "reasoning": "seq"}\n```')
    p16 = _parse_direction_reply(r16)
    ok(p16 is not None and p16["decision"] == "pick_pattern", "T16")

    print("\n[T17] _parse_direction_reply: garbage")
    r17 = AgentResult(job_id="t", runner="agy", status=JobStatus.OK.value,
                      reply_text="I think T1 is good...")
    ok(_parse_direction_reply(r17) is None, "T17")

    # ═══════════════════════════════════════════════════════════════
    # T18-T19: build_decision_brief_v2 (unchanged)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T18] build_decision_brief_v2: direction converged")
    brief = build_decision_brief_v2(
        decision_kind="direction",
        direction_result={"track": "direction", "dispatched_to": ["codex", "pi", "agy"],
                          "tally": {"majority_pick": "pick_task:T1", "majority_count": 2,
                                    "total_voters": 3, "supermajority_met": True,
                                    "votes": [{"source": "c", "decision": "pick_task", "value": "T1", "reasoning": "x"}],
                                    "minority": [], "needs_human_votes": 0, "needs_info": False}},
        review_result=None,
        facts={}, state={"goal": "t"}, host="claude",
        dispatched=["codex", "pi", "agy"], status="converged", budget_spent=18000,
    )
    ok("CONVERGED" in brief.upper(), "T18a")
    ok("INJECTION DEFENSE" in brief, "T18b: injection defense")
    ok("approximate" in brief.lower(), "T18c: approximate budget declared")

    print("\n[T19] build_decision_brief_v2: review track")
    b19 = build_decision_brief_v2(
        decision_kind="review", direction_result=None,
        review_result={"track": "review", "dispatched_to": ["c","p","a"],
                       "merged_findings": [
                           {"severity": "critical", "claim": "race", "source": "codex", "file":"x","evidence_to_check":""},
                           {"severity": "high", "claim": "no tests", "source": "pi", "file":"","evidence_to_check":""},
                       ]},
        facts={}, state={}, host="claude",
        dispatched=["c","p","a"], status="converged", budget_spent=18000,
    )
    ok("Union Merge" in b19, "T19a")
    ok("race" in b19 and "no tests" in b19, "T19b: both findings visible")

    # ═══════════════════════════════════════════════════════════════
    # T20-T21: run_decision edge cases (unchanged except renumbered)
    # ═══════════════════════════════════════════════════════════════

    print("\n[T20] run_decision: budget exhausted")
    with tempfile.TemporaryDirectory() as d:
        repo = Path(d) / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
        st20 = {"schema_version": 1, "run_id": "t", "goal": "t", "host_runtime": "claude",
                "current_phase": "impl", "tasks": [{"id": "T1", "title": "t", "status": "blocked",
                "blockers": [{"reason": "fail"}]}], "phase_transitions": [], "user_decisions": [], "blocked_by": "none"}
        facts20 = analyze(st20, repo)
        result = run_decision(repo, "t", facts20, st20, decision_kind="direction", budget_remaining=1000)
        ok(result["status"] == "needs_human", "T20a")
        ok("Budget Exhausted" in result["brief"], "T20b")

    print("\n[T21] run_decision: re-spawn dedup")
    with tempfile.TemporaryDirectory() as d:
        repo = Path(d) / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
        st21b = {"schema_version": 1, "run_id": "t", "goal": "t", "host_runtime": "claude",
                 "current_phase": "impl", "tasks": [{"id": "T1", "title": "t", "status": "blocked",
                 "blockers": [{"reason": "fail"}]}], "phase_transitions": [], "user_decisions": [], "blocked_by": "none"}
        st21 = {**st21b, "decision_escalate_points": {_build_escalate_key(analyze(st21b, repo)): "2026-06-03T00:00:00"}}
        facts21 = analyze(st21, repo)
        result21 = run_decision(repo, "t", facts21, st21, decision_kind="direction", budget_remaining=100000)
        ok(result21["status"] == "needs_human", "T21a")
        ok("already spawned" in result21["brief"].lower(), "T21b")

    # ═══════════════════════════════════════════════════════════════
    # T22 (FIX-3): _compose_result review with empty findings → converged
    # ═══════════════════════════════════════════════════════════════

    print("\n[T22] _compose_result: review with empty findings → converged (FIX-3)")
    review_empty = {
        "track": "review",
        "dispatched_to": ["codex", "pi", "agy"],
        "results": [
            AgentResult(job_id="r1", runner="codex", status=JobStatus.OK.value,
                       reply_text="[]", findings=[]),
            AgentResult(job_id="r2", runner="pi", status=JobStatus.OK.value,
                       reply_text="[]", findings=[]),
            AgentResult(job_id="r3", runner="agy", status=JobStatus.OK.value,
                       reply_text="[]", findings=[]),
        ],
        "merged_findings": [],
    }
    comp22 = _compose_result(
        decision_kind="review",
        direction_result=None,
        review_result=review_empty,
        dispatched=["codex", "pi", "agy"],
        budget_spent=18000,
        facts={},
        state={},
        host="claude",
    )
    ok(comp22["status"] == "converged", "T22a: empty review = converged (clean)")
    ok(len(comp22["result"]) == 0, "T22b: result is empty findings list")
    ok("review clean" in comp22["brief"].lower(), "T22c: brief says review clean")

    # ═══════════════════════════════════════════════════════════════
    # T23 (FIX-3b): _compose_result review with findings → converged
    # ═══════════════════════════════════════════════════════════════

    print("\n[T23] _compose_result: review with findings → converged (FIX-3b)")
    review_findings = {
        "track": "review",
        "dispatched_to": ["codex", "pi", "agy"],
        "results": [
            AgentResult(job_id="r1", runner="codex", status=JobStatus.OK.value,
                       reply_text="[{}]", findings=[]),
            AgentResult(job_id="r2", runner="pi", status=JobStatus.OK.value,
                       reply_text="[{}]", findings=[]),
            AgentResult(job_id="r3", runner="agy", status=JobStatus.OK.value,
                       reply_text="[{}]", findings=[]),
        ],
        "merged_findings": [
            {"severity": "high", "claim": "auth bug", "source": "codex", "file": "", "evidence_to_check": ""},
        ],
    }
    comp23 = _compose_result(
        decision_kind="review",
        direction_result=None,
        review_result=review_findings,
        dispatched=["codex", "pi", "agy"],
        budget_spent=18000,
        facts={},
        state={},
        host="claude",
    )
    ok(comp23["status"] == "converged", "T23a: review with findings = converged")
    ok(len(comp23["result"]) == 1, "T23b: 1 finding in result")

    # ═══════════════════════════════════════════════════════════════
    # T24 (FIX-4): run_decision with < 2 valid reviewers → needs_human
    # ═══════════════════════════════════════════════════════════════

    print("\n[T24] run_decision: < 2 valid reviewers → needs_human (FIX-4)")
    import os

    with tempfile.TemporaryDirectory() as d:
        dpath = Path(d)

        # ── scaffold repo ──
        repo = dpath / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

        # ── state with real tasks ──
        st24 = {
            "schema_version": 1, "run_id": "t24", "goal": "test_fix4",
            "host_runtime": "claude", "current_phase": "impl",
            "tasks": [
                {"id": "T1", "title": "task1", "status": "blocked", "blockers": [{"reason": "fail"}],
                 "command": "echo ok", "touched_files": ["x.py"]},
                {"id": "T2", "title": "task2", "status": "pending",
                 "command": "echo ok2", "touched_files": ["y.py"]},
            ],
            "phase_transitions": [], "user_decisions": [], "blocked_by": "none",
        }

        # ── fake runner: reads runner name from env var ──
        fake_runner = dpath / "fake_runner.py"
        fake_runner.write_text("""#!/usr/bin/env python3
import json, os, sys
prompt_file, reply_file, timeout = sys.argv[1:4]
runner = os.environ.get("FAKE_RUNNER", "unknown")
ctrl_path = os.environ.get("LTO_TEST_CTRL", "")
cfg = {}
if ctrl_path and os.path.exists(ctrl_path):
    with open(ctrl_path) as f:
        cfg = json.load(f).get(runner, {})
exit_code = cfg.get("exit_code", 0)
output = cfg.get("output", '{"decision":"pick_task","value":"T1","reasoning":"test"}')
with open(reply_file, 'w') as f:
    f.write(output)
sys.exit(exit_code)
""")
        fake_runner.chmod(0o755)

        runners_dir = dpath / "runners"
        runners_dir.mkdir()

        for name in ("codex", "pi", "agy"):
            sh = runners_dir / f"{name}.sh"
            sh.write_text(f'#!/usr/bin/env bash\nexport FAKE_RUNNER="{name}"\nexec python3 "{fake_runner}" "$@"\n')
            sh.chmod(0o755)

        # ── healthcheck — all OK ──
        hc = runners_dir / "healthcheck.sh"
        hc.write_text('#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"},{"agent":"pi","verdict":"OK"},{"agent":"agy","verdict":"OK"}]\'\nexit 0\n')
        hc.chmod(0o755)

        # ── control: codex + pi fail, only agy succeeds → 1 valid ──
        ctrl_path = dpath / "ctrl.json"
        ctrl_fail2 = {
            "codex": {"exit_code": 1, "output": ""},
            "pi": {"exit_code": 1, "output": ""},
            "agy": {"exit_code": 0, "output": '{"decision":"pick_task","value":"T1","reasoning":"only survivor"}'},
        }
        ctrl_path.write_text(json.dumps(ctrl_fail2))
        os.environ["LTO_TEST_CTRL"] = str(ctrl_path)

        facts24 = analyze(st24, repo)
        result24b = run_decision(
            repo, "t24b", facts24, dict(st24),
            decision_kind="direction",
            budget_remaining=100000,
            runners_dir=runners_dir,
        )
        ok(result24b["status"] == "needs_human", "T24a: 1 valid reviewer → needs_human")
        ok("有效异构审者不足" in result24b["brief"], "T24b: brief declares insufficient reviewers")
        ok("1" in result24b["brief"], "T24c: brief states actual count (1)")

    # ═══════════════════════════════════════════════════════════════
    # T25 (B2): run_decision direction happy-path — 3 valid votes → converged
    # ═══════════════════════════════════════════════════════════════

    print("\n[T25] run_decision: direction happy-path 3/3 → converged + majority pick (B2)")
    with tempfile.TemporaryDirectory() as d:
        dpath = Path(d)

        # ── scaffold repo (same shape as T24) ──
        repo = dpath / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

        st25 = {
            "schema_version": 1, "run_id": "t25", "goal": "test_happy_direction",
            "host_runtime": "claude", "current_phase": "impl",
            "tasks": [
                {"id": "T1", "title": "task1", "status": "blocked", "blockers": [{"reason": "fail"}],
                 "command": "echo ok", "touched_files": ["x.py"]},
                {"id": "T2", "title": "task2", "status": "pending",
                 "command": "echo ok2", "touched_files": ["y.py"]},
            ],
            "phase_transitions": [], "user_decisions": [], "blocked_by": "none",
        }

        # ── fake runner: reads runner name from env, config from LTO_TEST_CTRL ──
        fake_runner = dpath / "fake_runner.py"
        fake_runner.write_text("""#!/usr/bin/env python3
import json, os, sys
prompt_file, reply_file, timeout = sys.argv[1:4]
runner = os.environ.get("FAKE_RUNNER", "unknown")
ctrl_path = os.environ.get("LTO_TEST_CTRL", "")
cfg = {}
if ctrl_path and os.path.exists(ctrl_path):
    with open(ctrl_path) as f:
        cfg = json.load(f).get(runner, {})
exit_code = cfg.get("exit_code", 0)
output = cfg.get("output", '{"decision":"pick_task","value":"T1","reasoning":"test"}')
with open(reply_file, 'w') as f:
    f.write(output)
sys.exit(exit_code)
""")
        fake_runner.chmod(0o755)

        runners_dir = dpath / "runners"
        runners_dir.mkdir()
        for name in ("codex", "pi", "agy"):
            sh = runners_dir / f"{name}.sh"
            sh.write_text(f'#!/usr/bin/env bash\nexport FAKE_RUNNER="{name}"\nexec python3 "{fake_runner}" "$@"\n')
            sh.chmod(0o755)

        hc = runners_dir / "healthcheck.sh"
        hc.write_text('#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"},{"agent":"pi","verdict":"OK"},{"agent":"agy","verdict":"OK"}]\'\nexit 0\n')
        hc.chmod(0o755)

        # ── control: all three exit 0 with legal pick_task:T1 JSON → 2/3+ majority ──
        ctrl_path = dpath / "ctrl.json"
        ctrl_all_ok = {
            "codex": {"exit_code": 0, "output": '{"decision":"pick_task","value":"T1","reasoning":"most critical"}'},
            "pi": {"exit_code": 0, "output": '{"decision":"pick_task","value":"T1","reasoning":"agree"}'},
            "agy": {"exit_code": 0, "output": '{"decision":"pick_task","value":"T2","reasoning":"prefer T2"}'},
        }
        ctrl_path.write_text(json.dumps(ctrl_all_ok))
        os.environ["LTO_TEST_CTRL"] = str(ctrl_path)

        facts25 = analyze(st25, repo)
        result25 = run_decision(
            repo, "t25", facts25, dict(st25),
            decision_kind="direction",
            budget_remaining=100000,
            runners_dir=runners_dir,
        )
        ok(result25["status"] == "converged", "T25a: 3 valid votes 2/1 split → converged")
        ok(result25["result"] is not None, "T25b: result payload present")
        ok(result25["result"].get("pick") == "pick_task:T1", "T25c: majority pick = pick_task:T1")
        ok(result25["result"].get("count") == 2, "T25d: majority count = 2")
        ok(result25["result"].get("total") == 3, "T25e: 3 valid voters total")
        ok(result25["dispatched_to"] == ["codex", "pi", "agy"], "T25f: dispatched to all 3 heterogeneous")
        ok("CONVERGED" in result25["brief"].upper(), "T25g: brief declares converged")
        # minority (agy → T2) surfaces in dissent for host judgment
        minority = result25["dissent"].get("minority_votes", [])
        ok(len(minority) == 1 and minority[0]["source"] == "agy", "T25h: agy's T2 vote in minority dissent")

    # ═══════════════════════════════════════════════════════════════
    # T26 (B2): run_decision review happy-path — 3 findings union → converged
    # ═══════════════════════════════════════════════════════════════

    print("\n[T26] run_decision: review happy-path → union-merged findings + converged (B2)")
    with tempfile.TemporaryDirectory() as d:
        dpath = Path(d)

        repo = dpath / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        (repo / ".git" / "HEAD").write_text("ref: refs/heads/main\n")

        st26 = {
            "schema_version": 1, "run_id": "t26", "goal": "test_happy_review",
            "host_runtime": "claude", "current_phase": "impl",
            "tasks": [
                {"id": "T1", "title": "task1", "status": "blocked", "blockers": [{"reason": "fail"}],
                 "command": "echo ok", "touched_files": ["x.py"]},
                {"id": "T2", "title": "task2", "status": "pending",
                 "command": "echo ok2", "touched_files": ["y.py"]},
            ],
            "phase_transitions": [], "user_decisions": [], "blocked_by": "none",
        }

        fake_runner = dpath / "fake_runner.py"
        fake_runner.write_text("""#!/usr/bin/env python3
import json, os, sys
prompt_file, reply_file, timeout = sys.argv[1:4]
runner = os.environ.get("FAKE_RUNNER", "unknown")
ctrl_path = os.environ.get("LTO_TEST_CTRL", "")
cfg = {}
if ctrl_path and os.path.exists(ctrl_path):
    with open(ctrl_path) as f:
        cfg = json.load(f).get(runner, {})
exit_code = cfg.get("exit_code", 0)
output = cfg.get("output", '[]')
with open(reply_file, 'w') as f:
    f.write(output)
sys.exit(exit_code)
""")
        fake_runner.chmod(0o755)

        runners_dir = dpath / "runners"
        runners_dir.mkdir()
        for name in ("codex", "pi", "agy"):
            sh = runners_dir / f"{name}.sh"
            sh.write_text(f'#!/usr/bin/env bash\nexport FAKE_RUNNER="{name}"\nexec python3 "{fake_runner}" "$@"\n')
            sh.chmod(0o755)

        hc = runners_dir / "healthcheck.sh"
        hc.write_text('#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"},{"agent":"pi","verdict":"OK"},{"agent":"agy","verdict":"OK"}]\'\nexit 0\n')
        hc.chmod(0o755)

        # ── control: each runner returns a distinct finding → union of 3, no dedup ──
        ctrl_path = dpath / "ctrl.json"
        ctrl_review = {
            "codex": {"exit_code": 0,
                      "output": json.dumps([{"severity": "critical", "claim": "race condition", "file": "x.py", "evidence_to_check": ""}])},
            "pi": {"exit_code": 0,
                   "output": json.dumps([{"severity": "high", "claim": "no rollback", "file": "y.py", "evidence_to_check": ""}])},
            "agy": {"exit_code": 0,
                    "output": json.dumps([{"severity": "low", "claim": "unregistered risk", "file": "state.json", "evidence_to_check": ""}])},
        }
        ctrl_path.write_text(json.dumps(ctrl_review))
        os.environ["LTO_TEST_CTRL"] = str(ctrl_path)

        facts26 = analyze(st26, repo)
        result26 = run_decision(
            repo, "t26", facts26, dict(st26),
            decision_kind="review",
            budget_remaining=100000,
            runners_dir=runners_dir,
        )
        ok(result26["status"] == "converged", "T26a: review track always converges with enough reviewers")
        ok(isinstance(result26["result"], list), "T26b: review result is a findings list")
        ok(len(result26["result"]) == 3, "T26c: union merge keeps all 3 findings (no dedup)")
        claims = {f["claim"] for f in result26["result"]}
        ok(claims == {"race condition", "no rollback", "unregistered risk"}, "T26d: all 3 distinct claims preserved")
        sources = {f["source"] for f in result26["result"]}
        ok(sources == {"codex", "pi", "agy"}, "T26e: each finding tagged with its source agent")
        ok(result26["dispatched_to"] == ["codex", "pi", "agy"], "T26f: dispatched to all 3 heterogeneous")
        ok("Union Merge" in result26["brief"], "T26g: brief declares union merge")

    print(f"\n{'='*50}")
    print(f"Results: {tests_passed}/{tests_total} passed")
    if tests_passed == tests_total:
        print("DECISION SELFTEST OK")
        return 0
    else:
        print(f"DECISION SELFTEST FAILED ({tests_total - tests_passed} failures)")
        return 1


if __name__ == "__main__":
    import sys
    sys.exit(_run_selftest())
