# Spec: replan 触发器（外部观点 → 假设 → 待验证）

> 状态：**待对齐，未实施**。本文按原则 9（外部观点作为假设入库）写成，不构成核心变更授权。
> 来源：https://x.com/huangruiteng/status/2084986529080905875（LoopX 的 replan 机制，2026-08-05 抓取）
> 前置记忆：`loopx-vs-lto-reference`（LoopX 与 LTO 同类状态内核的既有对比）

---

## 0. 一句话

外部主张「长程 agent 的难点不是永远制定正确计划，而是检测计划失效并在不越权的前提下写出新的可执行前沿」。本文把它拆成四类触发时机，逐条对照 LTO 现状，只保留**实测存在缺口**的那些作为候选工作。

---

## 1. 来源主张（原文提炼，未验证）

四类 replan 触发时机：

1. 外部事实使原假设失效（PR review 改 public contract、CI 暴露兼容问题、branch/merge 出现 blocker）
2. 当前路线持续没有推进（单次 no-change 只 backoff；**同一 lane 连续无变化且无可执行工作**才构成 trigger）
3. 工作前沿耗尽但目标未满足（todo 全关 ≠ goal 完成；acceptance 仍有 gap / successor 缺失 / evidence 不足以支持 terminal）
4. 用户反馈或权限边界变化（改了目标/验收/decision scope 后，旧计划可能失去继续执行的授权）

四条约束：

- **证据先于 replan**：失败、等待、模型主观判断不能直接推翻计划
- **replan 不越权**：blocked 工作继续 blocked；只能推进不依赖该决定的安全工作，或提出具体问题
- **replan 必须写出状态变化**：只有「已重新规划」的 ACK 判为 `replan_noop`，不构成 material progress，不应 spend
- **有可执行工作时继续推进优先**：replan 不做周期性打断

---

## 2. 现状事实（2026-08-05 核对，实施前需复核）

| 主张 | LTO 现状 | file:line | 判定 |
|---|---|---|---|
| 触发 1 外部事实失效 | `resume` 有 HEAD-drift 检测 → 重新验证 task | `commands/resume.rs` | **部分覆盖**：只认 git HEAD 漂移，不认 PR review / CI 改变契约 |
| 触发 2 持续无推进 | `progress_digest` / `has_progressed` / `failure_fingerprint` / `blocked_task_got_success` 齐备 | `commands/ops.rs:3375,3457,3504,3536,3554` | **检测已有**，但结果只用于 autopilot 推进判定，不触发重规划 |
| 触发 3 前沿耗尽但目标未达 | `run_observability::assess` 已把 instrument 与 evidence 关联成三态，并接入自主闸门；closeout 另有重跑 | `run_observability.rs:90`、`autonomous_gate.rs:51`、`ops.rs:1611`、`closeout.rs:289` | **机制已有**，但 `assess` 只看最新一条证据，多 instrument 契约可被单条放行（已写测试实证），见 C1' |
| 触发 4 权限边界变化 | delivery contract 可持久化 targets/constraints/instruments/forced_entropy | `state.rs:41-54` | **部分覆盖**：契约可改，但改后不会让已有 task 重新过授权 |
| 约束 证据先于 replan | 原则 5「sensors are fallible」+ 原则 3「negative feedback first」已内化 | `CLAUDE.md` | **已覆盖**，无需新增 |
| 约束 replan 不越权 | 原则 1「host remains controller-in-chief」；`autonomous_gate` | `ops.rs:3599` | **已覆盖** |
| 约束 replan_noop 不算进展 | `has_progressed` 比对 task digest，agent 光说不做时 digest 不变 | `ops.rs:3457` | **已覆盖**，机制不同但效果等价 |
| 约束 不周期性打断 | autopilot 本就是 host 调用一次跑一轮，无后台定时器 | — | **已覆盖**（LTO 无 daemon，原则「不加 global daemon」） |

### 2.1 触发 3 的缺口确切位置

`DeliveryContract`（`state.rs:41-54`）有 `targets` / `constraints` / `instruments` / `forced_entropy`，**没有任何字段记录某个 target 是否已达成**。

`completeness_missing`（`state.rs:230`）查的是**契约本身填没填全**——有没有 target、有没有带命令的 instrument——不是 target 达成没达成。

实质验收只有一处：`closeout.rs:289` 在收口时重跑 `instruments`。它发生在**人主动收口**时，不发生在 autopilot 判定「无可执行 task」时。

