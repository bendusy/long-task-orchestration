use crate::agent_job::AgentResult;
use crate::budget::RunBudget;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
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
pub struct DeliveryContract {
    #[serde(default = "default_delivery_schema_version")]
    pub schema_version: u64,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub instruments: Vec<String>,
    #[serde(default)]
    pub forced_entropy: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReadinessAssessment {
    pub missing: Vec<&'static str>,
    pub advisory: Vec<&'static str>,
}

impl RunReadinessAssessment {
    pub fn is_ready(&self) -> bool {
        self.missing.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractCompletenessAssessment {
    pub present: bool,
    pub missing: Vec<&'static str>,
    pub advisory: Vec<&'static str>,
}

impl ContractCompletenessAssessment {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

pub fn assess_run_readiness(
    goal: &str,
    done_when: &str,
    why: &str,
    host: &str,
) -> RunReadinessAssessment {
    let mut missing = Vec::new();
    let mut advisory = Vec::new();
    if goal.trim().is_empty() {
        missing.push("--goal");
    }
    if done_when.trim().is_empty() {
        missing.push("--done-when");
    }
    if why.trim().is_empty() {
        advisory.push("--why");
    }
    if host.trim().is_empty() || host.trim().eq_ignore_ascii_case("unknown") {
        advisory.push("--host");
    }
    RunReadinessAssessment { missing, advisory }
}

impl Default for DeliveryContract {
    fn default() -> Self {
        Self {
            schema_version: default_delivery_schema_version(),
            targets: Vec::new(),
            constraints: Vec::new(),
            instruments: Vec::new(),
            forced_entropy: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl DeliveryContract {
    pub fn new(
        targets: Vec<String>,
        constraints: Vec<String>,
        instruments: Vec<String>,
        forced_entropy: Vec<String>,
    ) -> Self {
        Self {
            targets: clean_list(targets),
            constraints: clean_list(constraints),
            instruments: clean_instruments(instruments),
            forced_entropy: clean_list(forced_entropy),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
            && self.constraints.is_empty()
            && self.instruments.is_empty()
            && self.forced_entropy.is_empty()
            && self.extra.is_empty()
    }

    pub fn missing_sections(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.targets.is_empty() {
            missing.push("targets");
        }
        if self.constraints.is_empty() {
            missing.push("constraints");
        }
        if self.instruments.is_empty() {
            missing.push("instruments");
        }
        if self.forced_entropy.is_empty() {
            missing.push("forced_entropy");
        }
        missing
    }

    pub fn completeness_missing(&self) -> ContractCompletenessAssessment {
        let present = !self.targets.is_empty()
            || !self.constraints.is_empty()
            || !self.instruments.is_empty()
            || !self.forced_entropy.is_empty();
        if !present {
            return ContractCompletenessAssessment {
                present: false,
                missing: Vec::new(),
                advisory: Vec::new(),
            };
        }

        let mut missing = Vec::new();
        let has_valid_instrument = self
            .instruments
            .iter()
            .any(|instrument| instrument_has_command(instrument));
        let has_invalid_instrument = self
            .instruments
            .iter()
            .any(|instrument| !instrument_has_command(instrument));
        if self.targets.is_empty() && !has_valid_instrument {
            missing.extend(["--target", "--instrument"]);
        } else {
            if self.targets.is_empty() {
                missing.push("--target");
            }
            if !self.targets.is_empty() && !has_valid_instrument {
                missing.push("--instrument");
            }
        }
        if has_invalid_instrument && !missing.contains(&"--instrument") {
            missing.push("--instrument");
        }
        let mut advisory = Vec::new();
        if self.constraints.is_empty() {
            advisory.push("--constraint");
        }
        if self.forced_entropy.is_empty() {
            advisory.push("--entropy-check");
        }
        ContractCompletenessAssessment {
            present,
            missing,
            advisory,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.completeness_missing().is_complete()
    }
}

fn default_delivery_schema_version() -> u64 {
    1
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_instruments(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

fn instrument_has_command(instrument: &str) -> bool {
    let instrument = instrument.trim();
    match instrument.split_once("::") {
        Some((_label, command)) => !command.trim().is_empty(),
        None => !instrument.is_empty(),
    }
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
    #[serde(default, skip_serializing_if = "DeliveryContract::is_empty")]
    pub delivery_contract: DeliveryContract,
    #[serde(default)]
    pub last_failure: Value,
    #[serde(default)]
    pub user_decisions: Value,
    #[serde(default)]
    pub next_action: Value,
    #[serde(default)]
    pub blocked_by: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispatch_windows: Vec<DispatchWindowState>,
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
            delivery_contract: DeliveryContract::default(),
            last_failure: Value::Null,
            user_decisions: Value::Array(Vec::new()),
            next_action: Value::Null,
            blocked_by: Value::Null,
            notify_cmd: None,
            dispatch_windows: Vec::new(),
            artifacts: Value::Object(Map::new()),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchWindowState {
    pub window_id: String,
    pub target: String,
    pub runner: String,
    pub tmux_bin: String,
    pub cleanup_on_success: bool,
    pub status: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_reason: Option<String>,
}

pub fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

pub fn load_state(path: impl AsRef<Path>) -> anyhow::Result<LtoState> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save_state(path: impl AsRef<Path>, state: &LtoState) -> anyhow::Result<()> {
    let path = path.as_ref();
    let contents = serde_json::to_string_pretty(state)? + "\n";
    atomic_write(path, contents.as_bytes())
}

fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state.json");
    let existing_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    if let Some(permissions) = existing_permissions {
        temp.as_file().set_permissions(permissions)?;
    }
    temp.write_all(contents)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
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

pub fn agent_results_from_agent_runs(agent_runs: &Value) -> Vec<AgentResult> {
    let mut results = Vec::new();
    match agent_runs {
        Value::Object(by_job) => {
            for runs in by_job.values() {
                if let Value::Array(entries) = runs {
                    results.extend(entries.iter().filter_map(parse_agent_result));
                }
            }
        }
        Value::Array(entries) => {
            results.extend(entries.iter().filter_map(parse_agent_result));
        }
        _ => {}
    }
    results
}

fn parse_agent_result(value: &Value) -> Option<AgentResult> {
    serde_json::from_value(value.clone()).ok()
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

    #[test]
    fn run_readiness_separates_required_and_advisory_flags() {
        let missing = assess_run_readiness("  ", "", "", "unknown");
        assert!(!missing.is_ready());
        assert_eq!(missing.missing, vec!["--goal", "--done-when"]);
        assert_eq!(missing.advisory, vec!["--why", "--host"]);

        let ready = assess_run_readiness("ship", "tests pass", "user value", "codex");
        assert!(ready.is_ready());
        assert!(ready.missing.is_empty());
        assert!(ready.advisory.is_empty());
    }

    #[test]
    fn contract_completeness_requires_target_and_instrument_as_a_pair() {
        let target_only = DeliveryContract::new(vec!["ship".into()], vec![], vec![], vec![]);
        let assessment = target_only.completeness_missing();
        assert!(assessment.present);
        assert_eq!(assessment.missing, vec!["--instrument"]);
        assert_eq!(assessment.advisory, vec!["--constraint", "--entropy-check"]);
        assert!(!target_only.is_complete());

        let instrument_only =
            DeliveryContract::new(vec![], vec![], vec!["cargo test".into()], vec![]);
        assert_eq!(
            instrument_only.completeness_missing().missing,
            vec!["--target"]
        );
        assert!(!instrument_only.is_complete());

        for optional_only in [
            DeliveryContract::new(vec![], vec!["bounded".into()], vec![], vec![]),
            DeliveryContract::new(vec![], vec![], vec![], vec!["change hypothesis".into()]),
        ] {
            assert_eq!(
                optional_only.completeness_missing().missing,
                vec!["--target", "--instrument"]
            );
            assert!(!optional_only.is_complete());
        }

        let paired = DeliveryContract::new(
            vec!["ship".into()],
            vec![],
            vec!["cargo test".into()],
            vec![],
        );
        let assessment = paired.completeness_missing();
        assert!(assessment.is_complete());
        assert_eq!(assessment.advisory, vec!["--constraint", "--entropy-check"]);
        assert!(paired.is_complete());
    }

    #[test]
    fn contract_completeness_requires_every_instrument_to_have_a_command() {
        for valid in [
            "cargo test --locked",
            "::cargo test --locked",
            "smoke::cargo test --locked",
            "smoke::cargo test --locked::all",
        ] {
            let contract =
                DeliveryContract::new(vec!["ship".into()], vec![], vec![valid.into()], vec![]);
            assert!(contract.is_complete(), "expected valid instrument: {valid}");
        }

        for invalid in ["", "   ", "label::", "::", "label::   "] {
            let contract =
                DeliveryContract::new(vec!["ship".into()], vec![], vec![invalid.into()], vec![]);
            assert_eq!(
                contract.completeness_missing().missing,
                vec!["--instrument"],
                "expected invalid instrument: {invalid:?}"
            );
        }

        let mixed = DeliveryContract::new(
            vec!["ship".into()],
            vec![],
            vec!["smoke::cargo test".into(), "lint::".into()],
            vec![],
        );
        assert_eq!(mixed.completeness_missing().missing, vec!["--instrument"]);
    }

    #[test]
    fn save_state_atomically_replaces_existing_state() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        fs::write(&path, "not json\n").unwrap();
        let state = LtoState {
            run_id: "r1".into(),
            goal: "atomic replacement".into(),
            ..LtoState::default()
        };

        save_state(&path, &state).unwrap();

        assert_eq!(load_state(&path).unwrap(), state);
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "temporary file should be removed after persist"
        );
    }

    #[test]
    fn empty_and_extra_only_contracts_are_ordinary_empty_contracts() {
        let empty = DeliveryContract::default();
        let assessment = empty.completeness_missing();
        assert!(!assessment.present);
        assert!(assessment.is_complete());
        assert!(assessment.advisory.is_empty());

        let mut extra_only = DeliveryContract::default();
        extra_only.extra.insert(
            "future_contract_key".into(),
            serde_json::json!({"kept": true}),
        );
        assert!(!extra_only.is_empty());
        let assessment = extra_only.completeness_missing();
        assert!(!assessment.present);
        assert!(assessment.is_complete());
        assert!(assessment.advisory.is_empty());
        assert!(extra_only.is_complete());

        let encoded = serde_json::to_value(extra_only).unwrap();
        assert_eq!(encoded["future_contract_key"]["kept"], Value::Bool(true));
    }

    #[test]
    fn old_agent_runs_without_dimensions_load_as_unknown() {
        let raw = serde_json::json!({
            "j1": [{
                "job_id": "j1",
                "runner": "codex",
                "status": "ok",
                "reply_text": "done"
            }]
        });
        let results = agent_results_from_agent_runs(&raw);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_type, None);
        assert_eq!(results[0].size, crate::agent_job::TaskSize::Unknown);
        let cells = crate::dispatch::build_cells_from_results(&results);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].task_type, "unknown");
        assert_eq!(cells[0].size, crate::agent_job::TaskSize::Unknown);
    }
}
