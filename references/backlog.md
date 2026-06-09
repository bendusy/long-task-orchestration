# LTO Backlog — deferred 缺口收口路线

> 真源：替代散落记忆。本表只收**有意延后的功能项**（非 bug——仓库测试全绿）。
> 排序尺 = **LLM 友好度**：做了能否让宿主 agent 更会跑 / 更会验 / 更不漂 / 更省。
> 不是工程难度，是对 control loop（见 `control-loop-harness.md`）的增益。

## 优先级总览

| # | 项 | LLM 友好度 | 优先级 | 状态 | 阻塞 / 解锁关系 |
|---|---|---|---|---|---|
| ① | `events.jsonl` / `telemetry.json` 被动事件流 | ★★★ 最高 | **P0** | ✅ 已实现 | **地基**：解锁 ②③ |
| ② | `DEFERRED_V0` llm_judge 质量评分 + 假阳率 | ★★ 高 | P1 | ✅ 已实现 | judge 异构判读 + frozen hash，单独成层不进 promote |
| ⑥ | **跨 run 数据挖掘 → 进化**（按 模型×任务×时间 聚合 events，挖真实有效性喂回 host） | ★★★ 最高 | **P0-next** | 待实现 | **① 的下游闭环**：① 攒单 run 日志，⑥ 跨 run 挖掘；复用 interventions 的 `aggregate_across_runs` 模式 |
| ③ | `autopilot --autonomous` 全自动回路 | ★ 中 | P2 | 待实现 | 被 ① 解锁（spec：先攒 supervised 真实 escalate 数据） |
| ④ | `memory_sink` 记忆回写落地 | ★ 中 | P2 | 外部阻塞 | 等 am（animem）下游接口稳定 |
| ⑤ | `AgentJobKind.TOURNAMENT` / `LOOP` 枚举 | ☆ 低 | **P3 不做** | YAGNI | 无真实触发场景，保持占位 |

## 依赖链

```text
① events.jsonl  ──解锁──▶  ③ autonomous（要真实 escalate 数据）
                └─喂证据──▶  ② llm_judge（评分要可复现的事件证据）
④ memory_sink ◀──阻塞── am 开发中（外部）
⑤ tournament/loop  ── 不做（YAGNI）
```

① 是地基——②③ 都等它。路线不是 5 项并列，而是**先落 ① 传感器层，自然解锁 ②③，④ 等外部，⑤ 砍掉**。

---

## ① events.jsonl / telemetry.json（P0）

- **是什么**：append-only 运行事件流（runner 启停、gate 通过/拒绝、decision、escalate、token）+ `telemetry.json` 派生 run 信号。
- **为何最高**：`control-loop-harness.md` 把它定为 Phase 1「传感器层」。没它，`next`/`recap`/未来 eval 都缺一手证据，宿主只能靠状态快照猜，漂移检测与未来 tuning 失去地基。
- **LLM 友好点**：零 LLM、零决策、append-only——纯传感器，不引入主观判断。宿主读结构化事件比读散落 stdout 省 token。
- **落地约束**：append-only 不可改写；事件 schema 稳定可被 `next`/eval 消费；不替宿主决策（只记录）。
- **现状**：✅ 已实现（2026-06-09）。`scripts/lto/events.py`（8 类型 append-only + fcntl 锁 + 递归 redact）+ `scripts/lto/telemetry.py`（派生信号，无 recommendations）+ 5 处 emit 接入（safe_emit fail-safe）。codex+pi 两路异构审 union 修 7 条（3 BLOCKER：并发锁/lazy import/嵌套漏键），五验收 + 并发测试全绿。free-text cap 240（spec §5.0）。
- **解锁**：②③ 现可启动（有了可复现事件证据 + 真实 escalate 数据来源）。

## ② DEFERRED_V0 llm_judge（P1，被 ① 喂证据）

- **是什么**：eval-run 用 LLM 判 blocker 质量 / 假阳率（`llm_judge_blocker_quality`、`llm_judge_false_positive_rate`）。
- **为何**：动机3（插件测有效性）的质量闭环靠它。
- **风险**：本身引入 LLM 主观判断——**必须配 `frozen_evidence_hash_redact`**（同属 DEFERRED_V0），否则评分不可复现，反污染 eval 结论。
- **现状**：✅ 已实现（2026-06-09）。`scripts/lto/llm_judge.py`（异构 runner 判读 + `freeze_evidence` sha256 冻结 redacted 证据）+ `plugin_eval_run` 写 `comparison["judge"]` 单独成层标 `kind:"subjective_judgment"`。三铁律：judge 异构（复用 `_same_family`，同族 skip）/ 可复现（输入 redact+规范化+hash）/ 不夺权（promote 一行没碰，仍 human-gated）。`DEFERRED_V0` 缩到只剩 `automatic_promotion`。
- **裁决档**：用户拍板「判读+冻结，judge 不进 promote」（最稳，judge 只作额外参考）。

## ③ autopilot --autonomous（P2，被 ① 解锁）

- **是什么**：escalate 时自动 spawn 决策 agent + 自动执行回路（当前 `--supervised` / `--auto-exec` / `--decide` 已实现）。
- **为何延后**：spec 明说「先攒 supervised 真实 escalate 数据再决定值不值」——而数据正来自 ①。在 ① 落地、攒够真实 escalate 样本前做它=赌。
- **锚点**：`scripts/lto/commands/autopilot.py:47`、`SKILL.md` autopilot 档位说明。

## ④ memory_sink（P2，外部阻塞）

- **是什么**：`scripts/lto/memory_sink.py` 两个 `NotImplementedError` —— 记忆回写落地。
- **为何延后**：下游 am（animem）正在开发，接口未定，现在实现=对着移动靶。
- **解除条件**：am 回写接口稳定后接入，留 stub。

## ⑤ TOURNAMENT / LOOP 枚举（P3，不做）

- **是什么**：`scripts/lto/agent_job.py:28-29` 两个 `AgentJobKind` 枚举占位。
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

---

> 维护：项落地后更新本表「状态」列并在 `CHANGELOG.md` 记一笔；新 deferred 入此表，勿散落记忆。
