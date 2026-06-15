# LTO Open-Source Delivery Requirements

> Status: requirements and development guidance. This is not an implementation
> patch.
>
> Last checked: 2026-06-16. Local branch `feat/runner-fixes` is at `6e03dec`.
> GitHub tags exist for `v0.2.0` and `v0.3.0`, but the GitHub Releases API
> returns an empty release list, so there are no downloadable release binaries.

## Verdict

LTO is not ready to be pushed or announced as a clean open-source product until
the repository presents one coherent story:

- Rust is the default core path.
- Python is an explicit legacy fallback, not a hidden default.
- macOS and Linux are the supported release platforms.
- Windows native support is paused, not half-supported through shell fixtures.
- `.lto/` is the public file protocol and local truth source.
- Plugins are data-only extensions that compile into existing harness
  primitives.
- Releases provide verified macOS/Linux binaries, checksums, and installation
  instructions based on real assets.

Anything less ships confusion. Tags without assets are not a binary release.
Docs that disagree about Python, Rust, Windows, or plugin authority are bugs,
not harmless history.

## Non-Negotiable Goal

A new user must be able to clone or download LTO on macOS/Linux, understand that
Rust is the default implementation, run a complete task lifecycle, and verify
the result without reading private handoff context.

The target open-source experience is:

1. Build from source:
   `cargo build --release --locked --bin lto-rs`.
2. Run core smoke:
   `cargo run -- self-test`, `cargo run -- check --json`, and a minimal
   start/task/runner/check/closeout lifecycle.
3. Install wrapper:
   `bash scripts/install.sh`, then `lto self-test`.
4. Use Python fallback only when requested:
   `lto --use-python self-test` or `LTO_USE_PYTHON=1 lto ...`.
5. Install from release assets once they exist:
   download the macOS/Linux tarball, verify `.sha256`, unpack, run
   `./lto-rs self-test`.
6. Use the plugin path without giving plugins execution authority:
   `lto plugin list`, `validate`, `render-profile`, `eval`, and `mount`.
7. Resume old `.lto` runs without data loss or schema crashes.

## Hard Non-Goals

- No Windows native release until the runner protocol has a native design,
  implementation, and CI. Do not patch production scheduler logic to satisfy a
  Windows shell fixture.
- No stateful daemon. LTO may write explicit `.lto/` files, but it must not
  hide decisions in a background service or external mutable state.
- No plugin marketplace, executable plugin code, plugin DAG engine, dynamic
  tool installer, automatic route selector, or automatic promotion.
- No hidden Python default. Python is allowed only as compatibility fallback
  until a separate removal gate proves it can shrink further.
- No documentation claiming downloadable binaries exist until GitHub Releases
  actually exposes assets and the checksum/self-test path has been verified.
- No push or public release from a dirty worktree or with unclassified private
  paths, stale handoff files, or unexplained generated artifacts.

## Architecture Boundaries

### Host And Harness

The host agent remains the planner. LTO provides state, artifacts, runner
dispatch, audit, sandbox, resume/recap, budget, delivery contract, and gates.
`next` and `autopilot` may present facts or execute safe mechanical steps in a
sandbox, but they do not own the product decision.

### File Protocol

The public API is the `.lto/<run-id>/` file protocol. At minimum, these files
must have documented compatibility behavior:

| Surface | Requirement |
|---|---|
| `state.json` | Run/task/phase/budget/delivery truth. New optional fields must not break old runs. |
| `artifacts.json` | Repo-relative artifact index. No private absolute paths in public examples. |
| `events.jsonl` | Append-only operational events. Redacted, tolerant to future event types. |
| `telemetry.json` | Derived facts only. It must not become a command source. |
| `audit-ledger.md` | Audit convergence evidence. Script-computed counts beat hand-written claims. |
| `plugin-mounts.json` | Provenance lock for mounted data-only plugin influence. |
| `handoff.md` | Human/agent transfer capsule, never a substitute for machine state. |