于是存在这个组合：`has_actionable_autopilot_task` 返回 false → autopilot 认为没活可干 → 正常结束，**不问 target 达成了没有**。这正是来源主张的「todo 全关 ≠ goal 完成」。

---

## 3. 候选工作（未授权，需逐项裁决）

按缺口真实度排序。**不建议一次全做**——触发 1/4 的改动面大且收益未验证。

### C1. 前沿耗尽时检查验收（对应触发 3）

- **缺口**：autopilot 无活可干时静默结束，不检查 delivery contract

- **实跑方案已被实测否决**（2026-08-05）。本机 11 个 run 的真实 instruments 抽样显示，它们不是轻量探针而是**完整闸门串**：

  ```
  full-gates::cargo fmt --all --check && cargo check --locked --all-targets
    && cargo clippy --locked --all-targets -- -D warnings
    && cargo test --locked --all-targets
    && python3 scripts/check_docs_consistency.py && ...
  ```

  实测这条 25.77s；另有含 `cargo build --release` 与 `docker run` 的更重实例。在 autopilot 每次判定「无活可干」时实跑不可行——它会把一个廉价的结束路径变成半分钟的阻塞，且与 closeout 的重跑重复。

- **修正后的最小实现**：不实跑。`has_actionable_autopilot_task` 为 false 且契约非空时，读 `.lto/<run>/` 里已有的 instrument 执行证据，判断「每条 instrument 是否有过一次成功记录」：
  - 有 instrument 从未成功执行过 → 打印「契约未验收：<instrument>」并输出 `AUTOPILOT_STATUS: NEEDS_HOST`
  - 全部有成功记录 → 维持现状静默结束

  这样成本是一次状态读取，不是一次全闸门。判定依据是**已有证据**而非新执行，也符合原则 2「observe → log → derive signal」的顺序。

- **前置问题已答（2026-08-05）**：证据机制**已经存在且比预想完善**。`run_observability.rs:90` 的 `assess(state)` 通过 `instrument_ref`（或归一化命令 fallback）把 instrument 与 task evidence 关联，返回三态 `Missing` / `SignalDeclared` / `ObservableVerified`；`autonomous_gate.rs:51` 用它作为放行自主模式的前置条件之一；`ops.rs:1611` 据此把 autopilot 降级为 supervised。

  **所以 C1 的原始描述（「autopilot 无活可干时不检查契约」）不准确**——契约验收状态一直在被检查，且已接进闸门。

### C1'（取代 C1）. `assess` 只看最新一条证据，多 instrument 契约会被单条放行

- **实证缺陷**（新测试 `one_matching_instrument_verifies_the_whole_contract`，`run_observability.rs`）：`latest_command_evidence`(`:156`) 用 `max_by_key(ended_at)` **只取最新一条** evidence。契约声明 3 条 instrument、只跑过第 1 条时，`assess` 返回 `ObservableVerified`。

  本机 `20260715-c4-observability-subgate` 就是 8 条 instrument 的真实契约，命中这个形态。

- **影响**：`ObservableVerified` 是自主模式闸门的前置条件（`autonomous_gate.rs:51`）。「8 条验收只跑了 1 条」足以放行自主执行。这与来源主张的「acceptance 仍有 gap 不算 goal 完成」正是同一件事，但落点比原 C1 精确得多——不需要新增机制，是既有机制的判定范围问题。

- **未定**：这是缺陷还是有意的设计？现有 7 个测试全是单 instrument 场景，没有任何测试表达过多 instrument 的期望语义。可能的意图是「有任一可观测信号即可放行，逐条验收留给 closeout」——若是，则应写进 `assess` 的文档注释而非留白。**这一点必须先对齐，不能默认按缺陷修。**

- **若判定为缺陷，最小实现**：`assess` 改为遍历全部 evidence，要求每条 instrument 都有至少一次匹配记录；不满足则返回 `SignalDeclared` 并在 `missing` 里列出未验收的 instrument reference。不新增字段、不改协议、不实跑命令。

- **兼容性风险（已测，2026-08-05）**：收紧后，现有多 instrument 的 run 会从 `ObservableVerified` 掉回 `SignalDeclared`，自主模式降级为 supervised。这是**行为变更**。

  本机实测：14 个有契约的 run 中 3 个是多 instrument（21%）——`20260715-c3-finding-metadata`(2 条)、`20260715-c4-observability-subgate`(8 条)、`20260805-...-review-critical-high-p1-4-pha`(7 条，即本次 review 自身)。

  影响不算大但也非零。若实施，应同时提供一条明确的诊断输出（列出哪些 instrument 未验收），否则用户只会看到自主模式莫名降级。

