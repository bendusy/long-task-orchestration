#!/usr/bin/env python3
from __future__ import annotations

import argparse, hashlib, re, subprocess, sys, tempfile
from datetime import datetime, timezone
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent
TEMPLATE_DIR = SKILL_DIR / "templates"
CORE_FILES = ("run-state.md", "preflight.md", "audit-ledger.md")
VALID_PHASES = {"intake", "spec", "audit", "implementation", "deploy", "observe", "closed"}
ARTIFACT_FIELDS = {"preflight.md": ("preflight_verdict",), "audit-ledger.md": ("latest HIGH+CRITICAL count", "close / continue verdict")}
CORE_RUN_STATE_KEYS = (
    "run_id",
    "feature / goal",
    "started_at",
    "host_runtime",
    "repo",
    "initial_user_request",
    "current_phase",
    "current_git_head",
    "current_branch",
)


def run(cmd: list[str], cwd: Path) -> str:
    try:
        return subprocess.check_output(cmd, cwd=cwd, text=True, stderr=subprocess.DEVNULL).strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return ""


def git_value(repo: Path, *args: str) -> str:
    return run(["git", *args], repo)


def is_git_repo(repo: Path) -> bool:
    return git_value(repo, "rev-parse", "--is-inside-work-tree") == "true"


def git_dirty(repo: Path) -> bool:
    return bool(git_value(repo, "status", "--porcelain", "--", ".", ":(exclude).lto"))


def git_commit_exists(repo: Path, ref: str) -> bool:
    cmd = ["git", "cat-file", "-e", f"{ref}^{{commit}}"]
    return subprocess.run(cmd, cwd=repo, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def iso_now() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def slugify(text: str) -> str:
    text = text.strip().lower()
    text = re.sub(r"[^a-z0-9._-]+", "-", text)
    text = re.sub(r"-{2,}", "-", text).strip("-")
    return text[:40] or "task"


def default_run_id(goal: str) -> str:
    digest = hashlib.sha1(goal.encode("utf-8")).hexdigest()[:8]
    return f"{datetime.now().strftime('%Y%m%d-%H%M%S')}-{slugify(goal)}-{digest}"


def validate_run_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}", value):
        raise SystemExit(f"invalid run id: {value!r}")
    if value in {".", ".."} or ".." in value:
        raise SystemExit(f"invalid run id: {value!r}")
    return value


