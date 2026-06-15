"""lto hook — 外部边界检查。"""

from __future__ import annotations

import argparse, os, sys
from pathlib import Path

from .. import state as st
from .. import git_state as gs


def run(args: argparse.Namespace) -> int:
    gate = args.gate
    if gate == "pre-commit":
        return _pre_commit(args)
    elif gate == "pre-deploy":
        return _pre_deploy(args)
    elif gate == "pre-closeout":
        return _pre_closeout(args)
    else:
        raise SystemExit(f"unknown gate: {gate}")


def _pre_commit(args: argparse.Namespace) -> int:
    # Check LTO_HOOK_MODE
    mode = os.environ.get("LTO_HOOK_MODE", "warn")
    if mode == "off":
        return 0

    repo = args.repo.resolve()
    current_file = repo / ".lto" / "current"
    if not current_file.exists():
        return 0  # No active LTO run

    run_id = current_file.read_text(encoding="utf-8").strip()
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        return 0

    head = gs.git_head(repo)
    gates = state.get("gates", {})
    unresolved = gates.get("unresolved_blocks", [])

    # Check if staged diff is LTO-only or docs-only
    staged = gs.git_value(repo, "diff", "--cached", "--name-only")
    if staged and all(_is_lto_or_doc(f) for f in staged.splitlines()):
        return 0  # .lto files or docs only — skip hook

    # Check if WIP commit
    if _is_wip_commit(repo):
        return 0

    warnings = []
    blocks = []

    # Test staleness
    last_tested = gates.get("last_tested_head")
    if last_tested and last_tested != head:
        if _related_files_changed(repo, last_tested, head, state):
            warnings.append("tests stale for changed files")

    # Review staleness
    last_reviewed = gates.get("last_reviewed_head")
    if last_reviewed and last_reviewed != head:
        warnings.append("no review for current HEAD")

    # Unresolved blocks
    if unresolved:
        blocks.append(f"{len(unresolved)} unresolved blocks")

    # Output
    if blocks and mode == "block" and not args.force:
        print(f"LTO: BLOCKED — {'; '.join(blocks)}", file=sys.stderr)
        print("  use: git commit --no-verify", file=sys.stderr)
        print("  or: lto hook pre-commit --force --reason '...'", file=sys.stderr)
        return 1

    if warnings:
        print(f"LTO: {'; '.join(warnings)}", file=sys.stderr)
        if mode == "block" and not args.force:
            print("  use: git commit --no-verify", file=sys.stderr)
            return 1

    return 0


def _pre_deploy(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    current_file = repo / ".lto" / "current"
    if not current_file.exists():
        print("LTO: no active run, deploy blocked", file=sys.stderr)
        return 1

    run_id = current_file.read_text(encoding="utf-8").strip()
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        print("LTO: no state, deploy blocked", file=sys.stderr)
        return 1

    # Run strict check
    import subprocess
    lto_path = Path(__file__).resolve().parent.parent.parent.parent / "lto_run.py"
    proc = subprocess.run(
        [sys.executable, str(lto_path), "check", "--run-id", run_id, "--strict"],
        cwd=repo, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        print("LTO: check failed, deploy blocked", file=sys.stderr)
        print(proc.stderr, file=sys.stderr)
        return 1

    # Check unresolved blocks
    unresolved = state.get("gates", {}).get("unresolved_blocks", [])
    if unresolved:
        print(f"LTO: {len(unresolved)} unresolved blocks, deploy blocked", file=sys.stderr)
        return 1

    # Don't deploy closed runs
    if state.get("current_phase") == "closed":
        print("LTO: run closed, deploy blocked", file=sys.stderr)
        return 1

    print("LTO: pre-deploy OK")
    return 0


def _pre_closeout(args: argparse.Namespace) -> int:
    # Thin wrapper — real gate is in closeout command
    repo = args.repo.resolve()
    import subprocess
    lto_path = Path(__file__).resolve().parent.parent.parent.parent / "lto_run.py"
    proc = subprocess.run(
        [sys.executable, str(lto_path), "check", "--strict"],
        cwd=repo, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        print("LTO: check failed, closeout blocked", file=sys.stderr)
        return 1
    print("LTO: pre-closeout OK")
    return 0


def _is_lto_or_doc(path: str) -> bool:
    return path.startswith(".lto/") or path.endswith(".md") or path.startswith("docs/")


def _is_wip_commit(repo: Path) -> bool:
    """Check if commit message starts with WIP."""
    msg_file = repo / ".git" / "COMMIT_EDITMSG"
    if msg_file.exists():
        first_line = msg_file.read_text(encoding="utf-8").split("\n")[0].strip()
        return first_line.upper().startswith("WIP")
    return False


def _related_files_changed(repo: Path, old_head: str, new_head: str, state: dict) -> bool:
    touched = set()
    for task in state.get("tasks", []):
        for f in task.get("touched_files", []):
            touched.add(f)
    if not touched:
        return False
    result = gs.run(["git", "diff", "--name-only", old_head, new_head, "--"] + list(touched), repo)
    return bool(result)


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("hook", help="boundary gate checks")
    p.add_argument("gate", choices=["pre-commit", "pre-deploy", "pre-closeout"])
    p.add_argument("--force", action="store_true")
    p.add_argument("--reason", default="")
    p.set_defaults(func=run)
