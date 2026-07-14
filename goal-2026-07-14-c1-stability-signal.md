# Goal: C1 ledger 稳定性信号——硬 verdict + 正交 diagnostics（砍 Python 重复 evaluator）

> 致 codex：沿用约束（LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写 release 归 host）。
> **这份只做 C1，做完就停，别做 C2/C3/C4**（它们是独立 goal）。
> 进行中的 untracked 新文件（如 `src/ledger.rs`）是预期状态，审计报 untracked 不当 blocker。
> 别信 backlog/CHANGELOG 当 Rust 现状，以 `grep src/*.rs` 实证为准。

## 为什么做（目标 + 第一性）

审计收敛判据是 closeout 的硬闸门，但现状有四个真问题（异构评审 2026-07-14 坐实）：
1. **Rust/Python 双实现不等价**：Rust `src/commands/util.rs:681-700` 先判末轮=0 短路
   Converged（测试 `util.rs:885-905` 要求 `[1,2,0]→Converged`）；Python
   `scripts/audit_ledger_check.py:101-122` 先扫 rebound，同序列判 REBOUND。文档让用户跑
   Python 脚本，生产 gate 走 Rust——同一份 ledger 两个答案。
2. **空 ledger ≠ 零 blocker 语义分裂**：evaluator 把空轮次判 Converged，`check`
   （`src/commands/ops.rs:563-570`）报 no filled rounds，`closeout`
   （`src/commands/closeout.rs:157-175`）直接用 evaluator 因而可放行。
3. **事件名撒谎**：每次 append round 都发 `audit.converged`（`src/cli.rs:1995-2034`、
   `src/event_emit.rs:305-330`），不论 blocker 是否为零；telemetry 又把它当 audit round 计数。
4. **判据只看相邻轮次**：`REBOUND/STALLED` 是双点比较，识别不了振荡（modulated
   oscillation）和发散（envelope expanding）——「稳定性第一」（工程控制论域Ⅱ）要求
   对整个序列判形态。

## ⚠️ 必读：吸收的教训 / 前提

- **judge/诊断信号不进 promote 决策**：diagnostics 只提示 host，绝不改变 promote/route/
  human gate（LTO 原则 5：sensors are fallible）。
- **closeout 首版仍只依赖兼容后的硬 verdict**——diagnostics 是只读旁路。
- ledger 观测本质不可比（auditor 集合/审计范围/finding 身份会变）：没有 lineage 时
  diagnostics 必须标 low-confidence advisory，不许用控制论术语制造超出证据的结论。
- schema 演进：optional-load + new-write + 旧 fixture 回归（`tests/fixtures/legacy-run/`），
  不得把历史 JSON/ledger 判非法。

## 核心架构裁决（host 已拍板，别做歪）

- **硬 verdict 不动**：`Converged/Converging/Rebound/Stalled` 语义保持 Rust 现状
  （末轮=0 短路 Converged 是对的——历史反弹不应永久否定已修到零的当前轮）。
- **diagnostics 是正交只读维度，不塞一个 enum**：
  ```text
  sample_sufficiency: insufficient | sufficient      （轮次 <3 = insufficient）
  terminal:           zero | nonzero                 （末轮 blocker）
  direction:          improving | flat | worsening | mixed
  oscillation:        none | single_rebound | alternating
  envelope:           shrinking | flat | expanding | unknown（峰值包络）
  ```
- **evaluator 下沉 `src/ledger.rs`**：`LedgerRound`/`LedgerVerdict`/`parse_ledger`/
  `evaluate_ledger`（现 `src/commands/util.rs:60-111,649-701`）移到新模块，ops/closeout/
  telemetry 共用，避免 telemetry 反向依赖 commands。
- **Python evaluator 砍掉**：`scripts/audit_ledger_check.py` 改薄壳（调 `lto check` 的
  ledger 输出或直接删+文档指向 Rust），绝不保留第二份判定逻辑；共享 golden fixtures
  验证薄壳与 Rust 同答案。
- **事件更名**：新增 `audit.round.recorded`（append 时）+ `audit.ledger.evaluated`
  （带 verdict 字段）；`audit.converged` 停写、读路径按历史 schema 解释（telemetry 兼容读）。
- **振荡 → 换假设只 advisory**：`sample_sufficiency=sufficient` 且 `oscillation=alternating`
  或 `envelope!=shrinking` 时，check/closeout 提示展示 delivery contract 的 `forced_entropy`
  文本（若有），由 host/human 决定换假设；单次 rebound 不触发。

