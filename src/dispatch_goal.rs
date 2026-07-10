use crate::commands::util;
use crate::events::{self, EventRecord};
use crate::state::DispatchWindowState;
use crate::tmux_runner::{self, SkipPrompt, TmuxDispatchSafety, TmuxMode, TmuxRunnerConfig};
use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LTO_HOOK_MARKER: &str = "long-task-orchestration";
const DEFAULT_COMPLETION_WAIT_SEC: u64 = 600;
const DEFAULT_DISPATCH_READY_TIMEOUT_SEC: u64 = 60;
const CODEX_STOP_HOOK: &str = include_str!("../scripts/hooks/codex-stop-notify.sh");
const GOAL_CONSTRAINT_SUMMARY: &str = "Use LTO discipline: keep LTO self-managed, run required redlines, and write the local commit when accepted while leaving release/tag/push to the host. For heterogeneous audit before closeout, you are a host: run `lto audit --auto-dispatch` (and `lto dispatch-goal --runner <name> --goal <file>` for sub-tasks), then block on `lto events --wait --event-type agent.dispatch.completed --timeout <sec>` to collect replies. NEVER impersonate another runner or hand-write reply-*.md yourself — that fabricates cross-family audit evidence; if no healthy heterogeneous runner is available, stop and report blocked rather than self-audit. For any external docs/API/tool-capability lookup, route through `hs` first (Hybrid Search) rather than raw web fetches, then confirm against the local `--help`/config/binary before acting — external sources can name the wrong project; local evidence decides.";

#[derive(Debug, Clone)]
pub struct DispatchGoalOptions {
    pub run_id: Option<String>,
    pub runner: String,
    pub goal: PathBuf,
    pub target: Option<String>,
    pub new_window: bool,
    pub window_name: Option<String>,
    pub keep_window: bool,
    pub cwd: Option<PathBuf>,
    pub tmux_session: Option<String>,
    pub tmux_bin: Option<String>,
    pub ready_timeout_sec: Option<u64>,
    pub notify_cmd: Option<String>,
    pub no_install_hooks: bool,
    pub uninstall_hooks: bool,
}

#[derive(Debug, Clone)]
struct GoalRunnerPlan {
    launch: Option<String>,
    prompt: String,
    ready_patterns: Vec<String>,
    confirm_patterns: Vec<String>,
    needs_probe: bool,
    completion_event: Option<String>,
    completion_mode: String,
    /// When true the launch command already carries the initial prompt (e.g.
    /// `agy -i '<prompt>'`), so run_dispatch must NOT also send the prompt as a
    /// separate line — doing so would submit it twice. codex/pi start a REPL
    /// first and take the prompt on a later line, so they leave this false.
    launch_includes_prompt: bool,
}

#[derive(Debug, Clone)]
struct HookStatus {
    status: String,
    detail: String,
    script_path: Option<PathBuf>,
}

