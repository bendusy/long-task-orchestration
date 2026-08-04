use crate::agent_job::{AgentJob, AgentResult, JobStatus};
use crate::budget::{BudgetCheck, BudgetStatus};
use crate::events::{self, EventRecord};
use crate::worktree::SandboxResult;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractFieldCounts {
    pub targets: usize,
    pub constraints: usize,
    pub instruments: usize,
    pub forced_entropy: usize,
}

pub fn emit_contract_updated(
    repo: &Path,
    run_id: &str,
    phase: &str,
    changed_fields: &[&str],
    counts: ContractFieldCounts,
) -> anyhow::Result<()> {
    let emitted = events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "contract.updated".to_string(),
            actor_kind: "host".to_string(),
            phase: Some(phase.to_string()),
            object_type: Some("delivery_contract".to_string()),
            summary: format!("contract updated: {} field(s)", changed_fields.len()),
            fields: json!({
                "changed_fields": changed_fields,
                "target_count": counts.targets,
                "constraint_count": counts.constraints,
                "instrument_count": counts.instruments,
                "forced_entropy_count": counts.forced_entropy,
            }),
            ..EventRecord::default()
        },
    );
    if emitted.is_none() {
        anyhow::bail!("event emit failed for contract.updated");
    }
    Ok(())
}

pub fn emit_runner_results_checked(
    repo: &Path,
    run_id: &str,
    phase: Option<&str>,
    task_id: Option<&str>,
    context: &str,
    results: &[AgentResult],
) -> anyhow::Result<()> {
    if emit_runner_results_inner(repo, run_id, phase, task_id, context, results) {
        Ok(())
    } else {
        anyhow::bail!("event emit failed for runner results in {context}")
    }
}

fn emit_runner_results_inner(
    repo: &Path,
    run_id: &str,
    phase: Option<&str>,
    task_id: Option<&str>,
    context: &str,
    results: &[AgentResult],
) -> bool {
    let mut all_emitted = true;
    for result in results {
        all_emitted &= emit_runner_retries(repo, run_id, phase, task_id, context, result);
        if result.status == JobStatus::Skipped && result.error.starts_with("runner unhealthy:") {
            all_emitted &= events::safe_emit(
                repo,
                run_id,
                EventRecord {
                    event_type: "runner.healthcheck".to_string(),
                    actor_kind: "lto".to_string(),
                    actor_id: Some(result.runner.clone()),
                    phase: phase.map(str::to_string),
                    task_id: task_id.map(str::to_string),
                    object_id: Some(result.job_id.clone()),
                    object_type: Some("runner_job".to_string()),
                    summary: format!("{} unhealthy in {context}", result.runner),
                    fields: json!({
                        "runner": result.runner,
                        "model": result.model,
                        "status": result.status.as_str(),
                        "context": context,
                    }),
                    ..EventRecord::default()
                },
            )
            .is_some();
        }
        all_emitted &= events::safe_emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some(result.runner.clone()),
                phase: phase.map(str::to_string),
                task_id: task_id.map(str::to_string),
                object_id: Some(result.job_id.clone()),
                object_type: Some("runner_job".to_string()),
                summary: format!(
                    "{} {} status={} attempts={}",
                    result.runner,
                    result.job_id,
                    result.status.as_str(),
                    result.attempts
                ),
                fields: json!({
                    "runner": result.runner,
                    "model": result.model,
                    "status": result.status.as_str(),
                    "exit_code": result.exit_code,
                    "timeout": result.status == JobStatus::Timeout || result.exit_code == Some(124),
                    "attempts": result.attempts,
                    "retry_count": result.attempts.saturating_sub(1),
                    "elapsed_sec": result.cost.get("elapsed_sec").cloned().unwrap_or(Value::Null),
                    "findings_count": result.findings.len(),
                    "artifact_count": result.artifacts.len(),
                    "task_type": result.task_type,
                    "context": context,
                }),
                ..EventRecord::default()
            },
        )
        .is_some();
    }
    all_emitted
}

