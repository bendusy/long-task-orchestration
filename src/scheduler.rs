use crate::agent_job::{
    AgentJob, AgentJobError, AgentResult, JobStatus, PermissionPolicy, RetryPolicy, Sandbox,
    TaskSize,
};
use crate::dispatch::TaskDescriptor;
use crate::merge_review::{self, TestGateAction};
use crate::tmux_runner;
use crate::worktree::{self, WorktreeHandle};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use thiserror::Error;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{MissedTickBehavior, sleep, timeout};

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerConfig {
    pub max_total_agents: usize,
    pub max_concurrency: usize,
    pub max_backoff_sec: f64,
    pub total_retry_wall_sec: f64,
    pub healthcheck_retries: u32,
    pub stall_timeout_sec: Option<u64>,
    pub heartbeat_interval_sec: u64,
    pub drain_after_exit_timeout_sec: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_total_agents: 50,
            max_concurrency: 4,
            max_backoff_sec: 60.0,
            total_retry_wall_sec: 300.0,
            healthcheck_retries: 1,
            stall_timeout_sec: None,
            heartbeat_interval_sec: 30,
            drain_after_exit_timeout_sec: 5,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("job count {count} exceeds max_total_agents={limit}")]
    TooManyJobs { count: usize, limit: usize },
    #[error("duplicate job_id: {0}")]
    DuplicateJobId(String),
    #[error("job dependency graph has a cycle or unsatisfied dependency")]
    DependencyCycle,
    #[error("job {job_id:?} invalid: {source}")]
    InvalidJob {
        job_id: String,
        source: AgentJobError,
    },
    #[error("scheduler task join failed: {0}")]
    Join(String),
    #[error("scheduler runtime failed: {0}")]
    Runtime(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HealthcheckError {
    #[error("healthcheck script not found: {0}")]
    ScriptUnavailable(String),
    #[error("healthcheck command failed to start: {0}")]
    Io(String),
    #[error("healthcheck command returned non-zero: {0}")]
    NonZero(String),
    #[error("healthcheck command returned invalid JSON: {0}")]
    InvalidJson(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedExit {
    pub exit_code: i32,
    pub status: JobStatus,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    pub repo: PathBuf,
    pub runners_dir: PathBuf,
    pub config: SchedulerConfig,
}

struct WorktreeCleanupGuard {
    repo: PathBuf,
    handle: Option<WorktreeHandle>,
}

impl WorktreeCleanupGuard {
    fn new(repo: &Path, handle: WorktreeHandle) -> Self {
        Self {
            repo: repo.to_path_buf(),
            handle: Some(handle),
        }
    }

    fn handle(&self) -> &WorktreeHandle {
        self.handle.as_ref().expect("worktree guard handle present")
    }

    fn disarm(mut self) -> WorktreeHandle {
        self.handle.take().expect("worktree guard handle present")
    }
}

impl Drop for WorktreeCleanupGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = worktree::prune_worktree(&self.repo, &handle);
        }
    }
}

const RATE_LIMIT_MARKERS: &[&str] = &[
    "429",
    "too many requests",
    "rate limit",
    "rate_limit",
    "rate limited",
];

