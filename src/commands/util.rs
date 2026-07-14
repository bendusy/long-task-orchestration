use crate::agent_job::{AgentResult, JobStatus};
use crate::process;
use crate::state::{self, LtoState};
use anyhow::{Context, anyhow};
use chrono::{DateTime, FixedOffset, Local, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const VALID_PHASES: &[&str] = &[
    "intake",
    "spec",
    "audit",
    "implementation",
    "deploy",
    "observe",
    "closed",
];
pub const VALID_TASK_STATUSES: &[&str] = &["pending", "in_progress", "blocked", "done", "skipped"];
pub const VALID_EVIDENCE_KINDS: &[&str] = &[
    "test", "lint", "build", "manual", "review", "deploy", "worker",
];
pub const KNOWN_RUNNERS: &[&str] = &["codex", "pi", "agy", "gemini", "claude"];

#[derive(Debug, Clone, PartialEq)]
pub struct RunContext {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub state_path: PathBuf,
    pub state: LtoState,
}

#[derive(Debug, Clone, Default)]
pub struct TokenRunnerRollup {
    pub tokens: u64,
    pub runs_with_tokens: u64,
    pub runs_total: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TokenRollup {
    pub total_tokens: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub runs_with_tokens: u64,
    pub runs_total: u64,
    pub total_elapsed_sec: f64,
    pub by_runner: BTreeMap<String, TokenRunnerRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    pub head: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct ArtifactMeta<'a> {
    pub kind: &'a str,
    pub producer: &'a str,
    pub state: &'a LtoState,
    pub summary: &'a str,
    pub tags: &'a [&'a str],
}

pub fn resolve_run_id(repo: &Path, run_id: Option<&str>) -> anyhow::Result<String> {
    if let Some(run_id) = run_id {
        return Ok(state::validate_run_id(run_id)?.to_string());
    }
    let current = repo.join(".lto").join("current");
    let text = fs::read_to_string(&current)
        .with_context(|| format!("missing --run-id and {}", current.display()))?;
    let run_id = text.trim();
    if run_id.is_empty() {
        anyhow::bail!("missing --run-id and empty {}", current.display());
    }
    Ok(state::validate_run_id(run_id)?.to_string())
}

pub fn load_run(repo: &Path, run_id: Option<&str>) -> anyhow::Result<RunContext> {
    let run_id = resolve_run_id(repo, run_id)?;
    let run_dir = repo.join(".lto").join(&run_id);
    let state_path = run_dir.join("state.json");
    let state = state::load_state(&state_path)
        .with_context(|| format!("no state.json for {run_id}: {}", state_path.display()))?;
    Ok(RunContext {
        run_id,
        run_dir,
        state_path,
        state,
    })
}

pub fn save_run(ctx: &RunContext) -> anyhow::Result<()> {
    state::save_state(&ctx.state_path, &ctx.state)
}

pub fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn json_array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn json_array_mut(value: &mut Value) -> &mut Vec<Value> {
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value.as_array_mut().expect("value forced to array")
}

pub fn json_object_mut(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value forced to object")
}

pub fn risk_is_open(risk: &Value) -> bool {
    let disposition = risk
        .get("disposition")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let Some(disposition) = disposition {
        return disposition.eq_ignore_ascii_case("open");
    }
    risk.get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|status| status.eq_ignore_ascii_case("open"))
}

pub fn risk_is_verified(risk: &Value) -> bool {
    risk.get("disposition")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|disposition| disposition.eq_ignore_ascii_case("verified"))
        || risk.get("verified_by").is_some_and(|value| {
            value.as_str().is_some_and(|text| !text.trim().is_empty())
                || value.as_bool() == Some(true)
        })
}

pub fn risk_is_open_unverified(risk: &Value) -> bool {
    risk_is_open(risk) && !risk_is_verified(risk)
}

pub fn status_str(result: &AgentResult) -> &'static str {
    match result.status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Ok => "ok",
        JobStatus::Failed => "failed",
        JobStatus::Timeout => "timeout",
        JobStatus::RateLimited => "rate_limited",
        JobStatus::Skipped => "skipped",
    }
}

