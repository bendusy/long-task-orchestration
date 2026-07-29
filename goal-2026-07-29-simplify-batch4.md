# Goal: lto 精简第 4 批（收官轮）——你自己审出来的四项窄清理

> 致 codex：沿用既有约束（LTO 自管 / 红线不弱化 / commit 你写、release 归 host）。
> **这份做完就停，是精简的最后一轮。**不要扩抽象、不要扫 pedantic 噪音、不要碰 CLI 契约 / `.lto/` 文件协议 / gate 语义。

## 背景

这四项是**你自己在 `findings-final.md` 里审出来并已在临时副本验证过的**。host 已复核，确认你对我错：

- 我说"六处 clone 是 clippy 误报、删不掉"，你实测全删后 `cargo check` + 全套测试通过。host 亲验了 `closeout.rs:89-90` 两处，**确认能编译**，我的判断错了。原因是 `json!` 对 `&CloseoutOptions` 的 `String` 字段走序列化借用，不 move。
- `shell_quote` 那条是 host 上一轮的漏网之鱼：我当时用 `grep "fn shell_single_quote"` 找重复，名字不同的第四份因此没被抓到。

所以这轮**照你的结论做**，四项都做。

## 任务（四项，可以一个 commit 也可以分开，你定）

### 1. 补齐 `shell_single_quote` 归一（安全相关，优先级最高）

`src/commands/ops.rs:3298-3300` 的 `shell_quote` 与 `src/process.rs:37` 的 `shell_single_quote` **逐字节相同**（host 已比对确认），有 7 处调用。

`src/process.rs:31-38` 的文档注释明写这个转义 "must have exactly one definition" —— 现在是两处，注释与事实不符。

**做**：删掉 `ops.rs` 的局部 `shell_quote`，调用点改用 `crate::process::shell_single_quote`。

**验收**：`find src -name '*.rs' -exec grep -Hn "fn shell_quote\|fn shell_single_quote" {} \;` 只剩 `src/process.rs` 一处；`commands::ops::tests::tmux_worker_prompt_preserves_quoted_command_contract` 通过。

### 2. 删六处生产冗余 clone

`src/agent_turn.rs:107`、`src/commands/closeout.rs:89`、`:90`、`:232`、`:604`、`src/commands/ops.rs:2053`。

**注意 `agent_turn.rs:107` 那处**：它和 108 行是有意的字段别名（`goal_completion_proof` / `completion_proof` 指向同一值，代码里有注释说明）。删 clone 意味着调换两行顺序让最后一次 move——**如果这样做会让"这两个字段是别名"的意图变模糊，就保留这处 clone 并在汇报里说明**。其余五处按你验证的删。

测试代码里 `src/commands/ops.rs:5414` 那处可顺手删。

**验收**：`cargo clippy --locked --all-targets -- -W clippy::redundant_clone` 的生产代码告警数下降；全套测试绿。

### 3. blocker 判定改 `any`，不要 clone 收集

`src/commands/ops.rs:4114-4123`：为判断有没有 blocker，先把各 task 的 blocker 数组 `.cloned()` 收进 `Vec`，只为调用 `is_empty()`；`:4152-4171` 随后又遍历原数据渲染一遍。

**做**：前一段改成 `any(...is_some_and(|blockers| !blockers.is_empty()))`，省掉整批 JSON clone 与临时 Vec。

### 4. 去掉 `DependencyPlan::new` 的假 `Result`

`src/scheduler.rs:594-609`：构造函数只建索引，函数体**无任何失败路径**，却返回 `Result<Self, SchedulerError>`；唯一调用者 `src/scheduler.rs:169` 因此带一枚无意义的 `?`。

**做**：改为直接返回 `Self`，调用点去掉 `?`。

## 死规矩

- **每一项都要实际编译+跑测试验证**，不许只凭 lint 或静态阅读下结论（这是上一轮双方都踩过的坑：我引用 lint 没逐处验证，host 静态阅读没实测）。
- 测试数只许 ≥ 基线（当前 408 lib + 42 集成）。
- 不许 `#[ignore]`、`|| true`、放宽断言、mock 掉被测对象、改验收脚本。
- 任一项做不成，写进 `BLOCKED.md` 说明原因，**其余三项照做**，不要整批放弃。

## 界限

- **只允许改**：`src/process.rs`、`src/commands/ops.rs`、`src/agent_turn.rs`、`src/commands/closeout.rs`、`src/scheduler.rs`、`PROGRESS.md`、`BLOCKED.md`。
- 判卷标准（`scripts/`、`.github/`）碰都不许碰。
- 不要动 `run_job_file`、`enforce_gates` 的逻辑（上两批刚改过，已验收）。

## 收口

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/check_docs_consistency.py
python3 scripts/check_python_rust_ownership.py
git diff --check
```

## 完成条件

1. 四项各自完成或有 `BLOCKED.md` 说明。
2. `shell_quote` 全仓只剩 `src/process.rs` 一处定义（贴 grep 输出）。
3. 收口 6 条全绿，测试数 ≥ 408 lib。
4. 每项贴出**实际命令输出**，不是"做完了"。
5. 止损：跑满 8 轮即停。
