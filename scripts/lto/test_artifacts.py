#!/usr/bin/env python3
"""Standalone tests for lto.artifacts."""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from lto import artifacts as af
from lto import state as st

FAIL = 0


def ok(condition: bool, label: str, detail: str = "") -> None:
    global FAIL
    if condition:
        print(f"  OK {label}")
    else:
        FAIL += 1
        print(f"  FAIL {label}: {detail}")


def make_repo() -> tuple[Path, str]:
    root = Path(tempfile.mkdtemp(prefix="lto_artifacts_test_"))
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
    return repo, run_id


def cleanup(repo: Path) -> None:
    shutil.rmtree(repo.parent)


def test_init_and_changelog() -> None:
    repo, run_id = make_repo()
    try:
        state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
        manifest = af.init_manifest(repo, run_id, state)
        kinds = {e["kind"] for e in manifest["artifacts"]}
        ok({"state_json", "run_state_md"}.issubset(kinds), "init registers core files")
        (repo / "CHANGELOG.md").write_text("change\n", encoding="utf-8")
        entry = af.register_path(repo, run_id, repo / "CHANGELOG.md", kind="changelog",
                                 producer="test", state=state)
        ok(entry["volatile"] is True, "changelog volatile")
        ok("sha256" not in entry and "bytes" not in entry, "changelog skips hard hash")
    finally:
        cleanup(repo)


def test_path_boundaries() -> None:
    repo, run_id = make_repo()
    try:
        state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
        af.init_manifest(repo, run_id, state)
        good = repo / ".lto" / run_id / "evidence" / "out.txt"
        good.parent.mkdir(parents=True)
        good.write_text("ok", encoding="utf-8")
        af.register_path(repo, run_id, good, kind="evidence_stdout", producer="test", state=state)
        for bad in (repo / "outside.txt", repo / ".lto" / "other" / "x.txt"):
            bad.parent.mkdir(parents=True, exist_ok=True)
            bad.write_text("bad", encoding="utf-8")
            try:
                af.register_path(repo, run_id, bad, kind="evidence_stdout", producer="test", state=state)
                ok(False, f"rejects {bad}")
            except ValueError:
                ok(True, f"rejects {bad.name}")
        try:
            af.write_text(repo, run_id, "../escape.txt", "x", kind="other", producer="test", state=state)
            ok(False, "write_text rejects traversal")
        except ValueError:
            ok(True, "write_text rejects traversal")
    finally:
        cleanup(repo)


def test_decision_record_allowlist() -> None:
    repo, run_id = make_repo()
    try:
        state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
        af.init_manifest(repo, run_id, state)
        decision = repo / "docs" / "decisions" / "2026-06-04-keep-wrapper.md"
        decision.parent.mkdir(parents=True, exist_ok=True)
        decision.write_text("decision\n", encoding="utf-8")
        entry = af.register_path(repo, run_id, decision, kind="decision_record",
                                 producer="test", state=state)
        ok(entry["kind"] == "decision_record", "decision_record kind preserved")
        ok(entry["relative_path"] == "docs/decisions/2026-06-04-keep-wrapper.md",
           "decision relative path is repo-relative")
        ok(entry["run_relative_path"] == entry["relative_path"],
           "decision run_relative_path uses outside-run convention")

        bad = repo / "docs" / "other" / "bad.md"
        bad.parent.mkdir(parents=True, exist_ok=True)
        bad.write_text("bad\n", encoding="utf-8")
        try:
            af.register_path(repo, run_id, bad, kind="decision_record", producer="test", state=state)
            ok(False, "decision_record rejects docs/other")
        except ValueError:
            ok(True, "decision_record rejects docs/other")
    finally:
        cleanup(repo)


def test_dedupe_and_synthesize_no_write() -> None:
    repo, run_id = make_repo()
    try:
        state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
        path = repo / ".lto" / run_id / "evidence" / "T1-stdout.txt"
        path.parent.mkdir(parents=True)
        path.write_text("one", encoding="utf-8")
        af.register_path(repo, run_id, path, kind="evidence_stdout", producer="test",
                         state=state, summary="one")
        manifest = af.load_manifest(repo, run_id, synthesize=False)
        kinds = {e["kind"] for e in manifest["artifacts"]}
        ok({"state_json", "run_state_md", "evidence_stdout"}.issubset(kinds),
           "first register on old run synthesizes existing core files")
        path.write_text("two", encoding="utf-8")
        af.register_path(repo, run_id, path, kind="evidence_stdout", producer="test",
                         state=state, summary="two")
        manifest = af.load_manifest(repo, run_id, synthesize=False)
        matches = [e for e in manifest["artifacts"] if e["relative_path"].endswith("T1-stdout.txt")]
        ok(len(matches) == 1 and matches[0]["summary"] == "two", "dedupe updates entry")

        (repo / ".lto" / run_id / "artifacts.json").unlink()
        state["current_phase"] = "closed"
        manifest = af.load_manifest(repo, run_id, state=state)
        ok(manifest["synthesized"] is True, "synthesized manifest marked")
        ok(not (repo / ".lto" / run_id / "artifacts.json").exists(), "closed synthesize does not write")
        ok(all(e["source"] == "synthesized" for e in manifest["artifacts"]), "entries marked synthesized")
    finally:
        cleanup(repo)


def test_concurrent_registers() -> None:
    repo, run_id = make_repo()
    try:
        state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
        af.init_manifest(repo, run_id, state)

        def write_one(i: int) -> None:
            p = repo / ".lto" / run_id / "evidence" / f"T{i}-stdout.txt"
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(str(i), encoding="utf-8")
            af.register_path(repo, run_id, p, kind="evidence_stdout", producer="test", state=state)

        with ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(write_one, range(20)))
        data = json.loads((repo / ".lto" / run_id / "artifacts.json").read_text(encoding="utf-8"))
        evidence = [e for e in data["artifacts"] if e["kind"] == "evidence_stdout"]
        ok(len(evidence) == 20, "concurrent register keeps all entries", str(len(evidence)))
    finally:
        cleanup(repo)


def main() -> int:
    test_init_and_changelog()
    test_path_boundaries()
    test_decision_record_allowlist()
    test_dedupe_and_synthesize_no_write()
    test_concurrent_registers()
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
