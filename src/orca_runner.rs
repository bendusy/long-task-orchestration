use crate::tmux_runner::{TmuxRunnerConfig, first_matching_pattern, one_line_tail};
use anyhow::{Context, anyhow};
use serde_json::Value;
use std::process::Output;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

const ORCA_BIN_ENV: &str = "LTO_ORCA_BIN";
const SERVER_HINT: &str = "start the Orca app or use the default tmux backend";

fn orca_bin() -> String {
    std::env::var(ORCA_BIN_ENV).unwrap_or_else(|_| "orca".to_string())
}

async fn run(args: &[String]) -> anyhow::Result<Output> {
    let bin = orca_bin();
    Command::new(&bin)
        .args(args)
        .output()
        .await
        .with_context(|| format!("start orca binary {bin:?}; {SERVER_HINT}"))
}

/// Orca always exits 0 and reports failure as `{"ok": false, "error": {...}}`,
/// so the envelope — not the exit status — decides success.
fn envelope_error(value: &Value) -> Option<String> {
    if value.get("ok") == Some(&Value::Bool(true)) {
        return None;
    }
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("orca reported no message");
    Some(format!("{code}: {message}"))
}

fn command_error(args: &[String], output: &Output) -> anyhow::Error {
    anyhow!(
        "orca command failed: orca {} (status={}) {}",
        args.join(" "),
        output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        one_line_tail(&String::from_utf8_lossy(&output.stderr), 500)
    )
}

async fn output(args: &[String]) -> anyhow::Result<Value> {
    let result = run(args).await?;
    let stdout = String::from_utf8_lossy(&result.stdout);
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else {
        if !result.status.success() {
            return Err(command_error(args, &result));
        }
        return Err(anyhow!(
            "orca {} returned non-JSON output: {}",
            args.join(" "),
            one_line_tail(&stdout, 500)
        ));
    };
    if let Some(reason) = envelope_error(&value) {
        return Err(anyhow!("orca {} failed: {reason}", args.join(" ")));
    }
    Ok(value)
}

async fn ensure_runtime() -> anyhow::Result<()> {
    let args = vec!["status".to_string(), "--json".to_string()];
    let value = output(&args)
        .await
        .map_err(|err| anyhow!("{err}; {SERVER_HINT}"))?;
    if value.pointer("/result/runtime/reachable") == Some(&Value::Bool(true)) {
        return Ok(());
    }
    let state = value
        .pointer("/result/runtime/state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Err(anyhow!(
        "orca runtime is not reachable (state={state}); {SERVER_HINT}"
    ))
}

fn read_args(config: &TmuxRunnerConfig, target: &str) -> Vec<String> {
    vec![
        "terminal".to_string(),
        "read".to_string(),
        "--terminal".to_string(),
        target.to_string(),
        // Without --screen orca returns the accumulated byte stream, where any
        // repainted line (shell prompts, TUI runners) arrives as stacked
        // fragments — unusable for the pattern matching below.
        "--screen".to_string(),
        "--limit".to_string(),
        config.capture_lines.to_string(),
        "--json".to_string(),
    ]
}

/// Orca drops the scrollback of a terminal whose process exited: `read`
/// answers with an empty tail even though `show` still lists the handle
/// (`orphaned: true`), leaving only a one-line `preview`. Runners that quit
/// when done — codex does — therefore have to be read before they exit, and
/// a goal whose result matters should write it to a file rather than rely on
/// terminal output surviving.
async fn read_terminal(config: &TmuxRunnerConfig, target: &str) -> anyhow::Result<String> {
    let value = output(&read_args(config, target)).await?;
    Ok(join_tail(&value))
}

