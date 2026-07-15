use crate::events::{self, EventRecord};
use crate::state;
use anyhow::Context;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AgentTurnOptions {
    pub run_id: Option<String>,
    pub runner: String,
    pub payload_file: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
    pub summary: Option<String>,
    pub rc: Option<i32>,
    pub window_id: Option<String>,
    pub source: String,
    /// Ring the terminal/tmux bell on completion so a watching human notices.
    pub bell: bool,
    /// Optional human-notification command template run on completion. Supports
    /// {summary}/{rc}/{run_id}/{runner} placeholders. LTO does not hardcode any
    /// notifier (e.g. iaf) — the host wires its own through this hook.
    pub notify_cmd: Option<String>,
}

pub fn cmd_agent_turn_completed(repo: &Path, options: AgentTurnOptions) -> anyhow::Result<()> {
    let payload_text = read_payload(options.payload_file.as_deref())?;
    let payload = parse_payload(&payload_text);
    let cwd = options.cwd.or_else(|| payload_cwd(payload.as_ref()));
    let session_id = options
        .session_id
        .or_else(|| payload_string(payload.as_ref(), &["session_id", "sessionId"]));
    let summary = options
        .summary
        .or_else(|| payload_summary(payload.as_ref()))
        .unwrap_or_else(|| "agent turn completed".to_string());
    // goal-self-report is an explicit dispatch completion signal: it must carry
    // --run-id (no silent cwd routing), so a stray self-report cannot attach to
    // the wrong run.
    if options.source == "goal-self-report" && options.run_id.is_none() {
        anyhow::bail!("goal-self-report requires --run-id");
    }
    let Some(run_id) = route_run(repo, options.run_id.as_deref(), cwd.as_deref())? else {
        println!("agent.turn.completed ignored: no matching LTO run");
        return Ok(());
    };
    let mut run_state = load_run_state(repo, &run_id);
    let phase = run_state
        .as_ref()
        .map(|state| state.current_phase.clone())
        .filter(|phase| !phase.trim().is_empty());
    let notify_cmd = options.notify_cmd.clone().or_else(|| {
        run_state
            .as_ref()
            .and_then(|state| state.notify_cmd.clone())
    });
    let window_id = options
        .window_id
        .clone()
        .or_else(|| std::env::var("LTO_WINDOW_ID").ok())
        .filter(|value| !value.trim().is_empty());
    let self_report = options.source == "goal-self-report";
    let goal_completion_proof = if self_report {
        Some("goal-self-report".to_string())
    } else if options.source == "codex-stop-hook" {
        payload_goal_completion_proof(payload.as_ref())
    } else {
        None
    };
    let process_exit = options.source.ends_with("-process-exit");
    let dispatch_completed = goal_completion_proof.is_some() || process_exit;
    let event_type = if dispatch_completed {
        "agent.dispatch.completed"
    } else if options.source.ends_with("-session-end-hook") {
        "agent.session.ended"
    } else {
        "agent.turn.completed"
    };
    // codex-update-goal-complete implies success (rc=0). self-report and
    // process-exit carry the caller's real rc (0 = done, non-zero = failed/blocked).
    let effective_rc = if self_report || process_exit {
        options.rc
    } else if goal_completion_proof.is_some() {
        Some(0)
    } else {
        options.rc
    };

    let payload_hash = if payload_text.trim().is_empty() {
        None
    } else {
        Some(format!("{:x}", Sha256::digest(payload_text.as_bytes())))
    };
    let fields = json!({
        "runner": options.runner,
        "cwd": cwd.as_ref().map(|path| path.display().to_string()),
        "session_id": session_id,
        "rc": effective_rc,
        "window_id": window_id,
        "source": options.source,
        "completion_scope": if dispatch_completed { "dispatch" } else if event_type == "agent.session.ended" { "session" } else { "turn" },
        // Alias completion_proof == goal_completion_proof for docs/consumers.
        "goal_completion_proof": goal_completion_proof.clone(),
        "completion_proof": goal_completion_proof,
        "payload_sha256": payload_hash,
        "known_payload_schema": payload.is_some(),
    });
    let runner_name = options.runner.clone();
    let summary_text = summary.clone();
    let event = events::safe_emit(
        repo,
        &run_id,
        EventRecord {
            event_type: event_type.to_string(),
            actor_kind: "runner".to_string(),
            actor_id: Some(options.runner),
            phase,
            summary,
            fields,
            ..EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &run_id);
    if event.is_some() {
        println!("{event_type} emitted for run {run_id}");
    } else {
        println!("{event_type} dropped for run {run_id}");
    }

    // Last hop: signal that the turn is done. All best-effort and never fail the
    // command (Hook Shim discipline — a notifier must not stall/crash the turn).
    // 1. Wake any `lto events --wait` waiter for this run (machine -> machine).
    crate::notify::wake_run(repo, &run_id);
    // 2. Ring the bell so a watching human notices (machine -> local human).
    if options.bell && dispatch_completed {
        print!("\x07");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    // 3. Run the host-supplied notifier, e.g. iaf (machine -> remote human).
    if dispatch_completed && let Some(template) = notify_cmd.as_deref() {
        run_notify_cmd(template, &run_id, &runner_name, &summary_text, effective_rc);
    }
    // 4. Clean only the immutable window id created and recorded by this run.
    if dispatch_completed
        && let Err(err) = finish_dispatch_window(
            repo,
            &run_id,
            &runner_name,
            window_id.as_deref(),
            effective_rc,
            run_state.as_mut(),
        )
    {
        eprintln!("window cleanup failed; window retained for troubleshooting: {err}");
    }
    Ok(())
}

fn finish_dispatch_window(
    repo: &Path,
    run_id: &str,
    runner: &str,
    window_id: Option<&str>,
    rc: Option<i32>,
    state: Option<&mut state::LtoState>,
) -> anyhow::Result<()> {
    let (Some(window_id), Some(state)) = (window_id, state) else {
        return Ok(());
    };
    if !is_window_id(window_id) {
        anyhow::bail!("invalid tmux window id {window_id:?}");
    }
    let Some(index) = state.dispatch_windows.iter().rposition(|window| {
        window.window_id == window_id && window.runner == runner && window.status == "active"
    }) else {
        eprintln!("window {window_id} retained: no active dispatch record for runner {runner}");
        return Ok(());
    };

    let cleanup_on_success = state.dispatch_windows[index].cleanup_on_success;
    if rc != Some(0) || !cleanup_on_success {
        let reason = if rc != Some(0) {
            match rc {
                Some(rc) => format!("runner completion rc={rc}"),
                None => "runner completion rc missing".to_string(),
            }
        } else {
            "--keep-window requested".to_string()
        };
        state.dispatch_windows[index].status = "retained".to_string();
        state.dispatch_windows[index].finished_at = Some(crate::state::iso_now());
        state.dispatch_windows[index].retention_reason = Some(reason.clone());
        save_run_state(repo, run_id, state)?;
        eprintln!("window {window_id} retained for troubleshooting: {reason}");
        return Ok(());
    }

    let tmux_bin = state.dispatch_windows[index].tmux_bin.clone();
    let cleanup_command = format!(
        "sleep 0.5; {} kill-window -t {}",
        shell_single_quote(&tmux_bin),
        shell_single_quote(window_id)
    );
    let output = std::process::Command::new(&tmux_bin)
        .args(["run-shell", "-b", &cleanup_command])
        .output()
        .with_context(|| format!("schedule {tmux_bin} kill-window -t {window_id}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        state.dispatch_windows[index].status = "retained".to_string();
        state.dispatch_windows[index].finished_at = Some(crate::state::iso_now());
        state.dispatch_windows[index].retention_reason =
            Some(format!("tmux cleanup scheduling failed: {stderr}"));
        save_run_state(repo, run_id, state)?;
        anyhow::bail!("tmux cleanup scheduling for {window_id} failed: {stderr}");
    }

    state.dispatch_windows[index].status = "cleaned".to_string();
    state.dispatch_windows[index].finished_at = Some(crate::state::iso_now());
    state.dispatch_windows[index].retention_reason = None;
    save_run_state(repo, run_id, state)?;
    events::safe_emit(
        repo,
        run_id,
        EventRecord {
            event_type: "runner.window.cleaned".to_string(),
            actor_kind: "lto".to_string(),
            actor_id: Some(runner.to_string()),
            summary: format!("cleaned dispatch window {window_id}"),
            fields: json!({"runner": runner, "window_id": window_id, "scheduled": true}),
            ..EventRecord::default()
        },
    );
    println!("runner.window.cleaned emitted for run {run_id} window {window_id}");
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn save_run_state(repo: &Path, run_id: &str, state: &state::LtoState) -> anyhow::Result<()> {
    let state_path = repo.join(".lto").join(run_id).join("state.json");
    let mut next = state.clone();
    crate::commands::util::save_state_preserving_c2(&state_path, run_id, &mut next)
}

fn is_window_id(value: &str) -> bool {
    value.starts_with('@') && value.len() > 1 && value[1..].chars().all(|ch| ch.is_ascii_digit())
}

/// Run a host-supplied notification command, exposing the turn fields as
/// environment variables ($LTO_SUMMARY/$LTO_RC/$LTO_RUN_ID/$LTO_RUNNER) rather
/// than interpolating them into the shell string. The summary comes from
/// runner output and is untrusted; passing it via env keeps a value like
/// `; rm -rf ~` from being re-parsed by the shell (no command injection).
/// {placeholder} forms are still substituted for convenience but only for the
/// trusted internal fields (run_id/runner/rc), never the untrusted summary.
/// Best-effort: any failure is logged to stderr and swallowed so the turn never
/// fails because of a notifier.
fn run_notify_cmd(template: &str, run_id: &str, runner: &str, summary: &str, rc: Option<i32>) {
    let rc_str = rc.map(|v| v.to_string()).unwrap_or_default();
    // Only trusted, shell-safe internal fields are interpolated. The untrusted
    // summary is intentionally NOT substituted into the command string; use
    // $LTO_SUMMARY in the template to reference it safely.
    let rendered = template
        .replace("{run_id}", run_id)
        .replace("{runner}", runner)
        .replace("{rc}", &rc_str);
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .env("LTO_SUMMARY", summary)
        .env("LTO_RUN_ID", run_id)
        .env("LTO_RUNNER", runner)
        .env("LTO_RC", &rc_str)
        .status();
    if let Err(err) = status {
        eprintln!("notify-cmd failed (ignored): {err}");
    }
}

fn load_run_state(repo: &Path, run_id: &str) -> Option<state::LtoState> {
    let path = repo.join(".lto").join(run_id).join("state.json");
    state::load_state(path).ok()
}

fn read_payload(path: Option<&Path>) -> anyhow::Result<String> {
    match path {
        Some(path) if path == Path::new("-") => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Ok(text)
        }
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("read hook payload {}", path.display())),
        None => Ok(String::new()),
    }
}

fn parse_payload(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        None
    } else {
        serde_json::from_str(text).ok()
    }
}

fn payload_cwd(payload: Option<&Value>) -> Option<PathBuf> {
    payload_string(payload, &["cwd", "workspace", "repo", "repo_root"]).map(PathBuf::from)
}

fn payload_summary(payload: Option<&Value>) -> Option<String> {
    payload_string(
        payload,
        &[
            "summary",
            "prompt_response",
            "response",
            "last_response",
            "message",
        ],
    )
    .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn payload_string(payload: Option<&Value>, keys: &[&str]) -> Option<String> {
    let object = payload?.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn payload_goal_completion_proof(payload: Option<&Value>) -> Option<String> {
    let transcript = payload_string(payload, &["transcript_path", "transcriptPath"])?;
    if transcript_has_goal_completion(Path::new(&transcript)) {
        Some("codex-update-goal-complete".to_string())
    } else {
        None
    }
}

fn transcript_has_goal_completion(path: &Path) -> bool {
    const TAIL_BYTES: u64 = 2 * 1024 * 1024;
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return false;
    };
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
    {
        return false;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|value| value_has_goal_completion_call(&value))
}

fn value_has_goal_completion_call(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
        let name = object.get("name").and_then(Value::as_str).unwrap_or("");
        if name == "update_goal"
            && object
                .get("input")
                .or_else(|| object.get("arguments"))
                .is_some_and(value_declares_goal_complete)
        {
            return true;
        }
        if name == "exec"
            && let Some(input) = object.get("input").and_then(Value::as_str)
        {
            let compact = input
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            if let Some(call_at) = compact.find("tools.update_goal(") {
                let prefix = &compact[..call_at];
                let is_direct_await = prefix.ends_with("=await")
                    || prefix.ends_with("returnawait")
                    || prefix.ends_with(";await")
                    || prefix == "await";
                if is_direct_await
                    && js_position_is_unquoted(&compact, call_at)
                    && (compact[call_at..].contains("status:\"complete\"")
                        || compact[call_at..].contains("status:'complete'")
                        || compact[call_at..].contains("\"status\":\"complete\""))
                {
                    return true;
                }
            }
        }
    }
    object.values().any(value_has_goal_completion_call)
}

fn js_position_is_unquoted(text: &str, position: usize) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if index >= position {
            break;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            None if matches!(ch, '\'' | '"' | '`') => quote = Some(ch),
            _ => {}
        }
    }
    quote.is_none()
}

