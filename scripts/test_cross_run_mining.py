#!/usr/bin/env python3
"""Tests for cross_run_mining: model effectiveness + phase friction → host brief.

Builds 2-3 fake .lto runs (different runners / statuses / events) and asserts:
- by_runner_model aggregation (counts / success_rate / avg_tokens)
- phase friction counted by distinct run (within-run repeats = one pattern)
- brief contains NO route/promote/imperative wording (banned-word scan)
- honest degradation when there is nothing to mine
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto import cross_run_mining as crm  # noqa: E402
from lto import events as ev  # noqa: E402


_FAILS = 0


def check(cond: bool, msg: str) -> None:
    global _FAILS
    if cond:
        print(f"OK   {msg}")
    else:
        _FAILS += 1
        print(f"FAIL {msg}", file=sys.stderr)


def _write_run(repo: Path, run_id: str, *, agent_runs=None, events=None) -> None:
    run_dir = repo / ".lto" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    state = {"schema_version": 1, "run_id": run_id, "current_phase": "spec"}
    if agent_runs:
        state["agent_runs"] = agent_runs
    (run_dir / "state.json").write_text(json.dumps(state), encoding="utf-8")
    for e in (events or []):
        ev.append(repo, run_id, **e)


def _result(runner, status, *, tokens=None, tokens_in=None, tokens_out=None, model=None):
    cost = {}
    if tokens is not None:
        cost["tokens"] = tokens
    if tokens_in is not None:
        cost["tokens_in"] = tokens_in
    if tokens_out is not None:
        cost["tokens_out"] = tokens_out
    r = {"job_id": "j", "runner": runner, "status": status, "cost": cost}
    if model is not None:
        r["model"] = model
    return r


def build_fixture(repo: Path) -> None:
    # run A: codex 3 ok / 1 failed ; pi 1 ok / 1 timeout
    _write_run(
        repo, "20260101-000000-run-a",
        agent_runs={
            "j1": [
                _result("codex", "ok", tokens=1000),
                _result("codex", "ok", tokens=2000),
                _result("codex", "ok", tokens_in=500, tokens_out=500),  # 1000 via fallback
                _result("codex", "failed"),
            ],
            "j2": [
                _result("pi", "ok", tokens=3000),
                _result("pi", "timeout"),
            ],
        },
        events=[
            dict(type="runner.finished", actor_kind="runner", actor_id="lto-runner",
                 fields={"rc": 1}),
            dict(type="runner.finished", actor_kind="runner", actor_id="lto-runner",
                 fields={"rc": 1}),  # within-run repeat
            dict(type="phase.changed", actor_kind="lto", phase="spec"),
        ],
    )
    # run B: codex 1 ok ; pi 2 ok / 1 failed ; agy 1 ok (no tokens)
    _write_run(
        repo, "20260102-000000-run-b",
        agent_runs={
            "j1": [_result("codex", "ok", tokens=1500)],
            "j2": [
                _result("pi", "ok", tokens=2000),
                _result("pi", "ok", tokens=2000),
                _result("pi", "failed"),
            ],
            "j3": [_result("agy", "ok")],  # token-less runner
        },
        events=[
            dict(type="runner.finished", actor_kind="runner", actor_id="lto-runner",
                 fields={"rc": 2}),
        ],
    )
    # run C: no agent_runs, only events (no rc!=0)
    _write_run(
        repo, "20260103-000000-run-c",
        events=[
            dict(type="runner.finished", actor_kind="runner", actor_id="lto-runner",
                 fields={"rc": 0}),
        ],
    )


def test_model_effectiveness() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        build_fixture(repo)
        me = crm.mine_model_effectiveness(repo)
        by = me["by_runner_model"]

        # codex: 4 (A) + 1 (B) = 5 派工, 4 ok / 1 failed
        check(by["codex"]["runs"] == 5, "codex runs == 5")
        check(by["codex"]["ok"] == 4 and by["codex"]["failed"] == 1, "codex 4 ok / 1 failed")
        check(by["codex"]["success_rate"] == round(4 / 5, 3), "codex success_rate == 0.8")
        # codex tokens: 1000+2000+1000(fallback)+1500 = 5500 over 4 token-runs
        check(by["codex"]["total_tokens"] == 5500, "codex total_tokens == 5500 (in/out fallback works)")
        check(by["codex"]["tokens_runs"] == 4, "codex tokens_runs == 4 (failed result has no tokens)")
        check(by["codex"]["avg_tokens"] == round(5500 / 4), "codex avg_tokens over token-runs only")

        # pi: A(1 ok,1 timeout) + B(2 ok,1 failed) = 5; 3 ok / 1 failed / 1 timeout
        check(by["pi"]["runs"] == 5, "pi runs == 5")
        check(by["pi"]["ok"] == 3, "pi ok == 3")
        check(by["pi"]["timeout"] == 1, "pi timeout == 1")
        check(by["pi"]["failed"] == 1, "pi failed == 1")

        # agy: token-less → avg未计量 (avg_tokens==0, tokens_runs==0)
        check(by["agy"]["runs"] == 1 and by["agy"]["tokens_runs"] == 0,
              "agy token-less: counted but no token coverage")

        check(me["total_runner_results"] == 11, "total runner results == 11")
        check(me["runs_with_agent_runs"] == 2, "runs_with_agent_runs == 2 (run C has none)")
        # distinct-run tracking: codex/pi each span run-a + run-b
        check(by["codex"]["distinct_runs"] == 2, "codex distinct_runs == 2")
        check(by["pi"]["distinct_runs"] == 2, "pi distinct_runs == 2")
        check(by["agy"]["distinct_runs"] == 1, "agy distinct_runs == 1 (run-b only)")


def test_skipped_status_transparent() -> None:
    # #3: skipped is a terminal AgentResult status; it must be a tracked column,
    # not silently folded into other, and the success denominator must show it.
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        _write_run(repo, "20260201-000000-skip-a",
                   agent_runs={"j": [_result("codex", "ok", tokens=100),
                                     _result("codex", "skipped"),
                                     _result("codex", "weird_status")]})
        me = crm.mine_model_effectiveness(repo)
        s = me["by_runner_model"]["codex"]
        check(s["skipped"] == 1, "skipped tracked as its own column")
        check(s["other"] == 1, "unknown status → other")
        check(s["runs"] == 3 and s["ok"] == 1, "denominator (runs=3) includes skipped+other")
        check(s["success_rate"] == round(1 / 3, 3), "success_rate denominator transparent")
        brief = crm.render_mining_brief(repo)
        check("skipped" in brief and "other" in brief, "brief table shows skipped + other columns")


def test_bad_state_isolation() -> None:
    # #2: one corrupt state.json must not crash the whole cross-run scan.
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        _write_run(repo, "20260301-000000-good",
                   agent_runs={"j": [_result("codex", "ok", tokens=100)]})
        bad_dir = repo / ".lto" / "20260302-000000-bad"
        bad_dir.mkdir(parents=True)
        (bad_dir / "state.json").write_text("{not valid json", encoding="utf-8")
        me = crm.mine_model_effectiveness(repo)
        check(me["skipped_bad_runs"] == 1, "corrupt state.json counted as skipped_bad_runs")
        check(me["by_runner_model"]["codex"]["runs"] == 1, "good run still aggregated")
        brief = crm.render_mining_brief(repo)
        check("损坏" in brief, "brief reports skipped bad runs")


def test_cost_not_dict() -> None:
    # #6: cost may be a str/list in historical/corrupt state — must not crash.
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        run_dir = repo / ".lto" / "20260401-000000-badcost"
        run_dir.mkdir(parents=True)
        state = {"schema_version": 1, "run_id": "x", "current_phase": "spec",
                 "agent_runs": {"j": [
                     {"job_id": "j", "runner": "codex", "status": "ok", "cost": "oops"},
                     {"job_id": "j", "runner": "codex", "status": "ok", "cost": [1, 2]},
                 ]}}
        (run_dir / "state.json").write_text(json.dumps(state), encoding="utf-8")
        me = crm.mine_model_effectiveness(repo)  # must not raise
        s = me["by_runner_model"]["codex"]
        check(s["runs"] == 2 and s["tokens_runs"] == 0, "non-dict cost → 0 tokens, no crash")


def test_distinct_run_gate() -> None:
    # #1: HIGH — within a SINGLE run, codex 3 ok vs pi 3 failed must NOT trigger
    # a "X 优于 Y" comparison hint. Comparison requires >= min_runs distinct runs.
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        _write_run(repo, "20260501-000000-onerun",
                   agent_runs={"j1": [_result("codex", "ok", tokens=100),
                                      _result("codex", "ok", tokens=100),
                                      _result("codex", "ok", tokens=100)],
                               "j2": [_result("pi", "failed"),
                                      _result("pi", "failed"),
                                      _result("pi", "failed")]})
        brief = crm.render_mining_brief(repo)
        check("高于" not in brief, "single-run repeated dispatch does NOT crown a winner")
        check("无法比较" in brief or "不足" in brief,
              "single-run sample → cross-run comparison refused")

    # contrast: same skew but spread across 2 distinct runs → comparison allowed
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        _write_run(repo, "20260601-000000-r1",
                   agent_runs={"j1": [_result("codex", "ok", tokens=100)],
                               "j2": [_result("pi", "failed")]})
        _write_run(repo, "20260602-000000-r2",
                   agent_runs={"j1": [_result("codex", "ok", tokens=100)],
                               "j2": [_result("pi", "failed")]})
        brief = crm.render_mining_brief(repo)
        check("高于" in brief, "true cross-run skew (2 distinct runs) surfaces a hint")


def test_generic_events_aggregation() -> None:
    # #4: events beyond the 2 hardcoded signals must not be silently dropped.
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        # two runs, each with 5 task.status_changed + a timeout=true finish
        for rid in ("20260701-000000-e1", "20260702-000000-e2"):
            evs = [dict(type="task.status_changed", actor_kind="lto",
                        task_id=f"t{i}") for i in range(5)]
            evs.append(dict(type="runner.finished", actor_kind="runner",
                            actor_id="lto-runner", fields={"rc": 0, "timeout": True}))
            _write_run(repo, rid, events=evs)
        friction = crm.mine_phase_friction(repo, min_runs=2)
        sigs = {f["signal"] for f in friction}
        check(any("task.status_changed" in s for s in sigs),
              "generic high-volume task.status_changed surfaced (not dropped)")
        check("runner.finished timeout=true" in sigs,
              "runner.finished timeout=true surfaced as its own signal")
        # type field populated for host triage
        ts = next(f for f in friction if "task.status_changed" in f["signal"])
        check(ts.get("type") == "task.status_changed", "friction row carries event type")


def test_phase_friction() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        build_fixture(repo)
        friction = crm.mine_phase_friction(repo, min_runs=2)
        # runner.finished rc!=0: run A (2 events) + run B (1) = 2 runs, 3 events
        rc = [f for f in friction if f["signal"] == "runner.finished rc!=0"]
        check(len(rc) == 1, "rc!=0 friction surfaced at min_runs=2")
        check(rc[0]["runs"] == 2, "rc!=0 counted across 2 distinct runs (within-run repeat = 1)")
        check(rc[0]["count"] == 3, "rc!=0 total event count == 3")

        # below threshold: min_runs=3 drops it (only 2 runs have it)
        none = crm.mine_phase_friction(repo, min_runs=3)
        check(all(f["signal"] != "runner.finished rc!=0" for f in none),
              "rc!=0 dropped when min_runs=3 (only 2 runs qualify)")


def test_brief_wording() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        build_fixture(repo)
        brief = crm.render_mining_brief(repo)
        banned = [
            "必须派", "自动选", "promote", "route to", "routing order",
            "必须", "应该", "该用", "继续正常用", "多跑", "自动派", "建议派", "决定派",
        ]
        for b in banned:
            check(b.lower() not in brief.lower(), f"brief has no banned wording: {b!r}")
        # also scan the degraded/insufficient + thin-sample branches, which
        # historically carried imperative residue ("继续/多跑/别据此选").
        with tempfile.TemporaryDirectory() as td2:
            r2 = Path(td2); (r2 / ".lto").mkdir()
            empty_brief = crm.render_mining_brief(r2)
            _write_run(r2, "20260801-000000-thin",
                       agent_runs={"j": [_result("codex", "ok", tokens=100)]})
            thin_brief = crm.render_mining_brief(r2)
        for label, txt in (("empty", empty_brief), ("thin", thin_brief)):
            for b in banned:
                check(b.lower() not in txt.lower(),
                      f"{label} brief has no banned wording: {b!r}")
        # must read as host-decides evidence
        check("由你" in brief, "brief asserts host decides (由你)")
        check("agent_runs" in brief and "events.jsonl" in brief, "brief names both data sources")
        check("lto-runner" in brief or "本地执行器" in brief,
              "brief warns events are local executor, not a model")


def test_honest_degradation() -> None:
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        (repo / ".lto").mkdir()
        brief = crm.render_mining_brief(repo)
        check("数据不足" in brief, "empty .lto → 数据不足 (no fabricated conclusion)")
        # no fabricated runner names / success rates
        check("成功率" not in brief or "数据不足" in brief, "no success-rate table when no data")

        # thin sample (1 run, 2 codex派工) → hint must refuse to compare
        _write_run(repo, "20260101-000000-thin",
                   agent_runs={"j": [_result("codex", "ok", tokens=100),
                                     _result("codex", "ok", tokens=100)]})
        brief2 = crm.render_mining_brief(repo)
        check("无法比较" in brief2 or "样本仍偏小" in brief2 or "不下结论" in brief2,
              "thin sample → hint refuses to crown a winner")


def test_model_subgrouping() -> None:
    """⑦: agent_runs 落 model 字段时，brief 按 runner 下的 model 分组显示；
    旧 run 无 model 字段时不崩、不显示 model 分布（向后兼容）。"""
    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        # 同 runner(pi) 不同 model(deepseek vs glm) → 应能区分
        _write_run(repo, "20260101-000000-mdl-a",
                   agent_runs={"j": [_result("pi", "ok", tokens=100, model="deepseek-v4-pro")]})
        _write_run(repo, "20260102-000000-mdl-b",
                   agent_runs={"j": [_result("pi", "failed", model="glm-4.6")]})
        brief = crm.render_mining_brief(repo)
        check("model 分布" in brief, "model present → brief shows model distribution")
        check("deepseek-v4-pro" in brief and "glm-4.6" in brief,
              "same runner different models are distinguished")

    with tempfile.TemporaryDirectory() as td:
        repo = Path(td)
        # 旧 run：无 model 字段 → 不崩，不显示 model 分布
        _write_run(repo, "20260101-000000-old-a",
                   agent_runs={"j": [_result("codex", "ok", tokens=100)]})
        _write_run(repo, "20260102-000000-old-b",
                   agent_runs={"j": [_result("codex", "ok", tokens=100)]})
        brief = crm.render_mining_brief(repo)
        check("model 分布" not in brief, "no model field → no model section (backward compatible)")


def main() -> int:
    test_model_effectiveness()
    test_skipped_status_transparent()
    test_bad_state_isolation()
    test_cost_not_dict()
    test_distinct_run_gate()
    test_generic_events_aggregation()
    test_phase_friction()
    test_brief_wording()
    test_honest_degradation()
    test_model_subgrouping()
    if _FAILS:
        print(f"\n{_FAILS} FAILURES", file=sys.stderr)
        return 1
    print("\nCROSS_RUN_MINING OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