impl Scheduler {
    pub fn new(repo: impl Into<PathBuf>, runners_dir: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            runners_dir: runners_dir.into(),
            config: SchedulerConfig::default(),
        }
    }

    pub fn with_config(
        repo: impl Into<PathBuf>,
        runners_dir: impl Into<PathBuf>,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            repo: repo.into(),
            runners_dir: runners_dir.into(),
            config,
        }
    }

    pub fn submit_blocking(&self, jobs: Vec<AgentJob>) -> Result<Vec<AgentResult>, SchedulerError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|err| SchedulerError::Runtime(err.to_string()))?;
        runtime.block_on(self.submit(jobs))
    }

    pub async fn submit(&self, jobs: Vec<AgentJob>) -> Result<Vec<AgentResult>, SchedulerError> {
        validate_batch(&jobs, &self.config)?;
        let plan = DependencyPlan::new(&jobs)?;
        let runners = unique_runners(&jobs);
        let health = self.healthcheck(&runners).await;
        let scheduler = Arc::new(self.clone());
        let permits = Arc::new(Semaphore::new(self.config.max_concurrency.max(1)));
        let mut results = vec![None; jobs.len()];

        if plan.has_edges() {
            run_dependency_plan(scheduler, permits, jobs, &health, &plan, &mut results).await?;
        } else {
            let ready = jobs.into_iter().enumerate().collect::<Vec<_>>();
            for (idx, result) in run_parallel_jobs(scheduler, permits, ready, &health).await? {
                results[idx] = Some(result);
            }
        }

        Ok(results
            .into_iter()
            .map(|result| result.expect("all scheduler slots filled"))
            .collect())
    }

    pub async fn healthcheck(&self, runners: &[String]) -> HealthProbe {
        let mut healthy = match self.run_healthcheck_probe_checked(runners).await {
            Ok(health) => health,
            Err(_) => return empty_health_probe(runners),
        };
        for _ in 0..self.config.healthcheck_retries {
            let still_bad = runners
                .iter()
                .filter(|runner| !healthy.get(*runner).copied().unwrap_or(false))
                .cloned()
                .collect::<Vec<_>>();
            if still_bad.is_empty() {
                break;
            }
            let retry = self
                .run_healthcheck_probe_checked(&still_bad)
                .await
                .unwrap_or_else(|_| empty_health_probe(&still_bad));
            for runner in still_bad {
                if retry.get(&runner).copied().unwrap_or(false) {
                    healthy.insert(runner, true);
                }
            }
        }
        healthy
    }

    pub async fn healthcheck_checked(
        &self,
        runners: &[String],
    ) -> Result<HealthProbe, HealthcheckError> {
        let mut healthy = self.run_healthcheck_probe_checked(runners).await?;
        for _ in 0..self.config.healthcheck_retries {
            let still_bad = runners
                .iter()
                .filter(|runner| !healthy.get(*runner).copied().unwrap_or(false))
                .cloned()
                .collect::<Vec<_>>();
            if still_bad.is_empty() {
                break;
            }
            let retry = self.run_healthcheck_probe_checked(&still_bad).await?;
            for runner in still_bad {
                if retry.get(&runner).copied().unwrap_or(false) {
                    healthy.insert(runner, true);
                }
            }
        }
        Ok(healthy)
    }

    async fn run_healthcheck_probe_checked(
        &self,
        runners: &[String],
    ) -> Result<HealthProbe, HealthcheckError> {
        let mut health = empty_health_probe(runners);
        let script = self.runners_dir.join("healthcheck.sh");
        if fs::metadata(&script).await.is_err() {
            return Err(HealthcheckError::ScriptUnavailable(
                script.display().to_string(),
            ));
        }
        let output = Command::new(&script)
            .arg("--json")
            .args(runners)
            .output()
            .await;
        let output = output.map_err(|err| HealthcheckError::Io(err.to_string()))?;
        if !output.status.success() {
            return Err(HealthcheckError::NonZero(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        let entries = serde_json::from_slice::<Vec<HealthcheckEntry>>(&output.stdout)
            .map_err(|err| HealthcheckError::InvalidJson(err.to_string()))?;
        if entries.len() > runners.len() {
            return Err(HealthcheckError::InvalidJson(
                "more health entries than requested runners".to_string(),
            ));
        }
        for entry in entries {
            if let std::collections::btree_map::Entry::Occupied(mut slot) =
                health.entry(entry.agent)
            {
                slot.insert(entry.verdict.eq_ignore_ascii_case("OK"));
            }
        }
        Ok(health)
    }

    async fn run_job(self: Arc<Self>, job: AgentJob) -> AgentResult {
        let mut retry_sleep_elapsed = 0.0;
        let max_attempts = job.retry_policy.max_retries.saturating_add(1);
        let mut last = None;
        let mut retry_attempts = Vec::new();
        for attempt_idx in 0..max_attempts {
            let attempt = attempt_idx.saturating_add(1);
            let mut result = self.run_once(&job, attempt).await;
            let delay = retry_delay_after_attempt(
                result.status,
                &job.retry_policy,
                attempt_idx,
                &self.config,
                retry_sleep_elapsed,
                job.budget.timeout_sec,
            );
            if let Some(delay) = delay {
                retry_attempts.push(json!({
                    "attempt": attempt,
                    "status": result.status.as_str(),
                    "exit_code": result.exit_code,
                    "delay_sec": delay,
                }));
                retry_sleep_elapsed += delay;
                last = Some(result);
                sleep(Duration::from_secs_f64(delay)).await;
            } else {
                if !retry_attempts.is_empty() {
                    result
                        .cost
                        .insert("retry_attempts".to_string(), json!(retry_attempts));
                }
                return result;
            }
        }
        let mut result =
            last.unwrap_or_else(|| failure_result(&job, 0, None, "no attempt executed"));
        if !retry_attempts.is_empty() {
            result
                .cost
                .insert("retry_attempts".to_string(), json!(retry_attempts));
        }
        result
    }

    async fn run_once(&self, job: &AgentJob, attempt: u32) -> AgentResult {
        let started = Instant::now();
        let run_id = job
            .meta
            .get("run_id")
            .and_then(|value| value.as_str())
            .unwrap_or("rust-scheduler");
        let live_log_path = self
            .repo
            .join(".lto")
            .join(run_id)
            .join("live")
            .join(format!("{}.log", job.job_id));
        let heartbeat_path = self
            .repo
            .join(".lto")
            .join(run_id)
            .join("live")
            .join(format!("{}.hb.jsonl", job.job_id));

        let attempt_dir = match tempfile::Builder::new().prefix("lto_scheduler_").tempdir() {
            Ok(dir) => dir,
            Err(err) => return failure_result(job, attempt, None, format!("tempdir: {err}")),
        };
        let prompt = match materialize_prompt(job, &self.repo, &attempt_dir).await {
            Ok(prompt) => prompt,
            Err(err) => return failure_result(job, attempt, None, err),
        };
        let effective_job = with_effective_dimensions(job, &prompt.path).await;
        let job = &effective_job;
        if job.runner == "tmux" {
            if job_needs_worktree(job) {
                return failure_result(
                    job,
                    attempt,
                    None,
                    "tmux runner does not support scheduler-managed worktree isolation; target a pane already in the intended workspace or use a headless runner",
                );
            }
            let mut result = match tmux_runner::run_job(job, &prompt.path, &self.repo).await {
                Ok(outcome) => AgentResult {
                    job_id: job.job_id.clone(),
                    runner: job.runner.clone(),
                    model: job.model.clone(),
                    status: outcome.status,
                    exit_code: outcome.exit_code,
                    findings: vec![],
                    reply_text: outcome.reply_text,
                    cost: outcome.cost,
                    permissions: BTreeMap::new(),
                    artifacts: outcome.artifacts,
                    attempts: attempt,
                    error: outcome.error,
                    task_type: job.task_type.clone(),
                    size: job.size,
                    merge_review: None,
                },
                Err(err) => failure_result(job, attempt, None, err.to_string()),
            };
            result.permissions = permission_snapshot(&job.permission_policy);
            return result;
        }

        let runner = self.runners_dir.join(format!("{}.sh", job.runner));
        if fs::metadata(&runner).await.is_err() {
            return failure_result(
                job,
                attempt,
                None,
                format!("runner script not found: {}", runner.display()),
            );
        }

        let write_worktree = if job_needs_worktree(job) {
            match worktree::add_persistent_worktree(&self.repo, run_id, &job.job_id) {
                Ok(handle) => Some(WorktreeCleanupGuard::new(&self.repo, handle)),
                Err(err) => {
                    return failure_result(
                        job,
                        attempt,
                        None,
                        format!("persistent worktree: {err}"),
                    );
                }
            }
        } else {
            None
        };
        let command_dir = write_worktree
            .as_ref()
            .map(|guard| guard.handle().path.as_path())
            .unwrap_or(self.repo.as_path());
        let reply_path = attempt_dir.path().join("reply.txt");
        let timeout_arg = job.budget.timeout_sec.to_string();

        if let Some(parent) = live_log_path.parent()
            && let Err(err) = fs::create_dir_all(parent).await
        {
            return failure_result(job, attempt, None, format!("live log dir: {err}"));
        }
        let log = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&live_log_path)
            .await
        {
            Ok(file) => Arc::new(Mutex::new(file)),
            Err(err) => return failure_result(job, attempt, None, format!("live log: {err}")),
        };

        let mut command = Command::new(&runner);
        command
            .arg(prompt.path.as_os_str())
            .arg(reply_path.as_os_str())
            .arg(timeout_arg)
            .current_dir(command_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .envs(runner_env(job));
        #[cfg(unix)]
        command.process_group(0);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => return failure_result(job, attempt, None, format!("spawn: {err}")),
        };
        let child_id = child.id();
        let last_output_at = Arc::new(AtomicU64::new(now_millis()));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = tokio::spawn(drain_pipe(
            stdout,
            Arc::clone(&log),
            Arc::clone(&last_output_at),
        ));
        let stderr_task = tokio::spawn(drain_pipe(
            stderr,
            Arc::clone(&log),
            Arc::clone(&last_output_at),
        ));
        let (stop_heartbeat, heartbeat_task) = spawn_heartbeat(
            heartbeat_path.clone(),
            job.job_id.clone(),
            self.config.heartbeat_interval_sec,
        );

        let wait_outcome = wait_with_deadlines(
            &mut child,
            Duration::from_secs(job.budget.timeout_sec),
            self.config.stall_timeout_sec.map(Duration::from_secs),
            Arc::clone(&last_output_at),
        )
        .await;
        let _ = stop_heartbeat.send(());
        let _ = heartbeat_task.await;
        let _ = fs::remove_file(&heartbeat_path).await;
        if matches!(
            &wait_outcome,
            WaitOutcome::TimedOut | WaitOutcome::Stalled | WaitOutcome::WaitFailed(_)
        ) {
            kill_child_group(child_id).await;
            let _ = timeout(
                Duration::from_secs(self.config.drain_after_exit_timeout_sec.max(1)),
                child.wait(),
            )
            .await;
        }

        let drain_timeout = Duration::from_secs(self.config.drain_after_exit_timeout_sec.max(1));
        let (stdout_text, stdout_drain_error) =
            collect_drain_task(stdout_task, "stdout", drain_timeout).await;
        let (stderr_text, stderr_drain_error) =
            collect_drain_task(stderr_task, "stderr", drain_timeout).await;
        let reply_text = fs::read_to_string(&reply_path).await.unwrap_or_default();
        let elapsed = started.elapsed().as_secs_f64();

        let mut result = match wait_outcome {
            WaitOutcome::Exited(status) => match status.code() {
                Some(code) => result_from_attempt(job, code, reply_text, &stderr_text, attempt),
                None => failure_result(job, attempt, None, "runner terminated by signal"),
            },
            WaitOutcome::TimedOut => timeout_result(job, attempt, reply_text, "timeout"),
            WaitOutcome::Stalled => {
                timeout_result(job, attempt, reply_text, "stalled without output")
            }
            WaitOutcome::WaitFailed(err) => failure_result(job, attempt, None, err),
        };
        result
            .cost
            .insert("elapsed_sec".to_string(), json!(elapsed));
        merge_reply_meta_sidecar(&reply_path, &mut result.cost).await;
        result
            .cost
            .insert("stdout_bytes".to_string(), json!(stdout_text.len()));
        if let Some(err) = stdout_drain_error {
            result
                .cost
                .insert("stdout_drain_error".to_string(), json!(err));
        }
        if let Some(err) = stderr_drain_error {
            result
                .cost
                .insert("stderr_drain_error".to_string(), json!(err));
        }
        result.permissions = permission_snapshot(&job.permission_policy);
        if let Some(guard) = write_worktree {
            result = self.finalize_write_task(job, result, guard.disarm());
        }
        result
    }

    fn finalize_write_task(
        &self,
        job: &AgentJob,
        mut result: AgentResult,
        mut handle: WorktreeHandle,
    ) -> AgentResult {
        if result.status != JobStatus::Ok {
            let _ = worktree::prune_worktree(&self.repo, &handle);
            return result;
        }

        let diff = match merge_review::emit_diff(&handle, job.test_cmd.as_deref()) {
            Ok(diff) => diff,
            Err(err) => {
                let _ = worktree::prune_worktree(&self.repo, &handle);
                result.status = JobStatus::Failed;
                result.error = format!("emit diff: {err}");
                return result;
            }
        };
        if diff.diff.trim().is_empty() {
            let _ = worktree::prune_worktree(&self.repo, &handle);
            result.status = JobStatus::Failed;
            result.error = "write task produced no worktree changes".to_string();
            return result;
        }

        let gate = merge_review::test_gate_action(&diff);
        let review = merge_review::build_merge_review(diff);
        handle.keep = true;
        result
            .artifacts
            .push(format!("worktree:{}", handle.path.display()));
        result.merge_review = Some(review);
        result.cost.insert(
            "test_gate_action".to_string(),
            json!(match gate {
                TestGateAction::Batchable => "batchable",
                TestGateAction::ImmediateReport => "immediate_report",
            }),
        );
        if gate == TestGateAction::ImmediateReport {
            result.status = JobStatus::Failed;
            result.error =
                "test gate failed; merge review requires immediate host attention".to_string();
        }
        result
    }
}

#[derive(Debug, Clone)]
struct DependencyPlan {
    parents_by_idx: Vec<BTreeSet<usize>>,
}

impl DependencyPlan {
    fn new(jobs: &[AgentJob]) -> Result<Self, SchedulerError> {
        let index = jobs
            .iter()
            .enumerate()
            .map(|(idx, job)| (job.job_id.clone(), idx))
            .collect::<BTreeMap<_, _>>();
        let mut parents_by_idx = vec![BTreeSet::new(); jobs.len()];
        for (parent_idx, parent) in jobs.iter().enumerate() {
            for child in &parent.children {
                let Some(child_idx) = index.get(child).copied() else {
                    continue;
                };
                parents_by_idx[child_idx].insert(parent_idx);
            }
        }
        Ok(Self { parents_by_idx })
    }

    fn has_edges(&self) -> bool {
        self.parents_by_idx
            .iter()
            .any(|parents| !parents.is_empty())
    }
}

async fn run_dependency_plan(
    scheduler: Arc<Scheduler>,
    permits: Arc<Semaphore>,
    jobs: Vec<AgentJob>,
    health: &HealthProbe,
    plan: &DependencyPlan,
    results: &mut [Option<AgentResult>],
) -> Result<(), SchedulerError> {
    let mut remaining = (0..jobs.len()).collect::<BTreeSet<_>>();
    while !remaining.is_empty() {
        let blocked = dependency_blocked_jobs(&remaining, results, plan);
        for (idx, reason) in blocked {
            results[idx] = Some(skipped_for_dependency(&jobs[idx], reason));
            remaining.remove(&idx);
        }

        let ready = remaining
            .iter()
            .copied()
            .filter(|idx| dependencies_satisfied(&plan.parents_by_idx[*idx], results))
            .map(|idx| (idx, jobs[idx].clone()))
            .collect::<Vec<_>>();
        if ready.is_empty() {
            if remaining.is_empty() {
                break;
            }
            return Err(SchedulerError::DependencyCycle);
        }
        for (idx, result) in
            run_parallel_jobs(Arc::clone(&scheduler), Arc::clone(&permits), ready, health).await?
        {
            results[idx] = Some(result);
            remaining.remove(&idx);
        }
    }
    Ok(())
}

async fn run_parallel_jobs(
    scheduler: Arc<Scheduler>,
    permits: Arc<Semaphore>,
    jobs: Vec<(usize, AgentJob)>,
    health: &HealthProbe,
) -> Result<Vec<(usize, AgentResult)>, SchedulerError> {
    let mut out = Vec::with_capacity(jobs.len());
    let mut set = JoinSet::new();
    for (idx, job) in jobs {
        if !health.get(&job.runner).copied().unwrap_or(false) {
            out.push((idx, skipped_for_unhealthy(&job)));
            continue;
        }
        let scheduler = Arc::clone(&scheduler);
        let permits = Arc::clone(&permits);
        set.spawn(async move {
            let _permit = permits
                .acquire_owned()
                .await
                .expect("scheduler semaphore closed unexpectedly");
            let result = scheduler.run_job(job).await;
            (idx, result)
        });
    }
    while let Some(joined) = set.join_next().await {
        out.push(joined.map_err(|err| SchedulerError::Join(err.to_string()))?);
    }
    Ok(out)
}

fn dependency_blocked_jobs(
    remaining: &BTreeSet<usize>,
    results: &[Option<AgentResult>],
    plan: &DependencyPlan,
) -> Vec<(usize, String)> {
    let mut blocked = Vec::new();
    for idx in remaining {
        for parent_idx in &plan.parents_by_idx[*idx] {
            let Some(parent) = results[*parent_idx].as_ref() else {
                continue;
            };
            if parent.status != JobStatus::Ok {
                blocked.push((
                    *idx,
                    format!(
                        "dependency {} did not complete successfully ({:?})",
                        parent.job_id, parent.status
                    ),
                ));
                break;
            }
            if parent.merge_review.is_some() {
                blocked.push((
                    *idx,
                    format!(
                        "dependency {} produced a merge review; host must merge it before this job opens",
                        parent.job_id
                    ),
                ));
                break;
            }
        }
    }
    blocked
}

fn dependencies_satisfied(parents: &BTreeSet<usize>, results: &[Option<AgentResult>]) -> bool {
    parents.iter().all(|idx| {
        matches!(
            results.get(*idx).and_then(Option::as_ref),
            Some(result) if result.status == JobStatus::Ok && result.merge_review.is_none()
        )
    })
}

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
        merge_review: None,
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
        merge_review: None,
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

fn empty_health_probe(runners: &[String]) -> HealthProbe {
    runners
        .iter()
        .map(|runner| (runner.clone(), false))
        .collect()
}

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

#[derive(Debug, Deserialize)]
struct HealthcheckEntry {
    agent: String,
    verdict: String,
}

struct MaterializedPrompt {
    path: PathBuf,
}

async fn materialize_prompt(
    job: &AgentJob,
    repo: &Path,
    attempt_dir: &TempDir,
) -> Result<MaterializedPrompt, String> {
    if job.prompt_is_inline {
        let path = attempt_dir.path().join("prompt.md");
        fs::write(&path, &job.prompt_ref)
            .await
            .map_err(|err| format!("write inline prompt: {err}"))?;
        return Ok(MaterializedPrompt { path });
    }
    let path = PathBuf::from(&job.prompt_ref);
    let path = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    if fs::metadata(&path).await.is_err() {
        return Err(format!("prompt_ref not found: {}", path.display()));
    }
    Ok(MaterializedPrompt { path })
}

async fn with_effective_dimensions(job: &AgentJob, prompt_path: &Path) -> AgentJob {
    if job.size != TaskSize::Unknown {
        return job.clone();
    }
    let prompt_tokens = estimate_prompt_tokens(prompt_path).await;
    let expected_output_tokens = job.budget.max_tokens.unwrap_or_default();
    let task_type = job.task_type.as_deref().unwrap_or("unknown");
    let descriptor = TaskDescriptor::from_tokens(task_type, prompt_tokens, expected_output_tokens);
    let mut effective = job.clone();
    effective.size = descriptor.size;
    effective
}

async fn estimate_prompt_tokens(path: &Path) -> u64 {
    let Ok(text) = fs::read_to_string(path).await else {
        return 0;
    };
    ((text.chars().count() as u64).saturating_add(3)) / 4
}

fn job_needs_worktree(job: &AgentJob) -> bool {
    job.needs_worktree
        || job.isolation == "worktree"
        || (job.permission_policy.sandbox == Sandbox::WorkspaceWrite && is_write_task_type(job))
}

fn is_write_task_type(job: &AgentJob) -> bool {
    matches!(
        job.task_type.as_deref(),
        Some("write" | "implementation" | "feature" | "migration" | "refactor")
    )
}

fn runner_env(job: &AgentJob) -> BTreeMap<String, String> {
    let mut env = job.env.clone();
    env.insert(
        "CODEX_SANDBOX".to_string(),
        job.permission_policy.sandbox.as_str().to_string(),
    );
    if let Some(model) = &job.model {
        env.insert("CODEX_MODEL".to_string(), model.clone());
    }
    env
}

async fn merge_reply_meta_sidecar(path: &Path, cost: &mut BTreeMap<String, serde_json::Value>) {
    let meta_path = PathBuf::from(format!("{}.meta.json", path.to_string_lossy()));
    let Ok(text) = fs::read_to_string(meta_path).await else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(obj) = value.as_object() else {
        return;
    };
    for (key, value) in obj {
        cost.insert(key.clone(), value.clone());
    }
}

fn permission_snapshot(policy: &PermissionPolicy) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("sandbox".to_string(), json!(policy.sandbox.as_str())),
        ("reason".to_string(), json!(policy.reason)),
        ("user_approved".to_string(), json!(policy.user_approved)),
    ])
}

