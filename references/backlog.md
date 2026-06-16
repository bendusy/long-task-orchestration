# LTO Backlog — deferred 缺口收口路线

> 真源：替代散落记忆。本表只收**有意延后的功能项**（非 bug——仓库测试全绿）。
> 排序尺 = **LLM 友好度**：做了能否让宿主 agent 更会跑 / 更会验 / 更不漂 / 更省。
> 不是工程难度，是对 control loop（见 `control-loop-harness.md`）的增益。

## 优先级总览

| # | 项 | LLM 友好度 | 优先级 | 状态 | 阻塞 / 解锁关系 |
|---|---|---|---|---|---|
| ① | `events.jsonl` / `telemetry.json` 被动事件流 | ★★★ 最高 | **P0** | ✅ 已实现 | **地基**：解锁 ②③ |
| ② | `DEFERRED_V0` llm_judge 质量评分 + 假阳率 | ★★ 高 | P1 | ✅ 已实现 | judge 异构判读 + frozen hash，单独成层不进 promote |
| ⑥ | **跨 run 数据挖掘 → 进化**（按 runner模型×status×时间 聚合，挖真实有效性喂回 host） | ★★★ 最高 | **P0-next** | ✅ 已实现 | **① 的下游闭环**：双源(agent_runs+events)合一 brief；distinct-run 闸门；codex 审 6 条修 |
| ⑦ | **`AgentResult` 落 `model` 字段**（让 ⑥ 区分同 runner 不同 model） | ★★ 高 | **P1** | ✅ 已实现 | scheduler 单点回填 job.model；⑥ 挖掘出 model 分布；向后兼容 |
| ③ | `autopilot --autonomous` 机械闸门+机械执行（不 spawn 决策 agent） | ★ 中 | P2 | ✅ 已实现 | 证据闸门读⑥；codex 审 2BLOCKER+3HIGH 修；与 --decide 互斥 |
| ④ | `memory_sink` 记忆回写落地 | ★ 中 | P2 | ✅ 已实现 | am 0.7.0 AmCliSink 落地真跑；am 可选，无 am 优雅降级 |
| ⑤ | `AgentJobKind.TOURNAMENT` / `LOOP` 枚举 | ☆ 低 | **P3 不做** | YAGNI | 无真实触发场景，保持占位 |
| ⑧ | ACP 协议 fallback runner（任意 ACP agent 兜底派工） | ☆ 低 | **观察** | 远期 | acpx v0.9 alpha / ACP 协议 v0.13 仍 v1-v2 重构；协议稳了再接，不绑 acpx |
| ⑨ | Scheduler runner lifecycle events / O2 caller-side wiring | ★★ 高 | P1 | ✅ 已实现 | O2 采纳 Option A：调用方 emit runner started/finished/retry/healthcheck，`scheduler.rs` 保持无 run_id / 无事件 I/O |

## 依赖链

```text
① events.jsonl  ──解锁──▶  ③ autonomous（要真实 escalate 数据）
                └─喂证据──▶  ② llm_judge（评分要可复现的事件证据）
④ memory_sink  ── ✅ 已实现（AmCliSink / am 0.7.0，am 可选）
⑤ tournament/loop  ── 不做（YAGNI）
```

① 是地基——②③ 都等它。路线不是 5 项并列，而是**先落 ① 传感器层，自然解锁 ②③，④ 等外部，⑤ 砍掉**。

---

## ① events.jsonl / telemetry.json（P0）

