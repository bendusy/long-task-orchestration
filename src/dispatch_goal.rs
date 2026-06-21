use crate::commands::util;
use crate::events::{self, EventRecord};
use crate::tmux_runner::{self, SkipPrompt, TmuxMode, TmuxRunnerConfig};
use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LTO_HOOK_MARKER: &str = "long-task-orchestration";
const CODEX_STOP_HOOK: &str = include_str!("../scripts/hooks/codex-stop-notify.sh");
const GOAL_CONSTRAINT_SUMMARY: &str = "Use LTO discipline: keep LTO self-managed, run required redlines, use heterogeneous audit before closeout, write the local commit when accepted, and leave release/tag/push to the host.";

#[derive(Debug, Clone)]
pub struct DispatchGoalOptions {
    pub run_id: Option<String>,
    pub runner: String,
    pub goal: PathBuf,
    pub target: Option<String>,
    pub new_window: bool,
    pub window_name: Option<String>,
    pub cwd: Option<PathBuf>,
    pub tmux_session: Option<String>,
    pub tmux_bin: Option<String>,
    pub ready_timeout_sec: Option<u64>,
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
}

#[derive(Debug, Clone)]
struct HookStatus {
    status: String,
    detail: String,
    script_path: Option<PathBuf>,
}

pub fn cmd_dispatch_goal(repo: &Path, options: DispatchGoalOptions) -> anyhow::Result<()> {
    if options.uninstall_hooks {
        let status = uninstall_codex_hook(repo)?;
        println!(
            "codex hook uninstall: {} ({})",
            status.status, status.detail
        );
        return Ok(());
    }
    validate_runner(&options.runner)?;
    if !options.goal.exists() {
        anyhow::bail!("goal file does not exist: {}", options.goal.display());
    }
    validate_dispatch_target(options.target.as_deref(), options.new_window)?;

    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    let goal_path = absolutize(&options.goal)?;
    let cwd = options.cwd.clone().unwrap_or_else(|| repo.to_path_buf());
    let hook_status = if options.runner == "codex" && !options.no_install_hooks {
        install_codex_hook(repo).unwrap_or_else(|err| HookStatus {
            status: "degraded".to_string(),
            detail: err.to_string(),
            script_path: None,
        })
    } else {
        HookStatus {
            status: "skipped".to_string(),
            detail: "not a codex dispatch or --no-install-hooks".to_string(),
            script_path: None,
        }
    };

    let outcome = run_dispatch(repo, &ctx.run_id, &options, &goal_path, &cwd)?;
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
    println!("dispatch_record={}", dispatch_path.display());
    println!(
        "completion_event={}",
        outcome.completion_event.as_deref().unwrap_or("none")
    );
    println!("completion_mode={}", outcome.completion_mode);
    println!("hook_status={} {}", hook_status.status, hook_status.detail);
    Ok(())
}

