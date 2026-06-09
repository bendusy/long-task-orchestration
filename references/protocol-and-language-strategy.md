# Protocol-first LTO evolution and language strategy

**STATUS: Roadmap / research plan. Not an implementation spec for a rewrite.**

This document exists to prevent future agents from jumping straight to a Go,
Rust, or TypeScript rewrite before LTO's durable protocol is clear.

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

Language roadmap:

```text
Now:      Python core, because protocols are still changing.
Next:     Protocol freeze candidates + conformance tests.
Later:    Go shadow CLI reading/writing same .lto protocol.
Only then: consider Go as primary CLI core.
TS:       integration layer only.
Rust:     not needed unless a narrow security/sandbox component appears.
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

## Minimum protocol checklist before any rewrite

Do not start a Go/Rust/TS rewrite until these are true:

- `state.json` schema has a documented compatibility contract.
- `artifacts.json` kind list and path rules are documented.
- `interventions.jsonl` has stable event categories and redaction rules.
- `events.jsonl` Phase 1 event set is either implemented or explicitly deferred.
- A conformance test suite can run against any implementation and compare outputs.
- At least one real multi-run cycle shows the protocol improved host-agent briefs.
- Privacy scan covers new protocol files.
- Old `.lto` runs still load.

## Language choices

### Python now

Use Python while the protocol is still moving.

Why:

- fast iteration;
- easy local file/JSON/Markdown work;
- easy agent modification;
- current implementation and tests already exist.

Risk:

- packaging is weaker;
- type boundaries are looser;
- single-binary distribution is poor.

Decision: keep Python as primary implementation until protocol freeze candidates
exist.

### Go later

Go is the best candidate for the eventual core CLI.

Why:

- single binary;
- predictable distribution;
- good subprocess/concurrency support;
- good enough JSON/file tooling;
- simpler operational story than Python or Node.

But Go should start as a **shadow implementation**, not a rewrite.

Phase:

```text
lto-go check
lto-go judge
lto-go closeout
lto-go next
```

It must read/write the same `.lto` protocol and pass conformance tests against
Python outputs.

### TypeScript as integration layer

TypeScript is useful for:

- npm package wrapper;
- MCP server;
- editor/Claude Code/VSC integration;
- browser UI if explicitly approved later.

Do not move core control logic to TS just because other agent frameworks use TS.
LTO is local file-protocol harness first, not web/app framework first.

### Rust only for narrow components

Rust is not currently worth the cost for LTO core.

Potential future uses:

- path/security validation library;
- sandbox launcher;
- high-assurance redaction component.

No Rust rewrite without a specific narrow problem.

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

### Round 4: Go shadow decision

Question: is the protocol stable enough to justify a Go shadow CLI?

Start Go only if:

- conformance tests exist;
- Python behavior is no longer changing weekly;
- packaging/distribution pain is a real bottleneck;
- Go prototype can pass fixtures without special cases.

## Non-goals

Do not use this roadmap as permission to build:

- UI;
- server/daemon;
- worker marketplace;
- automatic model/router selection;
- executable plugin system;
- Go rewrite before protocol freeze;
- telemetry that stores raw transcripts or private paths.

## Current recommendation

Keep shipping small protocol-backed improvements in Python.

Next best steps:

1. Finish `interventions.jsonl` v0.
2. Add `next` / `resume` advisory use of intervention summary if real runs show value.
3. Write schema fixtures for existing protocol files.
4. Only after fixtures stabilize, design a Go shadow CLI.
