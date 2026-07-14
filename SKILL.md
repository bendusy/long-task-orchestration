---
name: long-task-orchestration
description: >-
  长任务 harness——帮 host agent 在 spec→审计→开发→部署→实测→记录 六个阶段保留状态、
  证据、审计、恢复点和人工刹车。
  Use when 一个任务要好多轮才能完成（设计+开发+上线），你担心做过头或者做着做着跑偏了。
  Do not trigger on 修个小 bug（走 diagnose）、让别人审一下代码（走 review）、
  只部署一下（走 ship）。普通「起个 spec / 做个功能」走对应专职 skill，
  只有当任务已经来回折腾好几轮、需要有人帮你踩刹车时才进 LTO。
metadata:
  tier: agent-driven
  domain: infra
  optional_integrations: [am, memory-flow]
  status: active
allowed-tools: [Bash, Read, Write, Edit, Task, AskUserQuestion]
---

# LTO：长任务 harness（四层引擎）

大功能要 50 轮对话，中间你会迷路：不知道做到哪了、是不是做过头了、什么时候该停。
LTO **不替你写代码、不替你选路**——host agent 是 planner；LTO 是 harness：state、
artifact、audit、runner、sandbox、resume/recap、human gate，让你有证据、有刹车、
有恢复点地推进。

## ① ROUTER · 先路由再读文档

入口顺序（P1，不可跳）：

1. **先看有无 active run**：`lto runs` / `.lto/current`。有 → 域Ⅰ接管；没有 → 才谈域Ⅱ立项。
2. **判动作意图 → 域**（下表）。3. **高风险才跨域**：默认读 1 个 reference，跨域/安全 ≤2。

| 意图 | 域 | 主 reference |
|---|---|---|
| 接手项目 / compact 恢复 | Ⅰ 接管与恢复 | onboarding.md、long-loop-state.md |
| 开新 run / 写交付契约 | Ⅱ 立项与契约 | run-state-workflow.md |
| 派 agent / 等完成 / autopilot | Ⅲ 执行与派工 | execution-loop.md |
| 多模型审 / 判收敛 | Ⅳ 验证与收敛 | audit-convergence.md、playbooks/review.md |
| 上线 / 发版 / 收尾 | Ⅴ 交付与发布 | deploy-sequencing.md、release-workflow.md |
| 记决策 / 记忆 / 挖掘 / 清理 | Ⅵ 学习与维护 | decision-logging.md、events-telemetry-contract.md |

六域详表 + 跨域场景 + 文档状态标注的唯一真源：`references/INDEX.md`。

## ② OPERATING POLICY · 推理纪律

**P1 必须**：
- **人说了算**：phase 切换、不可逆动作、语义争议、closeout 都过 human gate；三个 AI 都说「没问题」≠ 真没问题，autopilot 任何档位都不取消这条。
- **先观测后控制**：动作前先读 `.lto`/git/runtime 状态；未能观测的行为不自动化。
- **审者 ≠ host**：主观/对抗审计必须异构（你是 claude 就派 codex/pi/agy）；确定性测试直接跑，不需异构。
- **信息不足先自助补证**：缺证据先查 state/source/runtime；权威证据仍缺且影响方案才问用户。缺 `--goal/--done-when` 的 run 先补齐再推进。
- **证据先于断言**：区分「源码存在 / 二进制存在 / 当前 runtime 可用」；说做完了要拿得出 artifact。
- **调优必须有测评**：baseline、指标、及格线、复测命令，缺一只算假设。
- **turn 完成 ≠ goal 完成**：派工完成以 `agent.dispatch.completed` 为准，别把每轮 Stop 当交付。

**P2 建议**：最小版本优先（先删/并/复用再新增）；先定标准再看数（不然看到任何数字都觉得「还行」）；快 runner 优先收口（host 明确选择，不按历史 telemetry 自动路由）。

**P3 可选**：结论附局限与失效条件。

**停止规则 · 三个刹车**：

| 刹车 | 怎么踩 |
|---|---|
| **刹车1：喊不出缺什么** | 你说「以后可能需要」→ 现在不需要，停 |
| **刹车2：数据不达标** | 你先定的标准是 80%，数据只有 60% → 停 |
| **刹车3：人说了算** | AI 都说好 → 还是要问你：你同意吗？ |

已收敛 → 停（不再追加审计轮）；证据齐 → 停（不追求更多确认）。

**常见错觉**：

| 你觉得 | 实际上 |
|---|---|
| 「先让 AI 审一下再说」 | 没想清楚缺什么就审 = 浪费。先过刹车1 |
| 「服务没挂，上线成功」 | 新功能可能根本没通电。走一遍用户操作才算 |
| 「三个 AI 都说好，可以合了」 | 还得问你。AI 不替你决定 |
| 「跑个数看看」 | 先定标准再看数 |
| 「写完了，结束」 | 不记下来下次从头踩坑 |

