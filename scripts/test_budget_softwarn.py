#!/usr/bin/env python3
"""Tests for budget soft-warning injection in next brief / recap (fact layer).

软警告纯事实零阻断：维度 ratio >= warn_ratio 时简报出 '⚠️ budget' 行，未达不出。
"""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto import state as st  # noqa: E402
from lto.commands import next as next_cmd  # noqa: E402
from lto.commands import recap as recap_cmd  # noqa: E402


def _state_with(max_tokens, tokens):
    s = st.default_state(goal="g", host="h", repo="r", request="", phase="spec",
                         head="abc", branch="main", auditors="codex", timeout="900",
                         max_tokens=max_tokens)
    s["run_id"] = "20260101-000000-x"
    if tokens:
        s["agent_runs"] = {"j": [{"job_id": "j", "runner": "codex", "status": "ok",
                                  "cost": {"tokens": tokens, "tokens_in": 0, "tokens_out": tokens}}]}
    return s


class TestBudgetSoftWarn(unittest.TestCase):
    def test_next_brief_warns_over_80pct(self):
        s = _state_with(max_tokens=1000000, tokens=820000)  # 0.82
        facts = next_cmd.analyze(s, Path(tempfile.gettempdir()))
        brief = next_cmd.build_decision_brief(facts, s)
        self.assertIn("⚠️ budget", brief)
        self.assertIn("tokens", brief)

    def test_next_brief_no_warn_under_threshold(self):
        s = _state_with(max_tokens=1000000, tokens=100000)  # 0.1
        facts = next_cmd.analyze(s, Path(tempfile.gettempdir()))
        brief = next_cmd.build_decision_brief(facts, s)
        self.assertNotIn("⚠️ budget", brief)

    def test_next_brief_no_budget_block_no_warn(self):
        # 老 state 无 budget 上限 → 不出软警告
        s = st.default_state(goal="g", host="h", repo="r", request="", phase="spec",
                             head="abc", branch="main", auditors="codex", timeout="900")
        s["run_id"] = "20260101-000000-y"
        facts = next_cmd.analyze(s, Path(tempfile.gettempdir()))
        brief = next_cmd.build_decision_brief(facts, s)
        self.assertNotIn("⚠️ budget", brief)

    def test_recap_warns_over_80pct(self):
        s = _state_with(max_tokens=1000000, tokens=900000)  # 0.9
        out = recap_cmd._render_recap(s, "20260101-000000-x")
        self.assertIn("⚠️ budget", out)


if __name__ == "__main__":
    unittest.main()
