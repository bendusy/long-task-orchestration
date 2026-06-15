# Agent Instructions

## Scope

This repository is `long-task-orchestration`, the standalone LTO harness. LTO is a harness layer for long-running agent work: state, artifacts, runner dispatch, audit, resume/recap, gates, and release discipline. The host agent remains the planner.

Default response language for this repo is Chinese unless the user asks otherwise. Code, commands, API names, and logs stay in their original language.

## Current Direction

- Rust v2 is now the default local wrapper path for the old Python CLI. Keep Python as an explicit legacy fallback for parity checks and rollback.
- Keep macOS and Linux healthy first. Windows native support is paused while the built-in runner protocol depends on `scripts/delegate/runners/*.sh` and `healthcheck.sh`; WSL or Unix-like shells are separate user-side validation.
- Do not claim GitHub has downloadable Rust binaries without checking live releases. The workflow can build assets on future `v*` tag pushes, but existing tags/releases must be verified before stating availability.
- Next engineering priority is Rust takeover plus code cleanup, not expanding platform scope.
- `/goal` belongs in Rust core only as a delivery contract: targets, constraints, instruments, and forced entropy recorded in `.lto/<run-id>/state.json` and checked by phase gates. Do not implement it as a stateful background loop.

## LTO First

For multi-step work in this repo, use LTO state before broad edits:

```bash
cargo run --quiet -- runs
cargo run --quiet -- resume --run-id <run-id>
cargo run --quiet -- check --run-id <run-id>
```

If using the installed wrapper, Rust is the default. Choose Python only for explicit legacy fallback checks:

```bash
lto <command>
lto --use-python <command>
```

Do not close the Rust v2 main run until the PR branch is merged and the release/migration evidence is recorded.

## Development Gate

Before implementation or tuning, write these four evidence items into run-state, task evidence, or an equivalent artifact:

- `architecture_alignment`: where the change belongs, which boundaries it must respect, and which existing patterns it reuses.
- `first_principles`: the real constraint, user value, or root cause that justifies the work.
- `simplification_dedupe`: what can be deleted, merged, or reused before adding another path.
- `value_measurement`: for tuning, the baseline, metric, pass line, verification command, and post-change result.

No-baseline tuning is only a hypothesis. It is not completion evidence.

For `/goal`-style long deliveries, start with a delivery contract:

```bash
lto start --goal "..." \
  --target "..." \
  --constraint "..." \
  --instrument "..." \
  --entropy-check "..."
```

## Closeout Gate

Before closeout, release, or handoff, record these evidence items:

- `documentation_alignment`: docs that were checked or updated, including `SKILL.md`, `README.md`, `INSTALL.md`, `AGENTS.md`, `CLAUDE.md`, and relevant `references/`.
- `historical_cleanup`: stale paths, old instructions, obsolete LTO runs, compatibility notes, or historical artifacts that were removed, archived, or explicitly left as history.
- `clean_worktree`: `git status --short` is clean before packaging, or every remaining dirty path is intentional and named.
- `rebuild_package`: rebuild/repackage command and result after the repository is clean, such as `cargo build --release --locked --bin lto-rs`.

Do not treat a task as finished if docs still describe a different architecture, old history is still masquerading as current guidance, the repo has unexplained dirt, or release/build artifacts have not been regenerated from the final state.

## Rust Migration And Release

When touching Rust takeover, installer, release, or docs:

- Explain the Python-to-Rust switch path: source build with `cargo`, installed wrapper defaulting to Rust, and explicit fallback with `--use-python` / `LTO_USE_PYTHON=1`.
- State whether users can download a binary only after checking GitHub Releases and release assets.
- Keep release flow explicit: `lto release --dry-run`, verification, VERSION/CHANGELOG update, tag push, then CI `release-binaries` uploads macOS/Linux assets.
- Do not add Windows release targets until runner/healthcheck support is designed and tested natively.

## Verification

Prefer the repo's current Rust gates for Rust-path changes:

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo test --locked --all-targets
python3 scripts/smoke_test.py
git diff --check
cargo build --release --locked --bin lto-rs
```

If a command cannot run, record why and what evidence remains missing. Do not edit tests to hide an implementation or documentation contract mismatch.