pub fn cmd_dispatch_goal(repo: &Path, options: DispatchGoalOptions) -> anyhow::Result<()> {
    if options.uninstall_hooks {
        let codex = uninstall_codex_hook(repo)?;
        println!("codex hook uninstall: {} ({})", codex.status, codex.detail);
        let agy = uninstall_agy_hook(repo)?;
        println!("agy hook uninstall: {} ({})", agy.status, agy.detail);
        return Ok(());
    }
    validate_runner(&options.runner)?;
    if !options.goal.exists() {
        anyhow::bail!("goal file does not exist: {}", options.goal.display());
    }
    validate_dispatch_target(options.target.as_deref(), options.new_window)?;

    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    persist_notify_cmd(&mut ctx, options.notify_cmd.as_deref())?;
    let goal_path = absolutize(&options.goal)?;
    let cwd = options.cwd.clone().unwrap_or_else(|| repo.to_path_buf());
    let degraded = |err: anyhow::Error| HookStatus {
        status: "degraded".to_string(),
        detail: err.to_string(),
        script_path: None,
    };
    let hook_status = if options.no_install_hooks {
        HookStatus {
            status: "skipped".to_string(),
            detail: "--no-install-hooks".to_string(),
            script_path: None,
        }
    } else {
        match options.runner.as_str() {
            // Codex needs its Stop hook as a sampling point: the Rust handler
            // only promotes a turn to dispatch-complete after transcript proof
            // that the active /goal was marked complete. pi/agy use process-exit
            // wrappers with a real rc and need no global completion hook.
            "codex" => install_codex_hook(repo).unwrap_or_else(degraded),
            "agy" => uninstall_agy_hook(repo).unwrap_or_else(degraded),
            _ => HookStatus {
                status: "skipped".to_string(),
                detail: "runner completion uses the process-exit wrapper".to_string(),
                script_path: None,
            },
        }
    };

    let outcome = run_dispatch(repo, &mut ctx, &options, &goal_path, &cwd)?;
    let dispatch_path = write_dispatch_record(
        &ctx.run_dir,
        &options,
        &goal_path,
        &cwd,
        &outcome,
        &hook_status,
    )?;
    events::safe_emit(
        repo,
        &ctx.run_id,
        EventRecord {
            event_type: "runner.started".to_string(),
            actor_kind: "runner".to_string(),
            actor_id: Some(options.runner.clone()),
            phase: Some(ctx.state.current_phase.clone()),
            summary: format!("dispatch-goal {}", options.runner),
            artifact_refs: vec![dispatch_path.display().to_string()],
            fields: json!({
                "runner": options.runner,
                "goal": goal_path.display().to_string(),
                "target": outcome.target,
                "window_id": outcome.window_id,
                "cleanup_on_success": !options.keep_window,
                "completion_event": outcome.completion_event,
                "completion_mode": outcome.completion_mode,
                "turns_jsonl": false,
                "hook_status": hook_status.status,
            }),
            ..EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &ctx.run_id);

    println!("status=dispatched");
    println!("runner={}", options.runner);
    println!("target={}", outcome.target);
    println!(
        "window_id={}",
        outcome.window_id.as_deref().unwrap_or("none")
    );
    println!("dispatch_record={}", dispatch_path.display());
    println!(
        "completion_event={}",
        outcome.completion_event.as_deref().unwrap_or("none")
    );
    println!("completion_mode={}", outcome.completion_mode);
    println!("hook_status={} {}", hook_status.status, hook_status.detail);
    println!("wait_command={}", completion_wait_command(&ctx.run_id));
    println!(
        "dispatch_and_wait={}",
        dispatch_and_wait_command(&options, &ctx.run_id)
    );
    Ok(())
}

fn persist_notify_cmd(ctx: &mut util::RunContext, notify_cmd: Option<&str>) -> anyhow::Result<()> {
    if let Some(notify_cmd) = notify_cmd {
        ctx.state.notify_cmd = Some(notify_cmd.to_string());
        util::save_run(ctx)?;
    }
    Ok(())
}

fn persist_dispatch_window(
    ctx: &mut util::RunContext,
    options: &DispatchGoalOptions,
    config: &TmuxRunnerConfig,
    target: &str,
    window_id: &str,
) -> anyhow::Result<()> {
    if let Some(window) = ctx.state.dispatch_windows.iter_mut().rev().find(|window| {
        window.window_id == window_id
            && window.runner == options.runner
            && window.status != "cleaned"
    }) {
        window.target = target.to_string();
        window.tmux_bin = config.tmux_bin.clone();
        window.cleanup_on_success = !options.keep_window;
        window.status = "active".to_string();
        window.finished_at = None;
        window.retention_reason = None;
        return util::save_run(ctx);
    }
    ctx.state.dispatch_windows.push(DispatchWindowState {
        window_id: window_id.to_string(),
        target: target.to_string(),
        runner: options.runner.clone(),
        tmux_bin: config.tmux_bin.clone(),
        cleanup_on_success: !options.keep_window,
        status: "active".to_string(),
        created_at: crate::state::iso_now(),
        finished_at: None,
        retention_reason: None,
    });
    util::save_run(ctx)
}

fn dispatch_window_id(
    ctx: &util::RunContext,
    options: &DispatchGoalOptions,
    target: &str,
) -> Option<String> {
    if options.target.is_none() {
        return tmux_runner::window_id_from_target(target);
    }
    let target_window_id = tmux_runner::window_id_from_target(target);
    ctx.state
        .dispatch_windows
        .iter()
        .rev()
        .find(|window| {
            window.runner == options.runner
                && window.status != "cleaned"
                && (window.target == target
                    || target_window_id.as_deref() == Some(window.window_id.as_str()))
        })
        .map(|window| window.window_id.clone())
}

fn retain_dispatch_window(
    ctx: &mut util::RunContext,
    window_id: &str,
    reason: &str,
) -> anyhow::Result<()> {
    if let Some(window) = ctx
        .state
        .dispatch_windows
        .iter_mut()
        .rev()
        .find(|window| window.window_id == window_id && window.status == "active")
    {
        window.status = "retained".to_string();
        window.finished_at = Some(crate::state::iso_now());
        window.retention_reason = Some(reason.to_string());
        util::save_run(ctx)?;
    }
    Ok(())
}

fn dispatch_window_name(options: &DispatchGoalOptions, goal_path: &Path, run_id: &str) -> String {
    options
        .window_name
        .clone()
        .unwrap_or_else(|| goal_window_name(&options.runner, goal_path, run_id))
}

fn goal_window_name(runner: &str, goal_path: &Path, run_id: &str) -> String {
    let stem = goal_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mut source = if stem == "goal" {
        ""
    } else {
        stem.strip_prefix("goal-").unwrap_or(stem)
    };
    if is_date_prefix(source.as_bytes()) {
        source = &source[11..];
    }

    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in source.chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_lowercase() || normalized.is_ascii_digit() {
            slug.push(normalized);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    slug.truncate(20);
    let slug = slug.trim_matches('-');
    let fallback = run_id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let slug = if slug.is_empty() {
        if fallback.is_empty() {
            "window"
        } else {
            &fallback
        }
    } else {
        slug
    };
    format!("lto:{runner}:{slug}")
}

fn is_date_prefix(bytes: &[u8]) -> bool {
    bytes.len() >= 11
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'-'
}

pub fn retain_latest_dispatch_window(repo: &Path, run_id: &str, reason: &str) {
    let Ok(mut ctx) = util::load_run(repo, Some(run_id)) else {
        return;
    };
    let window_id = ctx
        .state
        .dispatch_windows
        .iter()
        .rev()
        .find(|window| window.status == "active")
        .map(|window| window.window_id.clone());
    if let Some(window_id) = window_id
        && let Err(err) = retain_dispatch_window(&mut ctx, &window_id, reason)
    {
        eprintln!("warning: could not retain dispatch window {window_id}: {err}");
    }
}

fn blocked_patterns(runner: &str) -> Vec<String> {
    let mut patterns = vec!["Press enter to confirm".to_string()];
    if runner == "codex" {
        patterns.extend([
            "Hooks need review".to_string(),
            "hook needs review".to_string(),
            "Trust all and continue".to_string(),
            "Press t to trust all".to_string(),
        ]);
    }
    patterns
}

fn blocked_prompt_hint(runner: &str) -> String {
    if runner == "codex" {
        "runner codex is blocked on an interactive trust prompt; select \"Trust all and continue\", then exit codex back to the shell"
            .to_string()
    } else {
        format!("runner {runner} is blocked on an interactive prompt")
    }
}

fn completion_wait_command(run_id: &str) -> String {
    format!(
        "lto events --wait --event-type agent.dispatch.completed --run-id {run_id} --timeout {DEFAULT_COMPLETION_WAIT_SEC}"
    )
}

fn dispatch_and_wait_command(options: &DispatchGoalOptions, run_id: &str) -> String {
    format!(
        "lto dispatch-and-wait --runner {} --goal {} --run-id {run_id} --timeout {DEFAULT_COMPLETION_WAIT_SEC}",
        options.runner,
        shell_single_quote(&options.goal.display().to_string())
    )
}

fn run_dispatch(
    repo: &Path,
    ctx: &mut util::RunContext,
    options: &DispatchGoalOptions,
    goal_path: &Path,
    cwd: &Path,
) -> anyhow::Result<GoalDispatchOutcome> {
    let run_id = ctx.run_id.clone();
    let repo_path = absolutize(repo)?;
    let plan = runner_plan(&options.runner, goal_path, &run_id);
    let config = TmuxRunnerConfig {
        mode: TmuxMode::Fire,
        target: options.target.clone(),
        session: options.tmux_session.clone(),
        new_window: options.new_window,
        new_session: false,
        window_name: dispatch_window_name(options, goal_path, &run_id),
        signal_name: "lto-dispatch-goal".to_string(),
        sentinel_path: None,
        ready_patterns: plan.ready_patterns.clone(),
        skip_prompts: default_skip_prompts(),
        dispatch_safety: TmuxDispatchSafety {
            blocked_patterns: blocked_patterns(&options.runner),
            blocked_prompt_hint: Some(blocked_prompt_hint(&options.runner)),
            reject_busy_target: true,
        },
        ready_timeout: Duration::from_secs(
            options
                .ready_timeout_sec
                .unwrap_or(DEFAULT_DISPATCH_READY_TIMEOUT_SEC),
        ),
        poll_interval: Duration::from_millis(500),
        capture_lines: 80,
        tmux_bin: options
            .tmux_bin
            .clone()
            .unwrap_or_else(|| "tmux".to_string()),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut created_window_id = None;
    let result = runtime.block_on(async {
        let target = tmux_runner::prepare_dispatch_target(&config).await?;
        let window_id = dispatch_window_id(ctx, options, &target);
        if let Some(window_id) = window_id.as_deref() {
            persist_dispatch_window(ctx, options, &config, &target, window_id).map_err(|err| {
                tmux_runner::TmuxRunnerError::Io(format!(
                    "persist dispatch window {window_id}: {err}"
                ))
            })?;
            created_window_id = Some(window_id.to_string());
            tmux_runner::send_dispatch_text(
                &config,
                &target,
                &format!("export LTO_WINDOW_ID={}", shell_single_quote(window_id)),
            )
            .await?;
        }
        tmux_runner::send_dispatch_text(
            &config,
            &target,
            &format!(
                "export LTO_REPO={}",
                shell_single_quote(&repo_path.display().to_string())
            ),
        )
        .await?;
        tmux_runner::send_dispatch_text(
            &config,
            &target,
            &format!("cd {}", shell_single_quote(&cwd.display().to_string())),
        )
        .await?;
        if let Some(launch) = &plan.launch {
            tmux_runner::send_dispatch_text(&config, &target, launch).await?;
            tmux_runner::wait_for_dispatch_ready(&config, &target).await?;
        }
        // When the launch line already carries the prompt (agy -i '<prompt>'),
        // the prompt is submitted at startup — do NOT probe or re-send it, or
        // it would be entered twice. codex/pi start a REPL first, so they
        // probe then send the prompt on a separate line.
        if !plan.launch_includes_prompt {
            if plan.needs_probe {
                let probe = format!("LTO_PROBE_{}", now_millis());
                let _ = tmux_runner::confirm_tui_input(&config, &target, &probe).await?;
            }
            tmux_runner::send_dispatch_text(&config, &target, &plan.prompt).await?;
        }
        let capture =
            tmux_runner::wait_for_capture_patterns(&config, &target, &plan.confirm_patterns)
                .await?;
        Ok::<_, tmux_runner::TmuxRunnerError>(GoalDispatchOutcome {
            target,
            window_id,
            capture,
            repo: cwd.display().to_string(),
            completion_event: plan.completion_event,
            completion_mode: plan.completion_mode,
        })
    });
    result.map_err(|err| {
        if let Some(window_id) = created_window_id.as_deref() {
            let reason = format!("dispatch failed: {err}");
            if let Err(save_err) = retain_dispatch_window(ctx, window_id, &reason) {
                eprintln!("warning: could not persist retained window {window_id}: {save_err}");
            }
            eprintln!("dispatch failed; window {window_id} retained for troubleshooting");
        }
        anyhow!("dispatch-goal tmux failure in {}: {err}", repo.display())
    })
}

#[derive(Debug, Clone)]
struct GoalDispatchOutcome {
    target: String,
    window_id: Option<String>,
    capture: String,
    repo: String,
    completion_event: Option<String>,
    completion_mode: String,
}

fn runner_plan(runner: &str, goal_path: &Path, run_id: &str) -> GoalRunnerPlan {
    let goal = goal_path.display().to_string();
    match runner {
        "codex" => GoalRunnerPlan {
            launch: Some(format!("LTO_RUN_ID={} codex", shell_single_quote(run_id))),
            prompt: format!("/goal {goal}"),
            ready_patterns: vec!["gpt-".to_string()],
            confirm_patterns: vec!["Pursuing goal".to_string(), "Working".to_string()],
            needs_probe: true,
            completion_event: Some("agent.dispatch.completed".to_string()),
            completion_mode: "codex-goal-state-hook".to_string(),
            // codex starts a REPL, then takes /goal on a later line.
            launch_includes_prompt: false,
        },
        "pi" => {
            let launch = process_exit_wrapper(
                &format!(
                    "LTO_RUN_ID={} pi --no-skills --no-context-files --no-extensions",
                    shell_single_quote(run_id)
                ),
                "pi",
                run_id,
            );
            GoalRunnerPlan {
                launch: Some(launch),
                prompt: goal_prompt(&goal),
                ready_patterns: vec!["deepseek".to_string(), "ctx".to_string()],
                confirm_patterns: vec!["Working".to_string()],
                needs_probe: true,
                completion_event: Some("agent.dispatch.completed".to_string()),
                completion_mode: "pi-process-exit".to_string(),
                // pi starts a REPL, then takes the prompt on a later line.
                launch_includes_prompt: false,
            }
        }
        "agy" => {
            // agy `-i`/--prompt-interactive runs a prompt in a real TUI session
            // and continues — it actually executes (`--print` only prints a plan
            // without executing: false-success trap, bug #5/#6).
            //
            // `-i` is a value flag, so a bare `agy -i` errors "flag needs an
            // argument". Earlier we baked the full goal prompt into the launch
            // command (`agy -i '<prompt>'`), but that prompt is ~1000+ chars and
            // gets TRUNCATED by tmux paste / terminal line limits, so agy never
            // saw the whole thing. Instead we start agy with an EMPTY placeholder
            // (`agy -i ''`) just to satisfy the flag, then paste the real prompt
            // into the TUI input box on a later line — exactly like codex/pi.
            //
            // The placeholder MUST be empty, not a word like "start": agy treats
            // any non-empty initial prompt as a real instruction and immediately
            // starts exploring the workspace, which races with (and corrupts) the
            // real prompt we send next. An empty value leaves agy idle at the
            // input box, so the real prompt is its first and only instruction.
            GoalRunnerPlan {
                launch: Some(process_exit_wrapper(
                    &format!(
                        "LTO_RUN_ID={} agy -i {}",
                        shell_single_quote(run_id),
                        shell_single_quote("")
                    ),
                    "agy",
                    run_id,
                )),
                prompt: goal_prompt(&goal),
                // Readiness must key on a marker that appears ONLY once agy's TUI
                // input box is live — NOT "agy", which the launch command echoes
                // (`agy -i ''`) while the shell is still running, causing a false
                // ready that sends the real prompt into the shell (prompt lost).
                // agy's idle input box shows "? for shortcuts"; wait for that.
                ready_patterns: vec!["? for shortcuts".to_string()],
                confirm_patterns: vec!["Working".to_string(), "Read the file".to_string()],
                needs_probe: true,
                completion_event: Some("agent.dispatch.completed".to_string()),
                completion_mode: "agy-process-exit".to_string(),
                launch_includes_prompt: false,
            }
        }
        _ => unreachable!("runner validated"),
    }
}

fn process_exit_wrapper(launch: &str, runner: &str, run_id: &str) -> String {
    format!(
        "{launch}; LTO_AGENT_RC=$?; {} --repo \"$LTO_REPO\" agent-turn-completed --run-id {} --runner {} --source {}-process-exit --rc \"$LTO_AGENT_RC\" --window-id \"$LTO_WINDOW_ID\" --bell",
        shell_single_quote(&current_lto_bin()),
        shell_single_quote(run_id),
        shell_single_quote(runner),
        runner,
    )
}

fn goal_prompt(goal: &str) -> String {
    format!(
        "Read the file {goal} and execute it. Follow only the instructions in that goal file. {GOAL_CONSTRAINT_SUMMARY}"
    )
}

fn write_dispatch_record(
    run_dir: &Path,
    options: &DispatchGoalOptions,
    goal_path: &Path,
    cwd: &Path,
    outcome: &GoalDispatchOutcome,
    hook_status: &HookStatus,
) -> anyhow::Result<PathBuf> {
    let dispatch_dir = run_dir.join("dispatch");
    fs::create_dir_all(&dispatch_dir)?;
    let path = dispatch_dir.join(format!("{}-{}.json", options.runner, now_millis()));
    let record = json!({
        "schema_version": 1,
        "runner": options.runner,
        "goal": goal_path.display().to_string(),
        "cwd": cwd.display().to_string(),
        "target": outcome.target,
        "window_id": outcome.window_id,
        "cleanup_on_success": !options.keep_window,
        "repo": outcome.repo,
        "dispatched_at": crate::state::iso_now(),
        "completion_event": outcome.completion_event,
        "completion_mode": outcome.completion_mode,
        "turns_jsonl": false,
        "hook_status": {
            "status": hook_status.status,
            "detail": hook_status.detail,
            "script_path": hook_status.script_path.as_ref().map(|path| path.display().to_string()),
        },
        "capture_excerpt": outcome.capture.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
    });
    fs::write(&path, serde_json::to_string_pretty(&record)? + "\n")?;
    Ok(path)
}

/// Get a mutable handle to `hooks.SessionEnd` in a gemini settings object,
/// creating the `hooks` object and `SessionEnd` array if absent. Never touches
/// other keys, so the user's settings are preserved.
fn session_end_hooks_mut(value: &mut Value) -> anyhow::Result<&mut Vec<Value>> {
    if !value.is_object() {
        anyhow::bail!("gemini settings.json root is not an object");
    }
    let object = value.as_object_mut().expect("settings object");
    object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = object
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("gemini settings 'hooks' is not an object"))?;
    hooks.entry("SessionEnd").or_insert_with(|| json!([]));
    hooks
        .get_mut("SessionEnd")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("hooks.SessionEnd is not an array"))
}

fn uninstall_agy_hook(repo: &Path) -> anyhow::Result<HookStatus> {
    let _ = repo;
    let Some(gemini_home) = gemini_home() else {
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: "HOME is not set".to_string(),
            script_path: None,
        });
    };
    uninstall_agy_hook_at(&gemini_home)
}

