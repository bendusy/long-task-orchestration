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
  optional_integrations: [agent-delegate, memory-flow]
  status: active
allowed-tools: [Bash, Read, Write, Edit, Task, AskUserQuestion]
---

# LTO：长任务 harness

做一个小功能只要 5 分钟。做一个大功能可能要 50 轮对话——中间你会迷路：不知道做到哪了、不知道是不是做过头了、不知道什么时候该停。

LTO 就是帮你解决这三个问题的。它**不替你写代码，也不替你选完整 workflow**。host agent 是 planner；LTO 是 harness：提供 state、artifact、audit、runner、sandbox、resume/recap 和 human gate，让你有证据、有刹车、有恢复点地推进。

## 架构哲学

- **主 agent 是 planner**：路径选择属于宿主模型。LTO 只提供状态、证据、工具、沙箱、审计和停止闸门。
- **Preset 是 playbook，不是硬路由**：`review` / `debug` / `migration` / `claim-verify` / `research` 是调度先验，详见 `references/workflow-playbook.md`。不要先做替模型决策的固定执行入口。
- **Primitive 优先于产品命令**：先组合 `runner`、`audit`、`judge`、`next`、`autopilot`、`recap`；只有真实重复路径自然沉淀，才抽最薄 CLI。
- **外部观点先进插件，不进 core**：有趣文章先做 `source_note → experimental path plugin → eval → promote/reject`，详见 `references/plugin-boundary.md`。插件只编译到现有 primitive，不替 host 规划、不自带升权。
- **自动化是梯度**：brief → supervised → sandboxed auto-exec → human gate。每一级都必须保留证据、可恢复状态和人工刹车。

## 三个核心原则

**1. 想做更多？先回答「缺了什么」**
你说「以后可能要支持 X」——LTO 会问：X 现在真的需要吗？你答不上来就说明你在过度设计。停。

**2. 你的假设对不对？拿真实数据验证**
你说「大部分用户都怎样怎样」——LTO 会问：你预设的及格线是什么？先定标准再查数据，避免「跑个数字然后找理由说够好了」。

**3. 该不该往下走？问人，别自己决定**
三个 AI 都说「没问题」≠ 真的没问题。最终拍板的是你，不是 AI。

> 记住这三个问题：(1) 缺的东西现在真的存在吗？(2) 数据按你说的标准过了吗？(3) 人说了算吗？

## 六个阶段（一张图说清楚）

```
你的想法
  ↓
[阶段1] 该不该做？
  ├─ 该做 → 往下
  └─ 太早 → 砍掉多余部分，只做现在能做完的最小版本
  ↓
[阶段2] 写方案
  └─ 把可能出问题的地方标出来，让 AI 重点审
  ↓
[阶段3] 让三个不同的 AI 审你的方案
  ├─ 它们说的问题 → 一个个核实，修完再审
  └─ 修到没有大问题为止
  ↓
  ⚠️ 问你：可以开始写代码了吗？
  ↓
[阶段4] 写代码
  ├─ 不同模块让不同 AI 同时写（不冲突）
  └─ 写完用同样方法审代码
  ↓
[阶段5] 部署到线上
  ├─ 先改数据库 → 试运行看有没有问题
  ├─ 先只读不写 → 确认安全
  └─ 正式上线 → 你真的测过新功能能用吗？（不是说「服务没挂」）
  ↓
[阶段6] 记下来
  └─ 这次踩了什么坑、做了哪些决策、为什么这样选
  ↓
下一个想法 → 回到阶段1
```

## 什么时候停？三个刹车

| 刹车 | 怎么踩 |
|---|---|
| **刹车1：喊不出缺什么** | 你说「以后可能需要」→ LTO：现在不需要，停 |
| **刹车2：数据不达标** | 你先定的标准是 80%，数据只有 60% → 停 |
| **刹车3：人说了算** | AI 都说好 → LTO 还是要问你：你同意吗？ |

## 让多个 AI 帮你审（阶段3 怎么操作）

找个跟你不一样的 AI 来审——你用 DeepSeek 就让 GPT 和 Gemini 来审，反之亦然。同个模型审自己等于没审。

**一键编排（推荐）**：LTO 扫高风险 task、写审计简报、给派工指令、收口判收敛。下面命令默认从 `agent-skills` 仓库根目录运行；装过 `scripts/install.sh` 且 `lto` 在 `PATH` 后，可把 `$LTO` 换成 `lto`。LTO 只编排不自审（harness 不是被审/写码方），派工交给 agent-delegate，强制「审者 ≠ 你这个 host」：

```bash
LTO="python3 scripts/lto_run.py"

# 全自动（推荐，装了 agent-delegate）：扫高风险 task → 自动派异构三方 → 收口判收敛
$LTO audit --auto-dispatch

# 派 agent 主动找漏掉的风险点（对抗"自报完整性"，未审 risk 会被 closeout 闸门拦）
$LTO audit --discover-risks

# 半自动（没装 agent-delegate 或想手动控制）：
$LTO audit                                           # 写简报 + 打印派工指令
#    （想强制审某些 task：--task-id T1 T2）
$LTO audit --collect .lto/<run-id>/audit/replies     # 派完收口
```

