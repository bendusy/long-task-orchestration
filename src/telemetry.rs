use crate::events;
use crate::redact::redact_text;
use crate::state::{self, LtoState};
use chrono::{DateTime, FixedOffset};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u64 = 1;

pub fn build(repo: &Path, run_id: &str) -> anyhow::Result<Value> {
    let state = state::load_state(state::state_path(repo, run_id)).unwrap_or_default();
    let events = events::read(repo, run_id)?;
    let now = state::iso_now();
    let run_metrics = run_metrics(run_id, &state, &events, &now);
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": now,
        "run_metrics": run_metrics,
        "runner_metrics": runner_metrics(&events),
        "audit_metrics": audit_metrics(&events),
        "task_metrics": task_metrics(repo, &state, &events),
        "worker_observations": [],
        "issue_metrics": {},
        "barrier_metrics": [],
        "budget": {
            "max_tokens": state.budget.max_tokens,
            "max_turns": state.budget.max_turns,
            "hard_deadline": state.budget.hard_deadline,
            "used_runner_calls": run_metrics["runner_calls"],
            "used_wall_seconds": run_metrics["wall_seconds"],
        },
        "redaction_summary": redaction_summary(&events),
        "event_log": event_log_metrics(&events, &now),
    }))
}

pub fn save(repo: &Path, run_id: &str) -> anyhow::Result<PathBuf> {
    let value = build(repo, run_id)?;
    let path = telemetry_path(repo, run_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(path)
}

pub fn telemetry_path(repo: &Path, run_id: &str) -> PathBuf {
    repo.join(".lto").join(run_id).join("telemetry.json")
}

fn run_metrics(run_id: &str, state: &LtoState, events: &[Value], now: &str) -> Value {
    let tasks = state.tasks.as_array().cloned().unwrap_or_default();
    let closed_at = events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("run.closed"))
        .and_then(|event| event.get("at"))
        .and_then(Value::as_str);
    let runner_finished = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("runner.finished"))
        .collect::<Vec<_>>();
    let timeout_count = runner_finished
        .iter()
        .filter(|event| {
            event
                .get("fields")
                .and_then(|fields| fields.get("timeout"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .count();
    json!({
        "run_id": run_id,
        "goal_label": redact_text(&state.goal).chars().take(80).collect::<String>(),
        "phase": redact_text(&state.current_phase),
        "started_at": state.started_at,
        "closed_at": closed_at,
        "wall_seconds": seconds_between(Some(state.started_at.as_str()), closed_at.or(Some(now))),
        "tasks_total": tasks.len(),
        "tasks_done": count_tasks(&tasks, "done"),
        "tasks_blocked": count_tasks(&tasks, "blocked"),
        "wip_count": count_tasks(&tasks, "in_progress"),
        "runner_calls": runner_finished.len(),
        "timeout_count": timeout_count,
        "status_transition_count": events.iter().filter(|event| event.get("type").and_then(Value::as_str) == Some("task.status_changed")).count(),
        "estimated_cost_usd": Value::Null,
    })
}

fn task_metrics(repo: &Path, state: &LtoState, events: &[Value]) -> Vec<Value> {
    state
        .tasks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|task| {
            let task_id = task.get("id").and_then(Value::as_str).unwrap_or("");
            let task_events = events
                .iter()
                .filter(|event| event.get("task_id").and_then(Value::as_str) == Some(task_id))
                .collect::<Vec<_>>();
            json!({
                "task_id": redact_text(task_id),
                "status": task.get("status").and_then(Value::as_str).map(redact_text),
                "created_at": task_events.iter().find(|event| event.get("type").and_then(Value::as_str) == Some("task.created")).and_then(|event| event.get("at")).and_then(Value::as_str),
                "last_updated_at": task.get("last_update").and_then(Value::as_str),
                "retry_count": task.get("retry_count").and_then(Value::as_u64).unwrap_or(0),
                "status_transition_count": task_events.iter().filter(|event| event.get("type").and_then(Value::as_str) == Some("task.status_changed")).count(),
                "evidence_count": task.get("evidence").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
                "touched_files": rel_files(task.get("touched_files").and_then(Value::as_array), repo),
            })
        })
        .collect()
}

#[derive(Default)]
struct RunnerRollup {
    calls: usize,
    ok: usize,
    failed: usize,
    timeout: usize,
    skipped: usize,
}

fn runner_metrics(events: &[Value]) -> Vec<Value> {
    let mut by_runner = std::collections::BTreeMap::<String, RunnerRollup>::new();
    for event in events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("runner.finished"))
    {
        let fields = event.get("fields").unwrap_or(&Value::Null);
        let runner = fields
            .get("runner")
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .get("actor")
                    .and_then(|actor| actor.get("id"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("unknown");
        let status = fields
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| match fields.get("rc").and_then(Value::as_i64) {
                Some(0) => "ok".to_string(),
                Some(124) => "timeout".to_string(),
                Some(_) => "failed".to_string(),
                None => "unknown".to_string(),
            });
        let rollup = by_runner.entry(runner.to_string()).or_default();
        rollup.calls += 1;
        match status.as_str() {
            "ok" => rollup.ok += 1,
            "timeout" => {
                rollup.timeout += 1;
                rollup.failed += 1;
            }
            "skipped" => rollup.skipped += 1,
            _ => rollup.failed += 1,
        }
    }
    by_runner
        .into_iter()
        .map(|(runner, slot)| {
            let failure_rate = if slot.calls == 0 {
                0.0
            } else {
                slot.failed as f64 / slot.calls as f64
            };
            json!({
                "runner": runner,
                "calls": slot.calls,
                "ok": slot.ok,
                "failed": slot.failed,
                "timeout": slot.timeout,
                "skipped": slot.skipped,
                "failure_rate": failure_rate,
            })
        })
        .collect()
}

fn audit_metrics(events: &[Value]) -> Value {
    let dispatched = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("audit.dispatched"))
        .count();
    let rounds = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("audit.converged"))
        .collect::<Vec<_>>();
    let mut severity_counts = std::collections::BTreeMap::<String, usize>::new();
    for event in events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("audit.finding"))
    {
        let severity = event
            .get("fields")
            .and_then(|fields| fields.get("severity"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        *severity_counts.entry(severity.to_string()).or_default() += 1;
    }
    let latest_blockers = rounds
        .last()
        .and_then(|event| event.get("fields"))
        .and_then(|fields| fields.get("blockers"))
        .and_then(Value::as_u64);
    json!({
        "audit_dispatches": dispatched,
        "audit_rounds": rounds.len(),
        "latest_blockers": latest_blockers,
        "findings": {
            "total": severity_counts.values().sum::<usize>(),
            "by_severity": severity_counts,
        },
    })
}

fn count_tasks(tasks: &[Value], status: &str) -> usize {
    tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some(status))
        .count()
}