fn uninstall_agy_hook_at(gemini_home: &Path) -> anyhow::Result<HookStatus> {
    let settings_path = gemini_home.join("settings.json");
    let script_path = gemini_home
        .join("hooks")
        .join("lto-agy-session-end-notify.sh");
    if !settings_path.exists() {
        let _ = fs::remove_file(&script_path);
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: format!("{} does not exist", settings_path.display()),
            script_path: Some(script_path),
        });
    }
    let mut value = serde_json::from_str::<Value>(&fs::read_to_string(&settings_path)?)?;
    let removed = {
        let hooks = session_end_hooks_mut(&mut value)?;
        let before = hooks.len();
        hooks.retain(|entry| {
            entry.get("_lto_marker").and_then(Value::as_str) != Some(LTO_HOOK_MARKER)
                && !entry.to_string().contains("lto-agy-session-end-notify.sh")
        });
        before.saturating_sub(hooks.len())
    };
    let script_existed = script_path.exists();
    if removed == 0 && !script_existed {
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: "process-exit completion already has no global agy hook".to_string(),
            script_path: Some(script_path),
        });
    }
    if removed > 0 {
        backup_hooks(&settings_path)?;
        fs::write(&settings_path, serde_json::to_string_pretty(&value)? + "\n")?;
    }
    if script_existed {
        let _ = fs::remove_file(&script_path);
    }
    Ok(HookStatus {
        status: "uninstalled".to_string(),
        detail: format!("removed {removed} hook group(s)"),
        script_path: Some(script_path),
    })
}