Every protocol change needs field definitions, nullability, redaction class,
backward compatibility behavior, and a fixture that proves old runs still load.

### Core Ownership

Rust core owns generic harness primitives:

- CLI command surface and argument contract.
- `.lto/` state read/write and compatibility.
- scheduler and runner result typing.
- worktree sandbox and permission policy.
- check/closeout gates.
- budget and event/telemetry rollups.
- delivery contract: `target`, `constraint`, `instrument`, and
  `entropy-check`.
- static data-only plugin commands.

`/goal` belongs in core only as a delivery contract. Its purpose is delivery:
making the target, constraints, measurement instrument, and anti-overfit move
explicit. It is not a supervisor, daemon, or route selector.

### Python Boundary

Python remains only for compatibility and legacy surfaces during the takeover:

- fallback command path through `--use-python` or `LTO_USE_PYTHON=1`;
- parity checks against historical `.lto` behavior;
- legacy plugin surfaces that are not yet Rust-owned, such as real
  `plugin eval-run` if still unported;
- tests that protect the fallback until a formal removal gate exists.

Do not delete Python just because a Rust command exists. First classify each
Python surface as `ported`, `fallback-only`, `legacy-plugin`, or
`removal-candidate`, then prove the Rust path owns the same external behavior.

### Plugin Boundary

Plugins are quarantine lanes for external ideas:

```text
source note -> falsifiable hypothesis -> data-only plugin/profile/eval
  -> evidence -> human promote/reject
```

They may lower permission ceilings or provide prompts, schemas, profiles, eval
cases, and playbook fragments. They may not raise permissions, execute code,
install tools, create hidden workflows, choose the runner automatically, or
promote themselves.

Promotion to core is rare. A feature enters core only when it is a generic
harness primitive that cannot be expressed by existing contracts. The delivery
contract qualifies. Most article or tweet ideas do not.

## Documentation Requirements

The active documentation set must tell one story. Before any open-source push,
audit and align at least:

- `README.md`
- `INSTALL.md`
- `AGENTS.md`
- `CLAUDE.md`
- `COMMANDS.md`
- `SKILL.md`
- `references/onboarding.md`
- `references/run-state-workflow.md`
- `references/engineering-map.md`
- `references/protocol-and-language-strategy.md`
- `references/rust-migration-release.md`
- `references/plugin-boundary.md`
- `references/plugin-real-eval-runner.md`
- `references/backlog.md`

Required cleanup:

1. Replace active `python3 scripts/lto_run.py ...` examples with Rust or
   wrapper examples unless the section is explicitly labeled legacy fallback.
2. Update or retire documents that still say Python is primary, Go is next, or
   Rust is not worth core use. That was a historical decision, not current
   truth.
3. Keep `COMMANDS.md` generated or checked against `src/cli.rs`; command count
   and flags must not drift.
4. Mark historical roadmap material as historical when it no longer represents
   current direction.
5. Keep binary download wording conditional on live release assets.
6. State the Windows policy consistently: macOS/Linux first, Windows native
   paused.
7. State the Python migration path in every install/release-facing document:
   source build, wrapper default Rust, explicit fallback.
8. Keep plugin docs explicit that Rust owns static data-only commands and
   Python legacy owns any unported real eval-run path.

## Rust Takeover Requirements

Rust takeover is complete only when these are true:

| Area | Requirement |
|---|---|
| Wrapper | `scripts/install.sh` installs `lto` that defaults to Rust and fails clearly if `lto-rs` is missing. |
| Fallback | `lto --use-python` and `LTO_USE_PYTHON=1` remain explicit and tested. |
| Command parity | All public commands either have Rust implementation or a documented legacy exception. |
| State compatibility | Rust can read old `.lto` runs and tolerate missing new fields. |
| Gates | `check --to implementation|closed` enforces delivery contract and closeout evidence where applicable. |
| Scheduler | Runner results are typed; timeout/rate-limit/failure states are not stringly guessed. |
| Pi/tool wrappers | Integration paths prefer Rust and use Python only when explicitly requested. |
| Plugin static path | `list/validate/render-profile/eval/mount` are Rust-owned and tested. |
| Release | CI builds release binaries on `v*` tags and uploads checksummed assets. |

