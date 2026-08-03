# LTO Backlog — deferred 缺口收口路线

> 真源：替代散落记忆。本表只收**有意延后的功能项**（非 bug——仓库测试全绿）。
> 排序尺 = **LLM 友好度**：做了能否让宿主 agent 更会跑 / 更会验 / 更不漂 / 更省。
> 不是工程难度，是对 control loop（见 `control-loop-harness.md`）的增益。

## 优先级总览

| # | 项 | LLM 友好度 | 优先级 | 状态 | 阻塞 / 解锁关系 |
|---|---|---|---|---|---|
| ① | `events.jsonl` / `telemetry.json` 被动事件流 | ★★★ 最高 | **P0** | ✅ 已实现 | **地基**：解锁 ②③ |
| ② | `DEFERRED_V0` llm_judge 质量评分 + 假阳率 | ★★ 高 | P1 | ✅ 已实现 | judge 异构判读 + frozen hash，单独成层不进 promote |
| ⑥ | **跨 run 数据挖掘 → 进化**（按 runner模型×status×时间 聚合，挖真实有效性喂回 host） | ★★★ 最高 | **P0-next** | 🟡 **部分**：聚合已实现 (2026-06-17)，stage 化 nextAction 未做 | `recap --mine` 出只读聚合 + 阈值 WARN；但 `state.next_action` 仍是裸字符串（`state.rs`），无 `nextAction{stage,command,why}` 结构与 actionable 计数 |
| ⑦ | **`AgentResult` 落 `model` 字段**（让 ⑥ 区分同 runner 不同 model） | ★★ 高 | **P1** | ✅ 已实现 | scheduler 单点回填 job.model；⑥ 挖掘出 model 分布；向后兼容 |
| ③ | `autopilot --autonomous` 机械闸门+机械执行（不 spawn 决策 agent） | ★ 中 | P2 | ✅ 已实现 | 证据闸门读⑥；run-scoped scheduler results 回填 `state.agent_runs`；codex 审 2BLOCKER+3HIGH 修；历史 `--decide` 未接到当前 CLI |
| ④ | `memory_sink` 记忆回写落地 | ★ 中 | P2 | ✅ 已实现 | am 0.7.0 AmCliSink 落地真跑；am 可选，无 am 优雅降级 |
| ⑤ | `AgentJobKind.TOURNAMENT` / `LOOP` 枚举 | ☆ 低 | **P3 不做** | YAGNI | 无真实触发场景，保持占位 |
| ⑧ | ACP 协议 fallback runner（任意 ACP agent 兜底派工） | ☆ 低 | **观察** | 远期 | acpx v0.9 alpha / ACP 协议 v0.13 仍 v1-v2 重构；协议稳了再接，不绑 acpx |
| ⑨ | Scheduler runner lifecycle events / O2 caller-side wiring | ★★ 高 | P1 | ✅ 已实现 | O2 采纳 Option A：调用方 emit runner started/finished/retry/healthcheck，`scheduler.rs` 保持无 run_id / 无事件 I/O |
| ⑩ | Host 合议 goal → tmux 短会话 loop → 异构审计 → 亲验闭环 playbook | ★★ 高 | P1 | ✅ 已实现 | T1/T2 Rust tmux 派工底座落地；playbook 进 `workflow-playbook.md`；closed check 默认拒绝无 evidence 的 done task |

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