fn gemini_home() -> Option<PathBuf> {
    std::env::var_os("LTO_GEMINI_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".gemini")))
}

fn install_codex_hook(repo: &Path) -> anyhow::Result<HookStatus> {
    let Some(codex_home) = codex_home() else {
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: "HOME is not set".to_string(),
            script_path: None,
        });
    };
    install_codex_hook_at(repo, &codex_home)
}

fn install_codex_hook_at(repo: &Path, codex_home: &Path) -> anyhow::Result<HookStatus> {
    let _ = repo;
    if !codex_home.exists() {
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: format!("{} does not exist", codex_home.display()),
            script_path: None,
        });
    }
    let hooks_dir = codex_home.join("hooks");
    fs::create_dir_all(&hooks_dir)?;
    let script_path = hooks_dir.join("lto-codex-stop-notify.sh");
    fs::write(&script_path, CODEX_STOP_HOOK)?;
    set_executable(&script_path)?;

    let hooks_path = codex_home.join("hooks.json");
    let mut value = if hooks_path.exists() {
        let text = fs::read_to_string(&hooks_path)?;
        serde_json::from_str::<Value>(&text)
            .with_context(|| format!("parse {}", hooks_path.display()))?
    } else {
        json!({"hooks": {}})
    };
    ensure_hooks_shape(&mut value)?;
    let lto_bin = current_lto_bin();
    let command = format!(
        "LTO_BIN={} bash {}",
        shell_single_quote(&lto_bin),
        shell_single_quote(&script_path.display().to_string())
    );
    if let Some(existing) = lto_stop_hook_mut(&mut value)? {
        if hook_command(existing).as_deref() == Some(command.as_str()) {
            return Ok(HookStatus {
                status: "already-installed".to_string(),
                detail: hooks_path.display().to_string(),
                script_path: Some(script_path),
            });
        }
        backup_hooks(&hooks_path)?;
        *existing = codex_stop_hook_entry(&command, &lto_bin);
        fs::write(&hooks_path, serde_json::to_string_pretty(&value)? + "\n")?;
        return Ok(HookStatus {
            status: "updated".to_string(),
            detail: hooks_path.display().to_string(),
            script_path: Some(script_path),
        });
    }
    backup_hooks(&hooks_path)?;
    stop_hooks_mut(&mut value)?.push(codex_stop_hook_entry(&command, &lto_bin));
    fs::write(&hooks_path, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(HookStatus {
        status: "installed".to_string(),
        detail: hooks_path.display().to_string(),
        script_path: Some(script_path),
    })
}

