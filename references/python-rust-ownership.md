# Rust Ownership After Python Retirement

> Status: ownership map after the v0.5.0 Python fallback retirement. Machine-readable source:
> [`python-rust-ownership.json`](./python-rust-ownership.json). Verification:
> `python3 scripts/check_python_rust_ownership.py`.

## Rule

Rust owns the public LTO core. The Python fallback package and legacy entrypoint
were removed during the v0.5.0 retirement work. Historical `.lto` run-state
compatibility is preserved by Rust fixture tests, not by keeping a Python entrypoint.

Do not reintroduce a second command implementation. If a new public command is
added, add it to this manifest and expose it through the Rust CLI.

## Visible Top-Level Commands

All 21 visible top-level business commands are Rust-owned. `lto-rs --help`
additionally shows clap's built-in `help` pseudo-command; it is not listed here.

| Command | Owner | Python Role |
|---|---|---|
| `start` | Rust core | removed |
| `check` | Rust core | removed |
| `closeout` | Rust core | removed |
| `resume` | Rust core | removed |
| `preflight` | Rust core | removed |
| `runner` | Rust core | removed |
| `judge` | Rust core | removed |
| `hook` | Rust core | removed |
| `self-test` | Rust core | removed |
| `audit` | Rust core | removed |
| `next` | Rust core | removed |
| `autopilot` | Rust core | removed |
| `recap` | Rust core | removed |
| `budget` | Rust core | removed |
| `release` | Rust core | removed |
| `task` | Rust core | removed |
| `run` | Rust core | removed |
| `collect-agent-run` | Rust core | removed |
| `runs` | Rust core | removed |
| `memory` | Rust core | removed |
| `plugin` | Rust core | removed |

## Hidden Compatibility Commands

These aliases are still parsed by Rust for one deprecation cycle, but they are
hidden from `lto-rs --help` and must not be used by new scripts. They are listed
separately so the ownership gate can verify that compatibility does not become
a second implementation path.

| Hidden Command | Replacement | Owner | Python Role |
|---|---|---|---|
| `task-add` | `task add` | Rust core | removed |
| `task-update` | `task update` | Rust core | removed |
| `phase` | `task phase` | Rust core | removed |
| `parallel` | `run parallel` | Rust core | removed |
| `pipeline` | `run pipeline` | Rust core | removed |

## Preserved Python Helpers

`scripts/write_decision.py` is intentionally preserved as a standalone
repository helper for ADR creation and artifact registration. It is not a CLI
fallback, does not route `lto` commands, and must not import the retired
`scripts/lto/` package.

## Plugin Subcommands

| Command | Owner | Python Role | Removal/Port Rule |
|---|---|---|---|
| `plugin list` | Rust core | removed | Rust owns the command. |
| `plugin validate` | Rust core | removed | Rust owns the command. |
| `plugin render-profile` | Rust core | removed | Rust owns the command. |
| `plugin eval` | Rust core | removed | Rust owns the command. |
| `plugin mount` | Rust core | removed | Rust owns the command. |
| `plugin source-note` | Rust core | removed | Rust owns source-note creation; parity evidence is recorded in `validation-log.md`. |
| `plugin eval-run` | Rust core | removed | Rust owns baseline-vs-candidate eval-run; parity evidence is recorded in `validation-log.md`. |

## Removal Record

The Python fallback was removed only after the staged transfer completed:

1. Classify each Python surface in the manifest as `rust-core`,
   `compatibility-fallback`, `python-legacy`, or `removal-candidate`.
2. Port the externally visible behavior to Rust and add focused Rust tests for
   success, failure, path safety, and old-run compatibility where applicable.
3. Prove parity against the legacy Python behavior with the same fixture or
   record an explicit retirement decision for behavior that will not be kept.
4. Move the manifest owner to `rust-core` only after the Rust command is exposed
   in help and the ownership gate passes.
5. Remove the Python fallback only after the staged removal gate records
   downstream wrapper, docs, and compatibility evidence.
6. Delete Python only after the wrapper no longer routes to it, active docs no
   longer teach it, tests/gates no longer import it, and rollback is preserved
   by fixtures or release notes.
7. Do not delete `scripts/delegate/runners/*.sh`; those are Rust-owned runner
   adapters, not Python fallback code.

## Gate

`scripts/check_python_rust_ownership.py` fails if:

- Rust help exposes a top-level command missing from the ownership manifest.
- A hidden compatibility command stops parsing through Rust.
- Rust plugin help exposes a subcommand not marked `rust-core`.
- Any manifest entry still claims an active Python role.
- This Markdown document stops naming a manifest entry.

This is deliberately stricter than a prose review. If a new command appears,
it must declare ownership before release.