fn failure_result(
    job: &AgentJob,
    attempts: u32,
    exit_code: Option<i32>,
    error: impl Into<String>,
) -> AgentResult {
    AgentResult {
        job_id: job.job_id.clone(),
        runner: job.runner.clone(),
        model: job.model.clone(),
        status: JobStatus::Failed,
        exit_code,
        findings: vec![],
        reply_text: String::new(),
        cost: BTreeMap::new(),
        permissions: permission_snapshot(&job.permission_policy),
        artifacts: vec![],
        attempts,
        error: error.into(),
        task_type: job.task_type.clone(),
        size: job.size,
        merge_review: None,
    }
}

fn timeout_result(
    job: &AgentJob,
    attempts: u32,
    reply_text: String,
    error: impl Into<String>,
) -> AgentResult {
    AgentResult {
        status: JobStatus::Timeout,
        exit_code: Some(124),
        reply_text,
        error: error.into(),
        ..failure_result(job, attempts, Some(124), "")
    }
}

fn skipped_for_dependency(job: &AgentJob, reason: String) -> AgentResult {
    AgentResult {
        job_id: job.job_id.clone(),
        runner: job.runner.clone(),
        model: job.model.clone(),
        status: JobStatus::Skipped,
        exit_code: None,
        findings: vec![],
        reply_text: String::new(),
        cost: BTreeMap::new(),
        permissions: permission_snapshot(&job.permission_policy),
        artifacts: vec![],
        attempts: 0,
        error: reason,
        task_type: job.task_type.clone(),
        size: job.size,
        merge_review: None,
    }
}

