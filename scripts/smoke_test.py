#!/usr/bin/env python3
"""Smoke test for long-task-orchestration skill.

Verifies:
1. SKILL.md parses correctly (frontmatter + required sections)
2. lto_run.py self-test passes
3. Templates are valid and complete
4. eval/queries.json has positive + negative coverage
5. All referenced files exist
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = SKILL_DIR / "scripts"


def check(condition: bool, msg: str) -> int:
    if not condition:
        print(f"FAIL {msg}", file=sys.stderr)
        return 1
    print(f"OK   {msg}")
    return 0


def main() -> int:
    errors = 0

    # 1. Project instructions and SKILL.md parse
    claude_md = SKILL_DIR / "CLAUDE.md"
    claude_txt = ""
    errors += check(claude_md.exists(), "CLAUDE.md exists")
    if claude_md.exists():
        claude_txt = claude_md.read_text(encoding="utf-8")
        errors += check("control harness" in claude_txt, "CLAUDE.md states control harness identity")
        errors += check("Host remains controller-in-chief" in claude_txt, "CLAUDE.md preserves host planner authority")
        errors += check("Run logs are tuning fuel" in claude_txt, "CLAUDE.md covers run telemetry")
        errors += check("Do not build a Cockpit UI" in claude_txt, "CLAUDE.md rejects UI-first drift")

    skill_md = SKILL_DIR / "SKILL.md"
    errors += check(skill_md.exists(), "SKILL.md exists")

    content = skill_md.read_text(encoding="utf-8")
    errors += check(content.startswith("---"), "SKILL.md has YAML frontmatter")
    errors += check("name: long-task-orchestration" in content, "name field present")
    errors += check("tier: agent-driven" in content, "tier field present")
    errors += check("optional_integrations:" in content, "optional_integrations field present")
    errors += check("allowed-tools:" in content, "allowed-tools field present")
    errors += check("model:" not in content[:200], "no model: pin in frontmatter")

    # 2. Required sections (check by content not header numbers)
    errors += check("三个核心原则" in content, "core principles present")
    errors += check("六个阶段" in content, "six phases present")
    errors += check("什么时候停" in content, "three gates present")
    errors += check("让多个 AI 帮你审" in content, "audit section present")
    errors += check("部署上线" in content, "deploy section present")
    errors += check("怎么记" in content, "logging section present")
    errors += check("多轮任务怎么不迷路" in content, "run-state section present")
    errors += check("常见错觉" in content, "anti-pattern section present")
    errors += check("Workload Profile" in content, "Workload Profile present")

    # 3. No stale terms
    stale_terms = ["进阶版", "精神续作", "一律归 ad", "depends_on: [memory-flow]", "debug skill"]
    for term in stale_terms:
        if term in content:
            print(f"FAIL stale term found: {term}", file=sys.stderr)
            errors += 1
        else:
            print(f"OK   no stale term: {term}")

    # 4. lto_run.py self-test
    result = subprocess.run(
        [sys.executable, str(SKILL_DIR / "scripts" / "lto_run.py"), "self-test"],
        capture_output=True, text=True, timeout=30,
    )
    errors += check(result.returncode == 0, f"lto_run.py self-test: {result.stdout.strip().split(chr(10))[-1]}")
    if result.returncode != 0:
        print(result.stderr, file=sys.stderr)

    # 4b. audit_ledger_check.py self-test
    ledger_script = SKILL_DIR / "scripts" / "audit_ledger_check.py"
    errors += check(ledger_script.exists(), "audit_ledger_check.py exists")
    if ledger_script.exists():
        ledger_result = subprocess.run(
            [sys.executable, str(ledger_script), "self-test"],
            capture_output=True, text=True, timeout=30,
        )
        errors += check(
            ledger_result.returncode == 0,
            f"audit_ledger_check.py self-test: {ledger_result.stdout.strip().split(chr(10))[-1]}",
        )
        if ledger_result.returncode != 0:
            print(ledger_result.stderr, file=sys.stderr)

    # 4c. Six load-bearing module self-tests / adversarial tests.
    #     These guard the harness safety logic (scheduler exit-code
    #     judgment, agent spawn, audit dispatch, decision convergence, next
    #     routing, worktree sandbox red-lines). Their tests exist but smoke
    #     never ran them — a P0 chmod bug shipped because of exactly this gap.
    #
    #     TRAP: the four lto.* modules MUST run via `-m <module>` with
    #     PYTHONPATH=scripts. Running `python3 path/to/lto/foo.py` directly
    #     fails ModuleNotFoundError (no 'lto' package on sys.path) → false
    #     green. The two scripts/-level test files self-insert sys.path, so
    #     they run by path. invoke = (cmd_argv, needs_pythonpath).
    module_env = {**os.environ, "PYTHONPATH": str(SCRIPTS_DIR)}
    module_tests = [
        ("scheduler",     [sys.executable, "-m", "lto.scheduler"],        True),
        ("agent_exec",    [sys.executable, "-m", "lto.agent_exec"],       True),
        ("audit",         [sys.executable, "-m", "lto.test_audit"],       True),
        ("decision",      [sys.executable, "-m", "lto.test_decision"],    True),
        ("artifacts",     [sys.executable, "-m", "lto.test_artifacts"],   True),
        ("phase_gate",    [sys.executable, "-m", "lto.test_phase_gate"],  True),
        ("next",          [sys.executable, str(SCRIPTS_DIR / "test_next.py")],              False),
        ("codex_runner",  [sys.executable, str(SCRIPTS_DIR / "test_codex_runner.py")],      False),
        ("pi_runner",     [sys.executable, str(SCRIPTS_DIR / "test_pi_runner.py")],         False),
        ("claude_runner", [sys.executable, str(SCRIPTS_DIR / "test_claude_runner.py")],     False),
        ("token_rollup",  [sys.executable, str(SCRIPTS_DIR / "test_token_rollup.py")],      False),
        ("cross_run_mining", [sys.executable, str(SCRIPTS_DIR / "test_cross_run_mining.py")], False),
        ("plugins",       [sys.executable, str(SCRIPTS_DIR / "test_plugins.py")],           False),
        ("privacy",       [sys.executable, str(SCRIPTS_DIR / "test_privacy_self_check.py")], False),
        ("worktree",      [sys.executable, str(SCRIPTS_DIR / "test_worktree_sandbox.py")],  False),
        ("live_log",      [sys.executable, str(SCRIPTS_DIR / "test_live_log.py")],          False),
        ("events",        [sys.executable, str(SCRIPTS_DIR / "test_events.py")],            False),
        ("telemetry",     [sys.executable, str(SCRIPTS_DIR / "test_telemetry.py")],         False),
        ("autonomous_gate", [sys.executable, str(SCRIPTS_DIR / "test_autonomous_gate.py")], False),
    ]
    for name, argv, needs_pp in module_tests:
        proc = subprocess.run(
            argv,
            cwd=str(SCRIPTS_DIR),
            capture_output=True, text=True, timeout=120,
            env=module_env if needs_pp else os.environ,
        )
        last = (proc.stdout.strip().split(chr(10)) or [""])[-1]
        errors += check(proc.returncode == 0, f"{name} self-test: {last or 'rc=' + str(proc.returncode)}")
        if proc.returncode != 0:
            print(proc.stdout[-2000:], file=sys.stderr)
            print(proc.stderr[-2000:], file=sys.stderr)

    # 4d. High-value orchestration command e2e (task-add / judge / recap)
    cmd_test = SCRIPTS_DIR / "test_orchestration_cmds.py"
    errors += check(cmd_test.exists(), "test_orchestration_cmds.py exists")
    if cmd_test.exists():
        proc = subprocess.run(
            [sys.executable, str(cmd_test)],
            cwd=str(SCRIPTS_DIR),
            capture_output=True, text=True, timeout=120,
        )
        last = (proc.stdout.strip().split(chr(10)) or [""])[-1]
        errors += check(proc.returncode == 0, f"orchestration cmds e2e: {last or 'rc=' + str(proc.returncode)}")
        if proc.returncode != 0:
            print(proc.stdout[-2000:], file=sys.stderr)
            print(proc.stderr[-2000:], file=sys.stderr)

    # 5. Templates exist
    for tmpl in ["run-state.md", "audit-ledger.md"]:
        path = SKILL_DIR / "templates" / tmpl
        errors += check(path.exists(), f"template {tmpl} exists")
        if path.exists():
            txt = path.read_text(encoding="utf-8")
            errors += check(len(txt) > 100, f"template {tmpl} has content ({len(txt)} chars)")

    # 6. eval/queries.json
    eval_path = SKILL_DIR / "eval" / "queries.json"
    errors += check(eval_path.exists(), "eval/queries.json exists")
    if eval_path.exists():
        queries = json.loads(eval_path.read_text(encoding="utf-8"))
        pos = [q for q in queries if q["should_trigger"]]
        neg = [q for q in queries if not q["should_trigger"]]
        errors += check(len(pos) >= 3, f"positive queries: {len(pos)} (need ≥3)")
        errors += check(len(neg) >= 2, f"negative queries: {len(neg)} (need ≥2)")

    # 7. References exist
    for ref in [
        "audit-convergence.md", "codex-cli-control.md", "control-loop-harness.md",
        "cross-runtime-host-notes.md", "decision-logging.md", "deploy-sequencing.md",
        "plugin-boundary.md", "plugin-real-eval-runner.md", "privacy-self-check.md",
        "engineering-map.md", "long-loop-state.md",
        "onboarding.md", "run-state-workflow.md", "sharing-guide.md",
        "validation-log.md", "workflow-playbook.md",
    ]:
        path = SKILL_DIR / "references" / ref
        errors += check(path.exists(), f"reference {ref} exists")

    # 8. References are non-empty (agent-driven skill routes refs via SKILL.md prose,
    #    not via a filename checklist; deep-detail refs need not be named verbatim)
    min_lines = 20
    for ref in [
        "audit-convergence.md", "codex-cli-control.md", "control-loop-harness.md",
        "cross-runtime-host-notes.md", "decision-logging.md", "deploy-sequencing.md",
        "plugin-boundary.md", "plugin-real-eval-runner.md", "privacy-self-check.md",
        "engineering-map.md", "long-loop-state.md",
        "onboarding.md", "run-state-workflow.md", "sharing-guide.md",
        "validation-log.md", "workflow-playbook.md",
    ]:
        path = SKILL_DIR / "references" / ref
        if path.exists():
            n_lines = len(path.read_text(encoding="utf-8").splitlines())
            errors += check(n_lines >= min_lines, f"reference {ref} has substance ({n_lines} lines)")

    # 9. Doc/code consistency lint
    md_files = list(SKILL_DIR.glob("*.md")) + list((SKILL_DIR / "references").glob("*.md"))
    stale_path = "skills/long-task-orchestration/scripts"
    for md_file in md_files:
        if md_file.exists():
            text = md_file.read_text(encoding="utf-8")
            count = text.count(stale_path)
            errors += check(
                count == 0,
                f"doc {md_file.relative_to(SKILL_DIR)} is clean of stale path '{stale_path}' (found {count} occurrences)",
            )

    readme = (SKILL_DIR / "README.md").read_text(encoding="utf-8")
    errors += check('L="python3 scripts/lto_run.py"' in readme, "README quickstart uses standalone lto_run.py path")
    errors += check((SCRIPTS_DIR / "lto_run.py").exists(), "README quickstart script exists")
    errors += check((SCRIPTS_DIR / "delegate" / "triad.sh").exists(), "bundled triad.sh exists")
    errors += check((SCRIPTS_DIR / "delegate" / "runners" / "healthcheck.sh").exists(), "bundled delegate runners exist")

    help_result = subprocess.run(
        [sys.executable, str(SCRIPTS_DIR / "lto_run.py"), "--help"],
        capture_output=True, text=True, timeout=10,
    )
    match = re.search(r"\{([^}]+)\}", help_result.stdout)
    actual_cmds = [x.strip() for x in match.group(1).split(",")] if match else []
    claimed = re.search(r"(\d+)\s*命令薄入口", content)
    if claimed:
        errors += check(int(claimed.group(1)) == len(actual_cmds), f"SKILL.md command count matches help ({len(actual_cmds)})")

    plugin_help = subprocess.run(
        [sys.executable, str(SCRIPTS_DIR / "lto_run.py"), "plugin", "--help"],
        capture_output=True, text=True, timeout=10,
    ).stdout
    errors += check("eval-run" in plugin_help, "plugin eval-run is advertised (v0 implemented)")
    real_eval_doc = (SKILL_DIR / "references" / "plugin-real-eval-runner.md").read_text(encoding="utf-8")
    errors += check("v0 implemented" in real_eval_doc, "plugin eval-run doc marks v0 implemented")
    errors += check("judge layer implemented" in real_eval_doc, "plugin eval-run doc marks judge layer implemented")
    # judge + frozen evidence hash 已兑现 → DEFERRED_V0 只剩 automatic_promotion
    if str(SCRIPTS_DIR) not in sys.path:
        sys.path.insert(0, str(SCRIPTS_DIR))
    from lto.plugin_eval_run import DEFERRED_V0
    errors += check(DEFERRED_V0 == ["automatic_promotion"], "eval-run deferred only automatic_promotion remains")

    control_doc = (SKILL_DIR / "references" / "control-loop-harness.md").read_text(encoding="utf-8")
    # Phase 1 sensor layer is implemented (2026-06-09); later phases stay spec.
    errors += check("Phase 1 passive logging implemented" in control_doc, "control-loop harness doc marks Phase 1 implemented")
    errors += check("Phase 1 sensor layer is implemented" in claude_txt, "CLAUDE.md marks events/telemetry Phase 1 implemented")

    # Summary
    if errors == 0:
        print("\nSMOKE OK")
        return 0
    print(f"\n{errors} FAILURES", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
