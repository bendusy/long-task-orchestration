# Python And Rust Ownership

> Status: ownership map for the Rust takeover. Machine-readable source:
> [`python-rust-ownership.json`](./python-rust-ownership.json). Verification:
> `python3 scripts/check_python_rust_ownership.py`.

## Rule

Rust owns the public LTO core. Python remains only as explicit compatibility
fallback or as named legacy plugin support.

Do not remove Python code by file age or by gut feel. Remove it only after the
surface is classified, the Rust path proves equivalent behavior, downstream
fallback is no longer needed, and the removal gate is recorded.

## Top-Level Commands

All 24 top-level public commands are Rust-owned. Python mirrors them as
`--use-python` / `LTO_USE_PYTHON=1` fallback while downstream integrations and
old-run compatibility are still being verified.

| Command | Owner | Python Role |
|---|---|---|
| `start` | Rust core | compatibility fallback |
| `check` | Rust core | compatibility fallback |
| `closeout` | Rust core | compatibility fallback |
| `resume` | Rust core | compatibility fallback |
| `preflight` | Rust core | compatibility fallback |
| `runner` | Rust core | compatibility fallback |
| `judge` | Rust core | compatibility fallback |
| `hook` | Rust core | compatibility fallback |
| `self-test` | Rust core | compatibility fallback |
| `parallel` | Rust core | compatibility fallback |
| `pipeline` | Rust core | compatibility fallback |
| `audit` | Rust core | compatibility fallback |
| `next` | Rust core | compatibility fallback |
| `autopilot` | Rust core | compatibility fallback |
| `recap` | Rust core | compatibility fallback |
| `budget` | Rust core | compatibility fallback |
| `release` | Rust core | compatibility fallback |
| `task-add` | Rust core | compatibility fallback |
| `task-update` | Rust core | compatibility fallback |
| `phase` | Rust core | compatibility fallback |
| `collect-agent-run` | Rust core | compatibility fallback |
| `runs` | Rust core | compatibility fallback |
| `memory` | Rust core | compatibility fallback |
| `plugin` | Rust core | compatibility fallback |

## Plugin Subcommands

| Command | Owner | Python Role | Removal/Port Rule |
|---|---|---|---|
| `plugin list` | Rust core | compatibility fallback | Python mirror can shrink after wrapper and plugin docs no longer depend on it. |
| `plugin validate` | Rust core | compatibility fallback | Python mirror can shrink after all bundled plugins validate through Rust in CI. |
| `plugin render-profile` | Rust core | compatibility fallback | Python mirror can shrink after profile rendering parity fixtures exist. |
| `plugin eval` | Rust core | compatibility fallback | Python mirror can shrink after static eval fixtures are Rust-owned. |
| `plugin mount` | Rust core | compatibility fallback | Python mirror can shrink after mount-lock compatibility fixtures exist. |
| `plugin source-note` | Rust core | compatibility fallback | Rust now owns source-note creation; Python mirror can shrink during the formal removal gate after parity evidence is recorded. |
| `plugin eval-run` | Rust core | compatibility fallback | Rust now owns baseline-vs-candidate eval-run; Python mirror can shrink only during the formal removal gate after B.5 parity evidence and human confirmation. |

## Safe Python Removal And Rust Takeover

Safe deletion is a staged ownership transfer, not a file cleanup pass:

1. Classify each Python surface in the manifest as `rust-core`,
   `compatibility-fallback`, `python-legacy`, or `removal-candidate`.
2. Port the externally visible behavior to Rust and add focused Rust tests for
   success, failure, path safety, and old-run compatibility where applicable.
3. Prove parity against the legacy Python behavior with the same fixture or
   record an explicit retirement decision for behavior that will not be kept.
4. Move the manifest owner to `rust-core` only after the Rust command is exposed
   in help and the ownership gate passes.
5. Keep the Python fallback callable until the staged removal gate records
   downstream wrapper, docs, and compatibility evidence.
6. Delete Python only after the wrapper no longer routes to it, active docs no
   longer teach it, tests/gates no longer import it, and rollback is preserved
   by fixtures or release notes.
7. Do not delete `scripts/delegate/runners/*.sh`; those are Rust-owned runner
   adapters, not Python fallback code.

## Gate

`scripts/check_python_rust_ownership.py` fails if:

- Rust help exposes a top-level command missing from the ownership manifest.
- Python fallback exposes a top-level command missing from the ownership
  manifest.
- Rust plugin help exposes a subcommand not marked `rust-core`.
- Python plugin help exposes a subcommand missing from the plugin ownership
  manifest.
- This Markdown document stops naming a manifest entry.

This is deliberately stricter than a prose review. If a new command appears,
it must declare ownership before release.
