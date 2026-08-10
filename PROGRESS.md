# Herdr Shell Readiness Fix

- `architecture_alignment`: Shell readiness and Herdr command transport belong in `herdr_runner`; dispatch failure retention reuses `dispatch_goal::retain_dispatch_window`; finish cleanup failure follows the existing tmux retention path in `agent_turn`.
- `first_principles`: A dispatch is usable only when the created shell accepts commands in the requested repository, and every failed dispatch or cleanup must leave inspectable retained state instead of a false `active` record.
- `simplification_dedupe`: Reuse `read_pane`, `TmuxRunnerConfig::{poll_interval,ready_timeout}`, `retain_dispatch_window`, and the existing dispatch-window state fields. Add no dependency or parallel command path.
- `value_measurement`: Baseline `cargo test --locked --all-targets` on `3bf02b5` passed 519 tests (`473+36+4+3+2+1`), with 0 failed and 0 ignored. Pass line: test count stays at least 519, the required four commands pass, regression tests turn red against old behavior, and a real Herdr smoke lands Codex in this repository without a trust prompt and submits the goal through `agent prompt`.

## Verification

- Reverse check: `cargo test --locked --test herdr_backend` failed 3 of 5 tests against the old implementation, covering shell transport/readiness, dispatch retention, and finish close retention.
- Post-change: `cargo fmt --all --check` passed.
- Post-change: `cargo clippy --locked --all-targets -- -D warnings` passed.
- Post-change: `env -u LTO_REPO -u LTO_RUN_ID -u LTO_WINDOW_ID cargo test --locked --all-targets` passed 521 tests (`473+36+6+3+2+1`), with 0 failed and 0 ignored. The inherited variables were cleared because the invoking host still had an unrelated long-running `dispatch-and-wait` process; the code/test command itself was unchanged.
- Live smoke attempt 1 exposed two missing server-boundary details: the default `repo="."` reached the detached Herdr server as a relative cwd, and the default `recent` snapshot stayed empty for a shell prompt without a newline. The fix now sends an absolute cwd and polls `pane read --source visible`; a regression covers the relative-repo case.
- Live smoke attempt 2 reached the correct repository but stopped at Codex's separate `Hooks need review` prompt. The generated LTO hook was byte-identical to `scripts/hooks/codex-stop-notify.sh`; after reviewing and trusting that hook, the clean retry proceeded without a prompt.
- Live smoke final: `./target/debug/lto-rs dispatch-goal --runner codex --goal /tmp/lto-herdr-C-shell-ready-smoke.md --backend herdr --run-id 20260806-yihub-table-gap-phase2-final` returned `status=dispatched`, target `w1:p6`. `herdr pane get` and `process-info` both reported cwd `/Users/ben/Projects/lto-release/long-task-orchestration`; visible capture showed `Goal active`, the submitted goal, and `HERDR_SHELL_READY_OK /Users/ben/Projects/lto-release/long-task-orchestration`, with no trust prompt.
