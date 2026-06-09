# Changelog

## Unreleased

### eval-run llm_judge — subjective quality layer, frozen and isolated

- **Summary**: `plugin eval-run` could only compare **deterministic** metrics (parse rate, timeout, permission violations, pointer-only). This adds the deferred LLM-judged quality pass — a heterogeneous runner reads each case's evidence and judges blocker quality / false-positive suspicion. Designed by the standing co-design pass, implemented by a sub-agent, then adversarially reviewed by codex (3 BLOCKER + 3 MEDIUM, all fixed before merge). User-chosen scope: **judge reads and is frozen, but never touches promotion** — deterministic metrics still own the promote gate.
- **Three invariants (test-pinned)**:
  - **Heterogeneous**: the judge runner is never the same family as the runner that produced the candidate reply (reuses `_same_family`); same-family → skipped, never silent self-judging. Unhealthy/missing judge runners fall through to the next heterogeneous candidate.
  - **Reproducible**: the judge's *redacted* input evidence and its *verdict* are hashed separately (`evidence_hash` + `judgment_hash`) and frozen to `frozen-evidence.json` / `judge-verdict.json`. Same evidence + a re-run/edited verdict → `judgment_hash` changes. Redaction eats whole private paths (dir + filename, POSIX/Windows/JSON-escaped) and full PEM blocks + key-value secrets — a judge prompt must never carry a secret.
  - **No promotion power**: the judge result is a separate `comparison["judge"]` layer marked `kind: "subjective_judgment"`; it never mixes into deterministic metrics, `case_ok`, deltas, or the promote path. `automatic_promotion` stays the only remaining `DEFERRED_V0` item (promotion stays human-gated).
- **Changes**: new `scripts/lto/llm_judge.py` (redact / `freeze_evidence` / `_freeze_verdict` / heterogeneous-healthy judge dispatch); `plugin_eval_run` freezes evidence + runs judge before writing `comparison.json`; judge input capped at 256KB (oversized → skipped, not dispatched). `DEFERRED_V0` shrinks to `["automatic_promotion"]`.

## v0.2.0 — 2026-06-09

Passive sensor layer (events.jsonl + telemetry.json), per-run token metering, live job logs, and the delegate `--sandbox` fix. Entries below.

## delegate: explicit `--sandbox` flag (codex was silently read-only)

- **Commit**: this commit.
- **Summary**: Delegating a *write* task to codex failed confusingly: `codex.sh` defaults to `CODEX_SANDBOX=read-only` (a sound safety default), but `delegate.sh` exposed no way to override it except an undocumented env var — so a caller asking codex to edit files would get an honest "I can't write" back, and mistake it for a codex regression. It is not a codex bug; codex correctly obeyed the read-only sandbox it was handed. The gap was a missing explicit dispatch-side control.
- **Fix**: `delegate.sh` now takes `--sandbox <read-only|workspace-write|danger-full-access>`, validates it, and maps it to `CODEX_SANDBOX` for the codex runner (subprocess and tmux paths). It is ignored with a stderr notice for non-codex agents (only codex has a sandbox concept). Default stays read-only — write access is opt-in.
- **Verified**: env passthrough tested for all four cases (workspace-write passes through, no-flag leaves it unset, invalid value rejected, non-codex ignored+warned); then a real codex run with `--sandbox workspace-write` wrote and read back a probe file, confirming the same codex that previously reported `WORKTREE_NOT_WRITABLE` can now write.

## Events log + telemetry — the passive sensor layer (control-loop Phase 1)

- **Commit**: `765e4eb`. Designed against the reviewed `control-loop-harness.md` Phase 1 spec, implemented by a sub-agent, then adversarially reviewed by 2 heterogeneous auditors (codex + pi) whose findings were union-merged (no voting) and fixed before merge.
- **Summary**: LTO could observe a run's *current* state (`state.json`) but kept no first-class record of *what happened over time* — so `next`/`recap` and any future eval had to guess from snapshots. This adds the spec's Phase 1 sensor layer: an append-only `.lto/<run-id>/events.jsonl` event stream and a derived `.lto/<run-id>/telemetry.json`. It is **pure sensor**: zero LLM, zero decisions, append-only. It records what occurred; it never routes, promotes, or decides. This is the foundation the deferred items (`autopilot --autonomous`, eval `llm_judge`) were waiting on — see `references/backlog.md`.

