use crate::agent_job::{AgentJob, AgentJobError, AgentResult, JobStatus, RetryPolicy};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerConfig {
    pub max_total_agents: usize,
    pub max_backoff_sec: f64,
    pub total_retry_wall_sec: f64,
    pub healthcheck_retries: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_total_agents: 50,
            max_backoff_sec: 60.0,
            total_retry_wall_sec: 300.0,
            healthcheck_retries: 1,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("job count {count} exceeds max_total_agents={limit}")]
    TooManyJobs { count: usize, limit: usize },
    #[error("duplicate job_id: {0}")]
    DuplicateJobId(String),
    #[error("job {job_id:?} invalid: {source}")]
    InvalidJob {
        job_id: String,
        source: AgentJobError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedExit {
    pub exit_code: i32,
    pub status: JobStatus,
    pub error: String,
}

const RATE_LIMIT_MARKERS: &[&str] = &[
    "429",
    "too many requests",
    "rate limit",
    "rate_limit",
    "rate limited",
];

pub fn validate_batch(jobs: &[AgentJob], config: &SchedulerConfig) -> Result<(), SchedulerError> {
    if jobs.len() > config.max_total_agents {
        return Err(SchedulerError::TooManyJobs {
            count: jobs.len(),
            limit: config.max_total_agents,
        });
    }
    let mut seen = BTreeSet::new();
    for job in jobs {
        if !seen.insert(job.job_id.clone()) {
            return Err(SchedulerError::DuplicateJobId(job.job_id.clone()));
        }
        job.validate()
            .map_err(|source| SchedulerError::InvalidJob {
                job_id: job.job_id.clone(),
                source,
            })?;
    }
    Ok(())
}

pub fn classify_exit(exit_code: i32, reply_text: &str, stderr: &str) -> ClassifiedExit {
    if exit_code == 0 {
        if reply_text.is_empty() {
            return ClassifiedExit {
                exit_code,
                status: JobStatus::Failed,
                error: "exit 0 but empty reply".to_string(),
            };
        }
        return ClassifiedExit {
            exit_code,
            status: JobStatus::Ok,
            error: String::new(),
        };
    }

    if contains_rate_limit_marker(stderr) || contains_rate_limit_marker(reply_text) {
        return ClassifiedExit {
            exit_code,
            status: JobStatus::RateLimited,
            error: format!("rate limited (exit={exit_code})"),
        };
    }

    if exit_code == 124 {
        return ClassifiedExit {
            exit_code,
            status: JobStatus::Timeout,
            error: "timeout (exit 124)".to_string(),
        };
    }

    let mut error = format!("exit code {exit_code}");
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        let mut tail = stderr.chars().take(500).collect::<String>();
        if stderr.chars().count() > 500 {
            tail.push_str("...");
        }
        error.push_str(": ");
        error.push_str(&tail);
    }
    ClassifiedExit {
        exit_code,
        status: JobStatus::Failed,
        error,
    }
}

