# Goal: lto 精简第 3 批——补齐 runner 生命周期事件的测试覆盖

> 致 codex：沿用既有约束（LTO 自管 / 红线不弱化 / commit 你写、release 归 host）。
> **这份只做测试补齐一件事，做完就停。**不要顺手重构 `run_job_file` 的实现、不要动 `enforce_gates`、不要碰 CLI 契约。

## 为什么做

上一批把 `cmd_runner` / `cmd_parallel` / `cmd_pipeline` 三份重复的 job-file 生命周期合并成了 `src/commands/ops.rs` 的 `run_job_file`（净减 83 行，已 commit `17a5934`）。

**但 host 亲验发现一个覆盖缺口**：把 `run_job_file` 里的 `emit_runner_started_jobs` 调用整个删掉，`cargo test --locked job_file` **依然全绿**。

实证复现（你自己跑一遍确认，别信我复述）：
```bash
# 在 run_job_file 里把 emit_runner_started_jobs(...) 那行换成 let _ = (run_id, &jobs);
cargo test --locked job_file    # 仍然 2 passed
```

也就是说：现有的 `job_file_scheduler_paths_record_agent_runs_with_explicit_run_id`（`src/commands/ops.rs` 内联测试）只核了 **agent_runs 落 state**，没核 **events.jsonl 里的生命周期事件**。这三条路径的事件如果哪天回归了，测试不会响。

要的结果：**这三个生命周期事件有测试守着**——started、submission-failed、results 各自有断言，删掉任一 emit 会让测试变红。

## ⚠️ 必读：前提

1. **`run_job_file` 的实现是对的，不要改它**。本 goal 只补测试。若你在写测试时发现实现真有 bug，写进 `BLOCKED.md` 报告，不要顺手改。
2. 事件写在 `.lto/<run-id>/events.jsonl`，追加式 JSONL。读取方式先 grep 现有测试怎么读的——**大概率已有 helper**（找 `events::read` / `read_events` / 测试里 `events.jsonl` 的既有断言写法），复用它，不要自己写 JSONL 解析。
3. 本项目测试惯例是**内联 `#[cfg(test)] mod tests`**（227+ 个），不要新建 `tests/` 文件。
4. 现有相关测试在 `src/commands/ops.rs` 的 tests 模块里，`job_file_scheduler_paths_record_agent_runs_with_explicit_run_id` 一带——**在它旁边加**，复用它已有的 fixture 搭建方式（临时 repo、job 文件、run 目录），不要重造。

## 核心裁决（host 已定）

**裁决 1：测什么** —— 三个事件各一条断言：
- `runner.started`（或实际的事件 type，你去 `event_emit.rs` 里确认真名）在三条路径下都发出，且 `fields` 里带正确的 source label（`runner.job_file` / `run.parallel` / `run.pipeline`）
- 提交失败时发出 submission-failed 事件
- 成功时 results 事件或 agent_runs 落 state（这条已有覆盖，确认即可，不必重复写）

**裁决 2：测到什么粒度** —— 断言"事件存在 + source label 正确"即可。**不要**断言完整 fields 结构——那会让事件 schema 的正常演进不断打断测试。

**裁决 3：三条路径都要覆盖还是一条就够** —— **三条都要**。它们现在共用 `run_job_file`，但 source label 是各自传的，写错了正是最可能的回归。

**裁决 4：submission-failed 怎么造** —— 需要让 `submit_jobs` 失败。先看现有测试有没有造失败的手法（比如非法 job、不存在的 runner）；如果造不出来，**写进 `BLOCKED.md` 说明原因，只做 started 那部分**，不要为了测试去改生产代码的错误路径。

## 任务

1. 先跑基线：`cargo test --locked --all-targets 2>&1 | tail -5`，把测试数记进 `PROGRESS.md`。
2. 读 `src/commands/ops.rs` 里 `job_file_scheduler_paths_record_agent_runs_with_explicit_run_id` 全文，搞清它的 fixture 怎么搭的。
3. 读 `src/event_emit.rs` 确认三个 emit 函数的**真实事件 type 字符串**（不要猜）。
4. 加测试。
5. **反向验证（必做）**：把 `run_job_file` 里的 `emit_runner_started_jobs` 那行临时改成 `let _ = (run_id, &jobs);`，跑测试**必须变红**，贴出红的输出；然后还原，再贴绿的输出。**只贴绿的不算完成。**

## 死规矩

- 做到测试数 ≥ 基线（只许加不许减）。
- 做到反向验证红→绿两段输出都贴出来，否则无法证明测试真的守住了事件。
- 不许改 `run_job_file` 的实现来迁就测试。
- 不许用 `#[ignore]`、`|| true`、放宽断言、mock 掉被测对象。

## 界限

- **只允许改**：`src/commands/ops.rs`（只在 `#[cfg(test)]` 模块内加代码）、`PROGRESS.md`、`BLOCKED.md`。
- 其余只读。特别是 `event_emit.rs` / `events.rs` **只读不改**。
- 判卷标准（`scripts/`、`.github/`）碰都不许碰。

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

1. 三条路径的 started 事件都有测试断言，且 source label 正确。
2. 反向验证的红→绿两段实际输出都贴在对话里。
3. 测试数 ≥ 基线，收口 6 条全绿。
4. `BLOCKED.md` 随交付提交，空的也写「无」。
5. 止损：跑满 8 轮即停，如实汇报卡在哪。
