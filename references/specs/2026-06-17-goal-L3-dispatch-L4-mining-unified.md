# Goal: L3 派工+完成通知 ⊕ L4 跨 run 复利 —— 一条 events.jsonl 数据流贯穿（统一 goal）

> 致 codex:沿用全部既有约束(LTO 自管 / 每 Phase 收口跑 `lto audit --auto-dispatch --discover-risks` 跨族异构审 / dogfooding 铁律:lto 自己调不通=lto bug 优先修 / 红线全绿 `cargo fmt --check`+`clippy -D warnings`+`test --locked --all-targets` / commit 你写、release/tag/push 归 host 别碰)。
> **这份是 L3+L4 统一 goal,一次做完。做完停。**
> **复用既有 spec 但修其割裂**:`2026-06-17-goal-tmux-dispatch-goal-primitive.md`(L3 派工+完成通知子系统设计,21 边界全在)是本 goal 的 L3 部分基础,但**它有一处割裂必须修**(见下)。

---

## 为什么是一份 goal(L3 与 L4 的真实关系，host 亲验)

L3(dispatch-goal 派工 + 完成通知)和 L4(⑥ 跨 run 复利)**不是两个独立任务,是一条数据流的两端**:

```
L3: dispatch 派 codex/pi/agy 长跑 → 每次 turn 完成 → 写 events.jsonl(agent.turn.completed)
                                                          ↓ 同一个文件
L4: 扫所有 run 的 events.jsonl → 按 (runner 模型 × 任务类型 × 时间) 跨 run 聚合
                              → 哪个 runner 在哪类任务真有效 / 哪条路径反复翻车
                              → 喂 host 出 tuning brief（host 决定，LTO 不自动改 harness）
```

**关键**:L3 派出去的 codex/pi/agy 的 turn 完成数据,**正是 L4 要挖的「不同 agent 模型 × 时间」的原料**。backlog ⑥ 明写「必须 ① events.jsonl 先攒够真实日志才有数据可挖」——L3 就是那个「攒数据」的传感器。**分开做会割裂数据流,合起做才是一条贯穿 L1→L4 的事件流。**

## ⚠️ 必读:旧 spec 的割裂(本 goal 必须修)

`2026-06-17-goal-tmux-dispatch-goal-primitive.md` 的完成通知子系统,**把 turn 完成写进独立的 `.lto/<run>/dispatch/*.turns.jsonl`**（spec line 99/114/126）。这是**割裂**——L4 要挖的是 `events.jsonl`,不是 turns.jsonl。

**修正(本 goal 的核心架构裁决)**:完成通知**写进 `events.jsonl`**,加一个 `agent.turn.completed` 事件类型,**不另写 turns.jsonl**。host 已亲验:
- `events.jsonl` 已是统一事件总线,`KNOWN_EVENT_TYPES`(events.rs:14)有 21 类型(runner.*/audit.*/gate.*/...)。
- 写入 API:`crate::events::safe_emit(repo, run_id, EventRecord{event_type, actor_kind, summary, fields, ...})`。
- **先决步骤**:`emit`(events.rs:68)对 event_type 做白名单硬校验——**必须先把 `agent.turn.completed` 加进 `KNOWN_EVENT_TYPES`**,否则 emit 直接拒写(和 O2 同款坑)。
- 这样 L3 完成通知就成了 events.jsonl 的事件,L4 天然能扫到。**完成通知子系统的其他设计(21 边界 / codex Stop hook / 自动装 / 仲裁 / 降级)全部保留,只把"写 turns.jsonl"换成"safe_emit agent.turn.completed"。**

---

## 核心架构裁决(host 已盘清，别另择)

**裁决 1:完成通知 = 写 events.jsonl 的 `agent.turn.completed` 事件(不写 turns.jsonl)。**
- codex Stop hook / pane-died / sentinel 收到完成信号 → `safe_emit(... "agent.turn.completed" ... fields{runner, session_id, cwd, summary, rc?})`。
- run 路由靠 payload 的 `cwd`/`session_id`(host 实测 codex Stop hook payload 有这俩)→ 映射到对应 run_id 写它的 events.jsonl。
- 先扩 `KNOWN_EVENT_TYPES` 加 `agent.turn.completed`(先决,否则拒写)。