def single_line(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip()


def replace_field(content: str, field: str, value: str) -> str:
    pattern = re.compile(rf"^- {re.escape(field)}:.*$", re.MULTILINE)
    replacement = f"- {field}: {single_line(value)}"
    if pattern.search(content):
        return pattern.sub(lambda _: replacement, content, count=1)
    return content


def read_field(content: str, field: str) -> str:
    match = re.search(rf"^- {re.escape(field)}:[ \t]*(.*)$", content, re.MULTILINE)
    return match.group(1).strip() if match else ""


def lto_root(repo: Path) -> Path:
    return repo / ".lto"


def current_file(repo: Path) -> Path:
    return lto_root(repo) / "current"


def resolve_run_id(repo: Path, run_id: str | None) -> str:
    if run_id:
        return validate_run_id(run_id)
    current = current_file(repo)
    if current.exists():
        value = current.read_text(encoding="utf-8").strip()
        if value:
            return validate_run_id(value)
    raise SystemExit("missing --run-id and .lto/current")


def run_dir(repo: Path, run_id: str) -> Path:
    return lto_root(repo) / run_id


def copy_template(name: str, target: Path, replacements: dict[str, str]) -> None:
    source = TEMPLATE_DIR / name
    if not source.exists():
        raise SystemExit(f"template missing: {source}")
    content = source.read_text(encoding="utf-8")
    for field, value in replacements.items():
        content = replace_field(content, field, value)
    target.write_text(content, encoding="utf-8")


def artifact_missing_fields(name: str, content: str) -> list[str]:
    return [field for field in ARTIFACT_FIELDS.get(name, ()) if not read_field(content, field)]


def cmd_start(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    if args.phase not in VALID_PHASES:
        raise SystemExit(f"invalid phase: {args.phase}")
    run_id = validate_run_id(args.run_id or default_run_id(args.goal))
    target_dir = run_dir(repo, run_id)
    if target_dir.exists() and not args.force:
        raise SystemExit(f"run already exists: {target_dir} (use --force to overwrite templates)")
    target_dir.mkdir(parents=True, exist_ok=True)

    head = git_value(repo, "rev-parse", "HEAD") or "unknown"
    branch = git_value(repo, "branch", "--show-current") or "unknown"
    replacements = {
        "run_id": run_id,
        "feature / goal": args.goal,
        "started_at": iso_now(),
        "host_runtime": args.host,
        "repo": str(repo),
        "initial_user_request": args.request or args.goal,
        "current_phase": args.phase,
        "current_git_head": head,
        "current_branch": branch,
        "task": args.goal,
        "requested_auditors": args.auditors,
        "planned_timeout": args.timeout,
    }
    copy_template("run-state.md", target_dir / "run-state.md", replacements)
    copy_template("preflight.md", target_dir / "preflight.md", replacements)
    copy_template("audit-ledger.md", target_dir / "audit-ledger.md", replacements)
    lto_root(repo).mkdir(parents=True, exist_ok=True)
    current_file(repo).write_text(run_id + "\n", encoding="utf-8")
    print(target_dir)
    return 0


def check_required_fields(content: str) -> list[str]:
    missing = []
    for field in CORE_RUN_STATE_KEYS:
        if not read_field(content, field):
            missing.append(field)
    return missing


def cmd_check(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = resolve_run_id(repo, args.run_id)
    target_dir = run_dir(repo, run_id)
    errors: list[str] = []
    warnings: list[str] = []

    for name in CORE_FILES:
        path = target_dir / name
        if not path.exists():
            if name == "run-state.md" or args.strict:
                errors.append(f"missing {path}")
            else:
                warnings.append(f"missing {path}")

    state_path = target_dir / "run-state.md"
    if state_path.exists():
        state = state_path.read_text(encoding="utf-8")
        for field in check_required_fields(state):
            errors.append(f"run-state missing field: {field}")
        phase = read_field(state, "current_phase")
        if phase and phase not in VALID_PHASES:
            errors.append(f"invalid current_phase: {phase}")
        recorded_head = read_field(state, "current_git_head")
        actual_head = git_value(repo, "rev-parse", "HEAD")
        if args.strict and not is_git_repo(repo):
            errors.append("strict check requires a git worktree")
        elif args.strict and (not recorded_head or recorded_head == "unknown" or not actual_head):
            errors.append("strict check requires a real git HEAD anchor")
        elif args.strict and recorded_head and not git_commit_exists(repo, recorded_head):
            errors.append(f"recorded git HEAD is not a commit: {recorded_head}")
        elif phase != "closed" and recorded_head and actual_head and recorded_head != actual_head:
            drift = git_value(repo, "diff", "--name-only", recorded_head, actual_head, "--", ".", ":(exclude).lto")
            if drift:
                msg = f"git HEAD drift outside .lto: run-state={recorded_head} actual={actual_head}"
                if args.strict:
                    errors.append(msg)
                else:
                    warnings.append(msg)
        if is_git_repo(repo) and git_dirty(repo):
            msg = "git worktree has uncommitted changes outside .lto"
            if args.strict:
                errors.append(msg)
            else:
                warnings.append(msg)
        handoff_path = target_dir / "handoff.md"
        if phase == "closed":
            if not handoff_path.exists() or not handoff_path.read_text(encoding="utf-8").strip():
                errors.append("closed run missing non-empty handoff.md")

    for warning in warnings:
        print(f"WARN {warning}", file=sys.stderr)
    for error in errors:
        print(f"ERROR {error}", file=sys.stderr)
    if errors:
        return 1
    print(f"OK {target_dir}")
    return 0


def cmd_closeout(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = resolve_run_id(repo, args.run_id)
    target_dir = run_dir(repo, run_id)
    state_path = target_dir / "run-state.md"
    if not state_path.exists():
        raise SystemExit(f"missing run-state.md: {state_path}")
    for name in CORE_FILES:
        path = target_dir / name
        if not path.exists() or not path.read_text(encoding="utf-8").strip():
            raise SystemExit(f"missing required artifact before closeout: {path}")
        missing_fields = artifact_missing_fields(name, path.read_text(encoding="utf-8"))
        if missing_fields:
            raise SystemExit(f"{name} missing fields before closeout: {', '.join(missing_fields)}")
    if not is_git_repo(repo):
        raise SystemExit("closeout requires a git worktree")
    if git_dirty(repo) and not args.allow_dirty:
        raise SystemExit("closeout refused: git worktree has uncommitted changes outside .lto")

    state = state_path.read_text(encoding="utf-8")
    if read_field(state, "current_phase") == "closed" and not args.force:
        raise SystemExit("run already closed (use --force to rewrite closeout)")
    missing = check_required_fields(state)
    if missing:
        raise SystemExit("run-state missing fields before closeout: " + ", ".join(missing))
    closed_at = iso_now()
    state = replace_field(state, "current_phase", "closed")
    state = replace_field(state, "current_git_head", git_value(repo, "rev-parse", "HEAD") or "unknown")
    state = replace_field(state, "current_branch", git_value(repo, "branch", "--show-current") or "unknown")
    state = replace_field(state, "blocked_by", args.blocked_by)
    state = replace_field(state, "next_command_or_question", args.next_action)
    state = state.split("\n## Closeout\n", 1)[0].rstrip() + "\n\n## Closeout\n\n"
    state += f"- closed_at: {closed_at}\n- summary: {single_line(args.summary)}\n- next_action: {single_line(args.next_action)}\n"
    state_path.write_text(state, encoding="utf-8")

    handoff = [
        "# LTO Handoff",
        "",
        f"- run_id: {run_id}",
        f"- goal: {read_field(state, 'feature / goal')}",
        f"- status: closed",
        f"- closed_at: {closed_at}",
        f"- git_head: {read_field(state, 'current_git_head')}",
        f"- branch: {read_field(state, 'current_branch')}",
        f"- blocked_by: {single_line(args.blocked_by)}",
        f"- summary: {single_line(args.summary)}",
        f"- next_action: {single_line(args.next_action)}",
        "",
        "## Artifacts",
        "",
        "- run-state.md",
        "- preflight.md",
        "- audit-ledger.md",
    ]
    (target_dir / "handoff.md").write_text("\n".join(handoff) + "\n", encoding="utf-8")
    print(target_dir / "handoff.md")
    return 0


def cmd_self_test(_: argparse.Namespace) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        run(["git", "init"], repo)
        (repo / "README.md").write_text("test\n", encoding="utf-8")
        run(["git", "add", "README.md"], repo)
        run(["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid", "commit", "-m", "init"], repo)
        start_args = argparse.Namespace(repo=repo, run_id="self-test", goal="self test", host="codex", request="self test", phase="intake", auditors="codex pi agy", timeout="900", force=False)
        if cmd_start(start_args) != 0:
            return 1
        rd = run_dir(repo, "self-test")
        for name, fields in {"preflight.md": {"preflight_verdict": "pass"}, "audit-ledger.md": {"latest HIGH+CRITICAL count": "0", "close / continue verdict": "close"}}.items():
            path = rd / name
            content = path.read_text(encoding="utf-8")
            for field, value in fields.items():
                content = replace_field(content, field, value)
            path.write_text(content, encoding="utf-8")
        run(["git", "add", ".lto"], repo)
        run(["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid", "commit", "-m", "start lto"], repo)
        check_args = argparse.Namespace(repo=repo, run_id="self-test", strict=True)
        if cmd_check(check_args) != 0:
            return 1
        close_args = argparse.Namespace(repo=repo, run_id="self-test", summary="self test complete", next_action="none", blocked_by="none", allow_dirty=False, force=False)
        if cmd_closeout(close_args) != 0:
            return 1
        if cmd_check(check_args) != 0:
            return 1
    print("SELFTEST OK")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Manage LTO run-state artifacts.")
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="target repository root")
    sub = parser.add_subparsers(dest="command", required=True)

    start = sub.add_parser("start", help="create .lto/<run-id> artifacts from templates")
    start.add_argument("--run-id")
    start.add_argument("--goal", required=True)
    start.add_argument("--host", default="unknown")
    start.add_argument("--request", default="")
    start.add_argument("--phase", default="intake")
    start.add_argument("--auditors", default="codex pi agy")
    start.add_argument("--timeout", default="900")
    start.add_argument("--force", action="store_true")
    start.set_defaults(func=cmd_start)

    check = sub.add_parser("check", help="validate a run-state directory")
    check.add_argument("--run-id")
    check.add_argument("--strict", action="store_true")
    check.set_defaults(func=cmd_check)

    closeout = sub.add_parser("closeout", help="mark a run closed and write handoff.md")
    closeout.add_argument("--run-id")
    closeout.add_argument("--summary", required=True)
    closeout.add_argument("--next-action", default="none")
    closeout.add_argument("--blocked-by", default="none")
    closeout.add_argument("--allow-dirty", action="store_true")
    closeout.add_argument("--force", action="store_true")
    closeout.set_defaults(func=cmd_closeout)

    self_test = sub.add_parser("self-test", help="run offline smoke coverage")
    self_test.set_defaults(func=cmd_self_test)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