fn codex_stop_hook_entry(command: &str, lto_bin: &str) -> Value {
    json!({
        "matcher": "",
        "_lto_marker": LTO_HOOK_MARKER,
        "_lto_bin": lto_bin,
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 10
        }]
    })
}

fn lto_stop_hook_mut(value: &mut Value) -> anyhow::Result<Option<&mut Value>> {
    Ok(stop_hooks_mut(value)?
        .iter_mut()
        .find(|entry| entry.get("_lto_marker").and_then(Value::as_str) == Some(LTO_HOOK_MARKER)))
}

fn hook_command(entry: &Value) -> Option<String> {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .and_then(|hooks| hooks.first())
        .and_then(|hook| hook.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn uninstall_codex_hook(repo: &Path) -> anyhow::Result<HookStatus> {
    let _ = repo;
    let Some(codex_home) = codex_home() else {
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: "HOME is not set".to_string(),
            script_path: None,
        });
    };
    let hooks_path = codex_home.join("hooks.json");
    let script_path = codex_home.join("hooks").join("lto-codex-stop-notify.sh");
    if !hooks_path.exists() {
        let _ = fs::remove_file(&script_path);
        return Ok(HookStatus {
            status: "skipped".to_string(),
            detail: format!("{} does not exist", hooks_path.display()),
            script_path: Some(script_path),
        });
    }
    let mut value = serde_json::from_str::<Value>(&fs::read_to_string(&hooks_path)?)?;
    let removed = {
        let hooks = stop_hooks_mut(&mut value)?;
        let before = hooks.len();
        hooks.retain(|entry| {
            entry.get("_lto_marker").and_then(Value::as_str) != Some(LTO_HOOK_MARKER)
                && !entry.to_string().contains("lto-codex-stop-notify.sh")
        });
        before.saturating_sub(hooks.len())
    };
    backup_hooks(&hooks_path)?;
    fs::write(&hooks_path, serde_json::to_string_pretty(&value)? + "\n")?;
    let _ = fs::remove_file(&script_path);
    Ok(HookStatus {
        status: "uninstalled".to_string(),
        detail: format!("removed {removed} hook group(s)"),
        script_path: Some(script_path),
    })
}

