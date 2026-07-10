use crate::events::{self, EventRecord};
use crate::state;
use anyhow::Context;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
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
    let Some(run_id) = route_run(repo, options.run_id.as_deref(), cwd.as_deref())? else {
        println!("agent.turn.completed ignored: no matching LTO run");
        return Ok(());
    };
    let run_state = load_run_state(repo, &run_id);
    let phase = run_state
        .as_ref()
        .map(|state| state.current_phase.clone())
        .filter(|phase| !phase.trim().is_empty());
    let notify_cmd = options
        .notify_cmd
        .clone()
        .or_else(|| run_state.and_then(|state| state.notify_cmd));

    let payload_hash = if payload_text.trim().is_empty() {
        None
    } else {
        Some(format!("{:x}", Sha256::digest(payload_text.as_bytes())))
    };
    let fields = json!({
        "runner": options.runner,
        "cwd": cwd.as_ref().map(|path| path.display().to_string()),
        "session_id": session_id,
        "rc": options.rc,
        "source": options.source,
        "payload_sha256": payload_hash,
        "known_payload_schema": payload.is_some(),
    });
    let runner_name = options.runner.clone();
    let summary_text = summary.clone();
    let event = events::safe_emit(
        repo,
        &run_id,
        EventRecord {
            event_type: "agent.turn.completed".to_string(),
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
        println!("agent.turn.completed emitted for run {run_id}");
    } else {
        println!("agent.turn.completed dropped for run {run_id}");
    }

    // Last hop: signal that the turn is done. All best-effort and never fail the
    // command (Hook Shim discipline — a notifier must not stall/crash the turn).
    // 1. Wake any `lto events --wait` waiter for this run (machine -> machine).
    crate::notify::wake_run(repo, &run_id);
    // 2. Ring the bell so a watching human notices (machine -> local human).
    if options.bell {
        print!("\x07");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    // 3. Run the host-supplied notifier, e.g. iaf (machine -> remote human).
    if let Some(template) = notify_cmd.as_deref() {
        run_notify_cmd(template, &run_id, &runner_name, &summary_text, options.rc);
    }
    Ok(())
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
    use crate::state::{LtoState, WorkspaceSnapshot};

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
        assert_eq!(fs::read_to_string(notified).unwrap(), "done");
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
}
