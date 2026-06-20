# LTO Commands

Source of truth: `src/cli.rs` `COMMANDS` plus the clap argument definitions in `src/cli.rs`.

Command count: 25.

This is the `lto-rs --help` top-level row count: 24 Rust-owned business
commands plus clap built-in `help`. The table below lists only the
Rust-owned business commands tracked by `src/cli.rs` `COMMANDS`.

Compatibility note: `task-add`, `task-update`, `phase`, `parallel`, and
`pipeline` remain runnable as hidden legacy top-level commands for one
deprecation cycle. New scripts should use `task add`, `task update`,
`task phase`, `run parallel`, and `run pipeline`. Their Rust ownership is
tracked separately in `references/python-rust-ownership.json` because hidden
aliases do not appear in the public help table below.

| Command | Summary | Required | Optional |
|---|---|---|---|
| `start` | Create a new Rust v2 run directory and current marker. | None | `--run-id`, `--goal`, `--why`, `--done-when`, `--host`, `--target`, `--constraint`, `--instrument`, `--entropy-check`, `--force` |
| `check` | Read a run and report phase/goal, optionally as JSON. | None | `--run-id`, `--strict`, `--to`, `--json` |
| `closeout` | Gate closeout, update state/run-state, write handoff and changelog. | `--summary` | `--run-id`, `--next-action`, `--blocked-by`, `--allow-dirty`, `--no-changelog`, `--force` |
| `resume` | Print an active-session capsule and detect HEAD drift. | None | `--run-id` |
| `preflight` | Check write access, git repo status, and delegate runner health. | None | `--run-id`, `--record` |
| `runner` | Run a task command or dispatch prompt/job-file work through scheduler. | Mode-dependent: `--task-id --command`, `--prompt`, `--prompt-file`, or `--job-file`; `--runner tmux --command` may dispatch without `--task-id` | `--run-id`, `--kind`, `--cwd`, `--timeout`, `--touch`, `--note`, `--status-on-fail`, `--runner`, `--job-id`, `--target`, `--tmux-mode`, `--sentinel`, `--tmux-session`, `--new-window`, `--new-session`, `--window-name`, `--ready-pattern`, `--skip-prompt`, `--ready-timeout`, `--tmux-bin` |
| `judge` | Write a state verdict or run LLM judge mode over frozen evidence. | None for state mode; LLM mode requires `--brief --baseline-reply --candidate-reply --candidate-runner` | `--run-id`, `--task-id`, `--phase`, `--runner`, `--rerun-tests`, `--case-dir`, `--judge-runner`, `--execute` |
| `hook` | Run boundary gates. | `gate` | `--force`, `--reason` |
| `self-test` | Assert the Rust CLI command contract. | None | None |
| `run` | Run batch and staged job primitives. | Subcommand: `parallel` or `pipeline` | `run parallel --run-id --task-ids --phase --kind --command --timeout --concurrency --job-file`; `run pipeline --run-id --task-ids --phase --stages --kind --timeout --concurrency --continue-on-error --job-file` |
| `audit` | Prepare audit dispatch facts and auditor selection. | None | `--run-id`, `--auto-dispatch`, `--discover-risks`, `--allow-same-family`, `--prefer-runner` (repeatable; restricts/orders the cross-family auditor pool, e.g. keep slow `pi` off the closeout critical path) |
| `next` | Print deterministic next-step facts and route suggestion. | None | `--run-id`, `--json` |
| `autopilot` | Print supervised route facts and optionally auto-exec task commands through sandbox or tmux workers. | None | `--run-id`, `--supervised`, `--auto-exec`, `--autonomous`, `--timeout`, `--worker-runner`, `--target`, `--tmux-bin`, `--ready-timeout` |
| `recap` | Render a human recap of goal, why, progress, remaining work, tokens, live jobs, or read-only cross-run mining. | None | `--run-id`, `--artifacts`, `--mine` |
| `budget` | Check budget status. | `check --run-id` | `--tokens` |
| `release` | Print a host-owned release plan. | `--date` | `--part`, `--dry-run` |
| `task` | Add/update tasks or show/set the current run phase. | Subcommand: `add`, `update`, or `phase` | `task add --run-id --task-id --title --phase --command`; `task update --run-id --task-id --status --phase --note --touch`; `task phase --run-id --set` |
| `collect-agent-run` | Register an already-produced runner reply into `agent_runs`. | `--task-id`, `--runner`, `--reply` | `--run-id`, `--meta`, `--model`, `--status`, `--elapsed-sec`, `--note` |
| `runs` | List local `.lto` runs with `state.json`. | None | None |
| `memory` | Export/publish/resume redacted local run projection. | Subcommand: `export`, `publish`, or `resume` | `--run-id`, `--project`, `--am-bin`, `--timeout`, `--dry-run` |
| `plugin` | List, validate, render, statically eval, run real A/B evals, create source notes, or data-only mount plugin manifests. | Subcommand: `list`, `validate <dir>`, `render-profile <dir> <profile-id>`, `eval <dir>`, `eval-run <dir>`, `source-note <dir>`, or `mount <dir>` | `render-profile --input --output --meta-output --json`; `eval --eval-id --output --json`; `eval-run --run-id --eval-id --case --max-concurrency --no-persist --runners-dir --output --json`; `source-note --id --title --url --claim --hypothesis --append-manifest --no-append-manifest --json`; `mount --run-id`, `mount --mounts-json` |
| `dispatch-goal` | Dispatch a goal file to codex, pi, or agy through tmux. | `--runner <runner> --goal <path>` | `--run-id`, `--target`, `--new-window`, `--window-name`, `--cwd`, `--tmux-session`, `--tmux-bin`, `--ready-timeout`, `--no-install-hooks`, `--uninstall-hooks` |
| `agent-turn-completed` | Emit an agent turn completion event from a hook. | None | `--run-id`, `--runner`, `--payload-file`, `--cwd`, `--session-id`, `--summary`, `--rc`, `--source` |
| `events` | Block until a matching run event appears. | None | `--run-id`, `--wait`, `--event-type`, `--after`, `--timeout`, `--json` |
