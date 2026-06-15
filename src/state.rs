use crate::budget::RunBudget;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorkspaceSnapshot {
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub dirty_fingerprint: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EnvironmentSnapshot {
    #[serde(default)]
    pub sandbox: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub mcp_services: Vec<String>,
    #[serde(default)]
    pub write_roots: Vec<String>,
    #[serde(default)]
    pub captured_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LtoState {
    #[serde(default)]
    pub schema_version: u64,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub done_when: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub host_runtime: String,
    #[serde(default)]
    pub workspace: WorkspaceSnapshot,
    #[serde(default)]
    pub environment_snapshot: EnvironmentSnapshot,
    #[serde(default)]
    pub current_phase: String,
    #[serde(default)]
    pub original_user_request: String,
    #[serde(default)]
    pub phase_transitions: Value,
    #[serde(default)]
    pub tasks: Value,
    #[serde(default)]
    pub active_task_id: Value,
    #[serde(default)]
    pub risk_points: Value,
    #[serde(default)]
    pub agent_runs: Value,
    #[serde(default)]
    pub decision_escalate_points: Value,
    #[serde(default)]
    pub gates: Value,
    #[serde(default)]
    pub budget: RunBudget,
    #[serde(default)]
    pub last_failure: Value,
    #[serde(default)]
    pub user_decisions: Value,
    #[serde(default)]
    pub next_action: Value,
    #[serde(default)]
    pub blocked_by: Value,
    #[serde(default)]
    pub artifacts: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for LtoState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            run_id: String::new(),
            goal: String::new(),
            why: String::new(),
            done_when: String::new(),
            started_at: iso_now(),
            host_runtime: String::new(),
            workspace: WorkspaceSnapshot::default(),
            environment_snapshot: EnvironmentSnapshot::default(),
            current_phase: "intake".to_string(),
            original_user_request: String::new(),
            phase_transitions: Value::Array(Vec::new()),
            tasks: Value::Array(Vec::new()),
            active_task_id: Value::Null,
            risk_points: Value::Array(Vec::new()),
            agent_runs: Value::Array(Vec::new()),
            decision_escalate_points: Value::Array(Vec::new()),
            gates: Value::Object(Map::new()),
            budget: RunBudget {
                warn_ratio: 0.8,
                ..RunBudget::default()
            },
            last_failure: Value::Null,
            user_decisions: Value::Array(Vec::new()),
            next_action: Value::Null,
            blocked_by: Value::Null,
            artifacts: Value::Object(Map::new()),
            extra: Map::new(),
        }
    }
}

pub fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

pub fn load_state(path: impl AsRef<Path>) -> anyhow::Result<LtoState> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save_state(path: impl AsRef<Path>, state: &LtoState) -> anyhow::Result<()> {
    fs::write(path, serde_json::to_string_pretty(state)? + "\n")?;
    Ok(())
}

pub fn state_path(repo: &Path, run_id: &str) -> PathBuf {
    repo.join(".lto").join(run_id).join("state.json")
}

pub fn validate_run_id(run_id: &str) -> anyhow::Result<&str> {
    if run_id.is_empty()
        || run_id.contains('/')
        || run_id.contains('\\')
        || run_id.contains("..")
        || run_id.starts_with('.')
    {
        anyhow::bail!("invalid run_id: {run_id:?}");
    }
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_top_level_keys_survive_roundtrip() {
        let raw = r#"{
          "schema_version": 1,
          "run_id": "r1",
          "goal": "g",
          "budget": {"max_tokens": 10, "turns_used": 1, "warn_ratio": 0.8},
          "future_key": {"kept": true}
        }"#;
        let parsed: LtoState = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.extra["future_key"]["kept"], Value::Bool(true));
        let out = serde_json::to_value(parsed).unwrap();
        assert_eq!(out["future_key"]["kept"], Value::Bool(true));
    }

    #[test]
    fn bad_run_ids_are_rejected() {
        assert!(validate_run_id("ok-123").is_ok());
        assert!(validate_run_id("../x").is_err());
        assert!(validate_run_id("a/b").is_err());
    }
}
