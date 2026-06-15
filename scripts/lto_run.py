#!/usr/bin/env python3
"""LTO 长任务编排 — 薄入口，命令分发到 lto/commands/。

Usage:
  lto_run.py start --goal "..." [--profile minimal|audit|deploy] [--with-audit]
                   # minimal=run-state；audit=+audit-ledger；deploy=audit+preflight 环境快照
  lto_run.py check [--strict] [--run-id ...] [--to implementation|closed] [--json]
  lto_run.py closeout --summary "..." [--run-id ...]
  lto_run.py resume [--run-id ...]
  lto_run.py preflight [--record]
  lto_run.py runner --task-id T1 --command "..." [--kind test]
  lto_run.py judge [--phase ...] [--task-id ...] [--rerun-tests]
  lto_run.py hook <pre-commit|pre-deploy|pre-closeout> [--force --reason "..."]
  lto_run.py self-test
"""

from __future__ import annotations

import argparse, sys
from pathlib import Path

# Ensure we can import from the lto package
_SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(_SCRIPT_DIR))

from lto.commands import start, check, closeout, resume, preflight, runner, judge, hook, selftest, parallel, pipeline, audit
from lto.commands import next as next_cmd
from lto.commands import autopilot, recap, task_add, task_update, phase as phase_cmd, collect_agent_run, runs as runs_cmd, memory, plugin, budget as budget_cmd, release as release_cmd


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="LTO — Long Task Orchestration",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="See SKILL.md for full workflow documentation.",
    )
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="target repository root")
    sub = parser.add_subparsers(dest="command", required=True)

    start.add_parser(sub)
    check.add_parser(sub)
    closeout.add_parser(sub)
    resume.add_parser(sub)
    preflight.add_parser(sub)
    runner.add_parser(sub)
    judge.add_parser(sub)
    hook.add_parser(sub)
    selftest.add_parser(sub)
    parallel.add_parser(sub)
    pipeline.add_parser(sub)
    audit.add_parser(sub)
    next_cmd.add_parser(sub)
    autopilot.add_parser(sub)
    recap.add_parser(sub)
    budget_cmd.add_parser(sub)
    release_cmd.add_parser(sub)
    task_add.add_parser(sub)
    task_update.add_parser(sub)
    phase_cmd.add_parser(sub)
    collect_agent_run.add_parser(sub)
    runs_cmd.add_parser(sub)
    memory.add_parser(sub)
    plugin.add_parser(sub)

    return parser


def _normalize_repo_arg(argv: list[str]) -> list[str]:
    """Allow `lto check --repo .` as a wrapper-friendly alias.

    argparse global options normally need to appear before the subcommand. The
    global shell wrapper is easier to use across repos if `--repo` can also be
    supplied after the command, so move the first repo option pair to the front.
    """
    for idx, token in enumerate(argv):
        if idx == 0:
            continue
        if token == "--repo" and idx + 1 < len(argv):
            return [token, argv[idx + 1], *argv[:idx], *argv[idx + 2:]]
        if token.startswith("--repo="):
            return [token, *argv[:idx], *argv[idx + 1:]]
    return argv


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    raw = list(sys.argv[1:] if argv is None else argv)
    args = parser.parse_args(_normalize_repo_arg(raw))
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
