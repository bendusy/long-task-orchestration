# LTO Backlog — deferred 缺口收口路线

> 真源：替代散落记忆。本表只收**有意延后的功能项**（非 bug——仓库测试全绿）。
> 排序尺 = **LLM 友好度**：做了能否让宿主 agent 更会跑 / 更会验 / 更不漂 / 更省。
> 不是工程难度，是对 control loop（见 `control-loop-harness.md`）的增益。

## 优先级总览

| # | 项 | LLM 友好度 | 优先级 | 状态 | 阻塞 / 解锁关系 |
|---|---|---|---|---|---|
| ① | `events.jsonl` / `telemetry.json` 被动事件流 | ★★★ 最高 | **P0** | 待实现 | **地基**：解锁 ②③ |
| ② | `DEFERRED_V0` llm_judge 质量评分 + 假阳率 | ★★ 高 | P1 | 待实现 | 需 ① 的可复现事件证据；须配 `frozen_evidence_hash` |
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
- **现状锚点**：`references/protocol-and-language-strategy.md` 标 "planned, not implemented"；`control-loop-harness.md` STATUS 标 Phase 1 未实现。
- **下一步**：独立 LTO run 实现，子代理写码 + 三方异构审 + 红线验收。

## ② DEFERRED_V0 llm_judge（P1，被 ① 喂证据）

- **是什么**：eval-run 用 LLM 判 blocker 质量 / 假阳率（`llm_judge_blocker_quality`、`llm_judge_false_positive_rate`）。
- **为何**：动机3（插件测有效性）的质量闭环靠它。
- **风险**：本身引入 LLM 主观判断——**必须配 `frozen_evidence_hash_redact`**（同属 DEFERRED_V0），否则评分不可复现，反污染 eval 结论。
- **前置**：① 落地后有可复现事件证据再做。
- **锚点**：`scripts/lto/plugin_eval_run.py` `DEFERRED_V0`。

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

---

> 维护：项落地后更新本表「状态」列并在 `CHANGELOG.md` 记一笔；新 deferred 入此表，勿散落记忆。
