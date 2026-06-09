#!/usr/bin/env python3
"""Tests for autopilot autonomous evidence gate (_autonomous_gate).

The gate is mechanical, zero-LLM: it reads ⑥ cross-run mining facts and lets
--autonomous proceed only when enough real dispatch data has accumulated.
Otherwise it honestly blocks and falls back to supervised. LTO never spawns a
decision agent and never reflects — reflection stays with the host agent.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto.commands import autopilot as ap  # noqa: E402

_FAILS = 0


def check(cond: bool, msg: str) -> None:
    global _FAILS
    if cond:
        print(f"OK   {msg}")
    else:
        _FAILS += 1
        print(f"FAIL {msg}", file=sys.stderr)


def _write_run(repo: Path, run_id: str, agent_runs: dict) -> None:
    d = repo / ".lto" / run_id
    d.mkdir(parents=True, exist_ok=True)
    state = {"schema_version": 1, "run_id": run_id, "current_phase": "spec"}
    if agent_runs:
        state["agent_runs"] = agent_runs
    (d / "state.json").write_text(json.dumps(state), encoding="utf-8")


def _result(runner="codex", status="ok"):
    return {"job_id": "j", "runner": runner, "status": status, "cost": {}}


def test_empty_repo_blocks() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        (repo / ".lto").mkdir()
        passed, reason = ap._autonomous_gate(repo)
        check(not passed, "empty repo → gate BLOCKED")
        check("证据不足" in reason, "blocked reason explains insufficient evidence")


def test_thin_data_blocks() -> None:
    # 2 runs, few results — below both thresholds
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        _write_run(repo, "20260101-000000-a", {"j": [_result()]})
        _write_run(repo, "20260102-000000-b", {"j": [_result(status="failed")]})
        passed, reason = ap._autonomous_gate(repo)
        check(not passed, "thin data (2 run / 2 result) → BLOCKED")


def test_enough_data_passes() -> None:
    # >= 5 runs each with results, >= 10 total results
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        for i in range(5):
            _write_run(
                repo, f"2026010{i+1}-000000-run{i}",
                {"j": [_result(), _result()]},  # 2 results each → 10 total
            )
        passed, reason = ap._autonomous_gate(repo)
        check(passed, "5 runs / 10 results → gate PASS")
        check("通过" in reason, "pass reason names the threshold met")
        check("git push" in reason and "停人类" in reason,
              "pass reason still reserves push/escalate for the human")


def test_boundary_one_short_blocks() -> None:
    # exactly 4 runs (one short of 5) with plenty of results → still BLOCKED
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        for i in range(4):
            _write_run(repo, f"2026010{i+1}-000000-r{i}",
                       {"j": [_result(), _result(), _result()]})
        passed, _ = ap._autonomous_gate(repo)
        check(not passed, "4 runs (one short of min) → BLOCKED even with enough results")


def test_fail_closed_on_mine_error(monkeypatch=None) -> None:
    # mine raising must fail-closed (block), never open the gate
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        import lto.cross_run_mining as crm
        orig = crm.mine
        crm.mine = lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom"))
        try:
            passed, reason = ap._autonomous_gate(repo)
        finally:
            crm.mine = orig
        check(not passed, "mine error → fail-closed BLOCKED")
        check("fail-closed" in reason, "fail-closed reason is explicit")


def test_empty_dicts_dont_pass_gate() -> None:
    """codex ③ HIGH: 5 run × 2 个空 {} 不该刷过 5/10 闸门——严格计数只算合规 AgentResult。"""
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        for i in range(5):
            _write_run(repo, f"2026010{i+1}-000000-e{i}", {"j": [{}, {}]})  # 10 个空 dict
        passed, _ = ap._autonomous_gate(repo)
        check(not passed, "5 run × empty {} → BLOCKED (空 dict 不是合规 result)")


def test_noncontract_results_dont_pass() -> None:
    """非合规 result（缺 job_id / 未知 runner / 非终态 status）不计入闸门。"""
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        for i in range(5):
            _write_run(repo, f"2026010{i+1}-000000-n{i}", {"j": [
                {"runner": "codex", "status": "ok"},          # 缺 job_id
                {"job_id": "j", "runner": "bogus", "status": "ok"},  # 未知 runner
                {"job_id": "j", "runner": "codex", "status": "pending"},  # 非终态
            ]})
        passed, _ = ap._autonomous_gate(repo)
        check(not passed, "non-contract results (no job_id / bad runner / pending) → BLOCKED")


def test_strict_schema_fail_closed() -> None:
    """codex ③ HIGH: mine 返回畸形（None / 非 dict me / 字符串数字）必须 fail-closed。"""
    import lto.cross_run_mining as crm
    orig = crm.mine
    try:
        for bad in (None, {"model_effectiveness": None}, {"model_effectiveness": "x"},
                    {"model_effectiveness": {"gate_runs": "5", "gate_results": "10"}}):
            crm.mine = lambda *a, _b=bad, **k: _b
            passed, reason = ap._autonomous_gate(Path("/tmp"))
            check(not passed, f"malformed mine ({bad!r:.40}) → fail-closed")
    finally:
        crm.mine = orig


def main() -> int:
    test_empty_repo_blocks()
    test_thin_data_blocks()
    test_enough_data_passes()
    test_boundary_one_short_blocks()
    test_fail_closed_on_mine_error()
    test_empty_dicts_dont_pass_gate()
    test_noncontract_results_dont_pass()
    test_strict_schema_fail_closed()
    if _FAILS:
        print(f"\n{_FAILS} FAILURES", file=sys.stderr)
        return 1
    print("\nAUTONOMOUS_GATE OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