fn contains_rate_limit_marker(text: &str) -> bool {
    RATE_LIMIT_MARKERS
        .iter()
        .any(|marker| contains_ascii_case_insensitive(text, marker))
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

pub fn result_from_attempt(
    job: &AgentJob,
    exit_code: i32,
    reply_text: impl Into<String>,
    stderr: &str,
    attempts: u32,
) -> AgentResult {
    let reply_text = reply_text.into();
    let classified = classify_exit(exit_code, &reply_text, stderr);
    AgentResult {
        job_id: job.job_id.clone(),
        runner: job.runner.clone(),
        model: job.model.clone(),
        status: classified.status,
        exit_code: Some(classified.exit_code),
        findings: vec![],
        reply_text,
        cost: BTreeMap::new(),
        permissions: BTreeMap::new(),
        artifacts: vec![],
        attempts,
        error: classified.error,
        task_type: job.task_type.clone(),
        size: job.size,
    }
}

pub fn skipped_for_unhealthy(job: &AgentJob) -> AgentResult {
    AgentResult {
        job_id: job.job_id.clone(),
        runner: job.runner.clone(),
        model: job.model.clone(),
        status: JobStatus::Skipped,
        exit_code: None,
        findings: vec![],
        reply_text: String::new(),
        cost: BTreeMap::new(),
        permissions: BTreeMap::new(),
        artifacts: vec![],
        attempts: 0,
        error: format!("runner unhealthy: {}", job.runner),
        task_type: job.task_type.clone(),
        size: job.size,
    }
}

pub fn next_backoff_sec(
    policy: &RetryPolicy,
    retry_number: u32,
    config: &SchedulerConfig,
    retry_sleep_elapsed: f64,
    job_timeout_sec: u64,
) -> Option<f64> {
    if retry_number == 0 || retry_number > policy.max_retries {
        return None;
    }
    let raw = policy.backoff_sec * 2_f64.powi((retry_number - 1) as i32);
    let capped = raw.min(config.max_backoff_sec);
    let retry_budget = config.total_retry_wall_sec.min(job_timeout_sec as f64);
    if retry_sleep_elapsed + capped > retry_budget {
        None
    } else {
        Some(capped)
    }
}

pub fn retry_delay_after_attempt(
    status: JobStatus,
    policy: &RetryPolicy,
    retries_already_used: u32,
    config: &SchedulerConfig,
    retry_sleep_elapsed: f64,
    job_timeout_sec: u64,
) -> Option<f64> {
    if !policy.retry_on.contains(&status) {
        return None;
    }
    next_backoff_sec(
        policy,
        retries_already_used + 1,
        config,
        retry_sleep_elapsed,
        job_timeout_sec,
    )
}

pub type HealthProbe = BTreeMap<String, bool>;

pub fn healthcheck_with_retries(
    runners: &[String],
    probes: &[HealthProbe],
    healthcheck_retries: u32,
) -> HealthProbe {
    let mut healthy = runners
        .iter()
        .map(|runner| {
            (
                runner.clone(),
                probes
                    .first()
                    .and_then(|probe| probe.get(runner))
                    .copied()
                    .unwrap_or(false),
            )
        })
        .collect::<HealthProbe>();
    for probe in probes.iter().skip(1).take(healthcheck_retries as usize) {
        let still_bad = runners
            .iter()
            .filter(|runner| !healthy.get(*runner).copied().unwrap_or(false))
            .cloned()
            .collect::<Vec<_>>();
        if still_bad.is_empty() {
            break;
        }
        for runner in still_bad {
            if probe.get(&runner).copied().unwrap_or(false) {
                healthy.insert(runner, true);
            }
        }
    }
    healthy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::{Budget, PermissionPolicy, Sandbox, TaskSize};

    fn job(id: &str) -> AgentJob {
        AgentJob {
            job_id: id.to_string(),
            prompt_ref: format!("prompts/{id}.md"),
            runner: "codex".to_string(),
            prompt_is_inline: false,
            model: Some("gpt-5".to_string()),
            env: BTreeMap::new(),
            permission_policy: PermissionPolicy {
                sandbox: Sandbox::ReadOnly,
                ..PermissionPolicy::default()
            },
            isolation: "none".to_string(),
            output_schema: None,
            parent_pattern: crate::agent_job::Pattern::Linear,
            budget: Budget {
                timeout_sec: 300,
                max_tokens: None,
            },
            retry_policy: RetryPolicy::default(),
            verifier_of: None,
            children: vec![],
            task_type: Some("audit".to_string()),
            size: TaskSize::Small,
            meta: BTreeMap::new(),
        }
    }

    #[test]
    fn classify_exit_keeps_429_in_successful_reply_as_content() {
        let got = classify_exit(0, "API docs mention 429 backoff", "");
        assert_eq!(got.status, JobStatus::Ok);
        let got = classify_exit(1, "", "ERROR: 429 Too Many Requests");
        assert_eq!(got.status, JobStatus::RateLimited);
    }

    #[test]
    fn classify_exit_separates_empty_reply_timeout_and_generic_failure() {
        assert_eq!(classify_exit(0, "", "").status, JobStatus::Failed);
        assert_eq!(classify_exit(124, "", "").status, JobStatus::Timeout);
        let got = classify_exit(2, "", "runner missing");
        assert_eq!(got.status, JobStatus::Failed);
        assert!(got.error.contains("runner missing"));
    }

    #[test]
    fn validate_batch_rejects_duplicate_ids_and_over_cap() {
        let config = SchedulerConfig {
            max_total_agents: 1,
            ..SchedulerConfig::default()
        };
        assert!(matches!(
            validate_batch(&[job("a"), job("b")], &config),
            Err(SchedulerError::TooManyJobs { .. })
        ));
        assert!(matches!(
            validate_batch(&[job("a"), job("a")], &SchedulerConfig::default()),
            Err(SchedulerError::DuplicateJobId(id)) if id == "a"
        ));
    }

    #[test]
    fn retry_backoff_uses_exponential_caps_and_total_budget() {
        let policy = RetryPolicy {
            max_retries: 10,
            backoff_sec: 5.0,
            retry_on: vec![JobStatus::RateLimited],
        };
        let config = SchedulerConfig {
            max_backoff_sec: 0.3,
            total_retry_wall_sec: 0.8,
            ..SchedulerConfig::default()
        };
        assert_eq!(
            retry_delay_after_attempt(JobStatus::RateLimited, &policy, 0, &config, 0.0, 300),
            Some(0.3)
        );
        assert_eq!(
            retry_delay_after_attempt(JobStatus::RateLimited, &policy, 2, &config, 0.6, 300),
            None
        );
        assert_eq!(
            retry_delay_after_attempt(JobStatus::Failed, &policy, 0, &config, 0.0, 300),
            None
        );
    }

    #[test]
    fn healthcheck_retries_only_need_one_later_ok_for_bad_runner() {
        let runners = vec!["codex".to_string(), "pi".to_string()];
        let probes = vec![
            BTreeMap::from([("codex".to_string(), true), ("pi".to_string(), false)]),
            BTreeMap::from([("pi".to_string(), true)]),
        ];
        let healthy = healthcheck_with_retries(&runners, &probes, 1);
        assert!(healthy["codex"]);
        assert!(healthy["pi"]);
    }

    #[test]
    fn result_from_attempt_preserves_job_dimensions() {
        let job = job("a");
        let result = result_from_attempt(&job, 0, "ok", "", 1);
        assert_eq!(result.status, JobStatus::Ok);
        assert_eq!(result.model.as_deref(), Some("gpt-5"));
        assert_eq!(result.task_type.as_deref(), Some("audit"));
        assert_eq!(result.size, TaskSize::Small);
    }
}
