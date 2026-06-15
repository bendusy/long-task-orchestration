#!/usr/bin/env python3
"""Tests for scripts/privacy_self_check.sh."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "privacy_self_check.sh"


def ok(condition: bool, message: str) -> int:
    if condition:
        print(f"OK   {message}")
        return 0
    print(f"FAIL {message}", file=sys.stderr)
    return 1


def run(cmd: list[str], *, env: dict[str, str], input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, text=True, input=input_text, capture_output=True, env=env, timeout=60)


def main() -> int:
    errors = 0
    errors += ok(SCRIPT.exists(), "privacy script exists")

    with tempfile.TemporaryDirectory(prefix="lto_privacy_test_") as td:
        base = Path(td)
        repo = base / "repo"
        home = base / "home"
        repo.mkdir()
        home.mkdir()
        subprocess.run(["git", "init"], cwd=repo, check=True, stdout=subprocess.DEVNULL)
        (repo / ".gitignore").write_text(".lto/\n", encoding="utf-8")
        (repo / ".claude").mkdir()
        (repo / ".claude" / "local.jsonl").write_text("secret transcript\n", encoding="utf-8")
        (repo / ".lto").mkdir()
        (repo / ".lto" / "state.json").write_text("{}\n", encoding="utf-8")
        (home / ".claude" / "feedback-bundles").mkdir(parents=True)
        (home / ".claude" / "feedback-bundles" / "bundle.jsonl").write_text("feedback\n", encoding="utf-8")

        env = {
            **os.environ,
            "PRIVACY_CHECK_HOME": str(home),
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY": "1",
            "DISABLE_TELEMETRY": "1",
            "DO_NOT_TRACK": "1",
            "CLAUDE_CODE_SKIP_PROMPT_HISTORY": "1",
        }

        dry = run([str(SCRIPT), "--repo", str(repo), "--no-gitleaks"], env=env)
        errors += ok(dry.returncode == 0, f"dry-run exits 0 (got {dry.returncode})")
        errors += ok("dry-run mode" in dry.stdout, "dry-run mode announced")
        errors += ok((repo / ".claude").exists(), "dry-run keeps repo .claude")
        errors += ok((home / ".claude" / "feedback-bundles").exists(), "dry-run keeps feedback bundles")
        errors += ok("CANDIDATE" in dry.stdout, "cleanup candidates reported")

        delete = run(
            [str(SCRIPT), "--repo", str(repo), "--no-gitleaks", "--delete"],
            env=env,
            input_text="no\ndelete\nno\n",
        )
        errors += ok(delete.returncode == 0, f"delete mode exits 0 (got {delete.returncode})")
        errors += ok("Type exactly \"delete\"" in delete.stdout, "delete prompt requires exact token")
        errors += ok((home / ".claude" / "feedback-bundles").exists(), "non-delete answer skips first candidate")
        errors += ok(not (repo / ".claude").exists(), "exact delete removes one repo-local candidate")
        errors += ok((repo / ".lto").exists(), "later non-delete answer keeps next candidate")

    with tempfile.TemporaryDirectory(prefix="lto_privacy_classify_test_") as td:
        base = Path(td)
        repo = base / "repo"
        home = base / "home"
        repo.mkdir()
        home.mkdir()
        subprocess.run(["git", "init"], cwd=repo, check=True, stdout=subprocess.DEVNULL)
        (repo / ".gitignore").write_text(".lto/\n", encoding="utf-8")
        (repo / "scripts").mkdir()
        (repo / "scripts" / "test_redaction.py").write_text(
            'fake = "sk-ant-abcdefghijkl1234567890"\n',
            encoding="utf-8",
        )

        env = {
            **os.environ,
            "PRIVACY_CHECK_HOME": str(home),
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY": "1",
            "DISABLE_TELEMETRY": "1",
            "DO_NOT_TRACK": "1",
            "CLAUDE_CODE_SKIP_PROMPT_HISTORY": "1",
        }

        strict = run([str(SCRIPT), "--repo", str(repo), "--strict", "--no-gitleaks"], env=env)
        errors += ok(strict.returncode == 0, f"classified test fixture strict scan exits 0 (got {strict.returncode})")
        errors += ok("classified regex test fixture" in strict.stdout, "test fixture hit is classified")
        errors += ok("findings=0" in strict.stdout, "classified fixture does not count as finding")

    if errors == 0:
        print("PRIVACY SELF-CHECK TESTS OK")
        return 0
    print(f"{errors} FAILURES", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
