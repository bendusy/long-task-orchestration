# Changelog

## Token metering + codex probe hardening

- **Commits**: `edeed19` (per-run token stats), `9816022` (codex probe timeout), plus runner token sidecars (`976f778` claude, earlier codex/pi).
- **Summary**: Every LTO run now reports how many tokens it actually burned, and the codex runner can no longer hang indefinitely on its startup probe.

### Token metering — "how many tokens did this run cost?"

- Runners optionally write a `<reply>.meta.json` token sidecar; the scheduler merges it into `AgentResult.cost.tokens`. **Real, measured tokens** are available for **codex** (`codex exec --json` → `turn.completed.usage`), **pi** (`pi --mode json` → assistant `message_end.usage`), and **claude** (`claude -p --output-format json` → `result` envelope `usage`). **agy** exposes no usage via its CLI, so it is honestly reported as unmetered (not faked).
- New `state.token_rollup()` aggregates per-run usage across all `agent_runs`, broken down by runner, and **distinguishes metered vs total runs** so coverage is never overstated.
- `lto recap` shows a human line: `花了多少 token ── 约 69.5k tokens（2/3 次派工有计量）：pi 40.7k，codex 28.8k`.
- `lto closeout` embeds a machine-readable `token_usage:` line in `handoff.md`: `69464 total (in=…, out=…; 2/3 runs metered; pi=…, codex=…)`.

### codex probe — fix the "codex appears to hang" footgun

