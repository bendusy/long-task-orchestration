use crate::events;
use crate::redact::redact_text;
use crate::state::{self, LtoState};
use chrono::{DateTime, FixedOffset};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
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
    let mut by_runner = BTreeMap::<String, RunnerRollup>::new();
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrossRunMining {
    pub run_count: usize,
    pub entries: Vec<CrossRunMiningEntry>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrossRunMiningEntry {
    pub runner: String,
    pub model: String,
    pub task_type: String,
    pub time_window: String,
    pub dispatches: usize,
    pub ok: usize,
    pub failed: usize,
    pub timeout: usize,
    pub rate_limited: usize,
    pub skipped: usize,
    pub avg_elapsed_sec: Option<f64>,
    pub avg_retry: Option<f64>,
    pub avg_audit_rounds: Option<f64>,
    pub agent_dispatch_completed: usize,
    pub distinct_runs: usize,
    pub subjective_non_measurement: bool,
}

#[derive(Debug, Clone, Default)]
struct CrossRunSlot {
    ok_runs: BTreeSet<String>,
    failed_runs: BTreeSet<String>,
    timeout_runs: BTreeSet<String>,
    rate_limited_runs: BTreeSet<String>,
    skipped_runs: BTreeSet<String>,
    elapsed_by_run: BTreeMap<String, f64>,
    retry_by_run: BTreeMap<String, u64>,
    completed_runs: BTreeSet<String>,
    distinct_runs: BTreeSet<String>,
    subjective_runs: BTreeSet<String>,
}

pub fn discover_run_ids(repo: &Path) -> anyhow::Result<Vec<String>> {
    let lto = repo.join(".lto");
    if !lto.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in fs::read_dir(lto)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("state.json").exists() {
            runs.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    runs.sort();
    Ok(runs)
}

pub fn cross_run_mining(repo: &Path) -> anyhow::Result<CrossRunMining> {
    let run_ids = discover_run_ids(repo)?;
    let mut audit_rounds_by_run = BTreeMap::<String, usize>::new();
    let mut mined_run_ids = BTreeSet::<String>::new();
    let mut subjective_runs = BTreeSet::<String>::new();
    let mut slots = BTreeMap::<(String, String, String, String), CrossRunSlot>::new();

    for run_id in &run_ids {
        let events = match events::read(repo, run_id) {
            Ok(events) => events,
            Err(_) => continue,
        };
        let audit_rounds = normalized_audit_rounds(&events).len();
        audit_rounds_by_run.insert(run_id.clone(), audit_rounds);

        if events
            .iter()
            .any(|event| matches!(event_type(event), "decision.voted" | "judge.skipped"))
        {
            subjective_runs.insert(run_id.clone());
        }

        let model_hints = model_hints_for_run(&events);
        for event in events {
            match event_type(&event) {
                "runner.finished" => {
                    mined_run_ids.insert(run_id.clone());
                    record_runner_finished(&mut slots, run_id, &event);
                }
                "agent.dispatch.completed" => {
                    mined_run_ids.insert(run_id.clone());
                    record_agent_dispatch_completed(&mut slots, run_id, &event, &model_hints);
                }
                _ => {}
            }
        }
    }

    for slot in slots.values_mut() {
        for run_id in &slot.distinct_runs {
            if subjective_runs.contains(run_id) {
                slot.subjective_runs.insert(run_id.clone());
            }
        }
    }

    let entries = slots
        .into_iter()
        .map(|((runner, model, task_type, time_window), slot)| {
            let dispatches = slot.distinct_runs.len();
            let failed = slot.failed_runs.len();
            let total_audit_rounds = slot
                .distinct_runs
                .iter()
                .map(|run_id| audit_rounds_by_run.get(run_id).copied().unwrap_or(0))
                .sum::<usize>();
            CrossRunMiningEntry {
                runner,
                model,
                task_type,
                time_window,
                dispatches,
                ok: slot.ok_runs.len(),
                failed,
                timeout: slot.timeout_runs.len(),
                rate_limited: slot.rate_limited_runs.len(),
                skipped: slot.skipped_runs.len(),
                avg_elapsed_sec: average_f64(slot.elapsed_by_run.values().copied()),
                avg_retry: average_f64(slot.retry_by_run.values().map(|value| *value as f64)),
                avg_audit_rounds: if dispatches == 0 {
                    None
                } else {
                    Some(total_audit_rounds as f64 / dispatches as f64)
                },
                agent_dispatch_completed: slot.completed_runs.len(),
                distinct_runs: dispatches,
                subjective_non_measurement: !slot.subjective_runs.is_empty(),
            }
        })
        .collect();

    Ok(CrossRunMining {
        run_count: mined_run_ids.len(),
        entries,
    })
}

fn record_runner_finished(
    slots: &mut BTreeMap<(String, String, String, String), CrossRunSlot>,
    run_id: &str,
    event: &Value,
) {
    let runner = runner_from_event(event);
    let key = mining_key(event, &runner);
    let slot = slots.entry(key).or_default();
    slot.distinct_runs.insert(run_id.to_string());
    match runner_status(event).as_str() {
        "ok" => {
            slot.ok_runs.insert(run_id.to_string());
        }
        "timeout" => {
            slot.timeout_runs.insert(run_id.to_string());
            slot.failed_runs.insert(run_id.to_string());
        }
        "rate_limited" => {
            slot.rate_limited_runs.insert(run_id.to_string());
            slot.failed_runs.insert(run_id.to_string());
        }
        "skipped" => {
            slot.skipped_runs.insert(run_id.to_string());
        }
        _ => {
            slot.failed_runs.insert(run_id.to_string());
        }
    }
    if let Some(elapsed) = event
        .get("fields")
        .and_then(|fields| fields.get("elapsed_sec"))
        .and_then(Value::as_f64)
    {
        slot.elapsed_by_run
            .entry(run_id.to_string())
            .or_insert(elapsed);
    }
    if let Some(retries) = event
        .get("fields")
        .and_then(|fields| fields.get("retry_count"))
        .and_then(Value::as_u64)
    {
        slot.retry_by_run
            .entry(run_id.to_string())
            .or_insert(retries);
    }
}

fn record_agent_dispatch_completed(
    slots: &mut BTreeMap<(String, String, String, String), CrossRunSlot>,
    run_id: &str,
    event: &Value,
    model_hints: &BTreeMap<(String, String, String), Option<String>>,
) {
    let runner = runner_from_event(event);
    let key = mining_key_with_model(
        event,
        &runner,
        model_for_completed(event, &runner, model_hints),
    );
    let slot = slots.entry(key).or_default();
    slot.completed_runs.insert(run_id.to_string());
    slot.distinct_runs.insert(run_id.to_string());
    match runner_status(event).as_str() {
        "ok" => {
            slot.ok_runs.insert(run_id.to_string());
        }
        "timeout" => {
            slot.timeout_runs.insert(run_id.to_string());
            slot.failed_runs.insert(run_id.to_string());
        }
        "rate_limited" => {
            slot.rate_limited_runs.insert(run_id.to_string());
            slot.failed_runs.insert(run_id.to_string());
        }
        "failed" => {
            slot.failed_runs.insert(run_id.to_string());
        }
        _ => {}
    }
    if let Some(elapsed) = event
        .get("fields")
        .and_then(|fields| fields.get("elapsed_sec"))
        .and_then(Value::as_f64)
    {
        slot.elapsed_by_run
            .entry(run_id.to_string())
            .or_insert(elapsed);
    }
}

fn runner_from_event(event: &Value) -> String {
    event
        .get("fields")
        .and_then(|fields| fields.get("runner"))
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("actor")
                .and_then(|actor| actor.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string()
}

fn runner_status(event: &Value) -> String {
    let fields = event.get("fields").unwrap_or(&Value::Null);
    fields
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| match fields.get("rc").and_then(Value::as_i64) {
            Some(0) => "ok".to_string(),
            Some(124) => "timeout".to_string(),
            Some(_) => "failed".to_string(),
            None => "unknown".to_string(),
        })
}

fn model_hints_for_run(events: &[Value]) -> BTreeMap<(String, String, String), Option<String>> {
    let mut hints = BTreeMap::new();
    for event in events {
        if event_type(event) != "runner.finished" {
            continue;
        }
        let model = model_from_event(event);
        if model == "unknown" {
            continue;
        }
        let runner = runner_from_event(event);
        let key = task_slot_key(event, &runner);
        hints
            .entry(key)
            .and_modify(|existing: &mut Option<String>| {
                if existing.as_ref() != Some(&model) {
                    *existing = None;
                }
            })
            .or_insert(Some(model));
    }
    hints
}

fn model_from_event(event: &Value) -> String {
    event
        .get("fields")
        .and_then(|fields| fields.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn model_for_completed(
    event: &Value,
    runner: &str,
    model_hints: &BTreeMap<(String, String, String), Option<String>>,
) -> String {
    let model = model_from_event(event);
    if model != "unknown" {
        return model;
    }
    model_hints
        .get(&task_slot_key(event, runner))
        .and_then(|model| model.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn mining_key(event: &Value, runner: &str) -> (String, String, String, String) {
    mining_key_with_model(event, runner, model_from_event(event))
}

fn mining_key_with_model(
    event: &Value,
    runner: &str,
    model: String,
) -> (String, String, String, String) {
    let (runner, task_type, time_window) = task_slot_key(event, runner);
    (runner, model, task_type, time_window)
}

fn task_slot_key(event: &Value, runner: &str) -> (String, String, String) {
    (
        runner.to_string(),
        task_type(
            event.get("task_id").and_then(Value::as_str),
            event.get("phase").and_then(Value::as_str),
        ),
        time_window(event.get("at").and_then(Value::as_str)),
    )
}

fn task_type(task_id: Option<&str>, phase: Option<&str>) -> String {
    let name = task_id.unwrap_or(phase.unwrap_or("unknown")).to_lowercase();
    if name.contains("audit") {
        "audit".to_string()
    } else if name.contains("verify") || name.contains("test") {
        "verify".to_string()
    } else if name.contains("doc") || name.contains("readme") {
        "doc".to_string()
    } else if name.contains("impl") || name.contains("coding") || name.starts_with('l') {
        "implementation".to_string()
    } else {
        "other".to_string()
    }
}

fn time_window(at: Option<&str>) -> String {
    at.and_then(|value| value.get(0..10))
        .unwrap_or("all")
        .to_string()
}

fn event_type(event: &Value) -> &str {
    event.get("type").and_then(Value::as_str).unwrap_or("")
}

const LEGACY_AUDIT_ROUND_EVENT: &str = "audit.converged"; // legacy read compatibility

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AuditRoundKey {
    Round(String),
    EventId(u64),
    Position(usize),
}

fn normalized_audit_rounds(events: &[Value]) -> Vec<&Value> {
    let mut by_key = BTreeMap::<AuditRoundKey, (usize, &Value)>::new();
    for (position, event) in events.iter().enumerate() {
        if !matches!(
            event_type(event),
            LEGACY_AUDIT_ROUND_EVENT | "audit.round.recorded"
        ) {
            continue;
        }
        let key = audit_round_key(event, position);
        by_key.insert(key, (position, event));
    }
    let mut rounds = by_key.into_values().collect::<Vec<_>>();
    rounds.sort_by_key(|(position, _)| *position);
    rounds.into_iter().map(|(_, event)| event).collect()
}

fn audit_round_key(event: &Value, position: usize) -> AuditRoundKey {
    normalized_round_identity(event)
        .map(AuditRoundKey::Round)
        .or_else(|| {
            event
                .get("event_id")
                .and_then(Value::as_u64)
                .map(AuditRoundKey::EventId)
        })
        .unwrap_or(AuditRoundKey::Position(position))
}

fn normalized_round_identity(event: &Value) -> Option<String> {
    normalize_round_label(event.get("object_id").and_then(Value::as_str)).or_else(|| {
        normalize_round_label(
            event
                .get("fields")
                .and_then(|fields| fields.get("round"))
                .and_then(Value::as_str),
        )
    })
}

fn normalize_round_label(label: Option<&str>) -> Option<String> {
    label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_ascii_lowercase)
}

fn average_f64(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut count = 0usize;
    let mut sum = 0.0;
    for value in values {
        count += 1;
        sum += value;
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

fn audit_metrics(events: &[Value]) -> Value {
    let dispatched = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("audit.dispatched"))
        .count();
    let rounds = normalized_audit_rounds(events);
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
                event_type: "audit.round.recorded".to_string(),
                actor_kind: "lto".to_string(),
                object_id: Some("R1".to_string()),
                fields: json!({"round": "R1", "blockers": 1}),
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

    #[test]
    fn audit_rounds_normalize_old_only_events() {
        let metrics = audit_metrics(&[
            json!({
                "event_id": 1,
                "type": LEGACY_AUDIT_ROUND_EVENT,
                "object_id": "R1",
                "fields": {"round": "R1", "blockers": 2}
            }),
            json!({
                "event_id": 2,
                "type": LEGACY_AUDIT_ROUND_EVENT,
                "object_id": "R2",
                "fields": {"round": "R2", "blockers": 1}
            }),
        ]);

        assert_eq!(metrics["audit_rounds"], 2);
        assert_eq!(metrics["latest_blockers"], 1);
    }

    #[test]
    fn audit_rounds_normalize_new_only_events_and_ignore_evaluations() {
        let metrics = audit_metrics(&[
            json!({
                "event_id": 1,
                "type": "audit.round.recorded",
                "object_id": "R1",
                "fields": {"round": "R1", "blockers": 2}
            }),
            json!({
                "event_id": 2,
                "type": "audit.ledger.evaluated",
                "object_id": "audit-ledger",
                "fields": {"verdict": "CONVERGING"}
            }),
            json!({
                "event_id": 3,
                "type": "audit.round.recorded",
                "fields": {"round": "R2", "blockers": 0}
            }),
        ]);

        assert_eq!(metrics["audit_rounds"], 2);
        assert_eq!(metrics["latest_blockers"], 0);
    }

    #[test]
    fn audit_rounds_deduplicate_mixed_same_round_last_wins() {
        let metrics = audit_metrics(&[
            json!({
                "event_id": 1,
                "type": LEGACY_AUDIT_ROUND_EVENT,
                "object_id": " R1 ",
                "fields": {"blockers": 3}
            }),
            json!({
                "event_id": 2,
                "type": "audit.round.recorded",
                "fields": {"round": "r1", "blockers": 0}
            }),
        ]);

        assert_eq!(metrics["audit_rounds"], 1);
        assert_eq!(metrics["latest_blockers"], 0);
    }

    #[test]
    fn audit_rounds_keep_mixed_different_rounds() {
        let metrics = audit_metrics(&[
            json!({
                "event_id": 1,
                "type": LEGACY_AUDIT_ROUND_EVENT,
                "object_id": "R1",
                "fields": {"blockers": 2}
            }),
            json!({
                "event_id": 2,
                "type": "audit.round.recorded",
                "fields": {"round": "R2", "blockers": 1}
            }),
        ]);

        assert_eq!(metrics["audit_rounds"], 2);
        assert_eq!(metrics["latest_blockers"], 1);
    }

    #[test]
    fn audit_rounds_fall_back_to_event_id_or_position_without_round_identity() {
        let events = vec![
            json!({
                "event_id": 7,
                "type": LEGACY_AUDIT_ROUND_EVENT,
                "fields": {"blockers": 3}
            }),
            json!({
                "event_id": 7,
                "type": "audit.round.recorded",
                "fields": {"blockers": 1}
            }),
            json!({
                "type": "audit.round.recorded",
                "fields": {"blockers": 0}
            }),
            json!({
                "type": "audit.ledger.evaluated",
                "fields": {"verdict": "CONVERGED"}
            }),
        ];

        let rounds = normalized_audit_rounds(&events);
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0]["fields"]["blockers"], 1);
        assert_eq!(rounds[1]["fields"]["blockers"], 0);
    }

    #[test]
    fn cross_run_mining_counts_new_audit_round_events() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            })
            .unwrap(),
        )
        .unwrap();
        let event_lines = [
            json!({
                "event_id": 1,
                "run_id": "r1",
                "type": "runner.finished",
                "task_id": "L3",
                "fields": {"runner": "codex", "status": "ok"}
            }),
            json!({
                "event_id": 2,
                "run_id": "r1",
                "type": "audit.round.recorded",
                "object_id": "R1",
                "fields": {"round": "R1", "blockers": 1}
            }),
            json!({
                "event_id": 3,
                "run_id": "r1",
                "type": "audit.ledger.evaluated",
                "fields": {"verdict": "CONVERGING"}
            }),
            json!({
                "event_id": 4,
                "run_id": "r1",
                "type": "audit.round.recorded",
                "object_id": "R2",
                "fields": {"round": "R2", "blockers": 0}
            }),
        ]
        .into_iter()
        .map(|event| event.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        fs::write(run_dir.join("events.jsonl"), event_lines + "\n").unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let entry = mining
            .entries
            .iter()
            .find(|entry| entry.runner == "codex")
            .unwrap();
        assert_eq!(entry.avg_audit_rounds, Some(2.0));
    }

    #[test]
    fn cross_run_mining_groups_by_runner_task_and_distinct_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        for run_id in ["r1", "r2"] {
            let run_dir = repo.join(".lto").join(run_id);
            fs::create_dir_all(&run_dir).unwrap();
            fs::write(
                run_dir.join("state.json"),
                serde_json::to_string_pretty(&LtoState {
                    run_id: run_id.to_string(),
                    ..LtoState::default()
                })
                .unwrap(),
            )
            .unwrap();
        }
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "status": "ok", "elapsed_sec": 10.0}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "status": "failed", "elapsed_sec": 99.0}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            "r2",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "status": "failed", "retry_count": 1}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            "r2",
            EventRecord {
                event_type: "agent.dispatch.completed".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "rc": 0}),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let entry = mining
            .entries
            .iter()
            .find(|entry| entry.runner == "codex" && entry.task_type == "implementation")
            .unwrap();
        assert_eq!(entry.model, "unknown");
        assert_eq!(entry.distinct_runs, 2);
        assert_eq!(entry.failed, 2);
        assert_eq!(entry.ok, 2);
        assert_eq!(entry.agent_dispatch_completed, 1);
        assert_eq!(entry.avg_retry, Some(1.0));
    }

    #[test]
    fn cross_run_mining_splits_same_runner_task_by_model() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        for run_id in ["r1", "r2"] {
            let run_dir = repo.join(".lto").join(run_id);
            fs::create_dir_all(&run_dir).unwrap();
            fs::write(
                run_dir.join("state.json"),
                serde_json::to_string_pretty(&LtoState {
                    run_id: run_id.to_string(),
                    ..LtoState::default()
                })
                .unwrap(),
            )
            .unwrap();
        }
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "model": "gpt-5", "status": "ok"}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            "r2",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "model": "gpt-5.5", "status": "failed"}),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let mut models = mining
            .entries
            .iter()
            .filter(|entry| entry.runner == "codex" && entry.task_type == "implementation")
            .map(|entry| (entry.model.as_str(), entry.ok, entry.failed))
            .collect::<Vec<_>>();
        models.sort();
        assert_eq!(models, vec![("gpt-5", 1, 0), ("gpt-5.5", 0, 1)]);
    }

    #[test]
    fn cross_run_mining_infers_missing_turn_model_from_unique_run_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            })
            .unwrap(),
        )
        .unwrap();
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "model": "gpt-5", "status": "ok"}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "agent.dispatch.completed".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "rc": 0}),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let mut models = mining
            .entries
            .iter()
            .filter(|entry| entry.runner == "codex" && entry.task_type == "implementation")
            .map(|entry| {
                (
                    entry.model.as_str(),
                    entry.ok,
                    entry.failed,
                    entry.agent_dispatch_completed,
                )
            })
            .collect::<Vec<_>>();
        models.sort();
        assert_eq!(models, vec![("gpt-5", 1, 0, 1)]);
    }

    #[test]
    fn cross_run_mining_keeps_missing_turn_model_unknown_when_run_slot_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            })
            .unwrap(),
        )
        .unwrap();
        for model in ["gpt-5", "gpt-5.5"] {
            emit(
                repo,
                "r1",
                EventRecord {
                    event_type: "runner.finished".to_string(),
                    actor_kind: "runner".to_string(),
                    actor_id: Some("codex".to_string()),
                    task_id: Some("L3".to_string()),
                    fields: json!({"runner": "codex", "model": model, "status": "ok"}),
                    ..EventRecord::default()
                },
            )
            .unwrap();
        }
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "agent.dispatch.completed".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                task_id: Some("L3".to_string()),
                fields: json!({"runner": "codex", "rc": 0}),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let mut models = mining
            .entries
            .iter()
            .filter(|entry| entry.runner == "codex" && entry.task_type == "implementation")
            .map(|entry| {
                (
                    entry.model.as_str(),
                    entry.ok,
                    entry.failed,
                    entry.agent_dispatch_completed,
                )
            })
            .collect::<Vec<_>>();
        models.sort();
        assert_eq!(
            models,
            vec![
                ("gpt-5", 1, 0, 0),
                ("gpt-5.5", 1, 0, 0),
                ("unknown", 1, 0, 1),
            ]
        );
    }

    #[test]
    fn cross_run_mining_marks_completed_nonzero_rc_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            })
            .unwrap(),
        )
        .unwrap();
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "agent.dispatch.completed".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("pi".to_string()),
                phase: Some("implementation".to_string()),
                fields: json!({"runner": "pi", "rc": 1}),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let entry = mining
            .entries
            .iter()
            .find(|entry| entry.runner == "pi")
            .unwrap();
        assert_eq!(entry.model, "unknown");
        assert_eq!(entry.task_type, "implementation");
        assert_eq!(entry.distinct_runs, 1);
        assert_eq!(entry.failed, 1);
        assert_eq!(entry.agent_dispatch_completed, 1);
    }

    #[test]
    fn cross_run_mining_tracks_rate_limited_runner_results() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("state.json"),
            serde_json::to_string_pretty(&LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            })
            .unwrap(),
        )
        .unwrap();
        emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                phase: Some("implementation".to_string()),
                fields: json!({
                    "runner": "codex",
                    "model": "gpt-5",
                    "status": "rate_limited"
                }),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let mining = cross_run_mining(repo).unwrap();
        let entry = mining
            .entries
            .iter()
            .find(|entry| entry.runner == "codex")
            .unwrap();
        assert_eq!(entry.rate_limited, 1);
        assert_eq!(entry.failed, 1);
    }
}
