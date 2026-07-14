#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path
from typing import NoReturn


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_DIR = SCRIPT_DIR.parent
DEFAULT_SELF_TEST_LEDGER = REPO_DIR / "tests/fixtures/audit-ledger/terminal-zero.md"


class ProxyError(Exception):
    """Usage or process-launch failure that maps to exit code 2."""


def validate_run_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}", value):
        raise ProxyError(f"invalid run id: {value!r}")
    if value in {".", ".."} or ".." in value:
        raise ProxyError(f"invalid run id: {value!r}")
    return value


def resolve_ledger_path(args: argparse.Namespace) -> Path:
    if args.path:
        return Path(args.path)
    if args.run_id:
        run_id = validate_run_id(args.run_id)
        repo = (args.repo or Path.cwd()).resolve()
        return repo / ".lto" / run_id / "audit-ledger.md"
    raise ProxyError("provide a ledger path or --run-id")


def exec_rust(path: Path, strict: bool) -> NoReturn:
    binary = os.environ.get("LTO_BIN", "lto")
    command = [binary, "check", "--ledger", str(path)]
    if strict:
        command.append("--strict")
    print(
        "audit_ledger_check.py is a compatibility proxy; use `lto check --ledger`.",
        file=sys.stderr,
        flush=True,
    )
    try:
        os.execvp(binary, command)
    except OSError as exc:
        raise ProxyError(f"cannot execute Rust LTO binary {binary!r}: {exc}") from exc


def cmd_check(args: argparse.Namespace) -> NoReturn:
    exec_rust(resolve_ledger_path(args), args.strict)


def cmd_self_test(_: argparse.Namespace) -> NoReturn:
    exec_rust(DEFAULT_SELF_TEST_LEDGER, strict=False)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Compatibility proxy for the Rust LTO ledger evaluator."
    )
    subparsers = parser.add_subparsers(dest="command")

    check = subparsers.add_parser("check", help="delegate ledger evaluation to Rust")
    check.add_argument("path", nargs="?", help="path to audit-ledger.md")
    check.add_argument("--run-id", help="resolve .lto/<run-id>/audit-ledger.md")
    check.add_argument("--repo", type=Path, default=None, help="repo root for --run-id")
    check.add_argument("--strict", action="store_true", help="enable strict Rust verdict")
    check.set_defaults(func=cmd_check)

    self_test = subparsers.add_parser("self-test", help="delegate a golden check to Rust")
    self_test.set_defaults(func=cmd_self_test)
    return parser


def main(argv: list[str] | None = None) -> int:
    raw = list(sys.argv[1:] if argv is None else argv)
    if raw and raw[0] not in {"check", "self-test", "-h", "--help"}:
        raw = ["check", *raw]
    parser = build_parser()
    args = parser.parse_args(raw)
    if not getattr(args, "func", None):
        parser.print_help(sys.stderr)
        return 2
    try:
        args.func(args)
    except ProxyError as exc:
        print(f"ERROR {exc}", file=sys.stderr)
        return 2
    raise AssertionError("os.execvp unexpectedly returned")


if __name__ == "__main__":
    raise SystemExit(main())