- **是什么**：append-only 运行事件流（runner 启停、gate 通过/拒绝、decision、escalate、token）+ `telemetry.json` 派生 run 信号。
- **为何最高**：`control-loop-harness.md` 把它定为 Phase 1「传感器层」。没它，`next`/`recap`/未来 eval 都缺一手证据，宿主只能靠状态快照猜，漂移检测与未来 tuning 失去地基。
- **LLM 友好点**：零 LLM、零决策、append-only——纯传感器，不引入主观判断。宿主读结构化事件比读散落 stdout 省 token。
- **落地约束**：append-only 不可改写；事件 schema 稳定可被 `next`/eval 消费；不替宿主决策（只记录）。
- **现状**：✅ Rust 已接管。`src/events.rs`（`KNOWN_EVENT_TYPES` 写入硬校验 + append-only + `.events.lock` + 递归 redact）+ `src/event_emit.rs`（O2 调用方接线 helper）+ `src/telemetry.rs`（派生 runner failure rate / audit rounds 等信号，无 recommendations）覆盖 Phase 1 + O2 sensor layer；旧 Python 实现已随 fallback 在 v0.5.0 退役。free-text cap 240（spec §5.0）。
- **解锁**：②③ 现可启动（有了可复现事件证据 + 真实 escalate 数据来源）。

## ② DEFERRED_V0 llm_judge（P1，被 ① 喂证据）

- **是什么**：eval-run 用 LLM 判 blocker 质量 / 假阳率（`llm_judge_blocker_quality`、`llm_judge_false_positive_rate`）。
- **为何**：动机3（插件测有效性）的质量闭环靠它。
- **风险**：本身引入 LLM 主观判断——**必须配 `frozen_evidence_hash_redact`**（同属 DEFERRED_V0），否则评分不可复现，反污染 eval 结论。
- **现状**：✅ Rust 已接管。`src/llm_judge.rs`（异构 runner 判读 + `freeze_evidence` sha256 冻结 redacted 证据）+ Rust `plugin eval-run` 写 `comparison["judge"]` 单独成层标 `kind:"subjective_judgment"`。三铁律：judge 异构（同族 skip）/ 可复现（输入 redact+规范化+hash）/ 不夺权（promote 仍 human-gated）。`DEFERRED_V0` 缩到只剩 `automatic_promotion`。
- **裁决档**：用户拍板「判读+冻结，judge 不进 promote」（最稳，judge 只作额外参考）。

## ③ autopilot --autonomous（P2，被 ① 解锁）

- **是什么**：机械证据闸门（`_autonomous_gate`）+ 机械执行 safe 子步骤，**绝不 spawn 决策 agent**（与 `--decide` 互斥，运行时强制清掉）；反思/决策永远归宿主 LLM（当前 `--supervised` / `--auto-exec` / `--decide` 已实现）。
- **为何延后**：spec 明说「先攒 supervised 真实 escalate 数据再决定值不值」——而数据正来自 ①。在 ① 落地、攒够真实 escalate 样本前做它=赌。
- **锚点**：`src/commands/ops.rs` 的 autonomous gate / autopilot 分支、`SKILL.md` autopilot 档位说明。

## ④ memory_sink（P2）

- **是什么**：Rust `memory publish/export/resume` 记忆回写落地。
- **现状**：✅ 已实现（am 0.7.0，AmCliSink 落地真跑）。`AmCliSink.publish()` 调 `am ingest -f - --json` 吃整个信封，`AmCliSink.resume()` 调 `am search`，两个方法均有完整实现；基类 `MemorySink` 的两个 `NotImplementedError` 是抽象方法占位符（非缺口），子类 `AmCliSink` / `LegacyMemoryFlowSink` 均已覆盖。am 是可选项——没装 am 时 `_require_binary` 优雅降级，`.lto/` 仍是真源。

## ⑤ TOURNAMENT / LOOP 枚举（P3，不做）

- **是什么**：`src/agent_job.rs` 中 agent job 类型/模式的扩展占位。
- **判定**：YAGNI，无真实触发场景，做了不增任何 control-loop 增益。保持占位，有真实场景再说。

## ⑥ 跨 run 数据挖掘 → 指导 LTO 进化（P0-next，① 的下游闭环）

- **是什么**（用户洞察 2026-06-09）：harness LTO 本身应根据**不同 agent 模型 × 随时间推移的运行日志**，挖掘出最真实可信、有效的数据，反过来指导 LTO 自己进化。不是"再加个功能"，是把 ①②③ 这些零件串成「数据 → 进化」的闭环。
- **数据流**：
  ```
  不同 agent 模型跑同一 LTO → events.jsonl 随时间累积真实日志
    → 按 (runner 模型 × 任务类型 × 时间) 聚合挖掘
    → 哪个模型在哪类任务真有效 / 哪个 profile 真改善 / 哪条路径反复翻车
    → 喂回 host agent 出 tuning brief（host 决定，LTO 不自动 route/promote）
  ```
