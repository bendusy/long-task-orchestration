"""lto autopilot — 自驱动 orchestrator（分阶段实现）。

当前阶段（spec 阶段 3）：仅 --supervised 的"增强 brief"形态。
- 读状态 → analyze → 出富决策简报 + 候选动作 + 路由建议 → 回吐宿主 LLM。
- **此阶段不自动执行任何命令**。自动执行档（阶段 5）需 worktree 沙箱
  四件套验收红线全绿才开（spec 强制规则）。
- 集成 G4 progress 做 stall 提示（与上一次 autopilot 快照比）。

设计契约（三方两轮复核定）：
- 决策权留宿主 LLM；autopilot 只整理事实 + 建议，不替宿主决定。
- --autonomous 档（spawn 决策 agent）不进本期。
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import progress as pg
from .. import worktree_exec as wx
from .. import evidence as ev
from .. import decision as dec
from .. import artifacts as af
from .. import cross_run_mining as crm
from ..autopilot_status import (
    AutopilotStatus,
    EXIT_CODES,
    STATUS_PRIORITY,
    emit_terminal_status,
    stronger_status,
)
from . import next as next_cmd


# ── autonomous 证据闸门 ──
# autonomous 让 LTO 在 escalate 点自动推进，比 supervised/auto-exec 多放一档权。
# 按 LTO 哲学「自动化是梯度，每升一级必须增加证据」——autonomous 不是裸 loop，
# 是一道数据驱动的闸门：只有跨 run 攒够真实派工样本后才解锁，否则诚实拒绝、
# 退回 supervised，并明示「还差多少数据」。判据来自 ⑥ 的 cross_run_mining：
#   - 真实派工样本（runs_with_agent_runs）：autonomous 要学的是真实运行模式，
#     零真实 run 谈不上自动化，等于凭空拍。
#   - 阈值保守，宁可继续要人类确认，也不在无证据时放权。
# git push / escalate / dangerous 子步骤永远停人类——闸门过了也不放开任何安全红线。
_AUTONOMOUS_MIN_RUNS = 5          # 至少 5 个有真实派工的 run 才谈自动化
_AUTONOMOUS_MIN_RESULTS = 10      # 且累计 >= 10 条真实派工结果


def _autonomous_gate(repo: Path) -> tuple[bool, str]:
    """autonomous 证据闸门：跨 run 攒够真实派工样本才放行。

    返回 (passed, reason)。不过则 reason 说明还差多少数据。零 LLM、纯事实。
    """
    try:
        mined = crm.mine(repo)
    except Exception as exc:  # 挖掘失败绝不放行（fail-closed）
        return False, f"挖掘跨 run 数据失败，fail-closed 拒绝 autonomous: {exc}"
    # 严格 schema fail-closed：任何畸形返回都拒绝，绝不放行。mined / model_effectiveness
    # 必须是 dict；计数必须是非 bool 的 int >= 0（拒绝 "5" 字符串、None、bool、NaN）。
    if not isinstance(mined, dict):
        return False, "fail-closed：mine() 返回非 dict，拒绝 autonomous"
    me = mined.get("model_effectiveness")
    if not isinstance(me, dict):
        return False, "fail-closed：model_effectiveness 缺失或非 dict，拒绝 autonomous"

    def _strict_count(v: object) -> int | None:
        return v if isinstance(v, int) and not isinstance(v, bool) and v >= 0 else None

    # 优先用严格计数（只数合规 AgentResult）；mine 的宽松 brief 计数不可信（空 {} 也算）。
    runs = _strict_count(me.get("gate_runs"))
    results = _strict_count(me.get("gate_results"))
    if runs is None:
        runs = _strict_count(me.get("runs_with_agent_runs"))
    if results is None:
        results = _strict_count(me.get("total_runner_results"))
    if runs is None or results is None:
        return False, "fail-closed：闸门计数字段缺失或类型非法，拒绝 autonomous"
    if runs < _AUTONOMOUS_MIN_RUNS or results < _AUTONOMOUS_MIN_RESULTS:
        return False, (
            f"证据不足：autonomous 需要跨 >= {_AUTONOMOUS_MIN_RUNS} 个真实派工 run "
            f"且 >= {_AUTONOMOUS_MIN_RESULTS} 条派工结果，当前只有 {runs} run / "
            f"{results} 结果。先用 --supervised [--auto-exec] 在真实任务里攒数据，"
            f"⑥ 跨 run 挖掘会随数据积累自动满足此闸门。"
        )
    return True, (
        f"证据闸门通过：{runs} run / {results} 派工结果（>= {_AUTONOMOUS_MIN_RUNS}/"
        f"{_AUTONOMOUS_MIN_RESULTS}）。autonomous 在 auto-exec 沙箱基础上受闸推进；"
        f"git push / escalate / dangerous 仍停人类。"
    )


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    mode = "autonomous" if args.autonomous else "supervised"

    # ── autonomous：机械证据闸门 + 机械执行，绝不 spawn 决策 agent ──
    # 设计边界（用户 2026-06-09 定）：LTO 不接 LLM 处理数据，产出全是机械的事实+
    # 派生信号；反思/决策永远归主 agent。所以 autonomous **不是** LTO 自主决策回路，
    # 它只做两件机械的事：(1) 读 ⑥ 跨 run 事实判证据闸门；(2) 过闸后在 worktree 沙箱
    # 机械执行 safe/reversible 子步骤（与 auto-exec 同一套，只是允许连续推进）。
    # escalate / dangerous / git push 一律回吐主 agent，LTO 不替它反思、不 spawn
    # 决策 agent。闸门不过 → 诚实退回 supervised，明示还差多少真实数据。
    if mode == "autonomous":
        passed, gate_reason = _autonomous_gate(repo)
        print(f"# LTO Autopilot — autonomous（机械闸门 + 机械执行，不替你决策）")
        print(f"  evidence gate: {'PASS' if passed else 'BLOCKED'}")
        print(f"  reason: {gate_reason}")
        if not passed:
            print(
                "\n  → autonomous 未解锁：这是数据驱动的闸门，不是硬编码禁用。\n"
                "     用 --supervised [--auto-exec] 在真实任务里攒派工数据，够了自动解锁。"
            )
            emit_terminal_status(AutopilotStatus.NEEDS_CONFIRM, "autonomous evidence gate not met")
            return EXIT_CODES[AutopilotStatus.NEEDS_CONFIRM]
        # 闸门过：强制走 auto-exec 机械执行路径（下方共用 supervised 主体 + auto_exec）。
        # autonomous 不引入新的执行/决策逻辑，只是「过闸后默认开 auto-exec 连续推进」。
        args.auto_exec = True
        # 硬边界：autonomous **绝不** spawn 决策 agent。--autonomous --decide 组合时
        # 强制清掉 decide——否则 escalate 会进 _run_decide() 派三方异构 agent 替你
        # 反思，正是用户边界禁止的「LTO 替主 agent 决策」。autonomous 只机械执行 +
        # 机械闸门，反思永远归你。
        if getattr(args, "decide", False):
            print("  ⚠️ --autonomous 与 --decide 互斥：autonomous 不 spawn 决策 agent，已忽略 --decide。")
            args.decide = False
        print("  → 闸门通过：在 auto-exec 沙箱里机械推进 safe 子步骤（escalate 仍回吐你）。\n")

    # ── supervised：仅 brief 形态 ──
    facts = next_cmd.analyze(state, repo)
    route = next_cmd.route(facts)

    # stall 检测：与上次 autopilot 快照比
    curr_digest = pg.progress_digest(state, repo)
    prev_digest = state.get("gates", {}).get("autopilot_last_digest", {})
    progressed, why = pg.has_progressed(prev_digest, curr_digest)

    # 输出决策简报（复用 next 的富简报）
    brief = next_cmd.build_decision_brief(facts, state)
    print(brief)
    print()

    # autopilot 档位状态
    auto_exec = getattr(args, "auto_exec", False)
    label = "supervised (auto-exec)" if auto_exec else "supervised (brief-only)"
    print(f"# LTO Autopilot — {label}")
    print(f"  mode: {mode}")
    print(f"  progress since last check: {'YES' if progressed else 'STALLED'} ({why})")
    print(f"  route: {route['action'].upper()} (unambiguous={route['unambiguous']})")
    print(f"  reason: {route['reason']}")

    # stall 刹车：无推进时不自动执行（避免在卡死任务上空转）
    terminal_status = AutopilotStatus.DONE
    terminal_reason = "supervised brief emitted"
    route_needs_host = False
    if not progressed and prev_digest:
        print(
            "\n  ⚠️ STALLED：与上次相比状态无实质推进（同失败指纹 / 无单调改善）。\n"
            "     反复跑同一个修不好的东西没有意义——停止自动执行，换思路或回吐人决策。"
        )
        auto_exec = False  # 停滞时强制退回 brief
        terminal_status = AutopilotStatus.STALLED
        terminal_reason = "no progress since previous autopilot digest"

    if route["action"] == "run" and route.get("unambiguous"):
        print(f"  suggested cmd: {route.get('cmd', '(n/a)')}")
        route_status = AutopilotStatus.NEEDS_CONFIRM
        route_reason = f"supervised route suggests command: {route.get('cmd', '(n/a)')}"
        if not auto_exec:
            print(
                "\n  ⚠️ 自动执行档未启用（默认）。加 --auto-exec 让 supervised 在 worktree\n"
                "     沙箱里自动跑 safe/reversible 子步骤。现在请宿主 LLM/人确认后手动执行。"
            )
        # 注：route 的无歧义命令是 lto judge/closeout 等 LTO 子命令；closeout 不可逆
        # 仍需人确认，autopilot 不自动 closeout。真正的自动执行针对 task 待跑命令（下方）。
        terminal_status = stronger_status(terminal_status, route_status)
        if terminal_status == route_status:
            terminal_reason = route_reason
    else:
        # else = route 不是「无歧义可跑」：含 action=escalate，也含 action=run 但
        # unambiguous=False（next 给了建议命令但信心不足）。两者都该回吐宿主判断，
        # --decide 时则派三方讨论——对这两种「需要判断」的态都成立。
        decide = getattr(args, "decide", False)
        if decide:
            # 宿主显式 opt-in：在这个 escalate 点派三方异构 agent 收敛（双轨引擎）。
            # 决策权仍留宿主——run_decision 出收敛 brief 给宿主读，不替宿主拍板执行。
            # G5 dedup / needs_human 回退 / budget 耗尽降级全在引擎内处理。
            decide_status, decide_reason = _run_decide(repo, run_id, facts, state, state_path, args)
            terminal_status = stronger_status(terminal_status, decide_status)
            if terminal_status == decide_status:
                terminal_reason = decide_reason
        else:
            print(
                "\n  → escalate：需要判断（多 blocked / 方案分歧 / 高风险 / 空 phase）。\n"
                "     请宿主 LLM 读上面的决策简报，自己推理下一步该用哪个 pattern。\n"
                "     或加 --decide 让 LTO 派三方异构 agent 讨论收敛（opt-in，烧 token）。"
            )
            host_status = AutopilotStatus.NEEDS_HOST
            route_needs_host = True
            terminal_status = stronger_status(terminal_status, host_status)
            if terminal_status == host_status:
                terminal_reason = route["reason"]

    # ── 自动执行档：对 pending/in_progress task 的 safe/reversible 命令，经沙箱自动跑 ──
    if auto_exec:
        exec_status, exec_reason = _auto_exec_tasks(repo, run_id, state, state_path, args)
        if route_needs_host and exec_status != AutopilotStatus.DONE:
            terminal_status = exec_status
            terminal_reason = exec_reason
        else:
            terminal_status = stronger_status(terminal_status, exec_status)
            if terminal_status == exec_status:
                terminal_reason = exec_reason

    # 落快照（供下次 stall 比对）+ 单向棘轮
    gates = state.setdefault("gates", {})
    gates["autopilot_last_digest"] = curr_digest
    pg.update_high_water(state, curr_digest)
    st.save_state(state_path, state)

    emit_terminal_status(terminal_status, terminal_reason)
    return EXIT_CODES[terminal_status]


_RETRY_LIMIT = 3

# --decide 默认 token 预算：够烧一轮 both（2×18K=36K）+ 余量。
# 不够时 run_decision 自己优雅降级为 budget_exhausted → needs_human，安全。
_DEFAULT_DECIDE_BUDGET = 50_000


def _auto_exec_tasks(repo: Path, run_id: str, state: dict, state_path: Path, args) -> tuple[AutopilotStatus, str]:
    """对 pending/in_progress task 的待跑命令，经 worktree 沙箱自动执行。

    安全全靠 worktree_exec：dangerous/逃逸/网络-push 一律 needs_semantic_judgement
    不执行；safe/reversible 在 worktree 沙箱里跑。本函数只管选 task + 落 evidence +
    retry/stall 刹车。closeout/deploy 等不可逆永不在此自动执行。
    """
    print("\n  ── auto-exec (worktree sandbox) ──")
    tasks = state.get("tasks", [])
    candidates = [
        t for t in tasks
        if t.get("status") in ("pending", "in_progress")
        and t.get("commands_run")  # 有待重跑的命令
    ]
    if not candidates:
        print("    no pending/in_progress task with a command to auto-run")
        return AutopilotStatus.DONE, "no task command candidates for auto-exec"

    executed_any = False
    held_count = 0
    failed_count = 0
    retry_blocked_count = 0
    for task in candidates:
        tid = task.get("id", "")
        command = task["commands_run"][-1]

        # G2 retry 刹车：同命令失败 >= 3 次，不再自动重试
        if task.get("retry_count", 0) >= _RETRY_LIMIT:
            print(f"    [{tid}] SKIP — retry_count={task['retry_count']} >= {_RETRY_LIMIT} (needs human)")
            retry_blocked_count += 1
            continue

        # autonomous（最自动的档）默认禁网：curl/wget/nc/ssh/scp 有外部副作用，
        # 不属 safe/reversible，沙箱挡不住——codex 审 ③ HIGH。supervised --auto-exec
        # 是人在场的半自动，保留默认放网。
        allow_net = not getattr(args, "autonomous", False)
        result = wx.run_in_sandbox(
            repo, command, timeout=getattr(args, "timeout", 300), allow_network=allow_net
        )

        if not result.executed:
            print(f"    [{tid}] HELD — {result.effect.level}: {result.effect.reason}")
            print(f"           command needs human confirm: {command[:80]}")
            held_count += 1
            continue

        executed_any = True
        rc = result.rc if result.rc is not None else 1
        if rc != 0:
            failed_count += 1
        status_icon = "✓" if rc == 0 else "✗"
        print(f"    [{tid}] {status_icon} rc={rc} (sandbox) — {command[:60]}")

        # 落 evidence（沙箱执行不改主树，故 head 不变）
        head = gs.git_head(repo)
        evd = ev.record_evidence(
            kind="test", command=command, cwd=result.worktree or str(repo),
            rc=rc, head_before=head, head_after=head,
            summary=f"autopilot sandbox: {'PASS' if rc == 0 else 'FAIL'}",
            verified_by="autopilot",
        )
        task.setdefault("evidence", []).append(evd)
        if rc != 0:
            # 复用 runner 的 retry 计数维度（按命令指纹）
            from .runner import _bump_retry
            _bump_retry(task, command)
        task["last_update"] = st.iso_now()

    if executed_any:
        st.save_state(state_path, state)
        print("    auto-exec results saved to state.json (evidence recorded)")
    else:
        print("    nothing auto-executed (all held for human confirm)")
    outcomes: list[tuple[AutopilotStatus, str]] = []
    if held_count:
        outcomes.append((
            AutopilotStatus.NEEDS_CONFIRM,
            f"{held_count} task command(s) held for human confirmation",
        ))
    if retry_blocked_count:
        outcomes.append((
            AutopilotStatus.NEEDS_HUMAN,
            f"{retry_blocked_count} task command(s) exceeded retry limit",
        ))
    if failed_count:
        outcomes.append((
            AutopilotStatus.NEEDS_HOST,
            f"{failed_count} sandbox command(s) failed",
        ))
    if outcomes:
        status = AutopilotStatus.DONE
        for candidate, _reason in outcomes:
            status = stronger_status(status, candidate)
        reasons = [
            reason for _status, reason in sorted(
                outcomes,
                key=lambda item: STATUS_PRIORITY[item[0]],
                reverse=True,
            )
        ]
        return status, "; ".join(reasons)
    return AutopilotStatus.DONE, "auto-exec completed without held or failed commands"


def _suggest_decision_kind(facts: dict) -> str:
    """从 facts 给 decision_kind 一个建议（宿主可用 --decide-kind 覆盖）。

    - 有 blocked / 多 pending 在竞争下一步 → direction（选路线，投票）。
    - 否则默认 review（找问题/风险，union 合并不投票）——更保守、无需共同选项。
    主 agent 在场时应自己判 kind；本函数只是 autopilot 的缺省建议。
    """
    if facts.get("blocked") or len(facts.get("pending", [])) >= 2:
        return "direction"
    return "review"


def _run_decide(
    repo: Path,
    run_id: str,
    facts: dict,
    state: dict,
    state_path: Path,
    args,
) -> tuple[AutopilotStatus, str]:
    """escalate 点派三方异构 agent 收敛，打印 brief 给宿主读。"""
    print("\n  ── decide (三方异构收敛) ──")

    # 空 phase（无 task）派三方等于让它们在白纸上找问题，纯噪声 + 白烧 budget。
    # 拒绝派工，回吐宿主自己拆 task。
    if not facts.get("has_tasks"):
        print(
            "    ⚠️ 当前 phase 无 task，派三方没有可讨论的对象（只会得到噪声）。\n"
            "       请宿主先把目标拆成 task（lto task-add）再 --decide，或直接人工决策。"
        )
        return AutopilotStatus.NEEDS_HOST, "current phase has no task to discuss"

    kind = getattr(args, "decide_kind", None) or _suggest_decision_kind(facts)
    # budget：argparse default=None 区分「没传」与「显式传 0」。
    # None → 默认 50000；显式 0 是合法意图（如 CI 禁烧 token），尊重为 0 → budget_exhausted。
    # 负数无意义（会显示 ≈-1 困惑用户），当无效输入回落默认。
    raw_budget = getattr(args, "decide_budget", None)
    if raw_budget is not None and raw_budget < 0:
        print(f"    ⚠️ --decide-budget {raw_budget} 为负，无意义，回落默认 {_DEFAULT_DECIDE_BUDGET}。")
        raw_budget = None
    budget = _DEFAULT_DECIDE_BUDGET if raw_budget is None else raw_budget

    print(f"    decision_kind: {kind}  (覆盖用 --decide-kind direction|review|both)")
    print(f"    budget_remaining≈{budget} tokens (覆盖用 --decide-budget)")

    # spawn 阶段（真起子进程）可能抛 runner 缺失/超时/IO 等异常——引擎内部只对
    # budget/dedup 做了优雅降级，spawn 异常没接。这里兜住，降级 brief-only 不崩 autopilot。
    try:
        result = dec.run_decision(
            repo, run_id, facts, state,
            decision_kind=kind,
            budget_remaining=budget,
        )
    except Exception as exc:
        # 降级标记同时进 stdout（宿主 LLM 读的主流）+ stderr（诊断）。stdout 给明确
        # 哨兵串让宿主能识别"这次 decide 没成、按 brief-only 处理"，不被半截块误导。
        print(f"\n    [decide] DEGRADED → brief-only — {type(exc).__name__}: {exc}")
        print("    决策引擎 spawn 异常，本次不收敛。请宿主读上方决策简报手动判断或重试。")
        print(f"    [decide] FAILED — {type(exc).__name__}: {exc}", file=sys.stderr)
        return AutopilotStatus.ERROR, f"decision engine failed: {type(exc).__name__}"

    # run_decision 已 mutate state 记 escalate dedup。run() 末尾还会 save 一次落 digest，
    # 但这里提前落盘是为了：若后续 _auto_exec_tasks 抛异常，dedup 记录不丢（防同点重 spawn）。
    st.save_state(state_path, state)

    print(f"    status: {result['status']}  dispatched_to: {result.get('dispatched_to', [])}")
    print(f"    budget_consumed_est: {result.get('budget_consumed_est', 0)}")
    print()
    # 收敛 brief 是给宿主 LLM 读的核心产物
    ts = st.iso_now().replace(":", "-")[:19]
    af.write_text(
        repo, run_id, f"audit/decision-host-brief-{ts}.md", result["brief"],
        kind="decision_host_brief", producer="lto.commands.autopilot.decide",
        state=state, summary=f"{kind} decision host brief",
        tags=["decision", "host-brief"],
    )
    print(result["brief"])
    if result.get("dissent", {}).get("reason") == "budget_exhausted":
        return AutopilotStatus.BUDGET_EXHAUSTED, "--decide budget exhausted before dispatch"
    if result["status"] == "needs_human":
        return AutopilotStatus.NEEDS_HUMAN, "decision engine requires human judgement"
    if result["status"] == "needs_info":
        return AutopilotStatus.NEEDS_HUMAN, "decision agents did not converge"
    return AutopilotStatus.NEEDS_HOST, "decision brief emitted for host judgement"


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "autopilot",
        help="self-driving orchestrator (supervised brief-only; autonomous is next-phase)",
    )
    p.add_argument("--run-id")
    p.add_argument("--supervised", action="store_true",
                   help="supervised mode: emit decision brief + route, escalate to host (default)")
    p.add_argument("--auto-exec", dest="auto_exec", action="store_true",
                   help="auto-run safe/reversible task commands in worktree sandbox (opt-in)")
    p.add_argument("--timeout", type=int, default=300, help="per-command timeout for auto-exec")
    p.add_argument("--decide", action="store_true",
                   help="on escalate, spawn tri-partite heterogeneous agents to converge "
                        "(opt-in, burns token; host still reads the brief and decides)")
    p.add_argument("--decide-kind", dest="decide_kind",
                   choices=["direction", "review", "both"],
                   help="decision track for --decide (default: inferred from state)")
    p.add_argument("--decide-budget", dest="decide_budget", type=int, default=None,
                   help="approx token budget for --decide (default 50000; pass 0 to force "
                        "needs_human without spawning; engine degrades gracefully if insufficient)")
    p.add_argument("--autonomous", action="store_true",
                   help="autonomous mode (NOT YET IMPLEMENTED — spec phase 6)")
    p.set_defaults(func=run)
