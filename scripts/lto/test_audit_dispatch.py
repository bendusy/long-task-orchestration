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

    return c.passed, c.total
