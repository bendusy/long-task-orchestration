# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, test, and run

This crate is `lto-rs` (binary + library), Rust edition 2024. There is no Python runtime — `scripts/*.py` are maintenance/CI checks only.

```bash
# Run the CLI from source (preferred during development)
cargo run --quiet -- <command>          # e.g. cargo run --quiet -- runs
cargo run --quiet -- self-test          # built-in CLI contract self-test

# Build
cargo build --release --locked --bin lto-rs

# Full local gate — must pass before claiming work done (mirrors CI rust-v2.yml)
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/check_docs_consistency.py        # docs vs CLI surface consistency
python3 scripts/check_python_rust_ownership.py   # enforces no Python runtime fallback
git diff --check

# Run a single test (227+ inline #[test]/#[tokio::test] across src/, plus tests/python_rust_compat.rs)
cargo test --locked <test_name>                  # by name substring
cargo test --locked --test python_rust_compat    # one integration test file
cargo test --locked <module>::                   # all tests in a module
```

`clippy -D warnings` is enforced — warnings fail CI. `unsafe_code = "forbid"` (see Cargo.toml). When changing logs/telemetry/artifacts/plugins/delegation, also run `scripts/privacy_self_check.sh` before pushing.

## Architecture

LTO (Long Task Orchestration) is a **control harness for long AI-agent work** — not a planner. The host agent plans; LTO provides state, evidence, gates, bounded actuators, and run logs. See the design principles below; this section maps the code.

### Control loop

```text
state → observe (events) → derive signal (telemetry) → propose → gate → execute → record outcome
```

Everything persists under `.lto/<run-id>/` — this file protocol is the product boundary (principle 7). The active run id lives in `.lto/current`.

### Persistence layout (`.lto/<run-id>/`)

| File | Role |
|------|------|
| `state.json` | Run state: phase, tasks, budget, workspace/env snapshot, delivery contract. Path constant in `state.rs`. |
| `events.jsonl` (+ `.counter`) | Append-only event log; warn@10K, hard-stop@50K events. `events.rs`. |
| `telemetry.json` | Derived signal snapshots only (run/runner/audit/task/budget metrics). `telemetry.rs`. |
| `artifacts.json` | Artifact manifest (kind/producer/path/tags). |
| `run-state.md` | Human-readable state; `closeout` appends a closeout section. |
| `plugin-mounts.json` | Approved plugin ids + sandbox grants. |
| `worktrees/<run-id>/<task-id>/` | Persistent git worktrees for isolated task edits. |

Legacy `.lto` run fixtures live in `tests/fixtures/legacy-run/` and back the v0.5.0 Python-removal parity test.

### Module map (`src/`)

The single binary entry is `main.rs` → `cli.rs` (2400+ lines: clap `Commands` enum + dispatch). Library re-exports in `lib.rs`.

**State & run lifecycle**: `state.rs` (run-state serde + delivery contract), `commands/resume.rs` (HEAD-drift detection → re-verify tasks), `commands/recap.rs` (human progress recap), `commands/closeout.rs` (closeout gate → phase "closed"), `commands/ops.rs` (preflight/check/runner/judge/next), `commands/util.rs` (load/save context, git status, token tallies).

**Dispatch & execution**: `scheduler.rs` (concurrent job scheduler: submit/retry/healthcheck/deps), `dispatch.rs` (capability-scored runner selection), `dispatch_goal.rs` (`dispatch-goal` → codex/pi/agy via tmux + hook install), `agent_job.rs` (`AgentJob`/`AgentResult`/`Sandbox`/`PermissionPolicy`), `agent_turn.rs` (`agent.turn.completed` hook routing), `process.rs` (git/shell command factory), `worktree.rs` (worktree lifecycle), `tmux_runner.rs` (tmux-driven runner; Signal/Sentinel/Fire completion modes).