- **是什么**：机械证据闸门（`_autonomous_gate`）+ 机械执行 safe 子步骤，**绝不 spawn 决策 agent**；反思/决策永远归宿主 LLM。当前 CLI 暴露 `--supervised` / `--auto-exec` / `--autonomous`；`src/decision.rs` 有 decision engine，但历史 `--decide` 未接到 CLI。
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
- **⚠️ 实现状态（2026-06-17 已实现）**：现已全面打通。通过 `lto recap --mine` 跨 run 扫描所有 `.lto/*/events.jsonl`，按 `(runner × task_type × 时间窗)` 聚合派工次数、失败率、耗时、retry、审计收敛轮次等，只读出 tuning brief (客观测量/主观非测量)。这标志着 L4 hill-climbing 成功落地。
- **⚠️ 未完成部分（2026-08-03 源码复核）**：上一条只覆盖「聚合与 brief」。下一条的 stage 化 **未实现**——`grep -rn "struct NextAction\|stage.*actionable" src/` 零命中，`state.rs` 的 `next_action` 仍是裸 `Value`。所以 ⑥ 的状态是 🟡 部分，别按「✅ 已实现」引用。
- **Rust 重做方向（业界对标，2026-06-17；stage 化仍待做）**：参考 `Nimrobo/superdense`（L4 产品化）的数据模型——session → enrichers 派生信号（run cost / errors / tool counts / first-intent / fingerprint / plan-mode / session-kind / subagent-summary）→ curation → reward snapshot；`next` 可学其 `nextAction{stage, command, why}` 机制（stage 化 + actionable 计数），但 **stage 要换成 LTO 自己领域**（挖 runner 有效性 → 识别反复翻车路径 → 出 tuning brief），不照搬 superdense 的 outcome stage。详见 am `2026-06-17-技术-从LTO架构承接loop启发的演进判断`。
- **铁律**：挖掘出的是**证据和派生信号**，不是命令——LTO 出 brief，host 决定调优，绝不自动 route/promote/晋升（沿用 control-loop 不变量）。judge 的主观分参与挖掘时仍标「主观非测量」。这正是 LTO L4 与 LangChain L4「自动改写 harness」的分歧线（LTO 守人在环，站 superdense「goal 永不自动改」一侧）。

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

## ⑩ Host 合议 goal → 派 coding agent 长跑 → 回收 → 亲验 闭环 playbook（✅ 已实现，Rust tmux 底座落地）

- **是什么**：把「host 合议形成 goal 文档 → 派一个 coding agent（如 codex）长时间自驱实现整个 goal → 完成后回收 → host 亲验」这个多轮闭环，沉淀成 host-agent **playbook**（不是 CLI、不是硬路由）。实测中它把长任务自动化推到很高程度。
- **定位**：先 playbook 后 CLI。已落在 `references/workflow-playbook.md` 的 `tmux-goal-loop`；仍不新增 `orchestrate` 命令，避免 harness 替 host agent 做语义判断。
- **已解除的阻塞依赖（2026-06-16 Rust 落地）**：
  - T1 把 tmux 派工能力吸收为 repo 内 Rust `runner: "tmux"`，直接调用 `Command::new("tmux")`，不依赖私有 `tmux-autopilot` skill。
  - T2 扩 `lto autopilot --worker-runner tmux`，用短会话 worker + completion contract 推进 `state.tasks`，不是让单 agent 啃完整大 goal。
  - T3 把闭环 playbook 写进开源 docs，并在 `lto check --to closed --strict` 加 default-FAIL evidence gate：done task 没有 evidence 时拒绝 closeout。
- **硬约束（playbook 必须保留）**：**host 亲验是硬停止点，不可自动跳过**。实测每轮 coding agent 报「全绿完成」host 亲验都揪出真 bug。能自动化的是派工+回收+记录，不能自动化的是亲验判真假；hook 回来即当完成 = 自动放过 bug。

## ⑪ runner 调度效率：headless 冷启重载 context（✅ 治标已实现 2026-06-17 / 治本评估中）

- **是什么（三层真因，逐层挖到底）**：codex 跑 audit 收口卡 1h33m+，逐层实测排查 pi 慢的真根因：
  1. **模型层（不是根因）**：`--thinking off` 只省 6s（44→38s）——慢不在 deepseek thinking。
  2. **context 层**：`runners/pi.sh` 用 `pi -p` **每次冷启重载约 4 万 token context**（AGENTS.md/CLAUDE.md/skills/extensions）。实测：默认 `pi -p "1+1"` = **44s / 40091 input tokens**；加 `--no-skills --no-context-files --no-extensions` = **1.4s / 395 tokens**（快 30 倍）。
  3. **调度层（最深根因）**：LTO 用最低效调度——每次 `pi -p` headless 一发一收冷启。pi **支持 `--mode rpc`（常驻 RPC）+ session 复用（`--continue`/`--resume`/`--session-id`）**，可「启一次、多次喂 prompt、context 只载一次」，但 LTO 一个都没用。这正是「headless 对第三方 CLI 是弱项」的具体落点。
