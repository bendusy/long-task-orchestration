# Source note: Batched migration/refactor with regression gates

URL: internal:agent-skills/lto-release/practices/migration-refactor-2026
Captured: 2026-06-10

## Problem

Large-scale migrations (API version bumps, framework swaps, call-site refactors) run by agents tend to fail in two patterns:

1. **Scope creep**: agent opportunistically "cleans up" adjacent code while migrating, breaking behavior outside the migration contract.
2. **Cascading breakage**: early batches silently break invariants that later batches depend on; the failure is only detected at the end when repair cost is highest.

## Core ideas

### Minimal exemplar first

Pick the single smallest representative call site. Migrate it end-to-end, run regression, commit as a precedent. All subsequent batch agents receive this commit as their reference template. This front-loads the "figure out the pattern" cost and prevents each batch from independently re-discovering (and diverging on) the right approach.

### Batch loop with per-batch gate

```
discover → rank/batch → for each batch:
  worktree_exec (isolated) → regression (compile+test+lint+artifact-diff) → gate
  PASS → merge → next batch
  FAIL → stop, repair or rollback this batch; do not proceed
```

The gate is the harness primitive `progress` + `audit`. It checks exit codes, artifact evidence, and diff shape. It does not accept "agent says it's done" as evidence.

### Worktree isolation

Each batch runs in its own `git worktree`. Benefits:

- Parallel batches do not share working tree state.
- Rollback = delete worktree branch; nothing touches main.
- `git push` from a worktree still requires human confirmation (LTO invariant).
- Merge conflicts stop immediately; no cascade.

### Behavior-preservation contract

The migration contract is declared before any batch runs:

- Which call sites / files are in scope (explicit list or glob).
- What observable behavior must be preserved (test suite, API contracts, artifact checksums).
- What is explicitly out of scope (refactors, cleanups, unrelated fixes).

Agents are instructed: if a test fails, the migration is wrong — fix the migration, not the test.

### Artifact-evidenced progress

A batch is complete when:

1. Regression command exits 0.
2. Diff summary artifact is written (lines changed, files touched, call sites migrated count).
3. No out-of-scope files appear in the diff.

Agent self-report is not accepted as completion evidence.

## LTO adaptation

This plugin encodes the above as:

- A `path` describing the discover → exemplar → batch-loop → closeout sequence using LTO primitives (`worktree_exec`, `runner`, `audit`, `progress`, `judge`).
- Two read-only `profiles` for auditing a completed batch diff: one for behavioral-change / weakened-test detection (codex), one for semantic equivalence review (claude).
- An `eval` pack whose cases are completable read-only (audit existing repo files for self-consistency).

## Boundary

Migration execution (writing files, running tests) is the host agent's job via `worktree_exec` + `PermissionPolicy`. Plugin profiles only cover **auditing the resulting diff** in read-only mode. A plugin cannot grant write permission; it can only lower the ceiling.
