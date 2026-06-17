# LTO control-loop harness spec

> **STATUS: Phase 1 passive logging plus O2 event wiring implemented.** The Rust sensor layer — append-only `events.jsonl` (known event registry enforced on write) + derived `telemetry.json` — is implemented in Rust (`src/events.rs`, `src/event_emit.rs`, `src/telemetry.rs`). O2 adds caller-side events for runner results, audit dispatch/findings, gates, budget, sandbox refusals, and judge/decision outcomes. Phase 0/0.5 remains this reviewed specification. Sections defining Issue/Claim/Barrier and typed workspace targets still describe future behavior, not current CLI behavior.

> LTO is not an agent UI, not a PM replacement, and not a marketplace of workflows. LTO is a control harness for long AI-agent work: observe state, detect drift, limit unsafe actions, improve flow, reduce waste, and preserve evidence for future tuning.

## 1. Purpose

Long agent tasks fail through unstable feedback:

- goal drift after many turns;
- self-verification bias;
- agentic laziness and premature completion;
- unbounded fan-out or audit loops;
- hidden cost/time burn;
- stale memory and source poisoning;
- weak evidence trails that prevent future tuning.

LTO should strengthen the control loop around the host agent without taking over planning.

Primary outcomes:

1. **Work paving**: make the next valuable work visible: critical path, blockers, WIP, pending gates, and highest-risk unknowns.
2. **Performance**: reduce wall time by exposing bottlenecks, safe parallelism, retries, and stale waits.
3. **Efficiency**: reduce duplicate agent work and low-yield review loops.
4. **Cost control**: track runner calls, elapsed time, budget burn, and waste.
5. **Quality**: make findings, issues, decisions, and verification evidence traceable.
6. **Future tuning**: preserve structured run logs and telemetry so later evals can learn from real runs without turning memory into truth.

## 2. Control-theory mapping

| Control concept | LTO construct | Notes |
|---|---|---|
| Setpoint | `goal`, `why`, `done_when`, `delivery_contract`, phase exit criteria | Human/host defines target; `/goal`-style work uses target/constraint/instrument/forced-entropy fields. |
| Plant | Real work being done in repo, docs, tests, deployment | Messy external system. |
| Sensors | `state.json`, `artifacts.json`, runner stdout/stderr, tests, git state, issue/finding logs | Sensors can be noisy. |
| Observer / estimator | `check`, `judge`, `audit`, privacy scan, schema parsers | Estimates truth; never truth itself. |
| Advisory brief (host-decided) | `next`, `recap`, `autopilot --supervised` | Surfaces signals and candidate facts; host decides. The decision engine exists in Rust internals, but historical `autopilot --decide` is not exposed by the current CLI. |
| Actuators | `runner`, `run parallel`, `run pipeline`, delegate runners, worktree exec | Must be bounded. |
| Actuator limiter | `PermissionPolicy`, worktree sandbox, budget, timeout, concurrency, human approval | Prevents runaway action. |
| Supervisor | Human + host agent | Final authority. |
| Final stabilization | `closeout`, changelog, handoff, validation evidence | Locks what happened. |

Invariant:

```text
LTO may observe, summarize, constrain, and propose.
LTO must not silently decide, route, promote, deploy, or erase.
```

## 3. Feedback loops

### 3.1 Stability loop

Goal: reduce drift and premature completion.

Signals:

- goal/done_when mismatch;
- open high/critical issues;
- unverified findings;
- stale blockers;
- missing test evidence;
- dirty worktree;
- phase gate violations.

Control actions:

- `next` flags drift and missing evidence;
- `judge` blocks closeout;
- `audit` only on high-risk / unresolved uncertainty;
- human gate for semantic ambiguity.

### 3.2 Flow/performance loop

Goal: improve wall-clock throughput without losing control.

Signals:

- task cycle time;
- blocked age;
- WIP count;
- dependency chain length;
- runner elapsed seconds;
- retry count;
- queue/wait time;
- parallelizable candidates;
- fan-out barrier readiness.

Control actions:

- split oversized tasks;
- freeze scope when WIP exceeds limit;
- propose safe fan-out only for independent tasks;
- surface critical path;
- recommend resolving oldest/highest-severity blocker first.

### 3.3 Cost loop

Goal: reduce token/model/runtime waste.

Signals:

- runner invocation count;
- elapsed seconds by runner;
- timeout/rate-limit count;
- duplicate findings;
- low accepted-finding ratio;
- audit rounds;
- estimated token/cost when available.

Control actions:

- budget hard stop;
- max audit rounds;
- max fan-out width;
- reuse frozen evidence;
- prefer static validation before runtime eval;
- mark low-yield runner/profile as observation, not automatic ban.

### 3.4 Quality loop

Goal: reduce rework and false confidence.

Signals:

- accepted/rejected finding ratio;
- issue reopen count;
- parse/schema success;
- test rerun pass/fail;
- judge reversal;
- false positive evidence;
- missed known blocker in eval cases.

Control actions:

- require structured outputs where possible;
- convert findings to issue lifecycle;
- require verification evidence before resolving;
- keep judged metrics distinct from deterministic metrics.

### 3.5 Learning loop

Goal: tune harness based on real runs without memory poisoning.

Pipeline:

```text
run telemetry -> observation -> hypothesis -> eval case -> promoted heuristic or rejected note
```

Rules:

- observations are not facts;
- external articles remain claims until tested;
- worker performance is advisory only;
- no auto-routing based on historical stats;
- promotion requires evidence and human approval.

## 4. Signals and metrics

### 4.1 Run-level metrics

```json
{
  "run_id": "20260605-...",
  "goal_label": "redacted-or-short-label",
  "goal_hash": "sha256:salted-per-run",
  "phase": "intake",
  "started_at": "...",
  "closed_at": null,
  "wall_seconds": 3600,
  "tasks_total": 8,
  "tasks_done": 5,
  "tasks_blocked": 1,
  "open_issues_by_severity": {"critical": 0, "high": 1, "medium": 2, "low": 0},
  "wip_count": 2,
  "oldest_blocker_seconds": 7200,
  "runner_calls": 14,
  "runner_calls_per_hour": 3.5,
  "audit_rounds": 2,
  "estimated_cost_usd": null,
  "estimated_cost_per_hour": null,
  "safety_events": 0,
  "privacy_findings": 0,
  "seconds_since_last_event": 120
}
```

### 4.2 Task-level metrics

```json
{
  "task_id": "T3",
  "status": "blocked",
  "created_at": "derived from task.created event or null",
  "started_at": "derived from runner.started event or null",
  "last_updated_at": "...",
  "cycle_seconds": 1800,
  "blocked_seconds": 600,
  "retry_count": 1,
  "status_transition_count": 3,
  "evidence_count": 3,
  "linked_issues": ["ISSUE-002"],
  "touched_files": ["src/commands/ops.rs"]
}
```

### 4.3 Worker observation metrics

```json
{
  "runner": "codex",
  "profile": "codex-audit-readonly-v1",
  "task_kind": "audit",
  "rc": 0,
  "elapsed_seconds": 132,
  "timeout": false,
  "rate_limited": false,
  "parse_success": true,
  "findings_total": 5,
  "findings_accepted": 3,
  "findings_rejected": 2,
  "private_path_leaks": 0,
  "permission_violations": 0,
  "cost_estimate_usd": null,
  "finding_acceptance_rate": null,
  "false_positive_rate": null,
  "recorded_by": "host-observed"
}
```

Worker observations feed future briefs and evals. They must not auto-select workers.

### 4.4 Fan-out / barrier metrics

```json
{
  "barrier_id": "BARRIER-001",
  "pattern": "fan-out",
  "expected_workers": ["codex", "pi", "agy"],
  "arrived_workers": ["codex", "pi"],
  "missing_workers": ["agy"],
  "duplicate_rate": 0.25,
  "contradiction_count": 2,
  "merge_drop_count": 0,
  "unique_useful_findings": 4,
  "synthesis_status": "blocked|ready|complete"
}
```

### 4.5 Metric catalog rules

Every metric must declare:

| Field | Meaning |
|---|---|
| `source` | `state`, `artifacts`, `events`, `git`, `privacy_scan`, `judge`, `manual` |
| `method` | `deterministic`, `oracle_assisted`, `judged`, `manual` |
| `nullable` | whether old runs / missing provider metadata may return null |
| `threshold` | optional hard gate or warning threshold |
| `intended_action` | what a host should consider when the signal trips |

Phase 1 metric catalog:

| Metric | Source | Method | Nullable | Threshold/action |
|---|---|---|---|---|
| `wall_seconds` | events/state | deterministic | yes | long runs surface in `next` |
| `tasks_total/done/blocked` | state | deterministic | no | blocked > 0 gates closeout context |
| `runner_calls` | events | deterministic | no | warn when over budget |
| `runner_calls_per_hour` | events | deterministic | yes | detect burn rate |
| `timeout_count` | runner.finished | deterministic | no | repeated timeout suggests split/runner change |
| `evidence_count` | artifacts | deterministic | no | missing evidence blocks confidence |
| `seconds_since_last_event` | events | deterministic | yes | stalled run warning |
| `status_transition_count` | events | deterministic | yes | rework/churn signal |
| `privacy_findings` | privacy scan | deterministic | yes | hard fail before export/push |
| `redaction_summary` | event writer | deterministic | yes | any failed redaction blocks export |
| `estimated_cost_usd` | provider metadata | deterministic when available | yes | budget gate if configured |

Metrics not computable until later phases (issue lifecycle, worker observation, barrier) must remain absent or null, not fabricated.

## 5. Run log and telemetry design

LTO needs two complementary logs:

1. **Event log**: append-only record of what changed.
2. **Telemetry snapshots**: derived metrics for tuning and briefs.

### 5.0 Privacy ingress and export boundary

Event logging adds value only if it is safer than raw transcripts. Privacy filtering happens **before** a line is appended, not only during export.

Field classes:

| Class | Examples | Rule |
|---|---|---|
| export-safe | event type, schema version, status, rc, elapsed, counts | May appear in `events.jsonl` and exported telemetry. |
| local-only | repo-relative artifact ids, task ids, salted goal hash | May appear locally; exporter may hash/redact. |
| forbidden inline | raw stdout/stderr, transcripts, secrets, private source contents, absolute private paths | Never inline in event lines. Store as artifacts only if existing artifact policy allows. |

Ingress redaction rules:

- free-text fields (`summary`, short reason strings) are capped at 240 chars;
- absolute paths under `$HOME` are rewritten to `<HOME>/...` or repo-relative form;
- obvious secrets/API keys/private-key blocks are rejected before append;
- artifact refs use ids or repo-relative paths, never absolute private paths;
- actor ids are pseudonymous runtime ids (`runner:codex`) unless human identity is explicitly needed for a gate record;
- redaction failure emits no detailed event; it increments a local error counter and returns an append error.

Exporter boundary:

- `events.jsonl` is local-first.
- `telemetry.json` is derived and may be redacted/exported.
- Exporters must test redaction at field level; `privacy_self_check.sh` repo scan is not sufficient as an event redactor.

Existing `state.json` and `artifacts.json` are also part of the log surface. Phase 1 must mark whether these files passed a privacy scan before events reference their artifact ids for export.

### 5.1 Event log

Path:

```text
.lto/<run-id>/events.jsonl
```

Properties:

- append-only;
- line-delimited JSON;
- append guarded by file lock / atomic replace discipline;
- event ids are monotonic per run and duplicate ids are rejected;
- schema validation happens before append;
- no raw output or secrets in event lines;
- every event has provenance;
- large outputs stay in artifacts, not events;
- old runs with missing fields must load with null/defaults.

Size policy:

- warn at 10,000 events;
- hard stop new non-critical events at 50,000 events unless `--force-log-growth` is explicitly used;
- closeout may produce `events.compact.jsonl` as a derived summary, but the original local event log remains the audit source.

Base event:

```json
{
  "event_id": "evt-20260605T150101Z-000001",
  "schema_version": 1,
  "run_id": "20260605-...",
  "at": "2026-06-05T15:01:01Z",
  "type": "task.status_changed",
  "actor": {
    "kind": "host|lto|runner|auditor|human",
    "id": "codex|pi|host-agent|ben"
  },
  "phase": "implementation",
  "task_id": "T3",
  "object_id": "T3",
  "object_type": "task",
  "summary": "T3 blocked by failing test",
  "artifact_refs": ["artifact:evidence_stdout:T3:..."],
  "privacy": {
    "contains_raw_output": false,
    "redaction_status": "not_required|passed|failed"
  }
}
```

Known event types:

| Type | Emitted when |
|---|---|
| `run.started` | `start` creates run |
| `run.closed` | `closeout` succeeds |
| `phase.changed` | phase transition when available |
| `task.created` | `task add` |
| `task.status_changed` | runner/host changes task status |
| `runner.started` | command/job begins |
| `runner.finished` | command/job ends; may include rc, elapsed, timeout, repo-relative touched files |
| `runner.retry` | scheduler result reports retry attempts; emitted by callers with run context |
| `runner.healthcheck` | runner is skipped because healthcheck marked it unhealthy |
| `artifact.registered` | artifact manifest changes |
| `audit.dispatched` | audit prepare, auto-dispatch, or risk discovery selects auditors |
| `audit.finding` | structured audit finding summary is parsed; claim text stays out of event fields |
| `audit.converged` | audit ledger round is recorded with blocker counts |
| `gate.evaluated` | check/judge/closeout gate evaluates structured checks |
| `gate.blocked` | check/closeout gate blocks progress |
| `budget.warned` | budget check crosses warning threshold |
| `budget.exceeded` | budget gate reaches hard limit |
| `sandbox.rejected` | worktree sandbox refuses a command before execution |
| `judge.skipped` | LLM judge dispatch is skipped with a structured reason |
| `decision.voted` | judge/decision path records a structured vote or verdict summary |
| `decision.escalated` | decision/judge path needs host intervention |