**裁决 2:L4 跨 run 挖掘 = 把 `telemetry.rs` 的单 run `by_runner` 提升到跨 run(不从零造)。**
- host 亲验:`telemetry.rs:129` 已有**单 run 内** `by_runner` 聚合 + `failure_rate`(line 170/182)。L4 = 扫**所有** `.lto/*/events.jsonl`,按 `(runner × task_type × 时间窗)` 跨 run 聚合,复用单 run 的聚合逻辑。
- `runs` 命令已能列所有 run(`cmd_runs`),L4 复用它枚举 run 目录。
- 出口:新命令 `lto recap --mine`(backlog ⑥ 口径)或 `lto mine`——出**跨 run tuning brief**(哪个 runner 在哪类任务有效 / 哪条路径反复翻车 / 哪个 profile 真改善)。

**裁决 3:L4 铁律 —— 出 brief 喂 host，绝不自动改 harness(守人在环,LTO 与 LangChain L4 的分歧线)。**
- L4 挖掘出的是**证据和派生信号**,不是命令。`recap --mine` 出 brief,host 读了决定调优,**LTO 绝不自动 route/promote/改 runner 优先级/改配置**。
- judge 的主观分参与挖掘时标「主观非测量」。
- 这是 LTO 站「superdense goal.md 永不自动改」一侧,不走 LangChain L4「自动改写 harness」。**写死在实现里:recap --mine 是只读分析命令,不写任何配置/不改任何 runner 行为。**

---

## Phase L3-1:dispatch-goal 核心派发(复用旧 spec Phase 1)

按 `2026-06-17-goal-tmux-dispatch-goal-primitive.md` 的 Phase 1 实现:`lto dispatch-goal --runner <codex|pi|agy> --goal <file>`,三家入口编排(codex `/goal` literal / pi 非交互 `--print` wrapper / agy 非交互 `--print` wrapper)+ TUI 坑封装(探针确认 + literal 路径)。复用 `tmux_runner.rs`。注:live `agy --help` 已核实 `-i` 是 `--prompt-interactive`,不能作为可靠的进程退出完成通知点;pi/agy 的 `--print` wrapper 才能由 shell 在退出时 emit `agent.turn.completed`。
- **判据**:三家 dogfood 实跑通(派测试 goal,确认开跑)。详见旧 spec Phase 1 判据。

## Phase L3-2:完成通知子系统 → 写 events.jsonl(修割裂)

按旧 spec 的完成通知子系统(codex Stop hook + pane-died + 21 边界 + 自动装 hook + 仲裁 + 降级),**但完成信号 emit `agent.turn.completed` 到 events.jsonl,不写 turns.jsonl**(裁决 1)。
- 先扩 `KNOWN_EVENT_TYPES` 加 `agent.turn.completed`(先决)。
- codex Stop hook 脚本(纯 bash / schema 容错 / always exit 0)→ 调 `lto` 内部命令或直接 emit:把 cwd/session_id/summary 写成 `agent.turn.completed` 事件到对应 run 的 events.jsonl。
- 21 边界、自动装(幂等+备份+可回滚)、仲裁(用户已有 Stop hook 不覆盖)、降级(没装 hook → pane-died/sentinel 兜底)全保留。
- **判据**:派 codex 测试 goal,turn 完成 → 对应 run 的 `events.jsonl` 真追加 `agent.turn.completed`(host 已验 Stop hook 真触发 + payload 有 cwd/session_id);`grep agent.turn.completed .lto/<run>/events.jsonl` 看到。

## Phase L4-1:跨 run 聚合挖掘(提升 telemetry by_runner)

