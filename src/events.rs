use crate::redact::{redact_text, redact_value};
use crate::state;
use fs2::FileExt;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u64 = 1;
pub const WARN_AT: usize = 10_000;
pub const HARD_STOP_AT: usize = 50_000;

pub const KNOWN_EVENT_TYPES: &[&str] = &[
    "run.started",
    "run.closed",
    "contract.updated",
    "phase.changed",
    "task.created",
    "task.status_changed",
    "runner.started",
    "runner.finished",
    "runner.retry",
    "runner.healthcheck",
    "agent.turn.completed",
    "agent.session.ended",
    "agent.dispatch.completed",
    "runner.window.cleaned",
    "artifact.registered",
    "audit.dispatched",
    "audit.finding",
    "audit.round.recorded",
    "audit.ledger.evaluated",
    "gate.evaluated",
    "gate.blocked",
    "budget.warned",
    "budget.exceeded",
    "sandbox.rejected",
    "judge.skipped",
    "decision.voted",
    "decision.escalated",
    "decision.recorded",
    "decision.reaffirmed",
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

    let state_path = state::state_path(repo, run_id);
    if !state_path.is_file() {
        anyhow::bail!("no state.json for {run_id}: {}", state_path.display());
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

pub fn wait_for(
    repo: &Path,
    run_id: &str,
    event_type: &str,
    after: Option<u64>,
    timeout: Duration,
) -> anyhow::Result<Option<Value>> {
    // Read once for both cursor derivation and the lookback check (after=None
    // would otherwise parse events.jsonl twice back-to-back).
    let existing = read(repo, run_id)?;
    let start_after = match after {
        Some(val) => val,
        None => existing
            .iter()
            .filter_map(|event| event.get("event_id").and_then(Value::as_u64))
            .max()
            .unwrap_or(0),
    };

    let start_time = Instant::now();
    let check_events = |events: &[Value]| -> Option<Value> {
        events
            .iter()
            .find(|event| {
                // Events without a numeric event_id are out-of-contract (emit()
                // always assigns one from 1); skip them rather than coercing to
                // 0, which could falsely (un)match at the start_after==0 boundary.
                let Some(event_id) = event.get("event_id").and_then(Value::as_u64) else {
                    return false;
                };
                let event_type_val = event.get("type").and_then(Value::as_str).unwrap_or("");
                event_id > start_after && event_type_val == event_type
            })
            .cloned()
    };

    // 1. Lookback check (reuse the read above)
    if let Some(matched) = check_events(&existing) {
        return Ok(Some(matched));
    }

    // Register a wake endpoint so `agent-turn-completed` can connect-drop to
    // unblock us the moment an event lands, instead of waiting out a full poll
    // tick. Registration failure is non-fatal: we degrade to plain polling.
    let waiter_id = format!("wait-{}", std::process::id());
    let server = crate::notify::NotifyServer::register(repo, run_id, &waiter_id).ok();

    // 2. Poll loop. The sleep is short when no wake transport is available; with
    // a server we still poll (as the correctness backstop) but a wake shortens
    // the effective latency by draining between sleeps.
    let sleep_interval = if timeout < Duration::from_secs(2) {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(500)
    };

    while start_time.elapsed() < timeout {
        thread::sleep(sleep_interval);
        if let Some(server) = &server {
            // Drain wake pings; a true return just means "re-check now".
            let _ = server.drain();
        }
        let current = read(repo, run_id)?;
        if let Some(matched) = check_events(&current) {
            return Ok(Some(matched));
        }
    }

    Ok(None)
}

pub fn cmd_events(
    repo: &Path,
    run_id: &str,
    wait: bool,
    event_type: Option<String>,
    after: Option<u64>,
    timeout: u64,
    json: bool,
) -> anyhow::Result<()> {
    use anyhow::Context;

    if !wait {
        anyhow::bail!("events command without --wait is not implemented in phase 1");
    }
    let event_type = event_type.context("events --wait requires --event-type")?;
    let duration = Duration::from_secs(timeout);

    let result = wait_for(repo, run_id, &event_type, after, duration)?;
    match result {
        Some(event) => {
            if json {
                println!("{}", serde_json::to_string(&event)?);
            } else {
                let id = event.get("event_id").and_then(Value::as_u64).unwrap_or(0);
                let at = event.get("at").and_then(Value::as_str).unwrap_or("");
                let summary = event.get("summary").and_then(Value::as_str).unwrap_or("");
                println!("Event #{id} [{at}] {event_type}: {summary}");
            }
            std::process::exit(0);
        }
        None => {
            eprintln!("Timeout waiting for event '{event_type}' after {timeout} seconds");
            std::process::exit(1);
        }
    }
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

struct ReclaimGuard {
    _file: fs::File,
}

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const LOCK_ABANDONED_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

fn acquire_events_lock(path: &Path) -> anyhow::Result<Option<EventsLockGuard>> {
    acquire_events_lock_with_timeout(path, LOCK_TIMEOUT)
}

fn acquire_events_lock_with_timeout(
    path: &Path,
    timeout: Duration,
) -> anyhow::Result<Option<EventsLockGuard>> {
    acquire_events_lock_with_timeout_and_stale(path, timeout, LOCK_STALE_AFTER)
}

fn acquire_events_lock_with_timeout_and_stale(
    path: &Path,
    timeout: Duration,
    stale_after: Duration,
) -> anyhow::Result<Option<EventsLockGuard>> {
    let lock_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".events.lock");
    let deadline = Instant::now() + timeout;
    let stale_probe_interval = if stale_after == Duration::ZERO {
        Duration::ZERO
    } else {
        Duration::from_millis(500)
    };
    let mut next_stale_probe = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                if let Err(err) = write_lock_owner(&mut file) {
                    let _ = fs::remove_file(&lock_path);
                    return Err(err);
                }
                return Ok(Some(EventsLockGuard { path: lock_path }));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let now = Instant::now();
                if now >= next_stale_probe {
                    next_stale_probe = now + stale_probe_interval;
                    if try_reclaim_stale_events_lock(&lock_path, stale_after).unwrap_or(false) {
                        continue;
                    }
                }
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

fn write_lock_owner(file: &mut fs::File) -> anyhow::Result<()> {
    let payload = json!({
        "pid": std::process::id(),
        "created_at_unix_ms": unix_ms_now(),
        "owner_exe": current_exe_name(),
    });
    file.write_all(payload.to_string().as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn try_reclaim_stale_events_lock(lock_path: &Path, stale_after: Duration) -> anyhow::Result<bool> {
    if !events_lock_is_stale(lock_path, stale_after).unwrap_or(false) {
        return Ok(false);
    }
    let Some(_reclaim_guard) = try_acquire_reclaim_guard(lock_path)? else {
        return Ok(false);
    };
    if !events_lock_is_stale(lock_path, stale_after).unwrap_or(false) {
        return Ok(false);
    }
    let reclaim_path = unique_reclaim_path(lock_path);
    fs::hard_link(lock_path, &reclaim_path)?;
    let still_stale = events_lock_is_stale(&reclaim_path, stale_after).unwrap_or(false);
    let lock_path_still_matches = same_file_identity(lock_path, &reclaim_path).unwrap_or(false);
    let reclaimed = still_stale && lock_path_still_matches;
    if reclaimed {
        let _ = fs::remove_file(lock_path);
    }
    let _ = fs::remove_file(&reclaim_path);
    Ok(reclaimed)
}

fn try_acquire_reclaim_guard(lock_path: &Path) -> anyhow::Result<Option<ReclaimGuard>> {
    let reclaim_guard_path = lock_path.with_file_name(".events.lock.reclaiming");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&reclaim_guard_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    file.set_len(0)?;
    write_lock_owner(&mut file)?;
    Ok(Some(ReclaimGuard { _file: file }))
}

fn unique_reclaim_path(lock_path: &Path) -> PathBuf {
    let file_name = lock_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(".events.lock");
    lock_path.with_file_name(format!(
        "{file_name}.reclaim.{}.{}",
        std::process::id(),
        unix_ms_now()
    ))
}

fn events_lock_is_stale(lock_path: &Path, stale_after: Duration) -> anyhow::Result<bool> {
    let text = fs::read_to_string(lock_path).unwrap_or_default();
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        let age_ms = value
            .get("created_at_unix_ms")
            .and_then(Value::as_u64)
            .map(lock_age_ms);
        if let Some(pid) = value.get("pid").and_then(Value::as_u64) {
            if pid > u64::from(u32::MAX) {
                return Ok(true);
            }
            return Ok(match probe_process(pid as u32) {
                ProcessProbe::Dead => true,
                ProcessProbe::Alive => {
                    owner_exe_mismatch(&value, pid as u32).unwrap_or(false)
                        || age_ms
                            .map(|age| age >= duration_ms(LOCK_ABANDONED_AFTER))
                            .unwrap_or(false)
                }
                ProcessProbe::Unknown => age_ms
                    .map(|age| age >= duration_ms(stale_after))
                    .unwrap_or(false),
            });
        }
        if let Some(age) = age_ms {
            return Ok(age >= duration_ms(stale_after));
        }
    }

    let modified = fs::metadata(lock_path)?.modified()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    Ok(age >= stale_after)
}

fn current_exe_name() -> Option<String> {
    std::env::current_exe().ok().and_then(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    })
}

fn owner_exe_mismatch(value: &Value, pid: u32) -> Option<bool> {
    let owner = value.get("owner_exe").and_then(Value::as_str)?;
    let live = process_exe_name(pid)?;
    Some(owner != live)
}

#[cfg(target_os = "linux")]
fn process_exe_name(pid: u32) -> Option<String> {
    let path = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())?;
    // A binary replaced on disk while running (cargo rebuild) reads back as
    // "name (deleted)"; strip it or the alive holder gets branded stale.
    Some(match name.strip_suffix(" (deleted)") {
        Some(stripped) => stripped.to_string(),
        None => name,
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_exe_name(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("comm=")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let command = text.trim();
    if command.is_empty() {
        return None;
    }
    Some(
        Path::new(command)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| command.to_string()),
    )
}

#[cfg(not(unix))]
fn process_exe_name(_pid: u32) -> Option<String> {
    None
}

#[cfg(unix)]
fn same_file_identity(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let a = fs::metadata(a)?;
    let b = fs::metadata(b)?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

#[cfg(not(unix))]
fn same_file_identity(_a: &Path, _b: &Path) -> std::io::Result<bool> {
    Ok(false)
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn lock_age_ms(created_at_unix_ms: u64) -> u64 {
    unix_ms_now().saturating_sub(created_at_unix_ms)
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProcessProbe {
    Alive,
    Dead,
    Unknown,
}

fn probe_process(pid: u32) -> ProcessProbe {
    if pid == std::process::id() {
        return ProcessProbe::Alive;
    }
    #[cfg(unix)]
    {
        let output = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .output();
        match output {
            Ok(output) => process_probe_from_kill_output(output.status.success(), &output.stderr),
            Err(_) => ProcessProbe::Unknown,
        }
    }
    #[cfg(not(unix))]
    {
        ProcessProbe::Unknown
    }
}

#[cfg(unix)]
fn process_probe_from_kill_output(success: bool, stderr: &[u8]) -> ProcessProbe {
    if success {
        return ProcessProbe::Alive;
    }
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if stderr.contains("no such process") {
        ProcessProbe::Dead
    } else if stderr.contains("operation not permitted") || stderr.contains("not permitted") {
        ProcessProbe::Alive
    } else {
        ProcessProbe::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_AUDIT_EVENT: &str = "audit.converged"; // legacy test fixture

    fn create_run(repo: &Path, run_id: &str) {
        let state_path = state::state_path(repo, run_id);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(state_path, b"{}").unwrap();
    }

    #[test]
    fn emit_rejects_missing_state_without_creating_run_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let run_id = "missing-state";
        let err = emit(
            tmp.path(),
            run_id,
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("no state.json for missing-state"));
        assert!(!tmp.path().join(".lto").join(run_id).exists());
    }

    #[test]
    fn emit_writes_and_reads_event_for_existing_run() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");

        emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "recorded".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let events = read(tmp.path(), "r1").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["summary"], "recorded");
    }

    #[test]
    fn safe_emit_returns_none_for_missing_state() {
        let tmp = tempfile::tempdir().unwrap();
        let emitted = safe_emit(
            tmp.path(),
            "missing-state",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                ..EventRecord::default()
            },
        );

        assert!(emitted.is_none());
    }

    #[test]
    fn writes_redacted_append_only_events_and_reads_unknown_types() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        create_run(repo, "r1");
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
    fn accepts_current_audit_types_but_rejects_legacy_and_typos_on_write() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        for event_type in ["audit.round.recorded", "audit.ledger.evaluated"] {
            emit(
                tmp.path(),
                "r1",
                EventRecord {
                    event_type: event_type.to_string(),
                    actor_kind: "lto".to_string(),
                    summary: event_type.to_string(),
                    ..EventRecord::default()
                },
            )
            .unwrap();
        }
        for event_type in [LEGACY_AUDIT_EVENT, "gate.blokced"] {
            let err = emit(
                tmp.path(),
                "r1",
                EventRecord {
                    event_type: event_type.to_string(),
                    actor_kind: "lto".to_string(),
                    summary: event_type.to_string(),
                    ..EventRecord::default()
                },
            )
            .unwrap_err();
            assert!(err.to_string().contains("invalid or deferred event type"));
        }
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
        create_run(&repo, "r1");
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
        create_run(tmp.path(), "r1");
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
    fn stale_json_lock_with_dead_pid_is_recovered() {
        let tmp = tempfile::tempdir().unwrap();
        create_run(tmp.path(), "r1");
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        let event = emit(
            tmp.path(),
            "r1",
            EventRecord {
                event_type: "artifact.registered".to_string(),
                actor_kind: "lto".to_string(),
                summary: "after stale lock".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        assert_eq!(event["event_id"], 1);
        assert!(
            !lock_path.exists(),
            "lock guard should clean recovered lock"
        );
    }

    #[test]
    fn live_pid_lock_still_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": std::process::id(),
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        match acquire_events_lock_with_timeout(&path, Duration::from_millis(0)) {
            Err(err) => assert!(err.to_string().contains("lock timeout"), "got: {err}"),
            Ok(_) => panic!("live pid lock must not be stolen"),
        }
    }

    #[test]
    fn old_live_pid_lock_still_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": std::process::id(),
                "created_at_unix_ms": unix_ms_now().saturating_sub(60_000)
            })
            .to_string(),
        )
        .unwrap();

        match acquire_events_lock_with_timeout_and_stale(
            &path,
            Duration::from_millis(0),
            Duration::ZERO,
        ) {
            Err(err) => assert!(err.to_string().contains("lock timeout"), "got: {err}"),
            Ok(_) => panic!("old live pid lock must not be stolen"),
        }
    }

    #[test]
    fn reclaim_removes_dead_pid_lock_only_after_second_stale_check() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        assert!(try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(!lock_path.exists());
    }

    #[test]
    fn reclaim_does_not_remove_live_pid_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": std::process::id(),
                "created_at_unix_ms": unix_ms_now().saturating_sub(60_000)
            })
            .to_string(),
        )
        .unwrap();

        assert!(!try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(lock_path.exists());
    }

    #[test]
    fn reclaim_guard_prevents_parallel_reclaimer_from_removing_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        let reclaim_guard_path = lock_path.with_file_name(".events.lock.reclaiming");
        let guard_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&reclaim_guard_path)
            .unwrap();
        guard_file.try_lock_exclusive().unwrap();
        fs::write(
            &lock_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        assert!(!try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(lock_path.exists());
        drop(guard_file);
        assert!(try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(!lock_path.exists());
    }

    #[test]
    fn orphaned_reclaim_guard_file_does_not_block_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        let reclaim_guard_path = lock_path.with_file_name(".events.lock.reclaiming");
        fs::write(
            &lock_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            &reclaim_guard_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        assert!(try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(!lock_path.exists());
        assert!(reclaim_guard_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn reclaim_guard_file_replacement_does_not_delete_new_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        let guard_path = lock_path.with_file_name(".events.lock.reclaiming");
        fs::write(
            &lock_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            &guard_path,
            json!({
                "pid": 999_999_999_u64,
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();
        fs::remove_file(&guard_path).unwrap();
        fs::write(
            &guard_path,
            json!({
                "pid": std::process::id(),
                "created_at_unix_ms": unix_ms_now()
            })
            .to_string(),
        )
        .unwrap();

        assert!(try_reclaim_stale_events_lock(&lock_path, Duration::ZERO).unwrap());
        assert!(guard_path.exists());
    }

    #[test]
    fn live_pid_with_different_owner_exe_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(
            &lock_path,
            json!({
                "pid": std::process::id(),
                "created_at_unix_ms": unix_ms_now(),
                "owner_exe": "definitely-not-this-process"
            })
            .to_string(),
        )
        .unwrap();

        assert!(events_lock_is_stale(&lock_path, LOCK_STALE_AFTER).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_exe_name_is_not_truncated_like_proc_comm() {
        let expected = current_exe_name().expect("current test executable should have a name");
        assert!(
            expected.len() > 15,
            "test executable must exceed Linux TASK_COMM_LEN to cover the lock-owner bug"
        );
        assert_eq!(process_exe_name(std::process::id()), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn same_file_identity_distinguishes_replaced_lock_path() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join(".events.lock");
        let reclaim_path = tmp.path().join(".events.lock.reclaim");
        fs::write(&lock_path, b"old").unwrap();
        fs::hard_link(&lock_path, &reclaim_path).unwrap();

        assert!(same_file_identity(&lock_path, &reclaim_path).unwrap());
        fs::remove_file(&lock_path).unwrap();
        fs::write(&lock_path, b"new").unwrap();
        assert!(!same_file_identity(&lock_path, &reclaim_path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn kill_probe_only_treats_no_such_process_as_dead() {
        assert_eq!(
            process_probe_from_kill_output(false, b"kill: 999999999: No such process\n"),
            ProcessProbe::Dead
        );
        assert_eq!(
            process_probe_from_kill_output(false, b"kill: 42: Operation not permitted\n"),
            ProcessProbe::Alive
        );
        assert_eq!(
            process_probe_from_kill_output(false, b"unexpected failure"),
            ProcessProbe::Unknown
        );
        assert_eq!(
            process_probe_from_kill_output(true, b""),
            ProcessProbe::Alive
        );
    }

    #[test]
    fn legacy_stale_empty_lock_is_recovered_only_after_stale_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let path = events_path(tmp.path(), "r1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_path = path.parent().unwrap().join(".events.lock");
        fs::write(&lock_path, b"").unwrap();

        let guard = acquire_events_lock_with_timeout_and_stale(
            &path,
            Duration::from_millis(0),
            Duration::ZERO,
        )
        .unwrap()
        .expect("stale legacy lock should be recovered");

        let text = fs::read_to_string(&lock_path).unwrap();
        assert!(text.contains("created_at_unix_ms"));
        drop(guard);
        assert!(!lock_path.exists());
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
        create_run(repo, "r1");
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
        create_run(repo, "r1");
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
        create_run(repo, "r1");
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
        create_run(&repo, "r1");
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
        create_run(repo, "r1");
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
        create_run(repo, "r1");
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

    #[test]
    fn wait_for_returns_existing_event_via_lookback() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";
        create_run(repo, run_id);

        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "agent.turn.completed".to_string(),
                actor_kind: "lto".to_string(),
                summary: "completed turn".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let matched = wait_for(
            repo,
            run_id,
            "agent.turn.completed",
            Some(0),
            Duration::from_secs(1),
        )
        .unwrap()
        .expect("must find existing event");

        assert_eq!(matched["event_id"], 1);
        assert_eq!(matched["type"], "agent.turn.completed");
    }

    #[test]
    fn wait_for_filters_by_event_type() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";
        create_run(repo, run_id);

        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "run.started".to_string(),
                actor_kind: "lto".to_string(),
                summary: "run start".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "agent.turn.completed".to_string(),
                actor_kind: "lto".to_string(),
                summary: "completed turn".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let matched = wait_for(
            repo,
            run_id,
            "agent.turn.completed",
            Some(0),
            Duration::from_secs(1),
        )
        .unwrap()
        .expect("must find matching event");

        assert_eq!(matched["event_id"], 2);
        assert_eq!(matched["type"], "agent.turn.completed");
    }

    #[test]
    fn wait_for_respects_after_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";
        create_run(repo, run_id);

        emit(
            repo,
            run_id,
            EventRecord {
                event_type: "agent.turn.completed".to_string(),
                actor_kind: "lto".to_string(),
                summary: "first".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let first_id = 1;

        let repo_clone = tmp.path().to_path_buf();
        let run_id_str = run_id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            emit(
                &repo_clone,
                &run_id_str,
                EventRecord {
                    event_type: "agent.turn.completed".to_string(),
                    actor_kind: "lto".to_string(),
                    summary: "second".to_string(),
                    ..EventRecord::default()
                },
            )
            .unwrap();
        });

        let matched = wait_for(
            repo,
            run_id,
            "agent.turn.completed",
            Some(first_id),
            Duration::from_secs(2),
        )
        .unwrap()
        .expect("must find second event");

        assert_eq!(matched["event_id"], 2);
        assert_eq!(matched["summary"], "second");
    }

    #[test]
    fn wait_for_times_out_when_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";

        let matched = wait_for(
            repo,
            run_id,
            "agent.turn.completed",
            None,
            Duration::from_millis(200),
        )
        .unwrap();

        assert!(matched.is_none(), "must time out and return None");
    }

    #[test]
    fn wait_for_is_woken_by_an_event_emitted_concurrently() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let run_id = "r1";
        create_run(&repo, run_id);
        // Seed the run dir so the endpoints file has a home.
        emit(
            &repo,
            run_id,
            EventRecord {
                event_type: "runner.started".to_string(),
                actor_kind: "runner".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();

        let repo_w = repo.clone();
        let waiter = std::thread::spawn(move || {
            wait_for(
                &repo_w,
                run_id,
                "agent.turn.completed",
                None,
                Duration::from_secs(5),
            )
            .unwrap()
        });

        // Let the waiter register its endpoint, then emit the target event and
        // wake it the way agent-turn-completed does.
        thread::sleep(Duration::from_millis(150));
        emit(
            &repo,
            run_id,
            EventRecord {
                event_type: "agent.turn.completed".to_string(),
                actor_kind: "runner".to_string(),
                summary: "done".to_string(),
                ..EventRecord::default()
            },
        )
        .unwrap();
        crate::notify::wake_run(&repo, run_id);

        let started = Instant::now();
        let matched = waiter.join().unwrap();
        assert!(matched.is_some(), "waiter must catch the emitted event");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "waiter must return well before the 5s timeout (woken, not timed out)"
        );
    }
}