Deferred event types:

| Type | Deferred until |
|---|---|
| `issue.created` / `issue.status_changed` | issue lifecycle phase |
| `human_override.recorded` | human override event schema phase |
| `permission.denied` / `permission.snapshot` | permission telemetry phase |
| `worker.observed` | worker observation phase |
| `barrier.created` / `barrier.synthesized` | fan-out barrier phase |
| `event_schema_error` / `redaction_failed` | event writer diagnostics phase |

### 5.2 Telemetry snapshots

Path:

```text
.lto/<run-id>/telemetry.json
```

Derived from `state.json`, `artifacts.json`, `events.jsonl`, and git status. It can be rebuilt.

```json
{
  "schema_version": 1,
  "run_id": "20260605-...",
  "generated_at": "...",
  "run_metrics": {},
  "task_metrics": [],
  "worker_observations": [],
  "issue_metrics": {},
  "barrier_metrics": [],
  "budget": {
    "max_wall_seconds": null,
    "max_runner_calls": null,
    "max_cost_usd": null,
    "used_wall_seconds": 3600,
    "used_runner_calls": 14,
    "used_cost_usd": null
  },
  "redaction_summary": {"passed": 0, "failed": 0, "not_required": 0},
  "event_log": {"event_count": 0, "seconds_since_last_event": 0}
}
```

Telemetry is derived signal only. It must not persist `control_recommendations` or route-like advice. `next` may read telemetry and produce an ephemeral host-facing brief; host decides.

### 5.3 Privacy and retention

- Event log must never inline full stdout/stderr, transcripts, secrets, or private source contents.
- It may reference artifacts by id/path.
- Exporters must run the same privacy scan as `privacy_self_check.sh` patterns.
- `telemetry.json` should be safe to include in handoff after redaction.
- Cleanup remains per-item confirmed; no telemetry auto-deletion.

## 6. Typed workspace objects

This spec does not require UI. Typed objects live in files and briefs.

### 6.1 Issue (future typed workspace object; not implemented)

```json
{
  "id": "ISSUE-001",
  "schema_version": 1,
  "status": "open|triaged|accepted|rejected|resolved|verified|waived",
  "severity": "critical|high|medium|low",
  "claim": "schema mismatch in plugin eval pack",
  "created_by": {"kind": "auditor", "id": "codex"},
  "created_at": "...",
  "linked_tasks": ["T2"],
  "finding_refs": ["FINDING-003"],
  "artifact_refs": ["artifact:audit_reply:..."],
  "decision_refs": [],
  "resolution": null,
  "human_approval_required": true
}
```

Future closeout gate (not implemented):

```text
open/accepted critical or high issue -> closeout refused unless waived with reason + human approval.
```

Current closeout gates use existing state/risk/blocker/audit-ledger checks; there is no Issue object gate yet.

### 6.2 Finding (future typed workspace object; not implemented)

```json
{
  "id": "FINDING-003",
  "status": "raw|triaged|accepted|rejected|converted_to_issue",
  "severity": "high",
  "claim": "profile env can self-authorize PATH",
  "evidence": "profile validation only checks plugin-owned allowlist",
  "confidence": "high",
  "source": {"runner": "codex", "artifact_ref": "..."},
  "issue_ref": "ISSUE-001"
}
```

Findings are claims. Issues are accepted work/risk items.

### 6.3 Research note / Claim / Hypothesis (future typed workspace object; not implemented)

```json
{
  "id": "CLAIM-001",
  "source_ref": "source-note:cockpit-shared-workspace",
  "status": "unverified|supported|contradicted|obsolete",
  "claim": "typed shared workspace improves multi-agent synthesis quality",
  "hypotheses": ["workspace event log reduces merge conflicts"],
  "counter_metrics": ["event_conflict_count", "closeout_residue"],
  "evidence_refs": []
}
```

### 6.4 Decision (future typed workspace object; not implemented)

