"""lto recap — 面向人类的回顾视图（不是面向 agent 的状态快照）。

与 resume 正交：
- resume 给接手的 AI 看（git head / task id / phase 枚举 / drift / dirty）——
  目的是让 agent 在 compact 后不丢上下文。
- recap 给人看——长任务跨 session 后，人会忘了当初要做什么、为什么、跑了多久。
  recap 用人话回答六个问题，数据全来自现有 state.json。

来源洞察（真实 session 日志挖掘，2026-06-03）：claude 长 session 10-15% 显式出现
"人忘记进展"——跨 session 断裂后用户手动重注约束、忘了早先定的验收标准。
resume 的 git-head 快照救不了一个隔了 87 小时回来的人。
"""

from __future__ import annotations

import argparse
import time
from datetime import datetime
from pathlib import Path

from .. import state as st
from .. import artifacts as af


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    print(_render_recap(state, run_id, repo=repo, include_artifacts=args.artifacts))

    # opt-in 跨 run 挖掘（默认关）：给 host 一份「越用越聪明」的证据 brief。
    # 默认不打扰人类回顾；只有显式 --mine 才附。仍是证据+假设，绝不路由。
    if getattr(args, "mine", False):
        from .. import cross_run_mining as crm
        print()
        print(crm.render_mining_brief(repo))
    return 0


def _render_recap(state: dict, run_id: str, *, repo: Path | None = None, include_artifacts: bool = False) -> str:
    goal = state.get("goal", "(未记录目标)")
    why = state.get("why", "") or state.get("original_user_request", "")
    done_when = state.get("done_when", "")
    started = state.get("started_at", "")
    phase = state.get("current_phase", "?")
    tasks = state.get("tasks", [])
    next_action = state.get("next_action", "") or ""
    blocked_by = state.get("blocked_by", "none")

    lines = [
        "╭─ LTO Recap ─ 给人看的回顾（不是给 AI 看的状态）",
        "│",
        f"│ 你当初要做什么 ── {goal}",
    ]

    # 为什么
    if why and why != goal:
        lines.append(f"│ 为什么要做 ────── {st.single_line(why)}")
    else:
        lines.append("│ 为什么要做 ────── （未记录 — 下次 lto start 加 --why 补上）")

    # 跑了多久
    elapsed = _elapsed_human(started)
    gaps = _session_gaps_human(state)
    dur_line = f"│ 跑了多久 ──────── {elapsed}"
    if gaps:
        dur_line += f"，{gaps}"
    lines.append(dur_line)

    # 已做到哪（done task + milestone）
    done = [t for t in tasks if t.get("status") == "done"]
    lines.append("│ 已经做到哪 ────── " + _done_summary(done, state))

    # 还剩什么
    pending = [t for t in tasks if t.get("status") == "pending"]
    blocked = [t for t in tasks if t.get("status") == "blocked"]
    lines.append("│ 还剩什么 ──────── " + _remaining_summary(pending, blocked, done_when))

    # 现在轮到你（决策点）
    # 花了多少 token（per-run 汇总，跨所有 agent_runs）
    token_line = _token_summary(state)
    if token_line:
        lines.append("│ 花了多少 token ── " + token_line)

    lines.append("│ 现在轮到你 ────── " + _next_for_human(state, blocked, next_action, blocked_by))
    if include_artifacts and repo is not None:
        lines.append("│ 关键产物 ──────── " + _artifact_summary(repo, run_id))

    # 当前在跑的 job（扫 live/ 目录，mtime 近 120s 内）
    if repo is not None:
        running = _running_jobs(repo, run_id, window_sec=120)
        if running:
            lines.append("│ 当前在跑 ──────── " + "；".join(running))

    lines.append("│")
    lines.append(f"╰─ run: {run_id}  phase: {phase}")
    return "\n".join(lines)


def _token_summary(state: dict) -> str:
    """One-line per-run token usage for humans. Empty string if nothing ran."""
    roll = st.token_rollup(state)
    if roll["runs_total"] == 0:
        return ""
    total = roll["total_tokens"]
    wt, rt = roll["runs_with_tokens"], roll["runs_total"]
    if total == 0:
        return f"未计量（{rt} 次派工，无 runner 上报 token；agy 等 CLI 不暴露用量）"
    # per-runner breakdown, biggest first
    parts = []
    for runner, s in sorted(roll["by_runner"].items(), key=lambda kv: -kv[1]["tokens"]):
        if s["tokens"] > 0:
            parts.append(f"{runner} {_fmt_tokens(s['tokens'])}")
    by = "，".join(parts)
    coverage = "" if wt == rt else f"（{wt}/{rt} 次派工有计量）"
    el = roll.get("total_elapsed_sec") or 0
    el_part = f" · 派工累计 {_fmt_duration(el)}" if el > 0 else ""
    return f"约 {_fmt_tokens(total)} tokens{coverage}：{by}{el_part}"


def _fmt_duration(sec: float) -> str:
    """秒 → 人话（95→1m35s / 3700→1h2m）。"""
    s = int(sec)
    if s < 60:
        return f"{s}s"
    if s < 3600:
        return f"{s // 60}m{s % 60}s"
    return f"{s // 3600}h{(s % 3600) // 60}m"