- `codex.sh` probes `codex exec --help` before the main run; that probe previously ran **unbounded**, so in an odd environment (e.g. an auth prompt waiting on stdin) it could hang until the scheduler's outer timeout. It is now bounded by its own `timeout 10s` — a hung probe exits 127 within ~10s instead of stalling the dispatch.
- Note on scope: the broader "codex hangs for minutes" symptom under a restricted host is a *runtime sandbox/approval* issue (codex waiting on an approval it can't get headlessly), documented in `cross-runtime-host-notes.md` / `validation-log.md`; the workaround is `--dangerously-bypass-approvals-and-sandbox` or scoped writable roots. This change only removes the one unbounded probe inside the runner.

## Intervention log v0

- **Run ID**: `20260605-171027-intervention-log-v0-for-reducing-meaning-50452529`
- **Summary**: Added a privacy-safe intervention log for measuring avoidable human work before larger telemetry, and documented the protocol-first language roadmap.

### Changes

- Added `.lto/<run-id>/interventions.jsonl` with redacted, low-sensitivity events.
- Judge logs avoided interventions when stale blockers are superseded by passing evidence.
- Closeout logs dirty-tree intervention candidates and force-closeout human interventions.
- Closeout prints and embeds an intervention summary in `handoff.md`.
- Artifact manifest now recognizes `interventions` artifacts.
- Added `references/protocol-and-language-strategy.md`: keep Python until protocol/conformance stabilizes; use Go later as shadow CLI; keep TypeScript for integration and Rust for narrow future components only.

## Refine closeout and stale-blocker workflow

- **Run ID**: `20260605-170401-refine-lto-robustness-workflow-after-sta-414f38ec`
- **Summary**: Simplified judge blocker classification and made closeout dirty-tree errors point to the intended workflow: commit/stash code first, then use `--no-changelog` for admin closeout.

### Changes

- Collapsed judge stale-blocker helpers into one `active` / `superseded` classifier.
- Reworded closeout dirty-tree refusal with direct operator guidance.
- Reworded `--auto-commit` help so it stays correct when `--no-changelog` is used.
- Added E2E coverage for actionable dirty-tree closeout guidance.

## Small robustness fixes to reduce meaningless intervention

- **Run ID**: `20260605-154647-small-robustness-fixes-to-reduce-meaning-6cee79d0`
- **Summary**: Added stale-blocker superseding, read-only judge classification for old blockers, and `closeout --no-changelog` for post-commit/admin closeout without tracked dirt.

### Changes

- Runner success archives previous blockers into `resolved_blockers` and clears active blockers.
- Judge treats blockers on done tasks with passing evidence as superseded instead of failing the verdict.
- Closeout supports `--no-changelog` and avoids including `CHANGELOG.md` in auto-commit hints when skipped.
- E2E tests cover blocker superseding and no-changelog closeout behavior.

## Docs and implementation consistency audit for control harness

- **Run ID**: `20260605-151521-docs-and-implementation-consistency-audi-0a4228a4`
- **Closed**: 2026-06-05T15:33:57+08:00
- **Summary**: Ran doc/implementation consistency triad audit and fixed standalone paths, future-spec banners, delegate wiring, source-note claim status, smoke doc-lint, and scheduler default runner path.

### Tasks

- ✅ **T1**: Run local consistency scan (done)
  - ❌ [review] review: FAIL
  - ✅ [review] review: PASS
- ✅ **T2**: Run triad doc audit (done)
  - ✅ [review] review: PASS
- ✅ **T3**: Synthesize findings (done)
  - ✅ [manual] manual: PASS
- ✅ **T4**: Apply safe corrections (done)
  - ✅ [test] test: PASS


## Control-loop harness spec with run telemetry

- **Run ID**: `20260605-145906-control-loop-harness-spec-with-run-telem-b783ec00`
- **Closed**: 2026-06-05T15:12:31+08:00
- **Summary**: Specified LTO control-loop harness principles, run logs, telemetry, privacy ingress, metric catalog, and Phase 1 passive logging plan after triad review.

### Tasks

- ✅ **T1**: Draft control harness spec (done)
  - ✅ [manual] manual: PASS
- ✅ **T2**: Run triad review (done)
  - ✅ [review] review: PASS
- ✅ **T3**: Synthesize review (done)
  - ❌ [manual] manual: FAIL
  - ✅ [manual] manual: PASS
- ✅ **T4**: Validate and push (done)
  - ✅ [test] test: PASS


## Privacy self-check script with confirmed cleanup

- **Run ID**: `20260605-091834-privacy-self-check-script-with-confirmed-849b6fcd`
- **Closed**: 2026-06-05T09:24:56+08:00
- **Summary**: Added privacy self-check script with dry-run default, per-item delete confirmation, gitignore protections, docs, and smoke coverage.

### Tasks

- ✅ **T1**: Design privacy checker (done)
  - ✅ [manual] manual: PASS
- ✅ **T2**: Implement privacy checker (done)
  - ✅ [test] test: PASS
- ✅ **T3**: Add docs and smoke coverage (done)
  - ✅ [test] test: PASS
- ✅ **T4**: Validate and push (done)
  - ✅ [test] test: PASS


## Plugin real eval runner with real-world evidence

- **Run ID**: `20260604-233201-plugin-real-eval-runner-with-real-world--ef2b67a1`
- **Closed**: 2026-06-05T08:30:35+08:00
- **Summary**: Optimized plugin real eval-run design as a sub-LTO-run compiler with critical source absorption, frozen evidence, metrics taxonomy, and promotion gates.

### Tasks

- ✅ **T1**: Design real eval contract (done)
  - ❌ [manual] manual: FAIL
  - ✅ [manual] manual: PASS
- ✅ **T2**: Research triad design (done)
  - ✅ [review] review: PASS
- ✅ **T3**: Synthesize implementation plan (done)
  - ✅ [manual] manual: PASS
- ✅ **T4**: Validate design closeout (done)
  - ✅ [test] test: PASS


## Plugin system phase 2 with render eval and triad audit

- **Run ID**: `20260604-231007-plugin-system-phase-2-with-render-eval-a-7a1fa1c0`
- **Closed**: 2026-06-04T23:26:37+08:00
- **Summary**: Completed plugin phase 2: render-profile, source-note workflow, static eval pack checks, triad audit, blocker fixes, docs and tests.

### Tasks

- ✅ **T1**: Implement plugin render and eval (done)
  - ✅ [test] test: PASS
- ✅ **T2**: Add source note workflow (done)
  - ✅ [test] test: PASS
- ✅ **T3**: Run triad audit (done)
  - ✅ [review] review: PASS
- ✅ **T4**: Validate closeout push (done)
  - ✅ [test] test: PASS


## Plugin boundary v0 for source notes and path profiles

- **Run ID**: `20260604-224630-plugin-boundary-v0-for-source-notes-and--9ee3507c`
- **Closed**: 2026-06-04T22:55:49+08:00
- **Summary**: Implemented plugin-boundary v0: data-only plugin validation/list/mount, source-note/profile sample plugin, mount-lock provenance, tests, docs, and LTO-mode evidence.

### Tasks

- ✅ **T1**: Design plugin boundary v0 (done)
  - ✅ [manual] manual: PASS
- ✅ **T2**: Add plugin validate mount list primitives (done)
  - ✅ [test] test: PASS
- ✅ **T3**: Create deep-agent profiles sample plugin (done)
  - ✅ [test] test: PASS
- ✅ **T4**: Validate and close out (done)
  - ✅ [test] test: PASS