The next cleanup pass must reduce duplicate logic. The correct order is:

1. Prove parity with tests and old-run fixtures.
2. Move one behavior owner to Rust.
3. Shrink or label the Python path.
4. Delete unreachable or duplicated code only after rollback is preserved.

## Release And Binary Requirements

GitHub must provide real release assets before users are told to download
binaries. A tag alone is not enough.

Required release targets:

- `lto-rs-x86_64-unknown-linux-musl.tar.gz`
- `lto-rs-x86_64-unknown-linux-musl.tar.gz.sha256`
- `lto-rs-aarch64-apple-darwin.tar.gz`
- `lto-rs-aarch64-apple-darwin.tar.gz.sha256`
- `lto-rs-x86_64-apple-darwin.tar.gz`
- `lto-rs-x86_64-apple-darwin.tar.gz.sha256`

The release workflow must:

1. Run Rust fmt/check/clippy/test on macOS and Linux.
2. Run Python fallback smoke or an explicit compatibility test job.
3. Build release binaries only from `v*` tags.
4. Upload tarballs and checksum files to GitHub Releases.
5. Verify at least one downloaded asset by checksum and `./lto-rs self-test`.
6. Record the release evidence in the LTO run before announcing availability.

Install docs must offer two paths:

- Developer path: clone and build from source.
- User path: download release asset, verify checksum, run self-test.

There should be no `curl | sh` install path until checksum, provenance, and
failure behavior are designed.

## Development Gate

Every implementation task after this document must record four evidence items
before coding or tuning:

| Evidence | Required content |
|---|---|
| `architecture_alignment` | Layer, boundary, reused pattern, and why the change belongs there. |
| `first_principles` | Real constraint, user value, or root cause. |
| `simplification_dedupe` | What was deleted, merged, reused, or intentionally left duplicated. |
| `value_measurement` | Baseline, metric, pass line, command, and post-change result for tuning. |

If a proposed change cannot explain the missing capability, it is not ready. If
an optimization has no baseline and retest, it is a guess.

## Verification Matrix

### Local Required Gates

Run these before declaring the branch ready:

```bash
cargo fmt --all --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/smoke_test.py
cargo build --release --locked --bin lto-rs
git diff --check
```

Wrapper and fallback smoke:

```bash
tmp_bin="$(mktemp -d)"
LTO_BIN_DIR="$tmp_bin" bash scripts/install.sh
"$tmp_bin/lto" self-test
"$tmp_bin/lto" plugin list
"$tmp_bin/lto" --use-python self-test
```

Plugin smoke:

```bash
lto plugin list
for dir in plugins/*; do
  test -d "$dir" || continue
  lto plugin validate "$dir" --json
  lto plugin eval "$dir" --json
done
```

Protocol compatibility smoke:

```bash
lto runs
lto resume --run-id <old-run-id>
lto check --run-id <old-run-id> --json
```

Privacy and repository hygiene:

```bash
bash scripts/privacy_self_check.sh
git status --short --branch
```

If an extra regex scan is added, keep its pattern outside the files being
scanned or use the existing privacy script, otherwise the verification command
will match its own documentation. Private-path hits may be allowed only if they
are redacted, historical, or test fixtures. Do not leave real local paths in
public guidance.

### CI Required Gates

PR CI must pass on:

- `ubuntu-latest`
- `macos-14`

Tag CI must pass release packaging for:

- Linux musl x86_64
- macOS Apple Silicon
- macOS Intel

Windows jobs must either be absent, non-blocking research jobs, or backed by a
real native design. A failing Windows shell-runner fixture must not block
macOS/Linux release, and it must not be "fixed" by weakening production logic.

### Release Asset Gate

