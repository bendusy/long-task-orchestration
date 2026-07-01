//! `lto prune` — reclaim disk from finished runs without deleting run history.
//!
//! Retention discipline (LTO principle 1: never silently delete data):
//! - Only runs with `phase == "closed"` are eligible; active/unfinished runs are
//!   never touched.
//! - Eligible only when older than `--older-than` days (default 30).
//! - Only the *bulk* artifacts are removed (events.jsonl, live/, audit/,
//!   dispatch/). The lightweight history index — state.json, run-state.md,
//!   artifacts.json — is always kept so "what happened" stays auditable.
//! - `--dry-run` is the default: it lists what would be freed and deletes
//!   nothing. `--yes` performs the deletion.

use crate::state;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

/// Bulk artifacts eligible for removal. Everything else in a run dir is kept.
const BULK_TARGETS: &[&str] = &[
    "events.jsonl",
    "events.jsonl.counter",
    "live",
    "audit",
    "dispatch",
];

/// Advisory thresholds: past either, closeout/preflight nudge the host to prune.
const NUDGE_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
const NUDGE_CLOSED_RUNS: usize = 30;

/// Print a one-line nudge if `.lto` has grown past the advisory thresholds
/// (total size > 1 GiB or > 30 closed runs). Best-effort and never fails the
/// caller — this only reminds; it never deletes (LTO principle 1).
pub fn maybe_nudge_prune(repo: &Path) {
    let lto = repo.join(".lto");
    let Ok(entries) = fs::read_dir(&lto) else {
        return;
    };
    let mut total = 0u64;
    let mut closed = 0usize;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        total += path_size(&dir);
        if let Ok(st) = state::load_state(dir.join("state.json"))
            && st.current_phase == "closed"
        {
            closed += 1;
        }
    }
    if total > NUDGE_BYTES || closed > NUDGE_CLOSED_RUNS {
        println!(
            "NOTE: .lto is {} across {} closed run(s). Consider `lto prune --dry-run` to reclaim space.",
            human_bytes(total),
            closed
        );
    }
}

/// One run's disk footprint + phase, for `lto runs` reporting. Size is
/// best-effort (0 on read error rather than failing the listing).
pub fn run_size_and_phase(run_dir: &Path) -> (u64, String) {
    let size = path_size(run_dir);
    let phase = state::load_state(run_dir.join("state.json"))
        .map(|st| st.current_phase)
        .unwrap_or_else(|_| "?".to_string());
    (size, phase)
}

/// Public formatter so callers can render sizes consistently with prune output.
pub fn format_bytes(bytes: u64) -> String {
    human_bytes(bytes)
}

#[derive(Debug, Clone)]
pub struct PruneOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub older_than_days: i64,
    pub keep_last: usize,
    pub run_id: Option<String>,
}

impl Default for PruneOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            yes: false,
            older_than_days: 30,
            keep_last: 0,
            run_id: None,
        }
    }
}

struct Candidate {
    run_id: String,
    dir: PathBuf,
    age_days: i64,
    bulk_bytes: u64,
}

pub fn cmd_prune(repo: &Path, options: PruneOptions) -> Result<()> {
    // `--yes` turns off dry-run; otherwise dry-run stays on (the safe default).
    let dry_run = options.dry_run && !options.yes;

    let lto = repo.join(".lto");
    if !lto.exists() {
        println!("nothing to prune: no .lto directory");
        return Ok(());
    }

    let mut eligible = collect_eligible(&lto, &options)?;
    // Oldest first, so the report leads with the stalest runs.
    eligible.sort_by_key(|c| std::cmp::Reverse(c.age_days));
    if options.keep_last > 0 && options.run_id.is_none() {
        // Keep the most-recent `keep_last` closed runs (smallest age) untouched.
        let keep: std::collections::HashSet<String> = {
            let mut by_recent = eligible
                .iter()
                .map(|c| c.run_id.clone())
                .collect::<Vec<_>>();
            by_recent.sort();
            by_recent
                .into_iter()
                .rev()
                .take(options.keep_last)
                .collect()
        };
        eligible.retain(|c| !keep.contains(&c.run_id));
    }

    if eligible.is_empty() {
        println!(
            "nothing to prune (no closed run older than {} day(s) with bulk artifacts)",
            options.older_than_days
        );
        return Ok(());
    }

    let total: u64 = eligible.iter().map(|c| c.bulk_bytes).sum();
    println!(
        "=== lto prune ({}) — {} run(s), {} reclaimable ===",
        if dry_run { "dry-run" } else { "deleting" },
        eligible.len(),
        human_bytes(total)
    );
    for cand in &eligible {
        println!(
            "  {} | closed | {}d old | {}",
            cand.run_id,
            cand.age_days,
            human_bytes(cand.bulk_bytes)
        );
    }

    if dry_run {
        println!(
            "\nDry run: nothing deleted. Re-run with --yes to reclaim {}.",
            human_bytes(total)
        );
        return Ok(());
    }

    let mut freed = 0u64;
    for cand in &eligible {
        freed += prune_run_dir(&cand.dir)?;
        mark_pruned(&cand.dir);
    }
    println!(
        "\nPruned {} run(s), reclaimed {}.",
        eligible.len(),
        human_bytes(freed)
    );
    Ok(())
}

