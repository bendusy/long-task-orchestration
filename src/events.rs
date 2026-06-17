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

pub const KNOWN_EVENT_TYPES: &[&str] = &[
    "run.started",
    "run.closed",
    "phase.changed",
    "task.created",
    "task.status_changed",
    "runner.started",
    "runner.finished",
    "runner.retry",
    "runner.healthcheck",
    "agent.turn.completed",
    "artifact.registered",
    "audit.dispatched",
    "audit.finding",
    "audit.converged",
    "gate.evaluated",
    "gate.blocked",
    "budget.warned",
    "budget.exceeded",
    "sandbox.rejected",
    "judge.skipped",
    "decision.voted",
    "decision.escalated",
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
    if !KNOWN_EVENT_TYPES.contains(&record.event_type.as_str()) {
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
    let count = read_event_counter(&path)?;
    if count >= HARD_STOP_AT && !record.force {
        anyhow::bail!("event log hard stop at {HARD_STOP_AT} events ({count} present)");
    }
    if count >= WARN_AT {
        eprintln!("warning: event log has {count} events (warn threshold {WARN_AT})");
    }
    let new_event_id = count + 1;
    // Persist counter BEFORE writing the event. If the process crashes after
    // the counter write but before the event append, the event_id gap is
    // harmless (monotonic, no duplicates). The alternative (counter last)
    // risks duplicate event_ids on crash — worse.
    write_event_counter(&path, new_event_id)?;
    let mut event = json!({
        "schema_version": SCHEMA_VERSION,
        "event_id": new_event_id,
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
    // Single write_all of the line+newline (backlog ⑫): writeln! emits the JSON
    // and the '\n' as two separate write() syscalls; one buffered write_all keeps
    // the record atomic under O_APPEND, so even a future concurrent writer cannot
    // interleave mid-line. Belt-and-suspenders with the fail-closed lock above.
    let mut line = serde_json::to_string(&event)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())?;
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
    read_event_counter(&events_path(repo, run_id))
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

fn events_counter_path(events_path: &Path) -> PathBuf {
    events_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".events.count")
}

/// Read the current event count.
/// If the counter file exists (and is valid), returns its value — O(1).
/// If the counter file is absent, falls back to counting events.jsonl
/// lines — O(N), one-time until the next emit persists the counter.
/// If the counter file exists but is corrupted, deletes it and falls back
/// to counting events.jsonl (self-healing).
/// Pure read — never writes the counter file. Callers that need to
/// persist a new count must call write_event_counter under the lock.
fn read_event_counter(events_path: &Path) -> anyhow::Result<usize> {
    let counter_path = events_counter_path(events_path);
    if !counter_path.exists() {
        return count_file_events(events_path);
    }
    let text = fs::read_to_string(&counter_path)?;
    match text.trim().parse::<usize>() {
        Ok(count) => Ok(count),
        Err(_) => {
            // Counter file corrupted (empty, non-numeric, half-written).
            // Delete it so the next read falls back to counting events.jsonl.
            // Do NOT silently return 0 — that would cause event_id=1 on the
            // next emit, duplicating existing event IDs and triggering the
            // read() dedup path to silently discard events.
            let _ = fs::remove_file(&counter_path);
            count_file_events(events_path)
        }
    }
}

/// Persist the current event count to the counter file.
/// Caller must hold the events lock.
fn write_event_counter(events_path: &Path, count: usize) -> anyhow::Result<()> {
    fs::write(events_counter_path(events_path), count.to_string()).map_err(Into::into)
}

struct EventsLockGuard {
    path: PathBuf,
}

impl Drop for EventsLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

fn acquire_events_lock(path: &Path) -> anyhow::Result<Option<EventsLockGuard>> {
    acquire_events_lock_with_timeout(path, LOCK_TIMEOUT)
}

fn acquire_events_lock_with_timeout(
    path: &Path,
    timeout: Duration,
) -> anyhow::Result<Option<EventsLockGuard>> {
    let lock_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".events.lock");
    let deadline = Instant::now() + timeout;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => return Ok(Some(EventsLockGuard { path: lock_path })),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() >= deadline {
                    // fail-closed (backlog ⑫): refuse a lock-less best-effort write.
                    // Two writers without the lock can interleave a half-written
                    // JSONL line that read() silently skips → lost events. Events
                    // are an observability projection (.lto state is the source of
                    // truth), so dropping one clean event via safe_emit's Err arm
                    // is strictly better than corrupting the log. Consistent with
                    // the repo's read-only / sandbox fail-closed posture.
                    anyhow::bail!(
                        "events lock timeout; refusing best-effort write to avoid interleave"
                    );
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
    fn accepts_o2_types_but_still_rejects_typos_on_write() {
        let tmp = tempfile::tempdir().unwrap();
        emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "gate.blocked".to_string(),
                actor_kind: "lto".to_string(),
                summary: "closeout blocked".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        let err = emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "gate.blokced".to_string(),
                actor_kind: "lto".to_string(),
                summary: "typo".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid or deferred event type"));
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
    fn lock_timeout_fails_closed_instead_of_lockless_write() {
        // backlog ⑫: when the lock is held, acquiring with a 0-timeout must bail
        // (fail-closed), never return Ok(None) to take the best-effort path. The
        // lock-less path previously risked interleaved/corrupt JSONL lines.
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Hold the lock by creating the lock file ourselves.
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(&lock_path, b"").unwrap();

        match acquire_events_lock_with_timeout(&path, Duration::from_millis(0)) {
            Err(err) => assert!(err.to_string().contains("lock timeout"), "got: {err}"),
            Ok(_) => panic!("held lock with 0 timeout must fail, not fall back to lock-less write"),
        }

        // Releasing the lock restores normal emit (no corruption, no leftover state).
        // We don't call emit() while the lock is held — that would block ~5s on the
        // real LOCK_TIMEOUT; the low-level fail-closed contract above is the point.
        fs::remove_file(&lock_path).unwrap();
        let ok = safe_emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "after unlock".to_string(),
                ..EventRecord::default()
            },
        );
        assert!(ok.is_some(), "emit should succeed once the lock is free");
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

    #[test]
    fn event_counter_increments_correctly_and_is_written_to_file() {
        // BUG-2: counter file replaces O(N) full-file line counting.
        // After N emits the counter file must read N and the (N+1)th
        // event must carry event_id N+1.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let ev_path = events_path(repo, "r1");
        let counter_path = events_counter_path(&ev_path);

        for i in 1..=100 {
            let event = emit(
                repo,
                "r1",
                EventRecord {
                    event_type: "artifact.registered".to_string(),
                    actor_kind: "lto".to_string(),
                    summary: format!("event {i}"),
                    ..EventRecord::default()
                },
            )
            .unwrap();
            assert_eq!(event["event_id"], i, "event_id mismatch at emit {i}");
            let raw = fs::read_to_string(&counter_path).unwrap();
            assert_eq!(
                raw.trim(),
                i.to_string(),
                "counter file mismatch at emit {i}"
            );
        }
        // Counter file is tiny — just a few bytes, not ~100 lines of JSON.
        let counter_size = fs::metadata(&counter_path).unwrap().len();
        let events_size = fs::metadata(&ev_path).unwrap().len();
        assert!(
            counter_size < events_size / 2,
            "counter file ({counter_size}B) should be much smaller than events file ({events_size}B)"
        );
    }

    #[test]
    fn hard_stop_triggers_at_limit_via_counter_file() {
        // HARD_STOP_AT must still fire when the counter reaches 50k.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let ev_path = events_path(repo, "r1");
        fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        // Seed the counter at HARD_STOP_AT.
        fs::write(
            events_counter_path(&ev_path),
            crate::events::HARD_STOP_AT.to_string(),
        )
        .unwrap();
        let err = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "should be blocked".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("hard stop"),
            "expected hard stop error, got: {err}"
        );
        // force=true bypasses the limit.
        let event = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "forced".to_string(),
                force: true,
                ..EventRecord::default()
            },
        )
        .unwrap();
        assert_eq!(event["event_id"], HARD_STOP_AT + 1);
    }

    #[test]
    fn counter_migrates_from_existing_events_file() {
        // When a run predates the counter file, the first emit must fall
        // back to counting events.jsonl lines and persist the counter.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let ev_path = events_path(repo, "r1");
        fs::create_dir_all(ev_path.parent().unwrap()).unwrap();
        fs::write(
            &ev_path,
            "{\"event_id\":1}\n{\"event_id\":2}\n{\"event_id\":3}\n",
        )
        .unwrap();
        assert!(!events_counter_path(&ev_path).exists());

        let event = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "migration test".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        // 3 existing events → next event_id = 4.
        assert_eq!(event["event_id"], 4);
        // Counter file must now exist with value 4.
        let counter_val = fs::read_to_string(events_counter_path(&ev_path)).unwrap();
        assert_eq!(counter_val.trim(), "4");
        // Second emit uses counter (O(1)), not re-counting the file.
        let event2 = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "second after migration".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        assert_eq!(event2["event_id"], 5);
    }

    #[test]
    fn counter_read_write_is_atomic_under_lock() {
        // Multiple concurrent emitters under the file lock must produce
        // unique, gap-free event_ids via the counter file (not full reads).
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let handles = (0..64)
            .map(|idx| {
                let repo = repo.clone();
                std::thread::spawn(move || {
                    emit(
                        &repo,
                        "r1",
                        EventRecord {
                            event_type: "artifact.registered".to_string(),
                            actor_kind: "lto".to_string(),
                            summary: format!("concurrent {idx}"),
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
        assert_eq!(ids, (1..=64).collect::<Vec<_>>());
    }

    #[test]
    fn corrupted_counter_file_self_heals_and_does_not_produce_duplicate_event_ids() {
        // If .events.count contains garbage (empty, non-numeric, half-written),
        // read_event_counter must delete it and re-count events.jsonl — never
        // return 0, which would cause event_id=1 duplicate and silent data loss
        // via read() dedup.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let ev_path = events_path(repo, "r1");
        fs::create_dir_all(ev_path.parent().unwrap()).unwrap();

        // Write 5 events first (creates counter=5 via emit).
        for i in 1..=5 {
            emit(
                repo,
                "r1",
                EventRecord {
                    event_type: "artifact.registered".to_string(),
                    actor_kind: "lto".to_string(),
                    summary: format!("pre {i}"),
                    ..EventRecord::default()
                },
            )
            .unwrap();
        }
        assert_eq!(
            fs::read_to_string(events_counter_path(&ev_path))
                .unwrap()
                .trim(),
            "5"
        );

        // Corrupt the counter file.
        fs::write(events_counter_path(&ev_path), "garbage\n").unwrap();

        // Next emit must self-heal: delete corrupted counter, count events.jsonl,
        // use event_id=6 (not 1!).
        let event = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "after corruption".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        assert_eq!(
            event["event_id"], 6,
            "must continue from 6, not restart at 1"
        );
        // Counter file must be recreated with correct value.
        assert_eq!(
            fs::read_to_string(events_counter_path(&ev_path))
                .unwrap()
                .trim(),
            "6"
        );
        // read() must see all 6 events, no duplicates.
        let all = read(repo, "r1").unwrap();
        assert_eq!(all.len(), 6);
        let ids: Vec<u64> = all
            .iter()
            .map(|e| e["event_id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, (1..=6).collect::<Vec<_>>());

        // Also test empty counter file.
        fs::write(events_counter_path(&ev_path), "").unwrap();
        let event2 = emit(
            repo,
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "after empty counter".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        assert_eq!(event2["event_id"], 7);
    }

    #[test]
    fn count_function_is_pure_read_no_lock_required() {
        // count() calls read_event_counter which is a pure read (no write
        // side-effects). It must work correctly without acquiring the events
        // lock, even when no counter file exists yet.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let ev_path = events_path(repo, "r1");
        fs::create_dir_all(ev_path.parent().unwrap()).unwrap();

        // No counter file, no events file → count is 0.
        assert_eq!(count(repo, "r1").unwrap(), 0);
        // Counter file must NOT have been created (pure read, no side-effect).
        assert!(
            !events_counter_path(&ev_path).exists(),
            "count() must not create counter file"
        );

        // Write some events, then emit (which creates the counter).
        for i in 1..=3 {
            emit(
                repo,
                "r1",
                EventRecord {
                    event_type: "artifact.registered".to_string(),
                    actor_kind: "lto".to_string(),
                    summary: format!("event {i}"),
                    ..EventRecord::default()
                },
            )
            .unwrap();
        }
        // count() reads counter file (O(1)), no lock needed.
        assert_eq!(count(repo, "r1").unwrap(), 3);
    }
}
