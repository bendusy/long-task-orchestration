#!/usr/bin/env python3
"""Tests for `lto budget check / extend`.

check 报当前用量；extend 抬上限（人显式动作）；extend 不能收紧到已用量以下（防自锁）。
"""
from __future__ import annotations

import argparse
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto import state as st  # noqa: E402
from lto.commands import budget as budget_cmd  # noqa: E402


def _make_run(repo: Path, run_id: str, *, budget: dict, tokens: int = 0) -> Path:
    d = repo / ".lto" / run_id
    d.mkdir(parents=True, exist_ok=True)
    s = st.default_state(goal="g", host="h", repo=str(repo), request="", phase="spec",
                         head="abc", branch="main", auditors="codex", timeout="900")
    s["run_id"] = run_id
    s["budget"].update(budget)
    if tokens:
        s["agent_runs"] = {"j": [{"job_id": "j", "runner": "codex", "status": "ok",
                                  "cost": {"tokens": tokens, "tokens_in": 0, "tokens_out": tokens}}]}
    st.save_state(d / "state.json", s)
    (repo / ".lto" / "current").write_text(run_id, encoding="utf-8")
    return d / "state.json"


def _args(repo, run_id, budget_cmd_name, **kw):
    ns = argparse.Namespace(repo=repo, run_id=run_id, budget_cmd=budget_cmd_name,
                            max_turns=None, max_tokens=None, hard_deadline=None)
    for k, v in kw.items():
        setattr(ns, k, v)
    return ns


def _cap(fn, *a):
    buf = io.StringIO()
    with redirect_stdout(buf):
        rc = fn(*a)
    return rc, buf.getvalue()


class TestBudgetCmd(unittest.TestCase):
    def test_check_reports_status(self):
        with tempfile.TemporaryDirectory() as td:
            repo = Path(td)
            _make_run(repo, "r1", budget={"max_tokens": 1000}, tokens=800)
            rc, out = _cap(budget_cmd.run, _args(repo, "r1", "check"))
            self.assertEqual(rc, 0)
            self.assertIn("tokens", out)
            self.assertIn("800/1000", out)

    def test_check_no_budget_reports_unlimited(self):
        with tempfile.TemporaryDirectory() as td:
            repo = Path(td)
            _make_run(repo, "r2", budget={})  # 全 None
            rc, out = _cap(budget_cmd.run, _args(repo, "r2", "check"))
            self.assertEqual(rc, 0)
            self.assertIn("unlimited", out)

    def test_extend_raises_cap(self):
        with tempfile.TemporaryDirectory() as td:
            repo = Path(td)
            sp = _make_run(repo, "r3", budget={"max_tokens": 1000})
            rc, out = _cap(budget_cmd.run, _args(repo, "r3", "extend", max_tokens=5000))
            self.assertEqual(rc, 0)
            s = st.load_state(sp)
            self.assertEqual(s["budget"]["max_tokens"], 5000)

    def test_extend_below_used_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            repo = Path(td)
            sp = _make_run(repo, "r4", budget={"max_tokens": 1000}, tokens=800)
            rc, out = _cap(budget_cmd.run, _args(repo, "r4", "extend", max_tokens=500))
            self.assertEqual(rc, 1)
            self.assertIn("below already-used", out)
            s = st.load_state(sp)
            self.assertEqual(s["budget"]["max_tokens"], 1000)  # 未写回

    def test_extend_turns_below_used_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            repo = Path(td)
            sp = _make_run(repo, "r5", budget={"max_turns": 10, "turns_used": 7})
            rc, out = _cap(budget_cmd.run, _args(repo, "r5", "extend", max_turns=5))
            self.assertEqual(rc, 1)
            s = st.load_state(sp)
            self.assertEqual(s["budget"]["max_turns"], 10)


if __name__ == "__main__":
    unittest.main()