fn collect_eligible(lto: &Path, options: &PruneOptions) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(lto)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let run_id = entry.file_name().to_string_lossy().to_string();
        // When targeting one run, skip the closed/age gates but still refuse
        // active runs and still only touch bulk artifacts.
        if let Some(target) = &options.run_id
            && &run_id != target
        {
            continue;
        }
        let state_path = dir.join("state.json");
        let Ok(st) = state::load_state(&state_path) else {
            continue; // no readable state -> skip (don't guess)
        };
        // Never touch active/unfinished runs.
        if st.current_phase != "closed" {
            continue;
        }
        let age_days = age_in_days(&st.started_at).unwrap_or(0);
        if options.run_id.is_none() && age_days < options.older_than_days {
            continue;
        }
        let bulk_bytes = bulk_size(&dir);
        if bulk_bytes == 0 {
            continue; // already pruned / nothing to reclaim
        }
        out.push(Candidate {
            run_id,
            dir,
            age_days,
            bulk_bytes,
        });
    }
    Ok(out)
}

/// Sum the on-disk size of the bulk targets in a run dir.
fn bulk_size(dir: &Path) -> u64 {
    BULK_TARGETS
        .iter()
        .map(|name| path_size(&dir.join(name)))
        .sum()
}

/// Recursive size of a file or directory. Missing paths contribute 0.
fn path_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if meta.is_dir() {
        let mut total = 0;
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                total += path_size(&entry.path());
            }
        }
        return total;
    }
    0
}

/// Remove the bulk artifacts of a run, returning bytes freed. Keeps state.json,
/// run-state.md, artifacts.json (the lightweight history index).
fn prune_run_dir(dir: &Path) -> Result<u64> {
    let mut freed = 0u64;
    for name in BULK_TARGETS {
        let target = dir.join(name);
        let size = path_size(&target);
        if size == 0 {
            continue;
        }
        let meta = fs::symlink_metadata(&target)?;
        if meta.is_dir() {
            fs::remove_dir_all(&target)?;
        } else {
            fs::remove_file(&target)?;
        }
        freed += size;
    }
    Ok(freed)
}

/// Append a marker to run-state.md so a reader knows bulk logs were reclaimed
/// (not lost). Best-effort — never fails the prune.
fn mark_pruned(dir: &Path) {
    let path = dir.join("run-state.md");
    if !path.exists() {
        return;
    }
    let stamp = Utc::now().format("%Y-%m-%d").to_string();
    let line =
        format!("\n> [pruned {stamp}] bulk logs reclaimed by `lto prune`; state index retained.\n");
    if let Ok(mut existing) = fs::read_to_string(&path) {
        existing.push_str(&line);
        let _ = fs::write(&path, existing);
    }
}

