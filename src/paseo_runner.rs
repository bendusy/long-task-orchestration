use crate::tmux_runner::{TmuxRunnerConfig, first_matching_pattern, one_line_tail};
use anyhow::{Context, anyhow};
use serde_json::Value;
use std::process::Output;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

const PASEO_BIN_ENV: &str = "LTO_PASEO_BIN";
const SERVER_HINT: &str = "start the paseo daemon or use the default tmux backend";

fn paseo_bin() -> String {
    std::env::var(PASEO_BIN_ENV).unwrap_or_else(|_| "paseo".to_string())
}

async fn run(args: &[String]) -> anyhow::Result<Output> {
    let bin = paseo_bin();
    Command::new(&bin)
        .args(args)
        .output()
        .await
        .with_context(|| format!("start paseo binary {bin:?}; {SERVER_HINT}"))
}

fn command_error(args: &[String], output: &Output) -> anyhow::Error {
    anyhow!(
        "paseo command failed: paseo {} (status={}) {}",
        args.join(" "),
        output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        one_line_tail(&String::from_utf8_lossy(&output.stderr), 500)
    )
}

/// Paseo has no success envelope: a successful call returns the bare JSON
/// object, a failed call returns `{"error": {...}}`. So success is "no
/// `error` key", checked alongside the exit status.
fn envelope_error(value: &Value) -> Option<String> {
    let code = value.pointer("/error/code").and_then(Value::as_str)?;
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("paseo reported no message");
    Some(format!("{code}: {message}"))
}

/// Paseo prints a successful result to stdout but an `{"error": {...}}`
/// envelope to STDERR, so reading stdout alone loses every error code and a
/// missing terminal would look like a hard failure instead of an already-closed
/// one. Parse stdout first, then fall back to stderr.
fn parse_envelope(args: &[String], result: &Output) -> anyhow::Result<Value> {
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    for stream in [stdout.trim(), stderr.trim()] {
        if let Ok(value) = serde_json::from_str::<Value>(stream) {
            return Ok(value);
        }
    }
    if !result.status.success() {
        return Err(command_error(args, result));
    }
    Err(anyhow!(
        "paseo {} returned non-JSON output: {}",
        args.join(" "),
        one_line_tail(&stdout, 500)
    ))
}

async fn output(args: &[String]) -> anyhow::Result<Value> {
    let result = run(args).await?;
    let value = parse_envelope(args, &result)?;
    if let Some(reason) = envelope_error(&value) {
        return Err(anyhow!("paseo {} failed: {reason}", args.join(" ")));
    }
    Ok(value)
}

async fn ensure_runtime() -> anyhow::Result<()> {
    let args = vec!["status".to_string(), "--json".to_string()];
    let value = output(&args)
        .await
        .map_err(|err| anyhow!("{err}; {SERVER_HINT}"))?;
    let local = value.get("localDaemon").and_then(Value::as_str);
    let connected = value.get("connectedDaemon").and_then(Value::as_str);
    if local == Some("running") && connected == Some("reachable") {
        return Ok(());
    }
    Err(anyhow!(
        "paseo daemon is not reachable (localDaemon={:?}, connectedDaemon={:?}); {SERVER_HINT}",
        local.unwrap_or("unknown"),
        connected.unwrap_or("unknown")
    ))
}

fn capture_args(config: &TmuxRunnerConfig, target: &str) -> Vec<String> {
    // paseo caps lines via --start/--end (line offsets), not a line-count
    // limit like orca/herdr; config.capture_lines has no direct equivalent
    // here, so the full scrollback window paseo returns by default is used.
    let _ = config.capture_lines;
    vec![
        "terminal".to_string(),
        "capture".to_string(),
        target.to_string(),
        "--json".to_string(),
    ]
}

async fn read_terminal(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<String> {
    let value = output(&capture_args(config, target)).await?;
    Ok(join_lines(&value))
}

fn join_lines(value: &Value) -> String {
    value
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

async fn terminal_present(target: &str) -> anyhow::Result<bool> {
    let args = vec![
        "terminal".to_string(),
        "capture".to_string(),
        target.to_string(),
        "--json".to_string(),
    ];
    match output(&args).await {
        Ok(_) => Ok(true),
        Err(err) if is_missing_error(&err.to_string()) => Ok(false),
        Err(err) => Err(err),
    }
}

fn is_missing_error(text: &str) -> bool {
    text.to_lowercase().contains("terminal_not_found") || text.contains("not found")
}

pub async fn prepare_dispatch_target(config: &TmuxRunnerConfig) -> anyhow::Result<String> {
    ensure_runtime().await?;
    if let Some(target) = config.target.as_deref() {
        if !terminal_present(target).await? {
            return Err(anyhow!("paseo terminal {target} is gone"));
        }
        return Ok(target.to_string());
    }
    // paseo has no --worktree concept; the workspace is addressed by cwd.
    let cwd = config
        .working_dir
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let args = vec![
        "terminal".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        cwd,
        "--name".to_string(),
        config.window_name.clone(),
        "--json".to_string(),
    ];
    let value = output(&args).await?;
    let target = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("paseo terminal create response has no id"))?;
    wait_for_shell_ready(config, &target).await?;
    Ok(target)
}

async fn wait_for_shell_ready(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<()> {
    let deadline = Instant::now() + config.ready_timeout;
    loop {
        let capture = read_terminal(config, target).await?;
        if !capture.trim().is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "paseo terminal {target} shell did not become ready within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            ));
        }
        sleep(config.poll_interval).await;
    }
}

/// paseo's `terminal` commands are plain-shell semantics with no agent-idle
/// wait, unlike its own agent primitives — so ready detection polls capture
/// stability, same as orca's plain-shell pane.
pub async fn wait_for_dispatch_ready(
    config: &TmuxRunnerConfig,
    target: &str,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + config.ready_timeout;
    let mut previous = None;
    loop {
        let capture = read_terminal(config, target).await?;
        reject_blocked(config, target, &capture)?;
        if !config.ready_patterns.is_empty()
            && first_matching_pattern(&capture, &config.ready_patterns).is_some()
        {
            return Ok(());
        }
        let normalized = capture.trim().to_string();
        if config.ready_patterns.is_empty()
            && !normalized.is_empty()
            && previous.as_ref() == Some(&normalized)
        {
            return Ok(());
        }
        previous = Some(normalized);
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "paseo terminal {target} did not become ready within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            ));
        }
        sleep(config.poll_interval).await;
    }
}

