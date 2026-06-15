"""lto start — 创建 .lto/<run-id>/ 状态文件。"""

from __future__ import annotations

import argparse, hashlib, re, sys
from datetime import datetime
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import artifacts as af
from .. import safe_emit


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


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    if args.phase not in st.VALID_PHASES:
        raise SystemExit(f"invalid phase: {args.phase}")

    run_id = validate_run_id(args.run_id or default_run_id(args.goal))
    target_dir = repo / ".lto" / run_id
    if target_dir.exists() and not args.force:
        raise SystemExit(f"run already exists: {target_dir} (use --force to overwrite)")

    head = gs.git_head(repo)
    branch = gs.git_branch(repo)

    # Build state
    state = st.default_state(
        goal=args.goal,
        host=args.host,
        repo=str(repo),
        request=args.request or args.goal,
        phase=args.phase,
        head=head,
        branch=branch,
        auditors=args.auditors,
        timeout=args.timeout,
        why=getattr(args, "why", "") or "",
        done_when=getattr(args, "done_when", "") or "",
        max_turns=getattr(args, "max_turns", None),
        max_tokens=getattr(args, "max_tokens", None),
        hard_deadline=getattr(args, "hard_deadline", None),
    )
    state["run_id"] = run_id
    state["artifacts"] = {"manifest": f".lto/{run_id}/artifacts.json"}

    # Write state.json (machine source of truth)
    state_path = target_dir / "state.json"
    st.save_state(state_path, state)

    # Write run-state.md from template (human-readable)
    template_path = Path(__file__).resolve().parent.parent.parent.parent / "templates" / "run-state.md"
    if template_path.exists():
        content = template_path.read_text(encoding="utf-8")
        field_replacements = {
            "run_id": run_id,
            "feature / goal": args.goal,
            "started_at": state["started_at"],
            "host_runtime": args.host,
            "repo": str(repo),
            "initial_user_request": args.request or args.goal,
            "current_phase": args.phase,
            "current_git_head": head,
            "current_branch": branch,
        }
        for field, value in field_replacements.items():
            content = st._replace_field(content, field, value)
        target_dir.mkdir(parents=True, exist_ok=True)
        (target_dir / "run-state.md").write_text(content, encoding="utf-8")

    # Conditionally create audit-ledger.
    # audit/deploy profiles both gate the ledger; deploy additionally captures a
    # preflight env snapshot below (so deploy ⊋ audit, not a dead alias).
    if args.with_audit or args.profile in ("audit", "deploy"):
        ledger_template = Path(__file__).resolve().parent.parent.parent.parent / "templates" / "audit-ledger.md"
        if ledger_template.exists():
            ledger = ledger_template.read_text(encoding="utf-8")
            for field, value in field_replacements.items():
                ledger = st._replace_field(ledger, field, value)
            (target_dir / "audit-ledger.md").write_text(ledger, encoding="utf-8")

    # Write current run marker.
    current_file = repo / ".lto" / "current"
    current_file.parent.mkdir(parents=True, exist_ok=True)
    current_file.write_text(run_id + "\n", encoding="utf-8")

    af.init_manifest(repo, run_id, state)

    # deploy profile: capture a preflight env snapshot into state.json.
    # This is what makes --profile deploy a strict superset of --profile audit
    # (codex audit B5: "preflight only needed before delegation/deploy").
    if args.profile == "deploy":
        from . import preflight as pf
        checks, verdict = pf.collect_checks(repo)
        pf._record_snapshot(repo, checks, verdict, run_id=run_id)

    # Install git hooks only when explicitly opted in (default off to avoid
    # silently mutating the user's .git/hooks and clashing with husky/pre-commit)
    if args.install_hooks:
        _maybe_install_hooks(repo)

    # Phase 1 passive event: run created. Fail-safe — never breaks start.
    safe_emit(
        repo, run_id, type="run.started", actor_kind="host", actor_id=args.host,
        phase=args.phase, object_id=run_id, object_type="run",
        summary=f"run started: {args.goal}",
    )

    print(target_dir)

    # 感知面：开工即陈列本机 affordance 事实（stderr，不污染 stdout 的 run 目录）。
    # 零推荐——任务形态与插件的匹配由 host 读 workflow-playbook 判断。
    try:
        from .. import plugins as plg
        aff = plg.affordance_facts(repo)
        if aff["available"]:
            ids = ", ".join(p["id"] for p in aff["available"])
            print(
                f"[lto] {len(aff['available'])} 个本机插件可挂载：{ids}\n"
                "[lto] 任务形态先验见 references/workflow-playbook.md；"
                "细节 `lto plugin list`，挂载 `lto plugin mount <dir> --run-id <id>`",
                file=sys.stderr,
            )
    except Exception:
        pass  # 感知层绝不弄崩 start

    return 0


