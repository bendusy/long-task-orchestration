# Progress

## 开工证据

- `architecture_alignment`: 改动归属 `state` 的 instrument 解析、`closeout` 的 gate 执行、`cli` 的参数接线；复用 `run_command_capture` 与现有 `gate.evaluated` 事件，不碰 task evidence。
- `first_principles`: delivery contract 是 host 声明的验收标准；closeout 若不独立执行 instruments，判定仍由执行 agent 自签。
- `simplification_dedupe`: 抽取唯一 `split_instrument` 给完整性校验与 closeout 共用；不新增依赖、执行器或 evidence store。
- `value_measurement`: 非调优任务；基线为 442 tests，及格线为测试数不减且 goal 所列 targeted/global gates 全绿。

## 基线

命令：

```bash
cargo test --locked --all-targets 2>&1 | tail -5
```

实际输出：

```text
running 1 test
test fixed_legacy_run_fixture_is_readable_by_rust_recap_resume_and_check ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.53s
```

总测试数复核：

```text
$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
442
```

## 进度

- [x] 读 goal、active run、相关源码、CLI 文档与默认 timeout 实证。
- [x] 记录全量测试基线。
- [x] 实现共享 instrument 解析与 closeout reverify gate。
- [x] 补齐反向、正向、空 instruments、`--force`、`--no-reverify`、timeout 测试。
- [x] 更新 `COMMANDS.md`。
- [x] 跑 targeted 与全局收口，核对白名单及错误原语；六项绿，一项既有漂移阻塞。
- [x] 提交限定改动；blocked completion protocol 待最后一步执行。

## Phase 1 验证

反向验证命令：

```bash
cargo test --locked enforce_gates_rejects_failed_instrument_reverify -- --nocapture
```

闸门实际输出：

```text
closeout reverify failed: fail (rc=1)
closeout refused: delivery contract instruments failed reverify: fail (use --force to override)
test commands::closeout::tests::enforce_gates_rejects_failed_instrument_reverify ... ok
```

全绿：

```text
$ cargo test --locked closeout
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 389 filtered out

$ cargo test --locked instrument
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 395 filtered out
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 28 filtered out
```

## 全局收口

```text
$ cargo fmt --all --check
(no output, rc=0)

$ cargo check --locked --all-targets
Checking lto-rs v0.10.1 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.76s

$ cargo clippy --locked --all-targets -- -D warnings
Checking lto-rs v0.10.1 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.25s

$ cargo test --locked --all-targets
test result: ok. 407 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ python3 scripts/check_docs_consistency.py
DOCS CONSISTENCY OK

$ python3 scripts/check_python_rust_ownership.py
FAIL Rust top-level help matches ownership manifest
1 ownership failure(s)
(同结果连续三次，见 BLOCKED.md)

$ git diff --check
(no output, rc=0)
```

附加硬指标：

```text
$ cargo test --locked --all-targets -- --list 2>/dev/null | rg ': test$' | wc -l
449

$ rg -n "run_task_command" src/commands/closeout.rs
(no output, rc=1: 无匹配)
```
