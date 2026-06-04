#!/usr/bin/env python3
"""Standalone tests for lto check --to phase evidence."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent.parent
LTO_PATH = SCRIPT_DIR / "lto_run.py"
FAIL: list[str] = []


def ok(condition: bool, msg: str, detail: str = "") -> None:
    if condition:
        print(f"OK   {msg}")
        return
    FAIL.append(msg)
    suffix = f": {detail}" if detail else ""
    print(f"FAIL {msg}{suffix}", file=sys.stderr)


def make_repo(tmp: Path) -> Path:
    repo = tmp
    subprocess.run(["git", "init"], cwd=repo, capture_output=True, check=True)
    (repo / "README.md").write_text("phase gate test\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=repo, capture_output=True, check=True)
    subprocess.run(
        ["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid",
         "commit", "-m", "init"],
        cwd=repo, capture_output=True, check=True,
    )
    return repo


def lto(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(LTO_PATH), "--repo", str(repo), *args],
        cwd=repo, capture_output=True, text=True,
    )


def start(repo: Path, run_id: str, phase: str = "audit", profile: str = "minimal") -> None:
    proc = lto(
        repo, "start", "--run-id", run_id, "--goal", run_id,
        "--host", "codex", "--phase", phase, "--profile", profile, "--force",
    )
    ok(proc.returncode == 0, f"{run_id}: start rc=0", proc.stderr.strip()[:160])


def state_path(repo: Path, run_id: str) -> Path:
    return repo / ".lto" / run_id / "state.json"


def load_state(repo: Path, run_id: str) -> dict:
    return json.loads(state_path(repo, run_id).read_text(encoding="utf-8"))


def save_state(repo: Path, run_id: str, state: dict) -> None:
    state_path(repo, run_id).write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def write_ledger(repo: Path, run_id: str, *rows: str) -> None:
    body = "\n".join([
        "# LTO Audit Ledger Template",
        "",
        "## Round Summary",
        "",
        "| round | artifact | auditors | high | critical | minor | trend | status |",
        "|---|---|---|---:|---:|---:|---|---|",
        *rows,
        "",
    ])
    (repo / ".lto" / run_id / "audit-ledger.md").write_text(body, encoding="utf-8")


def commit_file(repo: Path, rel_path: str, content: str, message: str) -> str:
    path = repo / rel_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    subprocess.run(["git", "add", rel_path], cwd=repo, capture_output=True, check=True)
    subprocess.run(
        ["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid",
         "commit", "-m", message],
        cwd=repo, capture_output=True, check=True,
    )
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


def set_done_task(repo: Path, run_id: str, touched_files: list[str]) -> None:
    state = load_state(repo, run_id)
    state["tasks"] = [{
        "id": "T1",
        "title": "done task",
        "status": "done",
        "phase": state.get("current_phase", "implementation"),
        "depends_on": [],
        "last_update": state.get("started_at", ""),
        "touched_files": touched_files,
        "commands_run": [],
        "evidence": [],
        "blockers": [],
        "assumptions": [],
        "retry_count": 0,
    }]
    save_state(repo, run_id, state)


def test_advisory_implementation(repo: Path) -> None:
    run_id = "phase-advisory"
    start(repo, run_id, phase="audit")
    state = load_state(repo, run_id)
    state["gates"]["unresolved_blocks"] = [{"id": "B1", "claim": "needs human review"}]
    save_state(repo, run_id, state)

    proc = lto(repo, "check", "--run-id", run_id, "--to", "implementation")
    ok(proc.returncode == 0, "implementation advisory rc=0", proc.stderr.strip()[:160])
    ok("LTO Phase Evidence: implementation" in proc.stdout, "implementation text report present")
    ok("MISSING required no_unresolved_blocks" in proc.stdout, "required missing printed but advisory")
    ok("human_gate_required: true" in proc.stdout, "human gate marker printed")


def test_json_is_clean(repo: Path) -> None:
    run_id = "phase-json"
    start(repo, run_id, phase="audit", profile="audit")
    proc = lto(repo, "check", "--run-id", run_id, "--to", "implementation", "--json")
    ok(proc.returncode == 0, "json mode rc=0", proc.stderr.strip()[:160])
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        ok(False, "json stdout parses", f"{exc}: {proc.stdout!r}")
        return
    ok(proc.stdout.lstrip().startswith("{") and "ledger:" not in proc.stdout, "json stdout has no prose")
    ok(proc.stderr == "", "json mode keeps stderr quiet", proc.stderr.strip()[:160])
    ok(data["target_phase"] == "implementation", "json target_phase present")
    ok(data["human_gate_required"] is True, "json human_gate_required true")
    ok("check" in data and "warnings" in data["check"], "json embeds base check result")


def test_strict_nonconverged_ledger(repo: Path) -> None:
    run_id = "phase-strict-ledger"
    start(repo, run_id, phase="audit", profile="audit")
    write_ledger(repo, run_id, "| R1 | spec | pi agy | 1 | 0 | 0 | start | open |")
    proc = lto(repo, "check", "--run-id", run_id, "--to", "implementation", "--strict")
    ok(proc.returncode != 0, "strict non-converged ledger rc!=0")
    text = proc.stdout + proc.stderr
    ok("phase evidence missing: audit_ledger_converged_if_present" in text,
       "strict non-converged ledger is required missing", text[-300:])


def test_argparse_rejects_other_phases(repo: Path) -> None:
    start(repo, "phase-argparse")
    proc = lto(repo, "check", "--run-id", "phase-argparse", "--to", "spec")
    ok(proc.returncode != 0, "argparse rejects --to spec")
    ok("invalid choice" in proc.stderr and "implementation" in proc.stderr,
       "argparse error names allowed choices", proc.stderr.strip()[:200])


def test_closed_task_gate(repo: Path) -> None:
    run_id = "phase-closed"
    start(repo, run_id, phase="implementation")
    proc = lto(repo, "task-add", "--run-id", run_id, "--task-id", "T1", "--title", "open task")
    ok(proc.returncode == 0, "closed gate: task-add rc=0", proc.stderr.strip()[:160])
    proc = lto(repo, "check", "--run-id", run_id, "--to", "closed", "--strict")
    ok(proc.returncode != 0, "closed strict fails with open task")
    ok("phase evidence missing: no_open_tasks" in (proc.stdout + proc.stderr),
       "closed strict reports open task", (proc.stdout + proc.stderr)[-300:])

    state = load_state(repo, run_id)
    state["tasks"][0]["status"] = "done"
    state["risk_points"] = [{
        "id": "RP1", "source": "test", "claim": "covered",
        "evidence_to_check": "state", "verified_by": "test", "disposition": "open",
    }]
    save_state(repo, run_id, state)
    proc = lto(repo, "check", "--run-id", run_id, "--to", "closed", "--strict")
    ok(proc.returncode == 0, "closed strict passes after task done and risk verified",
       (proc.stdout + proc.stderr)[-300:])
    ok("OK required no_open_tasks" in proc.stdout, "closed report shows no open tasks")


def test_forward_related_file_drift(repo: Path) -> None:
    commit_file(repo, "src/drift.txt", "one\n", "add drift file")
    run_id = "phase-drift-related"
    start(repo, run_id, phase="implementation")
    set_done_task(repo, run_id, ["src/drift.txt"])
    commit_file(repo, "src/drift.txt", "two\n", "change drift file")

    proc = lto(repo, "check", "--run-id", run_id)
    ok(proc.returncode == 0, "related drift advisory rc=0", proc.stderr.strip()[:200])
    ok("related task files changed since recorded HEAD" in proc.stderr,
       "related drift warning printed", proc.stderr.strip()[-300:])

    proc = lto(repo, "check", "--run-id", run_id, "--strict")
    ok(proc.returncode != 0, "related drift strict rc!=0")
    ok("related task files changed since recorded HEAD" in proc.stderr,
       "related drift strict error printed", proc.stderr.strip()[-300:])

    proc = lto(repo, "resume", "--run-id", run_id)
    ok(proc.returncode == 2, "resume marks related drift for revalidation", proc.stdout[-300:])
    state = load_state(repo, run_id)
    ok(state["tasks"][0]["status"] == "pending", "resume sets done task pending")


def test_closed_resume_is_read_only(repo: Path) -> None:
    commit_file(repo, "src/closed-owned.txt", "one\n", "add closed owned file")
    run_id = "phase-drift-closed"
    start(repo, run_id, phase="closed")
    set_done_task(repo, run_id, ["src/closed-owned.txt"])
    (repo / ".lto" / run_id / "handoff.md").write_text("closed\n", encoding="utf-8")
    before = load_state(repo, run_id)
    recorded_head = before["workspace"]["head"]
    commit_file(repo, "src/closed-owned.txt", "two\n", "change closed owned file")

    proc = lto(repo, "resume", "--run-id", run_id)
    ok(proc.returncode == 0, "closed resume rc=0 despite related drift", proc.stdout[-300:])
    text = proc.stdout + proc.stderr
    ok("run is closed; resume is read-only" in text, "closed resume explains read-only drift")
    after = load_state(repo, run_id)
    ok(after["workspace"]["head"] == recorded_head, "closed resume keeps recorded HEAD")
    ok(after["tasks"][0]["status"] == "done", "closed resume keeps done task done")
    ok(after["tasks"][0]["blockers"] == [], "closed resume does not add revalidation blocker")


def test_forward_unrelated_file_drift(repo: Path) -> None:
    commit_file(repo, "src/owned.txt", "owned\n", "add owned file")
    run_id = "phase-drift-unrelated"
    start(repo, run_id, phase="implementation")
    set_done_task(repo, run_id, ["src/owned.txt"])
    commit_file(repo, "src/unrelated.txt", "unrelated\n", "change unrelated file")

    proc = lto(repo, "check", "--run-id", run_id, "--strict")
    ok(proc.returncode == 0, "unrelated forward drift strict rc=0",
       (proc.stdout + proc.stderr)[-300:])
    ok("related task files changed" not in proc.stderr, "unrelated drift has no related warning")


def test_forward_no_touched_files_warning(repo: Path) -> None:
    run_id = "phase-drift-no-touched"
    start(repo, run_id, phase="implementation")
    set_done_task(repo, run_id, [])
    commit_file(repo, "src/no-touched-change.txt", "x\n", "forward no touched files")

    proc = lto(repo, "check", "--run-id", run_id, "--strict")
    ok(proc.returncode == 0, "no touched_files warning does not fail strict",
       (proc.stdout + proc.stderr)[-300:])
    ok("no task touched_files recorded; file drift precision unavailable" in proc.stderr,
       "no touched_files warning printed", proc.stderr.strip()[-300:])


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="lto_phase_gate_") as tmp:
        repo = make_repo(Path(tmp))
        test_advisory_implementation(repo)
        test_json_is_clean(repo)
        test_strict_nonconverged_ledger(repo)
        test_argparse_rejects_other_phases(repo)
        test_closed_task_gate(repo)
        test_forward_related_file_drift(repo)
        test_closed_resume_is_read_only(repo)
        test_forward_unrelated_file_drift(repo)
        test_forward_no_touched_files_warning(repo)
    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nPHASE GATE TESTS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