pub fn parse_status(value: &str) -> anyhow::Result<JobStatus> {
    match value {
        "pending" => Ok(JobStatus::Pending),
        "running" => Ok(JobStatus::Running),
        "ok" | "returned" => Ok(JobStatus::Ok),
        "failed" => Ok(JobStatus::Failed),
        "timeout" => Ok(JobStatus::Timeout),
        "rate_limited" => Ok(JobStatus::RateLimited),
        "skipped" => Ok(JobStatus::Skipped),
        other => anyhow::bail!(
            "invalid job status: {other:?}; expected one of: {}",
            crate::agent_job::JOB_STATUS_INPUT_VALUES.join(", ")
        ),
    }
}

pub fn token_rollup(state: &LtoState) -> TokenRollup {
    let mut out = TokenRollup::default();
    for result in iter_agent_runs(&state.agent_runs) {
        out.runs_total += 1;
        let slot = out.by_runner.entry(result.runner.clone()).or_default();
        slot.runs_total += 1;
        if let Some(elapsed) = json_f64(result.cost.get("elapsed_sec"))
            && elapsed >= 0.0
        {
            out.total_elapsed_sec += elapsed;
        }
        let ti = json_u64(result.cost.get("tokens_in")).unwrap_or(0);
        let to = json_u64(result.cost.get("tokens_out")).unwrap_or(0);
        let mut tokens = json_u64(result.cost.get("tokens")).unwrap_or(0);
        if tokens == 0 && (ti > 0 || to > 0) {
            tokens = ti.saturating_add(to);
        }
        if tokens > 0 {
            out.total_tokens = out.total_tokens.saturating_add(tokens);
            out.tokens_in = out.tokens_in.saturating_add(ti);
            out.tokens_out = out.tokens_out.saturating_add(to);
            out.runs_with_tokens += 1;
            slot.tokens = slot.tokens.saturating_add(tokens);
            slot.runs_with_tokens += 1;
        }
    }
    out
}

pub fn iter_agent_runs(agent_runs: &Value) -> Vec<AgentResult> {
    state::agent_results_from_agent_runs(agent_runs)
}

pub fn append_agent_results_to_state(
    state: &mut LtoState,
    task_key: Option<&str>,
    results: &[AgentResult],
) -> anyhow::Result<()> {
    let agent_runs = json_object_mut(&mut state.agent_runs);
    for result in results {
        let key = task_key.unwrap_or(result.job_id.as_str()).to_string();
        let entries = agent_runs
            .entry(key)
            .or_insert_with(|| Value::Array(Vec::new()));
        json_array_mut(entries).push(serde_json::to_value(result)?);
    }
    Ok(())
}

pub fn json_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

pub fn json_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

pub fn iso_now() -> String {
    state::iso_now()
}

pub fn elapsed_human(started: &str) -> String {
    if started.trim().is_empty() {
        return "（无开始时间）".to_string();
    }
    let Ok(start) = DateTime::parse_from_rfc3339(&started.replace('Z', "+00:00")) else {
        return started.to_string();
    };
    let now: DateTime<FixedOffset> = Utc::now().with_timezone(start.offset());
    let seconds = (now - start).num_seconds().max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    if days > 0 {
        if hours > 0 {
            return format!("{days} 天 {hours} 小时");
        }
        return format!("{days} 天");
    }
    if hours > 0 {
        return format!("{hours} 小时");
    }
    format!("{} 分钟", (seconds % 3_600) / 60)
}

pub fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

pub fn format_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub fn max_session_gap_hours(state: &LtoState) -> f64 {
    let mut max_gap = 0.0_f64;
    let mut previous: Option<DateTime<FixedOffset>> = None;
    for item in json_array(&state.phase_transitions) {
        let Some(at) = item.get("at").and_then(Value::as_str) else {
            continue;
        };
        let Ok(dt) = DateTime::parse_from_rfc3339(&at.replace('Z', "+00:00")) else {
            continue;
        };
        if let Some(prev) = previous {
            max_gap = max_gap.max((dt - prev).num_seconds() as f64 / 3_600.0);
        }
        previous = Some(dt);
    }
    max_gap
}

pub fn git_status(repo: &Path) -> GitStatus {
    let head =
        process::git_stdout(repo, ["rev-parse", "HEAD"]).unwrap_or_else(|_| "unknown".to_string());
    let branch = process::git_stdout(repo, ["branch", "--show-current"])
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = git_dirty(repo);
    GitStatus {
        head,
        branch,
        dirty,
    }
}

