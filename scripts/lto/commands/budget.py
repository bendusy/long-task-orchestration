#!/usr/bin/env python3
"""lto budget check / extend — 查询当前预算状态、人显式抬上限。

budget 契约是 run 级、可选、分级刹车：软警告在 next/recap（事实层），硬刹车在
autopilot（fail-closed）。本命令是人侧入口：check 看用量，extend 抬上限（解除刹车
的唯一显式途径之一，另一是重 start）。extend 只能放宽，不能收紧到已用量以下（防自锁）。
"""
from __future__ import annotations

import argparse

from .. import state as st
from ..budget import check_budget


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    if args.budget_cmd == "check":
        return _check(state, run_id)
    if args.budget_cmd == "extend":
        return _extend(state, state_path, args)
    raise SystemExit("unknown budget subcommand")


def _check(state: dict, run_id: str) -> int:
    b = state.get("budget") or {}
    if not any(b.get(k) for k in ("max_turns", "max_tokens", "hard_deadline")):
        print(f"run {run_id}: no budget caps set (unlimited)")
        return 0
    bud = check_budget(state, st.token_rollup(state)["total_tokens"], st.iso_now())
    print(f"# budget — run {run_id}")
    for name, d in bud["dimensions"].items():
        if d["limit"] is not None:
            pct = f"{int(d['ratio'] * 100)}%" if d["ratio"] is not None else "-"
            print(f"  {name}: {d['used']}/{d['limit']} ({pct}) [{d['status']}]")
    print(f"  overall: {bud['overall']}")
    return 0


def _extend(state: dict, state_path, args) -> int:
    b = state.setdefault("budget", {})
    # 只放宽不能收紧到已用量以下（防自锁）：抬 max 到比已用还低，刹车永远拦不住。
    if args.max_tokens is not None:
        used = st.token_rollup(state)["total_tokens"]
        if args.max_tokens < used:
            print(f"error: --max-tokens {args.max_tokens} below already-used {used}")
            return 1
        b["max_tokens"] = args.max_tokens
    if args.max_turns is not None:
        used_turns = b.get("turns_used", 0)
        if args.max_turns < used_turns:
            print(f"error: --max-turns {args.max_turns} below already-used {used_turns}")
            return 1
        b["max_turns"] = args.max_turns
    if args.hard_deadline is not None:
        b["hard_deadline"] = args.hard_deadline
    st.save_state(state_path, state)
    print("budget extended")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("budget", help="inspect / extend run budget caps")
    p.add_argument("--run-id")
    sub = p.add_subparsers(dest="budget_cmd", required=True)
    sub.add_parser("check", help="report current budget usage")
    ext = sub.add_parser("extend", help="raise budget caps (human action; cannot shrink below used)")
    ext.add_argument("--max-turns", type=int, default=None)
    ext.add_argument("--max-tokens", type=int, default=None)
    ext.add_argument("--deadline", dest="hard_deadline", default=None)
    p.set_defaults(func=run)
