#!/usr/bin/env python3
"""Tests for per-run token rollup (state.token_rollup + recap/closeout render).

Verifies the aggregate is honest about partial coverage: token-less runs
(agy, pre-sidecar) count toward runs_total but not runs_with_tokens, so the
human-facing line says "N/M metered" rather than pretending full coverage.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lto import state as st  # noqa: E402
from lto.commands import recap, closeout  # noqa: E402


def ok(cond: bool, msg: str) -> int:
    if cond:
        print(f"OK   {msg}")
        return 0
    print(f"FAIL {msg}", file=sys.stderr)
    return 1


def _state(runs):
    return {"agent_runs": {f"j{i}": [r] for i, r in enumerate(runs)}}


def main() -> int:
    e = 0

    # mixed: codex+pi metered, agy not
    s = _state([
        {"runner": "codex", "cost": {"tokens_in": 100, "tokens_out": 20, "tokens": 500}},
        {"runner": "pi", "cost": {"tokens_in": 200, "tokens_out": 30, "tokens": 800}},
        {"runner": "agy", "cost": {"elapsed_sec": 5}},
    ])
    r = st.token_rollup(s)
    e += ok(r["total_tokens"] == 1300, f"total = 500+800 (got {r['total_tokens']})")
    e += ok(r["runs_with_tokens"] == 2 and r["runs_total"] == 3, "2 of 3 runs metered")
    e += ok(r["by_runner"]["codex"]["tokens"] == 500, "codex breakdown")
    e += ok(r["by_runner"]["agy"]["tokens"] == 0, "agy contributes 0 tokens")

    # tokens absent but in/out present → fall back to sum
    s2 = _state([{"runner": "codex", "cost": {"tokens_in": 10, "tokens_out": 5}}])
    e += ok(st.token_rollup(s2)["total_tokens"] == 15, "tokens falls back to in+out")

    # bool / negative are not counted as ints
    s3 = _state([{"runner": "codex", "cost": {"tokens": True}}, {"runner": "pi", "cost": {"tokens": -5}}])
    r3 = st.token_rollup(s3)
    e += ok(r3["total_tokens"] == 0 and r3["runs_with_tokens"] == 0, "bool/negative tokens rejected")

    # empty / no agent_runs
    e += ok(st.token_rollup({})["runs_total"] == 0, "empty state → 0 runs")

    # recap human line: partial coverage shown
    line = recap._token_summary(s)
    e += ok("2/3" in line and "tokens" in line, f"recap shows partial coverage: {line!r}")
    # recap unmetered case
    s_un = _state([{"runner": "agy", "cost": {}}])
    e += ok("未计量" in recap._token_summary(s_un), "recap unmetered line")
    # recap empty
    e += ok(recap._token_summary({}) == "", "recap empty → no line")

    # closeout machine line
    cl = closeout._token_usage_line(s)
    e += ok("1300 total" in cl and "2/3 runs metered" in cl, f"closeout line: {cl!r}")
    e += ok(closeout._token_usage_line({}) == "no agent runs", "closeout no-runs line")

    # k/M formatting
    e += ok(recap._fmt_tokens(1500) == "1.5k", "1.5k format")
    e += ok(recap._fmt_tokens(2_500_000) == "2.5M", "2.5M format")
    e += ok(recap._fmt_tokens(42) == "42", "small int format")

    if e:
        print(f"\n{e} FAILURES", file=sys.stderr)
        return 1
    print("\nTOKEN ROLLUP TESTS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
