# LTO Commands

Source of truth: `src/cli.rs` `COMMANDS` plus the clap argument definitions in `src/cli.rs`.

Command count: 31.

This is the `lto-rs --help` top-level row count: 30 Rust-owned business
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
| `start` | Create a new Rust v2 run directory and current marker. Non-empty goal/done-when are hard requirements; why/host are advisory. An empty delivery contract is valid; otherwise target and instrument must be paired, while constraint/entropy-check omissions warn. `--instrument [LABEL::]CMD` accepts an optional stable label; without `::`, the entire value is the command. | `--goal`, `--done-when` | `--run-id`, `--why`, `--host`, `--target`, `--constraint`, `--instrument`, `--entropy-check`, `--force` |
| `contract` | Repair typed goal/done-when/host metadata and append delivery-contract fields without editing `state.json` by hand. `contract set` validates the merged result before writing and uses the same `[LABEL::]CMD` instrument syntax as `start`; `--replace-instrument` explicitly replaces invalid legacy instrument values. | Subcommand: `set` | `contract set --run-id --goal --done-when --host --target --constraint --instrument --replace-instrument --entropy-check` (delivery fields are repeatable; append and replace instrument modes conflict) |
| `decision` | Record, list, or reaffirm human decisions anchored to the current Git HEAD and run phase. Legacy unstructured entries remain visible and are marked for typed backfill. | Subcommand: `record`, `list`, or `reaffirm` | `record --run-id --text --scope-phase --scope-path`; `list --run-id --json`; `reaffirm --run-id --id` |
| `check` | Check a run's gates/phase evidence and ledger, or evaluate one standalone ledger with the Rust evaluator. Bare run-mode `--strict` enforces base readiness and non-empty-contract target/instrument requirements; `--to` adds phase evidence. Run-mode text and JSON include verdict plus five-dimensional diagnostics; advisory fields never gate. | None | Run mode: `--run-id`, `--strict`, `--to`, `--json`; standalone mode: `--ledger <path> [--strict]` (`--ledger` conflicts with `--run-id`, `--to`, and `--json`) |
| `closeout` | Gate closeout, including base readiness and non-empty-contract target/instrument requirements. Already-closed and unresolved-block refusals happen before any instrument; by default instruments run from the repo root with a 300s timeout per command, stop at the first failure, and report up to 8 trailing lines from each non-empty stdout/stderr stream. In `gate.evaluated`, `reverified_instruments` is the number actually attempted before success or the first failure, not the configured total. On success, update state/run-state and write handoff and changelog. | `--summary` | `--run-id`, `--next-action`, `--blocked-by`, `--allow-dirty`, `--no-changelog`, `--force`, `--reverify-timeout`, `--no-reverify` |
| `resume` | Print an active-session capsule and detect HEAD drift. | None | `--run-id` |
| `preflight` | Check environment health and, when an active or explicit run is selected, report a separate run-readiness result. `--json` changes output only; `--record` persists only the environment snapshot. An explicitly missing run is an error. | None | `--run-id`, `--json`, `--record` |
| `runner` | Run a task command or dispatch prompt/job-file work through scheduler. | Mode-dependent: `--task-id --command`, `--prompt`, `--prompt-file`, or `--job-file`; `--runner tmux --command` may dispatch without `--task-id` | `--run-id`, `--kind`, `--cwd`, `--timeout`, `--touch`, `--note`, `--status-on-fail`, `--runner`, `--allow-headless-write`, `--job-id`, `--target`, `--tmux-mode`, `--sentinel`, `--tmux-session`, `--new-window`, `--new-session`, `--window-name`, `--ready-pattern`, `--skip-prompt`, `--ready-timeout`, `--tmux-bin` |
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
| `runs` | List local `.lto` runs with `state.json`, each with phase + disk size. | None | None |
| `prune` | Reclaim disk from finished runs: remove bulk logs (events.jsonl/live/audit/dispatch) from closed runs older than N days, keeping the state index (state.json/run-state.md). Dry-run by default. | None | `--dry-run`, `--yes`, `--older-than <days>` (default 30), `--keep-last <N>`, `--run-id` |
| `memory` | Export/publish/resume redacted local run projection. | Subcommand: `export`, `publish`, or `resume` | `--run-id`, `--project`, `--am-bin`, `--timeout`, `--dry-run` |
| `plugin` | List, validate, render, statically eval, run real A/B evals, create source notes, or data-only mount plugin manifests. | Subcommand: `list`, `validate <dir>`, `render-profile <dir> <profile-id>`, `eval <dir>`, `eval-run <dir>`, `source-note <dir>`, or `mount <dir>` | `render-profile --input --output --meta-output --json`; `eval --eval-id --output --json`; `eval-run --run-id --eval-id --case --max-concurrency --no-persist --runners-dir --output --json`; `source-note --id --title --url --claim --hypothesis --append-manifest --no-append-manifest --json`; `mount --run-id`, `mount --mounts-json` |
| `dispatch-goal` | Dispatch a goal file to codex, pi, or agy through tmux and print a ready-to-copy completion wait command. | `--runner <runner> --goal <path>` | `--run-id`, `--target`, `--new-window`, `--window-name`, `--keep-window`, `--cwd`, `--tmux-session`, `--tmux-bin`, `--ready-timeout`, `--notify-cmd` (persist a host notifier template on the run; summary via `$LTO_SUMMARY`), `--no-install-hooks`, `--uninstall-hooks`, `--no-runner-constraints` (skip per-runner behavioral-constraints injection: built-in codex block and `$LTO_CONSTRAINTS_DIR`/`~/.config/lto/constraints/<runner>.md` overrides) |
| `dispatch-and-wait` | Dispatch a goal and block for `agent.dispatch.completed` (primary: agent `goal-self-report`; optional side-channels: Codex Stop/`update_goal` proof, pi/agy process-exit with real rc), then print a success/failure summary. | `--runner <runner> --goal <path>` | all dispatch-goal options + `--timeout <secs>` (default 600) |
| `agent-turn-completed` | Route a hook/process/self-report lifecycle signal as turn, session, or dispatch completion; only dispatch completion wakes goal waiters and may clean an owned window. Primary dispatch proof is `--source goal-self-report` (requires `--run-id`, uses caller rc). Codex Stop hook and `*-process-exit` remain optional side-channels. | None | `--run-id`, `--runner`, `--payload-file`, `--cwd`, `--session-id`, `--summary`, `--rc`, `--window-id`, `--source`, `--bell` (effective only for dispatch completion), `--notify-cmd` (host notifier template; trusted fields via `{run_id}`/`{runner}`/`{rc}`, untrusted summary via `$LTO_SUMMARY` env to avoid shell injection, e.g. an iaf call) |
| `events` | Block until a matching run event appears. | None | `--run-id`, `--wait`, `--event-type`, `--after`, `--timeout`, `--json` |
| `get` | List resources of a given kind (read-only). Currently only `task` is supported; other resource names error as not yet supported. Exact-match filters only. | `task` (resource) | `--run-id`, `--status`, `--phase`, `--json` |
| `describe` | Show full context for one resource object (read-only). Currently only `task` is supported; other resource names error as not yet supported. Missing id exits non-zero and lists up to 5 available ids. | `task` (resource), `<id>` | `--run-id`, `--json` |

Ledger verdicts come only from `src/ledger.rs`. `check --ledger` exits 0 for
`NO_OBSERVATIONS`, `CONVERGED`, or `CONVERGING`; 1 for `REBOUND` or strict
`STALLED`; and 2 for usage/read/parse errors. `scripts/audit_ledger_check.py`
is a one-release compatibility proxy that `exec`s this command and preserves
its output and exit code.