After pushing a `v*` tag:

```bash
curl -fsSL https://api.github.com/repos/bendusy/long-task-orchestration/releases
curl -LO https://github.com/bendusy/long-task-orchestration/releases/latest/download/<asset>.tar.gz
curl -LO https://github.com/bendusy/long-task-orchestration/releases/latest/download/<asset>.tar.gz.sha256
shasum -a 256 -c <asset>.tar.gz.sha256
tar -xzf <asset>.tar.gz
./lto-rs self-test
```

Only after this gate may docs say users can download binaries.

## Acceptance Checklist

The branch is publishable only if every answer is yes:

- Can a stranger build from source on macOS or Linux?
- Can a stranger install the wrapper and see Rust as default?
- Can a stranger intentionally run the Python fallback and understand why it is
  fallback?
- Can a stranger download a release binary, verify checksum, and run self-test?
- Can a stranger run a minimal start/task/runner/check/closeout lifecycle?
- Can a stranger understand that Windows native support is paused?
- Can a stranger understand `.lto/` as the state protocol without reading
  private handoff notes?
- Can old `.lto` runs still load under Rust?
- Can all bundled plugins validate and statically eval?
- Are plugin real eval limitations explicit?
- Are docs free of conflicting Python/Rust/Windows/release claims?
- Is the worktree clean before packaging?
- Are privacy scans clean or every hit classified?
- Is remote CI green for PR and tag release paths?

If one answer is no, do not push as a release candidate.

## Work Breakdown

### P0: Stop Shipping Contradictions

- Align active docs to Rust-default/macOS-Linux/Python-fallback truth.
- Mark stale roadmap documents as historical or rewrite them.
- Fix active examples that still teach Python as the default path.
- Add a docs consistency scan to CI or smoke tests.
- Verify release asset wording against live GitHub Releases.

### P0: Release Path That Actually Produces Binaries

- Keep `rust-v2` PR CI green on macOS/Linux.
- Ensure tag-triggered `release-binaries` creates GitHub Releases assets.
- Add at least one post-release download/checksum/self-test verification step.
- Document source-build and binary-install flows separately.

### P1: Rust Owns Core, Python Shrinks

- Build a command-by-command ownership table.
- Add old-run compatibility fixtures.
- Port or explicitly defer each Python-only behavior.
- Remove duplicated branches only after parity evidence exists.

### P1: Plugin Eval Boundary

- Keep Rust static plugin path authoritative.
- Decide whether real `plugin eval-run` stays legacy Python for now or ports to
  Rust.
- Preserve human-gated promotion and deterministic-vs-judged metric separation.

### P2: Windows Design, Not Fixture Chasing

- Define native runner protocol requirements.
- Check real availability and behavior of codex/pi/agy/claude runners on
  Windows.
- Only then add Windows release/CI targets.

## Publish Blockers

These are hard stops:

- Any active doc says Rust is not the current core path.
- Any active install doc implies Python is the default.
- Any release doc claims binaries exist while GitHub Releases has no assets.
- Windows CI is required but not backed by native runner support.
- A plugin can execute code, raise permissions, or auto-promote.
- `cargo test` passes but wrapper install/self-test fails.
- Python fallback is broken and not intentionally removed with migration notes.
- `.lto` old-run compatibility fails.
- Private local paths, secrets, or business-specific data appear in public docs
  or artifacts.
- Worktree is dirty without an explicit, accepted reason.

## Definition Of Done

Open-source delivery is done when:

1. All P0 requirements are complete.
2. Verification matrix passes locally.
3. PR CI passes on macOS/Linux.
4. A `v*` tag produces GitHub Release assets and checksums.
5. A downloaded asset passes checksum and `self-test`.
6. Documentation matches the shipped behavior.
7. The LTO run records docs alignment, historical cleanup, clean worktree, and
   rebuild/package evidence.
8. The maintainer explicitly approves publish or merge.

Until then, the repository can be a development branch. It is not a clean
open-source release.
