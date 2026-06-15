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
| `plugin eval-run` | Python legacy | legacy plugin | Port or retire separately; do not hide it behind the default Rust path until real model A/B evidence is Rust-owned. |
| `plugin source-note` | Python legacy | legacy plugin | Port only if plugin authoring becomes an active Rust requirement; otherwise keep explicit legacy helper. |

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
