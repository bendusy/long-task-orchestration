# Progress

## 开工证据

- `architecture_alignment`: 改动只归 `src/commands/closeout.rs` 的 gate 顺序与既有 reverify 循环；复用 `split_instrument`、`run_command_capture`、`gate.evaluated`，不动 state、CLI 逻辑或判卷脚本。
- `first_principles`: 已关闭或有 unresolved block 的 run 应先零副作用拒绝；真正执行 instrument 时，首败已足以否决 closeout，且 stdout/stderr 是定位失败的必要证据。
- `simplification_dedupe`: 不新增依赖或执行路径；仅加一个共享末行截取小函数，供 stdout/stderr 同用。
- `value_measurement`: 非调优任务；基线 449 tests，及格线为测试数不减、三项反向证明成立、goal 所列六项收口全绿。

## 基线

```text
$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
449

$ cargo test --locked --all-targets
test result: ok. 407 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## 进度

- [x] 读 goal、active run、相关源码、调用方、事件字段与 CLI 文档。
- [x] 复核 449 测试基线并确认全绿。
- [x] 实现纯状态早退、stdout/stderr 诊断、首败即停与事件实际尝试数。
- [x] 补 closed/unresolved 零副作用、首败不续跑、stdout/stderr 诊断测试。
- [x] 跑定向反向验证与六项全局收口。
- [x] 核对白名单；completion protocol 留作本文件停止写入后的最后一步。

## 硬指标一

```text
$ cargo test --locked enforce_gates_rejects_already_closed_run -- --nocapture
assertion: closed gate left .../closed-should-not-exist nonexistent
test commands::closeout::tests::enforce_gates_rejects_already_closed_run ... ok
test result: ok. 1 passed; 0 failed

$ cargo test --locked enforce_gates_rejects_unresolved_blocks -- --nocapture
assertion: unresolved gate left .../unresolved-should-not-exist nonexistent
test commands::closeout::tests::enforce_gates_rejects_unresolved_blocks ... ok
test result: ok. 1 passed; 0 failed

$ cargo test --locked enforce_gates_rejects_failed_instrument_reverify -- --nocapture
closeout reverify failed: fail (rc=1)
stdout tail:
stdout diagnostic
stderr tail:
stderr diagnostic
assertion: first failure left .../later-instrument-should-not-exist nonexistent
test commands::closeout::tests::enforce_gates_rejects_failed_instrument_reverify ... ok
test result: ok. 1 passed; 0 failed
```

反向验证：临时令 already-closed 条件恒假，同一测试确实变红；随即还原并复绿。

```text
closeout reverify: 1/1 instruments passed
called `Result::unwrap_err()` on an `Ok` value: ReverifyResult { attempted: 1, passed: 1, failed_labels: [] }
test commands::closeout::tests::enforce_gates_rejects_already_closed_run ... FAILED
test result: FAILED. 0 passed; 1 failed
```

## 全局收口

```text
$ cargo fmt --all --check
(no output, rc=0)

$ cargo check --locked --all-targets
Checking lto-rs v0.10.1 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.58s

$ cargo clippy --locked --all-targets -- -D warnings
Checking lto-rs v0.10.1 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.95s

$ cargo test --locked --all-targets
test result: ok. 407 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ python3 scripts/check_docs_consistency.py
DOCS CONSISTENCY OK

$ git diff --check
(no output, rc=0)

$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
449

$ git diff --name-only
BLOCKED.md
COMMANDS.md
PROGRESS.md
src/commands/closeout.rs
```

## 2026-07-29：runner 生命周期事件测试补齐

### 基线

```text
$ cargo test --locked --all-targets 2>&1 | tail -5
running 1 test
test fixed_legacy_run_fixture_is_readable_by_rust_recap_resume_and_check ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
449
```

### 反向验证

临时以 `let _ = (run_id, &jobs);` 替换 `emit_runner_started_jobs(...)` 后：

```text
assertion `left == right` failed
  left: []
 right: ["run.parallel", "run.pipeline", "runner.job_file"]
test commands::ops::tests::job_file_scheduler_paths_record_agent_runs_with_explicit_run_id ... FAILED
test result: FAILED. 0 passed; 1 failed
```

复原生产代码后：

```text
test commands::ops::tests::job_file_scheduler_paths_record_agent_runs_with_explicit_run_id ... ok
test result: ok. 1 passed; 0 failed
```

### 进度

- [x] 三条 job-file 路径均断言 `runner.started` 与正确 `context`。
- [x] 非法 runner 经真实 `submit_jobs` 失败，断言 submission-failed 事件与 `context`。
- [x] 成功结果仍由既有三项 `agent_runs` 断言覆盖。
- [x] started emit 反向验证红后复绿。

## 2026-07-29：精简第 4 批

### 开工证据与基线

- `architecture_alignment`: shell 转义复用 `src/process.rs` 唯一安全实现；clone 与 blocker 判定留在原调用点作局部删减；`DependencyPlan` 仍属 scheduler 内部类型，不改 CLI、state、gate 或文件协议。
- `first_principles`: 去掉同字节安全实现的重复定义、无消费语义的 clone、只为判空而复制的 JSON，及无失败路径的 `Result`；行为与契约不变。
- `simplification_dedupe`: 不增 helper、抽象、依赖或文件；只复用现有 `shell_single_quote`，其余皆直接删减。
- `value_measurement`: 基线为 408 lib + 42 集成（共 450）全绿；`redundant_clone` 基线为生产 6 告警、测试另 1 告警；及格线为测试数不减、生产告警下降、goal 六条收口全绿。

```text
$ cargo test --locked --all-targets
test result: ok. 408 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo clippy --locked --all-targets -- -W clippy::redundant_clone
warning: `lto-rs` (lib) generated 6 warnings
warning: `lto-rs` (lib test) generated 7 warnings (6 duplicates)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.65s