enum WaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Stalled,
    WaitFailed(String),
}

async fn wait_with_deadlines(
    child: &mut tokio::process::Child,
    timeout: Duration,
    stall_timeout: Option<Duration>,
    last_output_at: Arc<AtomicU64>,
) -> WaitOutcome {
    let wait = child.wait();
    tokio::pin!(wait);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        tokio::select! {
            status = &mut wait => {
                return match status {
                    Ok(status) => WaitOutcome::Exited(status),
                    Err(err) => WaitOutcome::WaitFailed(err.to_string()),
                };
            }
            _ = tokio::time::sleep_until(deadline) => {
                return WaitOutcome::TimedOut;
            }
            _ = sleep(Duration::from_millis(100)), if stall_timeout.is_some() => {
                let Some(stall_timeout) = stall_timeout else {
                    continue;
                };
                let quiet_for = now_millis().saturating_sub(last_output_at.load(Ordering::Relaxed));
                if quiet_for >= millis_u64(stall_timeout) {
                    return WaitOutcome::Stalled;
                }
            }
        }
    }
}

async fn drain_pipe<R>(
    reader: Option<R>,
    log: Arc<Mutex<tokio::fs::File>>,
    last_output_at: Arc<AtomicU64>,
) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return Ok(String::new());
    };
    let mut output = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        last_output_at.store(now_millis(), Ordering::Relaxed);
        output.extend_from_slice(&buf[..n]);
        let mut file = log.lock().await;
        file.write_all(&buf[..n]).await?;
        file.flush().await?;
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

async fn collect_drain_task(
    mut task: JoinHandle<std::io::Result<String>>,
    label: &str,
    deadline: Duration,
) -> (String, Option<String>) {
    match timeout(deadline, &mut task).await {
        Ok(Ok(Ok(text))) => (text, None),
        Ok(Ok(Err(err))) => (String::new(), Some(format!("{label} drain error: {err}"))),
        Ok(Err(err)) => (
            String::new(),
            Some(format!("{label} drain task join error: {err}")),
        ),
        Err(_) => {
            task.abort();
            let _ = task.await;
            (
                String::new(),
                Some(format!(
                    "{label} drain timed out after {:.3}s",
                    deadline.as_secs_f64()
                )),
            )
        }
    }
}

fn spawn_heartbeat(
    path: PathBuf,
    job_id: String,
    interval_sec: u64,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let (tx, mut rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        fs::create_dir_all(parent).await?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut interval = tokio::time::interval(Duration::from_secs(interval_sec.max(1)));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut rx => return Ok(()),
                _ = interval.tick() => {
                    let line = json!({
                        "job_id": job_id,
                        "ts_ms": now_millis(),
                    });
                    file.write_all(line.to_string().as_bytes()).await?;
                    file.write_all(b"\n").await?;
                    file.flush().await?;
                }
            }
        }
    });
    (tx, handle)
}

#[cfg(unix)]
async fn kill_child_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{pid}"))
            .output()
            .await;
    }
}

#[cfg(not(unix))]
async fn kill_child_group(_pid: Option<u32>) {}

