#!/usr/bin/env python3
"""Regression: read-only fail-closed must cover every runner without a
read-only enforcement mechanism.

2026-06-10 三方 spec 审（agy）审出：validate_for_runner 只拦 agy 的
read-only，gemini 同样无兑现机制却被放行。本测试把两家的拒绝与
合法路径一起钉死，防止未来加 runner 时再漏。
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lto.agent_job import KNOWN_RUNNERS, PermissionPolicy  # noqa: E402

FAILURES: list[str] = []


def ok(cond: bool, msg: str) -> None:
    print(("ok " if cond else "FAIL ") + msg)
    if not cond:
        FAILURES.append(msg)


def rejects(policy: PermissionPolicy, runner: str, needle: str) -> bool:
    try:
        policy.validate_for_runner(runner, {})
    except ValueError as e:
        return needle in str(e)
    return False


def accepts(policy: PermissionPolicy, runner: str) -> bool:
    try:
        policy.validate_for_runner(runner, {})
    except ValueError:
        return False
    return True


def main() -> int:
    ro = PermissionPolicy(sandbox="read-only")

    # 无 read-only 兑现机制的两家：validate 阶段即拒（fail-closed）。
    for runner in ("agy", "gemini"):
        ok(
            rejects(ro, runner, "cannot enforce read-only"),
            f"{runner} read-only job rejected fail-closed",
        )

    # 有兑现机制的三家：read-only 默认（空 tools）放行。
    for runner in ("codex", "claude", "pi"):
        ok(accepts(ro, runner), f"{runner} read-only job accepted")

    # agy/gemini 在 workspace-write（带 reason）仍可派。
    ww = PermissionPolicy(sandbox="workspace-write", reason="write batch output")
    for runner in ("agy", "gemini"):
        ok(accepts(ww, runner), f"{runner} workspace-write (with reason) accepted")

    # 防漏兜底：KNOWN_RUNNERS 中每一家在 read-only 下要么被拒、要么属于
    # 已知可兑现集合——新增 runner 未归类即测试红。
    enforceable = {"codex", "claude", "pi"}
    for runner in KNOWN_RUNNERS:
        if runner in enforceable:
            ok(accepts(ro, runner), f"{runner} classified enforceable and accepted")
        else:
            ok(
                rejects(ro, runner, "cannot enforce read-only"),
                f"{runner} unclassified runner rejected fail-closed",
            )

    if FAILURES:
        print(f"\n{len(FAILURES)} failure(s)")
        return 1
    print("\nALL PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
