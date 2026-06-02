---
name: long-task-orchestration
description: >-
  长程任务从 spec 到生产的上层编排纪律：用 premature 闸 + 真数据探针 + 用户拍板「三道闸」反过度设计，
  用异构多方审计收敛 + 亲核源码否决推进，用严格定序部署 + 端到端实测交付，每个拍板点决策即时落盘。
  Use when 任务同时满足：多轮迭代 + 从设计到上线 + 出现过度设计/长程失稳风险 + 需要显式判停闸门和状态恢复。
  Do not trigger on 单一 bugfix（走 diagnose/investigate）、纯一次性代码审查（走 review）、
  纯委派一轮给别的 runtime（走 agent-delegate）、写 skill 本身（走 skill-creator）、
  纯跑一条部署命令（走 ship/land-and-deploy）。
  注意：普通 "起 spec / 修 X 部署 / 开 MVP" 优先走对应专职 skill（blueprint / cs-feat / cc-new-feature），
  仅当这些任务已出现反复审计/判停/状态恢复问题时才进 LTO。
metadata:
  tier: agent-driven
  domain: infra
  optional_integrations: [agent-delegate, memory-flow]
  status: active
allowed-tools: [Bash, Read, Write, Edit, Task, AskUserQuestion]
---

# Long Task Orchestration

`agent-delegate` 的长程编排层：ad 解决「单轮委派怎么跑通」，LTO 解决「多轮推到上线怎么判停」。LTO **可选调用** `agent-delegate`（异构审计派工）和 `memory-flow`（经验落盘），但只管「为什么委派、收敛怎么判停、什么时候该拦、停了之后干嘛」——派工实现不重述。

> `optional_integrations` 声明 LTO 可选增强的后端；`agent-delegate` 和 `memory-flow` 都可降级（见 §5），不阻塞 LTO 核心纪律运行。

> **Runtime-agnostic**：本 skill 描述**能力动作**（「问用户拍板」「起一个独立 agent 审计」「后台并行不阻塞」），不绑某个 runtime 的工具名。宿主是 Claude Code 时这些落到 `AskUserQuestion`/`Task`/`Workflow`；宿主是 codex/pi/gemini 时落到各自的等价机制（交互式提问 / 子进程 agent / tmux 并行）。`allowed-tools` 是 Claude Code hint，其他 runtime 静默忽略。谁当宿主都能跑整套环；**异构审计**那步是「宿主把任务委派给**别的** runtime」——宿主是谁，就把另外两家当审计方（见 §3、§6）。

## 0. 防归因偏差（先读，否则会被抄成空仪式）

一场长程会话「该上的上、该拦的拦」**不是因为「干得猛 / 三方审计跑得全 / 用了 workflow」**。归因为「做得多 / 有仪式」会照抄出一个热闹但照样过度设计的空壳。

真正起作用的是三条**可证伪的闸门动作**，缺一就过度设计：

1. **premature 必须挂在一个具体缺失信号 X 上** —— 喊不出「缺的 X 是什么、X 此刻确实不存在」就不算 premature，只是偷懒。
2. **一条真数据探针 + 预设阈值能证伪整份纸面 spec** —— 先承诺阈值再看数，否则你会事后给任何数字找「继续做」的理由。
3. **判停权在用户手里，不在三方收敛** —— 三方一致只降低拍板成本，不替代「问用户拍板」这一步（宿主的交互提问机制）。

> **固化口令**：每个「想多做一层」的冲动，同时过三道闸——(a) 要消费的信号此刻存在吗？(b) 生产数据用预设阈值证伪你了吗？(c) 用户拍板要做吗？**仪式可换，三道闸不可省。**

**两条次级反归因**（防被仪式感稀释）：
- **核验否决 ≠ 服从审计**。价值不在「收了三份报告」，在你**亲核源码/数据后能否决任何一方**（含审计方、含你自己上一轮、含你自己写错的 SQL）。功劳是「亲核」。
- **运行态稳 ≠ 设计稳**。编译过 / health 200 不是收敛信号；机制**真通电**（运行路径上输出确实变了）才算。

## 1. 主循环骨架

每个功能都跑这一圈，跑通一次复用同一范式：