fn rel_files(files: Option<&Vec<Value>>, repo: &Path) -> Vec<String> {
    files
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| {
            let candidate = Path::new(path);
            if candidate.is_absolute() {
                candidate
                    .strip_prefix(repo)
                    .map(|rel| rel.to_string_lossy().to_string())
                    .unwrap_or_else(|_| redact_text(path))
            } else {
                redact_text(path)
            }
        })
        .collect()
}

fn redaction_summary(events: &[Value]) -> Value {
    let mut passed = 0;
    let mut failed = 0;
    let mut not_required = 0;
    for event in events {
        match event
            .get("privacy")
            .and_then(|value| value.get("redaction_status"))
            .and_then(Value::as_str)
        {
            Some("passed") => passed += 1,
            Some("failed") => failed += 1,
            Some("not_required") => not_required += 1,
            _ => {}
        }
    }
    json!({"passed": passed, "failed": failed, "not_required": not_required})
}

fn event_log_metrics(events: &[Value], now: &str) -> Value {
    let last_at = events
        .last()
        .and_then(|event| event.get("at"))
        .and_then(Value::as_str);
    json!({
        "event_count": events.len(),
        "seconds_since_last_event": seconds_between(last_at, Some(now)),
    })
}

fn seconds_between(start: Option<&str>, end: Option<&str>) -> Option<i64> {
    let start = parse_iso(start?)?;
    let end = parse_iso(end?)?;
    Some((end - start).num_seconds())
}

fn parse_iso(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventRecord, emit};
    use crate::state::{LtoState, WorkspaceSnapshot};

    #[test]
    fn telemetry_is_derived_without_recommendations() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";
        let run_dir = repo.join(".lto").join(run_id);
        fs::create_dir_all(&run_dir).unwrap();
        let state = LtoState {
            run_id: run_id.to_string(),
            goal: "ship without /Users/ben/private token sk-123456789012".to_string(),
            workspace: WorkspaceSnapshot {
                repo_root: repo.display().to_string(),
                ..WorkspaceSnapshot::default()
            },
            tasks: json!([{"id":"T1","status":"done","touched_files":["src/lib.rs"],"evidence":[{}]}]),
            ..LtoState::default()
        };
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                fields: json!({"runner": "codex", "status": "timeout", "timeout": true}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("pi".to_string()),
                fields: json!({"runner": "pi", "status": "failed", "timeout": false}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "audit.finding".to_string(),
                actor_kind: "auditor".to_string(),
                actor_id: Some("pi".to_string()),
                fields: json!({"severity": "high"}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "audit.converged".to_string(),
                actor_kind: "lto".to_string(),
                fields: json!({"blockers": 1}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        let path = save(repo, run_id).unwrap();
        let blob = fs::read_to_string(path).unwrap();
        assert!(!blob.contains("control_recommendations"));
        assert!(!blob.contains("/Users/ben/private"));
        assert!(!blob.contains("sk-123456789012"));
        let value = build(repo, run_id).unwrap();
        assert_eq!(value["run_metrics"]["runner_calls"], 2);
        assert_eq!(value["run_metrics"]["timeout_count"], 1);
        assert_eq!(value["runner_metrics"][0]["runner"], "codex");
        assert_eq!(value["runner_metrics"][0]["timeout"], 1);
        assert_eq!(value["runner_metrics"][1]["runner"], "pi");
        assert_eq!(value["runner_metrics"][0]["failure_rate"], 1.0);
        assert_eq!(value["audit_metrics"]["audit_rounds"], 1);
        assert_eq!(value["audit_metrics"]["findings"]["by_severity"]["high"], 1);
        assert_eq!(value["event_log"]["event_count"], 4);
    }
}
