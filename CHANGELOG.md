# Changelog

## Implement references/specs/2026-06-17-goal-L3-dispatch-L4-mining-unified.md

- **Run ID**: `20260617-034103-implement-references-specs-2026-06-17-go-213bfa75`
- **Closed**: 2026-06-17T05:49:56+00:00
- **Summary**: Implemented L3 dispatch-goal completion events and L4 read-only cross-run mining over events.jsonl; fixed audit blockers; redlines, docs, audit ledger, and release build passed.

### Tasks

- **L3**: L3 dispatch-goal and agent.turn.completed event notification (done)
  - [manual] architecture_alignment: L3 belongs in existing tmux_runner/runner dispatch surfa
  - [manual] first_principles: L3 is the sensor layer for L4; if turn completion is written t
  - [manual] L3 implemented: KNOWN_EVENT_TYPES includes agent.turn.completed; dispatch-goal r
- **L4**: L4 cross-run mining and readonly recap --mine brief (done)
  - [manual] simplification_dedupe: L4 must lift telemetry.rs runner_metrics/by_runner-style
  - [manual] L4 implemented: telemetry::cross_run_mining discovers .lto runs, reads events.js
- **DOC**: Backlog README SKILL workflow-playbook documentation alignment (done)
  - [manual] Docs aligned: COMMANDS documents recap --mine; INSTALL distinguishes agy -i manu
- **VERIFY**: Phase gates dogfood audit and split commits (done)
  - [manual] value_measurement: baseline is current CLI lacking dispatch-goal and recap --min
  - [manual] Verification so far passed: cargo fmt --all --check; cargo clippy --locked --all
  - [manual] Final verification passed after R1 audit fixes: cargo fmt --all --check; cargo c

**Next**: Host owns release/tag/push; local implementation is ready after split commits and post-commit strict check.


## Unreleased

### L3 dispatch-goal and L4 cross-run mining

- Added `lto dispatch-goal` for tmux-backed goal dispatch to codex, pi, and agy,
  reusing the Rust tmux carrier instead of adding a second tmux layer. pi and
  agy run through exiting shell wrappers so pane-exit completion can be emitted.
- Added `agent.turn.completed` to the events whitelist and a hook-facing
  `agent-turn-completed` emitter. Completion notifications now go to
  `.lto/<run>/events.jsonl`; no `turns.jsonl` stream is written.
- Codex dispatch installs an idempotent Stop hook with backup/uninstall support
  and updates its own LTO marker when the target repo changes. agy dispatch
  uses `agy --print` so the shell wrapper can emit completion on process exit;
  interactive `agy -i` remains a manual TUI mode.
- Moved `recap --mine` onto Rust `telemetry::cross_run_mining`: it scans run
  events across `.lto/*`, groups by runner/task/date, counts
  `agent.turn.completed`, and prints a read-only brief without changing config,
  routing, or runner priority.

### Hardening from heterogeneous audit findings (backlog ⑫)

- events.jsonl now fails closed on lock timeout: instead of a lock-less
  best-effort write that could interleave a corrupt JSONL line, `emit` returns
  an error (dropped by `safe_emit`) — events are an observability projection, so
  losing one clean event beats corrupting the log. The record is also written in
  a single `write_all` so it stays atomic under `O_APPEND`.
- Unified secret/path redaction into one source of truth in `redact.rs`. The
  former weaker copy leaked `/root`, Windows paths, `github_pat_` tokens and
  `key=value` pairs into events/telemetry; the merged superset covers them.
  `llm_judge` re-exports a verbatim variant (no whitespace-collapse/truncation)
  so frozen-evidence hashing keeps its exact shape.
