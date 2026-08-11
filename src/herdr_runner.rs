use crate::tmux_runner::{TmuxRunnerConfig, first_matching_pattern, one_line_tail};
use anyhow::{Context, anyhow};
use serde_json::Value;
use std::process::Output;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

const HERDR_BIN_ENV: &str = "LTO_HERDR_BIN";
const SERVER_HINT: &str = "start herdr or use the default tmux backend";

fn herdr_bin() -> String {
    std::env::var(HERDR_BIN_ENV).unwrap_or_else(|_| "herdr".to_string())
}

async fn run(args: &[String]) -> anyhow::Result<Output> {
    let bin = herdr_bin();
    Command::new(&bin)
        .args(args)
        .output()
        .await
        .with_context(|| format!("start herdr binary {bin:?}; {SERVER_HINT}"))
}

fn command_error(args: &[String], output: &Output) -> anyhow::Error {
    anyhow!(
        "herdr command failed: herdr {} (status={}) {}",
        args.join(" "),
        output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        one_line_tail(&String::from_utf8_lossy(&output.stderr), 500)
    )
}

async fn output(args: &[String]) -> anyhow::Result<String> {
    let result = run(args).await?;
    if !result.status.success() {
        return Err(command_error(args, &result));
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}

async fn ensure_server() -> anyhow::Result<()> {
    let args = vec!["status".to_string(), "--json".to_string()];
    let result = run(&args)
        .await
        .map_err(|err| anyhow!("{err}; {SERVER_HINT}"))?;
    if !result.status.success() {
        return Err(anyhow!("herdr server is not running; {SERVER_HINT}"));
    }
    let value: Value = serde_json::from_slice(&result.stdout)
        .map_err(|err| anyhow!("herdr status returned invalid JSON: {err}; {SERVER_HINT}"))?;
    if value.pointer("/server/running") == Some(&Value::Bool(true))
        || value.pointer("/server/status").and_then(Value::as_str) == Some("running")
    {
        return Ok(());
    }
    Err(anyhow!("herdr server is not running; {SERVER_HINT}"))
}

fn pane_read_args(config: &TmuxRunnerConfig, target: &str, source: &str) -> Vec<String> {
    vec![
        "pane".to_string(),
        "read".to_string(),
        target.to_string(),
        "--source".to_string(),
        source.to_string(),
        "--lines".to_string(),
        config.capture_lines.to_string(),
        "--format".to_string(),
        "text".to_string(),
    ]
}

async fn read_pane(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<String> {
    output(&pane_read_args(config, target, "recent")).await
}

async fn read_visible_pane(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<String> {
    output(&pane_read_args(config, target, "visible")).await
}

async fn agent_present(target: &str) -> anyhow::Result<bool> {
    let args = vec!["agent".to_string(), "get".to_string(), target.to_string()];
    let result = run(&args).await?;
    if result.status.success() {
        return Ok(true);
    }
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if text.contains("agent_not_found") || text.contains("agent not found") {
        return Ok(false);
    }
    Err(command_error(&args, &result))
}

pub async fn prepare_dispatch_target(config: &TmuxRunnerConfig) -> anyhow::Result<String> {
    ensure_server().await?;
    if let Some(target) = config.target.as_deref() {
        let args = vec!["pane".to_string(), "get".to_string(), target.to_string()];
        output(&args).await?;
        return Ok(target.to_string());
    }
    let cwd = config
        .working_dir
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let mut args = vec![
        "tab".to_string(),
        "create".to_string(),
        "--cwd".to_string(),
        cwd,
        "--label".to_string(),
        config.window_name.clone(),
        "--no-focus".to_string(),
    ];
    // Keep dispatch tabs in the host's own workspace; without --workspace,
    // herdr picks one on its own and the pane can land in another space.
    if let Ok(workspace) = std::env::var("HERDR_WORKSPACE_ID")
        && !workspace.is_empty()
    {
        args.push("--workspace".to_string());
        args.push(workspace);
    }
    let text = output(&args).await?;
    let value: Value = serde_json::from_str(&text).context("parse herdr tab create response")?;
    let target = value
        .pointer("/result/root_pane/pane_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("herdr tab create response has no root pane id"))?;
    wait_for_shell_ready(config, &target).await?;
    Ok(target)
}

async fn wait_for_shell_ready(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<()> {
    let deadline = Instant::now() + config.ready_timeout;
    loop {
        let capture = read_visible_pane(config, target).await?;
        if !capture.trim().is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "herdr pane {target} shell did not become ready within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            ));
        }
        sleep(config.poll_interval).await;
    }
}

pub async fn wait_for_dispatch_ready(
    config: &TmuxRunnerConfig,
    target: &str,
) -> anyhow::Result<()> {
    if config.ready_patterns.is_empty() {
        return wait_for_stable_capture(config, target).await;
    }
    let deadline = Instant::now() + config.ready_timeout;
    // herdr marks unfocused background panes "done" instead of "idle", so a
    // reused --target pane never matches --until idle alone; blocked returns
    // fast and is rejected below by reject_blocked.
    let args = vec![
        "agent".to_string(),
        "wait".to_string(),
        target.to_string(),
        "--until".to_string(),
        "idle".to_string(),
        "--until".to_string(),
        "done".to_string(),
        "--until".to_string(),
        "blocked".to_string(),
        "--timeout".to_string(),
        config
            .ready_timeout
            .as_millis()
            .min(u128::from(u64::MAX))
            .to_string(),
    ];
    loop {
        match output(&args).await {
            Ok(_) => {
                let capture = read_pane(config, target).await?;
                reject_blocked(config, target, &capture)?;
                return Ok(());
            }
            Err(err) if err.to_string().contains("agent_not_found") => {
                if Instant::now() >= deadline {
                    return Err(err);
                }
                sleep(config.poll_interval).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn wait_for_stable_capture(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<()> {
    let deadline = Instant::now() + config.ready_timeout;
    let mut previous = None;
    loop {
        let capture = read_pane(config, target).await?;
        reject_blocked(config, target, &capture)?;
        let normalized = capture.trim().to_string();
        if !normalized.is_empty() && previous.as_ref() == Some(&normalized) {
            return Ok(());
        }
        previous = Some(normalized);
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "herdr pane {target} did not produce stable ready output within {}s; last capture: {}",
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
    if !is_shell_command(text) {
        let args = vec![
            "agent".to_string(),
            "prompt".to_string(),
            target.to_string(),
            text.to_string(),
        ];
        output(&args).await?;
        return Ok(());
    }
    let args = vec![
        "pane".to_string(),
        "run".to_string(),
        target.to_string(),
        text.to_string(),
    ];
    output(&args).await.map(|_| ())
}

fn is_shell_command(text: &str) -> bool {
    text.starts_with("export ") || text.starts_with("cd ") || text.starts_with("LTO_RUN_ID=")
}

pub async fn confirm_tui_input(
    config: &TmuxRunnerConfig,
    target: &str,
    _probe: &str,
) -> anyhow::Result<String> {
    if !agent_present(target).await? {
        return Err(anyhow!("herdr agent is not ready in pane {target}"));
    }
    read_pane(config, target).await
}

async fn agent_status(target: &str) -> anyhow::Result<Option<String>> {
    let args = vec!["agent".to_string(), "get".to_string(), target.to_string()];
    let result = run(&args).await?;
    if !result.status.success() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_slice(&result.stdout).context("parse herdr agent get response")?;
    Ok(value
        .pointer("/result/agent/agent_status")
        .and_then(Value::as_str)
        .map(str::to_string))
}

pub async fn wait_for_capture_patterns(
    config: &TmuxRunnerConfig,
    target: &str,
    patterns: &[String],
) -> anyhow::Result<String> {
    let deadline = Instant::now() + config.ready_timeout;
    loop {
        let capture = read_pane(config, target).await?;
        reject_blocked(config, target, &capture)?;
        if patterns.is_empty() || first_matching_pattern(&capture, patterns).is_some() {
            return Ok(capture);
        }
        // agent prompt submits atomically, so "working" already proves the
        // runner accepted the goal — confirm patterns can scroll off the TUI
        // before we ever read them when the runner takes long on its first step.
        if agent_status(target).await? == Some("working".to_string()) {
            return Ok(capture);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "herdr pane {target} did not show dispatch confirmation within {}s; last capture: {}",
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
            "{hint} in herdr pane {target} (matched {pattern:?}); resolve it in herdr, then re-dispatch with --target {target}"
        );
    }
    Ok(())
}

pub async fn report_metadata(target: &str, run_id: &str, goal: &str) {
    let args = vec![
        "pane".to_string(),
        "report-metadata".to_string(),
        target.to_string(),
        "--source".to_string(),
        "lto".to_string(),
        "--token".to_string(),
        format!("run_id={run_id}"),
        "--token".to_string(),
        format!("goal={goal}"),
    ];
    if let Err(err) = output(&args).await {
        eprintln!("warning: herdr metadata report failed for {target}: {err}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    Missing,
}

pub fn close_dispatch_target(target: &str) -> anyhow::Result<CloseOutcome> {
    let bin = herdr_bin();
    let args = vec!["pane".to_string(), "close".to_string(), target.to_string()];
    let output = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .with_context(|| format!("start herdr binary {bin:?}; {SERVER_HINT}"))?;
    if output.status.success() {
        return Ok(CloseOutcome::Closed);
    }
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if text.contains("pane_not_found")
        || text.contains("tab_not_found")
        || text.contains("not found")
    {
        return Ok(CloseOutcome::Missing);
    }
    Err(command_error(&args, &output))
}