pub fn emit_runner_started_jobs(
    repo: &Path,
    run_id: &str,
    phase: Option<&str>,
    task_id: Option<&str>,
    context: &str,
    jobs: &[AgentJob],
) {
    for job in jobs {
        events::safe_emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.started".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some(job.runner.clone()),
                phase: phase.map(str::to_string),
                task_id: task_id.map(str::to_string),
                object_id: Some(job.job_id.clone()),
                object_type: Some("runner_job".to_string()),
                summary: format!("{} {} started for {context}", job.runner, job.job_id),
                fields: json!({
                    "runner": job.runner,
                    "model": job.model,
                    "context": context,
                    "task_type": job.task_type,
                    "timeout_sec": job.budget.timeout_sec,
                    "max_tokens": job.budget.max_tokens,
                }),
                ..EventRecord::default()
            },
        );
    }
}

pub fn emit_runner_submission_failed_jobs(
    repo: &Path,
    run_id: &str,
    phase: Option<&str>,
    task_id: Option<&str>,
    context: &str,
    jobs: &[AgentJob],
    error: &str,
) {
    for job in jobs {
        events::safe_emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some(job.runner.clone()),
                phase: phase.map(str::to_string),
                task_id: task_id.map(str::to_string),
                object_id: Some(job.job_id.clone()),
                object_type: Some("runner_job".to_string()),
                summary: format!(
                    "{} {} submission failed for {context}",
                    job.runner, job.job_id
                ),
                fields: json!({
                    "runner": job.runner,
                    "model": job.model,
                    "status": "failed",
                    "exit_code": Value::Null,
                    "timeout": false,
                    "attempts": 0,
                    "retry_count": 0,
                    "task_type": job.task_type,
                    "context": context,
                    "submission_failed": true,
                    "error_hash": hash_text(error),
                }),
                ..EventRecord::default()
            },
        );
    }
}

fn emit_runner_retries(
    repo: &Path,
    run_id: &str,
    phase: Option<&str>,
    task_id: Option<&str>,
    context: &str,
    result: &AgentResult,
) -> bool {
    let Some(attempts) = result.cost.get("retry_attempts").and_then(Value::as_array) else {
        return true;
    };
    let mut all_emitted = true;
    for attempt in attempts {
        all_emitted &= events::safe_emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.retry".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some(result.runner.clone()),
                phase: phase.map(str::to_string),
                task_id: task_id.map(str::to_string),
                object_id: Some(result.job_id.clone()),
                object_type: Some("runner_job".to_string()),
                summary: format!(
                    "{} retry attempt {} in {context}",
                    result.runner,
                    attempt.get("attempt").and_then(Value::as_u64).unwrap_or(0)
                ),
                fields: json!({
                    "runner": result.runner,
                    "model": result.model,
                    "attempt": attempt.get("attempt").cloned().unwrap_or(Value::Null),
                    "status": attempt.get("status").cloned().unwrap_or(Value::Null),
                    "exit_code": attempt.get("exit_code").cloned().unwrap_or(Value::Null),
                    "delay_sec": attempt.get("delay_sec").cloned().unwrap_or(Value::Null),
                    "context": context,
                }),
                ..EventRecord::default()
            },
        )
        .is_some();
    }
    all_emitted
}