pub fn git_dirty(repo: &Path) -> bool {
    process::git_stdout(repo, ["status", "--porcelain"])
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub fn tracked_dirty_paths(repo: &Path) -> Vec<String> {
    let Ok(output) = process::git_stdout(repo, ["status", "--porcelain"]) else {
        return Vec::new();
    };
    output
        .lines()
        .filter_map(|line| {
            let status = line.get(0..2)?;
            if status.starts_with("??") {
                return None;
            }
            let path = line.get(3..)?.trim();
            if path.starts_with(".lto/") {
                return None;
            }
            Some(path.to_string())
        })
        .collect()
}

pub fn untracked_paths(repo: &Path) -> Vec<String> {
    let Ok(output) = process::git_stdout(repo, ["status", "--porcelain"]) else {
        return Vec::new();
    };
    output
        .lines()
        .filter_map(|line| {
            if !line.starts_with("??") {
                return None;
            }
            let path = line.get(3..)?.trim();
            if path.starts_with(".lto/") {
                return None;
            }
            Some(path.to_string())
        })
        .collect()
}

pub fn commit_exists(repo: &Path, head: &str) -> bool {
    if head.is_empty() || head == "unknown" {
        return false;
    }
    process::git(repo, ["cat-file", "-e", &format!("{head}^{{commit}}")]).is_ok()
}

pub fn is_ancestor(repo: &Path, old: &str, new: &str) -> bool {
    process::git(repo, ["merge-base", "--is-ancestor", old, new]).is_ok()
}

pub fn head_drift(repo: &Path, recorded: &str, actual: &str) -> &'static str {
    if recorded.is_empty() || recorded == "unknown" || actual.is_empty() || actual == "unknown" {
        return "unreachable";
    }
    if recorded == actual {
        return "none";
    }
    if !commit_exists(repo, recorded) {
        return "unreachable";
    }
    if is_ancestor(repo, recorded, actual) {
        return "forward";
    }
    "rewrite"
}

pub fn git_changed_paths(repo: &Path, old: &str, new: &str) -> Vec<String> {
    process::git_stdout(repo, ["diff", "--name-only", old, new])
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn append_phase_transition(state: &mut LtoState, from: &str, to: &str, head: &str) {
    let transitions = json_array_mut(&mut state.phase_transitions);
    transitions.push(json!({
        "from": from,
        "to": to,
        "at": iso_now(),
        "head": head,
    }));
    state.current_phase = to.to_string();
}

pub fn sync_run_state_md(path: &Path, state: &LtoState) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut content = fs::read_to_string(path)?;
    let delivery_targets = state.delivery_contract.targets.join(" | ");
    let delivery_constraints = state.delivery_contract.constraints.join(" | ");
    let delivery_instruments = state.delivery_contract.instruments.join(" | ");
    let delivery_forced_entropy = state.delivery_contract.forced_entropy.join(" | ");
    let fields = [
        ("run_id", state.run_id.as_str()),
        ("feature / goal", state.goal.as_str()),
        ("started_at", state.started_at.as_str()),
        ("host_runtime", state.host_runtime.as_str()),
        ("repo", state.workspace.repo_root.as_str()),
        ("initial_user_request", state.original_user_request.as_str()),
        ("current_phase", state.current_phase.as_str()),
        ("current_git_head", state.workspace.head.as_str()),
        ("current_branch", state.workspace.branch.as_str()),
        (
            "next_command_or_question",
            state.next_action.as_str().unwrap_or_default(),
        ),
        ("blocked_by", state.blocked_by.as_str().unwrap_or("none")),
        ("delivery_targets", delivery_targets.as_str()),
        ("delivery_constraints", delivery_constraints.as_str()),
        ("delivery_instruments", delivery_instruments.as_str()),
        ("delivery_forced_entropy", delivery_forced_entropy.as_str()),
    ];
    for (field, value) in fields {
        if !value.is_empty() {
            content = replace_md_field(&content, field, value);
        }
    }
    fs::write(path, content)?;
    Ok(())
}