## ③ DOMAIN MAP · 六域卡

**Ⅰ 接管与恢复**——进项目第一件事 `lto runs`（`.lto/` 是本项目真源与本地记忆，am 只是下游投影）。`resume` 喂 AI（git head/task 状态，防 compact 丢上下文）；`recap` 给人（当初要做啥/为什么/做到哪/还剩啥）；`recap --mine` 跨 run 挖掘。冲突时信证据不信旧指令。不适用：新 run 立项（→Ⅱ）。

**Ⅱ 立项与契约**——先问「该不该做」（刹车1）。`start --goal --why --done-when`；/goal 型长交付加契约四件套 `--target/--constraint/--instrument/--entropy-check` 进 core delivery contract。进开发前四证据：architecture_alignment / first_principles / simplification_dedupe / value_measurement（详见 run-state-workflow.md）。不适用：已有 active run 的恢复（→Ⅰ）。

**Ⅲ 执行与派工**——`task add` 建单元，`runner` 跑命令落证据。**派外部 agent 首选 tmux 真 TUI**（`dispatch-goal` / `dispatch-and-wait`），可见可监督、机制级完成检测；headless delegate 只用于只读评审与兜底（agy `--print` 只出方案不执行=假成功）；写档的非 tmux runner fail-closed。派完挂 `events --wait --event-type agent.dispatch.completed`，别轮询。运行中 `tail -f .lto/<run-id>/live/<job-id>.log`。autopilot 档位 supervised→auto-exec（worktree 沙箱）→autonomous（机械证据闸门，不 spawn 决策 agent，反思归你）。不适用：确定性本地命令直接 runner 跑。

**Ⅳ 验证与收敛**——高风险 task 派异构对抗审计：`audit --auto-dispatch`（`--prefer-runner` 把慢 runner 挪出收口关键路径）、`audit --discover-risks` 对抗「自报完整性」（未审 risk 被 closeout 拦）。拿到 findings：不投票、亲自看源码核实每一条、大问题数须逐轮下降。ledger 收敛由脚本判（CONVERGED/CONVERGING/REBOUND/STALLED）——**closeout 只认降到 0，骗不过闸门**。手动派工的 reply 用 `collect-agent-run` 登记。不适用：确定性测试。

**Ⅴ 交付与发布**——部署必须按序：schema 先行可回滚 → 试运行 → 先只读 → 正式上线 → **走一遍真实用户路径**（不是 ping 服务活着）→ 清测试数据观察。收尾前四证据：documentation_alignment / historical_cleanup / clean_worktree / rebuild_package。`closeout --summary` 写 handoff+CHANGELOG；`release` 是 host-owned（.git 写操作 runner 沙箱做不了）。不适用：未过Ⅳ收敛闸门。

**Ⅵ 学习与维护**——每个决定**当时就记**（`scripts/write_decision.py` 写 ADR + 登记 artifact）；装 am 时 `memory publish` 走 am 原生 CLI（唯一 sink），没装 am 本地 `.lto/` 就是全部记忆。`lto prune` 手动清理 closed+超期 run 大件（默认 dry-run，`--yes` 才删，active run 永不动）。历史 telemetry 只作 advisory，不自动路由。

## ④ AUTHORITY & SOURCE · 权威层级

| 要核对什么 | 权威顺序 |
|---|---|
| 命令/参数 | 二进制 `--help` → `src/cli.rs` → `COMMANDS.md` |
| gate/状态语义 | Rust 实现 → 回归测试 → state/event 实物 |
| host workflow | active reference（状态标注见 INDEX.md） |
| 设计稿/backlog/历史 | 只作历史证据，**不证明现状** |

本表是按查询意图的速查；完整五级权威层级（runtime/source > COMMANDS 合同 > operating
policy(SKILL/AGENTS/ADR) > active reference > 历史/设计）以 `references/INDEX.md` §4 为准。
文档与 runtime 冲突 = 文档漂移，修文档，不做兼容解释。
LOOKUP → `COMMANDS.md`｜ROUTE → `references/INDEX.md`｜真源 → `.lto/<run-id>/state.json` + `artifacts.json`。

## 什么时候不要用 LTO

| 你要做什么 | 走哪里 |
|---|---|
| 修个报错 | diagnose |
| 让人审代码 | review |
| 写新 skill | skill-creator |
| 部署上线 | ship |

## Workload Profile

**Tier: heavy** — LTO 不替你写代码，但需要你（宿主 AI）在关键节点做判断：要不要继续、要不要修、要不要部署。