pub async fn send_dispatch_text(
    _config: &TmuxRunnerConfig,
    target: &str,
    text: &str,
) -> anyhow::Result<()> {
    let args = vec![
        "terminal".to_string(),
        "send-keys".to_string(),
        target.to_string(),
        text.to_string(),
        "Enter".to_string(),
        "--json".to_string(),
    ];
    output(&args).await.map(|_| ())
}

pub async fn confirm_tui_input(
    config: &TmuxRunnerConfig,
    target: &str,
    _probe: &str,
) -> anyhow::Result<String> {
    if !terminal_present(target).await? {
        return Err(anyhow!("paseo terminal {target} is gone"));
    }
    read_terminal(config, target).await
}

pub async fn wait_for_capture_patterns(
    config: &TmuxRunnerConfig,
    target: &str,
    patterns: &[String],
) -> anyhow::Result<String> {
    let deadline = Instant::now() + config.ready_timeout;
    loop {
        let capture = read_terminal(config, target).await?;
        reject_blocked(config, target, &capture)?;
        if patterns.is_empty() || first_matching_pattern(&capture, patterns).is_some() {
            return Ok(capture);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "paseo terminal {target} did not show dispatch confirmation within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            ));
        }
        sleep(config.poll_interval).await;
    }
}

fn reject_blocked(config: &TmuxRunnerConfig, target: &str, capture: &str) -> anyhow::Result<()> {
    if let Some(pattern) = first_matching_pattern(capture, &config.dispatch_safety.blocked_patterns)
    {
        let hint = config
            .dispatch_safety
            .blocked_prompt_hint
            .as_deref()
            .unwrap_or("runner is blocked on an interactive prompt");
        anyhow::bail!(
            "{hint} in paseo terminal {target} (matched {pattern:?}); resolve it in Paseo, then re-dispatch with --target {target}"
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    Missing,
}

pub fn close_dispatch_target(target: &str) -> anyhow::Result<CloseOutcome> {
    let bin = paseo_bin();
    let args = vec![
        "terminal".to_string(),
        "kill".to_string(),
        target.to_string(),
        "--json".to_string(),
    ];
    let output = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .with_context(|| format!("start paseo binary {bin:?}; {SERVER_HINT}"))?;
    let value = parse_envelope(&args, &output)?;
    match envelope_error(&value) {
        None => Ok(CloseOutcome::Closed),
        Some(reason) if is_missing_error(&reason) => Ok(CloseOutcome::Missing),
        Some(reason) => Err(anyhow!("paseo terminal kill failed: {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_error_reads_bare_object_vs_error_key() {
        assert_eq!(envelope_error(&json!({"id": "abc", "cwd": "/tmp"})), None);
        let failure = json!({"error": {"code": "TERMINAL_NOT_FOUND", "message": "gone"}});
        assert_eq!(
            envelope_error(&failure).as_deref(),
            Some("TERMINAL_NOT_FOUND: gone")
        );
    }

    #[test]
    fn join_lines_trims_trailing_blank_lines() {
        let value = json!({"lines": ["first", "second", "", "", ""]});
        assert_eq!(join_lines(&value), "first\nsecond");
        assert_eq!(join_lines(&json!({})), "");
    }

    #[test]
    fn parse_envelope_reads_error_json_from_stderr() {
        use std::os::unix::process::ExitStatusExt;
        let args = vec!["terminal".to_string(), "kill".to_string()];
        let failed = Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: br#"{"error":{"code":"TERMINAL_NOT_FOUND","message":"gone"}}"#.to_vec(),
        };
        let value = parse_envelope(&args, &failed).unwrap();
        assert_eq!(
            envelope_error(&value).as_deref(),
            Some("TERMINAL_NOT_FOUND: gone")
        );

        let ok = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: br#"{"terminalId":"t1","success":true}"#.to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(envelope_error(&parse_envelope(&args, &ok).unwrap()), None);
    }

    #[test]
    fn missing_errors_match_case_insensitively() {
        assert!(is_missing_error("TERMINAL_NOT_FOUND: No terminal found"));
        assert!(is_missing_error("terminal_not_found: gone"));
        assert!(!is_missing_error("DAEMON_UNREACHABLE: start the app"));
    }
}
