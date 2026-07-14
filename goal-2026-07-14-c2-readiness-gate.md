# Goal: C2 信息不足禁猜闸门——run readiness + delivery contract completeness 两层

> 致 codex：沿用约束（LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写 release 归 host）。
> **这份只做 C2，做完就停，别做 C1/C3/C4**。
> 别信文档声称，以 `src/cli.rs` / `src/state.rs` 实证为准。

## 为什么做（目标 + 第一性）

工程控制论推理策略 2.5「信息不足禁猜」：缺关键信息时输出「需补充：…」，禁止直接设计。
LTO 现状违反这条：
- `start` 把缺失的 `goal/why/done_when` 转成**空字符串静默写盘**（`src/cli.rs:1093-1118`，
  `start_run` `:2219-2259` 无验证）——run 从第一秒起就没有可判定的完成标准，recap/resume/
  closeout 全部退化。
- delivery contract 的完整性判定**已存在但没接线**：`src/state.rs:92-111`
  （`missing_sections()`/`is_complete()`）只在 check phase gate 且 contract 非空时用
  （`src/commands/ops.rs:794-818`）；`preflight` 完全不看 contract（`ops.rs:234-350`）。
- partial contract（如只给 `--target` 不给 `--instrument`——目标不可测量）静默写盘后会被
  phase gate 卡死，用户只能手改 JSON——因为没有 typed contract update 入口。
  实证：本 run（20260714-043510）开建时漏 `--host`，事后无入口补写，只能手改 state.json。

## ⚠️ 必读：前提

- **空 contract 必须继续兼容**：普通 run（非 /goal 型）四件套全空是合法状态；legacy
  state 必须可读（`tests/fixtures/legacy-run/` 回归）。
- **`--why` 保持 advisory**：缺它只 WARN 不拒（recap 会温和提示）。
- 报错信息输出**真实 CLI flags**（`--target/--constraint/--instrument/--entropy-check/
  --goal/--done-when`），不输出内部字段名（targets/forced_entropy）。

## 核心架构裁决

两层规则，名字叫 **run readiness**（基础）+ **contract completeness**（扩展）：

1. **base readiness**：新 run 要求非空 `--goal` 且非空 `--done-when`。缺任一 → 在**任何
   目录/state 写入之前** fail。**校验只放 CLI 参数层**（Start 分支，start_run 之前）：
   库层构造函数与 state 序列化不动——直接构造 state 的单测不受影响；确有走 CLI 空参路径的
   旧测试属于断言旧行为，随本 goal 更新。**禁止 `#[cfg(test)]` 测试后门**（会削弱闸门，
   异构评审 R3-F4 的建议按此驳回）。stderr 输出：
   ```
   需补充: --goal "<一句话目标>" --done-when "<怎么算做完>"
   （信息不足禁猜：没有完成标准的 run 无法判收敛，recap/closeout 都会退化）
   ```
2. **contract completeness**：四件套全空 → 放行（普通 run）。非空时**分级判定**
   （异构评审 R2-F2：全空或全满会诱导用户干脆全空规避校验）：
   - **成对强制**：`--target` 与 `--instrument` 必须成对（有目标必须有测量手段，反之
     亦然——不可证伪的 target 正是禁猜要拦的）；缺对 → 写盘前 fail，输出缺的真实 flag。
   - **可选项**：`--constraint`/`--entropy-check` 缺省只 WARN 不拒（渐进式加约束合法）。
3. **typed update 入口**：新增 `lto contract set [--run-id] --target ... --constraint ...
   --instrument ... --entropy-check ...`（可只补缺项；重复 flag 追加，与 start 一致）——
   服务旧 run / 后补契约；partial 已在 start 被拒，update 入口写盘前同样做 completeness 校验。
   **instrument 支持可选显式 label**：`--instrument "<label>::<cmd>"`（`::` 分隔，无 `::`
   则整串为 cmd、label 缺省）——这是 C4 可观性「label 优先」引用键的参数面入口（异构评审
   R5-F2：没有 CLI 入口 label 优先就是空中楼阁），start 与 contract set 同语法。
4. **preflight 解耦**：`preflight` 主职责仍是环境健康；有显式 `--run-id` 或 active run 时，
   增加独立子结果 `run_readiness`（ok/missing 列表），与 `--record`（`ops.rs:328-343`）解耦
   ——不带 `--record` 也报告 readiness；显式给了 `--run-id` 但 run 不存在必须报错不静默。
