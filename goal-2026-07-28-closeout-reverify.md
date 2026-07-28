# Goal: closeout 复跑明卷（host 侧独立复核 instrument）

> 致 codex：沿用既有约束（LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写、release 归 host）。
> **这份只做「closeout 时复跑 delivery contract 的 instruments」一件事，做完就停。**
> 不要做暗卷/隐藏检查（已被合议否掉，2026-07-28 codex+pi 一致：同 uid 藏不住）、不要碰 audit、不要改 ponytail 约束文案（host 自管，零代码）。

## 为什么做

现状：delivery contract 的 `instruments`（`src/state.rs:49`，`Vec<String>`，格式 `[LABEL::]CMD`）只在 `closeout` 里被**校验非空**（`src/state.rs:149-150,172-191`），**从来没有被真正跑过**。执行 agent 自己跑验收命令、自己在汇报里说"全绿"，host 只看到它的自述。

要的结果：closeout 时由 **host 侧独立复跑**一遍 instruments，rc 非 0 就拒绝 closeout。**安全模型是「标准公开、判定不自签」**——执行 agent 看得见验收标准（这是好事，它该知道目标），但最终判定由 host 在自己的上下文里跑，agent 说了不算。

干完世界什么样：host 不必手工复跑验收命令；agent 汇报"全绿"但实际没绿的情况会在 closeout 被当场拦住。

**让步顺序**：判得准 > 覆盖全 > 跑得快。

## ⚠️ 必读：前提与已知坑（合议实证，别重新推）

1. **绝不要复用 `run_task_command`（`src/commands/ops.rs:2347-2459`）**。它会把 `"command": command` 原文写进 task evidence 再 `save_run` 落 repo 内 `state.json`（实证落点 `src/commands/ops.rs:2376`），还会改 task status。本 goal 只要"跑命令拿 rc"，**正确原语是 `crate::commands::util::run_command_capture`（`src/commands/util.rs:766-803`，签名 `(repo, command, cwd, timeout_sec) -> (rc, stdout, stderr, elapsed)`）**。
2. **instrument 格式是 `[LABEL::]CMD`**，解析逻辑见 `src/state.rs:231-237 instrument_has_command`。有 `::` 则前段是 label、后段是命令；无 `::` 则整串是命令。**不要另写解析，抽一个共用函数出来给两边用**。
3. **`enforce_gates` 是唯一正确落点**：`src/commands/closeout.rs:25` 调用它，成功事件在 `:26`、状态写入在 `:64`。复跑必须在 `enforce_gates` 内部，**在任何成功事件与状态写入之前**。
4. **现有 gate 一律可 `--force` 绕过**（`closeout.rs:158`，文案统一是 `use --force to override`）。本 gate **必须遵循同一惯例**，不要发明"不可 force 的 gate"——与现有 UX 不一致会让 host 卡死无逃生。
5. 进行中的 untracked 新文件是预期状态，异构审计若报 untracked 为风险，记录但不当 blocker。
6. 别信 CHANGELOG/backlog 说某能力"已实现"，一律 `grep src/*.rs` 实证。

## 核心架构裁决（host 已定，不要另选）

**裁决 1：默认开还是默认关** —— **默认开**。有 instruments 就复跑。理由：instruments 本来就是 host 声明的验收标准，不跑等于白写。空 instruments（合法状态，见 `state.rs:136`）→ 跳过，行为不变。

**裁决 2：超时** —— 每条 instrument 默认 300 秒，与 runner 默认一致（`--timeout` 默认 300s，见 CLAUDE.md「Runner delegation」段，先 grep 实证再用）。加 `--reverify-timeout <SEC>` 覆盖。**必须有超时**，否则一条挂死的命令会让 closeout 永久挂住。

**裁决 3：cwd** —— repo root。与 `run_command_capture` 的 `cwd=None` 默认行为一致。

**裁决 4：失败输出** —— 打印失败的 label（无 label 则打印命令）、rc、stderr 尾部若干行。**这里不需要脱敏**——instruments 本来就是公开的，这是本方案与被否掉的暗卷方案的关键区别，不要引入任何隐藏逻辑。

**裁决 5：结果落哪** —— 只进 closeout 的输出与 `gate.evaluated` 事件的 fields（复跑了几条 / 过了几条 / 失败的 label 列表）。**不要新建 evidence store、不要落 stdout 全文**。要看详情就重跑那条命令。

## Phase 1：复跑与 gate

**要求**：

