---
name: long-task-orchestration
description: >-
  长程任务从 spec 到生产的上层编排纪律：用 premature 闸 + 真数据探针 + 用户拍板「三道闸」反过度设计，
  用异构多方审计收敛 + 亲核源码否决推进，用严格定序部署 + 端到端实测交付，每个拍板点决策即时落盘。
  Use when 用户要把一个功能从设计走到上线做多轮迭代；说「开个 MVP / 起 spec / 这功能要不要做 /
  是不是太早 / 过度设计了吗 / 长任务编排 / 防止 over-engineering」；或在「修 X 然后部署」
  「检查 X workflow 完成则继续」「框架稳了再进下一层」这类反复审计-修复-部署-实测-落盘 +
  后台派工不阻塞的推进上。Do not trigger on 单一 bugfix（走 debug）、纯一次性代码审查（走 review）、
  纯委派一轮给别的 runtime（走 agent-delegate，本 skill 只是它的调用方）、写 skill 本身（走 skill-creator）、
  纯跑一条部署命令（走各自的部署脚本/CI）。
metadata:
  tier: agent-driven
  domain: infra
  optional_backends: [agent-delegate, memory-flow]  # 可选增强，非硬依赖；不装仍可用核心纪律，见 §5 降级矩阵
  status: active
allowed-tools: [Bash, Read, Write, Edit, Task, AskUserQuestion]
---

# Long Task Orchestration

把一个功能**从 spec 编排到生产**的上层纪律。它**可选调用**异构审计后端（装了 `agent-delegate` 用它；否则用 agent 原生 subagent 多视角，须声明对抗性弱）和记忆后端（装了 `memory-flow` 用它；否则降级 ADR/MEMORY.md），但只管「为什么委派、收敛怎么判停、什么时候该拦、停了之后干嘛」——派工实现不重述。三个插槽的检测与降级逻辑见 §5。

> **Runtime-agnostic**：本 skill 描述**能力动作**（「问用户拍板」「起一个独立 agent 审计」「后台并行不阻塞」），不绑某个 runtime 的工具名。宿主是 Claude Code 时这些落到 `AskUserQuestion`/`Task`/`Workflow`；宿主是 codex/pi/gemini 时落到各自的等价机制（交互式提问 / 子进程 agent / tmux 并行）。`allowed-tools` 只是 Anthropic hint，其他 runtime 静默忽略。谁当宿主都能跑整套环；**异构审计**那步是「宿主把任务委派给**别的** runtime」——宿主是谁，就把另外两家当审计方（见 §3、§6）。

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
异构审计 → ④ 通过【异构审计后端】派工（装 agent-delegate 用 triad.sh 派 codex+pi+agy；否则用 agent 原生 subagent 多视角，须声明「未做异构交叉，对抗性弱」）→ ⑤ 亲核每 blocker(采纳附行号/否决附证伪)
         → ⑥ 按收敛度分档不投票 → 修 → 再审，blocker 单调递减判停(→0 / 仅 minor)