> 审者输出结构化 JSON findings（severity 是字段，不靠正文扫关键词）；`--collect`
> 校验异构（审者家族 ≠ host）+ 抽 blocker 计数 + 判 ledger 收敛趋势。

**手动派工（没装 agent-delegate 时）**：
```bash
AD="scripts/delegate/runners"  # standalone repo; use your agent-delegate install path if running elsewhere
$AD/codex.sh  方案.md 回复-codex.md  300 &
$AD/agy.sh    方案.md 回复-agy.md    300 &
$AD/claude.sh 方案.md 回复-claude.md 300 &
wait  # 等它们都跑完
# 回复存一个目录、文件名带 runtime，再 $LTO audit --collect <dir>
```

**拿到结果后做什么**：
1. 不投票。三个都说「没问题」≠ 真的没问题
2. 亲自看源码验证它们说的每一条
3. 大问题数量必须越来越少，反弹了回头查（别硬修下一版）
4. 修到零大问题为止

每轮审完，把这一轮的大问题数量填进 ledger（`.lto/<run-id>/audit-ledger.md` 的 Round Summary 表，仅 `--with-audit` 或 `--profile audit|deploy` 时生成），然后让脚本替你判收敛：

```bash
python3 scripts/audit_ledger_check.py .lto/<run-id>/audit-ledger.md
```

它会打出一行 `verdict:`——数量降到 0 是 CONVERGED（收敛了，可以收尾），还在降但没到 0 是 CONVERGING（继续修），数量反弹是 REBOUND、卡在原地不动（`--strict`）是 STALLED，这俩它会喊停，让你别自我感觉良好地往下冲。

`lto_run.py check` 会顺手帮你跑这个：反弹/停滞默认只**提醒**（WARN），`--strict` 下才**拦住**（ERROR）——提醒不等于强制停，最终还是你拍板。但 `closeout` 不一样：只要 ledger 填了轮次且没降到 0（CONVERGING/REBOUND/STALLED），它会**直接拒绝收尾**（除非你 `--force` 明确越过）。也就是说——脚本算的 Round Summary 收敛才是收尾的硬条件，你手填的 Closure Gate 字段只是辅助记录，骗不过收敛闸门。

## 部署上线（阶段5 怎么操作）

**必须按顺序**：
1. 数据库先改（要能改回去）
2. 试运行看会改什么
3. 先只查不写，确认安全
4. 正式上线
5. **真的测过新功能吗？**——不是 ping 一下看服务活着，是走一遍用户操作流程
6. 测完删掉测试数据，观察一段时间

## 怎么记（阶段6）

每个决定**当时就记**，别等做完再补。重点记：
- 为什么觉得太早（缺的到底是什么）
- 改了几轮、问题怎么变少的
- 查数据时定的标准是什么、实际是什么
- 别人做到什么程度了（天花板在哪）

用什么记：先用 `write_decision.py` 写 `docs/decisions/` ADR 并登记 artifact；有 memory-flow 时再把值得复利的经验写入库。

每次 closeout 会自动生成人类友好的 CHANGELOG.md，从 state.json 的 task/evidence 中提取。

### ANIMEM artifact memory（可选）

LTO 的真源仍是本地 `.lto/<run-id>/state.json` + `artifacts.json`。
ANIMEM / memory-flow 只是可选的 artifact-memory sink，用来让不同 runtime
跨项目发现“哪个 run 活着、产物在哪里、谁审过、下一步是什么”。

没装 ANIMEM 也能完整使用 LTO：

```bash
$LTO memory export --run-id <run-id> --dry-run  # 纯本地 redacted JSON
$LTO memory resume --project <key>              # 无 sink 时降级到本地 .lto
$LTO memory publish --run-id <run-id>           # 只有显式 publish 才要求 sink 配置
```

`memory export/publish` 不投影 `original_user_request` 原文、raw transcript、
`agent_runs`、secrets、完整源码或私有文档正文；长文本只写 redacted summary/hash。
`memory resume` 只读，不覆盖 `.lto/current` 或 `state.json`。

## 多轮任务怎么不迷路

用一个文件记住当前状态：
```bash
# 开始（--why/--done-when 记下为什么做、做完的标准，recap 会用到）
LTO="python3 scripts/lto_run.py"

$LTO start --goal "做用户登录" \
  --why "降低登录失败率" --done-when "失败率<5%，三端覆盖"

# 续接（上次 compact 之后，或新 session 恢复）
$LTO resume        # 给接手的 AI 拉上下文
$LTO memory resume # 可选：先查 ANIMEM/memory-flow，失败则降级本地
$LTO recap         # 给人看的回顾（隔了几天回来，人会忘）

# 检查状态 / 问下一步
$LTO check
$LTO check --to implementation   # 只读检查进入写码前的证据
$LTO check --to closed --strict  # 预查收尾前硬证据
$LTO next                        # 出事实简报+无歧义命令建议（零 LLM，判断由你做）

# 自驱推进（受约束 harness，不接管 planner）
$LTO autopilot --supervised   # 出 brief 回吐你判断
#   --auto-exec：safe/reversible 子步骤在 worktree 沙箱自动跑（dangerous 停下等确认）
#   --decide：escalate 时派三方异构 agent 讨论收敛（opt-in 烧 token，决策权仍归你）

# 完成
$LTO closeout --summary "做了什么，验证了什么"      # 默认写 CHANGELOG.md
$LTO closeout --summary "行政收尾" --no-changelog  # 已提交后避免新 tracked dirt
```