- **治标（✅ 已实现 2026-06-17）**：LTO 在构造 audit/judge 的 `AgentJob` 时给 `job.env` 设 `LTO_LEAN_CONTEXT=1`（`audit_dispatch.rs::audit_job` + `llm_judge.rs` judge job；`scheduler.rs::runner_env` clone `job.env` 自动带下去，不改 scheduler）。各 runner.sh 自己翻译成本 CLI 的等价 flag：
  - **pi.sh**：`--no-skills --no-context-files --no-extensions` —— 实测 `1+1` **54.9s→3.2s（17×）**，reply 仍正确。
  - **claude.sh**：`--setting-sources ''`（丢 user/project/local settings=skills/memory/CLAUDE.md/hooks）—— 实测 **19097→2539 tokens（7.5×）**；read-only 由 `--permission-mode plan`+`--allowedTools` 兜，丢 settings 不破契约。
  - **codex.sh**：无安全 context-only flag（`--ignore-user-config` 连 auth 都丢→实测 401；`--ignore-rules` 只跳 AGENTS.md），优雅降级（忽略 lean，正常跑）。
  - **agy.sh**：本就拒绝 read-only job（无 read-only 档），不在 audit/judge 派工路径上，忽略 lean。
  - 单元测试 `audit_dispatch::jobs_use_scheduler_contract_and_readonly_intent_policy` 断言两 job 都带 `LTO_LEAN_CONTEXT=1`；开发派工（autopilot worker / ops.rs）**不设**（写码可能需 skill 上下文）。
  - 关键判断：lean context（省 token）与 read-only（权限）**正交**——审计 read-only 时两者都加。
- **治本（✅ 已实现 2026-06-17：pi session 复用，headless 就够）**：warm prompt cache 是省 token 真机制（同 session 重发 context 走 cacheRead ~10% 价）。实现：
  - LTO 给 audit/risk-discovery job 设稳定 per-(run, auditor) session-id `lto-<run_id>-audit-<auditor>`（`audit_dispatch::audit_session_id` 纯函数 + `submit_auto_dispatch` / `cli.rs::dispatch_risk_discovery` 注入 `job.env["LTO_SESSION_ID"]`）。
  - pi.sh 读 `LTO_SESSION_ID` → `pi -p --session-id <id>`。**向后兼容**：env 未设时不传 `--session-id`，行为同旧（bash -x 实测验证 A 无 session-id / B 有）。
  - **host 实测铁证**：新进程 resume 同 `--session-id` 命中 cache——`pi -p --session-id` cacheRead=1408，记住跨进程 fact；RPC 续接 cacheRead=2816 input 仅 223。**pi input 不膨胀**。
  - 同 auditor 跨 audit 轮（每轮独立 `lto audit` 进程）复用同 session → 第二轮起 context 走 cacheRead。risk discovery 与 auto-dispatch 用同一 session-id 函数，跨两阶段也 warm。
- **RPC 常驻 runner 已证伪（不做）**：一度设想 `pi --mode rpc` 常驻进程批内复用，**实测证伪**——① audit 异构派工，同次 submit 无多个 pi job（批内复用前提不成立）；② warm cache 真实命中是跨轮（跨 submit），而 **headless `pi -p --session-id` 跨进程就命中 cache**，根本不需要 RPC 常驻进程那套工程（进程池/并发接缝/协议处理）。详见 `references/specs/2026-06-17-goal-pi-rpc-resident-runner.md`（已标 ⛔ 证伪）。
- **跨 runner 调查结论（DeepWiki+官方+实测 2026-06-17）**：pi=session 复用最干净（input 不膨胀，audit 主力）；codex=resume 命中 cache 但 **input 累积膨胀**（33K→67K，长会话变贵）故**不加** session 复用；agy=本就拒 read-only 不审计，session 复用无意义。所以 session 复用只在 pi.sh 落地。
- **不破坏**：异构性不变（仍跨族 failover）；只是去掉每次冷启重载的浪费 + warm cache 跨轮。
- **撤回的错误方向**：原 ⑪ 写「给 audit 加 runner 优先级绕开 pi」是**治标且方向错**——pi 慢是调度方式（冷启重载）不是 pi 本身,绕开它没解决根因（agy/claude headless 同样冷启）。先修调度效率，不是排序。