闸门     → ⑦ 进开发/进部署前 AskUserQuestion 用户拍板
实现     → ⑧ 复用 ④-⑥ 整套循环审「代码」(不只 spec)
部署     → ⑨ schema 先于代码 → dry-run → 只读探针 → 部署+自动回滚 → ⑩ 端到端真路径实测(非 health 200) → 留观察窗
落盘     → ⑪ 通过【记忆后端】落盘：装 memory-flow 用 experience_write；否则写 ADR/MEMORY.md。两层条目，slug/文件名外键互链，记反例与天花板
下一个   → 回 ①（方向由用户定）
```

**横切纪律**（贯穿整圈）：
- **后台派工不阻塞**：审计/调研后台跑（Claude Code 用 `Workflow`；装 agent-delegate 用 triad；codex/pi/agy 用子进程或 tmux window；各家机制见 §7），主对话去做别的；完成主动通知，设长心跳兜底；**别轮询**。等待期挖下一步的事实地基（真代码/真分布），不靠记忆。
- **错峰不减深度**：多批并行分批起（防几十上百并发过载），每批都完整深做。
- **commit 即记**：中文 commit、README 写更新日志、引经验 slug；**无 AI 署名 / 无 Co-Authored-By**。
- **stale 免疫**：/compact 或心跳唤醒后，先用一手证据交叉确认真实状态再动手（→ `references/long-loop-state.md`）。

## 2. 反过度设计「三道闸」（灵魂，最高 ROI）

| 闸 | 何时 | 判定/动作 | 判停信号 |
|---|---|---|---|
| **闸一 premature 挂 X** | 立项进实现前 | ① 要消费的信号此刻存在吗 ② 不在则上游落地了吗 ③ 都没→premature | 能写出缺的 X 且 X 当前确实不存在(可核验)；喊不出 X = 不成立 |
| **闸二 真数据探针** | spec 核心假设是「数据里有某分布」 | 一条最小聚合查询(先核真列名)+**预设量化阈值**；只看统计不看原文 | 真数据对照**预设**阈值得 pass/fail；「跑个数看看」不算闸门 |
| **闸三 用户拍板** | 每个 premature 判定/路线分裂/进开发或部署 | AI 先核验三方→分裂则压成「带推荐+逃生口的选项」问用户(Claude Code 用 AskUserQuestion，其他 runtime 用各自交互提问)→用户拍板才落盘 | 用户明确选定；三方一致也仍要过这关 |

**配套 · premature 后收缩，不放弃 + 零场景不抽象**：判太早的默认动作是切「现在能闭合的最小硬核子集」，其余拆独立后续 spec。合格收缩判据：复用现有信号/字段、不新建外部系统、不违既有铁律、一版能闭合。即使用户要「灵活性」也只留一条 trait 缝、**不预造第二实现**。

## 3. 异构审计收敛 + 核验否决（推进引擎）

派工通过【插槽1：异构审计后端】实现（本节只管收敛判停，不管派工机制）：
- **装了 agent-delegate**：用 `triad.sh`（runner/tmux/wait-for/5 条反迎合 prompt 的所有权在它）。
- **未装**：用 agent 原生 subagent 多视角（起 2-3 个独立子 agent 分头审，不共享上下文），**须在结论里声明「未做异构交叉，对抗性弱」**，不要高估结论可信度。

- **B1 触发**：新立项 spec / ≥3 维度变更 / 跨组件行为变更才启（单维小改自评即可）。派**异构三方**——**关键是「跟宿主不同模型家族」**：宿主 Claude 就派 codex(OpenAI)+pi(DeepSeek)+agy(Gemini)；宿主 codex 就派 claude+pi+agy；宿主 pi/agy 同理换掉自己。**必须异构**——同模型多实例≈自我重复，无交叉诊断价值。装 agent-delegate 的 Claude Code 环境用 `triad.sh`；其他 runtime 用各自的子进程/CLI 委派机制（agent-delegate 的 runner 表本身就覆盖 claude/codex/pi/agy 四家，互为委派方）。
- **B2 分档不投票**：逐 blocker 看收敛度——三方一致高置信=必修；两方+一方漏=二层核验；三方矛盾=亲核源码/数据自己裁；单方独占核不住=否决。**明写「不投票」**（多数决会淹没单方抓到的真 bug）。
- **B3 blocker 单调递减判停**：记每轮 HIGH+CRITICAL，要求单调非增。反弹（修 A 出 B）→ 暂停回退 debug、重审上一轮；连续 2 轮不降→质疑标准或需求本身。停于 blocker→0 或仅 minor。
- **B4 核验而非信仰**（纯方法论，零基建，最值钱）：任何 blocker/性能反驳/**你自己上一轮结论**，默认怀疑逐条核——亲自 Read 真源码(`文件:行号`)、必要时上生产复算(EXPLAIN/真延迟/真分布)、区分「看了真源码 vs 二手报告」。每条 claim 必有结论：采纳(附证据)或否决(附证伪)。**敢否决审计方、敢推翻自己、敢改自己写错的 SQL、敢抓自己脚本的误读。**
- **B5 机制真通电**：每个「A 触发 B」回路/降级链，手动走一遍数据从入口到出口的真实路径，确认中间没有更早的过滤/短路让它变死代码。指不出可观测效果 = 空转，打回。

详细轮次记账模板 → `references/audit-convergence.md`。

## 4. 部署严格 + 决策落盘（交付闸门）

- **严格定序**（缺一不发）：schema/DDL 先于代码且可反向 → dry-run 看真实变更 → 碰生产先只读探针(不打印密码、数据不外流) → health+自动 .bak 回滚 → **端到端黑盒真路径实测(非 health 200)，测完即删** → 部署后留观察窗。细节 → `references/deploy-sequencing.md`。
- **决策即时落盘**（【插槽2：记忆后端】）：每个拍板点拍完就写(非事后补)，重点记**反例与天花板**——为什么判 premature(缺的 X)、blocker 递减序列、真数据闸门结果、对标项目天花板。两层条目(里程碑+backlog) slug/文件名外键互链，commit 引 slug。
  - **装了 memory-flow**：`experience_write` 落盘，获检索命中率/衰减/复利。预留未来接其他兼容后端（`experience_write` 接口不变）。
  - **未装**：降级 `docs/decisions/` ADR + 项目根 `MEMORY.md` 索引；纪律不变，丢的只是检索复利。
  细节 → `references/decision-logging.md`。

## 5. 三插槽设计 × 降级矩阵（新用户第一眼看这张）

本 skill 依赖「接口」不依赖「具体实现」。同一 skill，用户插私有后端用私有后端，发布版插降级实现，不分叉。三个插槽：

| 插槽 | 能力 | 装了（私有/高配） | 未装（降级） | 须声明 | portability |
|---|---|---|---|---|---|
| **插槽1** 异构审计 | 跨模型家族交叉诊断 | `agent-delegate` + triad.sh + codex/pi/agy | agent 原生 subagent 多视角（不共享上下文） | **对抗性弱，须显式声明「未做异构交叉」** | 可降级 |
| **插槽2** 记忆落盘 | 决策检索/衰减/复利 | `memory-flow` experience_write（预留未来接其他兼容后端） | `docs/decisions/` ADR + 项目根 `MEMORY.md` | 丢检索复利，纪律不变 | 可降级 |
| **插槽3** 派工调度 | 后台并行不阻塞 | `triad.sh` / agent-delegate runner | agent 原生起 subagent（`Task`/`Workflow` 或各家等价机制） | — | 通用 |
| — | 真数据闸门 | 任意生产库/日志源 | 造样本（证伪力减，须声明） | 「先承诺阈值再看数」不可省 | 混合 |
| — | 亲核代码 / 机制通电 | 无 | 无 | **完整保留**（零基建，最强单点杠杆） | 通用 |
| — | 用户拍板 / stale 免疫 | 无 | 无 | **完整保留** | 通用 |
| — | 部署安全网 | 可回滚部署脚本 + 目标主机 | CI/CD staging + dry-run + down-migration | schema 先于代码 + 端到端实测 | 需复刻 |

**运行时插槽检测原则**：每个插槽在执行时检测后端是否可用，可用则用，不可用则自动降级并在结论里声明降级情况。不需要配置，不需要分叉 skill。

**净结论**：拿掉全部可选后端，**§3 B4 核验、闸一、闸三、收缩、stale 免疫**这几条零依赖直接可用，是本 skill 真正可移植的硬核。安装建议 → `references/sharing-guide.md`。

## 6. 与可选后端的边界（不重述实现）

`agent-delegate`（插槽1）= 「一轮委派怎么跑通」的**可选工具**；`memory-flow`（插槽2）= 「经验落盘检索」的**可选工具**；本 skill = 「为什么委派、收敛怎么判停、何时停、停了干嘛」的**上层纪律**。后两者是本 skill 可插拔的工具，本 skill 不依赖它们存在。本 skill **绝不写**：runner 封装、tmux window、wait-for 回收、CLI quirk、5 条反迎合 prompt 的实现——装了 agent-delegate 则指向 `[[agent-delegate]]`，未装则走插槽1降级路径（§5）。

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

**铁律**：① 审计方必须跟宿主异构（同家族≈自评，派跟宿主不同的家族）；② `allowed-tools` 的 `Task`/`AskUserQuestion` 在 Claude Code 原生匹配——**但据 pi 自评，pi 不静默忽略 tool name 而是按 body 能力描述自行映射（`Task`→`Agent`）**。所以 body 一律写**能力描述**（「起一个独立 agent 审计」）而非工具名（「用 Task」），让各家自行落地。Claude Code 外的宿主用 agent-delegate runner 表派工。

各家当宿主的专项坑（沙箱差异/codex preflight/pi thinking 预算/agy 启动/`--continue` stale）→ `references/cross-runtime-host-notes.md`（由 codex/pi/agy 各自审视本 skill 提炼，主 agent 核验采纳）。

## 不适用场景（边界）

- 单一 bugfix → 走 debug skill
- 纯一次性代码审查 → 走 review skill
- 纯委派一轮给别的 runtime → 走 `[[agent-delegate]]`（本 skill 是它的调用方，不是替代）
- 写 skill 本身 → 走 `[[skill-creator]]`
- 纯跑一条部署命令 → 直接走部署脚本，不需要整套编排

## Workload Profile

**Tier: heavy** — 跨多轮审计编排 + 多功能长程状态管理 + 多方产物综合裁决。立项裁决、收敛判停、部署定序都是宿主 AI 推理重头戏，不可外包给单个子代理。
