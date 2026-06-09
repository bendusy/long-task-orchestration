"""Cross-run data mining → host brief (LTO evolution mainline, v1).

「越用越聪明」needs to mine real effectiveness across runs, not just within
one. This is a single cross-run scanner: it walks every .lto/<run-id> **once**
and extracts two signals, then synthesises **one brief for the host agent to
read**.

Two sources, deliberately kept distinct:

- **Track 1 — model effectiveness** (source: state.json ``agent_runs``).
  agent_runs[job_id] is a list of AgentResult dicts carrying the heterogeneous
  runner (codex/pi/agy/claude), its status, and cost.tokens. This is the ONLY
  place a real model identity exists, so "by model" rolls up from here.
  Cost extraction reuses state.token_rollup's tolerant logic.

- **Track 2 — phase friction** (source: events.jsonl).
  events.runner.finished is the **local shell executor** (actor.id =
  "lto-runner"), NOT a heterogeneous model — so events can never be a model
  source. They only surface recurring phase friction (e.g. runner.finished with
  rc != 0 recurring across runs), aggregated by run count à la
  interventions.recurring_friction.

Iron law (references/control-loop-harness.md §3-5): the brief is **evidence +
derived signal + hypothesis hint**, never a command. It does not route, does
not promote, does not auto-select a model. The host decides. Wording must read
"由你(host)定", never "LTO 决定派 X".

Honest degradation: too few runs / no agent_runs → say "数据不足，需更多真实
run", never fabricate a conclusion.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from . import state as st
from . import events as ev


# ──────────────────────── shared cross-run walk ────────────────────────
#
# One pass over .lto/<run-id>. Reuses aggregate_across_runs' skeleton:
# sorted iterdir + validate_run_id to skip the "current" symlink target name
# and malformed dirs. Both tracks consume the same walk so we never scan twice.


def _iter_runs(repo: Path) -> list[str]:
    """Valid run-ids under .lto/, sorted (run-ids are time-prefixed → chrono)."""
    lto_root = repo / ".lto"
    if not lto_root.is_dir():
        return []
    run_ids: list[str] = []
    for run_dir in sorted(lto_root.iterdir()):
        if not run_dir.is_dir():
            continue
        try:
            run_ids.append(st.validate_run_id(run_dir.name))
        except (ValueError, SystemExit):
            continue  # "current" symlink target / malformed dir
    return run_ids


def _int(val: Any) -> int:
    return val if isinstance(val, int) and not isinstance(val, bool) and val >= 0 else 0


def _result_tokens(result: dict[str, Any]) -> int:
    """Token cost for one AgentResult dict. Mirrors token_rollup: prefer
    cost.tokens, fall back to tokens_in + tokens_out when tokens is absent.

    ``cost`` is asserted dict before .get — historical/corrupt state may carry a
    str/list there, which would otherwise crash the whole cross-run scan."""
    cost = result.get("cost")
    if not isinstance(cost, dict):
        return 0
    tok = _int(cost.get("tokens"))
    if tok == 0:
        tok = _int(cost.get("tokens_in")) + _int(cost.get("tokens_out"))
    return tok


# ──────────────────────── track 1: model effectiveness ────────────────────────
#
# Source: state.json agent_runs. Group by runner (the only real model identity
# in an AgentResult). status ∈ {ok, failed, timeout, rate_limited, skipped, ...}.


# Tracked statuses are derived from the AgentResult JobStatus enum so the
# columns stay in lockstep with the contract — no hand-maintained drift. Any
# status not in this set lands in "other" (shown in the table so the success
# denominator is transparent: ok / runs where runs includes other).
from .agent_job import JobStatus as _JobStatus  # noqa: E402

_STATUS_KEYS = tuple(
    s.value for s in _JobStatus
    if s.value not in ("pending", "running")  # transient, not a terminal outcome
)  # → ok, failed, timeout, rate_limited, skipped


def _load_state_safe(path: Path) -> dict[str, Any] | None | str:
    """Per-run isolated state load. Returns the dict on success, None when the
    file is absent, or the sentinel "bad" when it is corrupt/unreadable.

    A single historical corrupt state.json must not crash the whole cross-run
    scan — the caller skips "bad" runs and reports a count instead.
    """
    if not path.exists():
        return None
    try:
        state = st.load_state(path)
    except (ValueError, OSError, UnicodeDecodeError):
        # json.JSONDecodeError is a ValueError subclass — covered here.
        return "bad"
    return state if isinstance(state, dict) else None


def mine_model_effectiveness(repo: Path) -> dict[str, Any]:
    """Roll up runner × status across all runs' agent_runs.

    Returns::

        {
          "by_runner_model": {runner: {派工s(runs), distinct_runs, ok, failed,
              timeout, rate_limited, skipped, other, success_rate, avg_tokens,
              total_tokens, tokens_runs}},
          "timeline": [{run_id, runners: {runner: {ok, failed, ...}}}],
          "total_runner_results": int,
          "runs_with_agent_runs": int,
          "skipped_bad_runs": int,
        }

    ``runs`` is the count of AgentResult entries for that runner (a "派工").
    ``distinct_runs`` is how many distinct .lto runs that runner appeared in —
    THIS is the cross-run gate: effectiveness comparison requires distinct runs,
    not repeated dispatches inside one run. ``success_rate`` = ok / runs (the
    denominator includes skipped/other, both shown in the table for
    transparency). ``avg_tokens`` is over token-reporting results only.
    """
    by_runner: dict[str, dict[str, Any]] = {}
    runner_run_ids: dict[str, set[str]] = {}
    timeline: list[dict[str, Any]] = []
    total_results = 0
    runs_with_agent_runs = 0
    skipped_bad_runs = 0

    def _new_slot() -> dict[str, Any]:
        slot = {"runs": 0, "distinct_runs": 0, "other": 0,
                "total_tokens": 0, "tokens_runs": 0, "models": {}}
        for k in _STATUS_KEYS:
            slot[k] = 0
        return slot

    for run_id in _iter_runs(repo):
        state = _load_state_safe(repo / ".lto" / run_id / "state.json")
        if state == "bad":
            skipped_bad_runs += 1
            continue
        if not isinstance(state, dict):
            continue
        agent_runs = state.get("agent_runs") or {}
        if not isinstance(agent_runs, dict) or not agent_runs:
            continue

        run_has_result = False
        per_run: dict[str, dict[str, int]] = {}

        for results in agent_runs.values():
            if not isinstance(results, list):
                continue
            for r in results:
                if not isinstance(r, dict):
                    continue
                run_has_result = True
                total_results += 1
                runner = str(r.get("runner", "?"))
                status = str(r.get("status", "")).lower()
                # model 是 runner 下的具体型号（如 pi 跑 deepseek vs glm）。旧
                # agent_runs 无此字段 → None，按 runner 聚合不细分（向后兼容）。
                model = r.get("model")
                model = str(model) if isinstance(model, str) and model else None

                slot = by_runner.setdefault(runner, _new_slot())
                slot["runs"] += 1
                if model:
                    slot["models"][model] = slot["models"].get(model, 0) + 1
                if status in _STATUS_KEYS:
                    slot[status] += 1
                else:
                    slot["other"] += 1
                runner_run_ids.setdefault(runner, set()).add(run_id)

                tok = _result_tokens(r)
                if tok > 0:
                    slot["total_tokens"] += tok
                    slot["tokens_runs"] += 1

                pr = per_run.setdefault(runner, {k: 0 for k in (*_STATUS_KEYS, "other")})
                pr[status if status in _STATUS_KEYS else "other"] += 1

        if run_has_result:
            runs_with_agent_runs += 1
            timeline.append({"run_id": run_id, "runners": per_run})

    # derive rates / averages / distinct-run counts
    for runner, slot in by_runner.items():
        runs = slot["runs"]
        slot["distinct_runs"] = len(runner_run_ids.get(runner, set()))
        slot["success_rate"] = round(slot["ok"] / runs, 3) if runs else 0.0
        slot["avg_tokens"] = (
            round(slot["total_tokens"] / slot["tokens_runs"]) if slot["tokens_runs"] else 0
        )

    return {
        "by_runner_model": by_runner,
        "timeline": timeline,
        "total_runner_results": total_results,
        "runs_with_agent_runs": runs_with_agent_runs,
        "skipped_bad_runs": skipped_bad_runs,
    }


# ──────────────────────── track 2: phase friction ────────────────────────
#
# Source: events.jsonl. Aggregate by a "friction signal" across distinct runs,
# threshold by run count (à la interventions.recurring_friction): repeated
# events within one run = one recurring pattern, not many.
#
# Signals are facts derived from Phase 1 event fields, never model judgements.
# Two layers, kept simple:
#   1. Specific named signals (explicit, curated):
#        - "runner.finished rc!=0"       (a phase action failed)
#        - "runner.finished timeout=true" (a phase action timed out)
#        - "phase.changed churn (>=4/run)" (phase thrashing)
#   2. Generic per-type counts (so no event type is silently dropped):
#        - "task.status_changed (xN/run)" etc. — a type+count signature that
#          surfaces high-volume types (task churn, runner.started storms)
#          without hand-coding every type.


def _friction_signals(events: list[dict[str, Any]]) -> dict[str, int]:
    """Per-run friction signal → event count within this run."""
    signals: dict[str, int] = {}
    type_counts: dict[str, int] = {}
    phase_changes = 0
    for e in events:
        etype = e.get("type")
        if not isinstance(etype, str):
            continue
        type_counts[etype] = type_counts.get(etype, 0) + 1
        if etype == "runner.finished":
            fields = e.get("fields") or {}
            rc = fields.get("rc")
            if isinstance(rc, int) and rc != 0:
                signals["runner.finished rc!=0"] = signals.get("runner.finished rc!=0", 0) + 1
            if fields.get("timeout") is True:
                signals["runner.finished timeout=true"] = (
                    signals.get("runner.finished timeout=true", 0) + 1
                )
        elif etype == "phase.changed":
            phase_changes += 1
    if phase_changes >= 4:
        signals["phase.changed churn (>=4/run)"] = phase_changes
    # Generic high-volume type signature — catches churny types (task status
    # flapping, runner.started storms) that the curated signals above miss.
    # Threshold keeps it to genuinely repeated types within a single run.
    for etype, n in type_counts.items():
        if n >= 4 and etype not in ("phase.changed",):  # phase covered above
            signals[f"{etype} high volume (>=4/run)"] = n
    return signals


def mine_phase_friction(repo: Path, *, min_runs: int = 2) -> list[dict[str, Any]]:
    """Friction signals recurring across >= min_runs distinct runs.

    Returns ``[{signal, type, runs, count}]`` sorted by run count desc.
    ``runs`` = distinct .lto runs exhibiting the signal; ``count`` = total
    events; ``type`` = the originating event type (for host triage).
    """
    by_signal: dict[str, dict[str, Any]] = {}
    for run_id in _iter_runs(repo):
        events = ev.read(repo, run_id)
        if not events:
            continue
        for signal, count in _friction_signals(events).items():
            agg = by_signal.setdefault(
                signal, {"runs": 0, "count": 0, "type": signal.split(" ", 1)[0]}
            )
            agg["runs"] += 1          # once per run (within-run repeats = one pattern)
            agg["count"] += count
    out = [
        {"signal": sig, "type": agg["type"], "runs": agg["runs"], "count": agg["count"]}
        for sig, agg in by_signal.items()
        if agg["runs"] >= min_runs
    ]
    out.sort(key=lambda x: (-x["runs"], -x["count"], x["signal"]))
    return out


# ──────────────────────── combined mine ────────────────────────


def mine(repo: Path, *, min_runs: int = 2) -> dict[str, Any]:
    """Single cross-run scan → both signals. ``repo`` is the repo root."""
    repo = Path(repo)
    model = mine_model_effectiveness(repo)
    friction = mine_phase_friction(repo, min_runs=min_runs)
    return {
        "runs_scanned": len(_iter_runs(repo)),
        "model_effectiveness": model,
        "phase_friction": friction,
        "min_runs": min_runs,
    }


# ──────────────────────── host-facing brief ────────────────────────
#
# Wording iron law: evidence + derived signal + hypothesis, "由你(host)定".
# Never "必须派" / "自动选" / "promote" / "route to".

_INSUFFICIENT = (
    "数据不足。当前 .lto 里没有足够的 agent_runs / 事件历史来挖出可信信号——"
    "此处不编结论。后续积累更多真实派工与阶段事件后可复查。"
)


def render_mining_brief(repo: Path, *, min_runs: int = 2) -> str:
    """Markdown brief for the host agent. Evidence + hint, never a command."""
    data = mine(repo, min_runs=min_runs)
    me = data["model_effectiveness"]
    by_runner = me["by_runner_model"]
    friction = data["phase_friction"]

    lines = ["## 跨 run 挖掘 brief（证据 + 假设性提示，判断权归你）", ""]
    scan_line = (
        f"扫了 {data['runs_scanned']} 个 .lto run，"
        f"其中 {me['runs_with_agent_runs']} 个有 agent_runs（{me['total_runner_results']} 次派工）。"
    )
    bad = me.get("skipped_bad_runs", 0)
    if bad:
        scan_line += f"另有 {bad} 个 run 的 state.json 损坏/无法读取，已跳过（不计入聚合）。"
    lines.append(scan_line)
    lines.append("")

    # honest degradation: nothing to mine
    if me["total_runner_results"] == 0 and not friction:
        lines.append(_INSUFFICIENT)
        return "\n".join(lines)

    # ── Track 1: model effectiveness ──
    lines.append("### 模型有效性（数据源：agent_runs，按 runner 模型聚合）")
    lines.append("")
    if not by_runner:
        lines.append("- 没有任何 runner 派工记录——数据不足，此处不下结论。")
    else:
        lines.append(
            "| runner | 派工数 | 跨 run 数 | 成功率 | failed | timeout | rate_limited "
            "| skipped | other | avg tokens |"
        )
        lines.append("|---|---|---|---|---|---|---|---|---|---|")
        for runner, s in sorted(by_runner.items(), key=lambda kv: -kv[1]["runs"]):
            avg = _fmt_tokens(s["avg_tokens"]) if s["tokens_runs"] else "未计量"
            lines.append(
                f"| {runner} | {s['runs']} | {s['distinct_runs']} | {_pct(s['success_rate'])} "
                f"| {s['failed']} | {s['timeout']} | {s['rate_limited']} "
                f"| {s.get('skipped', 0)} | {s['other']} | {avg} |"
            )
        lines.append("")
        lines.append(
            "> 成功率分母 = 派工数（含 skipped/other），两列已列出便于核对低成功率的成因。"
        )
        lines.append("")
        # model 分布（runner 下的具体型号）——仅当 agent_runs 落了 model 字段才显示。
        # 旧 run 无 model → 不显示，按 runner 聚合（向后兼容）。
        model_lines = []
        for runner, s in sorted(by_runner.items(), key=lambda kv: -kv[1]["runs"]):
            models = s.get("models") or {}
            if models:
                dist = "，".join(
                    f"{mdl} {cnt}" for mdl, cnt in sorted(models.items(), key=lambda kv: -kv[1])
                )
                model_lines.append(f"- {runner}：{dist}")
        if model_lines:
            lines.append("**model 分布**（runner 下的具体型号）：")
            lines.extend(model_lines)
            lines.append("")
        # derived signal — hypothesis only, cross-run + low-N gated, host decides
        hint = _effectiveness_hint(by_runner, min_runs=min_runs)
        if hint:
            lines.append(hint)

    lines.append("")

    # ── Track 2: phase friction ──
    lines.append("### 阶段摩擦（数据源：events.jsonl，本地 shell 执行器，非模型）")
    lines.append("")
    lines.append(
        "> 注意：events 的 runner.finished 是 LTO 本地执行器（actor=lto-runner），"
        "不带异构模型——这一节只反映阶段动作摩擦，不是模型优劣。"
    )
    lines.append("")
    if not friction:
        lines.append(f"- 没有跨 >= {min_runs} 个 run 反复出现的阶段摩擦信号。")
    else:
        lines.append("| 摩擦信号 | 事件类型 | 出现的 run 数 | 累计事件数 |")
        lines.append("|---|---|---|---|")
        for f in friction:
            lines.append(f"| {f['signal']} | {f.get('type', '')} | {f['runs']} | {f['count']} |")
        lines.append("")
        lines.append(
            "这些信号在多个 run 反复出现，可能指向阶段流程的结构性摩擦（而非偶发）。"
            "是否值得改 harness、如何处理——由你定。"
        )

    return "\n".join(lines)


def _effectiveness_hint(by_runner: dict[str, dict[str, Any]], *, min_runs: int = 2) -> str:
    """A hypothesis hint, explicitly low-confidence & host-decides. Never a
    routing order. Returns "" when the sample is too thin to even hint.

    CROSS-RUN GATE (the whole point of ⑥): comparison requires runners that
    each appeared in >= min_runs DISTINCT .lto runs. 3 ok in one run vs 3 failed
    in the same run is within-run noise, not cross-run effectiveness — it must
    NOT trigger a "X 优于 Y" hint."""
    # only compare runners that span >= min_runs distinct runs
    eligible = [r for r in by_runner.items() if r[1].get("distinct_runs", 0) >= min_runs]
    ranked = sorted(
        eligible,
        key=lambda kv: (-kv[1]["success_rate"], -kv[1]["runs"]),
    )
    if len(ranked) < 2:
        return (
            f"> 派生提示：样本仅来自有限的真实 run（能跨 >= {min_runs} 个 run 的 runner "
            "不足两个），跨 run 有效性无法比较。此处不下结论，后续积累更多真实 run 后可复查。"
        )
    best_name, best = ranked[0]
    worst_name, worst = ranked[-1]
    if best["success_rate"] - worst["success_rate"] < 0.15:
        return (
            "> 派生提示：在跨 run 样本里各 runner 成功率差距不大，没有明显赢家。"
            "如何取舍由你定，此处不代为选择。"
        )
    return (
        f"> 派生提示（假设，非测量结论）：在已有跨 run 样本里 "
        f"{best_name} 成功率 {_pct(best['success_rate'])}"
        f"（{best['runs']} 次派工 / {best['distinct_runs']} 个 run）"
        f"高于 {worst_name} {_pct(worst['success_rate'])}"
        f"（{worst['runs']} 次 / {worst['distinct_runs']} 个 run）。"
        f"这是供你参考的一个观察，下一步如何取舍由你(host)定；样本仍偏小，非定论，亦非路由指令。"
    )


def _pct(rate: float) -> str:
    return f"{round(rate * 100)}%"


def _fmt_tokens(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)