## ⑫ 存量 security/并发关切（异构 auditor 挖出 → 逐条深核定性 → 真问题已修，2026-06-17）

> 来源：⑪ Phase 1 收口跑 `lto audit`（pi+agy 审全 codebase）挖出的**存量**问题。**处置方法（防 auditor 误报盲改）**：每条派独立子代理读真源 + 构造 PoC 判真假，host 亲验关键论据，只修真问题，误报记录不改。坐实「auditor 系统性偏差：把事实信号工具误当安全边界、不核引擎类型就报 ReDoS、把不相关环节凑成攻击链」。

- **events.rs lock 超时 fallback**（pi CRITICAL → 实为 LOW/MEDIUM → ✅ 已修）：深核判定半真——持锁路径完全串行、5s 超时正常永不触发、read dedup 兜底、LTO 实际不在进程内并发 emit，四层缓解被 pi 漏判。但仍是真隐患。**修**（fail-closed 哲学）：锁超时 `bail!` 拒绝无锁 best-effort 写（`events.rs` `acquire_events_lock_with_timeout`），事件经 `safe_emit` 优雅丢弃而非污染 JSONL（events 是投影，.lto state 才是真源）；顺手把 `writeln!` 改单次 `write_all` 保 O_APPEND 原子。测试 `lock_timeout_fails_closed_instead_of_lockless_write`（timeout 可注入，0ms 即测不等 5s）。
- **redact 双正则不一致**（pi HIGH + agy ReDoS → 覆盖盲点真 / 双标假 / ReDoS 误报 → ✅ 已修）：深核——`redact.rs` 弱套 ⊂ `llm_judge.rs` 强套，弱套漏 `/root`/Windows/`github_pat_`/`key=value`（这些落 events.jsonl/telemetry 是真泄漏）；但两套作用于**不相交输出域**，非「同数据双标」；ReDoS **误报**（Rust `regex` crate 线性引擎，实测 150k 病态输入 4ms）。**修**：合一为 `redact.rs` 单一真源（强套超集 + 修 PEM 单行 BEGIN 漏脱的回归），`llm_judge::redact_text` 转调 `crate::redact::redact_secrets_and_paths`（拆出**保原样不压空白不截断**的变体，judge 冻结证据需精确 shape；`redact_text` 仍压空白+截断给 events）。测试 `redacts_superset_secrets_and_paths_from_backlog_12`。
- **shell_command 可绕 classify_effect**（pi HIGH → 误报，不修）：深核坐实——test_cmd **根本不过 classify_effect gate**（该 gate 只在 worktree.rs/ops.rs 沙箱原语，host 已亲验）；test_cmd 是受信操作者配置（等价 CI script，能写 job-spec 者已有 RCE），`sh -c` 是必需语义。pi 把沙箱 gate 嫁接到 test_cmd 凑攻击链。加 test_cmd 的 gate 会误杀合法测试命令、零安全收益（CLAUDE.md「形似不等于同病」同款）。**不修**。
- **RunnerFamily::Unknown 隔离**（agy HIGH → 误报，不修）：host 亲验——`derive(PartialEq,Eq)` + `Unknown(String)` 带名，不同名 Unknown 是不同族（隔离成立）；AUDITOR_POOL 写死已知 runner，Unknown 实际不出现在审计选择。不构成真洞。
- 已知非新问题（不立项）：readonly intent 对 agy 升 workspace-write（pi HIGH）——`runner-readonly-contract.md` 早记录，agy 无 read-only 档，设计如此。

> 维护：项落地后更新本表「状态」列并在 `CHANGELOG.md` 记一笔；新 deferred 入此表，勿散落记忆。
