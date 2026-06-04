#!/usr/bin/env python3
"""Standalone tests for write_decision.py."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
WRITE_DECISION = SCRIPT_DIR / "write_decision.py"
sys.path.insert(0, str(SCRIPT_DIR))

from lto import artifacts as af  # noqa: E402
from lto import state as st  # noqa: E402

FAIL = 0


def ok(condition: bool, label: str, detail: str = "") -> None:
    global FAIL
    if condition:
        print(f"  OK {label}")
    else:
        FAIL += 1
        print(f"  FAIL {label}: {detail}", file=sys.stderr)


def make_repo() -> tuple[Path, str]:
    root = Path(tempfile.mkdtemp(prefix="lto_decision_test_"))
    repo = root / "repo"
    repo.mkdir()
    run_id = "r1"
    run_dir = repo / ".lto" / run_id
    run_dir.mkdir(parents=True)
    state = st.default_state("goal", "codex", str(repo), "request", "spec", "HEAD", "main", "", "")
    state["run_id"] = run_id
    state["artifacts"] = {"manifest": f".lto/{run_id}/artifacts.json"}
    st.save_state(run_dir / "state.json", state)
    (run_dir / "run-state.md").write_text("run state\n", encoding="utf-8")
    af.init_manifest(repo, run_id, state)
    return repo, run_id


def cleanup(repo: Path) -> None:
    shutil.rmtree(repo.parent)


def run_helper(repo: Path, run_id: str, *extra: str) -> subprocess.CompletedProcess:
    base = [
        sys.executable, str(WRITE_DECISION),
        "--repo", str(repo),
        "--run-id", run_id,
        "--title", "Keep wrapper opt in",
        "--context", "global wrappers can collide",
        "--decision", "install only a managed wrapper",
        "--consequences", "users reinstall after moving the repo",
    ]
    return subprocess.run([*base, *extra], cwd=repo, capture_output=True, text=True)


def test_success_and_registration() -> None:
    repo, run_id = make_repo()
    try:
        proc = run_helper(repo, run_id, "--slug", "keep-wrapper")
        ok(proc.returncode == 0, "helper rc=0", proc.stderr.strip())
        rel_path = proc.stdout.strip()
        ok(rel_path.startswith("docs/decisions/") and rel_path.endswith("keep-wrapper.md"),
           "prints decision path", rel_path)
        ok((repo / rel_path).exists(), "ADR file exists")

        state = json.loads((repo / ".lto" / run_id / "state.json").read_text(encoding="utf-8"))
        ok(state["user_decisions"][-1]["path"] == rel_path, "state user_decisions updated")
        manifest = af.load_manifest(repo, run_id, synthesize=False)
        entries = [e for e in manifest["artifacts"] if e["kind"] == "decision_record"]
        ok(len(entries) == 1 and entries[0]["relative_path"] == rel_path,
           "decision_record registered")

        dup = run_helper(repo, run_id, "--slug", "keep-wrapper")
        ok(dup.returncode != 0 and "already exists" in dup.stderr,
           "duplicate slug rejected", dup.stderr.strip())
    finally:
        cleanup(repo)


def test_bad_slug_and_missing_state() -> None:
    repo, run_id = make_repo()
    try:
        bad = run_helper(repo, run_id, "--slug", "../bad")
        ok(bad.returncode != 0 and "path traversal" in bad.stderr,
           "path traversal slug rejected", bad.stderr.strip())

        empty = subprocess.run([
            sys.executable, str(WRITE_DECISION),
            "--repo", str(repo),
            "--run-id", run_id,
            "--title", "纯中文标题",
            "--context", "ctx",
            "--decision", "dec",
            "--consequences", "cons",
        ], cwd=repo, capture_output=True, text=True)
        ok(empty.returncode != 0 and "slug is empty after normalization" in empty.stderr,
           "empty normalized slug rejected", empty.stderr.strip())

        missing = run_helper(repo, "missing-run", "--slug", "missing-state")
        ok(missing.returncode == 0 and "without LTO registration" in missing.stderr,
           "missing state writes ADR with warning", missing.stderr.strip())
        ok((repo / missing.stdout.strip()).exists(), "missing-state ADR exists")
    finally:
        cleanup(repo)


def main() -> int:
    test_success_and_registration()
    test_bad_slug_and_missing_state()
    return 1 if FAIL else 0


if __name__ == "__main__":
    raise SystemExit(main())
