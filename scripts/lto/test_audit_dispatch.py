"""Audit dispatch and risk-discovery integration self-tests."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

from lto import agent_exec
from lto import state as st
from lto.agent_job import AgentJob, Budget, Pattern
from lto.auditors import _parse_structured_reply, _pick_auditors, readonly_intent_policy
from lto.commands.audit import _build_brief, _discover_risks, _do_collect, _is_high_risk


class _Counter:
    def __init__(self) -> None:
        self.passed = 0
        self.total = 0

    def ok(self, cond: bool, label: str) -> None:
        self.total += 1
        if cond:
            self.passed += 1
            print(f"  OK {label}")
        else:
            print(f"  FAIL {label}")


def _git_repo(root: Path) -> Path:
    repo = root / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.name=T", "-c", "user.email=t@x.com",
         "commit", "-q", "--allow-empty", "-m", "init"],
        cwd=repo, capture_output=True,
    )
    return repo


def _runner_dir(root: Path, reply: str, names: tuple[str, ...] = ("codex", "pi", "agy")) -> Path:
    runners = root / "runners"
    runners.mkdir()
    fake = root / "fake_runner.py"
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "reply_file = sys.argv[2]\n"
        f"open(reply_file, 'w').write({reply!r})\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    for name in names:
        sh = runners / f"{name}.sh"
        sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8")
        sh.chmod(0o755)
    hc = runners / "healthcheck.sh"
    verdicts = ",".join(f'{{"agent":"{name}","verdict":"OK"}}' for name in names)
    hc.write_text(f"#!/usr/bin/env bash\necho '[{verdicts}]'\nexit 0\n", encoding="utf-8")
    hc.chmod(0o755)
    return runners


def _state_dir(repo: Path, run_id: str, state: dict) -> Path:
    state_dir = repo / ".lto" / run_id
    state_dir.mkdir(parents=True)
    st.save_state(state_dir / "state.json", state)
    (repo / ".lto" / "current").write_text(run_id + "\n", encoding="utf-8")
    return state_dir


def _risk_state(run_id: str) -> dict:
    return {
        "schema_version": 1,
        "run_id": run_id,
        "goal": "risk discovery",
        "host_runtime": "codex",
        "current_phase": "implementation",
        "tasks": [{"id": "T1", "title": "database migration", "touched_files": ["m.py"]}],
        "risk_points": [],
        "phase_transitions": [],
        "user_decisions": [],
        "blocked_by": "none",
    }


def _run_discover_case(reply: str) -> tuple[int, list[dict]]:
    tmp = Path(tempfile.mkdtemp(prefix="lto_audit_risk_"))
    try:
        repo = _git_repo(tmp)
        runners = _runner_dir(tmp, reply, names=("pi",))
        run_id = "risk-run"
        state = _risk_state(run_id)
        state_dir = _state_dir(repo, run_id, state)
        rc = _discover_risks(
            repo, run_id, state_dir, state, argparse.Namespace(discover_risks=True),
            _runners_dir=runners,
        )
        reloaded = st.load_state(state_dir / "state.json") or {}
        return rc, reloaded.get("risk_points", [])
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _run_discover_failover_case(
    *, unhealthy: tuple[str, ...], good_reply: str,
) -> tuple[int, list[dict], str | None]:
    """Drive _discover_risks with host=claude and a runner pool where some
    candidates are unhealthy. Returns (rc, risks, chosen_runner).

    healthcheck.sh reports every runner OK *except* those in ``unhealthy``.
    Each runner touches ``<name>.called`` when actually dispatched, so the
    test can assert which runner was chosen (fallthrough target).
    """
    tmp = Path(tempfile.mkdtemp(prefix="lto_audit_failover_"))
    try:
        repo = _git_repo(tmp)
        runners = tmp / "runners"
        runners.mkdir()
        called_dir = tmp / "called"
        called_dir.mkdir()
        fake = tmp / "fake_runner.py"
        fake.write_text(
            "#!/usr/bin/env python3\n"
            "import sys\n"
            "reply_file = sys.argv[2]\n"
            f"open(reply_file, 'w').write({good_reply!r})\n"
            "sys.exit(0)\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        for name in ("codex", "pi", "agy"):
            sh = runners / f"{name}.sh"
            sh.write_text(
                "#!/usr/bin/env bash\n"
                f'touch "{called_dir}/{name}.called"\n'
                f'exec python3 "{fake}" "$@"\n',
                encoding="utf-8",
            )
            sh.chmod(0o755)
        # healthcheck: OK for all but the unhealthy set.
        verdicts = ",".join(
            '{{"agent":"{n}","verdict":"{v}"}}'.format(
                n=n, v=("ERROR" if n in unhealthy else "OK"))
            for n in ("codex", "pi", "agy")
        )
        hc = runners / "healthcheck.sh"
        hc.write_text(f"#!/usr/bin/env bash\necho '[{verdicts}]'\nexit 0\n", encoding="utf-8")
        hc.chmod(0o755)

        run_id = "failover-run"
        state = _risk_state(run_id)
        state["host_runtime"] = "claude"  # _pick_auditors → [codex, pi, agy]
        state_dir = _state_dir(repo, run_id, state)
        rc = _discover_risks(
            repo, run_id, state_dir, state, argparse.Namespace(discover_risks=True),
            _runners_dir=runners,
        )
        reloaded = st.load_state(state_dir / "state.json") or {}
        chosen = None
        for name in ("codex", "pi", "agy"):
            if (called_dir / f"{name}.called").exists():
                chosen = name
                break
        return rc, reloaded.get("risk_points", []), chosen
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def run() -> tuple[int, int]:
    c = _Counter()

    print("\n[S6] auto-dispatch with fake runners")
    tmp = Path(tempfile.mkdtemp(prefix="lto_audit_dispatch_"))
    try:
        repo = _git_repo(tmp)
        reply = json.dumps([
            {"severity": "high", "claim": "auth issue", "file": "auth.py"},
            {"severity": "medium", "claim": "logging gap", "file": "log.py"},
        ])
        runners = _runner_dir(tmp, reply)
        run_id = "dispatch-run"
        state = {
            "schema_version": 1,
            "run_id": run_id,
            "goal": "audit selftest",
            "host_runtime": "claude",
            "current_phase": "audit",
            "tasks": [{"id": "T1", "title": "auth migration", "status": "done",
                       "touched_files": ["auth.py"], "commands_run": ["git commit"]}],
            "phase_transitions": [],
            "user_decisions": [],
            "blocked_by": "none",
        }
        state_dir = _state_dir(repo, run_id, state)
        audit_dir = state_dir / "audit"
        audit_dir.mkdir()
        targets = [t for t in state["tasks"] if _is_high_risk(t)]
        brief = audit_dir / "audit-brief-selftest.md"
        brief.write_text(_build_brief(state, targets), encoding="utf-8")
        schema = {"type": "array", "items": {"type": "object"}}
        jobs = [
            AgentJob(
                job_id=f"audit-{a}",
                prompt_ref=str(brief),
                runner=a,
                output_schema=schema,
                budget=Budget(timeout_sec=30),
                parent_pattern=Pattern.ADVERSARIAL.value,
                # match production (audit.py): per-runner read-only intent —
                # agy/gemini get workspace-write (their lowest enforceable), not
                # a bare read-only that fail-closes since W4.
                permission_policy=readonly_intent_policy(a),
            )
            for a in _pick_auditors("claude")
        ]
        results = agent_exec.spawn_agents(repo, run_id, jobs, persist=False, runners_dir=runners)
        c.ok(len(results) == 3 and all(r.ok for r in results), "S6a all auditors OK")
        replies = audit_dir / "replies"
        replies.mkdir()
        for job, result in zip(jobs, results):
            (replies / f"reply-{job.runner}.md").write_text(result.reply_text, encoding="utf-8")
        c.ok(all(_parse_structured_reply(p) is not None for p in replies.iterdir()),
             "S6b replies parse as structured findings")
        rc = _do_collect(repo, run_id, state_dir, state, reply_dir=replies)
        c.ok(rc == 0, f"S6c collect returns 0 (got {rc})")
        ledger = state_dir / "audit-ledger.md"
        c.ok(ledger.exists() and "| R1 |" in ledger.read_text(encoding="utf-8"),
             "S6d ledger R1 row written")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("\n[S8] risk discovery writes risks")
    risks_reply = json.dumps([
        {"claim": "no rollback", "evidence_to_check": "m.py:42", "severity": "high"},
        {"claim": "unguarded route", "evidence_to_check": "r.py:15", "severity": "critical"},
    ])
    rc, risks = _run_discover_case(risks_reply)
    c.ok(rc == 0, f"S8a discover risks returns 0 (got {rc})")
    c.ok(len(risks) == 2, f"S8b two risk points added (got {len(risks)})")
    c.ok(all(r.get("source") == "risk-agent" for r in risks), "S8c source set")

    print("\n[S8-empty] risk discovery accepts empty arrays")
    for label, reply in (("bare", "[]"), ("spaced", "[  ]"), ("fenced", "```json\n[]\n```")):
        rc_empty, risks_empty = _run_discover_case(reply)
        c.ok(rc_empty == 0 and risks_empty == [],
             f"S8-empty {label}: zero risks succeeds without state writes")

    print("\n[S9] risk discovery non-JSON degrades gracefully")
    rc_bad, risks_bad = _run_discover_case("No risks here.")
    c.ok(rc_bad != 0, f"S9a non-JSON returns non-zero (got {rc_bad})")
    c.ok(risks_bad == [], "S9b no risk points added on bad reply")

    print("\n[S10] risk discovery falls through unhealthy discoverer to next runner")
    rc_fo, risks_fo, chosen_fo = _run_discover_failover_case(
        unhealthy=("codex",), good_reply=json.dumps([
            {"claim": "ssrf in fetch", "evidence_to_check": "f.py:9", "severity": "critical"},
        ]),
    )
    # host=claude → _pick_auditors=[codex,pi,agy]; codex unhealthy must NOT be
    # the discoverer (old bug: auditors[0]=codex, single-point fail, rc=2, no risks).
    c.ok(rc_fo == 0, f"S10a fallthrough returns 0 (got {rc_fo})")
    c.ok(len(risks_fo) == 1, f"S10b risk from healthy fallback runner (got {len(risks_fo)})")
    c.ok(chosen_fo == "pi", f"S10c discoverer fell through to pi, not codex (got {chosen_fo})")

    print("\n[S10-all-unhealthy] risk discovery skips when no healthy heterogeneous runner")
    rc_none, risks_none, _ = _run_discover_failover_case(
        unhealthy=("codex", "pi", "agy"), good_reply="[]",
    )
    c.ok(rc_none == 1, f"S10d all-unhealthy skips with rc=1 (got {rc_none})")
    c.ok(risks_none == [], "S10e no risks written when discovery skipped")

    return c.passed, c.total
