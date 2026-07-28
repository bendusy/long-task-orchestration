# Goal: 修 reverify 审计 findings（HIGH 排序 + 2 条 MEDIUM）

> 致 codex：沿用既有约束。**这份只修下列三条，做完就停。**审计报告 `findings-audit-pi.md` 里的其余条目（MEDIUM 2 无总预算、LOW 1、LOW 2）本轮**不修**，host 已裁决记为已知限制。

## 背景

你上一轮做的 commit `59e08ef`（closeout 复跑明卷）经异构审计（pi）+ host 亲验，发现 1 条 HIGH、2 条值得一并修的 MEDIUM。功能主体是对的，只需调顺序与补诊断。

## 修复 1（HIGH）：reverify 必须排在零副作用的必拒闸之后

**问题实证**（host 端到端跑过，不是推测）：构造一个 `current_phase=closed` 且 instruments 为 `touch /tmp/x` 的 run，跑 `closeout` 得到：

```
closeout reverify: 1/1 instruments passed
Error: run already closed (use --force to rewrite)
```

`/tmp/x` **被创建了**。即：一个纯状态判断（run 已关闭）本该零成本早退，却让任意 shell 先跑完了。unresolved blocks 同理。

**落点**（当前 `src/commands/closeout.rs`）：
- reverify 段：`:207-250`
- unresolved 检查：`:287`
- already-closed 检查：`:304`
- dirty 检查：`:314`

**要求**：把 reverify 段整体挪到 **already-closed（:304）与 unresolved（:287）之后、dirty（:314）之前**。

**死规矩**：
- 做到 dirty 检查仍在 reverify **之后**，否则 instrument 改脏工作树将检测不到——这是原顺序里唯一正确的部分，别一起挪坏了。
- 做到 already-closed / unresolved 命中时**一条 instrument 都不执行**。

**验收（必须是"证明没被调用"，不是"看起来对"）**：
写一个回归测试：closed run + instrument 为 `touch <tempdir>/should-not-exist`，断言 `enforce_gates` 返回 Err 且**该文件不存在**。用 `tempfile::tempdir()`（本项目已用，见现有测试）。

## 修复 2（MEDIUM）：失败时保留 stdout

**问题**：`:214` 处 `let (rc, _, stderr, _)` 丢弃了 stdout。`cargo test` 的断言详情打在 **stdout**，失败时 host 只看到 `closeout reverify failed: label (rc=1)`，stderr 可能是空的——host 不知道为什么红，会被推向 `--force`，正好废掉这个功能的意义。

**要求**：失败时合并打印 stdout 与 stderr 的尾部各若干行（现在是 stderr 末 8 行，保持这个量级）。哪一侧为空就只打印非空的一侧。

## 修复 3（MEDIUM）：失败即停

**问题**：`:217-244` 一条失败后 `continue` 跑完剩余全部 instrument，最后才 bail。历史 run 里有 8 条 instrument 的（`.lto/20260715-c4-observability-subgate`），首条就失败时仍会把剩下 7 条任意 shell 跑完，白等且放大副作用。

**要求**：默认**首败即停**（记下该条 label 后立即 bail，不再跑后续）。不要加 flag，不要保留"跑完全部"的选项——需要看全部失败情况时 host 自己去掉那条重跑即可。

**注意**：`ReverifyResult.attempted` 的语义随之改变（变成"已尝试的条数"而非"总条数"）。相应调整 `gate.evaluated` 事件里的字段含义，并在 COMMANDS.md 里写清。

## 全局收口

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/check_docs_consistency.py
git diff --check
```

**注意**：`python3 scripts/check_python_rust_ownership.py` 有一条**先于本轮存在**的 FAIL（`references/python-rust-ownership.json` 缺 `get`/`describe`，host 已在基线复现确认）。这条不是你造成的，**不要修它**（manifest 不在白名单），跑到它红了就在汇报里注明"既有漂移，与本轮无关"。

## 规矩

- **防作弊点名**：`#[ignore]`、放松断言、mock 被测对象、删测试、`|| true`、改阈值——全算失败。测试数只许 ≥ 基线（当前基线 449，动手前复核）。
- **不新增依赖**。
- 同一条验收连败 3 次换下一项，卡住的写 `BLOCKED.md`。
- 结果比基线差就回滚如实报告。
- 进度写 `PROGRESS.md`（覆盖上一轮内容，只留本轮）。

## 界限

- **只允许改**：`src/commands/closeout.rs`、`COMMANDS.md`、`PROGRESS.md`、`BLOCKED.md`。
- 若修复 3 确需改 `src/cli.rs` 的 help 文案，可改 cli.rs，但**只改文案不改逻辑**。
- 其余只读。`state.rs` 本轮不需要动。
- 判卷标准（`scripts/`、`.github/`）碰都不许碰。

## 完成条件

1. **硬指标一**：closed run + 副作用 instrument 时，该副作用**不发生**（有测试断言文件不存在）；首败即停有测试；失败输出含 stdout。
2. **硬指标二**：`git diff --stat` 只落白名单文件；收口 6 条命令全绿（ownership 那条按上文说明处理）。
3. 每条贴**实际命令输出**，只说"做完了"不算。特别是硬指标一，要贴出测试断言"文件不存在"的实际输出。
4. `BLOCKED.md` 随交付提交，空的也写「无」。
5. 止损：跑满 8 轮即停。