> **resume vs recap**：resume 喂 AI（git head / task 状态，防 compact 后丢上下文）；
> recap 给人（人话回顾：当初要做啥/为什么/跑多久/做到哪/还剩啥/现在轮到你）。两者正交。
>
> **check --to**：`check --to implementation|closed` 只报告 phase-entry evidence，
> 不自动切阶段、不替人批准。`--strict` 才把缺失的 required evidence 变成非零退出；
> `--json` 输出单个 JSON 对象给其他 host 接手读取。
>
> **autopilot 档位**：`--supervised`（出 brief，默认）、`--auto-exec`（worktree 沙箱跑 safe 子步骤）、`--decide`（escalate 时 opt-in 派三方异构 agent 收敛，决策权仍归你）已实现；`--autonomous`（零人工全自动循环）是下一期，未实现——按 spec「先攒 supervised 真实 escalate 数据再决定值不值」。git push 永远停下等人确认，不随档位放开。

## 什么情况下不要用 LTO

| 你要做什么 | 走哪里 |
|---|---|
| 修个报错 | diagnose |
| 让人审代码 | review |
| 让别人跑个任务 | agent-delegate |
| 写新 skill | skill-creator |
| 部署上线 | ship |

## 常见错觉（看起来对其实错）

| 你觉得 | 实际上 |
|---|---|
| 「先让 AI 审一下再说」 | 没想清楚缺什么就审 = 浪费。先过刹车1 |
| 「服务没挂，上线成功」 | 新功能可能根本没通电。走一遍用户操作才算 |
| 「三个 AI 都说好，可以合了」 | 还得问你。AI 不替你决定 |
| 「跑个数看看」 | 先定标准再看数，不然看到任何数字都觉得「还行」 |
| 「写完了，结束」 | 不记下来下次从头踩坑 |

## Resources

**入口与文档**
- `scripts/lto_run.py` — 18 命令薄入口（分发到 `lto/commands/`）
- `scripts/write_decision.py` — ADR-first 决策落盘 helper（写 `docs/decisions/` + state + artifact manifest）
- `scripts/install.sh` — 安装 skills，并生成 sentinel-managed 全局 `lto` wrapper
- `references/onboarding.md` — **给 agent 读一份就懂怎么装载 LTO**（跨 runtime）
- `references/workflow-playbook.md` — `review/debug/migration/claim-verify/research` 调度先验
- `references/run-state-workflow.md` — 18 命令详细用法手册
- `references/execution-loop.md` — runner/judge/parallel/pipeline + agent 执行层
- `references/hooks.md` — pre-commit/pre-deploy/pre-closeout 边界 hook（opt-in）
- `references/sharing-guide.md` — 怎么装、怎么给朋友用、项目级注入
- `references/cross-runtime-host-notes.md` — 不同 AI 工具当宿主的具体用法

**状态产物**
- `.lto/<run-id>/state.json` — 机器真源（含 tasks/risk_points/why/done_when/agent_runs）
- `.lto/<run-id>/artifacts.json` — 跨 host 产物索引（replies/briefs/evidence/judge/decision records/handoff，repo-relative 路径）
- `templates/run-state.md` — 人类可读状态模板
- `templates/audit-ledger.md` — 审计台账（仅 `--with-audit` 时生成）

**harness primitive 底层模块**（不直接走 CLI，是 next/autopilot/audit 的地基）
- `scripts/lto/agent_job.py` — AgentJob/AgentResult 数据合同（agent 世界，非 shell）
- `scripts/lto/scheduler.py` — 并发调度 + 退出码三元判定(OK/FAILED/TIMEOUT/RATE_LIMITED) + 指数退避
- `scripts/lto/agent_exec.py` — spawn 原语（拉隔离 agent，落 state.agent_runs）
- `scripts/lto/worktree_exec.py` — autopilot 自动执行的 worktree 沙箱（17 攻击向量拦截 + env 隔离）
- `scripts/lto/progress.py` — 推进检测 + stall 闸门（防伪推进博弈，单向棘轮）
- `scripts/lto/pi_tool.py` — Pi 工具集成（让模型直接调用 LTO）
- `scripts/lto/decision.py` — 双轨收敛引擎（direction 投票 / review union 合并），被 `autopilot --decide` 调用 spawn 三方
- `scripts/lto/decision_brief.py` — --decide 收敛 brief 构造（给宿主读，不替宿主拍板）

## Workload Profile

**Tier: heavy** — LTO 不替你写代码，但需要你（宿主 AI）在关键节点做判断：要不要继续、要不要修、要不要部署。