**Evaluation as evidence (sensors are fallible — principle 5)**: `audit.rs` + `audit_dispatch.rs` (heterogeneous cross-family audit selection, fail-closed on unhealthy runners), `llm_judge.rs` (evidence-frozen blocking judgment: strong/adequate/weak/none), `decision.rs` (multi-runner voting / supermajority), `merge_review.rs` (diff + tests + sensitive-file detection).

**Bounding & observability**: `budget.rs` (token/round/deadline limits — principle 4), `events.rs` + `event_emit.rs` (event log + emit API), `telemetry.rs` (derived snapshots), `effect.rs` (regex command-effect classification: Reversible/Network/NeedsSemanticJudgement), `redact.rs` (**single source of truth** for secret/private-path regexes — ingress redaction before any write).

**Plugins (data-only boundary)**: `plugin.rs` (`PluginManifest`/`PluginSecurity` validation, discovery in `plugins/` and `.lto/plugins/`), `plugin_eval_run.rs` (eval-pack runs as sub-LTO-runs; leak detection).

### Runner delegation

The built-in runner protocol shells out to `scripts/delegate/runners/*.sh` (codex/claude/agy/pi/gemini) gated by `healthcheck.sh`. This is why Windows native support is paused (macOS/Linux first). `runner --kind` defaults to `test`; `--runner` defaults to `codex`; `--timeout` defaults to 300s.

### CLI surface

`cargo run -- --help` is authoritative; `COMMANDS.md` documents the full surface. Key commands: `start` (open a run / write delivery contract), `task`/`phase`, `runner` (execute + record evidence), `check`, `next`/`resume`/`recap` (deterministic context briefs), `audit --auto-dispatch`/`--discover-risks`, `judge`, `closeout`, `release` (host-owned), `autopilot --supervised`. Note: historical `audit --collect` is **not** a current command — use `runner`/`collect-agent-run`/artifacts.

### Reference docs

`references/` holds the design specs. Read the relevant one before changing an area — `control-loop-harness.md`, `plugin-boundary.md`, `protocol-and-language-strategy.md`, `run-state-workflow.md`, `runner-readonly-contract.md`, `privacy-self-check.md`. `AGENTS.md` defines the development gate (architecture_alignment / first_principles / simplification_dedupe / value_measurement) and closeout gate that must be recorded into run-state.

## Project identity

LTO is a **control harness for long AI-agent work**.

It is not:

- an agent UI;
- a PM/coordinator replacement;
- a workflow marketplace;
- an auto-routing system;
- a memory system that treats old notes as truth.

The host agent remains the planner. LTO provides state, evidence, feedback, gates, actuator limits, and run logs.

## Core control-loop principles

### 1. Host remains controller-in-chief

LTO may observe, summarize, constrain, and propose. It must not silently decide, route, promote, deploy, push, or erase.

`next` and `autopilot --supervised` produce control briefs. The host/human decides.

### 1a. Delivery contract is core

`/goal`-style work belongs in Rust core as a delivery contract, not as a plugin-only note and not as a daemon. The core state may persist targets, constraints, instruments, and forced-entropy checks so phase gates can tell whether the long run has an executable delivery target.

### 2. Observability before control

Do not automate a behavior until LTO can measure it.

Preferred order:

```text
observe -> log -> derive signal -> propose action -> gate -> execute -> record outcome
```

### 3. Negative feedback first

LTO should first correct drift, surface blockers, cap cost, and stop unsafe actions.

Avoid positive-feedback loops such as:

- “runner was fast once, route more work to it”;
- “article sounds right, promote it to core”;
- “audit found issues, keep spawning audits until empty”.

### 4. Every actuator is bounded

Any command that executes work must have explicit limits:

- timeout;
- cwd / worktree boundary;
- permission snapshot;
- max concurrency;
- max rounds;
- max budget where applicable;
- human gate for irreversible actions.

### 5. Sensors are fallible

