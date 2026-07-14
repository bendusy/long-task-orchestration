# LTO control-loop roadmap（design/future）

> 状态：design/future——本文件全部是设计目标，**不是现状**，不得当已实现能力引用。
> Typed workspace objects（Issue/Claim/Barrier 等）与实施计划从
> `control-loop-harness.md` 切出（2026-07-14）。

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