- Investigated but not changed (auditor false positives, verified): `test_cmd`
  shell execution is a trusted operator-supplied command that never passes
  through the `classify_effect` gate, and `RunnerFamily::Unknown` already
  isolates by name. ReDoS was a non-issue (Rust's regex engine is linear-time).

### Lean context for one-shot review dispatch (backlog ⑪)

- Audit and judge jobs now set `LTO_LEAN_CONTEXT=1` on the dispatched job env.
  Review work is one-shot and does not need the runner's skill/extension/context
  ecosystem, so each `runner.sh` translates the flag into its CLI's equivalent:
  pi gets `--no-skills --no-context-files --no-extensions` (~17x faster on a
  trivial prompt), claude gets `--setting-sources ''` (~7.5x fewer input
  tokens). codex and agy have no safe context-only flag and degrade gracefully.
- The flag is orthogonal to the read-only permission allowlist; read-only audit
  jobs apply both. Development dispatch (autopilot workers) does not set it.

### pi session reuse for warm prompt cache across audit rounds (backlog ⑪ 治本)

- Audit and risk-discovery jobs now carry a stable per-(run, auditor) session id
  (`lto-<run_id>-audit-<auditor>`) on the dispatched job env (`LTO_SESSION_ID`).
  pi's runner translates it to `pi -p --session-id <id>`, so the same auditor
  reused across audit rounds resumes its persistent session and hits the provider
  prompt cache (host-verified: a fresh process resuming the same session gets
  cacheRead>0 and pi's fresh input stays small — it does not bloat across turns).
- Backward compatible: when `LTO_SESSION_ID` is unset, no `--session-id` is passed
  and behavior is unchanged. Only pi's runner honors it — codex resume bloats
  input tokens across turns and agy does not run read-only audits, so neither
  gets session reuse (cross-runner investigation via official docs + live tests).

## v0.5.0 — 2026-06-16

### Host verification tmux loop playbook

- Added the open-source `tmux-goal-loop` playbook for host-consensus goals:
  repo-owned tmux short-session workers, heterogeneous audit, and an explicit
  host verification hard stop before closeout.
- Marked backlog item ⑩ as Rust-landed after T1/T2 supplied the repo-owned
  tmux dispatch substrate, and documented that no private `tmux-autopilot`
  skill is required.
- Hardened `lto check --to closed --strict` with a default-fail evidence gate:
  a `done` task without task evidence now blocks closeout.

### Tmux-backed autopilot workers

- Extended `lto autopilot --auto-exec` with a `--worker-runner`
  carrier selector. The existing sandbox path remains available, while
  `--worker-runner tmux` dispatches one bounded worker job per pending task
  through the Rust tmux runner.
- Added worker completion contracts under `.lto/<run>/live/*.worker.json`.
  Autopilot updates `state.tasks` from the contract `rc`, not from the tmux
  pane merely signaling that it stopped.

### Tmux runner adapter

- Added a Rust-owned `runner: "tmux"` scheduler path that dispatches prompts
  with direct `tmux` subprocess calls, supports signal/sentinel/fire
  completion modes, safe send-keys preflight, ready/skip prompt matching, and
  capture-pane output in `AgentResult`.
- Extended `lto runner` with tmux target/session/sentinel/ready options, and
  allowed `--runner tmux --command` without `--task-id` to exercise the
  scheduler-backed tmux dispatch path directly.

### Observability O2 event wiring

- Expanded the Rust event writer from the Phase 1 type list to a
  `KNOWN_EVENT_TYPES` registry covering runner retry/healthcheck, audit,
  gate, budget, sandbox, judge, and decision events while keeping production
  writes typo-guarded and reads tolerant of future event types.
- Added caller-side event wiring for scheduler-backed audit/run/plugin jobs,
  closeout/check gates, budget checks, autopilot sandbox refusals, and
  plugin eval-run judge outcomes. The scheduler remains a generic executor;
  callers emit runner events from `AgentResult` while they still have run_id
  context.
- Extended telemetry with runner failure-rate rollups and audit
  dispatch/finding/round metrics.

### Python fallback retirement

- Removed the legacy Python fallback package (`scripts/lto/`), the `lto_run.py`
  entrypoint, and Python fallback tests after Rust parity evidence was recorded.
  The installed `lto` wrapper now executes `lto-rs` only.
- Retired `--use-python` and `LTO_USE_PYTHON=1`. Those paths now fail with a
  clear v0.5.0 removal message instead of silently routing to Rust or producing
  an import traceback.
- Kept historical `.lto` compatibility through the Rust legacy fixture test;
  no live Python CLI is required to read old run state.
- Converted `scripts/write_decision.py` into a standalone ADR/artifact helper
  with no dependency on the removed fallback package.
- Updated ownership/docs gates so command ownership is Rust-only and active
  docs no longer teach Python fallback paths.

### Rust plugin legacy ports and observability

- Added Rust-owned `lto-rs plugin eval-run` for real baseline-vs-candidate A/B
  plugin evals through the existing scheduler, with env allowlist filtering,
  deterministic safety metrics, frozen judge evidence, token sidecar support,
  and focused Rust tests.
- Added Rust-owned `lto-rs plugin source-note` for creating inert plugin source
  notes with path-safety checks, atomic JSON writes, optional idempotent
  `plugin.json` updates, and focused unit coverage.
- Added Rust events/telemetry support before deleting the Python sensor layer:
  append-only event logging, recursive redaction, raw-output rejection,
  monotonic event ids under `.events.lock`, tolerant duplicate reads, and
  derived telemetry without `control_recommendations`.

### CLI surface and debt tracking

- Added a Phase 2.5 CLI surface decision record and reduced the visible
  top-level command surface from 24 to 21 business commands by grouping task
  lifecycle commands under `task` and batch execution commands under `run`.
- Kept `task-add`, `task-update`, `phase`, `parallel`, and `pipeline` as hidden
  compatibility entrypoints for one deprecation cycle, while documenting the
  new `task add/update/phase` and `run parallel/pipeline` forms.
- Extended the ownership gate to track those hidden compatibility entrypoints
  separately from visible help rows, and made auto-discovered audit risks
  closeout-blocking by default through `disposition: open`.
- Hardened closeout/check risk gates so legacy risk records with `status: open`
  and no `disposition` also block closeout instead of being silently ignored.
- Fixed ledger evaluation so a later zero-blocker audit round converges even if
  an earlier round rebounded before fixes landed.
- Added short help descriptions for every visible top-level command and a Rust
  test that fails if future visible commands omit help text.
- Recorded scheduler runner lifecycle events / O1-1 tracing as explicit
  deferred P1 backlog after the Python retirement, rather than leaving the
  observability gap as an untracked audit note.

## v0.4.3 — 2026-06-16

### Binary identity and release verification

- Exposed `lto-rs --version` through clap and aligned the Rust crate version
  with the release version, so downloaded binaries can prove their release
  identity without relying on filenames.
- Added a docs consistency gate that fails when `Cargo.toml` package version and
  `VERSION` drift apart.
- Replaced production LazyLock regex `unwrap()` calls with contextual
  `expect(...)` messages so future regex edits fail with a useful location.
- Re-ran release privacy verification with the real `gitleaks` scanner instead
  of the regex-only `--no-gitleaks` fallback.

## v0.4.2 — 2026-06-16

### Release package checksum verification

- Fixed tag-time package checksum verification to run `shasum -c` from the
  `dist/` directory where the tarball lives. The docs consistency gate now
  checks this invariant so release CI cannot regress to a root-relative checksum
  mismatch.

## v0.4.1 — 2026-06-16

### Open-source delivery gate hardening

- Strengthened the open-source delivery requirements with a maintainer review
  frame, repository cleanup requirements, a development-requirements design
  gate, and a push-candidate freeze gate. The release bar now explicitly
  requires coherent Rust/Python/platform/plugin/release stories, classified
  cleanup decisions, and current CI/release/asset evidence before publish.

### Release workflow hardening

- Fixed the tag-time release binary workflow before announcing downloadable
  assets: Linux musl builds now install `musl-tools`, macOS Intel builds use a
  current Intel runner label, and release asset upload/download verification is
  serialized to avoid concurrent GitHub Release races. The docs consistency
  gate now checks these release workflow invariants.

## v0.4.0 — 2026-06-16

### Development and closeout gates — architecture, docs, history, clean rebuild

- **Summary**: Added explicit pre-implementation/optimization and pre-closeout/release evidence gates to LTO docs and templates. Host agents should align with architecture before coding, reason from first principles, check simplification/deduplication opportunities, require value measurement for tuning work, align documentation, clean stale history, return the worktree to a clean state, and rebuild/repackage from the final state.
- **Artifacts**: `AGENTS.md`, `SKILL.md`, `references/workflow-playbook.md`, `references/run-state-workflow.md`, and `templates/run-state.md` now name the development evidence lines (`architecture_alignment`, `first_principles`, `simplification_dedupe`, `value_measurement`) and closeout evidence lines (`documentation_alignment`, `historical_cleanup`, `clean_worktree`, `rebuild_package`).
- **Boundary**: This remains a host-agent judgment gate and evidence contract, not a new automatic router or mandatory ceremony.

### Rust v2 core track — typed contracts and takeover path

- **Summary**: Added the Rust v2 workspace (`lto-rs`) from the staged 2026-06-15 specs and aligned the public docs around Rust as the takeover path. The Python CLI remains as a compatibility fallback until the wrapper default is flipped after parity verification.
- **Rust-native boundaries**: runner output is parsed into tagged enums (`pi` reply from `message_update/text_delta`, `codex` usage from `turn.completed`, `claude` result fallback); `Sandbox`/`ExitState`/`JobStatus`/`TaskSize` are typed; `state` uses `serde(flatten)` to preserve unknown Python keys; `RankedCandidate` has no execute method; `MergeReview` requires deterministic diff + test result and keeps `audit_opinion` optional.
- **Reusable core modules**: shared `process` helpers centralize shell/git CLI execution so worktree and merge-review logic do not each maintain their own git wrappers. `scheduler` now exposes the reusable deterministic core for batch validation, exit classification, health re-probe, retry backoff, and attempt-to-result conversion before the runner I/O layer is cut over. Plugins remain data-only JSON/Markdown; Rust validates existing manifests instead of migrating plugin code.
- **Heterogeneous review guard**: the decision reviewer gate now requires at least two valid non-empty reviewers from distinct runner families, so same-family aliases such as `codex`/`openai-gpt-5` cannot satisfy the heterogeneity contract.
- **Coverage**: `cargo test` covers runner parsers, scheduler classifier/backoff/health gates, budget semantics, state compatibility, worktree sandbox redlines, dispatch three-cell scoring, decision 2-vote/needs-human semantics, judge isolation, plugin validation, and CLI command-count parity (24).
- **CI**: added `.github/workflows/rust-v2.yml` for fmt/check/clippy/test on Linux/macOS and tag-time Linux/macOS release binary builds. Windows is deferred while the built-in runner protocol remains shell-script based. `unsafe_code = "forbid"` is set at the crate lint layer.

### Run-level budget contract — graded brake on autonomous over-run

- **Why**: distilled from elvis (@omarsar0)'s *Autonomous Long-Running Coding Agents* — a strong goal is a contract that includes *the number of turns and budget*. LTO had `why`/`done_when` (human-recap free text) but no run-level turn/token/deadline cap; `--timeout` was per-dispatch only. This closes the contract gap without touching the "host is planner" core.
- **Data model**: new optional `state.budget` block (`max_turns` / `max_tokens` / `hard_deadline`, all default `None` = unlimited → zero break for old runs). `turns_used` is a monotonic counter incremented **only** by autopilot auto-advance (human manual ops never count). `warn_ratio` defaults 0.8.
- **Pure measurement** (`scripts/lto/budget.py`): `check_budget(state, token_total, now_iso)` — no file/time I/O (token total + now injected by caller, like `next`). Per-dimension `ok|warn|exceeded`; overall = strictest. tz-naive normalization so a tz-aware `iso_now()` and a naive `--deadline` don't crash on compare.
- **Graded enforcement**: soft warning at `warn_ratio` surfaces in `next`/`recap` as a fact line (zero block); hard brake at 100% in autopilot emits `NEEDS_CONFIRM` + zero auto-advance (fail-closed), `turns_used += 1` happens *before* the check so the touching turn is caught. Unlocked only by explicit `lto budget extend` or re-start.
- **New `lto budget check / extend`**: check reports per-dimension usage; extend raises caps (human action) and **cannot shrink below already-used** (anti self-lock). Command count 22 → 23; core module count 26 → 27.
- **Verified**: `test_budget.py` (18) / `test_budget_gate.py` (11 asserts) / `test_budget_softwarn.py` (4) / `test_budget_cmd.py` (5) all green; regression green on `lto_run self-test`, `smoke_test`, `test_autonomous_gate`, `test_next`, `test_orchestration_cmds`; CLI end-to-end (start --max-tokens → check → extend → check) verified.

### dev-workflow plugin — the full idea-to-release development chain

- **Enterprise audit gate**: dev-workflow `0.2.1` adds `enterprise-audit-gate-v1`, a data-only layered audit prior for high-risk work: requirements → architecture → data model → interface contract → implementation → testing → operations/observability → security → migration/rollback → acceptance. It includes `enterprise-layer-auditor-v1`, a read-only cross-family profile, redline vocabulary, a strict enterprise output schema that requires `layer` / `redline`, and an eval case that checks the path/prompt coverage contract. This deliberately stays a plugin/playbook asset, not a new core command or mandatory committee.
- **Summary**: Adds `plugins/dev-workflow`, a data-only plugin distilled (and de-identified) from mining 60+ real development sessions across 5 host projects. Main path `feature-dev-main` covers six phases — specify → dispatch → impl-audit → converge → acceptance → release — as scheduling priors, not a state machine. Side paths: `docs-sync-loop` (doc-vs-code drift sweep with `drift-ok` intentional-divergence annotations) and `direction-review` (taste/direction disputes default to human escalation; `needs_human` from any auditor escalates immediately and cannot be outvoted; 2/3 voting only with explicit human pre-authorization). Prompts include a six-item acceptance gate (scripts green / artifacts actually read / adversarial findings converged / docs synced / lessons persisted / observable) and an observability checklist (structured log schema, doctor entry point, troubleshooting commands). The spec itself (`references/dev-workflow-spec.md`) went through a 3-runtime adversarial review (codex/pi block + agy revise, 21 findings union-processed, 1 rejected with evidence) before any implementation — dogfooding the exact workflow it encodes.
- **workflow-playbook**: four new sections (`feature-dev` / `docs-sync` / `release` / `direction-review`) in the same five-field structure as the existing five, plus an explicit note that mid-build verification belongs to `review`.
- **Existing plugin gap fixes**: adversarial-audit gains a fourth auditor (`claude-refuter-v1`, wired into the path with a same-family-host anti-pattern), an agy hallucination warning, and a direction-dispute exit; claim-verify-research hardens "local code claims require actually reading the source (`path:line`), an LLM assertion is not evidence"; migration-refactor gains exemplar-selection guidance, a static `codex-semantic-equivalence-v1` profile (host picks by family — no runtime logic in profiles), and a concrete 4-step merge-conflict rollback sequence (scheduler stops, host rolls back).
- **Fail-closed fixes from the spec review**: `gemini` (like agy) cannot enforce read-only and is now rejected at validate time instead of silently passing (regression-pinned in `test_agent_job_readonly.py`); profile `family` is now validated against a known enum and `runner_constraints` (`exclude_host_family` / `min_distinct_families`) gives the cross-family rule a machine-readable form.
- **eval-run negative cases**: `case_type: "negative"` + `expected_outcome: "scheduler_reject"` — the fail-closed rejection itself is the unit under test (zero spawn, zero judge). The adversarial-audit agy case is now such a regression assertion.
- **Plugin perception layer**: plugins were previously invisible at runtime decision points — `next` / `resume` / `start` never mentioned them, so an agent deep into a long task (SKILL.md long out of the context window) could not discover them. Now `next`'s Decision Brief carries a "Harness Affordances" section (mounted vs. available plugins, id + description + path intents), `resume`'s capsule lists plugins with a playbook pointer, and `start` prints the available count to stderr. Facts only, zero recommendation — matching a plugin to the task shape stays the host's job via workflow-playbook (perception ≠ routing).
- **Verified**: all 5 plugins `validate` + static `eval` green; `test_plugins.py`, `test_agent_job_readonly.py` (12), `test_plugin_eval_run.py` (21 passed) all green; real `eval-run`: 5/5 cases ok (4 positive with candidate `parse_ok=true`, agy negative case rejected with message hit), zero new `private_path_leak` / `permission_violation`.

### Three preset scenario plugins (playbook data packs, not workflows)

- **Summary**: Adds three data-only plugins distilled from real usage scenarios: `adversarial-audit` (heterogeneous refute-first audit squads; union-merge findings without voting, vote only on direction), `claim-verify-research` (claim → falsifiable hypothesis → frozen evidence → heterogeneous refute → explicit-confidence verdict), and `migration-refactor` (minimal exemplar first, batched migration with per-batch regression gates in isolated worktrees). All three follow plugin-boundary v0: `kind: path-plugin`, `stage: experimental`, read-only profiles, empty env allowlist, eval packs with the two mandatory safety metrics and `safety_regressions_allowed: 0`. They are **playbook priors for the host agent, not preset workflows** — host stays planner; nothing routes or promotes automatically.
- **Verified**: `lto plugin list` all OK; `plugin validate` + static `plugin eval` green for each; `scripts/test_plugins.py`, `scripts/test_plugin_eval_run.py` (18 passed), and `lto self-test` all pass; real `plugin eval-run` A/B evidence recorded per plugin (see samples or run artifacts).

### Per-dispatch token + elapsed feedback (no waiting for closeout)

- **Summary**: Each spawned subtask now reports its token + elapsed cost **the moment it finishes**, instead of only surfacing in `recap`/`closeout` aggregates. `spawn_agents` prints a per-job line to stderr (`⤷ pi/deepseek-v4-pro · ok · 40.7k tokens · 32s`) and a batch total when more than one job runs. Unmetered runners (agy) are honestly labeled, not faked as 0.
- **Run-level total elapsed**: `token_rollup` now also accumulates `total_elapsed_sec` across all `agent_runs`, so `recap` ("约 69.5k tokens：pi 40.7k，codex 28.8k · 派工累计 47s") and `closeout` handoff ("69500 total … 47s total …") report both how many tokens and how long the dispatches took. `report=False` silences the per-job lines (used in tests).

## v0.3.0 — 2026-06-09

The "gets smarter the more you use it" release — but the host agent does the reasoning, never LTO. Cross-run mining (model effectiveness over time, down to the specific model), eval-run's frozen-and-isolated quality judge, and the mechanical-gate `--autonomous`. Entries below.

### autopilot --autonomous — mechanical evidence gate + execution (LTO never reflects)

- **Summary**: Implements the last autopilot tier, but deliberately **narrowed** to LTO's boundary: LTO does not process data with an LLM and never reflects — reflection always belongs to the host agent. So `--autonomous` is **not** a self-deciding loop. It does two mechanical things: (1) reads ⑥ cross-run mining facts as an **evidence gate** (only unlocks once enough real dispatch data has accumulated; otherwise honestly falls back to supervised and says how much data is missing); (2) once the gate passes, mechanically runs safe/reversible substeps in the auto-exec worktree sandbox. It never spawns a decision agent. Reviewed by codex (2 BLOCKER + 3 HIGH, all fixed).
- **Hard boundary — never spawn a decision agent**: `--autonomous` is mutually exclusive with `--decide`; passing both clears `--decide` (otherwise escalate would spawn the convergence agents — exactly the "LTO reflects for you" the boundary forbids).
- **Gate is strict and fail-closed**: counts only contract-shaped `AgentResult`s (`job_id` + known runner + terminal status) — empty `{}` dicts can no longer pad past the 5-run / 10-result threshold. A malformed `mine()` return (None, non-dict, string-number counts) is rejected, never opened.
- **Safety arguments now actually hold**: the `git push` interceptor was widened to catch flag-injected variants (`git -C . push`, `git -c k=v push`, `git --git-dir=… push`) — the literal-only pattern was bypassable. autonomous also runs with `allow_network=False` (curl/wget/nc/ssh/scp are HELD), since network side effects aren't reversible. escalate / dangerous / push / network always return to the human.
- **Changes**: `autopilot.py` `_autonomous_gate` + autonomous branch; `cross_run_mining.py` strict `gate_runs`/`gate_results` counts; `worktree_exec.py` git-variant interception; `test_autonomous_gate.py` + worktree push-variant cases. Docs (SKILL/README/4 references) realigned from "not implemented / spawns decision agent" to the mechanical-gate reality.

### Cross-run mining — "which model is actually effective, over time" (evolution mainline v1)

- **Summary**: The first piece of "LTO gets smarter the more you use it". A cross-run scanner walks every `.lto/<run-id>` once and mines two signals into one host-facing brief: **model effectiveness** (from `state.json` `agent_runs`, grouped by runner × status — success rate, cost, failure/timeout counts) and **recurring phase friction** (from `events.jsonl`, by event type across runs). Designed by the standing co-design pass, implemented by a sub-agent, reviewed by codex (0 BLOCKER, 6 findings, all fixed).
- **Hard line — the brief is evidence, never a command**: it surfaces hypotheses ("codex succeeded 80% across 3 runs vs pi 17% — whether to prefer codex next is *yours* to decide; not a routing instruction"), never routes, promotes, or auto-selects. Banned command-phrasing is test-scanned across the empty / thin-sample / normal branches.
- **Cross-run means cross-run**: the effectiveness comparison only fires when both compared runners appear across `>= min_runs` *distinct* runs — repeated dispatches within a single run no longer crown a winner. A corrupt `state.json` from one historical run is isolated (counted as `skipped_bad_runs`), never crashes the whole mining pass. Status accounting is transparent (`skipped`/`other` shown so the success-rate denominator is explainable). `events` is explicitly *not* a model source (its `runner.finished` is the local shell executor).
- **Changes**: new `scripts/lto/cross_run_mining.py` (`mine` + `render_mining_brief`); `recap --mine` opt-in surfaces it (no new top-level command).

### `AgentResult.model` — mining can now tell same-runner different-model apart

- **Summary**: Closes the limitation noted above. `agent_runs` recorded the `runner` (family) but not the `model` (specific id), so cross-run mining could only group by runner — it couldn't tell `pi` running `deepseek-v4-pro` apart from `pi` running `glm-4.6`. Now `AgentResult` carries a `model` field; the scheduler stamps it from the originating job at a single backfill point (covers skip / normal / exception-fallback construction paths); cross-run mining shows a per-runner model distribution.
- **Backward compatible**: old `agent_runs` without `model` load fine (`None`) and the brief simply omits the model section — no model field, no model rows. `AgentResult.to_dict`/`from_dict` round-trip the new field; `from_dict` ignores its absence.

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
