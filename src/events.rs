use crate::redact::{redact_text, redact_value};
use crate::state;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const SCHEMA_VERSION: u64 = 1;
pub const WARN_AT: usize = 10_000;
pub const HARD_STOP_AT: usize = 50_000;

const PHASE1_EVENT_TYPES: &[&str] = &[
    "run.started",
    "run.closed",
    "phase.changed",
    "task.created",
    "task.status_changed",
    "runner.started",
    "runner.finished",
    "artifact.registered",
];

const ACTOR_KINDS: &[&str] = &["host", "lto", "runner", "auditor", "human"];

#[derive(Debug, Clone, Default)]
pub struct EventRecord {
    pub event_type: String,
    pub actor_kind: String,
    pub actor_id: Option<String>,
    pub phase: Option<String>,
    pub task_id: Option<String>,
    pub object_id: Option<String>,
    pub object_type: Option<String>,
    pub summary: String,
    pub artifact_refs: Vec<String>,
    pub contains_raw_output: bool,
    pub fields: Value,
    pub force: bool,
}

pub fn safe_emit(repo: &Path, run_id: &str, record: EventRecord) -> Option<Value> {
    match emit(repo, run_id, record) {
        Ok(event) => Some(event),
        Err(err) => {
            eprintln!("warning: event emit failed: {err}");
            None
        }
    }
}

pub fn emit(repo: &Path, run_id: &str, record: EventRecord) -> anyhow::Result<Value> {
    state::validate_run_id(run_id)?;
    if !PHASE1_EVENT_TYPES.contains(&record.event_type.as_str()) {
        anyhow::bail!("invalid or deferred event type: {}", record.event_type);
    }
    if !ACTOR_KINDS.contains(&record.actor_kind.as_str()) {
        anyhow::bail!("invalid actor kind: {}", record.actor_kind);
    }
    if record.contains_raw_output {
        anyhow::bail!("contains_raw_output events are forbidden; store output as artifact");
    }

    let path = events_path(repo, run_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = acquire_events_lock(&path)?;
    let count = count_file_events(&path)?;
    if count >= HARD_STOP_AT && !record.force {
        anyhow::bail!("event log hard stop at {HARD_STOP_AT} events ({count} present)");
    }
    if count >= WARN_AT {
        eprintln!("warning: event log has {count} events (warn threshold {WARN_AT})");
    }
    let mut event = json!({
        "schema_version": SCHEMA_VERSION,
        "event_id": count + 1,
        "run_id": run_id,
        "at": state::iso_now(),
        "type": record.event_type,
        "actor": {
            "kind": record.actor_kind,
            "id": record.actor_id.as_deref().map(redact_text),
        },
        "phase": record.phase.as_deref().map(redact_text),
        "task_id": record.task_id.as_deref().map(redact_text),
        "object_id": record.object_id.as_deref().map(redact_text),
        "object_type": record.object_type.as_deref().map(redact_text),
        "summary": redact_text(&record.summary),
        "artifact_refs": record.artifact_refs.iter().map(|item| redact_text(item)).collect::<Vec<_>>(),
        "privacy": {
            "contains_raw_output": false,
            "redaction_status": if record.summary.is_empty() && record.fields.is_null() { "not_required" } else { "passed" },
        },
    });
    let fields = redact_value(&record.fields);
    if !fields.is_null() && !fields.as_object().is_some_and(serde_json::Map::is_empty) {
        event["fields"] = fields;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    Ok(event)
}

pub fn read(repo: &Path, run_id: &str) -> anyhow::Result<Vec<Value>> {
    state::validate_run_id(run_id)?;
    let path = events_path(repo, run_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    let mut seen_ids = std::collections::HashSet::new();
    let mut events = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(item) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(event_id) = item.get("event_id").and_then(Value::as_u64)
            && !seen_ids.insert(event_id)
        {
            eprintln!("warning: duplicate event_id {event_id} in {run_id}; keeping first");
            continue;
        }
        events.push(item);
    }
    Ok(events)
}

pub fn count(repo: &Path, run_id: &str) -> anyhow::Result<usize> {
    count_file_events(&events_path(repo, run_id))
}

pub fn events_path(repo: &Path, run_id: &str) -> PathBuf {
    repo.join(".lto").join(run_id).join("events.jsonl")
}

fn count_file_events(path: &Path) -> anyhow::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

struct EventsLockGuard {
    path: PathBuf,
}

impl Drop for EventsLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_events_lock(path: &Path) -> anyhow::Result<Option<EventsLockGuard>> {
    let lock_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".events.lock");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => return Ok(Some(EventsLockGuard { path: lock_path })),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    eprintln!("warning: events lock timeout; proceeding best-effort");
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_redacted_append_only_events_and_reads_unknown_types() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let event = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some("codex".to_string()),
                summary: "done at /Users/ben/private with sk-123456789012".to_string(),
                fields: json!({"rc": 0, "stdout": "must not persist"}),
                ..EventRecord::default()
            },
        )
        .unwrap();
        assert_eq!(event["event_id"], 1);
        let blob = fs::read_to_string(events_path(repo, "r1")).unwrap();
        assert!(blob.contains("[REDACTED_PATH]"));
        assert!(blob.contains("[REDACTED_SECRET]"));
        assert!(!blob.contains("must not persist"));
        fs::write(
            events_path(repo, "r1"),
            format!("{blob}{}\n", json!({"type":"future.event"})),
        )
        .unwrap();
        assert_eq!(read(repo, "r1").unwrap().len(), 2);
    }

    #[test]
    fn rejects_raw_output_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let err = emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                contains_raw_output: true,
                ..EventRecord::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("contains_raw_output"));
    }

    #[test]
    fn concurrent_appends_get_unique_event_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let handles = (0..32)
            .map(|idx| {
                let repo = repo.clone();
                std::thread::spawn(move || {
                    emit(
                        &repo,
                        "r1",
                        EventRecord {
                            event_type: "artifact.registered".to_string(),
                            actor_kind: "lto".to_string(),
                            summary: format!("artifact {idx}"),
                            ..EventRecord::default()
                        },
                    )
                    .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
        let mut ids = read(&repo, "r1")
            .unwrap()
            .into_iter()
            .map(|event| event["event_id"].as_u64().unwrap())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, (1..=32).collect::<Vec<_>>());
    }

    #[test]
    fn read_keeps_first_duplicate_event_id() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            [
                json!({"event_id":1,"type":"run.started","summary":"first"}).to_string(),
                json!({"event_id":1,"type":"run.started","summary":"duplicate"}).to_string(),
                json!({"event_id":2,"type":"run.closed","summary":"second"}).to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let events = read(tmp.path(), "r1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["summary"], "first");
        assert_eq!(events[1]["summary"], "second");
    }
}