fn run_dispatch(
    repo: &Path,
    run_id: &str,
    options: &DispatchGoalOptions,
    goal_path: &Path,
    cwd: &Path,
) -> anyhow::Result<GoalDispatchOutcome> {
    let plan = runner_plan(&options.runner, goal_path, run_id);
    let config = TmuxRunnerConfig {
        mode: TmuxMode::Fire,
        target: options.target.clone(),
        session: options.tmux_session.clone(),
        new_window: options.new_window,
        new_session: false,
        window_name: options
            .window_name
            .clone()
            .unwrap_or_else(|| format!("lto-goal-{}", options.runner)),
        signal_name: "lto-dispatch-goal".to_string(),
        sentinel_path: None,
        ready_patterns: plan.ready_patterns.clone(),
        skip_prompts: default_skip_prompts(),
        ready_timeout: Duration::from_secs(options.ready_timeout_sec.unwrap_or(20)),
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
    runtime
        .block_on(async {
            let target = tmux_runner::prepare_dispatch_target(&config).await?;
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
            if plan.needs_probe {
                let probe = format!("LTO_PROBE_{}", now_millis());
                let _ = tmux_runner::confirm_tui_input(&config, &target, &probe).await?;
            }
            tmux_runner::send_dispatch_text(&config, &target, &plan.prompt).await?;
            let capture =
                tmux_runner::wait_for_capture_patterns(&config, &target, &plan.confirm_patterns)
                    .await?;
            Ok::<_, tmux_runner::TmuxRunnerError>(GoalDispatchOutcome {
                target,
                capture,
                repo: cwd.display().to_string(),
                completion_event: plan.completion_event,
                completion_mode: plan.completion_mode,
            })
        })
        .map_err(|err| anyhow!("dispatch-goal tmux failure in {}: {err}", repo.display()))
}

#[derive(Debug, Clone)]
struct GoalDispatchOutcome {
    target: String,
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
            completion_event: Some("agent.turn.completed".to_string()),
            completion_mode: "auto-event".to_string(),
        },
        "pi" => GoalRunnerPlan {
            launch: Some(format!(
                "LTO_RUN_ID={} pi --no-skills --no-context-files --no-extensions",
                shell_single_quote(run_id)
            )),
            prompt: goal_prompt(&goal),
            ready_patterns: vec!["deepseek".to_string(), "ctx".to_string()],
            confirm_patterns: vec!["Working".to_string()],
            needs_probe: true,
            completion_event: None,
            completion_mode: "manual-pi-tui".to_string(),
        },
        "agy" => {
            // agy `-i`/--prompt-interactive runs the initial prompt in a real
            // TUI session and continues — it actually executes. `--print` only
            // prints a plan without executing (false-success trap, bug #5/#6),
            // so dispatch must use the interactive entrypoint like codex/pi.
            GoalRunnerPlan {
                launch: Some("agy -i".to_string()),
                prompt: goal_prompt(&goal),
                ready_patterns: vec!["agy".to_string()],
                confirm_patterns: vec!["Working".to_string(), "Read the file".to_string()],
                needs_probe: true,
                completion_event: None,
                completion_mode: "manual-agy-tui".to_string(),
            }
        }
        _ => unreachable!("runner validated"),
    }
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
    let repo_abs = absolutize(repo)?.display().to_string();
    let lto_bin = current_lto_bin();
    let command = format!(
        "LTO_REPO_FALLBACK={} LTO_BIN={} bash {}",
        shell_single_quote(&repo_abs),
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
        *existing = codex_stop_hook_entry(&command, &repo_abs, &lto_bin);
        fs::write(&hooks_path, serde_json::to_string_pretty(&value)? + "\n")?;
        return Ok(HookStatus {
            status: "updated".to_string(),
            detail: hooks_path.display().to_string(),
            script_path: Some(script_path),
        });
    }
    backup_hooks(&hooks_path)?;
    stop_hooks_mut(&mut value)?.push(codex_stop_hook_entry(&command, &repo_abs, &lto_bin));
    fs::write(&hooks_path, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(HookStatus {
        status: "installed".to_string(),
        detail: hooks_path.display().to_string(),
        script_path: Some(script_path),
    })
}

fn codex_stop_hook_entry(command: &str, repo: &str, lto_bin: &str) -> Value {
    json!({
        "matcher": "",
        "_lto_marker": LTO_HOOK_MARKER,
        "_lto_repo": repo,
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
            pattern: "Hooks need review".to_string(),
            key: "t".to_string(),
        },
        SkipPrompt {
            pattern: "hook needs review".to_string(),
            key: "t".to_string(),
        },
        SkipPrompt {
            pattern: "Press t to trust all".to_string(),
            key: "t".to_string(),
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
            Some("agent.turn.completed")
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
        assert_eq!(runner_plan("pi", goal, "r1").completion_event, None);
        assert_eq!(
            runner_plan("pi", goal, "r1").completion_mode,
            "manual-pi-tui"
        );
        // agy must use the interactive entrypoint (`agy -i`), not `--print`
        // which only prints a plan without executing (bug #5/#6).
        assert_eq!(
            runner_plan("agy", goal, "r1").launch.as_deref(),
            Some("agy -i")
        );
        assert!(!runner_plan("agy", goal, "r1").prompt.contains("--print"));
        assert!(
            runner_plan("agy", goal, "r1")
                .prompt
                .contains("Use LTO discipline")
        );
        assert!(runner_plan("agy", goal, "r1").needs_probe);
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
    fn hook_install_updates_lto_marker_for_new_repo() {
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
        assert_eq!(stop[0]["_lto_repo"], repo.display().to_string());
        let command = stop[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("LTO_REPO_FALLBACK="));
        assert!(!command.contains("LTO_REPO="));
        assert!(command.contains(repo.to_str().unwrap()));
    }
}
