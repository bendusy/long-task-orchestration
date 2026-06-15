# LTO Commands

Source of truth: `src/cli.rs` `COMMANDS` plus the clap argument definitions in `src/cli.rs`.

Command count: 24.

| Command | Summary | Required | Optional |
|---|---|---|---|
| `start` | Create a new Rust v2 run directory and current marker. | None | `--run-id`, `--goal`, `--why`, `--done-when`, `--host`, `--target`, `--constraint`, `--instrument`, `--entropy-check`, `--force` |
| `check` | Read a run and report phase/goal, optionally as JSON. | None | `--run-id`, `--strict`, `--to`, `--json` |
| `closeout` | Gate closeout, update state/run-state, write handoff and changelog. | `--summary` | `--run-id`, `--next-action`, `--blocked-by`, `--allow-dirty`, `--no-changelog`, `--force` |
| `resume` | Print an active-session capsule and detect HEAD drift. | None | `--run-id` |
| `preflight` | Check write access, git repo status, and delegate runner health. | None | `--run-id`, `--record` |
| `runner` | Run a task command or dispatch prompt/job-file work through scheduler. | Mode-dependent: `--task-id --command`, `--prompt`, `--prompt-file`, or `--job-file` | `--run-id`, `--kind`, `--cwd`, `--timeout`, `--touch`, `--note`, `--status-on-fail`, `--runner`, `--job-id` |
| `judge` | Write a state verdict or run LLM judge mode over frozen evidence. | None for state mode; LLM mode requires `--brief --baseline-reply --candidate-reply --candidate-runner` | `--run-id`, `--task-id`, `--phase`, `--runner`, `--rerun-tests`, `--case-dir`, `--judge-runner`, `--execute` |
| `hook` | Run boundary gates. | `gate` | `--force`, `--reason` |
| `self-test` | Assert the Rust CLI command contract. | None | None |
| `parallel` | Run multiple task commands in a batch, or submit a job file. | None | `--run-id`, `--task-ids`, `--phase`, `--kind`, `--command`, `--timeout`, `--concurrency`, `--job-file` |
| `pipeline` | Run staged commands for selected tasks, or submit a job file. | `--stages` for command mode | `--run-id`, `--task-ids`, `--phase`, `--kind`, `--timeout`, `--concurrency`, `--continue-on-error`, `--job-file` |
| `audit` | Prepare audit dispatch facts and auditor selection. | None | `--run-id`, `--auto-dispatch`, `--discover-risks`, `--allow-same-family` |
| `next` | Print deterministic next-step facts and route suggestion. | None | `--run-id`, `--json` |
| `autopilot` | Print supervised route facts and optionally auto-exec safe task commands. | None | `--run-id`, `--supervised`, `--auto-exec`, `--autonomous`, `--timeout` |
| `recap` | Render a human recap of goal, why, progress, remaining work, tokens, and live jobs. | None | `--run-id`, `--artifacts` |
| `budget` | Check budget status. | `check --run-id` | `--tokens` |
| `release` | Print a host-owned release plan. | `--date` | `--part`, `--dry-run` |
| `task-add` | Add a pending task to the active run. | `--task-id`, `--title` | `--run-id`, `--phase`, `--command` |
| `task-update` | Update task status, phase, notes, or touched files. | `--task-id` plus at least one change flag | `--run-id`, `--status`, `--phase`, `--note`, `--touch` |
| `phase` | Print or change the current run phase. | None | `--run-id`, `--set` |
| `collect-agent-run` | Register an already-produced runner reply into `agent_runs`. | `--task-id`, `--runner`, `--reply` | `--run-id`, `--meta`, `--model`, `--status`, `--elapsed-sec`, `--note` |
| `runs` | List local `.lto` runs with `state.json`. | None | None |
| `memory` | Export/publish/resume redacted local run projection. | Subcommand: `export`, `publish`, or `resume` | `--run-id`, `--project`, `--am-bin`, `--timeout`, `--dry-run` |
| `plugin` | List, validate, render, statically eval, or data-only mount plugin manifests. | Subcommand: `list`, `validate <dir>`, `render-profile <dir> <profile-id>`, `eval <dir>`, or `mount <dir>` | `render-profile --input --output --meta-output --json`; `eval --eval-id --output --json`; `mount --run-id`, `mount --mounts-json` |