`judge`, `audit`, schema parsers, privacy scans, and external articles are evidence, not truth.

Separate:

- deterministic metrics;
- oracle-assisted metrics;
- judged metrics.

Never treat judged metrics as ground truth.

### 6. Work paving beats UI

Do not build a Cockpit UI. Strengthen CLI/file primitives that help the host pave work:

- critical path;
- WIP count;
- blocker age;
- issue lifecycle;
- budget burn;
- fan-out barriers;
- worker observations;
- closeout gates.

### 7. Protocol during Rust takeover

The `.lto/<run-id>/` file protocol is the product boundary. Do not rewrite LTO in Go/Rust/TS before protocol contracts, redaction rules, and conformance fixtures are stable.

Current posture:

- Rust v2 is the only supported local wrapper path.
- Python fallback was removed in v0.5.0 after parity evidence was recorded; rollback uses git history and legacy `.lto` fixtures, not a second live CLI.
- Go is not a near-term core path.
- TypeScript is for wrappers/MCP/editor integration, not core control logic.

### 8. Run logs are tuning fuel

LTO preserves structured run telemetry for future tuning. The Phase 1 sensor layer is implemented (2026-06-09); deeper signals remain future work:

- append-only `events.jsonl` run log (8 Phase 1 event types) — **implemented**;
- derived `telemetry.json` snapshots — **implemented**;
- worker observations — future;
- finding/issue/decision provenance — future (deferred event types);
- budget/time/quality signals — partial (budget + event-log counters in telemetry).

When implemented, logs must pass ingress redaction before write. They reference artifacts, not inline raw transcripts, secrets, private source contents, or absolute private paths. `telemetry.json` persists derived signals only; route-like recommendations belong in ephemeral `next` briefs, not persistent telemetry.

### 9. External viewpoints enter as hypotheses

Articles, tweets, framework claims, and model-specific advice enter via:

```text
source note -> unverified claim -> falsifiable hypothesis -> eval evidence -> promote/reject
```

No direct core changes from article authority. A generic primitive can still be promoted into core after an explicit user/maintainer decision; `delivery_contract` is one such core primitive.

### 10. Typed shared workspace, not PM platform

Absorb Cockpit-like ideas as typed workspace primitives, not as a coordinator daemon. Current implemented primitives are `Task`, `AgentJob`, `AgentResult`, and `Artifact`; the following are future typed workspace targets, not current state fields or CLI commands yet:

- Finding;
- Issue;
- ResearchNote;
- Claim;
- Hypothesis;
- Decision;
- WorkerObservation;
- Barrier.

Workers may produce proposed findings/reports/research artifacts today as normal artifacts. Future LTO gates may decide what becomes accepted issue/decision/state.

### 11. Privacy is part of control

Telemetry and logs must be redaction-aware. Never commit `.lto/`, transcripts, feedback bundles, secrets, or private local paths.

Run privacy checks before push when changing logs, telemetry, artifacts, plugins, or delegation code.

## Current design references

Read these before changing related areas:

- `references/control-loop-harness.md` — control-loop harness spec and run telemetry design.
- `references/plugin-boundary.md` — data-only plugin boundary.
- `references/plugin-real-eval-runner.md` — real eval as sub-LTO-run compiler.
- `references/privacy-self-check.md` — privacy scan and confirmed cleanup.
- `references/protocol-and-language-strategy.md` — protocol-first learning loop and language roadmap.
- `references/workflow-playbook.md` — host-agent playbook philosophy.

## Implementation posture

When implementing new LTO capabilities:

1. Prefer passive logging before behavior changes.
2. Add schema/tests before automation.
3. Keep commands resumable and auditable.
4. Do not add UI/server/global daemon unless explicitly approved later.
5. Do not let historical telemetry auto-route workers; surface it as advisory evidence.
6. Preserve backwards compatibility for existing `.lto/<run-id>/state.json` runs.