1. `src/state.rs`：抽出公共解析函数 `pub fn split_instrument(s: &str) -> (Option<&str>, &str)`（返回 label 与命令），让 `instrument_has_command`（`:231`）复用它，**不要留两份解析逻辑**。
2. `src/commands/closeout.rs`：在 `enforce_gates`（`:153`）内新增复跑段。位置在现有 delivery-contract 完整性检查之后（`:186` 附近）、ledger 检查之前。逻辑：
   - `options.force` 为真 → 跳过（与其他 gate 一致）
   - `ctx.state.delivery_contract.instruments` 为空 → 跳过
   - 逐条 `util::run_command_capture(repo, cmd, None, timeout)`，rc != 0 记为 fail
   - 有 fail → `anyhow::bail!` 拒绝，消息含失败 label 列表 + `(use --force to override)`
3. `src/cli.rs`：`Closeout` 命令加 `--reverify-timeout <SEC>`（默认 300）与 `--no-reverify`（显式关闭，给 host 逃生口）。

**死规矩**：
- 做到复跑走 `run_command_capture`，否则命令原文会写进 repo 内 state.json —— 见「必读」第 1 条，这是本 goal 唯一红线。
- 做到 instruments 为空时 closeout 行为**一字不变**，否则历史 run（本 repo 有 20+ 个）全被新闸拦住，是回归事故。这条必须有专门回归测试。
- 做到超时生效，否则挂死的命令让 closeout 永久卡住。

**验收（含反向验证，红→绿两段都必须贴）**：

```bash
cargo test --locked closeout
cargo test --locked instrument

# 反向验证：证明闸门真的会拦（"坏了没人会知道"类检查，必须证明报警器会响）
# 用单测注入而非全量 CLI —— 全量 closeout 前面还有 readiness/dirty/ledger 等闸，
# 会先被别的 gate 拒绝，红输出与本 gate 无关，是假红。
# 写两个单测：
#   1. instruments = ["fail::exit 1"] → enforce_gates 必须 Err，且错误消息含 "fail"
#   2. instruments = ["ok::true"]     → enforce_gates 在本段不报错
#   3. instruments = []               → 与改动前行为一致（回归测试）
```

判定：三个测试全过；**在对话里贴出测试 1 的失败断言实际输出**（证明它真的拦住了），再贴全绿。

## 全局收口

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/check_docs_consistency.py
python3 scripts/check_python_rust_ownership.py
git diff --check
```

`check_docs_consistency.py` 会校 CLI 面与 `COMMANDS.md` 一致——新增两个 flag 必须同步更新 `COMMANDS.md` 的 closeout 段。

## 规矩

- **防作弊点名**：让测试绿的最省力姿势是 `#[ignore]`、放松断言、mock 掉被测对象、删测试、`|| true`、改阈值——**全算失败**。测试数只许 ≥ 基线：动手前先跑 `cargo test --locked --all-targets 2>&1 | tail -5` 记下基线数字，写进 `PROGRESS.md`。
- **不新增依赖**。要加先写 `BLOCKED.md` 停下。
- **同一条验收连败 3 次换下一项**，卡住的写 `BLOCKED.md`。
- **结果比基线差就回滚如实报告**——"没做成但说清了"合格，"做了但更糟"不合格。
- 进度写 `PROGRESS.md`，每做完一项立刻更新；换会话先读它别重做。
- 拿不准的写 `BLOCKED.md`，跳过继续做别的，最后随交付提交。**中途没人可问**。

## 界限

- **只允许改**：`src/state.rs`、`src/commands/closeout.rs`、`src/cli.rs`、`COMMANDS.md`、`PROGRESS.md`（新建）、`BLOCKED.md`（新建）。测试写在上述 src 文件的内联 `#[cfg(test)]` 模块里（本项目惯例，227+ 内联测试）。
- **其余只读**。特别是 `ops.rs` / `dispatch_goal.rs` / `audit*.rs` 只读不改——本 goal 不需要动，想动就是跑偏了。
- **判卷标准碰都不许碰**：`scripts/check_*.py`、`scripts/privacy_self_check.sh`、`.github/`。
- 顺手活（一行能修的 bug、顺手重构、顺手升依赖）写 `BLOCKED.md` 待裁决，不动手。

## 完成条件

1. **硬指标一（结果）**：instruments 非空且任一条 rc != 0 时 closeout 被拒绝并列出失败 label；instruments 为空时行为不变；`--force` 与 `--no-reverify` 均可绕过；每条都有测试。
2. **硬指标二（守约束）**：`git diff --stat` 显示改动只落在「界限」白名单内；全局收口 7 条命令全绿；`grep -n "run_task_command" src/commands/closeout.rs` 无输出（证明没走错原语）。
3. 每条都要在对话里贴**实际命令输出**（含反向验证的红→绿证据），只说"做完了"不算。
4. `BLOCKED.md` 随交付提交，空的也写「无」。
5. 止损：跑满 10 轮即停，如实汇报卡在哪、还差什么。
