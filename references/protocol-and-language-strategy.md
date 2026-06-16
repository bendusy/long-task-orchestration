# Protocol-first LTO evolution and language strategy

**STATUS: protocol strategy, updated for Rust v2. Historical Python/Go/Rust
roadmap language in older revisions is superseded.**

This document exists to keep language work subordinate to LTO's durable file
protocol. It no longer argues for Python as primary or for a future Go shadow
CLI. Rust v2 is the current core path; Python fallback was removed in v0.5.0.

## Short version

LTO should first become better at learning from real LTO usage.

The important thing is not the implementation language. The important thing is a
stable local protocol that lets every run produce safe, useful tuning signals for
the next host agent.

```text
real LTO use
  -> structured local protocol
  -> redacted signals
  -> host-agent brief
  -> human-approved workflow improvement
  -> more useful next run
```

Current language posture:

```text
Now:          Rust v2 core path.
Compatibility: historical .lto state remains readable through Rust fixtures.
Protocol:     .lto/ files remain the product boundary.
Future:       conformance fixtures decide any additional implementation.
TS:           wrappers/MCP/editor integration only.
Go:           no near-term core path.
```

## What “越用越聪明” means

Not magic memory. Not autonomous routing. Not a PM platform.

It means LTO observes repeated friction in actual runs and feeds the host agent
small, evidence-backed tuning briefs.

Examples:

- A stale blocker was superseded by passing evidence.
  - Future brief: “this run avoided one manual blocker cleanup.”
- A closeout was blocked by dirty worktree.
  - Future brief: “commit/stash before closeout; use --no-changelog for admin closeout.”
- `--force` was used.
  - Future brief: “human override happened; decide if gate was too strict or action was genuinely exceptional.”
- A runner repeatedly times out.
  - Future brief: “runner reliability degraded; do not use this as write-critical reviewer until fixed.”
- A plugin/profile appears useful in eval cases but has no real-run win yet.
  - Future brief: “keep experimental; do not promote.”

The loop is:

```text
log -> summarize -> propose -> human/host decides -> record outcome
```

LTO must not jump from old telemetry to automatic routing or promotion.

## Protocol is the product boundary

The `.lto/<run-id>/` directory is LTO's real API. Language changes are allowed
only if they preserve this file protocol.

Current and near-term protocol surfaces:

| File | Role | Status |
|---|---|---|
| `state.json` | run/task/gate truth source | implemented |
| `artifacts.json` | artifact manifest | implemented |
| `audit-ledger.md` | adversarial audit convergence | implemented |
| `handoff.md` | closeout transfer capsule | implemented |
| `plugin-mounts.json` | mounted data-only plugin provenance | implemented |
| `interventions.jsonl` | low-sensitivity human-intervention log | v0 implemented |
| `events.jsonl` | broader append-only run events | implemented (Phase 1, 8 event types) |
| `telemetry.json` | derived run signals | implemented (Phase 1) |

Protocol rules:

1. Files must be append/read tolerant where possible.
2. Missing new fields must not crash old runs.
3. Logs must not inline raw transcripts, secrets, private source, or absolute private paths.
4. Recommendations are ephemeral; persistent files store evidence and derived signals, not commands to obey.
5. Every new schema needs:
   - field definitions;
   - nullability;
   - source;
   - redaction class;
   - test fixture;
   - backwards-compatibility behavior.

## `interventions.jsonl` schema (v1)

One JSON object per line, append-only.  This is the authoritative field
reference; code docstrings defer to it.

| field | type | nullable | source | redaction class | notes |
|---|---|---|---|---|---|
| `schema_version` | int | no | tool | none | currently `1`; adding optional fields does not bump it |
| `event_id` | int | no | tool | none | 1-based, per-run sequence |
| `at` | string | no | tool | none | ISO timestamp |
| `type` | enum | no | tool | none | `human_intervention` \| `intervention_candidate` \| `avoided_intervention` |
| `category` | enum | no | caller | none | whitelisted; see `_ALLOWED_CATEGORIES` |
| `source` | string | no | caller | cleaned | short label, e.g. `lto closeout` |
| `reason` | string | no | caller | cleaned | one-line human-readable cause |
| `meaningful` | bool | no | caller | none | **author-asserted label, not a measurement** |
| `avoidable` | bool | no | caller | none | **author-asserted label, not a measurement** |
| `preventable` | bool | no | caller | none | **author-asserted label, not a measurement** |
| `actor` | enum | yes | tool | whitelist | `runner` \| `gate` \| `operator`; primary cross-run group-by key |
| `gate` | string | yes | tool | cleaned | gate name, e.g. `closeout`, `judge` |
| `details` | object | no | caller | cleaned | **enums/numbers/bools only — no free text, diffs, or commands** |
| `dedupe_key` | string | yes | caller | cleaned | repeat appends with same key return the existing event |

"cleaned" = secrets redacted, `/Users/...` and `/home/...` paths redacted,
whitespace collapsed, truncated to 500 chars.

### Hard rules (CI must enforce these)

1. **`details` carries only enums, numbers, and booleans.** No free text, no
   diffs, no command lines, no source bodies.  Free-form context goes in
   `reason` (which is cleaned), not `details`.