```json
{
  "id": "DECISION-001",
  "kind": "accept|reject|waive|promote|defer",
  "claim_refs": ["CLAIM-001"],
  "issue_refs": ["ISSUE-001"],
  "reason": "use event log, reject PM auto-planning",
  "decided_by": "human|host",
  "at": "..."
}
```

## 7. Actuator limits

Every actuator must be bounded.

| Actuator | Required limits |
|---|---|
| `runner` | timeout, cwd, touched files, permission snapshot |
| `audit --auto-dispatch` | runner health, timeout, max auditors, no same-runtime self-audit |
| `run parallel` / `run pipeline` | concurrency, shell command list, rc capture |
| `autopilot --auto-exec` | worktree sandbox, safe command allowlist, no push/deploy/delete |
| future `eval-run` | plan-only default, execute explicit, budget, max concurrency, frozen evidence |
| future fan-out | barrier object, max workers, synthesis requirement, contradiction report |

## 8. Work paving briefs

`next` should evolve from task facts toward control facts:

```text
Current State
- phase: implementation
- WIP: 3 (limit 2) -> over limit
- critical path: T2 -> T5 -> closeout
- oldest blocker: T3, 7h
- open high issues: ISSUE-002
- budget: 12/20 runner calls used
- duplicate audit findings last round: 60%

Candidate Actions
1. Resolve ISSUE-002 before new work
2. Split T5; it blocks closeout and touches 8 files
3. Stop fan-out; last round low yield and high duplication
```

No automatic route. Host decides.

## 9. Anti-patterns

- UI-first Cockpit clone.
- PM coordinator daemon.
- Automatic worker selection from historical stats.
- Global KB treated as fact.
- Loop until issues empty.
- Adding metrics without actionability.
- Letting telemetry include raw private output.
- Optimizing for speed while hiding quality regressions.
- Treating judged metrics as deterministic truth.
- Creating issue noise without lifecycle/closeout gates.

## 10. Implementation plan

### Phase 0 — spec and review

- Write this spec.
- Triad review for overreach and missing controls.
- Record accepted/rejected design decisions.

### Phase 0.5 — logging safety contract

Before implementation:

- define event schema validation;
- define ingress redaction and redaction failure behavior;
- define local-only vs export-safe fields;
- define event log size warnings and hard stop;
- define old-run null/default handling;
- define privacy scan gate for `state.json` and `artifacts.json` before export.

### Phase 1 — passive logging

Goal: future tuning with low risk and no behavior change.

- Add `events.jsonl` writer helpers.
- Emit initial Phase 1 events: `run.started`, `run.closed`, `phase.changed`, `task.created`, `task.status_changed`, `runner.started`, `runner.finished`, `artifact.registered`.
- Add `telemetry build` helper that derives deterministic `telemetry.json`.
- O2 expands sensor coverage to runner retry/healthcheck results, audit dispatch/findings/rounds, gate evaluation/blocking, budget warnings/exceeded, sandbox rejection, and judge/decision summaries. Production writes still enforce `KNOWN_EVENT_TYPES`; readers remain tolerant of future event types.
- Do not emit issue/worker/barrier events yet.
- Do not persist `control_recommendations`.
- Add tests that events contain no raw stdout/stderr, no absolute private paths, and no obvious secrets.
- No behavior changes.

### Phase 2 — issue lifecycle

- Add `issue add/list/triage` commands.
- Convert judge/audit blockers into suggested findings, not auto issues.
- Closeout refuses open high/critical issues unless waived.

### Phase 3 — worker observations

- Append `worker.observed` events after delegated/audit/eval jobs.
- Add worker summary to `recap` and `next`.
- Keep advisory only.

### Phase 4 — fan-out barrier

- Extract implicit audit fan-out into a barrier object.
- Track missing workers, contradictions, duplicate rate, synthesis status.
- No generic swarm yet.

### Phase 5 — control brief upgrades

- Add WIP, blocker age, critical path, budget burn, issue status, and low-yield loop warnings to `next`.

## 11. Review questions

Triad reviewers should challenge:

1. Does event logging risk privacy or data bloat?
2. Are metrics actionable or vanity?
3. Does issue lifecycle create too much friction?
4. Does telemetry tempt auto-routing?
5. Are actuator limits strong enough?
6. Is Phase 1 passive enough to ship safely?
7. Which Cockpit ideas remain unabsorbed but valuable?

## 12. Non-goals

- No UI.
- No central server.
- No PM coordinator.
- No auto-routing.
- No global worker pool allocator.
- No automatic promotion.
- No raw transcript telemetry export.
- No long autonomous loop.
