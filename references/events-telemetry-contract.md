# LTO events & telemetry contract

> 状态：active/current——`events.jsonl` + `telemetry.json` 的现状合同。
> 从 `control-loop-harness.md` 切出（2026-07-14），实现真源 `src/events.rs` /
> `src/event_emit.rs` / `src/telemetry.rs`。

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