fn replace_md_field(content: &str, field: &str, value: &str) -> String {
    let replacement = format!("- {field}: {}", single_line(value));
    let needle = format!("- {field}:");
    let mut replaced = false;
    let lines = content
        .lines()
        .map(|line| {
            if !replaced && line.starts_with(&needle) {
                replaced = true;
                replacement.clone()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn latest_artifacts(repo: &Path, run_id: &str, limit: usize) -> Vec<Value> {
    let path = repo.join(".lto").join(run_id).join("artifacts.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let mut entries = value
        .get("artifacts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        let aa = a.get("created_at").and_then(Value::as_str).unwrap_or("");
        let bb = b.get("created_at").and_then(Value::as_str).unwrap_or("");
        bb.cmp(aa)
    });
    entries.truncate(limit);
    entries
}

pub fn register_artifact(
    repo: &Path,
    run_id: &str,
    path: &Path,
    meta: ArtifactMeta<'_>,
) -> anyhow::Result<()> {
    let manifest_path = repo.join(".lto").join(run_id).join("artifacts.json");
    let mut manifest = if manifest_path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&manifest_path)?)?
    } else {
        json!({
            "schema_version": 1,
            "run_id": run_id,
            "created_at": iso_now(),
            "updated_at": iso_now(),
            "artifacts": [],
        })
    };
    let entry = artifact_entry(repo, run_id, path, meta.clone())?;
    let artifacts = json_array_mut(
        manifest
            .as_object_mut()
            .ok_or_else(|| anyhow!("artifact manifest is not an object"))?
            .entry("artifacts")
            .or_insert_with(|| Value::Array(Vec::new())),
    );
    let entry_id = entry
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    artifacts.retain(|item| item.get("id").and_then(Value::as_str) != Some(entry_id.as_str()));
    artifacts.push(entry);
    manifest["updated_at"] = json!(iso_now());
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        manifest_path,
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;
    crate::events::safe_emit(
        repo,
        run_id,
        crate::events::EventRecord {
            event_type: "artifact.registered".to_string(),
            actor_kind: "lto".to_string(),
            phase: Some(meta.state.current_phase.clone()),
            object_id: Some(entry_id),
            object_type: Some(meta.kind.to_string()),
            summary: meta.summary.to_string(),
            artifact_refs: vec![repo_relative_path(repo, path).unwrap_or_default()],
            fields: json!({"kind": meta.kind, "producer": meta.producer}),
            ..crate::events::EventRecord::default()
        },
    );
    Ok(())
}

fn artifact_entry(
    repo: &Path,
    run_id: &str,
    path: &Path,
    meta: ArtifactMeta<'_>,
) -> anyhow::Result<Value> {
    let rel = repo_relative_path(repo, path)?;
    let run_prefix = format!(".lto/{run_id}/");
    let run_rel = rel.strip_prefix(&run_prefix).unwrap_or(&rel);
    let id = {
        let mut hasher = Sha256::new();
        hasher.update(format!("{}|{rel}", meta.kind).as_bytes());
        format!("af_{:x}", hasher.finalize())[..19].to_string()
    };
    let mut entry = json!({
        "id": id,
        "kind": meta.kind,
        "relative_path": rel,
        "run_relative_path": run_rel,
        "producer": meta.producer,
        "host_runtime": meta.state.host_runtime,
        "runner": Value::Null,
        "task_id": Value::Null,
        "job_id": Value::Null,
        "phase": meta.state.current_phase,
        "source": "registered",
        "volatile": meta.kind == "changelog",
        "created_at": iso_now(),
        "summary": single_line(meta.summary),
        "consumed_by": [],
        "tags": meta.tags,
    });
    if path.exists() && meta.kind != "changelog" {
        let data = fs::read(path)?;
        entry["bytes"] = json!(data.len());
        entry["sha256"] = json!(format!("{:x}", Sha256::digest(&data)));
    }
    Ok(entry)
}

pub fn repo_relative_path(repo: &Path, path: &Path) -> anyhow::Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    };
    let rel = path
        .canonicalize()
        .or_else(|_| Ok::<PathBuf, std::io::Error>(path.clone()))?
        .strip_prefix(repo.canonicalize()?)
        .with_context(|| format!("path outside repo: {}", path.display()))?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(rel)
}