fn unique_runners(jobs: &[AgentJob]) -> Vec<String> {
    jobs.iter()
        .map(|job| job.runner.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn millis_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::{Budget, Pattern, Sandbox, TaskSize};
    use crate::process;
    use std::fs as std_fs;
    use std::io::Write;
    use std::time::Instant;

    struct Harness {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        runners_dir: PathBuf,
        control: PathBuf,
        health: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path().join("repo");
            let runners_dir = tmp.path().join("runners");
            std_fs::create_dir_all(&repo).unwrap();
            std_fs::create_dir_all(&runners_dir).unwrap();
            let control = tmp.path().join("control.json");
            let health = tmp.path().join("health.json");
            write_fake_runner(tmp.path(), &runners_dir);
            write_healthcheck(&runners_dir, &health);
            let harness = Self {
                _tmp: tmp,
                repo,
                runners_dir,
                control,
                health,
            };
            harness.set_health(json!([{"agent":"codex","verdict":"OK"}]));
            harness.set_control(json!({}));
            harness
        }

        fn scheduler(&self) -> Scheduler {
            Scheduler::with_config(
                &self.repo,
                &self.runners_dir,
                SchedulerConfig {
                    max_concurrency: 2,
                    heartbeat_interval_sec: 1,
                    ..SchedulerConfig::default()
                },
            )
        }

        fn scheduler_with(&self, config: SchedulerConfig) -> Scheduler {
            Scheduler::with_config(&self.repo, &self.runners_dir, config)
        }

        fn set_control(&self, value: serde_json::Value) {
            std_fs::write(&self.control, value.to_string()).unwrap();
        }

        fn set_health(&self, value: serde_json::Value) {
            let _ = std_fs::remove_file(self.health.with_extension("json.callcount"));
            std_fs::write(&self.health, value.to_string()).unwrap();
        }

        fn job(&self, id: &str) -> AgentJob {
            make_job(id, "codex")
        }
    }

    fn make_job(id: &str, runner: &str) -> AgentJob {
        AgentJob {
            job_id: id.to_string(),
            prompt_ref: format!("# JOB_ID:{id}\nTest prompt for {id}"),
            runner: runner.to_string(),
            prompt_is_inline: true,
            model: Some("gpt-5".to_string()),
            env: BTreeMap::new(),
            permission_policy: PermissionPolicy {
                sandbox: if matches!(runner, "agy" | "gemini") {
                    Sandbox::WorkspaceWrite
                } else {
                    Sandbox::ReadOnly
                },
                reason: if matches!(runner, "agy" | "gemini") {
                    "test workspace-write".to_string()
                } else {
                    String::new()
                },
                ..PermissionPolicy::default()
            },
            isolation: "none".to_string(),
            output_schema: None,
            parent_pattern: Pattern::Linear,
            budget: Budget {
                timeout_sec: 30,
                max_tokens: None,
            },
            retry_policy: RetryPolicy {
                max_retries: 0,
                ..RetryPolicy::default()
            },
            verifier_of: None,
            children: vec![],
            task_type: Some("audit".to_string()),
            size: TaskSize::Small,
            test_cmd: None,
            needs_worktree: false,
            meta: BTreeMap::new(),
        }
    }

    fn write_fake_runner(tmp: &Path, runners_dir: &Path) {
        let fake_runner = tmp.join("fake_runner.py");
        std_fs::write(
            &fake_runner,
            r##"#!/usr/bin/env python3
import json, os, subprocess, sys, time
prompt_file, reply_file, timeout_sec = sys.argv[1:4]
with open(prompt_file) as f:
    first_line = f.readline().strip()
job_id = first_line.replace("# JOB_ID:", "").strip()
ctrl = os.environ.get("SCHEDULER_TEST_CONTROL", "")
data = {}
if ctrl and os.path.exists(ctrl):
    with open(ctrl) as f:
        data = json.load(f)
behaviour = data.get(job_id, {})
if behaviour.get("escape_stdout_holder"):
    pid_file = behaviour["pid_file"]
    child_code = (
        "import os, sys, time; "
        "os.setsid(); "
        "sys.stdout.write('escaped child holds stdout\\n'); sys.stdout.flush(); "
        "time.sleep(30)"
    )
    child = subprocess.Popen([sys.executable, "-c", child_code], stdout=sys.stdout, stderr=sys.stderr, close_fds=False)
    with open(pid_file, "w") as f:
        f.write(str(child.pid))
time.sleep(float(behaviour.get("sleep", 0)))
output = str(behaviour.get("output", ""))
if "output_env" in behaviour:
    output = json.dumps({k: os.environ.get(k) for k in behaviour["output_env"]}, sort_keys=True)
if "write_file" in behaviour:
    target = behaviour["write_file"]
    parent = os.path.dirname(target)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(target, "w") as f:
        f.write(str(behaviour.get("write_content", "")))
with open(reply_file, "w") as f:
    f.write(output)
sys.exit(int(behaviour.get("exit_code", 0)))
"##,
        )
        .unwrap();
        for runner in ["codex", "pi", "agy", "gemini", "claude"] {
            let wrapper = runners_dir.join(format!("{runner}.sh"));
            let mut file = std_fs::File::create(&wrapper).unwrap();
            writeln!(file, "#!/usr/bin/env bash").unwrap();
            writeln!(
                file,
                "export SCHEDULER_TEST_CONTROL=\"{}\"",
                tmp.join("control.json").display()
            )
            .unwrap();
            writeln!(file, "exec python3 \"{}\" \"$@\"", fake_runner.display()).unwrap();
            make_executable(&wrapper);
        }
    }

    fn write_healthcheck(runners_dir: &Path, health: &Path) {
        let fake_healthcheck = health
            .parent()
            .expect("health path has parent")
            .join("fake_healthcheck.py");
        std_fs::write(
            &fake_healthcheck,
            r##"#!/usr/bin/env python3
import json, os, sys
path = os.environ["SCHEDULER_TEST_HEALTHCHECK"]
requested = [arg for arg in sys.argv[1:] if arg != "--json"]
with open(path) as f:
    data = json.load(f)
if isinstance(data, dict) and "__sequence__" in data:
    count_path = path + ".callcount"
    try:
        with open(count_path) as f:
            idx = int(f.read().strip() or "0")
    except Exception:
        idx = 0
    with open(count_path, "w") as f:
        f.write(str(idx + 1))
    seq = data["__sequence__"]
    data = seq[idx] if idx < len(seq) else seq[-1]
if requested and isinstance(data, list) and all(isinstance(e, dict) for e in data):
    data = [e for e in data if e.get("agent") in requested]
print(json.dumps(data))
"##,
        )
        .unwrap();
        let script = runners_dir.join("healthcheck.sh");
        let mut file = std_fs::File::create(&script).unwrap();
        writeln!(file, "#!/usr/bin/env bash").unwrap();
        writeln!(
            file,
            "export SCHEDULER_TEST_HEALTHCHECK=\"{}\"",
            health.display()
        )
        .unwrap();
        writeln!(
            file,
            "exec python3 \"{}\" \"$@\"",
            fake_healthcheck.display()
        )
        .unwrap();
        make_executable(&script);
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std_fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std_fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn init_git_repo(path: &Path) {
        process::git(path, ["init", "-q"]).unwrap();
        process::git(path, ["config", "user.name", "T"]).unwrap();
        process::git(path, ["config", "user.email", "t@example.com"]).unwrap();
        std_fs::write(path.join("base.txt"), "base\n").unwrap();
        process::git(path, ["add", "."]).unwrap();
        process::git(path, ["commit", "-q", "-m", "init"]).unwrap();
    }

    fn write_job(harness: &Harness, id: &str) -> AgentJob {
        let mut job = harness.job(id);
        job.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            reason: "implementation write task".to_string(),
            user_approved: true,
            tools: Vec::new(),
        };
        job.task_type = Some("write".to_string());
        job.size = TaskSize::Unknown;
        job.needs_worktree = true;
        job.test_cmd = Some("true".to_string());
        job
    }

    fn worktree_path(result: &AgentResult) -> PathBuf {
        let entry = result
            .artifacts
            .iter()
            .find_map(|artifact| artifact.strip_prefix("worktree:"))
            .expect("worktree artifact present");
        PathBuf::from(entry)
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
        let harness = Harness::new();
        let config = SchedulerConfig {
            max_total_agents: 1,
            ..SchedulerConfig::default()
        };
        assert!(matches!(
            validate_batch(&[harness.job("a"), harness.job("b")], &config),
            Err(SchedulerError::TooManyJobs { .. })
        ));
        assert!(matches!(
            validate_batch(&[harness.job("a"), harness.job("a")], &SchedulerConfig::default()),
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
        let harness = Harness::new();
        let job = harness.job("a");
        let result = result_from_attempt(&job, 0, "ok", "", 1);
        assert_eq!(result.status, JobStatus::Ok);
        assert_eq!(result.model.as_deref(), Some("gpt-5"));
        assert_eq!(result.task_type.as_deref(), Some("audit"));
        assert_eq!(result.size, TaskSize::Small);
    }

    #[tokio::test]
    async fn submit_respects_concurrency_cap_and_order() {
        let harness = Harness::new();
        harness.set_control(json!({
            "c1_j0": {"exit_code": 0, "output": "ok 0", "sleep": 0.3},
            "c1_j1": {"exit_code": 0, "output": "ok 1", "sleep": 0.3},
            "c1_j2": {"exit_code": 0, "output": "ok 2", "sleep": 0.3},
            "c1_j3": {"exit_code": 0, "output": "ok 3", "sleep": 0.3},
            "c1_j4": {"exit_code": 0, "output": "ok 4", "sleep": 0.3}
        }));
        let jobs = (0..5)
            .map(|idx| harness.job(&format!("c1_j{idx}")))
            .collect::<Vec<_>>();
        let results = harness.scheduler().submit(jobs).await.unwrap();
        assert!(results.iter().all(AgentResult::ok));
        assert_eq!(results[4].job_id, "c1_j4");
    }

    #[tokio::test]
    async fn exit_zero_empty_reply_is_failed() {
        let harness = Harness::new();
        harness.set_control(json!({"c2_empty": {"exit_code": 0, "output": ""}}));
        let result = harness
            .scheduler()
            .submit(vec![harness.job("c2_empty")])
            .await
            .unwrap();
        assert_eq!(result[0].status, JobStatus::Failed);
        assert!(result[0].error.contains("empty reply"));
    }

    #[tokio::test]
    async fn rate_limit_retries_exhaust_and_backoff_sleeps() {
        let harness = Harness::new();
        harness.set_control(json!({"c3_rl": {"exit_code": 1, "output": "error 429 Too Many Requests", "sleep": 0.01}}));
        let mut job = harness.job("c3_rl");
        job.retry_policy = RetryPolicy {
            max_retries: 2,
            backoff_sec: 0.05,
            retry_on: vec![JobStatus::RateLimited, JobStatus::Timeout],
        };
        let result = harness.scheduler().submit(vec![job]).await.unwrap();
        assert_eq!(result[0].status, JobStatus::RateLimited);
        assert_eq!(result[0].attempts, 3);

        harness.set_control(
            json!({"c3_timing": {"exit_code": 1, "output": "429 rate limit", "sleep": 0.01}}),
        );
        let mut timing = harness.job("c3_timing");
        timing.retry_policy = RetryPolicy {
            max_retries: 3,
            backoff_sec: 0.1,
            retry_on: vec![JobStatus::RateLimited],
        };
        let start = Instant::now();
        let _ = harness.scheduler().submit(vec![timing]).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(500));
    }

    #[tokio::test]
    async fn exit_124_is_timeout() {
        let harness = Harness::new();
        harness.set_control(json!({"c4_to": {"exit_code": 124, "output": ""}}));
        let result = harness
            .scheduler()
            .submit(vec![harness.job("c4_to")])
            .await
            .unwrap();
        assert_eq!(result[0].status, JobStatus::Timeout);
        assert_eq!(result[0].exit_code, Some(124));
    }

    #[tokio::test]
    async fn unhealthy_runner_is_skipped_and_reprobe_can_recover() {
        let harness = Harness::new();
        harness.set_health(json!([
            {"agent":"codex","verdict":"OK"},
            {"agent":"agy","verdict":"TIMEOUT"}
        ]));
        harness.set_control(json!({"c6_ok": {"exit_code": 0, "output": "ok"}}));
        let results = harness
            .scheduler()
            .submit(vec![harness.job("c6_ok"), make_job("c6_bad", "agy")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Ok);
        assert_eq!(results[1].status, JobStatus::Skipped);

        harness.set_health(json!({"__sequence__": [
            [{"agent":"pi","verdict":"TIMEOUT"}],
            [{"agent":"pi","verdict":"OK"}]
        ]}));
        harness.set_control(json!({"c6b_flaky": {"exit_code": 0, "output": "ok"}}));
        let results = harness
            .scheduler()
            .submit(vec![make_job("c6b_flaky", "pi")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Ok);

        harness.set_health(json!({"__sequence__": [
            [{"agent":"pi","verdict":"TIMEOUT"}],
            [{"agent":"pi","verdict":"TIMEOUT"}]
        ]}));
        let results = harness
            .scheduler()
            .submit(vec![make_job("c6c_dead", "pi")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Skipped);

        let no_retry = harness.scheduler_with(SchedulerConfig {
            healthcheck_retries: 0,
            ..SchedulerConfig::default()
        });
        harness.set_health(json!({"__sequence__": [
            [{"agent":"pi","verdict":"TIMEOUT"}],
            [{"agent":"pi","verdict":"OK"}]
        ]}));
        let results = no_retry
            .submit(vec![make_job("c6d_noretry", "pi")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Skipped);
    }

    #[tokio::test]
    async fn adversarial_scheduler_cases_match_python_selftest() {
        let harness = Harness::new();
        let scheduler = harness.scheduler();
        harness.set_control(json!({
            "adv_429_ok": {"exit_code": 0, "output": "这个API遇到429时应退避重试"},
            "adv_fail": {"exit_code": 1, "output": ""},
            "adv_bad_path": {"exit_code": 0, "output": "unused"}
        }));
        let results = scheduler
            .submit(vec![harness.job("adv_429_ok")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Ok);

        let results = scheduler
            .submit(vec![harness.job("adv_fail")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Failed);
        assert_eq!(results[0].exit_code, Some(1));

        std_fs::remove_file(harness.runners_dir.join("gemini.sh")).unwrap();
        harness.set_health(json!([
            {"agent":"codex","verdict":"OK"},
            {"agent":"gemini","verdict":"OK"}
        ]));
        let results = scheduler
            .submit(vec![make_job("adv_no_runner", "gemini")])
            .await
            .unwrap();
        assert_eq!(results[0].status, JobStatus::Failed);
        assert!(results[0].error.contains("not found"));

        let mut bad_path = harness.job("adv_bad_path");
        bad_path.prompt_is_inline = false;
        bad_path.prompt_ref = "/nonexistent/prompt.txt".to_string();
        let results = scheduler.submit(vec![bad_path]).await.unwrap();
        assert_eq!(results[0].status, JobStatus::Failed);
        assert!(results[0].error.contains("not found"));
    }

    #[tokio::test]
    async fn ten_job_batch_has_no_lost_or_mixed_results_and_costs() {
        let harness = Harness::new();
        harness.set_control(json!(
            (0..10)
                .map(|idx| (
                    format!("adv10_{idx}"),
                    json!({"exit_code": 0, "output": format!("result_{idx}"), "sleep": 0.01})
                ))
                .collect::<serde_json::Map<_, _>>()
        ));
        let jobs = (0..10)
            .map(|idx| harness.job(&format!("adv10_{idx}")))
            .collect::<Vec<_>>();
        let results = harness.scheduler().submit(jobs).await.unwrap();
        for (idx, result) in results.iter().enumerate() {
            assert_eq!(result.job_id, format!("adv10_{idx}"));
            assert_eq!(result.status, JobStatus::Ok);
            assert!(result.reply_text.contains(&format!("result_{idx}")));
            assert!(result.cost["elapsed_sec"].as_f64().unwrap() >= 0.0);
        }
    }

    #[tokio::test]
    async fn backoff_caps_and_artifacts_contract_hold() {
        let harness = Harness::new();
        harness.set_control(
            json!({"reg3_cap": {"exit_code": 1, "output": "429 rate limit", "sleep": 0.01}}),
        );
        let capped = harness.scheduler_with(SchedulerConfig {
            max_concurrency: 1,
            max_backoff_sec: 0.3,
            total_retry_wall_sec: 0.8,
            ..SchedulerConfig::default()
        });
        let mut job = harness.job("reg3_cap");
        job.retry_policy = RetryPolicy {
            max_retries: 10,
            backoff_sec: 5.0,
            retry_on: vec![JobStatus::RateLimited],
        };
        let result = capped.submit(vec![job]).await.unwrap();
        assert!(result[0].attempts <= 4);

        harness.set_control(json!({"reg4_art": {"exit_code": 0, "output": "success text"}}));
        let result = harness
            .scheduler()
            .submit(vec![harness.job("reg4_art")])
            .await
            .unwrap();
        assert!(result[0].artifacts.is_empty());
        assert_eq!(result[0].reply_text, "success text");
    }

    #[tokio::test]
    async fn healthcheck_bad_json_shapes_fail_closed() {
        let harness = Harness::new();
        let scheduler = harness.scheduler();
        harness.set_health(json!({"agent":"codex","verdict":"OK"}));
        let result = scheduler
            .healthcheck(&["codex".to_string(), "pi".to_string()])
            .await;
        assert_eq!(
            result,
            BTreeMap::from([("codex".into(), false), ("pi".into(), false)])
        );

        std_fs::write(
            &harness.health,
            r#"[{"agent":"codex","verdict":"OK"},"bad",{"agent":"pi","verdict":"OK"}]"#,
        )
        .unwrap();
        let result = scheduler
            .healthcheck(&["codex".to_string(), "pi".to_string()])
            .await;
        assert_eq!(
            result,
            BTreeMap::from([("codex".into(), false), ("pi".into(), false)])
        );

        harness.set_health(json!([
            {"agent":"codex","verdict":"OK"},
            {"agent":"pi","verdict":"TIMEOUT"}
        ]));
        let result = scheduler
            .healthcheck(&["codex".to_string(), "pi".to_string()])
            .await;
        assert_eq!(
            result,
            BTreeMap::from([("codex".into(), true), ("pi".into(), false)])
        );
    }

    #[tokio::test]
    async fn healthcheck_checked_distinguishes_probe_failure_from_all_unhealthy() {
        let harness = Harness::new();
        let scheduler = harness.scheduler();
        harness.set_health(json!([
            {"agent":"codex","verdict":"TIMEOUT"},
            {"agent":"pi","verdict":"ERROR"}
        ]));
        let result = scheduler
            .healthcheck_checked(&["codex".to_string(), "pi".to_string()])
            .await
            .unwrap();
        assert_eq!(
            result,
            BTreeMap::from([("codex".into(), false), ("pi".into(), false)])
        );

        std_fs::remove_file(harness.runners_dir.join("healthcheck.sh")).unwrap();
        let err = scheduler
            .healthcheck_checked(&["codex".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, HealthcheckError::ScriptUnavailable(_)));
        let fail_closed = scheduler.healthcheck(&["codex".to_string()]).await;
        assert_eq!(fail_closed, BTreeMap::from([("codex".into(), false)]));
    }

    #[tokio::test]
    async fn healthcheck_lenient_api_preserves_initial_success_when_retry_probe_fails() {
        let harness = Harness::new();
        let scheduler = harness.scheduler();
        harness.set_health(json!({"__sequence__": [
            [{"agent":"codex","verdict":"OK"}, {"agent":"pi","verdict":"TIMEOUT"}],
            {"agent":"pi","verdict":"OK"}
        ]}));
        let result = scheduler
            .healthcheck(&["codex".to_string(), "pi".to_string()])
            .await;
        assert_eq!(
            result,
            BTreeMap::from([("codex".into(), true), ("pi".into(), false)])
        );
        let err = scheduler
            .healthcheck_checked(&["codex".to_string(), "pi".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, HealthcheckError::InvalidJson(_)));
    }

    #[tokio::test]
    async fn env_permission_snapshot_timeout_and_live_files_are_recorded() {
        let harness = Harness::new();
        harness.set_control(json!({
            "reg6_env": {
                "exit_code": 0,
                "output_env": ["CODEX_SANDBOX", "CODEX_MODEL", "CUSTOM_FLAG"]
            },
            "stall": {"exit_code": 0, "output": "late", "sleep": 2.0}
        }));
        let mut env_job = harness.job("reg6_env");
        env_job.model = Some("gpt-test".to_string());
        env_job
            .env
            .insert("CUSTOM_FLAG".to_string(), "yes".to_string());
        env_job.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            reason: "user approved implementation job".to_string(),
            user_approved: true,
            tools: Vec::new(),
        };
        let result = harness.scheduler().submit(vec![env_job]).await.unwrap();
        let env_seen: serde_json::Value = serde_json::from_str(&result[0].reply_text).unwrap();
        assert_eq!(env_seen["CODEX_SANDBOX"], "workspace-write");
        assert_eq!(env_seen["CODEX_MODEL"], "gpt-test");
        assert_eq!(env_seen["CUSTOM_FLAG"], "yes");
        assert_eq!(result[0].permissions["sandbox"], "workspace-write");
        assert_eq!(
            result[0].permissions["reason"],
            "user approved implementation job"
        );

        let mut stall_job = harness.job("stall");
        stall_job.budget.timeout_sec = 5;
        let scheduler = harness.scheduler_with(SchedulerConfig {
            stall_timeout_sec: Some(1),
            heartbeat_interval_sec: 1,
            ..SchedulerConfig::default()
        });
        let result = scheduler.submit(vec![stall_job]).await.unwrap();
        assert_eq!(result[0].status, JobStatus::Timeout);
        assert!(
            harness
                .repo
                .join(".lto/rust-scheduler/live/stall.log")
                .exists()
        );
        assert!(
            !harness
                .repo
                .join(".lto/rust-scheduler/live/stall.hb.jsonl")
                .exists(),
            "heartbeat sidecar should be removed when the job closes"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn escaped_stdout_holder_does_not_hang_scheduler_drain() {
        let harness = Harness::new();
        let pid_file = harness.repo.join("escaped-child.pid");
        harness.set_control(json!({
            "escape_stdout": {
                "exit_code": 0,
                "output": "done",
                "escape_stdout_holder": true,
                "pid_file": pid_file
            }
        }));
        let scheduler = harness.scheduler_with(SchedulerConfig {
            drain_after_exit_timeout_sec: 1,
            heartbeat_interval_sec: 1,
            ..SchedulerConfig::default()
        });

        let result = tokio::time::timeout(
            Duration::from_secs(15),
            scheduler.submit(vec![harness.job("escape_stdout")]),
        )
        .await
        .expect("scheduler must not hang waiting for escaped stdout holder")
        .unwrap();

        if let Ok(pid) = std_fs::read_to_string(&pid_file) {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(pid.trim())
                .status();
        }
        assert_eq!(result[0].status, JobStatus::Ok);
        assert!(
            result[0]
                .cost
                .get("stdout_drain_error")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value.contains("timed out")),
            "expected stdout_drain_error, got {:?}",
            result[0].cost
        );
        assert!(
            !harness
                .repo
                .join(".lto/rust-scheduler/live/escape_stdout.hb.jsonl")
                .exists()
        );
    }

    #[tokio::test]
    async fn write_task_emits_merge_review_and_keeps_worktree_without_merging() {
        let harness = Harness::new();
        init_git_repo(&harness.repo);
        harness.set_control(json!({
            "write_ok": {
                "exit_code": 0,
                "output": "implemented",
                "write_file": "feature.txt",
                "write_content": "new feature\n"
            }
        }));
        let mut job = write_job(&harness, "write_ok");
        job.needs_worktree = false;
        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        let result = &result[0];
        assert_eq!(result.status, JobStatus::Ok);
        assert_eq!(result.size, TaskSize::Small);
        assert_eq!(result.cost["test_gate_action"], "batchable");
        let review = result.merge_review.as_ref().expect("merge review");
        assert!(review.diff.diff.contains("feature.txt"));
        assert!(matches!(
            review.diff.test_result.status,
            merge_review::TestStatus::Passed
        ));
        let wt = worktree_path(result);
        assert_eq!(
            std_fs::read_to_string(wt.join("feature.txt")).unwrap(),
            "new feature\n"
        );
        assert!(!harness.repo.join("feature.txt").exists());
    }

    #[tokio::test]
    async fn write_task_with_no_artifact_fails_closed() {
        let harness = Harness::new();
        init_git_repo(&harness.repo);
        harness.set_control(json!({
            "noop_write": {"exit_code": 0, "output": "done without write"}
        }));
        let mut job = write_job(&harness, "noop_write");
        job.permission_policy = PermissionPolicy::default();

        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        assert_eq!(result[0].status, JobStatus::Failed);
        assert!(result[0].error.contains("no worktree changes"));
        assert!(result[0].merge_review.is_none());
    }

    #[tokio::test]
    async fn write_task_spawn_failure_prunes_persistent_worktree() {
        let harness = Harness::new();
        init_git_repo(&harness.repo);
        let runner_path = harness.runners_dir.join("codex.sh");
        std_fs::remove_file(&runner_path).unwrap();
        std_fs::create_dir(&runner_path).unwrap();
        let job = write_job(&harness, "spawn_fail_write");
        let leaked_path = harness
            .repo
            .join(".lto/worktrees/rust-scheduler/spawn_fail_write");

        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        assert_eq!(result[0].status, JobStatus::Failed);
        assert!(result[0].error.contains("spawn:"));
        assert!(
            !leaked_path.exists(),
            "spawn failure must prune scheduler-created persistent worktree at {}",
            leaked_path.display()
        );
    }

    #[tokio::test]
    async fn tmux_runner_fails_closed_for_scheduler_managed_worktree() {
        let harness = Harness::new();
        harness.set_health(json!([{"agent":"tmux","verdict":"OK"}]));
        let mut job = make_job("tmux-write", "tmux");
        job.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            reason: "tmux cannot enforce read-only".to_string(),
            user_approved: false,
            tools: Vec::new(),
        };
        job.needs_worktree = true;

        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        assert_eq!(result[0].status, JobStatus::Failed);
        assert!(result[0].error.contains("worktree isolation"));
    }

    #[tokio::test]
    async fn unhealthy_tmux_runner_is_skipped_before_dispatch() {
        let harness = Harness::new();
        harness.set_health(json!([{"agent":"tmux","verdict":"MISSING"}]));
        let mut job = make_job("tmux-missing", "tmux");
        job.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            reason: "tmux cannot enforce read-only".to_string(),
            user_approved: false,
            tools: Vec::new(),
        };
        job.meta
            .insert("tmux_target".to_string(), serde_json::json!("missing:1.0"));

        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        assert_eq!(result[0].status, JobStatus::Skipped);
        assert!(result[0].error.contains("runner unhealthy"));
    }

    #[tokio::test]
    async fn failing_test_gate_returns_merge_review_for_immediate_report() {
        let harness = Harness::new();
        init_git_repo(&harness.repo);
        harness.set_control(json!({
            "write_bad_tests": {
                "exit_code": 0,
                "output": "implemented",
                "write_file": "broken.txt",
                "write_content": "broken\n"
            }
        }));
        let mut job = write_job(&harness, "write_bad_tests");
        job.test_cmd = Some("false".to_string());

        let result = harness.scheduler().submit(vec![job]).await.unwrap();

        assert_eq!(result[0].status, JobStatus::Failed);
        assert_eq!(result[0].cost["test_gate_action"], "immediate_report");
        assert!(result[0].merge_review.is_some());
        assert!(worktree_path(&result[0]).join("broken.txt").exists());
    }

    #[tokio::test]
    async fn dependency_child_waits_for_host_merge_but_independent_job_runs() {
        let harness = Harness::new();
        init_git_repo(&harness.repo);
        harness.set_control(json!({
            "dep_parent": {
                "exit_code": 0,
                "output": "parent done",
                "write_file": "parent.txt",
                "write_content": "parent\n"
            },
            "dep_child": {"exit_code": 0, "output": "child should not run"},
            "dep_independent": {"exit_code": 0, "output": "independent done"}
        }));
        let mut parent = write_job(&harness, "dep_parent");
        parent.children = vec!["dep_child".to_string()];
        let child = write_job(&harness, "dep_child");
        let independent = harness.job("dep_independent");

        let results = harness
            .scheduler()
            .submit(vec![parent, child, independent])
            .await
            .unwrap();

        assert_eq!(results[0].status, JobStatus::Ok);
        assert!(results[0].merge_review.is_some());
        assert_eq!(results[1].status, JobStatus::Skipped);
        assert!(results[1].error.contains("host must merge"));
        assert_eq!(results[2].status, JobStatus::Ok);
        assert_eq!(results[2].reply_text, "independent done");
    }

    #[test]
    fn sandbox_escalation_guards_match_python_selftest_reg7() {
        let mut write = make_job("reg7_bad_write", "codex");
        write.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            ..PermissionPolicy::default()
        };
        assert!(matches!(
            write.validate(),
            Err(AgentJobError::MissingWorkspaceWriteReason)
        ));

        let mut danger = make_job("reg7_danger", "codex");
        danger.permission_policy = PermissionPolicy {
            sandbox: Sandbox::DangerFullAccess,
            reason: "test only".to_string(),
            user_approved: false,
            tools: Vec::new(),
        };
        assert!(matches!(
            danger.validate(),
            Err(AgentJobError::MissingDangerApproval)
        ));

        let mut conflict = make_job("reg7_conflict", "codex");
        conflict
            .env
            .insert("CODEX_SANDBOX".to_string(), "read-only".to_string());
        conflict.permission_policy = PermissionPolicy {
            sandbox: Sandbox::WorkspaceWrite,
            reason: "conflicting env test".to_string(),
            user_approved: true,
            tools: Vec::new(),
        };
        assert!(matches!(
            conflict.validate(),
            Err(AgentJobError::CodexSandboxConflict { .. })
        ));
    }
}