fn ensure_hooks_shape(value: &mut Value) -> anyhow::Result<()> {
    if !value.is_object() {
        *value = json!({"hooks": {}});
    }
    let object = value.as_object_mut().expect("value object");
    object.entry("hooks").or_insert_with(|| json!({}));
    if !object.get("hooks").is_some_and(Value::is_object) {
        anyhow::bail!("hooks.json field 'hooks' is not an object");
    }
    Ok(())
}

fn stop_hooks_mut(value: &mut Value) -> anyhow::Result<&mut Vec<Value>> {
    ensure_hooks_shape(value)?;
    let hooks = value
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .expect("hooks object");
    hooks.entry("Stop").or_insert_with(|| json!([]));
    hooks
        .get_mut("Stop")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("hooks.Stop is not an array"))
}

fn backup_hooks(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        let backup = path.with_extension(format!("json.lto-bak-{}", now_millis()));
        fs::copy(path, backup)?;
    }
    Ok(())
}

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("LTO_CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

fn default_skip_prompts() -> Vec<SkipPrompt> {
    vec![
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
        SkipPrompt {
            // agy (Gemini CLI) prompts this on first entry to a new project.
            // The default selection is "Yes, I trust this folder", so Enter
            // confirms it and lets dispatch proceed unattended.
            pattern: "Do you trust the contents".to_string(),
            key: "Enter".to_string(),
        },
        SkipPrompt {
            pattern: "esc to close".to_string(),
            key: "Escape".to_string(),
        },
        SkipPrompt {
            pattern: "Press enter to view hooks".to_string(),
            key: "Escape".to_string(),
        },
    ]
}

fn validate_runner(runner: &str) -> anyhow::Result<()> {
    match runner {
        "codex" | "pi" | "agy" => Ok(()),
        _ => anyhow::bail!("dispatch-goal runner must be one of codex, pi, agy"),
    }
}

/// `--target` and `--new-window` are mutually exclusive, but neither is
/// required. With both unset, `prepare_target` auto-detects the attached
/// tmux session and opens a visible window there — the "try hard to use
/// tmux" default so a host that forgets the flag still lands in the current
/// cc session instead of bailing or spawning a detached session it can't see.
fn validate_dispatch_target(target: Option<&str>, new_window: bool) -> anyhow::Result<()> {
    if target.is_some() && new_window {
        anyhow::bail!("pass at most one of --target or --new-window");
    }
    Ok(())
}

fn current_lto_bin() -> String {
    std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "lto".to_string())
}