pub fn read_to_string_lossy(path: &Path) -> anyhow::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

pub fn append_to_object_array(root: &mut Value, key: &str, value: Value) {
    let object = json_object_mut(root);
    let array = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    json_array_mut(array).push(value);
}

pub fn now_for_filename() -> String {
    Local::now().format("%Y-%m-%dT%H-%M-%S").to_string()
}

pub fn run_command_capture(
    repo: &Path,
    command: &str,
    cwd: Option<&Path>,
    timeout_sec: u64,
) -> anyhow::Result<(i32, String, String, f64)> {
    let started = std::time::Instant::now();
    let mut child = process::shell_command(command)
        .current_dir(cwd.unwrap_or(repo))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let timeout = std::time::Duration::from_secs(timeout_sec);
    let mut timed_out = false;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    let rc = if timed_out {
        124
    } else {
        output.status.code().unwrap_or(1)
    };
    Ok((
        rc,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        started.elapsed().as_secs_f64(),
    ))
}

pub fn git_add_plan_commands(tag: &str) -> Vec<String> {
    vec![
        "git add VERSION CHANGELOG.md".to_string(),
        format!("git commit -m 'chore(release): {tag}'"),
        format!("git tag {tag}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, LtoState};
    use serde_json::json;
    use std::process::Command;

    #[test]
    fn load_run_reads_current_marker_and_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(tmp.path().join(".lto").join("current"), "r1\n").unwrap();
        let state = LtoState {
            run_id: "r1".to_string(),
            goal: "load me".to_string(),
            ..LtoState::default()
        };
        state::save_state(run_dir.join("state.json"), &state).unwrap();

        let loaded = load_run(tmp.path(), None).unwrap();
        assert_eq!(loaded.run_id, "r1");
        assert_eq!(loaded.state.goal, "load me");
        assert_eq!(loaded.state_path, run_dir.join("state.json"));
    }

    #[test]
    fn risk_status_helpers_treat_legacy_status_open_as_unverified() {
        let legacy = json!({"id": "R1", "status": "open"});
        assert!(risk_is_open(&legacy));
        assert!(risk_is_open_unverified(&legacy));

        let verified = json!({"id": "R2", "status": "open", "verified_by": "codex"});
        assert!(risk_is_verified(&verified));
        assert!(!risk_is_open_unverified(&verified));

        let closed = json!({"id": "R3", "status": "open", "disposition": "rejected"});
        assert!(!risk_is_open(&closed));
    }

    #[test]
    fn git_status_tracks_dirty_and_untracked_paths_outside_lto() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init"]);
        fs::write(tmp.path().join("tracked.txt"), "tracked\n").unwrap();
        git(tmp.path(), &["add", "tracked.txt"]);
        fs::write(tmp.path().join("untracked.txt"), "untracked\n").unwrap();
        fs::create_dir_all(tmp.path().join(".lto")).unwrap();
        fs::write(tmp.path().join(".lto").join("ignored.txt"), "ignored\n").unwrap();

        let status = git_status(tmp.path());
        assert!(status.dirty);
        assert_ne!(status.branch, "");
        assert!(tracked_dirty_paths(tmp.path()).contains(&"tracked.txt".to_string()));
        assert!(untracked_paths(tmp.path()).contains(&"untracked.txt".to_string()));
        assert!(
            !untracked_paths(tmp.path())
                .iter()
                .any(|path| path.starts_with(".lto/"))
        );
    }

    #[test]
    fn append_phase_transition_and_token_rollup_preserve_state_contracts() {
        let mut state = LtoState {
            agent_runs: json!({
                "j1": [{
                    "job_id": "j1",
                    "runner": "codex",
                    "status": "ok",
                    "cost": {"tokens_in": 5, "tokens_out": 7, "elapsed_sec": 1.5}
                }]
            }),
            ..LtoState::default()
        };
        append_phase_transition(&mut state, "intake", "implementation", "abc");
        assert_eq!(state.current_phase, "implementation");
        assert_eq!(json_array(&state.phase_transitions).len(), 1);

        let rollup = token_rollup(&state);
        assert_eq!(rollup.total_tokens, 12);
        assert_eq!(rollup.runs_with_tokens, 1);
        assert_eq!(rollup.by_runner["codex"].tokens, 12);
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