fn join_tail(value: &Value) -> String {
    value
        .pointer("/result/terminal/tail")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

async fn terminal_present(target: &str) -> anyhow::Result<bool> {
    let args = vec![
        "terminal".to_string(),
        "show".to_string(),
        "--terminal".to_string(),
        target.to_string(),
        "--json".to_string(),
    ];
    match output(&args).await {
        Ok(_) => Ok(true),
        Err(err) if is_missing_error(&err.to_string()) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Treat a gone terminal as already closed. `terminal_handle_stale` belongs
/// here: once the dispatched runner exits, its pty dies and Orca reports the
/// handle stale (`orphaned: true`) even though the tab record lingers, so a
/// close after a normal completion would otherwise fail every run.
fn is_missing_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("terminal_not_found")
        || text.contains("terminal_handle_stale")
        || text.contains("selector_not_found")
        || text.contains("not found")
}

pub async fn prepare_dispatch_target(config: &TmuxRunnerConfig) -> anyhow::Result<String> {
    ensure_runtime().await?;
    if let Some(target) = config.target.as_deref() {
        let args = vec![
            "terminal".to_string(),
            "show".to_string(),
            "--terminal".to_string(),
            target.to_string(),
            "--json".to_string(),
        ];
        output(&args).await?;
        return Ok(target.to_string());
    }
    // `--worktree active` resolves against the CLI's own cwd, which is not the
    // dispatch working dir; address the workspace by path so the terminal lands
    // in the repo the goal is about.
    let cwd = worktree_path(config.working_dir.as_deref())?;
    let args = vec![
        "terminal".to_string(),
        "create".to_string(),
        "--worktree".to_string(),
        format!("path:{cwd}"),
        "--title".to_string(),
        config.window_name.clone(),
        "--json".to_string(),
    ];
    let value = output(&args).await?;
    let target = value
        .pointer("/result/terminal/handle")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("orca terminal create response has no terminal handle"))?;
    wait_for_shell_ready(config, &target).await?;
    Ok(target)
}

/// Orca matches `path:` selectors against its own workspace registry, so the
/// path must be absolute and free of `.`/`..` segments — a working dir of "."
/// otherwise becomes `path:/repo/.` and matches no workspace.
fn worktree_path(working_dir: Option<&std::path::Path>) -> anyhow::Result<String> {
    let raw = match working_dir {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir()
            .context("resolve orca worktree path")?
            .join(path),
        None => std::env::current_dir().context("resolve orca worktree path")?,
    };
    // canonicalize also resolves symlinks, which is what Orca stored when the
    // workspace was registered from a symlinked checkout.
    let resolved = raw.canonicalize().unwrap_or(raw);
    Ok(resolved.display().to_string())
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
                "orca terminal {target} shell did not become ready within {}s; last capture: {}",
                config.ready_timeout.as_secs(),
                one_line_tail(&capture, 500)
            ));
        }
        sleep(config.poll_interval).await;
    }
}

/// `terminal wait --for tui-idle` only settles once a TUI agent is idle, so a
/// plain shell pane always times out there; poll for a stable capture instead.
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
                "orca terminal {target} did not become ready within {}s; last capture: {}",
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
        "send".to_string(),
        "--terminal".to_string(),
        target.to_string(),
        "--text".to_string(),
        text.to_string(),
        "--enter".to_string(),
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
        return Err(anyhow!("orca terminal {target} is gone"));
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
                "orca terminal {target} did not show dispatch confirmation within {}s; last capture: {}",
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
            "{hint} in orca terminal {target} (matched {pattern:?}); resolve it in Orca, then re-dispatch with --target {target}"
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
    let bin = orca_bin();
    let args = vec![
        "terminal".to_string(),
        "close".to_string(),
        "--terminal".to_string(),
        target.to_string(),
        "--tab".to_string(),
        "--json".to_string(),
    ];
    let output = std::process::Command::new(&bin)
        .args(&args)
        .output()
        .with_context(|| format!("start orca binary {bin:?}; {SERVER_HINT}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else {
        if !output.status.success() {
            return Err(command_error(&args, &output));
        }
        return Err(anyhow!(
            "orca terminal close returned non-JSON output: {}",
            one_line_tail(&stdout, 500)
        ));
    };
    match envelope_error(&value) {
        None => Ok(CloseOutcome::Closed),
        Some(reason) if is_missing_error(&reason) => Ok(CloseOutcome::Missing),
        Some(reason) => Err(anyhow!("orca terminal close failed: {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_error_reads_ok_and_error_code() {
        assert_eq!(envelope_error(&json!({"ok": true, "result": {}})), None);
        let failure =
            json!({"ok": false, "error": {"code": "terminal_not_found", "message": "gone"}});
        assert_eq!(
            envelope_error(&failure).as_deref(),
            Some("terminal_not_found: gone")
        );
    }

    #[test]
    fn join_tail_renders_screen_lines() {
        let value = json!({"result": {"terminal": {"tail": ["first", "second"]}}});
        assert_eq!(join_tail(&value), "first\nsecond");
        assert_eq!(join_tail(&json!({"result": {}})), "");
    }

    #[test]
    fn worktree_path_strips_dot_segments_and_absolutizes() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.canonicalize().unwrap_or(cwd.clone());
        let expected = expected.display().to_string();
        assert_eq!(worktree_path(None).unwrap(), expected);
        assert_eq!(
            worktree_path(Some(std::path::Path::new("."))).unwrap(),
            expected
        );
        assert_eq!(worktree_path(Some(&cwd)).unwrap(), expected);
    }

    #[test]
    fn missing_errors_map_to_missing_outcome() {
        assert!(is_missing_error("terminal_not_found: gone"));
        assert!(is_missing_error("selector_not_found: no worktree"));
        // A runner that finished leaves a stale handle; that is not a failure.
        assert!(is_missing_error(
            "terminal_handle_stale: terminal_handle_stale"
        ));
        assert!(is_missing_error("TERMINAL_HANDLE_STALE"));
        assert!(!is_missing_error("runtime_unavailable: start the app"));
    }
}