fn set_executable(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn absolutize(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_options(goal: &Path) -> DispatchGoalOptions {
        DispatchGoalOptions {
            run_id: Some("r1".to_string()),
            runner: "codex".to_string(),
            goal: goal.to_path_buf(),
            target: None,
            new_window: false,
            window_name: None,
            keep_window: false,
            cwd: None,
            tmux_session: None,
            tmux_bin: None,
            ready_timeout_sec: None,
            notify_cmd: None,
            no_install_hooks: false,
            uninstall_hooks: false,
        }
    }

    #[test]
    fn persists_notify_cmd_in_run_state() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        let state_path = run_dir.join("state.json");
        crate::state::save_state(
            &state_path,
            &crate::state::LtoState {
                run_id: "r1".to_string(),
                ..crate::state::LtoState::default()
            },
        )
        .unwrap();
        let mut ctx = util::RunContext {
            run_id: "r1".to_string(),
            run_dir,
            state_path: state_path.clone(),
            state: crate::state::load_state(&state_path).unwrap(),
        };

        persist_notify_cmd(&mut ctx, Some("notify $LTO_SUMMARY")).unwrap();

        let persisted = crate::state::load_state(state_path).unwrap();
        assert_eq!(persisted.notify_cmd.as_deref(), Some("notify $LTO_SUMMARY"));
    }

    #[test]
    fn wait_timeout_marks_latest_window_retained() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        crate::state::save_state(
            run_dir.join("state.json"),
            &crate::state::LtoState {
                run_id: "r1".to_string(),
                dispatch_windows: vec![DispatchWindowState {
                    window_id: "@9".to_string(),
                    target: "@9.0".to_string(),
                    runner: "codex".to_string(),
                    tmux_bin: "tmux".to_string(),
                    cleanup_on_success: true,
                    status: "active".to_string(),
                    created_at: crate::state::iso_now(),
                    finished_at: None,
                    retention_reason: None,
                }],
                ..crate::state::LtoState::default()
            },
        )
        .unwrap();

        retain_latest_dispatch_window(tmp.path(), "r1", "wait timeout");

        let persisted = crate::state::load_state(run_dir.join("state.json")).unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "retained");
        assert_eq!(
            persisted.dispatch_windows[0].retention_reason.as_deref(),
            Some("wait timeout")
        );
    }

    #[test]
    fn goal_window_name_is_stable_and_human_readable() {
        assert_eq!(
            goal_window_name(
                "codex",
                Path::new("goal-2026-07-10-invocation-ux.md"),
                "run-12345678",
            ),
            "lto:codex:invocation-ux"
        );
        assert_eq!(
            goal_window_name(
                "codex",
                Path::new("goal-2026-07-10-phase3-tmux-window.md"),
                "run-12345678",
            ),
            "lto:codex:phase3-tmux-window"
        );
        assert_eq!(
            goal_window_name("codex", Path::new("goal.md"), "run-12345678"),
            "lto:codex:12345678"
        );
        assert_eq!(
            goal_window_name(
                "codex",
                Path::new("goal-2026-07-10-Foo___Bar!!.md"),
                "run-12345678",
            ),
            "lto:codex:foo-bar"
        );
        assert_eq!(
            goal_window_name(
                "codex",
                Path::new("goal-2026-07-10-中文 Foo.md"),
                "run-12345678",
            ),
            "lto:codex:foo"
        );
    }

    #[test]
    fn goal_window_name_truncates_slug_without_trailing_dash() {
        let name = goal_window_name(
            "agy",
            Path::new("goal-2026-07-10-abcdefghijklmnopqrs-more.md"),
            "run-12345678",
        );
        let slug = name.rsplit(':').next().unwrap();
        assert!(slug.len() <= 20);
        assert!(!slug.ends_with('-'));
        assert_eq!(name, "lto:agy:abcdefghijklmnopqrs");
    }

    #[test]
    fn explicit_window_name_is_preserved_verbatim() {
        let mut options = test_options(Path::new("goal-2026-07-10-x.md"));
        options.window_name = Some("Host Chosen / Name".to_string());
        assert_eq!(
            dispatch_window_name(&options, &options.goal, "run-12345678"),
            "Host Chosen / Name"
        );
    }

    #[test]
    fn dispatch_ready_defaults_and_blocked_patterns_match_contract() {
        assert_eq!(DEFAULT_DISPATCH_READY_TIMEOUT_SEC, 60);
        let patterns = blocked_patterns("codex");
        assert!(patterns.iter().any(|item| item == "Hooks need review"));
        assert!(patterns.iter().any(|item| item == "Trust all and continue"));
        assert!(patterns.iter().any(|item| item == "Press enter to confirm"));
    }

    #[test]
    fn completion_commands_are_ready_to_copy() {
        let options = test_options(Path::new("goal with space.md"));
        assert_eq!(
            completion_wait_command("r1"),
            "lto events --wait --event-type agent.dispatch.completed --run-id r1 --timeout 600"
        );
        assert_eq!(
            dispatch_and_wait_command(&options, "r1"),
            "lto dispatch-and-wait --runner codex --goal 'goal with space.md' --run-id r1 --timeout 600"
        );
    }

    #[test]
    fn all_completion_hooks_request_a_bell() {
        assert!(CODEX_STOP_HOOK.contains("--source codex-stop-hook --bell"));
        assert!(!CODEX_STOP_HOOK.contains("--rc 0"));
        assert!(CODEX_STOP_HOOK.contains("--window-id"));
        for runner in ["pi", "agy"] {
            let plan = runner_plan(runner, Path::new("/tmp/goal.md"), "r1");
            let launch = plan.launch.as_deref().unwrap();
            assert!(launch.contains("--repo \"$LTO_REPO\""));
            assert!(launch.contains("--rc \"$LTO_AGENT_RC\""));
            assert!(launch.contains("--window-id \"$LTO_WINDOW_ID\""));
            assert!(launch.contains("--bell"));
        }
    }

    #[test]
    fn dispatch_target_defaults_to_auto_detect() {
        // No flags: must NOT bail — prepare_target auto-detects the attached
        // tmux session (the "try hard to use tmux" default).
        assert!(validate_dispatch_target(None, false).is_ok());
        // Either flag alone is fine.
        assert!(validate_dispatch_target(Some("sess:1.0"), false).is_ok());
        assert!(validate_dispatch_target(None, true).is_ok());
        // Both together is the only error.
        assert!(validate_dispatch_target(Some("sess:1.0"), true).is_err());
    }

    #[test]
    fn runner_plan_uses_required_entrypoints() {
        let goal = Path::new("/tmp/goal.md");
        assert_eq!(
            runner_plan("codex", goal, "r1").prompt,
            "/goal /tmp/goal.md"
        );
        assert_eq!(
            runner_plan("codex", goal, "r1").ready_patterns,
            vec!["gpt-".to_string()]
        );
        assert_eq!(
            runner_plan("codex", goal, "r1").completion_event.as_deref(),
            Some("agent.dispatch.completed")
        );
        assert!(
            runner_plan("pi", goal, "r1")
                .launch
                .as_deref()
                .unwrap()
                .starts_with("LTO_RUN_ID='r1' pi --no-skills")
        );
        assert!(
            !runner_plan("pi", goal, "r1")
                .launch
                .as_deref()
                .unwrap()
                .contains("--print")
        );
        assert!(
            runner_plan("pi", goal, "r1")
                .prompt
                .starts_with("Read the file ")
        );
        assert!(
            runner_plan("pi", goal, "r1")
                .prompt
                .contains(GOAL_CONSTRAINT_SUMMARY)
        );
        assert!(runner_plan("pi", goal, "r1").needs_probe);
        let pi_plan = runner_plan("pi", goal, "r1");
        let pi_launch = pi_plan.launch.as_deref().unwrap();
        assert!(!pi_launch.contains(" -e "));
        assert!(pi_launch.contains("pi-process-exit"));
        assert_eq!(
            pi_plan.completion_event.as_deref(),
            Some("agent.dispatch.completed")
        );
        assert_eq!(pi_plan.completion_mode, "pi-process-exit");
        // agy must use the interactive entrypoint (`agy -i`), not `--print`
        // which only prints a plan without executing (bug #5/#6).
        assert!(
            runner_plan("agy", goal, "r1")
                .launch
                .as_deref()
                .unwrap()
                .starts_with("LTO_RUN_ID='r1' agy -i")
        );
        assert!(!runner_plan("agy", goal, "r1").prompt.contains("--print"));
        assert!(
            runner_plan("agy", goal, "r1")
                .prompt
                .contains("Use LTO discipline")
        );
        assert!(runner_plan("agy", goal, "r1").needs_probe);

        // Regression — two bugs must both stay fixed:
        //  v0.8.0: a bare `agy -i` errors "flag needs an argument" and never
        //          starts, so `-i` must still be followed by a value.
        //  v0.9.0: baking the ~1000-char goal prompt into `agy -i '<prompt>'`
        //          got truncated by tmux paste/terminal limits. So the launch
        //          carries an EMPTY placeholder and the real prompt is sent later
        //          into the TUI (launch_includes_prompt = false), like codex/pi.
        //  v0.9.1-followup: the placeholder MUST be empty ('') — a word like
        //          "start" makes agy treat it as a real instruction and explore
        //          the workspace, racing with/corrupting the real prompt.
        let agy = runner_plan("agy", goal, "r1");
        let launch = agy.launch.as_deref().unwrap();
        // `-i` is followed by a value (bug #1 stays fixed)...
        assert!(
            launch.contains("agy -i '"),
            "agy -i must still carry a value, got: {launch}"
        );
        // ...and that value is EMPTY, not a word that agy would act on, and NOT
        // the long goal prompt (bugs #2 + #3 stay fixed).
        assert!(
            launch.contains("agy -i ''"),
            "agy launch must use an EMPTY placeholder ('') so agy stays idle, got: {launch}"
        );
        assert!(
            !launch.contains("Read the file"),
            "the long goal prompt must NOT be in the launch line (truncation risk): {launch}"
        );
        assert!(
            launch.split(';').next().unwrap().len() < 120,
            "agy agent launch must stay short: {launch}"
        );
        assert!(launch.contains("agy-process-exit"));
        // agy takes the real prompt on a later line, like codex/pi.
        assert!(!agy.launch_includes_prompt);
        assert!(!runner_plan("codex", goal, "r1").launch_includes_prompt);
        assert!(!runner_plan("pi", goal, "r1").launch_includes_prompt);
        // The real prompt still exists (sent later), and it's the full goal prompt.
        assert!(agy.prompt.contains("Read the file"));
    }

    #[test]
    fn agy_hook_uninstall_removes_only_lto_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let gemini = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        fs::write(
            gemini.join("settings.json"),
            r#"{"hooks":{"SessionEnd":[{"matcher":"*","hooks":[{"type":"command","command":"echo user"}]},{"_lto_marker":"long-task-orchestration","hooks":[{"type":"command","command":"old lto-agy-session-end-notify.sh"}]}]}}"#,
        )
        .unwrap();
        let status = uninstall_agy_hook_at(&gemini).unwrap();
        assert_eq!(status.status, "uninstalled");
        let value: Value =
            serde_json::from_str(&fs::read_to_string(gemini.join("settings.json")).unwrap())
                .unwrap();
        let se = value["hooks"]["SessionEnd"].as_array().unwrap();
        // Only the user's hook remains.
        assert_eq!(se.len(), 1);
        assert!(se[0].to_string().contains("echo user"));
        assert!(!se[0].to_string().contains(LTO_HOOK_MARKER));
    }

    #[test]
    fn pi_dispatch_confirmation_does_not_reuse_ready_text() {
        let goal = Path::new("/tmp/goal.md");
        let plan = runner_plan("pi", goal, "r1");

        assert!(
            plan.confirm_patterns
                .iter()
                .any(|pattern| pattern == "Working")
        );
        for ready in &plan.ready_patterns {
            assert!(
                !plan
                    .confirm_patterns
                    .iter()
                    .any(|confirm| confirm.eq_ignore_ascii_case(ready)),
                "pi confirm pattern must not accept ready text `{ready}`"
            );
        }
    }

    #[test]
    fn hook_install_preserves_existing_stop_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join(".codex");
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            codex.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
        )
        .unwrap();
        let status = install_codex_hook_at(tmp.path(), &codex).unwrap();
        assert_eq!(status.status, "installed");
        let value: Value =
            serde_json::from_str(&fs::read_to_string(codex.join("hooks.json")).unwrap()).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop[0].to_string().contains("echo keep"));
        assert!(stop[1].to_string().contains(LTO_HOOK_MARKER));
    }

    #[test]
    fn hook_install_is_repo_neutral_across_dispatches() {
        let tmp = tempfile::tempdir().unwrap();
        let codex = tmp.path().join(".codex");
        fs::create_dir_all(&codex).unwrap();
        fs::write(
            codex.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"matcher":"","_lto_marker":"long-task-orchestration","hooks":[{"type":"command","command":"LTO_REPO='/old' LTO_BIN='old' bash '/old-hook'","timeout":10}]}]}}"#,
        )
        .unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let status = install_codex_hook_at(&repo, &codex).unwrap();
        assert_eq!(status.status, "updated");
        let value: Value =
            serde_json::from_str(&fs::read_to_string(codex.join("hooks.json")).unwrap()).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert!(stop[0].get("_lto_repo").is_none());
        let command = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(!command.contains("LTO_REPO_FALLBACK="));
        assert!(!command.contains("LTO_REPO="));
        assert!(!command.contains(repo.to_str().unwrap()));

        let other_repo = tmp.path().join("other-repo");
        fs::create_dir_all(&other_repo).unwrap();
        let again = install_codex_hook_at(&other_repo, &codex).unwrap();
        assert_eq!(again.status, "already-installed");
    }
}
