# Phase 2.5 CLI command surface simplification

- status: accepted
- date: 2026-06-16
- lto_run: 20260616-015522-python-removal-gate-rust-fallback-legacy-e51661fe
- slug: phase-25-cli-command-surface

## Context

The Rust-only CLI exposes 24 business commands plus clap help. Phase 2.5 requires design before implementation because command grouping is a compatibility-sensitive UX change. The audit finding is valid: visible commands lack short help and several lifecycle operations are split across top-level names.

## Decision

Use a conservative compatibility-first grouping for v0.5.0 follow-up: add visible 'task' with subcommands 'add', 'update', and 'phase'; add visible 'run' with subcommands 'parallel' and 'pipeline'; keep legacy top-level task-add, task-update, phase, parallel, and pipeline as hidden compatibility aliases for one deprecation cycle. Do not merge semantically distinct reader-facing commands such as check, next, recap, resume, and runs. Add one-line clap about text for every visible top-level command and the new grouped subcommands. Update COMMANDS.md and the docs consistency gate to treat visible help rows as the public command surface while preserving legacy command parsing tests.

## Consequences

Top-level help drops from 24 business commands to 21 visible business commands without breaking existing scripts. The command surface becomes easier to scan and the old names remain runnable. Deeper grouping toward 12-14 commands is deferred until usage evidence shows which reader-facing commands can be merged without ambiguity.