2. **A new `category` is not mergeable without a fixture.** Every category in
   `_ALLOWED_CATEGORIES` must have a conformance fixture exercising it.
3. **`meaningful` / `avoidable` / `preventable` are author labels, not metrics.**
   They are the caller's assertion at write time.  Downstream (briefs,
   aggregation, future Go reimpl) must not weight them as objective truth.
   They are retained as-is until real multi-run data proves whether they carry
   signal beyond `actor`; if they don't, they collapse to a single `blame`
   enum (`system_should_prevent` | `human_judgment_required`).

## Minimum protocol checklist for takeover or future rewrites

Do not change the primary implementation strategy or remove fallback behavior
until these are true:

- `state.json` schema has a documented compatibility contract.
- `artifacts.json` kind list and path rules are documented.
- `interventions.jsonl` has stable event categories and redaction rules.
- `events.jsonl` Phase 1 event set is either implemented or explicitly deferred.
- A conformance test suite can run against any implementation and compare outputs.
- At least one real multi-run cycle shows the protocol improved host-agent briefs.
- Privacy scan covers new protocol files.
- Old `.lto` runs still load.

## Language choices

### Rust v2 default

Rust v2 is the current CLI/core path.

Why:

- single-binary distribution is the release target;
- typed state, scheduler, budget, delivery-contract, and plugin static paths
  make invariants explicit;
- installer and wrapper now default to Rust;
- macOS/Linux CI already exercises the Rust workspace.

Risk:

- stale compatibility language can imply a second live CLI path after retirement;
- old `.lto` runs may expose compatibility gaps;
- release claims can get ahead of GitHub assets.

Decision: Rust owns generic harness primitives. Every new core feature should
prefer Rust unless it is explicitly a legacy fallback or a Python-only test
fixture.

### Retired Python legacy fallback

Python is no longer a live compatibility bridge or comparison oracle.

Why:

- old behavior remains valuable as release history and fixed legacy fixtures;
- Rust now owns the public command surface and plugin/eval paths;
- keeping a second live CLI path would hide drift after v0.5.0.

Risk:

- hidden fallback can become a second product;
- active docs can accidentally teach Python as the default;
- duplicate command behavior can hide bugs until release.

Decision: Python fallback was removed after parity evidence existed for the
public command surface and plugin legacy commands. Do not reintroduce a second
live CLI path; preserve historical compatibility through fixtures and release
notes.

### TypeScript as integration layer

TypeScript is useful for:

- npm package wrapper;
- MCP server;
- editor/Claude Code/VSC integration;
- browser UI if explicitly approved later.

Do not move core control logic to TS just because other agent frameworks use TS.
LTO is local file-protocol harness first, not web/app framework first.

### Go

Go is not a near-term core path. A future Go experiment must start from
protocol conformance fixtures, not from taste or packaging anxiety.

Decision: no Go core work until Rust takeover and release distribution are
boring, documented, and measured.

## Research plan

Use multiple LTO runs to study the protocol before freezing it.

### Round 1: intervention log usefulness

Question: does `interventions.jsonl` actually reduce meaningless human work?

Evidence:

- count avoided interventions;
- count force closeouts;
- count dirty-closeout candidates;
- check if closeout summary helps next host.

Outcome:

- keep/change/remove event categories;
- decide whether intervention summary belongs in `next` brief too.

### Round 2: host-agent tuning brief

Question: what minimum signals should be fed to the host agent at resume/next?

Candidate signals:

- stale blockers avoided;
- repeated runner failure by runtime;
- gate too strict vs correctly strict;
- tasks with repeated retries;
- audit convergence trend;
- closeout friction.

Output should be a small advisory section, not routing authority.

### Round 3: protocol conformance

Question: can another implementation reproduce key outputs from `.lto` files?

Build fixtures:

- simple run;
- stale blocker run;
- force closeout run;
- plugin-mounted run;
- dirty-closeout blocked run;
- old run missing new fields.

Expected outputs:

- judge verdict;
- closeout summary;
- recap capsule;
- intervention summary;
- artifact manifest handling.

### Round 4: implementation strategy review

Question: is the protocol stable enough to justify another implementation or
major fallback shrink?

Change strategy only if:

- conformance tests exist;
- Rust behavior is no longer changing weekly;
- packaging/distribution pain is a real bottleneck;
- another implementation can pass fixtures without special cases.

## Non-goals

Do not use this roadmap as permission to build:

- UI;
- server/daemon;
- worker marketplace;
- automatic model/router selection;
- executable plugin system;
- another core rewrite before protocol freeze;
- telemetry that stores raw transcripts or private paths.

## Current recommendation

Keep shipping small protocol-backed improvements in Rust core while preserving
only explicit historical compatibility fixtures.

Next best steps:

1. Keep Rust default, wrapper behavior, and retired fallback errors verified.
2. Add conformance fixtures for existing protocol files.
3. Keep removed Python surfaces classified as historical, fixture, or
   removal-candidate.
4. Only after fixtures stabilize, decide whether any additional implementation
   is justified.
