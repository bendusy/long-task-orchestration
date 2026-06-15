#!/usr/bin/env python3
"""Tests for the run-level budget contract pure-measurement layer (lto.budget).

budget.py 是零副作用纯函数：不读文件、不取系统时间。token 总数与当前时间
由调用方注入。这里覆盖单维度三态、deadline 进度比、聚合取最严、向后兼容
（缺 budget 块 = 全 None = 永远 ok）。
"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto.budget import (  # noqa: E402
    check_budget,
    deadline_status,
    dimension_status,
)


class TestDimensionStatus(unittest.TestCase):
    def test_none_limit_is_ok(self):
        s = dimension_status(limit=None, used=999999, warn_ratio=0.8)
        self.assertEqual(s["status"], "ok")
        self.assertIsNone(s["ratio"])

    def test_below_warn_is_ok(self):
        s = dimension_status(limit=1000, used=790, warn_ratio=0.8)  # 0.79
        self.assertEqual(s["status"], "ok")

    def test_at_warn_is_warn(self):
        s = dimension_status(limit=1000, used=800, warn_ratio=0.8)  # 0.80
        self.assertEqual(s["status"], "warn")

    def test_below_limit_is_warn(self):
        s = dimension_status(limit=1000, used=999, warn_ratio=0.8)  # 0.999
        self.assertEqual(s["status"], "warn")

    def test_at_limit_is_exceeded(self):
        s = dimension_status(limit=1000, used=1000, warn_ratio=0.8)  # 1.0
        self.assertEqual(s["status"], "exceeded")

    def test_over_limit_is_exceeded(self):
        s = dimension_status(limit=1000, used=1040, warn_ratio=0.8)  # 1.04
        self.assertEqual(s["status"], "exceeded")
        self.assertAlmostEqual(s["ratio"], 1.04)


class TestDeadlineStatus(unittest.TestCase):
    def test_no_deadline_is_ok(self):
        s = deadline_status(deadline=None, started_at="2026-06-15T00:00:00",
                            now="2026-06-15T12:00:00", warn_ratio=0.8)
        self.assertEqual(s["status"], "ok")
        self.assertIsNone(s["ratio"])

    def test_early_is_ok(self):
        # 区间 10h，过了 1h → 0.1
        s = deadline_status(deadline="2026-06-15T10:00:00", started_at="2026-06-15T00:00:00",
                            now="2026-06-15T01:00:00", warn_ratio=0.8)
        self.assertEqual(s["status"], "ok")

    def test_past_80pct_is_warn(self):
        # 区间 10h，过了 8h → 0.8
        s = deadline_status(deadline="2026-06-15T10:00:00", started_at="2026-06-15T00:00:00",
                            now="2026-06-15T08:00:00", warn_ratio=0.8)
        self.assertEqual(s["status"], "warn")

    def test_past_deadline_is_exceeded(self):
        s = deadline_status(deadline="2026-06-15T10:00:00", started_at="2026-06-15T00:00:00",
                            now="2026-06-15T11:00:00", warn_ratio=0.8)
        self.assertEqual(s["status"], "exceeded")

    def test_missing_started_at_degrades_to_ok_before_deadline(self):
        s = deadline_status(deadline="2026-06-15T10:00:00", started_at="",
                            now="2026-06-15T05:00:00", warn_ratio=0.8)
        self.assertEqual(s["status"], "ok")


def _state(budget=None, started="2026-06-15T00:00:00"):
    s = {"started_at": started}
    if budget is not None:
        s["budget"] = budget
    return s


class TestCheckBudget(unittest.TestCase):
    def test_no_budget_block_is_ok(self):
        # 老 state 无 budget 块 → 全 None → overall ok，warnings 空
        r = check_budget(_state(), token_total=999999, now_iso="2026-06-15T12:00:00")
        self.assertEqual(r["overall"], "ok")
        self.assertEqual(r["warnings"], [])

    def test_takes_strictest_dimension(self):
        # tokens=ok, turns=warn(0.9), deadline=exceeded → overall exceeded
        budget = {"max_turns": 10, "max_tokens": 1000000, "hard_deadline": "2026-06-15T10:00:00",
                  "turns_used": 9, "warn_ratio": 0.8}
        r = check_budget(_state(budget), token_total=100, now_iso="2026-06-15T11:00:00")
        self.assertEqual(r["overall"], "exceeded")

    def test_warn_emits_warning_line(self):
        budget = {"max_turns": None, "max_tokens": 1000000, "hard_deadline": None,
                  "turns_used": 0, "warn_ratio": 0.8}
        r = check_budget(_state(budget), token_total=820000, now_iso="2026-06-15T01:00:00")  # 0.82
        self.assertEqual(r["overall"], "warn")
        self.assertTrue(any("tokens" in w for w in r["warnings"]))

    def test_all_ok_no_warnings(self):
        budget = {"max_turns": 100, "max_tokens": 1000000, "hard_deadline": None,
                  "turns_used": 1, "warn_ratio": 0.8}
        r = check_budget(_state(budget), token_total=100, now_iso="2026-06-15T01:00:00")
        self.assertEqual(r["overall"], "ok")
        self.assertEqual(r["warnings"], [])

    def test_default_warn_ratio_when_missing(self):
        # budget 块缺 warn_ratio → 默认 0.8
        budget = {"max_tokens": 1000, "turns_used": 0}
        r = check_budget(_state(budget), token_total=800, now_iso="2026-06-15T01:00:00")
        self.assertEqual(r["overall"], "warn")


if __name__ == "__main__":
    unittest.main()