- 扫所有 `.lto/*/events.jsonl`(复用 `cmd_runs` 枚举),按 `(runner × task_type × 时间窗)` 跨 run 聚合:派工次数、failure_rate、平均耗时、retry 率、audit 收敛轮次、agent.turn.completed 计数。复用 `telemetry.rs:129 by_runner` 的单 run 逻辑,提升到跨 run。
- distinct-run 闸门:同一 run 的重复事件不重复计(按 run_id 去重)。
- **判据**:有 ≥2 个含 runner 事件的 run 时,聚合出「runner × task 的 failure_rate / 次数」跨 run 表;单元测试用 fixture（2+ run 的 events.jsonl）断言聚合正确。

## Phase L4-2:`recap --mine` 出 tuning brief（只读,守人在环）

- 新增 `lto recap --mine`(或 `lto mine`):消费 L4-1 的聚合,出**跨 run tuning brief**——哪个 runner 在哪类任务有效 / 哪条路径反复翻车 / 哪个 profile 真改善。
- **只读**:不写任何配置、不改 runner 优先级、不自动 promote(裁决 3)。输出是给 host 读的 brief（markdown/json）。
- judge 主观分参与时标「主观非测量」。
- **判据**:`lto recap --mine` 出 brief,内容是事实+派生信号(带 why,如「codex 在 audit 类 failure_rate 40% over 5 runs」);grep 确认命令存在 + 实跑出 brief;**审计确认它不写任何配置文件/不改 runner**(只读性是硬判据)。

## Phase L4-3:修 backlog ⑥ + 文档

- backlog ⑥ 从「⬜ Rust 未实现」改为「✅ 已实现(L4-1/L4-2)」,正文更新 Rust 落点(替换退役的 Python 锚点 interventions.py 等)。
- README/SKILL 的「L4 Hill-climbing」状态从「路线」更新为「已实现(recap --mine)」。
- workflow-playbook 加「用 recap --mine 看跨 run 有效性」。

---

## 执行顺序 + 收口

1. **L3-1 → L3-2** 先做(派发 + 完成通知接 events),L3-2 收口 commit。**L3 是 L4 的数据源,必须先通。**
2. **L4-1 → L4-2 → L4-3**(跨 run 聚合 + recap --mine + 文档),L4 收口 commit。
3. 每批收口:`cargo fmt/clippy -D warnings/test --locked` 全绿 → `lto audit --auto-dispatch --discover-risks` 异构审本批 diff(HIGH/CRITICAL 消解)→ `lto check` → commit。
4. 建议 L3、L4 各独立 commit(防长 thread);L3-2 完成通知子系统较重可再拆子批(hook 模板/安装仲裁/降级)。
5. backlog ⑩(tmux-goal-loop)/⑪ 关联更新。

## 提醒(安全阀)
- **完成通知写 events.jsonl 不写 turns.jsonl**(裁决 1,修旧 spec 割裂)——这是 L3→L4 数据流贯穿的关键,错了 L4 挖不到数据。
- **先扩 KNOWN_EVENT_TYPES**(先决,否则 agent.turn.completed 被拒写)。
- **L4 只读守人在环**(裁决 3):recap --mine 绝不自动改 harness/配置/runner 优先级,只出 brief 喂 host。这是硬红线(LTO 与 LangChain L4 的分歧线),审计要确认只读性。
- **复用不重写**:tmux_runner.rs(L3)/ telemetry.rs by_runner(L4)/ cmd_runs(枚举)/ safe_emit(events)/ codex Stop hook 自动装(旧 spec 设计)——别从零造。
- 三家入口按实测表并结合 live CLI 纠偏(codex /goal、pi `--print` wrapper、agy `--print` wrapper);TUI 探针+literal 硬要求。
- 自动装 codex hook 改用户全局 config 必须幂等+备份+可回滚+仲裁(不覆盖用户已有 hook)。
- dogfood:dispatch-goal 自己派测试 goal 三家实跑通 + 完成事件真进 events.jsonl + recap --mine 真出 brief 才算完。
- host 亲验是硬停止点;commit 你写,release/tag/push 归 host。
