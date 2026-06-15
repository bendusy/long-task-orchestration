#!/usr/bin/env python3
"""Integration tests for autopilot's run-level budget hard-brake.

超 budget → autopilot fail-closed NEEDS_CONFIRM，零推进；未超 → 放行；老 state
（无 budget 块）→ 行为不变。turns_used 在 check 前自增：触顶那回合即被拦。
"""
from __future__ import annotations

import argparse
import io
import json
import sys
import tempfile
from contextlib import redirect_stdout
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto.commands import autopilot as ap  # noqa: E402
from lto.autopilot_status import AutopilotStatus, EXIT_CODES  # noqa: E402
from lto import state as st  # noqa: E402

_FAILS = 0


def check(cond: bool, msg: str) -> None:
    global _FAILS
    if cond:
        print(f"OK   {msg}")
    else:
        _FAILS += 1
        print(f"FAIL {msg}", file=sys.stderr)


def _make_run(repo: Path, run_id: str, *, budget: dict, agent_runs: dict | None = None) -> Path:
    d = repo / ".lto" / run_id
    d.mkdir(parents=True, exist_ok=True)
    state = st.default_state(
        goal="g", host="h", repo=str(repo), request="", phase="spec",
        head="abc", branch="main", auditors="codex", timeout="900",
    )
    state["run_id"] = run_id
    state["budget"].update(budget)
    if agent_runs:
        state["agent_runs"] = agent_runs
    st.save_state(d / "state.json", state)
    (repo / ".lto" / "current").write_text(run_id, encoding="utf-8")
    return d / "state.json"


def _args(repo: Path, run_id: str, *, autonomous: bool = False) -> argparse.Namespace:
    return argparse.Namespace(repo=repo, run_id=run_id, autonomous=autonomous)


def _tokens_run(total_tokens: int) -> dict:
    # 一个带 token 计量的 agent_run，让 token_rollup 累计到 total_tokens
    return {"j": [{"job_id": "j", "runner": "codex", "status": "ok",
                   "cost": {"tokens": total_tokens, "tokens_in": 0, "tokens_out": total_tokens}}]}


def _run_capture(args) -> tuple[int, str]:
    buf = io.StringIO()
    with redirect_stdout(buf):
        rc = ap.run(args)
    return rc, buf.getvalue()


def test_exceeded_tokens_blocks() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        sp = _make_run(repo, "20260101-000000-a",
                       budget={"max_tokens": 1000},
                       agent_runs=_tokens_run(1040))  # 1.04 → exceeded
        rc, out = _run_capture(_args(repo, "20260101-000000-a"))
        check(rc == EXIT_CODES[AutopilotStatus.NEEDS_CONFIRM],
              "tokens over budget → rc NEEDS_CONFIRM")
        check("budget exceeded" in out and "tokens" in out,
              "block reason names tokens overage")
        # 零推进：没有进入 supervised brief（不应出现 facts/route 输出）
        check("budget gate BLOCKED" in out, "stops at budget gate, no auto-advance")


def test_exceeded_turns_blocks_on_touching_turn() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        # max_turns=3, turns_used 已 2 → 本次进 run() +1 = 3 → 触顶 exceeded
        sp = _make_run(repo, "20260102-000000-b",
                       budget={"max_turns": 3, "turns_used": 2})
        rc, out = _run_capture(_args(repo, "20260102-000000-b"))
        check(rc == EXIT_CODES[AutopilotStatus.NEEDS_CONFIRM],
              "turns hitting cap this call → BLOCKED same turn")
        check("turns" in out, "block reason names turns")
        # turns_used 已落盘为 3
        state = st.load_state(sp)
        check(state["budget"]["turns_used"] == 3, "turns_used incremented and persisted")


def test_past_deadline_blocks() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        sp = _make_run(repo, "20260103-000000-c",
                       budget={"hard_deadline": "2000-01-01T00:00:00"})  # 早已过
        rc, out = _run_capture(_args(repo, "20260103-000000-c"))
        check(rc == EXIT_CODES[AutopilotStatus.NEEDS_CONFIRM],
              "past hard_deadline → BLOCKED")
        check("deadline" in out, "block reason names deadline")


def test_under_budget_proceeds() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        sp = _make_run(repo, "20260104-000000-d",
                       budget={"max_tokens": 1000000, "max_turns": 100},
                       agent_runs=_tokens_run(100))  # 0.0001 → ok
        rc, out = _run_capture(_args(repo, "20260104-000000-d"))
        # 未超 → 不被 budget 拦（会继续走 supervised brief，rc 非 NEEDS_CONFIRM-by-budget）
        check("budget gate BLOCKED" not in out, "under budget → not blocked by budget gate")


def test_old_state_without_budget_unchanged() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        d = repo / ".lto" / "20260105-000000-e"
        d.mkdir(parents=True, exist_ok=True)
        # 老 state：手写一个完全没有 budget 块的 state
        old = {"schema_version": 1, "run_id": "20260105-000000-e",
               "current_phase": "spec", "started_at": "2026-01-05T00:00:00"}
        (d / "state.json").write_text(json.dumps(old), encoding="utf-8")
        (repo / ".lto" / "current").write_text("20260105-000000-e", encoding="utf-8")
        rc, out = _run_capture(_args(repo, "20260105-000000-e"))
        check("budget gate BLOCKED" not in out,
              "old state without budget block → never blocked (all None = ok)")
        # budget 块被 setdefault 补出来、turns_used=1 落盘，但不崩
        state = st.load_state(d / "state.json")
        check(state.get("budget", {}).get("turns_used") == 1,
              "turns_used bootstrapped to 1 on old state")


def main() -> int:
    test_exceeded_tokens_blocks()
    test_exceeded_turns_blocks_on_touching_turn()
    test_past_deadline_blocks()
    test_under_budget_proceeds()
    test_old_state_without_budget_unchanged()
    if _FAILS:
        print(f"\n{_FAILS} FAILURES", file=sys.stderr)
        return 1
    print("\nBUDGET_GATE OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