fn value_declares_goal_complete(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.get("status").and_then(Value::as_str) == Some("complete"),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .is_some_and(value_declares_goal_complete),
        _ => false,
    }
}

fn route_run(
    repo: &Path,
    explicit: Option<&str>,
    cwd: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    if let Some(run_id) = explicit {
        return Ok(Some(state::validate_run_id(run_id)?.to_string()));
    }
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let cwd = normalize_path(cwd);
    let lto_dir = repo.join(".lto");
    let current = fs::read_to_string(lto_dir.join("current"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let mut matches = Vec::new();
    let Ok(entries) = fs::read_dir(&lto_dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path().join("state.json");
        if !path.exists() {
            continue;
        }
        let Ok(state) = state::load_state(&path) else {
            continue;
        };
        if state.current_phase == "closed" {
            continue;
        }
        let root = workspace_root(repo, &state.workspace.repo_root);
        if cwd.starts_with(&root) || root.starts_with(&cwd) {
            matches.push((state.run_id, state.started_at));
        }
    }
    if let Some(current) = current
        && matches.iter().any(|(run_id, _)| run_id == &current)
    {
        return Ok(Some(current));
    }
    matches.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(matches.pop().map(|(run_id, _)| run_id))
}

fn workspace_root(repo: &Path, recorded: &str) -> PathBuf {
    let path = PathBuf::from(recorded);
    if path.as_os_str().is_empty() || path == Path::new(".") {
        normalize_path(repo)
    } else if path.is_absolute() {
        normalize_path(&path)
    } else {
        normalize_path(&repo.join(path))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DispatchWindowState, LtoState, WorkspaceSnapshot};

    fn state_with_window(repo: &Path, tmux_bin: &Path, cleanup_on_success: bool) -> LtoState {
        let state = LtoState {
            run_id: "r1".to_string(),
            dispatch_windows: vec![DispatchWindowState {
                window_id: "@42".to_string(),
                target: "@42.0".to_string(),
                runner: "codex".to_string(),
                tmux_bin: tmux_bin.display().to_string(),
                cleanup_on_success,
                status: "active".to_string(),
                created_at: crate::state::iso_now(),
                finished_at: None,
                retention_reason: None,
            }],
            ..LtoState::default()
        };
        let run_dir = repo.join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        state::save_state(run_dir.join("state.json"), &state).unwrap();
        state
    }

    fn fake_tmux(repo: &Path) -> (PathBuf, PathBuf) {
        let bin = repo.join("tmux-fake");
        let log = repo.join("tmux.log");
        fs::write(
            &bin,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n", log.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&bin, permissions).unwrap();
        }
        (bin, log)
    }

    #[test]
    fn routes_by_cwd_and_emits_agent_turn_event() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_id = "r1";
        let run_dir = repo.join(".lto").join(run_id);
        let notified = repo.join("notified.txt");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(repo.join(".lto").join("current"), format!("{run_id}\n")).unwrap();
        let state = LtoState {
            run_id: run_id.to_string(),
            current_phase: "implementation".to_string(),
            workspace: WorkspaceSnapshot {
                repo_root: repo.display().to_string(),
                ..WorkspaceSnapshot::default()
            },
            notify_cmd: Some(format!(
                "printf '%s' \"$LTO_SUMMARY\" > {}",
                notified.display()
            )),
            ..LtoState::default()
        };
        state::save_state(run_dir.join("state.json"), &state).unwrap();

        cmd_agent_turn_completed(
            repo,
            AgentTurnOptions {
                run_id: None,
                runner: "codex".to_string(),
                payload_file: None,
                cwd: Some(repo.to_path_buf()),
                session_id: Some("s1".to_string()),
                summary: Some("done".to_string()),
                rc: Some(0),
                window_id: None,
                source: "test".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        let events = events::read(repo, run_id).unwrap();
        assert_eq!(events[0]["type"], "agent.turn.completed");
        assert_eq!(events[0]["phase"], "implementation");
        assert_eq!(events[0]["fields"]["runner"], "codex");
        assert_eq!(events[0]["fields"]["session_id"], "s1");
        assert!(
            !notified.exists(),
            "per-turn Stop events must not notify as done"
        );
    }

    #[test]
    fn notify_cmd_substitutes_trusted_fields_and_passes_summary_via_env() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("notified.txt");
        // Trusted fields via {placeholder}; untrusted summary via $LTO_SUMMARY.
        let template = format!(
            "printf '%s' \"{{run_id}}|{{runner}}|$LTO_SUMMARY|{{rc}}\" > {}",
            out.display()
        );
        run_notify_cmd(&template, "run-7", "agy", "all green", Some(0));
        let written = std::fs::read_to_string(&out).unwrap();
        assert_eq!(written, "run-7|agy|all green|0");
    }

    #[test]
    fn notify_cmd_does_not_execute_injection_in_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let pwned = tmp.path().join("pwned.txt");
        let out = tmp.path().join("safe.txt");
        // A malicious summary that WOULD run `touch pwned` if interpolated into sh -c.
        let evil = format!("x\"; touch {} ; echo \"", pwned.display());
        let template = format!("printf '%s' \"$LTO_SUMMARY\" > {}", out.display());
        run_notify_cmd(&template, "r1", "agy", &evil, Some(0));
        // The injection file must NOT exist — summary was passed as data, not code.
        assert!(!pwned.exists(), "summary must not be executed as a command");
        // And the literal evil string lands in the output as plain data.
        assert_eq!(std::fs::read_to_string(&out).unwrap(), evil);
    }

    #[test]
    fn notify_cmd_failure_is_swallowed() {
        // A failing command must not panic / propagate (Hook Shim discipline).
        run_notify_cmd("exit 3", "r1", "codex", "x", Some(3));
    }

    #[test]
    fn codex_stop_without_goal_proof_does_not_clean_window() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        state_with_window(tmp.path(), &tmux_bin, true);
        let transcript = tmp.path().join("rollout.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"response_item","payload":{"type":"message","content":"status complete is only user text"}}
"#,
        )
        .unwrap();
        let payload = tmp.path().join("payload.json");
        fs::write(
            &payload,
            json!({"cwd": tmp.path(), "transcript_path": transcript}).to_string(),
        )
        .unwrap();

        cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: Some("r1".to_string()),
                runner: "codex".to_string(),
                payload_file: Some(payload),
                cwd: None,
                session_id: None,
                summary: None,
                rc: None,
                window_id: Some("@42".to_string()),
                source: "codex-stop-hook".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        assert!(!log.exists());
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "active");
        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "agent.turn.completed");
        assert_eq!(events[0]["fields"]["completion_scope"], "turn");
        assert!(events[0]["fields"]["rc"].is_null());
    }

    #[test]
    fn update_goal_text_inside_patch_is_not_completion_proof() {
        let value = json!({
            "type": "custom_tool_call",
            "name": "exec",
            "input": "const patch = \"const x = await tools.update_goal({status:'complete'});\"; await tools.apply_patch(patch);"
        });

        assert!(!value_has_goal_completion_call(&value));
    }

    #[test]
    fn codex_stop_with_update_goal_proof_cleans_window() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        state_with_window(tmp.path(), &tmux_bin, true);
        let transcript = tmp.path().join("rollout.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"const result = await tools.update_goal({status:\"complete\"});"}}
"#,
        )
        .unwrap();
        let payload = tmp.path().join("payload.json");
        fs::write(
            &payload,
            json!({"cwd": tmp.path(), "transcript_path": transcript}).to_string(),
        )
        .unwrap();

        cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: Some("r1".to_string()),
                runner: "codex".to_string(),
                payload_file: Some(payload),
                cwd: None,
                session_id: None,
                summary: Some("goal complete".to_string()),
                rc: None,
                window_id: Some("@42".to_string()),
                source: "codex-stop-hook".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        let cleanup = fs::read_to_string(log).unwrap();
        assert!(cleanup.starts_with("run-shell -b sleep 0.5;"));
        assert!(cleanup.contains("kill-window -t '@42'"));
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "cleaned");
        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "agent.dispatch.completed");
        assert_eq!(events[0]["fields"]["rc"], 0);
        assert_eq!(
            events[0]["fields"]["goal_completion_proof"],
            "codex-update-goal-complete"
        );
        assert_eq!(events[1]["type"], "runner.window.cleaned");
    }

    #[test]
    fn process_exit_uses_real_rc_for_dispatch_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        state::save_state(
            run_dir.join("state.json"),
            &LtoState {
                run_id: "r1".to_string(),
                ..LtoState::default()
            },
        )
        .unwrap();

        cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: Some("r1".to_string()),
                runner: "agy".to_string(),
                payload_file: None,
                cwd: None,
                session_id: None,
                summary: Some("process exited".to_string()),
                rc: Some(7),
                window_id: None,
                source: "agy-process-exit".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "agent.dispatch.completed");
        assert_eq!(events[0]["fields"]["rc"], 7);
        assert_eq!(events[0]["fields"]["completion_scope"], "dispatch");
    }

    #[test]
    fn goal_self_report_rc0_marks_dispatch_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        // state_with_window records runner="codex"; match it so cleanup fires.
        state_with_window(tmp.path(), &tmux_bin, true);

        cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: Some("r1".to_string()),
                runner: "codex".to_string(),
                payload_file: None,
                cwd: None,
                session_id: None,
                summary: Some("self report done".to_string()),
                rc: Some(0),
                window_id: Some("@42".to_string()),
                source: "goal-self-report".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "agent.dispatch.completed");
        assert_eq!(events[0]["fields"]["rc"], 0);
        assert_eq!(events[0]["fields"]["completion_scope"], "dispatch");
        assert_eq!(
            events[0]["fields"]["goal_completion_proof"],
            "goal-self-report"
        );
        assert_eq!(events[0]["fields"]["completion_proof"], "goal-self-report");
        assert_eq!(events[0]["fields"]["source"], "goal-self-report");
        let cleanup = fs::read_to_string(log).unwrap();
        assert!(cleanup.contains("kill-window -t '@42'"));
    }

    #[test]
    fn goal_self_report_rc1_completes_dispatch_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        state_with_window(tmp.path(), &tmux_bin, true);

        cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: Some("r1".to_string()),
                runner: "codex".to_string(),
                payload_file: None,
                cwd: None,
                session_id: None,
                summary: Some("blocked".to_string()),
                rc: Some(1),
                window_id: Some("@42".to_string()),
                source: "goal-self-report".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap();

        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "agent.dispatch.completed");
        assert_eq!(events[0]["fields"]["rc"], 1);
        assert_eq!(events[0]["fields"]["completion_scope"], "dispatch");
        assert_eq!(
            events[0]["fields"]["goal_completion_proof"],
            "goal-self-report"
        );
        // failed self-report retains the window for troubleshooting
        assert!(!log.exists());
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "retained");
    }

    #[test]
    fn goal_self_report_without_run_id_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = cmd_agent_turn_completed(
            tmp.path(),
            AgentTurnOptions {
                run_id: None,
                runner: "pi".to_string(),
                payload_file: None,
                cwd: Some(tmp.path().to_path_buf()),
                session_id: None,
                summary: None,
                rc: Some(0),
                window_id: None,
                source: "goal-self-report".to_string(),
                bell: false,
                notify_cmd: None,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("requires --run-id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn successful_completion_cleans_recorded_window_and_emits_event() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        let mut state = state_with_window(tmp.path(), &tmux_bin, true);

        finish_dispatch_window(
            tmp.path(),
            "r1",
            "codex",
            Some("@42"),
            Some(0),
            Some(&mut state),
        )
        .unwrap();

        let cleanup = fs::read_to_string(log).unwrap();
        assert!(cleanup.starts_with("run-shell -b sleep 0.5;"));
        assert!(cleanup.contains("kill-window -t '@42'"));
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "cleaned");
        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "runner.window.cleaned");
        assert_eq!(events[0]["fields"]["window_id"], "@42");
    }

    #[test]
    fn failed_completion_retains_window() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        let mut state = state_with_window(tmp.path(), &tmux_bin, true);

        finish_dispatch_window(
            tmp.path(),
            "r1",
            "codex",
            Some("@42"),
            Some(7),
            Some(&mut state),
        )
        .unwrap();

        assert!(!log.exists());
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "retained");
        assert_eq!(
            persisted.dispatch_windows[0].retention_reason.as_deref(),
            Some("runner completion rc=7")
        );
    }

    #[test]
    fn keep_window_retains_successful_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let (tmux_bin, log) = fake_tmux(tmp.path());
        let mut state = state_with_window(tmp.path(), &tmux_bin, false);

        finish_dispatch_window(
            tmp.path(),
            "r1",
            "codex",
            Some("@42"),
            Some(0),
            Some(&mut state),
        )
        .unwrap();

        assert!(!log.exists());
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "retained");
        assert_eq!(
            persisted.dispatch_windows[0].retention_reason.as_deref(),
            Some("--keep-window requested")
        );
    }

    #[test]
    fn deferred_cleanup_survives_the_target_pane_exit() {
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let socket = format!("lto-cleanup-test-{}", std::process::id());
        let wrapper = tmp.path().join("tmux-isolated");
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec tmux -L '{}' \"$@\"\n", socket),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&wrapper, permissions).unwrap();
        }
        let tmux = |args: &[&str]| {
            std::process::Command::new(&wrapper)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(
            tmux(&["new-session", "-d", "-s", "cleanup", "-n", "anchor"])
                .status
                .success()
        );
        let created = tmux(&[
            "new-window",
            "-P",
            "-F",
            "#{window_id}.#{pane_index}",
            "-t",
            "cleanup",
            "-n",
            "worker",
        ]);
        let target = String::from_utf8_lossy(&created.stdout).trim().to_string();
        let window_id = target.split('.').next().unwrap().to_string();
        let run_dir = tmp.path().join(".lto").join("r1");
        fs::create_dir_all(&run_dir).unwrap();
        let mut state = LtoState {
            run_id: "r1".to_string(),
            dispatch_windows: vec![DispatchWindowState {
                window_id: window_id.clone(),
                target,
                runner: "codex".to_string(),
                tmux_bin: wrapper.display().to_string(),
                cleanup_on_success: true,
                status: "active".to_string(),
                created_at: crate::state::iso_now(),
                finished_at: None,
                retention_reason: None,
            }],
            ..LtoState::default()
        };
        state::save_state(run_dir.join("state.json"), &state).unwrap();

        finish_dispatch_window(
            tmp.path(),
            "r1",
            "codex",
            Some(&window_id),
            Some(0),
            Some(&mut state),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(900));
        let lookup = tmux(&["display-message", "-p", "-t", &window_id, "#{window_name}"]);
        let _ = tmux(&["kill-server"]);

        assert!(
            String::from_utf8_lossy(&lookup.stdout).trim().is_empty(),
            "worker window should be gone"
        );
        let persisted = load_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(persisted.dispatch_windows[0].status, "cleaned");
        let events = events::read(tmp.path(), "r1").unwrap();
        assert_eq!(events[0]["type"], "runner.window.cleaned");
        assert_eq!(events[0]["fields"]["scheduled"], true);
    }
}