pub fn emit_audit_dispatched(
    repo: &Path,
    run_id: &str,
    host: &str,
    auditors: &[String],
    mode: &str,
    selected: Option<&str>,
) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "audit.dispatched".to_string(),
            actor_kind: "lto".to_string(),
            object_type: Some("audit_batch".to_string()),
            summary: format!(
                "audit {mode} host={host} auditors={}",
                if auditors.is_empty() {
                    0
                } else {
                    auditors.len()
                }
            ),
            fields: json!({
                "mode": mode,
                "host": host,
                "auditors": auditors,
                "selected": selected,
                "auditor_count": auditors.len(),
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_audit_findings(
    repo: &Path,
    run_id: &str,
    source: &str,
    findings: &[Value],
    context: &str,
) {
    for finding in findings {
        let severity = finding
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let claim = finding.get("claim").and_then(Value::as_str).unwrap_or("");
        let confidence = finding.get("reported_confidence");
        let has_reported_confidence = confidence.is_some_and(|value| !value.is_null());
        let confidence_level = confidence.and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("level").and_then(Value::as_str))
        });
        let confidence_level = confidence_level
            .and_then(crate::audit::normalize_reported_confidence_level)
            .map(|level| level.as_str())
            .unwrap_or("unknown");
        let confidence_rationale = confidence
            .and_then(Value::as_object)
            .and_then(|value| value.get("rationale"))
            .and_then(Value::as_str);
        let invalidated_when = finding.get("invalidated_when").and_then(Value::as_str);
        events::safe_emit(
            repo,
            run_id,
            EventRecord {
                event_type: "audit.finding".to_string(),
                actor_kind: "auditor".to_string(),
                actor_id: Some(source.to_string()),
                object_type: Some("finding".to_string()),
                summary: format!("{source} audit finding severity={severity}"),
                fields: json!({
                    "source": source,
                    "severity": severity,
                    "claim_hash": hash_text(claim),
                    "has_file": finding.get("file").is_some(),
                    "has_reported_confidence": has_reported_confidence,
                    "confidence_level": confidence_level,
                    "has_confidence_rationale": confidence_rationale.is_some(),
                    "confidence_rationale_hash": confidence_rationale.map(hash_text),
                    "has_invalidated_when": invalidated_when.is_some(),
                    "invalidated_when_hash": invalidated_when.map(hash_text),
                    "context": context,
                }),
                ..EventRecord::default()
            },
        );
    }
}

pub fn emit_audit_round_recorded(
    repo: &Path,
    run_id: &str,
    round_label: &str,
    high: u64,
    critical: u64,
    minor: u64,
) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "audit.round.recorded".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some(round_label.to_string()),
            object_type: Some("audit_round".to_string()),
            summary: format!(
                "audit round {round_label}: high={high} critical={critical} minor={minor}"
            ),
            fields: json!({
                "round": round_label,
                "high": high,
                "critical": critical,
                "minor": minor,
                "blockers": high + critical,
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_audit_ledger_evaluated(
    repo: &Path,
    run_id: &str,
    round_label: &str,
    verdict: &str,
    terminal: &str,
    oscillation: &str,
) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "audit.ledger.evaluated".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some(round_label.to_string()),
            object_type: Some("audit_ledger".to_string()),
            summary: format!("audit ledger evaluated after {round_label}: {verdict}"),
            fields: json!({
                "round": round_label,
                "verdict": verdict,
                "terminal": terminal,
                "oscillation": oscillation,
                "strict": false,
                "source": "audit_append",
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_gate_evaluated(repo: &Path, run_id: &str, gate: &str, report: &Value) {
    let counts = status_counts(report.get("checks").and_then(Value::as_array));
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "gate.evaluated".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some(gate.to_string()),
            object_type: Some("gate".to_string()),
            summary: format!(
                "{gate}: ok={} warn={} missing={}",
                counts.ok, counts.warn, counts.missing
            ),
            fields: json!({
                "gate": gate,
                "ok": counts.ok,
                "warn": counts.warn,
                "missing": counts.missing,
                "evidence_status": report.get("evidence_status").cloned().unwrap_or(Value::Null),
                "target_phase": report.get("target_phase").cloned().unwrap_or(Value::Null),
            }),
            ..EventRecord::default()
        },
    );
    if counts.missing > 0 {
        emit_gate_blocked(
            repo,
            run_id,
            gate,
            "required gate checks missing",
            json!({"missing": counts.missing}),
        );
    }
}

pub fn emit_gate_blocked(repo: &Path, run_id: &str, gate: &str, reason: &str, fields: Value) {
    let mut payload = match fields {
        Value::Object(map) => map,
        other => Map::from_iter([("detail".to_string(), other)]),
    };
    payload.insert("gate".to_string(), json!(gate));
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "gate.blocked".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some(gate.to_string()),
            object_type: Some("gate".to_string()),
            summary: format!("{gate} blocked: {reason}"),
            fields: Value::Object(payload),
            ..EventRecord::default()
        },
    );
}