def _fmt_tokens(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}k"
    return str(n)


def _artifact_summary(repo: Path, run_id: str) -> str:
    entries = af.recent(repo, run_id, limit=5)
    if not entries:
        return "未发现已登记产物"
    parts = []
    for entry in entries:
        marker = "*" if entry.get("source") == "synthesized" else ""
        parts.append(f"{entry.get('kind')}:{entry.get('run_relative_path', entry.get('relative_path'))}{marker}")
    return "；".join(parts)


def _elapsed_human(started: str) -> str:
    if not started:
        return "（无开始时间）"
    try:
        start_dt = datetime.fromisoformat(started)
    except ValueError:
        return started
    now = datetime.now(start_dt.tzinfo)
    delta = now - start_dt
    days = delta.days
    hours = delta.seconds // 3600
    if days > 0:
        return f"{days} 天{f' {hours} 小时' if hours else ''}"
    if hours > 0:
        return f"{hours} 小时"
    return f"{delta.seconds // 60} 分钟"


def _session_gaps_human(state: dict) -> str:
    """从 phase_transitions 推断 session 间隔（中间停了多久）。"""
    transitions = state.get("phase_transitions", [])
    max_gap_hours = 0.0
    prev_dt = None
    for tr in transitions:
        at = tr.get("at", "")
        if not at:
            continue
        try:
            dt = datetime.fromisoformat(at)
        except ValueError:
            continue
        if prev_dt is not None:
            gap = (dt - prev_dt).total_seconds() / 3600
            max_gap_hours = max(max_gap_hours, gap)
        prev_dt = dt
    if max_gap_hours >= 24:
        return f"中间最长停了 {int(max_gap_hours)} 小时（约 {int(max_gap_hours/24)} 天）"
    if max_gap_hours >= 2:
        return f"中间停过 {int(max_gap_hours)} 小时"
    return ""


def _done_summary(done: list[dict], state: dict) -> str:
    if not done:
        # 退到 phase transition 看走了哪些阶段
        phases = [tr.get("to", "") for tr in state.get("phase_transitions", [])]
        if phases:
            return "走过阶段：" + " → ".join(p for p in phases if p)
        return "还没有完成的任务"
    titles = [t.get("title", t.get("id", "?")) for t in done]
    head = "、".join(titles[:4])
    more = f" 等 {len(titles)} 项" if len(titles) > 4 else ""
    return f"已完成 {len(done)} 项：{head}{more}"


def _remaining_summary(pending: list[dict], blocked: list[dict], done_when: str) -> str:
    parts = []
    if blocked:
        bt = blocked[0]
        reason = ""
        if bt.get("blockers"):
            reason = f"（卡在：{st.single_line(bt['blockers'][-1].get('reason', ''))[:40]}）"
        parts.append(f"{len(blocked)} 项卡住{reason}")
    if pending:
        parts.append(f"{len(pending)} 项待做")
    if not parts:
        if done_when:
            return f"看起来都做完了。验收标准：{st.single_line(done_when)}"
        return "看起来任务都完成了"
    tail = ""
    if done_when:
        tail = f"。算做完的标准：{st.single_line(done_when)}"
    return "；".join(parts) + tail


def _next_for_human(state: dict, blocked: list[dict], next_action: str, blocked_by: str) -> str:
    if state.get("current_phase") == "closed":
        return "这个任务已经收尾（closed）。可以开新的了。"
    if blocked_by and blocked_by != "none":
        return f"需要你处理：{st.single_line(blocked_by)}"
    if blocked:
        return f"决定怎么处理那 {len(blocked)} 个卡住的任务（修、跳过、还是换思路）"
    if next_action:
        return st.single_line(next_action)
    return "跑 `lto next` 看系统建议的下一步，或继续推进待做项"


def _running_jobs(repo: Path, run_id: str, window_sec: float = 120) -> list[str]:
    """扫 .lto/<run-id>/live/*.log，返回 mtime 在 window_sec 内的描述列表。

    格式："<job_id>（N 秒前有输出）"。无 live/ 目录时优雅降级返回空列表。
    """
    live_dir = repo / ".lto" / run_id / "live"
    if not live_dir.exists():
        return []
    now = time.time()
    results = []
    try:
        for log_file in sorted(live_dir.glob("*.log")):
            try:
                mtime = log_file.stat().st_mtime
            except OSError:
                continue
            age = now - mtime
            if age <= window_sec:
                job_id = log_file.stem
                age_str = f"{int(age)}秒前有输出" if age >= 1 else "刚有输出"
                results.append(f"{job_id}（{age_str}）")
    except OSError:
        return []
    return results


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "recap",
        help="human-facing recap: what you set out to do, why, how long, where you are, what's next",
    )
    p.add_argument("--run-id")
    p.add_argument("--artifacts", action="store_true", help="include recent artifact paths")
    p.add_argument("--mine", action="store_true",
                   help="append cross-run mining brief (model effectiveness + phase friction; evidence only)")
    p.set_defaults(func=run)
