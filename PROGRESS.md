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
