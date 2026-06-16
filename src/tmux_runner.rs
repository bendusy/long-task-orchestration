use crate::agent_job::{AgentJob, JobStatus};
use crate::llm_judge::redact_text;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

const DEFAULT_READY_TIMEOUT_SEC: u64 = 30;
const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const DEFAULT_CAPTURE_LINES: usize = 200;
const SEND_ENTER_DELAY_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxMode {
    Signal,
    Sentinel,
    Fire,
}

impl TmuxMode {
    fn parse(value: Option<&str>) -> Result<Self, TmuxRunnerError> {
        match value.unwrap_or("signal") {
            "signal" => Ok(Self::Signal),
            "sentinel" => Ok(Self::Sentinel),
            "fire" => Ok(Self::Fire),
            other => Err(TmuxRunnerError::Config(format!(
                "invalid tmux mode: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::Sentinel => "sentinel",
            Self::Fire => "fire",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipPrompt {
    pub pattern: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxRunnerConfig {
    pub tmux_bin: String,
    pub mode: TmuxMode,
    pub target: Option<String>,
    pub session: Option<String>,
    pub new_window: bool,
    pub new_session: bool,
    pub window_name: String,
    pub signal_name: String,
    pub sentinel_path: Option<PathBuf>,
    pub ready_patterns: Vec<String>,
    pub skip_prompts: Vec<SkipPrompt>,
    pub ready_timeout: Duration,
    pub poll_interval: Duration,
    pub capture_lines: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TmuxJobOutcome {
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub reply_text: String,
    pub error: String,
    pub cost: BTreeMap<String, Value>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TmuxRunnerError {
    #[error("{0}")]
    Config(String),
    #[error("tmux binary not found or failed to start ({bin}): {reason}")]
    Start { bin: String, reason: String },
    #[error("tmux command failed: tmux {args} (status={status}) {stderr}")]
    CommandFailed {
        args: String,
        status: String,
        stderr: String,
    },
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DispatchSummary {
    target: String,
    mode: TmuxMode,
    capture: String,
    signal_name: Option<String>,
    sentinel_path: Option<PathBuf>,
}

impl TmuxRunnerConfig {
    pub fn from_job(job: &AgentJob, repo: &Path) -> Result<Self, TmuxRunnerError> {
        let meta = &job.meta;
        let mode = TmuxMode::parse(meta_str(meta, &["tmux_mode"]).as_deref())?;
        let run_id = meta_str(meta, &["run_id"]).unwrap_or_else(|| "rust-scheduler".to_string());
        let signal_name = parse_signal_name(
            meta_str(meta, &["tmux_signal", "signal"]),
            &run_id,
            &job.job_id,
        )?;
        let sentinel_path = match meta_str(meta, &["tmux_sentinel", "sentinel"]) {
            Some(path) => Some(resolve_sentinel_path(repo, &path)?),
            None => None,
        };
        let sentinel_path = if mode == TmuxMode::Sentinel && sentinel_path.is_none() {
            Some(
                repo.join(".lto")
                    .join(&run_id)
                    .join("live")
                    .join(format!("{}.sentinel", job.job_id)),
            )
        } else {
            sentinel_path
        };
        let target = meta_str(meta, &["tmux_target", "target"]);
        let session = meta_str(meta, &["tmux_session", "session"]);
        let new_session = meta_bool(meta, &["tmux_new_session", "new_session"]).unwrap_or(false);
        let new_window = meta_bool(meta, &["tmux_new_window", "new_window"])
            .unwrap_or_else(|| target.is_none() && session.is_some() && !new_session);
        Ok(Self {
            tmux_bin: meta_str(meta, &["tmux_bin"]).unwrap_or_else(|| "tmux".to_string()),
            mode,
            target,
            session,
            new_window,
            new_session,
            window_name: meta_str(meta, &["tmux_window_name", "window_name"])
                .unwrap_or_else(|| format!("lto-{}", job.job_id)),
            signal_name,
            sentinel_path,
            ready_patterns: meta_string_list(meta, &["tmux_ready_patterns", "ready_patterns"]),
            skip_prompts: skip_prompts(meta),
            ready_timeout: Duration::from_secs(
                meta_u64(meta, &["tmux_ready_timeout_sec", "ready_timeout_sec"])
                    .unwrap_or(DEFAULT_READY_TIMEOUT_SEC),
            ),
            poll_interval: Duration::from_millis(
                meta_u64(meta, &["tmux_poll_interval_ms", "poll_interval_ms"])
                    .unwrap_or(DEFAULT_POLL_INTERVAL_MS),
            ),
            capture_lines: meta_u64(meta, &["tmux_capture_lines", "capture_lines"])
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(DEFAULT_CAPTURE_LINES),
        })
    }
}

pub async fn run_job(
    job: &AgentJob,
    prompt_path: &Path,
    repo: &Path,
) -> Result<TmuxJobOutcome, TmuxRunnerError> {
    let prompt = fs::read_to_string(prompt_path)
        .await
        .map_err(|err| TmuxRunnerError::Io(format!("read prompt: {err}")))?;
    let config = TmuxRunnerConfig::from_job(job, repo)?;
    let live_log_path = tmux_live_log_path(job, repo);
    let started = Instant::now();
    let dispatch = dispatch(
        &config,
        &prompt,
        Duration::from_secs(job.budget.timeout_sec),
        Some(live_log_path.as_path()),
    )
    .await;
    let elapsed_sec = started.elapsed().as_secs_f64();
    match dispatch {
        Ok(summary) => {
            let mut cost = BTreeMap::from([
                ("elapsed_sec".to_string(), serde_json::json!(elapsed_sec)),
                ("tmux_target".to_string(), serde_json::json!(summary.target)),
                (
                    "tmux_mode".to_string(),
                    serde_json::json!(summary.mode.as_str()),
                ),
                (
                    "capture_bytes".to_string(),
                    serde_json::json!(summary.capture.len()),
                ),
            ]);
            if let Some(signal) = summary.signal_name {
                cost.insert("tmux_signal".to_string(), serde_json::json!(signal));
            }
            if summary.mode == TmuxMode::Fire {
                cost.insert("tmux_fire_and_forget".to_string(), serde_json::json!(true));
            }
            let mut artifacts = Vec::new();
            if let Some(path) = summary.sentinel_path {
                artifacts.push(format!("sentinel:{}", path.display()));
                cost.insert(
                    "tmux_sentinel".to_string(),
                    serde_json::json!(path.display().to_string()),
                );
            }
            Ok(TmuxJobOutcome {
                status: JobStatus::Ok,
                exit_code: if summary.mode == TmuxMode::Fire {
                    None
                } else {
                    Some(0)
                },
                reply_text: summary.capture,
                error: String::new(),
                cost,
                artifacts,
            })
        }
        Err(TmuxRunnerError::Timeout(err)) => Ok(TmuxJobOutcome {
            status: JobStatus::Timeout,
            exit_code: Some(124),
            reply_text: String::new(),
            error: err,
            cost: BTreeMap::from([("elapsed_sec".to_string(), serde_json::json!(elapsed_sec))]),
            artifacts: Vec::new(),
        }),
        Err(err) => Err(err),
    }
}

async fn dispatch(
    config: &TmuxRunnerConfig,
    prompt: &str,
    completion_timeout: Duration,
    live_log_path: Option<&Path>,
) -> Result<DispatchSummary, TmuxRunnerError> {
    let target = prepare_target(config).await?;
    wait_until_ready(config, &target).await?;
    if let Ok(capture) = capture_pane(&config.tmux_bin, &target, config.capture_lines).await {
        let _ = append_live_snapshot(live_log_path, "ready", &target, &capture).await;
    }
    match config.mode {
        TmuxMode::Signal => {
            run_signal(config, &target, prompt, completion_timeout, live_log_path).await
        }
        TmuxMode::Sentinel => {
            run_sentinel(config, &target, prompt, completion_timeout, live_log_path).await
        }
        TmuxMode::Fire => run_fire(config, &target, prompt, live_log_path).await,
    }
}

pub async fn capture_pane(
    tmux_bin: &str,
    target: &str,
    lines: usize,
) -> Result<String, TmuxRunnerError> {
    tmux_output(
        tmux_bin,
        &[
            "capture-pane".to_string(),
            "-p".to_string(),
            "-J".to_string(),
            "-S".to_string(),
            format!("-{lines}"),
            "-t".to_string(),
            target.to_string(),
        ],
    )
    .await
}

async fn prepare_target(config: &TmuxRunnerConfig) -> Result<String, TmuxRunnerError> {
    if config.new_session {
        let session = config.session.as_ref().ok_or_else(|| {
            TmuxRunnerError::Config("tmux_new_session requires tmux_session".to_string())
        })?;
        let args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            session.clone(),
            "-n".to_string(),
            config.window_name.clone(),
            "-P".to_string(),
            "-F".to_string(),
            "#{session_name}:#{window_index}.#{pane_index}".to_string(),
        ];
        return match tmux_output(&config.tmux_bin, &args).await {
            Ok(target) => Ok(target.trim().to_string()),
            Err(TmuxRunnerError::CommandFailed { stderr, .. })
                if contains_case_insensitive(&stderr, "duplicate session") =>
            {
                new_window_in_session(config, Some(session.clone())).await
            }
            Err(err) => Err(err),
        };
    }

    if config.new_window {
        let target = match config.target.as_ref().or(config.session.as_ref()) {
            Some(target) => Some(target.clone()),
            None => current_session(&config.tmux_bin).await?,
        };
        return new_window_in_session(config, target).await;
    }

    if let Some(target) = &config.target {
        return Ok(target.clone());
    }

    if let Some(session) = current_session(&config.tmux_bin).await? {
        return new_window_in_session(config, Some(session)).await;
    }

    Err(TmuxRunnerError::Config(
        "tmux target required; pass tmux_target, tmux_session+tmux_new_window, or tmux_session+tmux_new_session".to_string(),
    ))
}

async fn new_window_in_session(
    config: &TmuxRunnerConfig,
    target: Option<String>,
) -> Result<String, TmuxRunnerError> {
    let mut args = vec![
        "new-window".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{session_name}:#{window_index}.#{pane_index}".to_string(),
        "-n".to_string(),
        config.window_name.clone(),
    ];
    if let Some(target) = target {
        args.push("-t".to_string());
        args.push(target);
    }
    tmux_output(&config.tmux_bin, &args)
        .await
        .map(|target| target.trim().to_string())
}

async fn current_session(tmux_bin: &str) -> Result<Option<String>, TmuxRunnerError> {
    let pane = match std::env::var("TMUX_PANE") {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    };
    let Some(pane) = pane else {
        return current_session_from_client(tmux_bin).await;
    };
    let session = match tmux_output(
        tmux_bin,
        &[
            "display-message".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            pane,
            "#{session_name}".to_string(),
        ],
    )
    .await
    {
        Ok(session) => session,
        Err(_) => return current_session_from_client(tmux_bin).await,
    };
    let session = session.trim();
    if session.is_empty() {
        Ok(None)
    } else {
        Ok(Some(session.to_string()))
    }
}

async fn current_session_from_client(tmux_bin: &str) -> Result<Option<String>, TmuxRunnerError> {
    match std::env::var("TMUX") {
        Ok(value) if !value.trim().is_empty() => {}
        _ => return Ok(None),
    }
    let session = match tmux_output(
        tmux_bin,
        &[
            "display-message".to_string(),
            "-p".to_string(),
            "#{session_name}".to_string(),
        ],
    )
    .await
    {
        Ok(session) => session,
        Err(_) => return Ok(None),
    };
    let session = session.trim();
    if session.is_empty() {
        Ok(None)
    } else {
        Ok(Some(session.to_string()))
    }
}

async fn wait_until_ready(config: &TmuxRunnerConfig, target: &str) -> Result<(), TmuxRunnerError> {
    if config.ready_patterns.is_empty() {
        return wait_for_stable_capture(config, target).await;
    }

    let deadline = Instant::now() + config.ready_timeout;
    let mut skipped = BTreeSet::new();
    loop {
        let capture = capture_pane(&config.tmux_bin, target, config.capture_lines).await?;
        apply_skip_prompts(config, target, &capture, &mut skipped).await?;
        if config
            .ready_patterns
            .iter()
            .any(|pattern| contains_case_insensitive(&capture, pattern))
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(TmuxRunnerError::Timeout(format!(
                "tmux target {target} did not become ready within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            )));
        }
        sleep(config.poll_interval).await;
    }
}

async fn wait_for_stable_capture(
    config: &TmuxRunnerConfig,
    target: &str,
) -> Result<(), TmuxRunnerError> {
    let deadline = Instant::now() + config.ready_timeout;
    let mut previous = None;
    let mut skipped = BTreeSet::new();
    loop {
        let capture = capture_pane(&config.tmux_bin, target, config.capture_lines).await?;
        apply_skip_prompts(config, target, &capture, &mut skipped).await?;
        let normalized = capture.trim().to_string();
        if !normalized.is_empty() && previous.as_ref() == Some(&normalized) {
            return Ok(());
        }
        previous = Some(normalized);
        if Instant::now() >= deadline {
            return Err(TmuxRunnerError::Timeout(format!(
                "tmux target {target} did not produce stable ready output within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            )));
        }
        sleep(config.poll_interval).await;
    }
}

async fn apply_skip_prompts(
    config: &TmuxRunnerConfig,
    target: &str,
    capture: &str,
    skipped: &mut BTreeSet<String>,
) -> Result<(), TmuxRunnerError> {
    for skip in &config.skip_prompts {
        if contains_case_insensitive(capture, &skip.pattern) && skipped.insert(skip.pattern.clone())
        {
            tmux_status(
                &config.tmux_bin,
                &[
                    "send-keys".to_string(),
                    "-t".to_string(),
                    target.to_string(),
                    skip.key.clone(),
                ],
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_signal(
    config: &TmuxRunnerConfig,
    target: &str,
    prompt: &str,
    completion_timeout: Duration,
    live_log_path: Option<&Path>,
) -> Result<DispatchSummary, TmuxRunnerError> {
    let body = append_line(prompt, &format!("tmux wait-for -S {}", config.signal_name));
    send_text(config, target, &body).await?;
    let wait_args = vec!["wait-for".to_string(), config.signal_name.clone()];
    let wait = tmux_status(&config.tmux_bin, &wait_args);
    tokio::pin!(wait);
    let deadline = Instant::now() + completion_timeout;
    let snapshot_every = snapshot_interval(config);
    let mut skipped = BTreeSet::new();
    loop {
        if Instant::now() >= deadline {
            let capture = capture_pane(&config.tmux_bin, target, config.capture_lines)
                .await
                .unwrap_or_default();
            let _ = append_live_snapshot(live_log_path, "timeout", target, &capture).await;
            return Err(TmuxRunnerError::Timeout(format!(
                "tmux signal mode timed out waiting for {}; last capture: {}",
                config.signal_name,
                one_line_tail(&capture, 500)
            )));
        }
        tokio::select! {
            result = &mut wait => {
                result?;
                let capture = capture_pane(&config.tmux_bin, target, config.capture_lines).await?;
                let _ = append_live_snapshot(live_log_path, "finished", target, &capture).await;
                return Ok(DispatchSummary {
                    target: target.to_string(),
                    mode: TmuxMode::Signal,
                    capture,
                    signal_name: Some(config.signal_name.clone()),
                    sentinel_path: None,
                });
            }
            _ = sleep(snapshot_every) => {
                let capture = capture_pane(&config.tmux_bin, target, config.capture_lines).await?;
                apply_skip_prompts(config, target, &capture, &mut skipped).await?;
                let _ = append_live_snapshot(live_log_path, "running", target, &capture).await;
            }
        }
    }
}

async fn run_sentinel(
    config: &TmuxRunnerConfig,
    target: &str,
    prompt: &str,
    completion_timeout: Duration,
    live_log_path: Option<&Path>,
) -> Result<DispatchSummary, TmuxRunnerError> {
    let sentinel = config.sentinel_path.as_ref().ok_or_else(|| {
        TmuxRunnerError::Config("tmux sentinel mode requires sentinel_path".to_string())
    })?;
    if let Some(parent) = sentinel.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| TmuxRunnerError::Io(format!("create sentinel dir: {err}")))?;
    }
    let _ = fs::remove_file(sentinel).await;
    let body = append_line(
        prompt,
        &format!(
            "# LTO sentinel marker: interactive agents should run the next command only after finishing the requested work.\nprintf '%s\\n' done > {}",
            shell_single_quote(&sentinel.display().to_string())
        ),
    );
    send_text(config, target, &body).await?;

    let deadline = Instant::now() + completion_timeout;
    let snapshot_every = snapshot_interval(config);
    let mut next_snapshot = Instant::now();
    let mut skipped = BTreeSet::new();
    loop {
        if fs::metadata(sentinel).await.is_ok() {
            let sentinel_text = fs::read_to_string(sentinel).await.unwrap_or_default();
            let capture = capture_pane(&config.tmux_bin, target, config.capture_lines)
                .await
                .unwrap_or_default();
            let _ = append_live_snapshot(live_log_path, "finished", target, &capture).await;
            return Ok(DispatchSummary {
                target: target.to_string(),
                mode: TmuxMode::Sentinel,
                capture: if sentinel_text.trim().is_empty() {
                    capture
                } else {
                    sentinel_text
                },
                signal_name: None,
                sentinel_path: Some(sentinel.clone()),
            });
        }
        if Instant::now() >= deadline {
            let capture = capture_pane(&config.tmux_bin, target, config.capture_lines)
                .await
                .unwrap_or_default();
            let _ = append_live_snapshot(live_log_path, "timeout", target, &capture).await;
            return Err(TmuxRunnerError::Timeout(format!(
                "tmux sentinel mode timed out waiting for {}; last capture: {}",
                sentinel.display(),
                one_line_tail(&capture, 500)
            )));
        }
        if Instant::now() >= next_snapshot {
            let capture = capture_pane(&config.tmux_bin, target, config.capture_lines).await?;
            apply_skip_prompts(config, target, &capture, &mut skipped).await?;
            let _ = append_live_snapshot(live_log_path, "running", target, &capture).await;
            next_snapshot = Instant::now() + snapshot_every;
        }
        sleep(config.poll_interval).await;
    }
}

async fn run_fire(
    config: &TmuxRunnerConfig,
    target: &str,
    prompt: &str,
    live_log_path: Option<&Path>,
) -> Result<DispatchSummary, TmuxRunnerError> {
    send_text(config, target, prompt).await?;
    let capture = capture_pane(&config.tmux_bin, target, config.capture_lines)
        .await
        .unwrap_or_default();
    let _ = append_live_snapshot(live_log_path, "sent", target, &capture).await;
    Ok(DispatchSummary {
        target: target.to_string(),
        mode: TmuxMode::Fire,
        capture: String::new(),
        signal_name: None,
        sentinel_path: None,
    })
}

async fn append_live_snapshot(
    path: Option<&Path>,
    label: &str,
    target: &str,
    capture: &str,
) -> Result<(), TmuxRunnerError> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| TmuxRunnerError::Io(format!("create live log dir: {err}")))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|err| TmuxRunnerError::Io(format!("open live log: {err}")))?;
    file.write_all(
        format!(
            "\n--- tmux {label} target={target} at={} ---\n",
            now_millis()
        )
        .as_bytes(),
    )
    .await
    .map_err(|err| TmuxRunnerError::Io(format!("write live log header: {err}")))?;
    let redacted = redact_text(capture);
    file.write_all(redacted.as_bytes())
        .await
        .map_err(|err| TmuxRunnerError::Io(format!("write live log capture: {err}")))?;
    file.write_all(b"\n")
        .await
        .map_err(|err| TmuxRunnerError::Io(format!("write live log newline: {err}")))?;
    Ok(())
}

async fn send_text(
    config: &TmuxRunnerConfig,
    target: &str,
    text: &str,
) -> Result<(), TmuxRunnerError> {
    let _ = tmux_status_allow_failure(
        &config.tmux_bin,
        &[
            "send-keys".to_string(),
            "-t".to_string(),
            target.to_string(),
            "-X".to_string(),
            "cancel".to_string(),
        ],
    )
    .await;
    tmux_status(
        &config.tmux_bin,
        &[
            "send-keys".to_string(),
            "-t".to_string(),
            target.to_string(),
            "C-u".to_string(),
        ],
    )
    .await?;
    let buffer_name = sanitize_signal(&format!("lto-paste-{target}-{}", now_millis()));
    tmux_status_with_stdin(
        &config.tmux_bin,
        &[
            "load-buffer".to_string(),
            "-b".to_string(),
            buffer_name.clone(),
            "-".to_string(),
        ],
        text,
    )
    .await?;
    tmux_status(
        &config.tmux_bin,
        &[
            "paste-buffer".to_string(),
            "-d".to_string(),
            "-b".to_string(),
            buffer_name,
            "-t".to_string(),
            target.to_string(),
        ],
    )
    .await?;
    sleep(Duration::from_millis(SEND_ENTER_DELAY_MS)).await;
    tmux_status(
        &config.tmux_bin,
        &[
            "send-keys".to_string(),
            "-t".to_string(),
            target.to_string(),
            "Enter".to_string(),
        ],
    )
    .await
}

async fn tmux_status(tmux_bin: &str, args: &[String]) -> Result<(), TmuxRunnerError> {
    let output = run_tmux(tmux_bin, args).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(args, &output))
    }
}

async fn tmux_status_allow_failure(tmux_bin: &str, args: &[String]) -> Result<(), TmuxRunnerError> {
    let _ = run_tmux(tmux_bin, args).await?;
    Ok(())
}

async fn tmux_status_with_stdin(
    tmux_bin: &str,
    args: &[String],
    input: &str,
) -> Result<(), TmuxRunnerError> {
    let output = run_tmux_with_stdin(tmux_bin, args, input).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(args, &output))
    }
}

async fn tmux_output(tmux_bin: &str, args: &[String]) -> Result<String, TmuxRunnerError> {
    let output = run_tmux(tmux_bin, args).await?;
    if !output.status.success() {
        return Err(command_failed(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_tmux(
    tmux_bin: &str,
    args: &[String],
) -> Result<std::process::Output, TmuxRunnerError> {
    Command::new(tmux_bin)
        .args(args)
        .output()
        .await
        .map_err(|err| TmuxRunnerError::Start {
            bin: tmux_bin.to_string(),
            reason: err.to_string(),
        })
}

async fn run_tmux_with_stdin(
    tmux_bin: &str,
    args: &[String],
    input: &str,
) -> Result<std::process::Output, TmuxRunnerError> {
    let mut child = Command::new(tmux_bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| TmuxRunnerError::Start {
            bin: tmux_bin.to_string(),
            reason: err.to_string(),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|err| TmuxRunnerError::Io(format!("write tmux stdin: {err}")))?;
    }
    child
        .wait_with_output()
        .await
        .map_err(|err| TmuxRunnerError::Start {
            bin: tmux_bin.to_string(),
            reason: err.to_string(),
        })
}

fn command_failed(args: &[String], output: &std::process::Output) -> TmuxRunnerError {
    TmuxRunnerError::CommandFailed {
        args: args.join(" "),
        status: output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        stderr: one_line_tail(&String::from_utf8_lossy(&output.stderr), 500),
    }
}

fn append_line(text: &str, line: &str) -> String {
    let mut out = text.trim_end_matches('\n').to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
    out
}

fn tmux_live_log_path(job: &AgentJob, repo: &Path) -> PathBuf {
    let run_id = meta_str(&job.meta, &["run_id"]).unwrap_or_else(|| "rust-scheduler".to_string());
    repo.join(".lto")
        .join(run_id)
        .join("live")
        .join(format!("{}.log", job.job_id))
}

fn snapshot_interval(config: &TmuxRunnerConfig) -> Duration {
    let floor = Duration::from_secs(5);
    if config.poll_interval > floor {
        config.poll_interval
    } else {
        floor
    }
}

fn skip_prompts(meta: &BTreeMap<String, Value>) -> Vec<SkipPrompt> {
    let mut prompts = vec![
        SkipPrompt {
            pattern: "update available".to_string(),
            key: "n".to_string(),
        },
        SkipPrompt {
            pattern: "upgrade?".to_string(),
            key: "n".to_string(),
        },
        SkipPrompt {
            pattern: "new version".to_string(),
            key: "n".to_string(),
        },
    ];
    for item in meta_string_list(meta, &["tmux_skip_prompts", "skip_prompts"]) {
        if let Some((pattern, key)) = item.split_once('=') {
            prompts.push(SkipPrompt {
                pattern: pattern.trim().to_string(),
                key: key.trim().to_string(),
            });
        }
    }
    if let Some(Value::Array(items)) = meta.get("tmux_skip_prompt_map") {
        for item in items {
            let Some(pattern) = item.get("pattern").and_then(Value::as_str) else {
                continue;
            };
            let Some(key) = item.get("key").and_then(Value::as_str) else {
                continue;
            };
            prompts.push(SkipPrompt {
                pattern: pattern.to_string(),
                key: key.to_string(),
            });
        }
    }
    prompts
}

fn meta_str(meta: &BTreeMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| meta.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn meta_bool(meta: &BTreeMap<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| meta.get(*key).and_then(Value::as_bool))
}

fn meta_u64(meta: &BTreeMap<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| meta.get(*key).and_then(Value::as_u64))
}

fn meta_string_list(meta: &BTreeMap<String, Value>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = meta.get(*key) else {
            continue;
        };
        match value {
            Value::String(item) => return vec![item.clone()],
            Value::Array(items) => {
                return items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

fn parse_signal_name(
    configured: Option<String>,
    run_id: &str,
    job_id: &str,
) -> Result<String, TmuxRunnerError> {
    if let Some(value) = configured {
        let sanitized = sanitize_signal(&value);
        if sanitized.is_empty() || sanitized != value {
            return Err(TmuxRunnerError::Config(format!(
                "invalid tmux signal name: {value:?}; use only ASCII letters, digits, '.', '_' or '-'"
            )));
        }
        return Ok(sanitize_signal(&format!(
            "{value}-{run_id}-{job_id}-{}-done",
            now_millis()
        )));
    }
    Ok(sanitize_signal(&format!(
        "lto-{run_id}-{job_id}-{}-done",
        now_millis()
    )))
}

fn resolve_sentinel_path(repo: &Path, value: &str) -> Result<PathBuf, TmuxRunnerError> {
    let repo = lexical_normalize(&absolutize(repo)?);
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    let path = lexical_normalize(&path);
    if !path.starts_with(&repo) {
        return Err(TmuxRunnerError::Config(format!(
            "tmux sentinel path must stay inside repo: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn absolutize(path: &Path) -> Result<PathBuf, TmuxRunnerError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|err| TmuxRunnerError::Config(format!("cannot resolve current dir: {err}")))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn sanitize_signal(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn one_line_tail(text: &str, limit: usize) -> String {
    let text = text.replace(['\n', '\r'], " ");
    let mut tail = text.chars().rev().take(limit).collect::<Vec<_>>();
    tail.reverse();
    tail.into_iter().collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::{Budget, PermissionPolicy, RetryPolicy, Sandbox, TaskSize};
    use std::fs as std_fs;

    fn job_with_meta(meta: BTreeMap<String, Value>) -> AgentJob {
        AgentJob {
            job_id: "job-1".to_string(),
            prompt_ref: "prompt".to_string(),
            runner: "tmux".to_string(),
            prompt_is_inline: true,
            model: None,
            env: BTreeMap::new(),
            permission_policy: PermissionPolicy {
                sandbox: Sandbox::ReadOnly,
                ..PermissionPolicy::default()
            },
            isolation: "none".to_string(),
            output_schema: None,
            parent_pattern: crate::agent_job::Pattern::Linear,
            budget: Budget {
                timeout_sec: 1,
                max_tokens: None,
            },
            retry_policy: RetryPolicy {
                max_retries: 0,
                ..RetryPolicy::default()
            },
            verifier_of: None,
            children: Vec::new(),
            task_type: Some("runner".to_string()),
            size: TaskSize::Small,
            test_cmd: None,
            needs_worktree: false,
            meta,
        }
    }

    fn fake_tmux(tmp: &Path, capture: &str, sentinel: Option<&Path>) -> PathBuf {
        let bin = tmp.join("tmux");
        let log = tmp.join("tmux-log.jsonl");
        let capture_path = tmp.join("capture.txt");
        std_fs::write(&capture_path, capture).unwrap();
        let sentinel_code = sentinel
            .map(|path| {
                format!(
                    "sentinel = r'''{}'''\n",
                    path.display().to_string().replace('\\', "\\\\")
                )
            })
            .unwrap_or_else(|| "sentinel = ''\n".to_string());
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
log = pathlib.Path(r'''{log}''')
capture = pathlib.Path(r'''{capture}''')
{sentinel_code}
args = sys.argv[1:]
stdin_data = sys.stdin.read() if args and args[0] == "load-buffer" else None
with log.open("a") as f:
    f.write(json.dumps(args + (["stdin", stdin_data] if stdin_data is not None else [])) + "\n")
if args and args[0] == "capture-pane":
    print(capture.read_text())
    sys.exit(0)
if args and args[0] in ("new-window", "new-session"):
    print("sess:9.0")
    sys.exit(0)
if args and args[0] == "display-message":
    print("sess")
    sys.exit(0)
if args and args[0] == "paste-buffer" and sentinel:
    pathlib.Path(sentinel).write_text("done")
sys.exit(0)
"#,
            log = log.display(),
            capture = capture_path.display(),
            sentinel_code = sentinel_code,
        );
        std_fs::write(&bin, script).unwrap();
        make_executable(&bin);
        bin
    }

    fn fake_tmux_duplicate_session(tmp: &Path) -> PathBuf {
        let bin = tmp.join("tmux");
        let log = tmp.join("tmux-log.jsonl");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
log = pathlib.Path(r'''{log}''')
args = sys.argv[1:]
with log.open("a") as f:
    f.write(json.dumps(args) + "\n")
if args and args[0] == "new-session":
    print("duplicate session: sess", file=sys.stderr)
    sys.exit(1)
if args and args[0] == "new-window":
    print("sess:2.0")
    sys.exit(0)
if args and args[0] == "capture-pane":
    print("ready")
    sys.exit(0)
sys.exit(0)
"#,
            log = log.display(),
        );
        std_fs::write(&bin, script).unwrap();
        make_executable(&bin);
        bin
    }

    fn fake_tmux_target_disappears_after_ready(tmp: &Path) -> PathBuf {
        let bin = tmp.join("tmux");
        let log = tmp.join("tmux-log.jsonl");
        let count = tmp.join("capture-count");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
log = pathlib.Path(r'''{log}''')
count = pathlib.Path(r'''{count}''')
args = sys.argv[1:]
with log.open("a") as f:
    f.write(json.dumps(args) + "\n")
if args and args[0] == "capture-pane":
    n = int(count.read_text() or "0") if count.exists() else 0
    count.write_text(str(n + 1))
    if n < 2:
        print("ready")
        sys.exit(0)
    print("can't find pane", file=sys.stderr)
    sys.exit(1)
sys.exit(0)
"#,
            log = log.display(),
            count = count.display(),
        );
        std_fs::write(&bin, script).unwrap();
        make_executable(&bin);
        bin
    }

    fn read_log(tmp: &Path) -> Vec<Vec<String>> {
        let text = std_fs::read_to_string(tmp.join("tmux-log.jsonl")).unwrap();
        text.lines()
            .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
            .collect()
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

    #[tokio::test]
    async fn fire_mode_uses_safe_send_key_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux(tmp.path(), "ready", None);
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Fire,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: None,
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        dispatch(&config, "echo ok", Duration::from_secs(1), None)
            .await
            .unwrap();

        let log = read_log(tmp.path());
        assert!(log.contains(&vec![
            "send-keys".to_string(),
            "-t".to_string(),
            "sess:1.0".to_string(),
            "-X".to_string(),
            "cancel".to_string()
        ]));
        assert!(log.contains(&vec![
            "send-keys".to_string(),
            "-t".to_string(),
            "sess:1.0".to_string(),
            "C-u".to_string()
        ]));
        assert!(
            log.contains(&vec![
                "load-buffer".to_string(),
                "-b".to_string(),
                "lto-paste-sess-1.0-".to_string()
            ]) || log.iter().any(|args| {
                args.first().map(String::as_str) == Some("load-buffer")
                    && args.get(1).map(String::as_str) == Some("-b")
                    && args
                        .get(2)
                        .is_some_and(|item| item.starts_with("lto-paste-sess-1.0-"))
                    && args.get(4).map(String::as_str) == Some("stdin")
                    && args.get(5).map(String::as_str) == Some("echo ok")
            })
        );
        assert!(log.iter().any(|args| {
            args.first().map(String::as_str) == Some("paste-buffer")
                && args.iter().any(|item| item == "-d")
                && args.iter().any(|item| item == "-t")
                && args.iter().any(|item| item == "sess:1.0")
        }));
        assert!(log.contains(&vec![
            "send-keys".to_string(),
            "-t".to_string(),
            "sess:1.0".to_string(),
            "Enter".to_string()
        ]));
    }

    #[tokio::test]
    async fn signal_mode_appends_and_waits_for_tmux_signal() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux(tmp.path(), "finished", None);
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Signal,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done-test".to_string(),
            sentinel_path: None,
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        let summary = dispatch(&config, "echo ok", Duration::from_secs(1), None)
            .await
            .unwrap();

        assert_eq!(summary.capture.trim(), "finished");
        let log = read_log(tmp.path());
        assert!(
            log.iter()
                .any(|args| { args == &vec!["wait-for".to_string(), "done-test".to_string()] })
        );
        assert!(log.iter().any(|args| {
            args.first().map(String::as_str) == Some("load-buffer")
                && args
                    .last()
                    .is_some_and(|item| item.contains("tmux wait-for -S done-test"))
        }));
    }

    #[tokio::test]
    async fn sentinel_mode_polls_contract_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("done.sentinel");
        let bin = fake_tmux(tmp.path(), "waiting", Some(&sentinel));
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Sentinel,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: Some(sentinel.clone()),
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        let summary = dispatch(&config, "finish this", Duration::from_secs(1), None)
            .await
            .unwrap();

        assert_eq!(summary.capture, "done");
        assert_eq!(summary.sentinel_path, Some(sentinel));
        let log = read_log(tmp.path());
        assert!(log.iter().any(|args| {
            args.first().map(String::as_str) == Some("load-buffer")
                && args.last().is_some_and(|item| {
                    item.contains("printf '%s\\n' done >") && !item.contains("When finished")
                })
        }));
    }

    #[tokio::test]
    async fn sentinel_mode_fails_fast_when_target_disappears() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux_target_disappears_after_ready(tmp.path());
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Sentinel,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: Some(tmp.path().join("missing.sentinel")),
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };

        let err = dispatch(&config, "finish this", Duration::from_secs(30), None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("can't find pane"));
    }

    #[tokio::test]
    async fn fire_mode_outcome_is_fire_and_forget() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux(tmp.path(), "finished later", None);
        let prompt = tmp.path().join("prompt.txt");
        std_fs::write(&prompt, "sleep 5 && echo done").unwrap();
        let job = job_with_meta(BTreeMap::from([
            ("run_id".to_string(), serde_json::json!("r1")),
            (
                "tmux_bin".to_string(),
                serde_json::json!(bin.display().to_string()),
            ),
            ("tmux_mode".to_string(), serde_json::json!("fire")),
            ("tmux_target".to_string(), serde_json::json!("sess:1.0")),
            ("tmux_ready_timeout_sec".to_string(), serde_json::json!(5)),
            ("tmux_poll_interval_ms".to_string(), serde_json::json!(1)),
        ]));

        let outcome = run_job(&job, &prompt, tmp.path()).await.unwrap();

        assert_eq!(outcome.status, JobStatus::Ok);
        assert_eq!(outcome.exit_code, None);
        assert_eq!(outcome.reply_text, "");
        assert_eq!(
            outcome.cost.get("tmux_fire_and_forget"),
            Some(&serde_json::json!(true))
        );
    }

    #[tokio::test]
    async fn send_text_pastes_large_payload_before_enter() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux(tmp.path(), "ready", None);
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Fire,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: None,
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        let payload = "x".repeat(5000);
        dispatch(&config, &payload, Duration::from_secs(1), None)
            .await
            .unwrap();

        let log = read_log(tmp.path());
        let load_buffer = log
            .iter()
            .find(|args| args.first().map(String::as_str) == Some("load-buffer"))
            .unwrap();
        assert_eq!(load_buffer.last(), Some(&payload));
        let paste_idx = log
            .iter()
            .position(|args| args.first().map(String::as_str) == Some("paste-buffer"))
            .unwrap();
        let enter_idx = log
            .iter()
            .position(|args| args.last().map(String::as_str) == Some("Enter"))
            .unwrap();
        assert!(enter_idx > paste_idx);
    }

    #[tokio::test]
    async fn missing_tmux_binary_returns_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let config = TmuxRunnerConfig {
            tmux_bin: tmp.path().join("missing-tmux").display().to_string(),
            mode: TmuxMode::Fire,
            target: Some("sess:1.0".to_string()),
            session: None,
            new_window: false,
            new_session: false,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: None,
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        let err = dispatch(&config, "echo ok", Duration::from_secs(1), None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("tmux binary not found"));
    }

    #[tokio::test]
    async fn duplicate_new_session_reuses_existing_session_window() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_tmux_duplicate_session(tmp.path());
        let config = TmuxRunnerConfig {
            tmux_bin: bin.display().to_string(),
            mode: TmuxMode::Fire,
            target: None,
            session: Some("sess".to_string()),
            new_window: false,
            new_session: true,
            window_name: "lto-job".to_string(),
            signal_name: "done".to_string(),
            sentinel_path: None,
            ready_patterns: Vec::new(),
            skip_prompts: Vec::new(),
            ready_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(1),
            capture_lines: 20,
        };
        let summary = dispatch(&config, "echo ok", Duration::from_secs(1), None)
            .await
            .unwrap();

        assert_eq!(summary.target, "sess:2.0");
        let log = read_log(tmp.path());
        assert!(
            log.iter()
                .any(|args| args.first().map(String::as_str) == Some("new-session"))
        );
        assert!(
            log.iter()
                .any(|args| args.first().map(String::as_str) == Some("new-window"))
        );
    }

    #[test]
    fn config_reads_tmux_metadata_without_changing_agent_job_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([
            ("tmux_mode".to_string(), serde_json::json!("sentinel")),
            ("tmux_target".to_string(), serde_json::json!("sess:2.0")),
            ("tmux_sentinel".to_string(), serde_json::json!("done.txt")),
            (
                "tmux_ready_patterns".to_string(),
                serde_json::json!(["›", "ready"]),
            ),
            (
                "tmux_skip_prompts".to_string(),
                serde_json::json!(["upgrade now=n"]),
            ),
        ]));
        let config = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap();

        assert_eq!(config.mode, TmuxMode::Sentinel);
        assert_eq!(config.target.as_deref(), Some("sess:2.0"));
        assert_eq!(config.sentinel_path, Some(tmp.path().join("done.txt")));
        assert_eq!(config.ready_patterns, vec!["›", "ready"]);
        assert!(
            config
                .skip_prompts
                .iter()
                .any(|item| item.pattern == "upgrade now" && item.key == "n")
        );
    }

    #[test]
    fn config_ignores_generic_mode_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([(
            "mode".to_string(),
            serde_json::json!("sentinel"),
        )]));
        let config = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap();

        assert_eq!(config.mode, TmuxMode::Signal);
        assert_eq!(config.sentinel_path, None);
    }

    #[test]
    fn config_rejects_unsafe_signal_name() {
        let tmp = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([(
            "tmux_signal".to_string(),
            serde_json::json!("done; rm -rf /"),
        )]));
        let err = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap_err();

        assert!(err.to_string().contains("invalid tmux signal name"));
    }

    #[test]
    fn configured_signal_name_is_namespaced_per_job() {
        let tmp = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([
            ("run_id".to_string(), serde_json::json!("r1")),
            ("tmux_signal".to_string(), serde_json::json!("custom")),
        ]));
        let config = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap();

        assert!(config.signal_name.starts_with("custom-r1-job-1-"));
        assert!(config.signal_name.ends_with("-done"));
    }

    #[tokio::test]
    async fn live_snapshots_are_redacted_before_write() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("live.log");
        append_live_snapshot(
            Some(&log),
            "running",
            "sess:1.0",
            "token sk-123456789012 lives in /Users/ben/private/file",
        )
        .await
        .unwrap();

        let text = std_fs::read_to_string(log).unwrap();
        assert!(!text.contains("sk-123456789012"));
        assert!(!text.contains("/Users/ben/private"));
        assert!(text.contains("[REDACTED_SECRET]"));
        assert!(text.contains("[REDACTED_PATH]"));
    }

    #[test]
    fn config_rejects_sentinel_outside_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([
            ("tmux_mode".to_string(), serde_json::json!("sentinel")),
            (
                "tmux_sentinel".to_string(),
                serde_json::json!(outside.path().join("done").display().to_string()),
            ),
        ]));
        let err = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap_err();

        assert!(
            err.to_string()
                .contains("sentinel path must stay inside repo")
        );
    }

    #[test]
    fn config_rejects_sentinel_parent_dir_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let job = job_with_meta(BTreeMap::from([
            ("tmux_mode".to_string(), serde_json::json!("sentinel")),
            (
                "tmux_sentinel".to_string(),
                serde_json::json!("../outside/done"),
            ),
        ]));
        let err = TmuxRunnerConfig::from_job(&job, tmp.path()).unwrap_err();

        assert!(
            err.to_string()
                .contains("sentinel path must stay inside repo")
        );
    }
}
