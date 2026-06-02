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
import re
import subprocess
import sys
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parent.parent


def check(condition: bool, msg: str) -> int:
    if not condition:
        print(f"FAIL {msg}", file=sys.stderr)
        return 1
    print(f"OK   {msg}")
    return 0


def main() -> int:
    errors = 0

    # 1. SKILL.md parses
    skill_md = SKILL_DIR / "SKILL.md"
    errors += check(skill_md.exists(), "SKILL.md exists")

    content = skill_md.read_text(encoding="utf-8")
    errors += check(content.startswith("---"), "SKILL.md has YAML frontmatter")
    errors += check("name: long-task-orchestration" in content, "name field present")
    errors += check("tier: agent-driven" in content, "tier field present")
    errors += check("optional_integrations:" in content, "optional_integrations field present")
    errors += check("allowed-tools:" in content, "allowed-tools field present")
    errors += check("model:" not in content[:200], "no model: pin in frontmatter")

    # 2. Required sections
    errors += check("## 0. 防归因偏差" in content, "§0 present")
    errors += check("## 1. 主循环骨架" in content, "§1 present")
    errors += check("## 2. 反过度设计" in content, "§2 present")
    errors += check("## 3. 异构审计" in content, "§3 present")
    errors += check("## 4. 部署严格" in content, "§4 present")
    errors += check("## 5. 可选集成" in content, "§5 present")
    errors += check("## 6. 与 agent-delegate" in content, "§6 present")
    errors += check("## 7. 谁当宿主" in content, "§7 present")
    errors += check("## 反模式" in content, "anti-pattern section present")
    errors += check("## Workload Profile" in content, "Workload Profile present")

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

    # 5. Templates exist
    for tmpl in ["run-state.md", "preflight.md", "audit-ledger.md"]:
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
        "audit-convergence.md", "cross-runtime-host-notes.md",
        "decision-logging.md", "deploy-sequencing.md",
        "long-loop-state.md", "run-state-workflow.md",
        "sharing-guide.md", "validation-log.md",
    ]:
        path = SKILL_DIR / "references" / ref
        errors += check(path.exists(), f"reference {ref} exists")

    # 8. No orphan references (every ref mentioned in SKILL.md or another ref)
    refs_mentioned = set()
    for fpath in SKILL_DIR.glob("**/*.md"):
        txt = fpath.read_text(encoding="utf-8")
        for ref in ["audit-convergence", "cross-runtime-host-notes", "decision-logging",
                     "deploy-sequencing", "long-loop-state", "run-state-workflow",
                     "sharing-guide", "validation-log"]:
            if ref in txt:
                refs_mentioned.add(ref)
    all_refs = {"audit-convergence", "cross-runtime-host-notes", "decision-logging",
                "deploy-sequencing", "long-loop-state", "run-state-workflow",
                "sharing-guide", "validation-log"}
    orphans = all_refs - refs_mentioned
    errors += check(len(orphans) == 0, f"no orphan references: {orphans or 'none'}")

    # Summary
    if errors == 0:
        print("\nSMOKE OK")
        return 0
    print(f"\n{errors} FAILURES", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