### Changes

- New `scripts/lto/events.py`: append-only writer for the **8 Phase 1 event types** (`run.started` / `run.closed` / `phase.changed` / `task.created` / `task.status_changed` / `runner.started` / `runner.finished` / `artifact.registered`); deferred types are rejected. Reuses `interventions.py`'s redaction model.
- New `scripts/lto/telemetry.py`: derives `telemetry.json` (run/task metrics, budget, redaction summary, event-log counters) from `state.json` + `events.jsonl`. It is rebuildable and **never** persists `control_recommendations` / route / promote advice (test-pinned).
- Emit is wired into `start` / `closeout` / `runner` / `task-add` / `artifacts` — only **added** calls, no behavior change to the existing commands.
- **Privacy is enforced before append, not at export**: event lines never inline stdout/stderr/transcripts/secrets/private paths. Redaction is recursive (nested `details.stderr`, `*_excerpt`/`*_tail` suffix keys are stripped), and an event flagged `contains_raw_output` is rejected outright. `telemetry.json` redacts all string fields (e.g. `goal_label`) and keeps `touched_files` repo-relative.
- **Concurrency-safe**: append takes an `fcntl.flock` (mirroring `artifacts._manifest_lock`) over the read-count→assign-id→write window, so parallel runners can't produce duplicate `event_id`s or interleave bytes. Verified by a multiprocess test (6 workers × 40 appends → 240 contiguous ids, 0 dups, every line valid JSON).
- **Fail-safe by design**: emit goes through a single `safe_emit()` helper with a lazy `events` import wrapped in `try/except` — a broken/missing events module can never crash a core command (a sensor must not take down the system it observes).
- Free-text fields are capped at 240 chars per spec §5.0.

## Live log — see what a job is doing while it runs

- **Commit**: `fdc5912`. Designed by a 3-runtime co-design pass, implemented by a sub-agent, then adversarially reviewed by 3 heterogeneous auditors whose findings were merged back in.
- **Summary**: LTO jobs were a black box — `scheduler` ran each runner via `subprocess.run(capture_output=True)`, so while a job was running you saw nothing; a stuck job was invisible until its timeout fired. Now every job streams its output to `.lto/<run-id>/live/<job_id>.log` as it runs, so the host agent (or a human) can `tail` it live. This borrows tmux-autopilot's "observability is a feature" idea **without** using tmux — the scheduler stays on plain `subprocess` so it remains deterministic and CI-friendly (the 16-case self-test and fake-runner tests keep working unchanged).

### Changes

- `scheduler` now uses `Popen` + two drain threads (`read1` streaming) instead of `subprocess.run`; stdout is teed to the live log while still captured for the result. Process group via `start_new_session=True` + `os.killpg` so timeouts kill grandchild processes cleanly.
- Runners (`codex`/`pi`/`claude`/`agy`) changed their stdout from `> tmpfile` to `| tee tmpfile` (keeping `PIPESTATUS[0]` for the real exit code), so the CLI's output reaches the scheduler's pipe **and** the temp file used for reply/token parsing. Verified end-to-end: a codex run with `CODEX_JSON=1` writes a 317-byte live log containing the real `turn.completed` NDJSON, with token metering unaffected.
- Optional **stall detection** (`stall_timeout`, default `0` = off): when enabled, a job whose stdout stops growing for N seconds is killed early instead of waiting for the full timeout. Off by default because thinking-heavy runners (pi/codex reasoning) can be silent for a long time before emitting — opt in only with a sane lower bound.
- `lto recap` shows a "currently running" line by scanning `live/*.log` mtimes; absent/old runs degrade gracefully (no line, no error).
- Security: the `run_id` used to locate `live/` is now whitelist-validated, so a tampered `.lto/current` can't escape the repo directory.

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
