use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

pub const KNOWN_RUNNERS: &[&str] = &["codex", "pi", "agy", "gemini", "claude"];
pub const CODEX_SANDBOXES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AgentJobError {
    #[error("unknown runner: {0}")]
    UnknownRunner(String),
    #[error("invalid sandbox: {0}")]
    InvalidSandbox(String),
    #[error("workspace-write requires permission_policy.reason")]
    MissingWorkspaceWriteReason,
    #[error("danger-full-access requires permission_policy.reason")]
    MissingDangerReason,
    #[error("danger-full-access requires user_approved=true")]
    MissingDangerApproval,
    #[error("CODEX_SANDBOX conflicts with permission_policy.sandbox ({actual} != {expected})")]
    CodexSandboxConflict { actual: String, expected: String },
    #[error("{runner} cannot enforce read-only; defer it for read-only jobs")]
    CannotEnforceReadOnly { runner: String },
    #[error("{runner} read-only tools exceed allowlist: {extra:?}")]
    ReadOnlyToolsExceeded { runner: String, extra: Vec<String> },
    #[error("prompt_ref is required")]
    MissingPromptRef,
    #[error("invalid isolation: {0}")]
    InvalidIsolation(String),
    #[error("invalid parent_pattern: {0}")]
    InvalidPattern(String),
    #[error("env keys and values must be strings")]
    InvalidEnv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sandbox {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl Sandbox {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

impl fmt::Display for Sandbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Sandbox {
    type Err = AgentJobError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read-only" => Ok(Self::ReadOnly),
            "workspace-write" => Ok(Self::WorkspaceWrite),
            "danger-full-access" => Ok(Self::DangerFullAccess),
            other => Err(AgentJobError::InvalidSandbox(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSize {
    Small,
    Medium,
    Large,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Ok,
    Failed,
    Timeout,
    RateLimited,
    Skipped,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitState {
    Ok(i32),
    Failed(i32),
    SignalKilled,
    Timeout,
}

impl ExitState {
    pub fn from_status_code(code: Option<i32>, timed_out: bool) -> Self {
        if timed_out {
            return Self::Timeout;
        }
        match code {
            Some(0) => Self::Ok(0),
            Some(124) => Self::Timeout,
            Some(code) => Self::Failed(code),
            None => Self::SignalKilled,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    #[default]
    Linear,
    FanOut,
    Adversarial,
    Tournament,
    Loop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub tokens: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub sandbox: Sandbox,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub user_approved: bool,
    #[serde(default)]
    pub tools: Vec<String>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            sandbox: Sandbox::ReadOnly,
            reason: String::new(),
            user_approved: false,
            tools: Vec::new(),
        }
    }
}

impl PermissionPolicy {
    pub fn validate_for_runner(
        &self,
        runner: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<(), AgentJobError> {
        if !KNOWN_RUNNERS.contains(&runner) {
            return Err(AgentJobError::UnknownRunner(runner.to_string()));
        }

        match self.sandbox {
            Sandbox::ReadOnly => {}
            Sandbox::WorkspaceWrite if self.reason.trim().is_empty() => {
                return Err(AgentJobError::MissingWorkspaceWriteReason);
            }
            Sandbox::WorkspaceWrite => {}
            Sandbox::DangerFullAccess if self.reason.trim().is_empty() => {
                return Err(AgentJobError::MissingDangerReason);
            }
            Sandbox::DangerFullAccess if !self.user_approved => {
                return Err(AgentJobError::MissingDangerApproval);
            }
            Sandbox::DangerFullAccess => {}
        }

        match runner {
            "codex" => {
                let actual = env
                    .get("CODEX_SANDBOX")
                    .map(String::as_str)
                    .unwrap_or_else(|| self.sandbox.as_str());
                if actual != self.sandbox.as_str() {
                    return Err(AgentJobError::CodexSandboxConflict {
                        actual: actual.to_string(),
                        expected: self.sandbox.to_string(),
                    });
                }
            }
            "agy" | "gemini" if self.sandbox == Sandbox::ReadOnly => {
                return Err(AgentJobError::CannotEnforceReadOnly {
                    runner: runner.to_string(),
                });
            }
            "claude" | "pi" if self.sandbox == Sandbox::ReadOnly && !self.tools.is_empty() => {
                let allow = readonly_tool_allowlist(runner);
                let mut extra = self
                    .tools
                    .iter()
                    .filter(|tool| !allow.contains(&tool.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                extra.sort();
                if !extra.is_empty() {
                    return Err(AgentJobError::ReadOnlyToolsExceeded {
                        runner: runner.to_string(),
                        extra,
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn effective_readonly_tools(&self, runner: &str) -> Vec<String> {
        if self.sandbox != Sandbox::ReadOnly || !matches!(runner, "claude" | "pi") {
            return Vec::new();
        }
        if !self.tools.is_empty() {
            return self.tools.clone();
        }
        readonly_tool_allowlist(runner)
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    }
}

pub fn readonly_tool_allowlist(runner: &str) -> &'static [&'static str] {
    match runner {
        "claude" => &["Glob", "Grep", "Read", "WebFetch"],
        "pi" => &["find", "grep", "ls", "read"],
        _ => &[],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default = "default_timeout")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

fn default_timeout() -> u64 {
    300
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            timeout_sec: default_timeout(),
            max_tokens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub backoff_sec: f64,
    #[serde(default = "default_retry_on")]
    pub retry_on: Vec<JobStatus>,
}

fn default_max_retries() -> u32 {
    1
}

fn default_backoff() -> f64 {
    5.0
}

fn default_retry_on() -> Vec<JobStatus> {
    vec![JobStatus::RateLimited, JobStatus::Timeout]
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            backoff_sec: default_backoff(),
            retry_on: default_retry_on(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentJob {
    pub job_id: String,
    pub prompt_ref: String,
    #[serde(default = "default_runner")]
    pub runner: String,
    #[serde(default)]
    pub prompt_is_inline: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub permission_policy: PermissionPolicy,
    #[serde(default = "default_isolation")]
    pub isolation: String,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub parent_pattern: Pattern,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    #[serde(default)]
    pub verifier_of: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
    #[serde(default)]
    pub task_type: Option<String>,
    #[serde(default)]
    pub size: TaskSize,
    #[serde(default)]
    pub meta: BTreeMap<String, serde_json::Value>,
}

fn default_runner() -> String {
    "codex".to_string()
}

fn default_isolation() -> String {
    "none".to_string()
}

impl AgentJob {
    pub fn validate(&self) -> Result<(), AgentJobError> {
        if !KNOWN_RUNNERS.contains(&self.runner.as_str()) {
            return Err(AgentJobError::UnknownRunner(self.runner.clone()));
        }
        if !matches!(self.isolation.as_str(), "none" | "worktree") {
            return Err(AgentJobError::InvalidIsolation(self.isolation.clone()));
        }
        if self.prompt_ref.is_empty() {
            return Err(AgentJobError::MissingPromptRef);
        }
        self.permission_policy
            .validate_for_runner(&self.runner, &self.env)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResult {
    pub job_id: String,
    pub runner: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_status")]
    pub status: JobStatus,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub findings: Vec<serde_json::Value>,
    #[serde(default)]
    pub reply_text: String,
    #[serde(default)]
    pub cost: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub permissions: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub task_type: Option<String>,
    #[serde(default)]
    pub size: TaskSize,
}

fn default_status() -> JobStatus {
    JobStatus::Pending
}

fn default_attempts() -> u32 {
    1
}

impl AgentResult {
    pub fn ok(&self) -> bool {
        self.status == JobStatus::Ok
    }

    pub fn severity_counts(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::from([
            ("critical".to_string(), 0),
            ("high".to_string(), 0),
            ("medium".to_string(), 0),
            ("low".to_string(), 0),
        ]);
        for finding in &self.findings {
            if let Some(sev) = finding.get("severity").and_then(|v| v.as_str()) {
                let key = sev.to_ascii_lowercase();
                if let Some(count) = counts.get_mut(&key) {
                    *count += 1;
                }
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_sandbox_env_must_match_policy() {
        let policy = PermissionPolicy::default();
        let env = BTreeMap::from([("CODEX_SANDBOX".to_string(), "workspace-write".to_string())]);
        assert!(matches!(
            policy.validate_for_runner("codex", &env),
            Err(AgentJobError::CodexSandboxConflict { .. })
        ));
    }

    #[test]
    fn agy_readonly_fails_closed() {
        let policy = PermissionPolicy::default();
        assert!(matches!(
            policy.validate_for_runner("agy", &BTreeMap::new()),
            Err(AgentJobError::CannotEnforceReadOnly { .. })
        ));
    }

    #[test]
    fn danger_full_requires_reason_and_approval() {
        let policy = PermissionPolicy {
            sandbox: Sandbox::DangerFullAccess,
            ..PermissionPolicy::default()
        };
        assert_eq!(
            policy.validate_for_runner("codex", &BTreeMap::new()),
            Err(AgentJobError::MissingDangerReason)
        );
        let policy = PermissionPolicy {
            sandbox: Sandbox::DangerFullAccess,
            reason: "release publish".to_string(),
            user_approved: false,
            tools: Vec::new(),
        };
        assert_eq!(
            policy.validate_for_runner("codex", &BTreeMap::new()),
            Err(AgentJobError::MissingDangerApproval)
        );
    }

    #[test]
    fn signal_killed_is_not_empty_reply_or_timeout() {
        assert_eq!(
            ExitState::from_status_code(None, false),
            ExitState::SignalKilled
        );
        assert_eq!(
            ExitState::from_status_code(Some(124), false),
            ExitState::Timeout
        );
        assert_eq!(
            ExitState::from_status_code(Some(1), false),
            ExitState::Failed(1)
        );
    }
}