pub fn emit_budget_event(repo: &Path, run_id: &str, check: &BudgetCheck, context: &str) {
    let event_type = match check.overall {
        BudgetStatus::Exceeded => "budget.exceeded",
        BudgetStatus::Warn => "budget.warned",
        BudgetStatus::Ok => return,
    };
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: event_type.to_string(),
            actor_kind: "lto".to_string(),
            object_type: Some("budget".to_string()),
            summary: format!(
                "{context} budget status={}",
                budget_status_str(check.overall)
            ),
            fields: json!({
                "context": context,
                "overall": budget_status_str(check.overall),
                "warnings_count": check.warnings.len(),
                "dimensions": check.dimensions,
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_sandbox_rejected(repo: &Path, run_id: &str, task_id: &str, result: &SandboxResult) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "sandbox.rejected".to_string(),
            actor_kind: "lto".to_string(),
            task_id: Some(task_id.to_string()),
            object_id: Some(task_id.to_string()),
            object_type: Some("task".to_string()),
            summary: format!("sandbox rejected {task_id}: {}", result.note),
            fields: json!({
                "task_id": task_id,
                "effect": result.effect,
                "note": result.note,
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_judge_skipped(repo: &Path, run_id: &str, case_id: &str, reason: Option<&str>) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "judge.skipped".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some(case_id.to_string()),
            object_type: Some("judge_case".to_string()),
            summary: format!("judge skipped for {case_id}"),
            fields: json!({
                "case_id": case_id,
                "reason": reason,
            }),
            ..EventRecord::default()
        },
    );
}

pub fn emit_decision_voted(
    repo: &Path,
    run_id: &str,
    source: &str,
    decision_kind: &str,
    fields: Value,
) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "decision.voted".to_string(),
            actor_kind: "auditor".to_string(),
            actor_id: Some(source.to_string()),
            object_type: Some("decision".to_string()),
            summary: format!("{source} voted on {decision_kind}"),
            fields,
            ..EventRecord::default()
        },
    );
}

pub fn emit_decision_escalated(repo: &Path, run_id: &str, reason: &str, fields: Value) {
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "decision.escalated".to_string(),
            actor_kind: "lto".to_string(),
            object_type: Some("decision".to_string()),
            summary: format!("decision escalated: {reason}"),
            fields,
            ..EventRecord::default()
        },
    );
}

#[derive(Default)]
struct StatusCounts {
    ok: usize,
    warn: usize,
    missing: usize,
}

fn status_counts(checks: Option<&Vec<Value>>) -> StatusCounts {
    let mut counts = StatusCounts::default();
    for check in checks.into_iter().flatten() {
        match check.get("status").and_then(Value::as_str) {
            Some("ok") => counts.ok += 1,
            Some("missing") => counts.missing += 1,
            _ => counts.warn += 1,
        }
    }
    counts
}