```
立项调研 → ① premature 三道闸预检(§2)；判太早→收缩切最小硬核子集，余拆后续 spec
         → ② (信号存疑) 真数据探针：先承诺阈值，一条聚合查询
spec 起草 → ③ 自己嗅到可能空转处，显式写进 spec 当「待审点」让三方严判
异构审计 → ④ triad.sh 派 codex+pi+agy(调 agent-delegate) → ⑤ 亲核每 blocker(采纳附行号/否决附证伪)
         → ⑥ 按收敛度分档不投票 → 修 → 再审，blocker 单调递减判停(→0 / 仅 minor)
闸门     → ⑦ 进开发/进部署前 AskUserQuestion 用户拍板
实现     → ⑧ 复用 ④-⑥ 整套循环审「代码」(不只 spec)
部署     → ⑨ schema 先于代码 → dry-run → 只读探针 → 部署+自动回滚 → ⑩ 端到端真路径实测(非 health 200) → 留观察窗
落盘     → ⑪ 里程碑 + backlog 两层条目，slug 外键，记反例与天花板
下一个   → 回 ①（方向由用户定）
```

**横切纪律**（贯穿整圈）：
- **后台派工不阻塞**：审计/调研后台跑（Claude Code 用 `Workflow`/triad；codex/pi 用子进程或 tmux window），主对话去做别的；完成主动通知，设长心跳兜底；**别轮询**。等待期挖下一步的事实地基（真代码/真分布），不靠记忆。
- **错峰不减深度**：多批并行分批起（防几十上百并发过载），每批都完整深做。
- **commit 即记**：中文 commit、README 写更新日志、引经验 slug；**无 AI 署名 / 无 Co-Authored-By**。
- **stale 免疫**：/compact 或心跳唤醒后，先用一手证据交叉确认真实状态再动手（→ `references/long-loop-state.md`）。

**状态产物（按需启用，非全量必填）**：
- `run-state.md`：**多轮长任务必需**。记录宿主、git SHA、阶段闸、决策 slug 和下一步。它是 resume/compact 后的真源。
- `preflight.md`：**仅在 delegation / deploy / child runner 启动前必需**。记录 runner healthcheck、宿主权限画像和降级声明。
- `audit-ledger.md`：**仅在异构审计 loop 启动后必需**。没有 ledger，就不能声称 blocker 单调递减或审计已收敛。
- 创建、检查、收尾优先用 `scripts/lto_run.py`（支持 `--profile minimal|audit|deploy` 按需选 artifact），不要手工复制模板。用法见 `references/run-state-workflow.md`。

## 2. 反过度设计「三道闸」（灵魂，最高 ROI）

| 闸 | 何时 | 判定/动作 | 判停信号 |
|---|---|---|---|
| **闸一 premature 挂 X** | 立项进实现前 | ① 要消费的信号此刻存在吗 ② 不在则上游落地了吗 ③ 都没→premature | 能写出缺的 X 且 X 当前确实不存在(可核验)；喊不出 X = 不成立 |
| **闸二 真数据探针** | spec 核心假设是「数据里有某分布」 | 一条最小聚合查询(先核真列名)+**预设量化阈值**；只看统计不看原文 | 真数据对照**预设**阈值得 pass/fail；「跑个数看看」不算闸门 |
| **闸三 用户拍板** | 每个 premature 判定/路线分裂/进开发或部署 | AI 先核验三方→分裂则压成「带推荐+逃生口的选项」问用户(Claude Code 用 AskUserQuestion，其他 runtime 用各自交互提问)→用户拍板才落盘 | 用户明确选定；三方一致也仍要过这关 |

**配套 · premature 后收缩，不放弃 + 零场景不抽象**：判太早的默认动作是切「现在能闭合的最小硬核子集」，其余拆独立后续 spec。合格收缩判据：复用现有信号/字段、不新建外部系统、不违既有铁律、一版能闭合。即使用户要「灵活性」也只留一条 trait 缝、**不预造第二实现**。

## 3. 异构审计收敛 + 核验否决（推进引擎）

异构审计派工是**可选能力插槽**：装了 `agent-delegate` 则用 triad.sh 派 codex+pi+agy；未装则降级为同模型 subagent 多视角自审（须声明对抗性弱）。本节只管收敛判停，不管派工机制——派工细节归 `[[agent-delegate]]`。