def _detect_hook_framework(repo: Path) -> str | None:
    """检测已有的 hook 管理框架，命中返回名称，否则 None。"""
    if (repo / ".pre-commit-config.yaml").exists() or (repo / ".pre-commit-config.yml").exists():
        return "pre-commit framework"
    if (repo / ".husky").is_dir():
        return "husky"
    pkg = repo / "package.json"
    if pkg.exists():
        try:
            import json
            data = json.loads(pkg.read_text(encoding="utf-8"))
            deps = {**data.get("devDependencies", {}), **data.get("dependencies", {})}
            if "husky" in deps:
                return "husky"
        except (ValueError, OSError):
            pass
    return None


def _maybe_install_hooks(repo: Path) -> None:
    """安装 LTO pre-commit hook，但先检测冲突；冲突时跳过并警告。"""
    hooks_dir = repo / ".git" / "hooks"
    if not hooks_dir.is_dir():
        return

    framework = _detect_hook_framework(repo)
    if framework is not None:
        print(
            f"[lto] 检测到 {framework}，跳过 pre-commit hook 自动安装，"
            "避免与其冲突。如需 LTO 闸门请手动在你的 hook 链里加："
            "lto hook pre-commit"
        )
        return

    pre_commit_path = hooks_dir / "pre-commit"
    lto_hook_line = "lto hook pre-commit"

    if pre_commit_path.exists():
        existing = pre_commit_path.read_text(encoding="utf-8")
        if lto_hook_line in existing:
            return
        # 已存在非 LTO 的 pre-commit 内容：不盲目追加，交还用户决定
        meaningful = [
            ln for ln in existing.splitlines()
            if ln.strip() and not ln.strip().startswith("#") and ln.strip() not in ("#!/bin/bash", "#!/bin/sh")
        ]
        if meaningful:
            print(
                "[lto] 已存在自定义 pre-commit hook，跳过自动安装避免破坏它。"
                f"如需 LTO 闸门请手动追加：{lto_hook_line} || exit 1"
            )
            return
        with open(pre_commit_path, "a", encoding="utf-8") as f:
            f.write(f"\n# LTO pre-commit gate\n{lto_hook_line} || exit 1\n")
    else:
        pre_commit_path.write_text(
            "#!/bin/bash\n# LTO pre-commit gate\n"
            f"{lto_hook_line} || exit 1\n",
            encoding="utf-8",
        )
        pre_commit_path.chmod(0o755)


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("start", help="create .lto/<run-id> artifacts")
    p.add_argument("--run-id")
    p.add_argument("--goal", required=True)
    p.add_argument("--host", default="unknown")
    p.add_argument("--request", default="")
    p.add_argument("--phase", default="intake")
    p.add_argument("--auditors", default="codex pi agy")
    p.add_argument("--timeout", default="900")
    p.add_argument("--profile", default="minimal", choices=["minimal", "audit", "deploy"],
                   help="minimal=只建 run-state；audit=加 audit-ledger；deploy=audit + 落 preflight 环境快照")
    p.add_argument("--why", default="", help="why this run exists (for human recap after long gaps)")
    p.add_argument("--done-when", dest="done_when", default="",
                   help="done-criteria: how you'll know it's finished (for human recap)")
    p.add_argument("--max-turns", type=int, default=None,
                   help="run-level cap on autopilot auto-advance turns (default: unlimited)")
    p.add_argument("--max-tokens", type=int, default=None,
                   help="run-level cap on cumulative dispatch tokens (default: unlimited)")
    p.add_argument("--deadline", dest="hard_deadline", default=None,
                   help="ISO8601 hard deadline for the run (default: none)")
    p.add_argument("--with-audit", action="store_true", help="force generate audit-ledger.md")
    p.add_argument("--install-hooks", action="store_true",
                   help="install LTO pre-commit gate into .git/hooks (opt-in; skips if husky/pre-commit detected)")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=run)