- **为何 P0-next**：这是 `protocol-and-language-strategy.md`「越用越聪明」的真正落点，比 ②③ 更接近终极目标。但**必须 ① 先攒够真实日志**才有数据可挖——所以紧随 ①。
- **关键复用**：`interventions.py` 已有 `aggregate_across_runs` / `recurring_friction` / `render_cross_run_advisory`——**跨 run 挖掘摩擦的成熟模式已存在**。⑥ 是把同样模式套到 events.jsonl + ② 的 judge 结果上，新增维度：**按 runner 模型分组**（哪个模型在哪类任务有效），不只是按 category。
- **缺口锚点**：`events.py` 当前**只有单 run 读取，零跨 run 聚合**（已核实）。
- **铁律**：挖掘出的是**证据和派生信号**，不是命令——LTO 出 brief，host 决定调优，绝不自动 route/promote/晋升（沿用 control-loop 不变量）。judge 的主观分参与挖掘时仍标「主观非测量」。

## ⑧ ACP 协议 fallback runner（远期观察，协议稳了再接）

- **是什么**：让 LTO 能把任意 ACP（Agent Client Protocol）coding agent 当 runner——作为现有 4 家硬编码 runner（codex/pi/claude/agy）之外的**兜底通道**，不抢主路径。
- **定位**：**fallback，不是主路径**。现有 delegate 四家 runner 已实测可用（headless 子进程 + token sidecar + sandbox），不缺派工能力。ACP 只在「需要派一个非这 4 家的 ACP agent」时兜底。
- **为何现在不做（一手数据，2026-06-09 网查）**：
  - **acpx CLI**（ACP 的 headless 客户端）：v0.9.0（2026-05-22），README 仍标 **alpha**「CLI/runtime interfaces likely to change」；其 README **不碰 orchestration 集成**。
  - **ACP 协议本身**：v0.13.6（2026-06-05），**仍 0.x breaking-change 阶段**，正在 **v1/v2 架构分裂重构**，release 频繁 `(unstable)`/`(unstable-v2)`，remote agent support 还 work-in-progress。
  - 结论：协议自身还在重构、breaking change 满天飞——现在接 = 对着移动靶。把要稳定开源的 LTO 绑 alpha 协议是引入已知不稳定依赖。
- **触发条件（满足才动手）**：ACP 协议出 **1.0 / 摘掉 unstable 标 + remote agent 做完**，或 acpx 摘 alpha。在此之前**只观察不立项**。
- **接的时候接什么**：接 **ACP 协议**（标准、可复用），不绑 acpx 这一个 alpha CLI。

## ⑨ Scheduler runner lifecycle events / O2 tracing（P1）

- **是什么**：在 scheduler-backed 调用方发出 runner lifecycle 事件，包括
  runner.finished、retry summary、timeout、healthcheck unhealthy skip。`runner.started`
  仍需 scheduler callback/event channel 才能不污染 `Scheduler::submit` 签名。
- **裁决**：✅ 选项 A。Scheduler 保持通用 executor，不接 `.lto` run_id；调用方在已有
  run_id 上下文里用 `AgentResult` 发事件。已验 `AgentResult` 字段足够覆盖
  finished/retry/timeout/unhealthy skip。
- **验收线**：scheduler-backed audit/parallel/pipeline/eval-run 路径有结构化 event；
  plugin eval-run 和 audit 自动继承事件；`telemetry.json` 能派生 runner failure rate
  和 audit round/finding 计数；隐私脚本仍为 0 unclassified hits。

## ⑩ Host 合议 goal → 派 coding agent 长跑 → 回收 → 亲验 闭环 playbook（远期，待派工底座）