- **B1 触发**：新立项 spec / ≥3 维度变更 / 跨组件行为变更才启（单维小改自评即可）。派**异构三方**——**关键是「跟宿主不同模型家族」**：宿主 Claude 就派 codex(OpenAI)+pi(DeepSeek)+agy(Gemini)；宿主 codex 就派 claude+pi+agy；宿主 pi/agy 同理换掉自己。**必须异构**——同模型多实例≈自我重复，无交叉诊断价值。Claude Code 环境用 agent-delegate 的 `triad.sh`；其他 runtime 用各自的子进程/CLI 委派机制（agent-delegate 的 runner 表本身就覆盖 claude/codex/pi/agy 四家，互为委派方）。
- **B2 分档不投票**：逐 blocker 看收敛度——三方一致高置信=必修；两方+一方漏=二层核验；三方矛盾=亲核源码/数据自己裁；单方独占核不住=否决。**明写「不投票」**（多数决会淹没单方抓到的真 bug）。
- **B3 blocker 单调递减判停**：记每轮 HIGH+CRITICAL，要求单调非增。反弹（修 A 出 B）→ 暂停回退 debug、重审上一轮；连续 2 轮不降→质疑标准或需求本身。停于 blocker→0 或仅 minor。
- **B4 核验而非信仰**（纯方法论，零基建，最值钱）：任何 blocker/性能反驳/**你自己上一轮结论**，默认怀疑逐条核——亲自 Read 真源码(`文件:行号`)、必要时上生产复算(EXPLAIN/真延迟/真分布)、区分「看了真源码 vs 二手报告」。每条 claim 必有结论：采纳(附证据)或否决(附证伪)。**敢否决审计方、敢推翻自己、敢改自己写错的 SQL、敢抓自己脚本的误读。**
- **B5 机制真通电**：每个「A 触发 B」回路/降级链，手动走一遍数据从入口到出口的真实路径，确认中间没有更早的过滤/短路让它变死代码。指不出可观测效果 = 空转，打回。

详细轮次记账模板 → `references/audit-convergence.md`。

## 4. 部署严格 + 决策落盘（交付闸门）

- **严格定序**（缺一不发）：schema/DDL 先于代码且可反向 → dry-run 看真实变更 → 碰生产先只读探针(不打印密码、数据不外流) → health+自动 .bak 回滚 → **端到端黑盒真路径实测(非 health 200)，测完即删** → 部署后留观察窗。细节 → `references/deploy-sequencing.md`。
- **决策即时落盘**：每个拍板点拍完就写(非事后补)，重点记**反例与天花板**——为什么判 premature(缺的 X)、blocker 递减序列、真数据闸门结果、对标项目天花板。两层条目(里程碑+backlog) slug 外键互链，commit 引 slug。无 memory-flow 则降级 ADR/MEMORY.md，纪律不变。细节 → `references/decision-logging.md`。

## 5. 可选集成 × 降级矩阵（朋友第一眼看这张）

| 环节 | 完整版（装对应 skill） | 降级（未装） | 剩的纯方法论 | portability |
|---|---|---|---|---|
| 异构审计 | agent-delegate + codex/pi/agy | 同模型 subagent 自审(**对抗性缩水，须声明**) | 反迎合 prompt + 单调递减判停 + 不投票分档 | 需私有基建 |
| 真数据闸门 | 生产库 + ssh | 换任意数据源跑统计；无则造样本(**证伪力减**) | 「先承诺阈值再看数」 | 混合 |
| 亲核代码 / 机制通电 | 无 | 无 | **完整保留**（最强单点杠杆，零基建） | 通用 |
| 用户拍板 / stale 免疫 | 无 | 无 | **完整保留** | 通用 |
| 部署 | deploy.sh + 部署目标主机 | CI/CD staging + dry-run + down-migration | schema 先于代码 + 端到端实测 | 需私有基建 |
| 经验落盘 | memory-flow（检索/衰减/复利） | ADR / MEMORY.md | 拍板即记、记反例与天花板 | 可选增强 |

**净结论**：拿掉全部可选后端，**§3 B4 核验、闸一、闸三、收缩、stale 免疫**这几条零依赖直接可用，是本 skill 真正可移植的硬核。前置安装清单 → `references/sharing-guide.md`。

## 6. 与 agent-delegate 的关系：LTO 是编排层，ad 是派工插槽

`agent-delegate`（ad）→ 解决「单次委派怎么跑通」：runner 封装、tmux window、wait-for 回收、5 条反迎合 prompt。

本 skill（LTO）→ ad 的长程编排层，解决「多轮推到上线怎么判停」：
- **什么时候该委派**（B1 触发条件：新立项 / ≥3 维度变更 / 跨组件）
- **收敛怎么判停**（B2 分档不投票、B3 blocker 单调递减）
- **什么时候该拦**（三道闸：premature 挂 X / 真数据探针 / 用户拍板）
- **停了之后干嘛**（部署定序 / 落盘 / 下一个方向）

**派工是可选插槽**：装了 ad 则用 triad.sh 异构三方审计；未装则降级同模型 subagent 自审（§5）。本 skill **绝不写** ad 的实现细节——一律指向 `[[agent-delegate]]`。

模板产物边界：`run-state` / `preflight` / `audit-ledger` 只记录 LTO 层的状态、证据和判停，不复制 ad 的 runner 实现细节；具体命令和回收结果以 ad 输出为准。

## Resources

- `scripts/lto_run.py` — 创建 `.lto/<run-id>/` 状态三件套、校验 run-state/git drift、closeout 写 handoff。
- `templates/run-state.md` — 长任务恢复锚点，记录宿主、阶段、派工、证据、下一步。
- `templates/preflight.md` — 异构审计/后台派工/部署前的 runner 和宿主权限画像。
- `templates/audit-ledger.md` — 审计 blocker register 和单调下降判停记录。
- `references/run-state-workflow.md` — `start` / `check` / `closeout` 命令流程。

## 7. 谁当宿主都能跑（cross-runtime）

本 skill 的纪律不绑 runtime。落地时把「能力动作」映射到当前宿主：

本 skill 的纪律不绑 runtime，但**各家落地机制不同，不能合并成一栏**（三家自评纠正：早期把它们当统一的「子进程/tmux」是错的）：

| 能力 | Claude Code | codex | pi (DeepSeek) | agy (Gemini) |
|---|---|---|---|---|
| 问用户拍板 | `AskUserQuestion` | 交互式提问 | 终端原生交互 | 交互式提问 |
| 起独立 agent | `Task`/`Workflow` | 子进程 `codex exec`/tmux | **`Agent` 工具**（subagent_type，非子进程，据 pi 自述） | 子进程/tmux |
| 后台并行 | `Workflow`+心跳 | tmux 多 window | `Agent(run_in_background)` | tmux 多 window |
| 派工沙箱前提 | — | **需放开沙箱** `--dangerously-bypass-...` | 默认可派工 | 默认可派工（细粒度授权） |
| 启动方式 | — | `codex` 直接进 | 默认交互 | `agy -i "初始prompt"`（须带 prompt） |
| thinking 慢 | — | 中 | **审16KB ~170-200s，timeout≥240s** | 快 |

**铁律**：① 审计方必须跟宿主异构（同家族≈自评，派跟宿主不同的家族）；② body 一律写**能力描述**（「起一个独立 agent 审计」）而非工具名（「用 Task」），让各家自行落地。Claude Code 外的宿主用 agent-delegate runner 表派工，或各自的原生子进程/subagent 机制。

各家当宿主的专项坑（沙箱差异/codex preflight/pi thinking 预算/agy 启动/`--continue` stale）→ `references/cross-runtime-host-notes.md`（由 codex/pi/agy 各自审视本 skill 提炼，主 agent 核验采纳）。

## 不适用场景（边界）

- 单一 bugfix → 走 debug skill
- 纯一次性代码审查 → 走 review skill
- 纯委派一轮给别的 runtime → 走 `[[agent-delegate]]`（本 skill 是它的调用方，不是替代）
- 写 skill 本身 → 走 `[[skill-creator]]`
- 纯跑一条部署命令 → 直接 deploy.sh，不需要整套编排

## Workload Profile

**Tier: heavy** — 跨多轮审计编排 + 多功能长程状态管理 + 多方产物综合裁决。立项裁决、收敛判停、部署定序都是宿主 AI 推理重头戏，不可外包给单个子代理。