fn budget_status_str(status: BudgetStatus) -> &'static str {
    match status {
        BudgetStatus::Ok => "ok",
        BudgetStatus::Warn => "warn",
        BudgetStatus::Exceeded => "exceeded",
    }
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::TaskSize;
    use std::collections::BTreeMap;

    fn create_run(repo: &Path, run_id: &str) {
        let state_path = crate::state::state_path(repo, run_id);
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(state_path, b"{}").unwrap();
    }

    #[test]
    fn runner_result_event_omits_reply_text() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        let result = AgentResult {
            job_id: "j1".to_string(),
            runner: "codex".to_string(),
            model: Some("m".to_string()),
            status: JobStatus::Ok,
            exit_code: Some(0),
            findings: vec![],
            reply_text: "SECRET_REPLY_SHOULD_NOT_APPEAR".to_string(),
            cost: BTreeMap::from([("elapsed_sec".to_string(), json!(1.2))]),
            permissions: BTreeMap::new(),
            artifacts: vec![],
            attempts: 1,
            error: String::new(),
            task_type: Some("audit".to_string()),
            size: TaskSize::Small,
            merge_review: None,
        };
        emit_runner_results_checked(
            tmp.path(),
            "r1",
            Some("implementation"),
            None,
            "test",
            &[result],
        )
        .unwrap();
        let blob = std::fs::read_to_string(crate::events::events_path(tmp.path(), "r1")).unwrap();
        assert!(blob.contains("runner.finished"));
        assert!(!blob.contains("SECRET_REPLY_SHOULD_NOT_APPEAR"));
    }

    #[test]
    fn audit_finding_event_keeps_only_confidence_level_presence_and_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        emit_audit_findings(
            tmp.path(),
            "r1",
            "pi",
            &[json!({
                "severity": "high",
                "claim": "PRIVATE CLAIM",
                "reported_confidence": {
                    "level": "medium",
                    "rationale": "PRIVATE RATIONALE"
                },
                "invalidated_when": "PRIVATE INVALIDATION"
            })],
            "audit.auto_dispatch",
        );

        let events = crate::events::read(tmp.path(), "r1").unwrap();
        let event = &events[0];
        assert_eq!(event["type"], "audit.finding");
        assert_eq!(event["fields"]["confidence_level"], "medium");
        assert_eq!(event["fields"]["has_reported_confidence"], true);
        assert_eq!(event["fields"]["has_confidence_rationale"], true);
        assert_eq!(event["fields"]["has_invalidated_when"], true);
        assert!(
            event["fields"]["confidence_rationale_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(
            event["fields"]["invalidated_when_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        let serialized = event.to_string();
        assert!(!serialized.contains("PRIVATE CLAIM"));
        assert!(!serialized.contains("PRIVATE RATIONALE"));
        assert!(!serialized.contains("PRIVATE INVALIDATION"));
    }

    #[test]
    fn audit_finding_event_normalizes_or_rejects_raw_confidence_literals() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        emit_audit_findings(
            tmp.path(),
            "r1",
            "pi",
            &[
                json!({
                    "severity": "high",
                    "claim": "capitalized",
                    "reported_confidence": "High"
                }),
                json!({
                    "severity": "medium",
                    "claim": "unsupported",
                    "reported_confidence": "extremely confident"
                }),
                json!({
                    "severity": "low",
                    "claim": "missing"
                }),
            ],
            "audit.auto_dispatch",
        );

        let events = crate::events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["fields"]["confidence_level"], "high");
        assert_eq!(events[0]["fields"]["has_reported_confidence"], true);
        assert_eq!(events[1]["fields"]["confidence_level"], "unknown");
        assert_eq!(events[1]["fields"]["has_reported_confidence"], true);
        assert_eq!(events[2]["fields"]["confidence_level"], "unknown");
        assert_eq!(events[2]["fields"]["has_reported_confidence"], false);
    }

    #[test]
    fn contract_updated_event_contains_only_changed_fields_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        emit_contract_updated(
            tmp.path(),
            "r1",
            "implementation",
            &["goal", "instruments"],
            ContractFieldCounts {
                targets: 1,
                constraints: 0,
                instruments: 2,
                forced_entropy: 0,
            },
        )
        .unwrap();

        let events = crate::events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["type"], "contract.updated");
        assert_eq!(
            event["fields"]["changed_fields"],
            json!(["goal", "instruments"])
        );
        assert_eq!(event["fields"]["target_count"], 1);
        assert_eq!(event["fields"]["constraint_count"], 0);
        assert_eq!(event["fields"]["instrument_count"], 2);
        assert_eq!(event["fields"]["forced_entropy_count"], 0);
        assert!(!event.to_string().contains("cargo test"));
    }
}