- **是什么**：把「host 合议形成 goal 文档 → 派一个 coding agent（如 codex）长时间自驱实现整个 goal → 完成后回收 → host 亲验」这个多轮闭环，沉淀成 host-agent **playbook**（不是 CLI、不是硬路由）。实测中它把长任务自动化推到很高程度。
- **定位**：先 playbook 后 CLI。闭环目前才跑 3-4 轮且流程仍在演进（异构审计、dogfooding、撞车警示、授权口径都是边跑边加），合同未沉淀，现在落 CLI 违背 harness-first。
- **阻塞依赖（为何现在不做）**：
  - 当前可用形态「tmux 开交互窗口发 /goal 让 agent 多轮自驱」依赖 **host 侧 tmux-autopilot skill（不在本 repo）**。写进开源 playbook = 让 stranger 依赖私有工具，违反交付契约。
  - 本 repo 的 `scripts/delegate/`（headless `codex exec`）是**一发一收**，跑不了「派一个大 goal 让 agent 自驱几小时」——任务粒度不匹配。
- **触发条件（满足才动手）**：派工底座就位之一——① tmux-autopilot rust 化或被 LTO 吸收为 repo 内能力；或 ② LTO 自建「常驻交互式 runner」（能驱动多轮长任务，比现有 headless runner 重）。
- **硬约束（写 playbook 时必须保留）**：**host 亲验是硬停止点，不可自动跳过**。实测每轮 coding agent 报「全绿完成」host 亲验都揪出真 bug。能自动化的是派工+回收+记录，不能自动化的是亲验判真假；hook 回来即当完成 = 自动放过 bug。

## ⑪ runner 调度效率：headless 冷启重载 context（P1，dogfood 实测真根因）

- **是什么（三层真因，逐层挖到底）**：codex 跑 audit 收口卡 1h33m+，逐层实测排查 pi 慢的真根因：
  1. **模型层（不是根因）**：`--thinking off` 只省 6s（44→38s）——慢不在 deepseek thinking。
  2. **context 层**：`runners/pi.sh` 用 `pi -p` **每次冷启重载约 4 万 token context**（AGENTS.md/CLAUDE.md/skills/extensions）。实测：默认 `pi -p "1+1"` = **44s / 40091 input tokens**；加 `--no-skills --no-context-files --no-extensions` = **1.4s / 395 tokens**（快 30 倍）。
  3. **调度层（最深根因）**：LTO 用最低效调度——每次 `pi -p` headless 一发一收冷启。pi **支持 `--mode rpc`（常驻 RPC）+ session 复用（`--continue`/`--resume`/`--session-id`）**，可「启一次、多次喂 prompt、context 只载一次」，但 LTO 一个都没用。这正是「headless 对第三方 CLI 是弱项」的具体落点。
- **治标（立刻可做，P1）**：所有 runner.sh 当审计用时加 `--no-skills --no-context-files --no-extensions`（审计是一次性纯评审，不需加载 skill 生态）——pi 从 44s 降到秒级，根本不用「绕开 pi」。read-only 审计仍保留 `--tools read,grep,find,ls`（实测 2.4s / 1388 tokens）。
- **治本（与 tmux runner goal 接上）**：audit 的 runner 调度从 headless 冷启 → 常驻载体:① 走 tmux runner（交互常驻，context 复用）② 或 pi `--mode rpc` / session 复用。tmux runner goal（`2026-06-16-goal-tmux-orchestration-runner.md`）做好后,audit 该能走 tmux 调度而非 headless pi.sh。
- **不破坏**：异构性不变（仍跨族 failover）；只是去掉每次冷启重载的浪费。
- **撤回的错误方向**：原 ⑪ 写「给 audit 加 runner 优先级绕开 pi」是**治标且方向错**——pi 慢是调度方式（冷启重载）不是 pi 本身,绕开它没解决根因（agy/claude headless 同样冷启）。先修调度效率，不是排序。

> 维护：项落地后更新本表「状态」列并在 `CHANGELOG.md` 记一笔；新 deferred 入此表，勿散落记忆。