$ rg -n 'shell_quote' src/commands/ops.rs
3239,3240,3241,3242,3243,3245: 6 call sites
3298: 1 local definition
```

### 进度

- [x] 归一 `shell_single_quote` 并跑定向编译、测试。
- [x] 删除六处生产冗余 clone 与一处测试 clone，并跑定向编译、测试。
- [x] blocker 判定改 `any`，并跑定向编译、测试。
- [x] `DependencyPlan::new` 直接返回 `Self`，并跑定向编译、测试。
- [x] 六条全局收口与测试计数。
- [x] 白名单核对；实现已入 `701df6a`，收口证据另作 scoped commit。

### 第 1 项实跑

```text
$ cargo check --locked --all-targets
Checking lto-rs v0.11.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.94s

$ cargo test --locked commands::ops::tests::tmux_worker_prompt_preserves_quoted_command_contract -- --nocapture
test commands::ops::tests::tmux_worker_prompt_preserves_quoted_command_contract ... ok
test result: ok. 1 passed; 0 failed; 407 filtered out

$ find src -name '*.rs' -exec grep -Hn "fn shell_quote\|fn shell_single_quote" {} \;
src/process.rs:37:pub fn shell_single_quote(value: &str) -> String {
```

### 第 2 项实跑

```text
$ cargo check --locked --all-targets
Checking lto-rs v0.11.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.67s

$ cargo test --locked agent_turn::tests::goal_self_report_rc0_marks_dispatch_completed -- --nocapture
test agent_turn::tests::goal_self_report_rc0_marks_dispatch_completed ... ok
test result: ok. 1 passed; 0 failed

$ cargo test --locked commands::closeout::tests -- --nocapture
test result: ok. 17 passed; 0 failed

$ cargo test --locked commands::ops::tests::collect_agent_run_emits_runner_and_model_fields -- --nocapture
test commands::ops::tests::collect_agent_run_emits_runner_and_model_fields ... ok
test result: ok. 1 passed; 0 failed

$ cargo test --locked commands::ops::tests::cmd_runner_job_file_requires_headless_write_override -- --nocapture
test commands::ops::tests::cmd_runner_job_file_requires_headless_write_override ... ok
test result: ok. 1 passed; 0 failed

$ cargo clippy --locked --all-targets -- -W clippy::redundant_clone
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.80s
(no warnings, rc=0)
```

### 第 3 项实跑与反向验证

```text
$ cargo check --locked --all-targets
Checking lto-rs v0.11.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.36s

$ cargo test --locked commands::ops::tests::build_state_verdict_fails_when_any_task_has_blockers -- --nocapture
test commands::ops::tests::build_state_verdict_fails_when_any_task_has_blockers ... ok
test result: ok. 1 passed; 0 failed
```

### 全局收口

```text
$ cargo fmt --all --check
(no output, rc=0)

$ cargo clippy --locked --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s

$ cargo test --locked --all-targets
test result: ok. 409 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ python3 scripts/check_docs_consistency.py
DOCS CONSISTENCY OK

$ python3 scripts/check_python_rust_ownership.py
RUST OWNERSHIP OK

$ git diff --check
(no output, rc=0)

$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
451

$ find src -name '*.rs' -exec grep -Hn "fn shell_quote\|fn shell_single_quote" {} \;
src/process.rs:37:pub fn shell_single_quote(value: &str) -> String {
```

全套首跑曾有两条无关 tmux 稳定输出超时；各自单跑皆复绿，原命令第二跑全绿，未改 `tmux_runner.rs`。

```text
test tmux_runner::tests::send_text_pastes_large_payload_before_enter ... FAILED
test tmux_runner::tests::signal_mode_appends_and_waits_for_tmux_signal ... FAILED
test result: FAILED. 407 passed; 2 failed

$ cargo test --locked tmux_runner::tests::send_text_pastes_large_payload_before_enter -- --nocapture
test tmux_runner::tests::send_text_pastes_large_payload_before_enter ... ok

$ cargo test --locked tmux_runner::tests::signal_mode_appends_and_waits_for_tmux_signal -- --nocapture
test tmux_runner::tests::signal_mode_appends_and_waits_for_tmux_signal ... ok
```

### 第 4 项实跑

```text
$ cargo check --locked --all-targets
Checking lto-rs v0.11.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.60s

$ cargo test --locked scheduler::tests::dependency_child_waits_for_host_merge_but_independent_job_runs -- --nocapture
test scheduler::tests::dependency_child_waits_for_host_merge_but_independent_job_runs ... ok
test result: ok. 1 passed; 0 failed

$ cargo test --locked scheduler::tests::submit_respects_concurrency_cap_and_order -- --nocapture
test scheduler::tests::submit_respects_concurrency_cap_and_order ... ok
test result: ok. 1 passed; 0 failed
```

反向验证：临时将 `!blockers.is_empty()` 反置为 `blockers.is_empty()`；初版样例含空数组，误触错误谓词而未红，遂改为“一个无字段、一个非空 blocker”。第二次反置确实变红；还原后复绿。

```text
verdict: pass
reason: still blocked
test commands::ops::tests::build_state_verdict_fails_when_any_task_has_blockers ... FAILED
test result: FAILED. 0 passed; 1 failed

$ cargo test --locked commands::ops::tests::build_state_verdict_fails_when_any_task_has_blockers -- --nocapture
test commands::ops::tests::build_state_verdict_fails_when_any_task_has_blockers ... ok
test result: ok. 1 passed; 0 failed
```
