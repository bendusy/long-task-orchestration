# Goal: C4 可观性子检查——autonomous_gate 增 current_run_observability（不单立 gate）

> 致 codex：沿用约束（LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写 release 归 host）。
> **这份只做 C4，做完就停，别做 C1/C2/C3**。注意：C1 也动 `src/telemetry.rs`——两份 goal
> 不可同时派工；若 C1 已合入，先 rebase 实证现状再动手。

## 为什么做（目标 + 第一性）

「先观测后控制」（LTO 原则 2 / 工程控制论：能控性先问可观性）目前只对**历史**成立：
`autonomous_gate`（`src/commands/ops.rs:3192-3291`）验的全是 repo 级 runner 历史
（≥5 run、≥10 agent_runs、跨 run 事件挖掘、拒纯主观/超时/高失败），**完全不看当前
run 的 goal/done_when/instruments**——`cmd_autopilot`（`ops.rs:1215-1241`）加载了当前
ctx 却调 `autonomous_gate(repo)` 不传 state。结果：一个没有任何可观测完成信号的 run，
只要仓库历史够厚就能解锁 autonomous。

同时现有 reliability gate 有两个实证缺陷：
1. **一次历史 timeout 永久污染**：`ops.rs:3256-3267` 对任意历史 timeout/rate_limited 用
   `any` 拒绝，直到 events 被手动 prune——旧网络故障把 autonomous 永久锁死。
2. **`mining_dispatches` 名实不符**：`ops.rs:3230-3237` 实为各 slot `distinct_runs` 求和，
   不是 dispatch 次数——阈值语义错位。

## ⚠️ 必读：前提（红线）

- **不新增平行 gate**：扩展现有 `autonomous_gate`，返回两个命名子结果；不加新命令、
  不加 daemon。
- **失败方向只降级**：observability 未证实 → 回 supervised/NEEDS_CONFIRM；不自动补
  instrument、不自动 route、不替 host 选目标、human gate 不削弱。
- **区分声明与证实**：instrument 只有非空字符串 = `signal_declared`；有结构化关联证据
  = `observable_verified`。不许把声明假装成已观测。

## 核心架构裁决

```text
autonomous_gate(repo, state) -> GateReport {
  operational_reliability:      pass | fail(原因)     // 现有逻辑，修两个缺陷
  current_run_observability:    observable_verified | signal_declared | missing(缺哪些)
}
放行条件：reliability=pass 且 observability=observable_verified；
signal_declared → NEEDS_CONFIRM（打印「已声明未证实」+ 缺的证据），missing → 退回 supervised。
```

`observable_verified` 判据（当前 run）：
1. `goal` 与 `done_when` 非空（C2 落地后新 run 恒真；旧 run 在这里兜底）；
2. delivery contract 的 `instruments` 至少一条；
3. instrument 与最新 evidence 有结构化关联：state 里存在引用该 instrument 命令的
   task evidence / runner 记录，且其结果可解析（rc 或输出中可判 pass/stop）。
   实现从 task evidence 的 command/note 与 instrument 字符串做归一匹配（trim/引号容差），
   不做语义猜测——匹配不上就是 signal_declared，诚实降级。

reliability 修复：
- timeout/rate_limited 判定改为**按 runner/model/task type 匹配 + 有界近期样本**
  （建议：各 slot 取最近 N=20 条完成记录，failed 比例 ≥0.5 或连续 3 次失败才拒；
  单次历史 timeout 不再永久否决）。
- `mining_dispatches` 改名 `mining_distinct_runs`（或改成真 dispatch 计数——裁决：改名，
  语义诚实优先，阈值常量同步改名 `AUTONOMOUS_MIN_MINING_RUNS_SUM`），
  `telemetry.rs::cross_run_mining`（`telemetry.rs:245-332,518-547`）字段名若外露 JSON
  需 serde 别名兼容旧 telemetry 读取。

## Phase 划分

### Phase 1：gate 签名扩展 + observability 判定
- `autonomous_gate(repo)` → `autonomous_gate(repo, &state)`；`cmd_autopilot` 传当前 state；
  输出两个命名子结果（stdout 文本 + 结构化）。
- 测试：无 instrument → missing；有 instrument 无 evidence 关联 → signal_declared →
  NEEDS_CONFIRM；有关联且可解析 → observable_verified；observability 不过时绝不执行
  （现有 `:1233-1242` NEEDS_CONFIRM 提前返回路径复用）。
- 收口：cargo 全绿 + `lto audit --auto-dispatch`。

### Phase 2：reliability 有界样本 + 改名
- 近期样本窗口 + 比例/连续失败规则替换 `any`；常量集中在 `ops.rs:3185-3190` 现有
  `AUTONOMOUS_*` 区。
- `mining_dispatches` 改名 + serde 兼容；telemetry 消费方同步。
- 测试：单次历史 timeout 不再锁死（构造 events fixture：19 ok + 1 timeout → pass）；
  连续 3 failed → fail；比例 0.5 边界。
- 收口：cargo 全绿 + privacy 自检（动 telemetry）。

### Phase 3：文档
- SKILL.md 域Ⅲ卡 autonomous 一句补「先观测后控制：当前 run 无已证实观测信号不放行」；
  run-state-workflow.md Autopilot 节；execution-loop.md autopilot 段。
- 收口：全套 gate + docs checker + 异构审计 + ledger 收敛。

## 复用（勿重写）

- `autonomous_gate` 主体与 `AUTONOMOUS_*` 常量（`ops.rs:3185-3291`）。
- `telemetry.rs::cross_run_mining` 聚合（只改字段名与消费口径，不重写扫描）。
- NEEDS_CONFIRM 返回路径（`ops.rs:1233-1242`）。
- delivery contract 读取（`state.rs:39-53`）。

## 完成判据（可验证）

- 新增 ≥7 测试全绿（observability 三态 ×2、timeout 有界 ×3、改名兼容 ×1、端到端降级 ×1）。
- 真机：本 repo（历史 run 充足）`lto autopilot --autonomous` 在无 instrument run 上输出
  NEEDS_CONFIRM/missing 并退回 supervised，不执行任何子步骤。
- `grep -n 'mining_dispatches' src/` 为 0（或只剩 serde 别名）。
- baseline/pass line：改动前记录 `lto autopilot --autonomous` 在 fixture 上的输出，改后
  reliability 判定对无 timeout 历史的 fixture 不变（回归）。
- 全套 gate + privacy 自检绿。

## 不可自动化的安全阀

- host 亲验：真实 run 三态各验一次（missing / signal_declared / observable_verified）。
- 任何放宽（如样本窗口 N、比例阈值）不得让「纯主观证据」或「零观测」通过——测试断言。