5. **`check --to implementation --strict` 与 start 规则一致**：新写**分级判定函数**
   `state.rs::completeness_missing()`（成对强制 target↔instrument；constraint/entropy-check
   只出 WARN 列表），start / `contract set` / check phase gate **三处共用同一函数**，不写
   第二份逻辑。现有 `missing_sections()`/`is_complete()` 是四项全满语义，**check gate 不得
   再直接用它**——否则 start 按分级放行的 run 会死锁在 `check --strict`（异构评审 R4-F1，
   goal 内部曾自相矛盾，已修正）；`is_complete()` 若无其他消费者随本 goal 收敛为
   completeness_missing 的包装或删除。

## Phase 划分

### Phase 1：base readiness + contract completeness（start 写盘前拒）
- 落点：`src/cli.rs` Start 分支（`:1093-1122`）在调 `start_run` 前校验；校验函数放
  `src/state.rs`（挨着 `missing_sections`，如 `readiness_missing(goal, done_when) -> Vec<&str>`）。
- **顺手修 host 静默默认**：`cli.rs:2123` 不传 `--host` 时 `unwrap_or("codex")`——host 未知
  不该猜 codex（实证：2026-07-14 run 因此把健康的 codex 错误排除出异构审计池三轮）。改法：
  未传时写 `unknown`，`pick_auditors` 对 unknown 不做同族排除（全池可用）并在 audit 输出
  WARN 提示补 host；readiness 输出提示补 `--host`。
- 测试：缺 goal / 缺 done-when / 双缺 → 非零退出 + 无 `.lto` 新目录；只给 target 不给
  instrument（或反之）→ 拒 + 列缺 flag；target+instrument 成对但缺 constraint/entropy-check
  → 成功 + WARN；四项全满 → 成功；全空 contract → 成功；`--force` **不**豁免 readiness
  （force 语义是覆盖已有 run-id，不是跳过禁猜）。
- 收口：cargo 全绿 + `lto audit --auto-dispatch`。

### Phase 2：`contract set` 命令
- 落点：`src/cli.rs` 新 `Commands::Contract { Set { ... } }`（或 flat `contract-set`——裁决：
  用 `contract set` 子命令树，与 `task add`/`run parallel` 风格一致）；实现放
  `src/commands/ops.rs` 或新 `src/commands/contract.rs`（若 ops.rs 已超载优先新文件）。
- 行为：读 state → merge 非空参数 → completeness 校验（merge 后仍缺 → fail 列缺项）→ 写盘
  + 发既有 state 更新事件；`COMMANDS.md` 行数与 `src/cli.rs` COMMANDS 同步（checker 强制）。
- 测试：旧 run（空 contract）补写全套成功；补一半拒绝；merge 追加语义。
- 收口：cargo + docs checker 全绿。

### Phase 3：preflight readiness 子结果 + 文档
- 落点：`ops.rs::cmd_preflight` 增 `run_readiness` 段（文本 + `--json` 字段）。
- 文档：run-state-workflow.md Start/Preflight 节、SKILL.md 域Ⅱ卡一句话、COMMANDS.md。
- 回归矩阵（全部成测试）：空 contract 兼容 / partial 拒绝 / complete 成功 / legacy state
  可读 / explicit preflight missing run 报错 / `check --to implementation --strict` 与 start
  同判。
- 收口：全套 gate + 异构审计 + ledger 收敛。

## 复用（勿重写）

- `state.rs:92-111` `missing_sections()`/`is_complete()`——completeness 唯一判定源。
- `ops.rs:794-818` `add_delivery_contract_phase_check`——phase gate 消费方，改为复用同一函数。
- clap 子命令树模式参考 `Commands::Task`/`Commands::Run`（`src/cli.rs`）。

## 完成判据（可验证）

- 上述回归矩阵 6 类全部有测试且绿。
- `lto start --goal x`（无 done-when）非零退出且 `.lto` 无新目录：
  `ls .lto | wc -l` 前后一致。
- `lto contract set --target t --constraint c --instrument i --entropy-check e` 对旧 run
  成功补写，`lto check --to implementation` 的 `delivery_contract_complete` 转 ok。
- baseline/pass line：`tests/fixtures/legacy-run/` 全部命令行为不变（回归）。
- 全套 gate：fmt/check/clippy/test + docs checker ×2 + `git diff --check`。

## 不可自动化的安全阀

- host 亲验：真机跑缺参 start / 补契约 / preflight readiness 三条流。
- 报错文案由 host 终审（信息不足提示是给人读的，机器判据是退出码）。