## Phase 划分

### Phase 1：evaluator 下沉 + 空 ledger 语义
- 新建 `src/ledger.rs`，搬四件套；`ops.rs`/`closeout.rs`/`cli.rs` 改 import；
  行为零变化（现有测试 `[1,2,0]→Converged` 等全部原样过）。
- 裁决空 ledger：`parse_ledger` 无已填轮次 → 新 verdict 变体 `NoObservations`（或
  evaluate 返回 Option）；closeout 对高风险 run 视同「无 ledger」拒绝（复用
  `closeout.rs:260` 路径语义），check 报 no filled rounds 保持。
- 测试：空表/全空行/только模板 → 不再是 Converged；closeout 高风险 + 空 ledger 拒收尾。
- 收口：cargo 全绿 + `lto audit --auto-dispatch` + `lto check`。

### Phase 2：diagnostics 五维 + 事件更名
- `src/ledger.rs` 增 `LedgerDiagnostics` 与 `diagnose(&[LedgerRound]) -> LedgerDiagnostics`；
  纯函数，输入序列输出五维。
- ledger 行观测可比性：`append_audit_ledger_round` 记 auditor set + coverage 说明列
  （模板 `templates/audit-ledger.md` 同步）；缺 lineage 时 diagnostics 输出附
  `confidence: low (no lineage)`。
- 事件：append 发 `audit.round.recorded`；evaluate 发 `audit.ledger.evaluated{verdict,
  terminal, oscillation}`；`event_emit.rs`/`telemetry.rs` 接线；`audit.converged` 只读兼容。
  **telemetry 轮数统计必须双事件兼容**（异构评审 R3-F1：`telemetry.rs:259` 现硬编码只数
  `audit.converged`，停写后新 run 的 audit_rounds 恒 0，会把 C4 autonomous_gate 的跨 run
  证据饿死）：`audit_rounds` = 旧 `audit.converged` + 新 `audit.round.recorded` 合并计数
  （同 run 同轮去重），加测试：纯新事件 run 的 audit_rounds > 0。
- check 输出增一行 diagnostics 摘要 + forced_entropy advisory 提示。
- 测试：`[1,2,0]`→Converged+single_rebound；`[5,2,4,1,3]`→oscillation=alternating+
  envelope 判定；`[5,4,3]`→improving+nonzero；telemetry 对旧 `audit.converged` 事件仍能读。
- 收口：cargo 全绿 + `grep -rn 'audit.converged' src/` 只剩读路径 + 异构审计。

### Phase 3：Python 薄壳化 + 文档
- `scripts/audit_ledger_check.py` → 薄壳或删除（裁决：优先薄壳打印「use `lto check`」
  并以相同退出码代理调用 Rust，保 CI 兼容一个版本）。
- 文档同步：SKILL.md 域Ⅳ卡、audit-convergence.md 机器判收敛节、COMMANDS.md（若 check
  输出面变化）；`scripts/check_docs_consistency.py` 的 fenced 检查须仍绿。
- 收口：`python3 scripts/check_docs_consistency.py` + `scripts/check_python_rust_ownership.py`
  + cargo 全绿 + `scripts/privacy_self_check.sh`（改了 events/telemetry）。

## 复用（勿重写）

- 解析/判定四件套 `src/commands/util.rs:60-111,649-701`（搬家不重写）。
- 事件注册表机制 `src/events.rs`（known event registry enforced on write）——新增事件类型
  照现有注册模式。
- 收敛测试 `util.rs:885-905`、closeout 测试 `closeout.rs:593-665`。

## 完成判据（可验证）

- `cargo test --locked ledger` 新增 ≥8 个测试全绿（含上述序列用例 + 空 ledger + legacy fixture）。
- `grep -rn 'audit.converged' src/ | grep -v 'read\|legacy\|test'` 无写入路径。
- `python3 scripts/audit_ledger_check.py <fixture>` 与 `lto check` 对 3 个 golden fixtures
  给出相同 verdict（或脚本已删且文档零引用）。
- 全套 gate：fmt/check/clippy/test + 两个 docs checker + privacy 自检。
- baseline/pass line：改动前后对 `tests/fixtures/legacy-run/` 跑 `lto check --json`，
  硬 verdict 输出不变（回归证明），新增 diagnostics 字段只增不改。

## 不可自动化的安全阀

- host 亲验：不信自述，收口后 host 用真实 run 的 ledger 手跑 `lto check` 对照。
- diagnostics 不得接进任何 promote/route/gate 决策路径——测试里断言。
