use crate::commands::util;
use crate::events::{self, EventRecord};
use crate::process::shell_single_quote;
use crate::state::{self, DispatchWindowState};
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
/// Prompt hard cap: long constraints live in the goal file, not the paste line.
const GOAL_PROMPT_MAX_CHARS: usize = 500;

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
    /// Skip per-runner behavioral-constraints injection (built-in codex block
    /// and `$LTO_CONSTRAINTS_DIR` / `~/.config/lto/constraints/<runner>.md`).
    pub no_runner_constraints: bool,
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

/// Dispatch a goal and block until the agent reports completion.
///
/// Lives here rather than in the CLI arm because the waiting half is dispatch
/// semantics, not argument parsing: it interprets the completion event's rc,
/// and on timeout it retains the tmux window via
/// [`retain_latest_dispatch_window`] so the run can be inspected.
pub fn cmd_dispatch_and_wait(
    repo: &Path,
    options: DispatchGoalOptions,
    timeout_sec: u64,
) -> anyhow::Result<()> {
    let run_id = util::resolve_run_id(repo, options.run_id.as_deref())
        .context("dispatch-and-wait requires --run-id or .lto/current")?;
    cmd_dispatch_goal(
        repo,
        DispatchGoalOptions {
            run_id: Some(run_id.clone()),
            ..options
        },
    )?;
    println!("\nwaiting up to {timeout_sec}s for agent.dispatch.completed on run {run_id} ...");
    let Some(event) = events::wait_for(
        repo,
        &run_id,
        "agent.dispatch.completed",
        None,
        Duration::from_secs(timeout_sec),
    )?
    else {
        retain_latest_dispatch_window(
            repo,
            &run_id,
            &format!("dispatch-and-wait timeout after {timeout_sec}s"),
        );
        anyhow::bail!(
            "TIMEOUT after {timeout_sec}s; the agent may still be running and its window was retained. Check with `lto events --wait --run-id {run_id}` or `tmux` directly."
        );
    };

    let summary = event
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("(no summary)");
    let fields = event.get("fields");
    let runner = fields
        .and_then(|f| f.get("runner"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let rc = fields
        .and_then(|f| f.get("rc"))
        .and_then(|value| value.as_i64());
    if rc != Some(0) {
        anyhow::bail!(
            "dispatch completed without success (runner={runner}, rc={}); window retained for troubleshooting",
            rc.map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
    println!("DONE runner={runner} rc=0 summary={summary}");
    println!(
        "Register the reply as evidence with: lto collect-agent-run --run-id {run_id} --runner {runner} --reply <path>"
    );
    Ok(())
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
    validate_dispatch_cwd(options.cwd.as_deref())?;
    validate_dispatch_target(options.target.as_deref(), options.new_window)?;

    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    persist_notify_cmd(&mut ctx, options.notify_cmd.as_deref())?;
    let goal_path = absolutize(&options.goal)?;
    let constraints = if options.no_runner_constraints {
        None
    } else {
        runner_constraints(&options.runner)
    };
    let goal_path = materialize_goal_with_completion_protocol(
        &goal_path,
        repo,
        &ctx.run_id,
        &options.runner,
        constraints.as_deref(),
    )?;
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
            // Completion is self-reported via goal_prompt() (source=goal-self-report).
            // Codex Stop hook is an optional side-channel only: if goal-runtime is
            // installed, update_goal complete can still promote a turn. pi/agy keep
            // process-exit wrappers as a secondary signal when the REPL actually exits.
            "codex" => install_codex_hook(repo).unwrap_or_else(degraded),
            "agy" => uninstall_agy_hook(repo).unwrap_or_else(degraded),
            // aix inverts the usual arrangement: process-exit IS the primary
            // signal (its rc is 0 only on StopReason::TaskDone) and there is no
            // self-report at all, so saying otherwise here would mislead anyone
            // debugging a missing completion.
            "aix" => HookStatus {
                status: "skipped".to_string(),
                detail: "completion is process-exit (rc); aix has no self-report session"
                    .to_string(),
                script_path: None,
            },
            _ => HookStatus {
                status: "skipped".to_string(),
                detail: "primary completion is goal-self-report; process-exit is a side-channel"
                    .to_string(),
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
    if let Some(window_id) = window_id {
        match retain_dispatch_window(&mut ctx, &window_id, reason) {
            Ok(()) => eprintln!("window {window_id} retained: {reason}"),
            Err(err) => {
                eprintln!("window {window_id} retained: {reason}; state update failed: {err}")
            }
        }
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
    // ready_patterns / launch don't depend on window_id; prompt is rebuilt once
    // the real window_id is known so self-report can inline it (no env inheritance).
    let mut plan = runner_plan(&options.runner, goal_path, &repo_path, &run_id, "pending");
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
        // Inline the real window_id into the self-report command (preference ②).
        plan.prompt = goal_prompt(
            &goal_path.display().to_string(),
            &repo_path.display().to_string(),
            &run_id,
            &options.runner,
            window_id.as_deref().unwrap_or("unknown"),
        );
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

fn runner_plan(
    runner: &str,
    goal_path: &Path,
    repo: &Path,
    run_id: &str,
    window_id: &str,
) -> GoalRunnerPlan {
    let goal = goal_path.display().to_string();
    let prompt = goal_prompt(
        &goal,
        &repo.display().to_string(),
        run_id,
        runner,
        window_id,
    );
    match runner {
        "codex" => GoalRunnerPlan {
            // Isolate from the user's global MCP servers (2026-07-15: bare
            // `codex` hung on "Starting MCP servers" chrome-devtools for minutes).
            // `-c 'mcp_servers={}'` overrides ~/.codex/config.toml [mcp_servers] to
            // an empty table — same isolation intent as pi's --no-skills/--no-extensions.
            launch: Some(format!(
                "LTO_RUN_ID={} codex -c {}",
                shell_single_quote(run_id),
                shell_single_quote("mcp_servers={}")
            )),
            // Prompt carries a `/goal` prefix (applied in goal_prompt): it
            // engages codex's built-in goal-runtime (thread_goals + continuation
            // in ~/.codex/goals_1.sqlite), which pulls the thread back to the
            // objective until it is marked complete. A bare text prompt is a
            // single turn with no such pull. Verified 2026-08-04: tmux
            // load-buffer/paste-buffer + Enter triggers the command
            // ("Goal active Objective: …", status bar "Pursuing goal").
            prompt,
            // Idle TUI after optional update prompt is dismissed (2026-07-15 probe).
            ready_patterns: vec![
                "gpt-".to_string(),
                "model:".to_string(),
                "codex>".to_string(),
            ],
            // "Goal active" echoes once goal-runtime accepts the objective.
            confirm_patterns: vec![
                "Goal active".to_string(),
                "Working".to_string(),
                "Read the file".to_string(),
            ],
            needs_probe: true,
            completion_event: Some("agent.dispatch.completed".to_string()),
            completion_mode: "goal-self-report".to_string(),
            // codex starts a REPL, then takes the prompt on a later line.
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
                // Deliberately NO `/goal` prefix for pi (goal_prompt leaves it
                // bare). pi has no goal runtime, and its prompt-template
                // expansion re-joins $ARGUMENTS after shell-like splitting,
                // which STRIPS single quotes (verified 2026-08-04:
                // "--repo '/tmp/x y'" came back as "--repo /tmp/x y") — that
                // would corrupt the quoted self-report command in the prompt.
                // A template-free plain prompt keeps the text verbatim.
                prompt,
                // Idle status line is model-agnostic: "0.0%/… (auto)" + "(model) …" (2026-07-15:
                // default model is grok, not deepseek — hardcoding model names flaked).
                ready_patterns: vec!["0.0%".to_string(), "(auto)".to_string()],
                confirm_patterns: vec!["Working".to_string(), "Read the file".to_string()],
                needs_probe: true,
                completion_event: Some("agent.dispatch.completed".to_string()),
                // Primary completion is self-report; process-exit remains a side-channel.
                completion_mode: "goal-self-report".to_string(),
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
                // Deliberately NO `/goal` prefix for agy (goal_prompt leaves it
                // bare). A custom command exists (~/.gemini/commands/goal.toml),
                // but agy's TUI paste-expansion and — critically — its
                // unknown-command failure mode are UNVERIFIED (2026-08-04:
                // personal quota exhausted mid-test). If agy rejects an unknown
                // /name instead of submitting it verbatim, the dispatch prompt
                // would be silently lost. Re-test both before adding agy to the
                // `/goal` arm in goal_prompt.
                prompt,
                // Readiness must key on a marker that appears ONLY once agy's TUI
                // input box is live — NOT "agy", which the launch command echoes
                // (`agy -i ''`) while the shell is still running, causing a false
                // ready that sends the real prompt into the shell (prompt lost).
                // agy's idle input box shows "? for shortcuts"; wait for that.
                ready_patterns: vec!["? for shortcuts".to_string()],
                confirm_patterns: vec!["Working".to_string(), "Read the file".to_string()],
                needs_probe: true,
                completion_event: Some("agent.dispatch.completed".to_string()),
                // Primary completion is self-report; process-exit remains a side-channel.
                completion_mode: "goal-self-report".to_string(),
                launch_includes_prompt: false,
            }
        }
        "aix" => {
            // aix is NOT an interactive TUI like codex/pi/agy — it is a
            // one-shot command that takes the task as argv and exits when the
            // run reaches a terminal state. That difference drives every field
            // below.
            //
            // The prompt goes INTO the launch line (launch_includes_prompt),
            // so there is no "wait for an idle input box, then paste" step —
            // aix starts working the moment the line is submitted. Quoting is
            // safe here despite pi's lesson: pi lost quotes because its slash
            // template re-split and re-joined $ARGUMENTS (an extra parse layer
            // above the shell). aix does no template expansion at all, so the
            // prompt survives one shell parse verbatim — verified 2026-08-04:
            // the embedded `--repo '/tmp/x y'` reached argv with quotes intact.
            //
            // Completion is process-exit, not goal-self-report. aix has no
            // session a model could call `lto agent-turn-completed` from, but
            // its exit code IS trustworthy: rc=0 only when the run ends in
            // StopReason::TaskDone (the model called the task_done tool).
            // Anything else — self-report without task_done, max turns,
            // provider failure — exits non-zero. That is exactly the signal
            // process_exit_wrapper forwards.
            GoalRunnerPlan {
                launch: Some(process_exit_wrapper(
                    &format!(
                        "LTO_RUN_ID={} aix -k {} {}",
                        shell_single_quote(run_id),
                        shell_single_quote(&format!("lto:{run_id}")),
                        shell_single_quote(&prompt),
                    ),
                    "aix",
                    run_id,
                )),
                // No `/goal` prefix: aix has no goal-runtime (same call as
                // pi/agy). goal_prompt already leaves it bare for non-codex.
                prompt,
                // Nothing to wait for: the launch line runs aix directly rather
                // than dropping into a REPL, so there is no idle marker. An
                // empty pattern list makes wait_for_dispatch_ready return as
                // soon as the shell has taken the line.
                ready_patterns: Vec::new(),
                // aix prints a `── aix ──` banner then `任务：<task>` before the
                // first model call, so the banner proves the process actually
                // started (as opposed to the shell rejecting the line).
                confirm_patterns: vec!["── aix ──".to_string()],
                // No TUI input box to probe — probing would type a stray line
                // into the shell after aix already owns the terminal.
                needs_probe: false,
                completion_event: Some("agent.dispatch.completed".to_string()),
                completion_mode: "process-exit".to_string(),
                // The task is already in the launch line; sending it again
                // would leave a stray command in the shell after aix exits.
                launch_includes_prompt: true,
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

const COMPLETION_PROTOCOL_MARKER: &str = "<!-- lto:completion-protocol -->";
const RUNNER_CONSTRAINTS_MARKER: &str = "<!-- lto:runner-constraints -->";

/// Per-runner behavioral constraints injected into the dispatched goal file.
///
/// Built-in text exists only for codex（GPT-5.x 系）：实测爱绕弯子——自发扇出子代理
/// "优化"未要求的产物、再开子代理检查自己、长篇复述烧 token（2026-07-23 社区反馈
/// 固化）。其余 runner 默认不注入：约束提示是否弱化模型能力尚无证据，保持保守。
/// 本机可通过 `$LTO_CONSTRAINTS_DIR`（默认 `~/.config/lto/constraints`）下的
/// `<runner>.md` 覆盖文件为任意 runner 启用或替换文案；空文件表示对该 runner
/// 显式关闭内置约束。约束走 goal 文件注入段，不进 ≤500 字符的 dispatch prompt。
fn runner_constraints(runner: &str) -> Option<String> {
    runner_constraints_from(runner, constraints_dir().as_deref())
}

fn constraints_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LTO_CONSTRAINTS_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/lto/constraints"))
}

fn runner_constraints_from(runner: &str, dir: Option<&Path>) -> Option<String> {
    if let Some(dir) = dir
        && let Ok(text) = fs::read_to_string(dir.join(format!("{runner}.md")))
    {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return Some(wrap_runner_constraints(text));
    }
    match runner {
        "codex" => Some(wrap_runner_constraints(
            "- **只做本 goal 要求的事**：禁止顺手优化/重构/美化未提及的代码；发现题外问题记进汇报，不动手改。\n\
- **修改范围最小化**：满足完成判据的最小 diff；遵循 KISS，不引入未要求的抽象、依赖、配置。\n\
- **禁止自发扇出子代理**：不为\"优化已有产物\"或\"自我检查\"开子代理/并行任务；goal 明确要求的除外。\n\
- **默认简短、中文回复**：过程输出与最终汇报用中文要点式，只写做了什么/判据结果/遗留项，不复述 goal 内容。",
        )),
        _ => None,
    }
}

fn wrap_runner_constraints(body: &str) -> String {
    format!(
        "\n\n---\n{RUNNER_CONSTRAINTS_MARKER}\n## 执行约束（dispatch 注入，优先级高于默认行为）\n\n{body}\n"
    )
}

/// Ensure the goal file the agent reads carries the completion protocol.
///
/// Both A/B arms of run 20260722-seed-ab finished real work but never ran the
/// paste-line report command: execution agents satisfy the goal FILE's
/// completion criteria and drop the ephemeral prompt line from attention over
/// long runs. So the self-report command must live in the file itself. If the
/// goal already mentions `agent-turn-completed` and carries the runner's
/// behavioral constraints (if any), dispatch it untouched; otherwise
/// materialize a sibling `<stem>.dispatch.md` (same directory, so relative
/// references inside the goal keep resolving) with the original content plus
/// the missing constraint/completion-protocol footers, and point the agent
/// at that.
/// `--window-id` is intentionally omitted here — it is only known after the
/// window exists; the paste-line command still carries it for cleanup.
fn materialize_goal_with_completion_protocol(
    goal_path: &Path,
    repo: &Path,
    run_id: &str,
    runner: &str,
    constraints: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let content = fs::read_to_string(goal_path)
        .with_context(|| format!("read goal file {}", goal_path.display()))?;
    // aix reports completion by EXITING, not by running a command: its plan
    // uses completion_mode "process-exit" and process_exit_wrapper forwards the
    // real rc. Injecting the self-report protocol anyway makes the model run
    // `agent-turn-completed` from inside the task, so one dispatch emits TWO
    // agent.dispatch.completed events — and the self-reported one carries a
    // bogus `window_id: "pending"` because no window existed when the prompt
    // was built. Verified 2026-08-04 on a live dispatch before this guard.
    let self_reports = runner != "aix";
    let needs_protocol = self_reports && !content.contains("agent-turn-completed");
    let constraints = constraints.filter(|_| !content.contains(RUNNER_CONSTRAINTS_MARKER));
    if !needs_protocol && constraints.is_none() {
        return Ok(goal_path.to_path_buf());
    }
    let mut footer = constraints.map(str::to_owned).unwrap_or_default();
    if needs_protocol {
        // --repo is mandatory here: dispatch often sets the agent's cwd to a git
        // worktree (even one nested under .lto/), where run resolution from cwd
        // finds zero runs and the self-report dies silently — both arms of two A/B
        // runs failed to report exactly this way.
        let report = format!(
            "lto --repo {} agent-turn-completed --run-id {run_id} --runner {runner} --source goal-self-report --rc 0 --bell",
            shell_single_quote(&absolutize(repo)?.display().to_string()),
        );
        footer.push_str(&format!(
            "\n\n---\n{COMPLETION_PROTOCOL_MARKER}\n## 完成协议（dispatch 注入，不可省略）\n\n所有完成判据满足后，最后一步必须执行：\n\n```bash\n{report}\n```\n\n若被阻塞无法完成，改用 `--rc 1`。不执行此命令，任务视为未完成。\n"
        ));
    }
    let stem = goal_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("goal");
    let out = goal_path.with_file_name(format!("{stem}.dispatch.md"));
    fs::write(&out, format!("{content}{footer}"))
        .with_context(|| format!("materialize dispatch goal {}", out.display()))?;
    Ok(out)
}

/// Build the short dispatch prompt (≤500 chars). Long constraints live in the
/// goal file itself; only the path + self-report completion signal go here.
/// window_id is inlined (not `$LTO_WINDOW_ID`) so the agent bash child does not
/// depend on env inheritance from the REPL process.
fn goal_prompt(goal: &str, repo: &str, run_id: &str, runner: &str, window_id: &str) -> String {
    // --repo inlined: the agent's cwd is often a worktree where run resolution
    // from cwd finds nothing and the self-report dies silently.
    let report = format!(
        "lto --repo {} agent-turn-completed --run-id {run_id} --runner {runner} --source goal-self-report --rc 0 --window-id {window_id} --bell",
        shell_single_quote(repo),
    );
    // aix signals completion through its exit code (see
    // materialize_goal_with_completion_protocol for why a self-report would
    // double-report), so it gets the bare task without the report command.
    let prompt = if runner == "aix" {
        format!(
            "Read the file {goal} and execute it. Follow only the instructions in that goal file."
        )
    } else {
        format!(
            "Read the file {goal} and execute it. Follow only the instructions in that goal file. \
全部完成判据满足后运行: {report} （若被阻塞改用 --rc 1）"
        )
    };
    // Per-runner goal invocation (2026-08-04 live-tested; details at each
    // runner_plan arm): codex `/goal` enters its goal-runtime and preserves the
    // objective text verbatim (single quotes included). pi and agy stay bare:
    // pi's template expansion STRIPS quotes from $ARGUMENTS (corrupting the
    // self-report `--repo '<path>'`), and agy's unknown-command failure mode is
    // unverified.
    let prompt = match runner {
        "codex" => format!("/goal {prompt}"),
        _ => prompt,
    };
    if prompt.chars().count() > GOAL_PROMPT_MAX_CHARS {
        eprintln!("goal_prompt exceeds {GOAL_PROMPT_MAX_CHARS} chars; truncating to the hard cap");
        prompt.chars().take(GOAL_PROMPT_MAX_CHARS).collect()
    } else {
        prompt
    }
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
        state::atomic_write(
            &settings_path,
            (serde_json::to_string_pretty(&value)? + "\n").as_bytes(),
        )?;
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
        state::atomic_write(
            &hooks_path,
            (serde_json::to_string_pretty(&value)? + "\n").as_bytes(),
        )?;
        return Ok(HookStatus {
            status: "updated".to_string(),
            detail: hooks_path.display().to_string(),
            script_path: Some(script_path),
        });
    }
    backup_hooks(&hooks_path)?;
    stop_hooks_mut(&mut value)?.push(codex_stop_hook_entry(&command, &lto_bin));
    state::atomic_write(
        &hooks_path,
        (serde_json::to_string_pretty(&value)? + "\n").as_bytes(),
    )?;
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
    state::atomic_write(
        &hooks_path,
        (serde_json::to_string_pretty(&value)? + "\n").as_bytes(),
    )?;
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
            // codex 0.144+ shows a numbered update menu:
            // 1. Update now / 2. Skip / 3. Skip until next version.
            // Prefer "2" (Skip) so dispatch does not mutate the host toolchain.
            // Pattern keys on the menu footer; after skip, idle TUI shows model:.
            pattern: "Press enter to continue".to_string(),
            key: "2".to_string(),
        },
        SkipPrompt {
            pattern: "update available".to_string(),
            key: "2".to_string(),
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
        "codex" | "pi" | "agy" | "aix" => Ok(()),
        _ => anyhow::bail!("dispatch-goal runner must be one of codex, pi, agy, aix"),
    }
}

/// --target and --new-window are mutually exclusive, but neither is required.
/// With both unset, prepare_target uses the current pane when its foreground
/// command is an idle shell; a busy pane falls back to a visible new window
/// with a warning. An explicit busy target fails closed and suggests retrying
/// with --new-window.
fn validate_dispatch_target(target: Option<&str>, new_window: bool) -> anyhow::Result<()> {
    if target.is_some() && new_window {
        anyhow::bail!("pass at most one of --target or --new-window");
    }
    Ok(())
}

fn validate_dispatch_cwd(cwd: Option<&Path>) -> anyhow::Result<()> {
    let Some(cwd) = cwd else {
        return Ok(());
    };
    if cwd.is_dir() {
        return Ok(());
    }
    let absolute = absolutize(cwd)?;
    anyhow::bail!(
        "dispatch --cwd directory does not exist or is not a directory: {} (absolute: {})",
        cwd.display(),
        absolute.display()
    );
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
            no_runner_constraints: false,
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
    fn materialize_appends_completion_protocol_when_goal_lacks_report() {
        let tmp = tempfile::tempdir().unwrap();
        let goal = tmp.path().join("goal-x.md");
        fs::write(&goal, "# Goal\n\n做完就停。\n").unwrap();
        let out = materialize_goal_with_completion_protocol(
            &goal,
            Path::new("/repo"),
            "run-1",
            "codex",
            runner_constraints_from("codex", None).as_deref(),
        )
        .unwrap();
        assert_eq!(out, tmp.path().join("goal-x.dispatch.md"));
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("# Goal"), "original content must lead");
        assert!(text.contains(COMPLETION_PROTOCOL_MARKER));
        // --repo must be inlined: agents run this from a worktree cwd where
        // run resolution would otherwise find nothing.
        assert!(text.contains(
            "lto --repo '/repo' agent-turn-completed --run-id run-1 --runner codex --source goal-self-report"
        ));
        // Original goal file must stay untouched.
        assert_eq!(fs::read_to_string(&goal).unwrap(), "# Goal\n\n做完就停。\n");
        // codex additionally gets the behavioral-constraints block, before the
        // completion protocol so the report command stays the literal last step.
        assert!(text.contains(RUNNER_CONSTRAINTS_MARKER));
        assert!(text.contains("修改范围最小化"));
        assert!(
            text.find(RUNNER_CONSTRAINTS_MARKER).unwrap()
                < text.find(COMPLETION_PROTOCOL_MARKER).unwrap()
        );
    }

    #[test]
    fn materialize_keeps_goal_as_is_when_report_already_present() {
        let tmp = tempfile::tempdir().unwrap();
        let goal = tmp.path().join("goal-y.md");
        fs::write(&goal, "# Goal\n最后运行 lto agent-turn-completed --rc 0\n").unwrap();
        let out = materialize_goal_with_completion_protocol(
            &goal,
            Path::new("/repo"),
            "run-1",
            "pi",
            runner_constraints_from("pi", None).as_deref(),
        )
        .unwrap();
        assert_eq!(out, goal);
        assert!(!tmp.path().join("goal-y.dispatch.md").exists());
    }

    #[test]
    fn materialize_adds_codex_constraints_even_when_report_already_present() {
        let tmp = tempfile::tempdir().unwrap();
        let goal = tmp.path().join("goal-z.md");
        let content = "# Goal\n最后运行 lto agent-turn-completed --rc 0\n";
        fs::write(&goal, content).unwrap();
        let constraints = runner_constraints_from("codex", None);
        let out = materialize_goal_with_completion_protocol(
            &goal,
            Path::new("/repo"),
            "run-1",
            "codex",
            constraints.as_deref(),
        )
        .unwrap();
        assert_eq!(out, tmp.path().join("goal-z.dispatch.md"));
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.contains(RUNNER_CONSTRAINTS_MARKER));
        // Goal already carries the report command: no second protocol footer.
        assert!(!text.contains(COMPLETION_PROTOCOL_MARKER));
        // Re-materializing the dispatched file must be a no-op (idempotent).
        let again = materialize_goal_with_completion_protocol(
            &out,
            Path::new("/repo"),
            "run-1",
            "codex",
            constraints.as_deref(),
        )
        .unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn non_codex_runners_get_no_builtin_constraint_block() {
        for runner in ["pi", "agy", "claude", "gemini"] {
            assert!(
                runner_constraints_from(runner, None).is_none(),
                "runner {runner}"
            );
        }
    }

    #[test]
    fn constraint_override_file_enables_any_runner_and_replaces_builtin() {
        let dir = tempfile::tempdir().unwrap();
        // Override file enables a runner that has no built-in block.
        fs::write(dir.path().join("pi.md"), "以浅近文言作答。\n").unwrap();
        let pi = runner_constraints_from("pi", Some(dir.path())).unwrap();
        assert!(pi.contains(RUNNER_CONSTRAINTS_MARKER));
        assert!(pi.contains("以浅近文言作答。"));
        // Override file replaces the built-in codex block wholesale.
        fs::write(dir.path().join("codex.md"), "只此一条。\n").unwrap();
        let codex = runner_constraints_from("codex", Some(dir.path())).unwrap();
        assert!(codex.contains("只此一条。"));
        assert!(!codex.contains("修改范围最小化"));
        // An empty override file explicitly disables the built-in block.
        fs::write(dir.path().join("codex.md"), "\n").unwrap();
        assert!(runner_constraints_from("codex", Some(dir.path())).is_none());
        // No file for the runner falls back to the built-in (codex only).
        assert!(runner_constraints_from("agy", Some(dir.path())).is_none());
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
            let plan = runner_plan(
                runner,
                Path::new("/tmp/goal.md"),
                Path::new("/repo"),
                "r1",
                "@1",
            );
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
    fn explicit_dispatch_cwd_missing_path_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing-cwd");
        let err = validate_dispatch_cwd(Some(&missing)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&missing.display().to_string()));
        assert!(text.contains("directory does not exist"));
        assert!(text.contains(&absolutize(&missing).unwrap().display().to_string()));
    }

    #[test]
    fn explicit_dispatch_cwd_file_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("cwd-file");
        fs::write(&file, "not a directory").unwrap();
        let err = validate_dispatch_cwd(Some(&file)).unwrap_err();
        assert!(err.to_string().contains(&file.display().to_string()));
    }

    #[test]
    fn explicit_dispatch_cwd_directory_passes() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(validate_dispatch_cwd(Some(tmp.path())).is_ok());
    }

    #[test]
    fn omitted_dispatch_cwd_skips_validation() {
        assert!(validate_dispatch_cwd(None).is_ok());
    }

    #[test]
    fn runner_plan_uses_required_entrypoints() {
        let goal = Path::new("/tmp/goal.md");
        let codex = runner_plan("codex", goal, Path::new("/repo"), "r1", "@9");
        // codex `/goal` enters its built-in goal-runtime (2026-08-04 live test).
        assert!(codex.prompt.starts_with("/goal Read the file /tmp/goal.md"));
        assert!(codex.prompt.contains("goal-self-report"));
        assert!(codex.prompt.contains("--run-id r1"));
        assert!(codex.prompt.contains("--runner codex"));
        assert!(codex.prompt.contains("--window-id @9"));
        assert!(codex.prompt.contains("--rc 0"));
        assert!(
            codex.prompt.chars().count() <= GOAL_PROMPT_MAX_CHARS,
            "prompt must stay ≤{GOAL_PROMPT_MAX_CHARS} chars, got {}",
            codex.prompt.chars().count()
        );
        assert_eq!(
            codex.ready_patterns,
            vec![
                "gpt-".to_string(),
                "model:".to_string(),
                "codex>".to_string()
            ]
        );
        assert_eq!(
            runner_plan("pi", goal, Path::new("/repo"), "r1", "@1").ready_patterns,
            vec!["0.0%".to_string(), "(auto)".to_string()]
        );
        assert_eq!(
            codex.completion_event.as_deref(),
            Some("agent.dispatch.completed")
        );
        assert_eq!(codex.completion_mode, "goal-self-report");
        let codex_launch = codex.launch.as_deref().unwrap();
        assert!(
            codex_launch.starts_with("LTO_RUN_ID='r1' codex -c "),
            "codex launch must disable MCP via -c override, got: {codex_launch}"
        );
        assert!(
            codex_launch.contains("mcp_servers={}"),
            "codex launch must empty mcp_servers for isolation, got: {codex_launch}"
        );
        assert!(
            runner_plan("pi", goal, Path::new("/repo"), "r1", "@1")
                .launch
                .as_deref()
                .unwrap()
                .starts_with("LTO_RUN_ID='r1' pi --no-skills")
        );
        assert!(
            !runner_plan("pi", goal, Path::new("/repo"), "r1", "@1")
                .launch
                .as_deref()
                .unwrap()
                .contains("--print")
        );
        let pi_plan = runner_plan("pi", goal, Path::new("/repo"), "r1", "@2");
        // pi stays bare-text: its template expansion strips single quotes from
        // $ARGUMENTS, which would corrupt the quoted self-report command.
        assert!(pi_plan.prompt.starts_with("Read the file "));
        assert!(!pi_plan.prompt.starts_with("/goal"));
        assert!(pi_plan.prompt.contains("goal-self-report"));
        assert!(pi_plan.prompt.contains("--window-id @2"));
        assert!(pi_plan.prompt.chars().count() <= GOAL_PROMPT_MAX_CHARS);
        // Long LTO constraints stay out of the prompt (live in the goal file).
        assert!(!pi_plan.prompt.contains("Use LTO discipline"));
        assert!(!pi_plan.prompt.contains("NEVER impersonate"));
        assert!(pi_plan.needs_probe);
        let pi_launch = pi_plan.launch.as_deref().unwrap();
        assert!(!pi_launch.contains(" -e "));
        // process-exit wrapper remains as a side-channel when the REPL exits.
        assert!(pi_launch.contains("pi-process-exit"));
        assert_eq!(
            pi_plan.completion_event.as_deref(),
            Some("agent.dispatch.completed")
        );
        assert_eq!(pi_plan.completion_mode, "goal-self-report");

        // aix is a one-shot command, not a TUI. Each assertion below guards a
        // failure mode that would look like a working dispatch until the run
        // silently produced nothing.
        let aix_plan = runner_plan("aix", goal, Path::new("/repo"), "r1", "@4");
        let aix_launch = aix_plan.launch.as_deref().unwrap();
        // The task must ride in the launch line; if it were sent separately
        // (like codex/pi) it would land in the shell AFTER aix already exited.
        assert!(aix_plan.launch_includes_prompt);
        assert!(aix_launch.contains("aix -k "));
        assert!(aix_launch.contains("Read the file /tmp/goal.md"));
        // Completion comes from the exit code, which aix only makes 0 when the
        // model called task_done. No session exists for a self-report, so the
        // prompt must NOT carry the report command — injecting it made one
        // dispatch emit two agent.dispatch.completed events (live-verified
        // 2026-08-04, the self-reported one had a bogus window_id "pending").
        assert!(!aix_plan.prompt.contains("agent-turn-completed"));
        assert!(!aix_plan.prompt.contains("goal-self-report"));
        assert_eq!(aix_plan.completion_mode, "process-exit");
        assert!(aix_launch.contains("aix-process-exit"));
        // No REPL means nothing to wait for and nothing to probe; a probe
        // would type a stray line into the shell aix is running in.
        assert!(aix_plan.ready_patterns.is_empty());
        assert!(!aix_plan.needs_probe);
        // aix has no goal-runtime, so it stays bare like pi/agy.
        assert!(!aix_plan.prompt.starts_with("/goal"));

        // agy must use the interactive entrypoint (`agy -i`), not `--print`
        // which only prints a plan without executing (bug #5/#6).
        assert!(
            runner_plan("agy", goal, Path::new("/repo"), "r1", "@3")
                .launch
                .as_deref()
                .unwrap()
                .starts_with("LTO_RUN_ID='r1' agy -i")
        );
        assert!(
            !runner_plan("agy", goal, Path::new("/repo"), "r1", "@3")
                .prompt
                .contains("--print")
        );
        assert!(
            runner_plan("agy", goal, Path::new("/repo"), "r1", "@3")
                .prompt
                .contains("goal-self-report")
        );
        assert!(runner_plan("agy", goal, Path::new("/repo"), "r1", "@3").needs_probe);

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
        let agy = runner_plan("agy", goal, Path::new("/repo"), "r1", "@4");
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
        assert_eq!(agy.completion_mode, "goal-self-report");
        // agy takes the real prompt on a later line, like codex/pi.
        assert!(!agy.launch_includes_prompt);
        assert!(!runner_plan("codex", goal, Path::new("/repo"), "r1", "@5").launch_includes_prompt);
        assert!(!runner_plan("pi", goal, Path::new("/repo"), "r1", "@5").launch_includes_prompt);
        // The real prompt still exists (sent later), and it's the short self-report prompt.
        assert!(agy.prompt.contains("Read the file"));
        assert!(agy.prompt.contains("--window-id @4"));
        // agy stays bare-text until its unknown-command failure mode is
        // verified — a rejected /name would silently lose the prompt.
        assert!(!agy.prompt.starts_with("/goal"));
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
        let plan = runner_plan("pi", goal, Path::new("/repo"), "r1", "@1");

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