- **不做什么**：不自动生成新 task（那是 host 的活，原则 1）；不实跑 instruments

- **完成判据**：构造「task 全 done + 某 instrument 无成功证据」的 fixture，autopilot 必须输出 NEEDS_HOST 而非静默成功；且该路径不得执行任何 instrument 命令（用一个会留痕的假 instrument 验证它确实没被跑）

### C2. 停滞检测接到 replan 提示（对应触发 2）

- **缺口**：`has_progressed` 已能判定停滞，但结果不产出「该重规划了」这个信号
- **最小实现**：停滞时在 `next` brief 里加一条 replan 建议，附停滞轮数与最后的 failure_fingerprint
- **注意**：来源主张强调「单次 no-change 只 backoff，连续无变化**且无可执行工作**才 trigger」——这个「且」不能省，否则正常等待会被误判为停滞（正好对应 LTO 原则 3 反对正反馈）
- **收益存疑**：LTO 已把停滞信息喂给 autopilot digest，host 看得到。加一条建议是否真有增量，需先看实际 brief 输出

### C3. 契约变更使 task 重新过授权（对应触发 4）

- **缺口**：改 contract 后已有 task 不重新审视
- **改动面大**：涉及 contract 变更事件、task 授权状态、resume 逻辑三处
- **建议**：**暂不做**。等 C1 落地后看实际是否有人踩到这个坑

### C4. 外部事实失效检测扩展（对应触发 1）

- **现状**：HEAD-drift 已覆盖最常见的一类
- **建议**：**不做**。PR review / CI 改变契约属于 GitHub 集成范畴，LTO 定位是控制骨架不是 CI 集成（原则「不做 auto-routing / 不做 PM 平台」）

---

## 4. 待验证问题（对齐时必须回答）

1. ~~C1 的 instruments 实跑耗时是多少？~~ **已答（2026-08-05）**：典型 `full-gates` 实测 25.77s，另有含 `cargo build --release` / `docker run` 的更重实例。实跑方案否决，改为读已有证据，见 C1。
1b. **新的前置问题**：instrument 的执行证据落在哪里？有没有「某 instrument 成功过」这个可查的事实？查不到则 C1 需要先补记录，改动面变大，要重新评估。
2. C2 的增量收益能否举证？拿一份真实 `next` brief 看停滞信息现在长什么样
3. 来源主张的 `replan_noop` 与 LTO 的 `has_progressed` 是否真等价？找一个 agent 只回 ACK 不改状态的真实案例验证 digest 确实不变
4. 四类触发器是否有 LTO 特有的第五类？（例如 runner 家族全部不健康导致 audit 无法进行）

---

## 5. 不做的事（明确划界）

- 不引入后台 daemon 做周期性 replan 检查（原则：不加 global daemon）
- 不让 LTO 自动生成新 task（原则 1：host 是规划者）
- 不把外部文章的措辞直接搬进代码命名（`lane` / `frontier` / `successor` 不是 LTO 的既有术语，硬搬会制造两套词汇）
- 不因为「LoopX 有所以 LTO 也要有」而实施——每项都要有本仓实测的缺口证据

---

## 6. 下一步

**唯一待裁决的问题**：`assess` 的多 instrument 语义（C1'）是缺陷还是有意设计？

- 判为**缺陷** → 收紧为「每条 instrument 都需有匹配证据」，影响本机 3 个 run（含本次 review 自身），需配套诊断输出
- 判为**有意** → 不改代码，把「有任一可观测信号即放行，逐条验收归 closeout」写进 `assess` 的文档注释，消除留白

无论哪种，实证测试 `one_matching_instrument_verifies_the_whole_contract` 都应保留——它把当前行为钉死，防止无声漂移。

C2/C3/C4 维持原判（收益存疑 / 暂不做 / 不做），本轮不再展开。

---

## 7. 本文的方法论副产品

写这份 spec 的过程本身推翻了它自己的两版方案：

1. 初版说「autopilot 无活可干时不检查契约」→ 读 `run_observability.rs` 发现检查机制一直存在且已接闸门，断言不成立
2. 二版说「跑一次 instruments」→ 实测真实 instrument 是 25.77s 的完整闸门串，方案不可行

留下的教训：**把外部主张翻译成本仓缺口时，先读本仓已有的同类机制，再谈新增**。两次返工都源于「按文章的概念找代码」而非「按代码的现状对概念」。这与 `templates/review-goal.md` 质量闸门第 1 条（子代理的定级结论必须核对其前提事实）同源，只是这次犯错的是 host 自己。