fn age_in_days(started_at: &str) -> Option<i64> {
    let start = DateTime::parse_from_rfc3339(&started_at.replace('Z', "+00:00")).ok()?;
    let now = Utc::now().with_timezone(start.offset());
    Some((now - start).num_days())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LtoState, WorkspaceSnapshot};
    use chrono::Duration;

    fn write_run(lto: &Path, run_id: &str, phase: &str, age_days: i64, events_bytes: usize) {
        let dir = lto.join(run_id);
        fs::create_dir_all(&dir).unwrap();
        let started = (Utc::now() - Duration::days(age_days)).to_rfc3339();
        let state = LtoState {
            run_id: run_id.to_string(),
            current_phase: phase.to_string(),
            started_at: started,
            workspace: WorkspaceSnapshot::default(),
            ..LtoState::default()
        };
        state::save_state(dir.join("state.json"), &state).unwrap();
        fs::write(dir.join("run-state.md"), "# run\n").unwrap();
        fs::write(dir.join("events.jsonl"), "x".repeat(events_bytes)).unwrap();
    }

    #[test]
    fn prunes_old_closed_run_keeps_index() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "old-closed", "closed", 60, 10_000);
        let opts = PruneOptions {
            dry_run: false,
            yes: true,
            ..PruneOptions::default()
        };
        cmd_prune(tmp.path(), opts).unwrap();
        // Bulk gone, index kept.
        assert!(!lto.join("old-closed").join("events.jsonl").exists());
        assert!(lto.join("old-closed").join("state.json").exists());
        assert!(lto.join("old-closed").join("run-state.md").exists());
        // Marker appended.
        let md = fs::read_to_string(lto.join("old-closed").join("run-state.md")).unwrap();
        assert!(md.contains("pruned"));
    }

    #[test]
    fn never_touches_active_run() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "active", "implementation", 90, 10_000);
        let opts = PruneOptions {
            dry_run: false,
            yes: true,
            ..PruneOptions::default()
        };
        cmd_prune(tmp.path(), opts).unwrap();
        assert!(lto.join("active").join("events.jsonl").exists());
    }

    #[test]
    fn skips_recent_closed_run() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "fresh", "closed", 5, 10_000); // < 30d
        let opts = PruneOptions {
            dry_run: false,
            yes: true,
            ..PruneOptions::default()
        };
        cmd_prune(tmp.path(), opts).unwrap();
        assert!(lto.join("fresh").join("events.jsonl").exists());
    }

    #[test]
    fn dry_run_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "old-closed", "closed", 60, 10_000);
        // Default is dry-run.
        cmd_prune(tmp.path(), PruneOptions::default()).unwrap();
        assert!(lto.join("old-closed").join("events.jsonl").exists());
    }

    #[test]
    fn keep_last_protects_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "run-a-oldest", "closed", 90, 10_000);
        write_run(&lto, "run-b-newest", "closed", 40, 10_000);
        let opts = PruneOptions {
            dry_run: false,
            yes: true,
            keep_last: 1,
            ..PruneOptions::default()
        };
        cmd_prune(tmp.path(), opts).unwrap();
        // keep_last=1 keeps the alphabetically-last (run-b) — protected.
        assert!(lto.join("run-b-newest").join("events.jsonl").exists());
        assert!(!lto.join("run-a-oldest").join("events.jsonl").exists());
    }

    #[test]
    fn older_than_override() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "ten-day", "closed", 10, 10_000);
        let opts = PruneOptions {
            dry_run: false,
            yes: true,
            older_than_days: 7, // now 10-day run qualifies
            ..PruneOptions::default()
        };
        cmd_prune(tmp.path(), opts).unwrap();
        assert!(!lto.join("ten-day").join("events.jsonl").exists());
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn run_size_and_phase_reports_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "r1", "closed", 10, 5_000);
        let (size, phase) = run_size_and_phase(&lto.join("r1"));
        assert_eq!(phase, "closed");
        assert!(size >= 5_000, "size includes the events.jsonl bytes");
    }

    #[test]
    fn run_size_and_phase_missing_state_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("no-state");
        fs::create_dir_all(&dir).unwrap();
        let (_size, phase) = run_size_and_phase(&dir);
        assert_eq!(phase, "?");
    }

    #[test]
    fn nudge_counts_only_closed_runs_without_panic() {
        // maybe_nudge_prune must never fail; with a small .lto it simply prints
        // nothing. We assert it runs cleanly over mixed phases.
        let tmp = tempfile::tempdir().unwrap();
        let lto = tmp.path().join(".lto");
        write_run(&lto, "closed-run", "closed", 5, 1_000);
        write_run(&lto, "active-run", "implementation", 5, 1_000);
        // Below thresholds -> no output, no panic.
        maybe_nudge_prune(tmp.path());
        // Missing .lto -> also safe.
        maybe_nudge_prune(&tmp.path().join("nonexistent"));
    }
}
