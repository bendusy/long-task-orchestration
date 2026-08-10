use lto_rs::state::{DispatchWindowState, LtoState, save_state};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write_state(repo: &Path, window: Option<(&str, &str)>) {
    let run_dir = repo.join(".lto").join("r1");
    fs::create_dir_all(&run_dir).unwrap();
    let dispatch_windows = window
        .map(|(id, target)| {
            vec![DispatchWindowState {
                window_id: id.to_string(),
                target: target.to_string(),
                runner: "codex".to_string(),
                backend: "herdr".to_string(),
                tmux_bin: "tmux".to_string(),
                cleanup_on_success: true,
                status: "active".to_string(),
                created_at: lto_rs::state::iso_now(),
                finished_at: None,
                retention_reason: None,
            }]
        })
        .unwrap_or_default();
    save_state(
        run_dir.join("state.json"),
        &LtoState {
            run_id: "r1".to_string(),
            dispatch_windows,
            ..LtoState::default()
        },
    )
    .unwrap();
}

fn fake_herdr(repo: &Path) -> PathBuf {
    let bin = repo.join("fake-herdr");
    let log = repo.join("herdr.log");
    let capture = repo.join("capture.txt");
    let read_count = repo.join("read-count");
    fs::write(&capture, "Goal active\nWorking\n").unwrap();
    let script = format!(
        r#"#!/bin/sh
set -eu
log='{log}'
capture='{capture}'
read_count='{read_count}'
printf '%s\n' "$*" >> "$log"
case "$1 $2" in
  'status --json')
    if [ "${{FAKE_SERVER_DOWN:-0}}" = 1 ]; then
      printf '{{"server":{{"running":false}}}}\n'
    else
      printf '{{"server":{{"running":true,"status":"running"}}}}\n'
    fi
    ;;
  'tab create')
    printf '{{"result":{{"root_pane":{{"pane_id":"w1:p1"}}}}}}\n'
    ;;
  'pane get') printf '{{"result":{{"pane_id":"w1:p1"}}}}\n' ;;
  'pane run')
    case "$*" in *codex*) touch '{agent}' ;; esac
    ;;
  'agent get')
    if [ -e '{agent}' ]; then printf '{{"result":{{"agent":"codex"}}}}\n'; else printf '{{"error":{{"code":"agent_not_found"}}}}\n'; exit 1; fi
    ;;
  'agent wait') printf '{{"result":{{"status":"idle"}}}}\n' ;;
  'agent prompt') printf '{{"result":{{"status":"working"}}}}\n' ;;
  'pane read')
    count=0
    if [ -f "$read_count" ]; then count=$(cat "$read_count"); fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$read_count"
    if [ "$count" -gt 1 ]; then cat "$capture"; fi
    ;;
  'pane report-metadata') printf '{{"result":{{"type":"ok"}}}}\n' ;;
  'pane close')
    if [ "${{FAKE_MISSING_CLOSE:-0}}" = 1 ]; then printf '{{"error":{{"code":"pane_not_found"}}}}\n'; exit 1; fi
    if [ "${{FAKE_CLOSE_ERROR:-0}}" = 1 ]; then printf '{{"error":{{"code":"server_unavailable"}}}}\n' >&2; exit 1; fi
    printf '{{"result":{{"type":"ok"}}}}\n'
    ;;
esac
"#,
        log = log.display(),
        capture = capture.display(),
        read_count = read_count.display(),
        agent = repo.join("agent-ready").display(),
    );
    fs::write(&bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
    }
    bin
}

fn dispatch_command(repo: &Path, fake: &Path, goal: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lto-rs"))
        .args([
            "--repo",
            repo.to_str().unwrap(),
            "dispatch-goal",
            "--run-id",
            "r1",
            "--runner",
            "codex",
            "--goal",
            goal.to_str().unwrap(),
            "--backend",
            "herdr",
            "--ready-timeout",
            "5",
            "--no-install-hooks",
            "--no-runner-constraints",
        ])
        .env("LTO_HERDR_BIN", fake)
        .output()
        .unwrap()
}

fn dispatch_command_from_default_repo(
    repo: &Path,
    fake: &Path,
    goal: &Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lto-rs"))
        .current_dir(repo)
        .args([
            "dispatch-goal",
            "--run-id",
            "r1",
            "--runner",
            "codex",
            "--goal",
            goal.to_str().unwrap(),
            "--backend",
            "herdr",
            "--ready-timeout",
            "5",
            "--no-install-hooks",
            "--no-runner-constraints",
        ])
        .env("LTO_HERDR_BIN", fake)
        .output()
        .unwrap()
}

#[test]
fn herdr_dispatch_waits_for_shell_and_uses_pane_run_before_atomic_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), None);
    let fake = fake_herdr(tmp.path());
    let goal = tmp.path().join("goal.md");
    fs::write(&goal, "# Goal\n\nRead the file and stop.\n").unwrap();

    let output = dispatch_command(tmp.path(), &fake, &goal);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(tmp.path().join("herdr.log")).unwrap();
    assert!(log.contains("tab create"));
    assert!(!log.contains("pane send-text"));
    assert!(!log.contains("pane send-keys"));
    let first_run = log.find("pane run").unwrap();
    assert_eq!(log[..first_run].matches("pane read").count(), 2, "{log}");
    assert!(log.contains("pane run w1:p1 export LTO_WINDOW_ID="));
    assert!(log.contains("pane run w1:p1 export LTO_REPO="));
    assert!(log.contains("pane run w1:p1 cd "));
    assert!(log.contains("pane run w1:p1 LTO_RUN_ID="));
    assert!(log.contains("agent prompt w1:p1"));
    assert!(log.contains("pane report-metadata w1:p1 --source lto"));
    assert!(log.contains("run_id=r1"));
    assert!(log.contains("goal=goal.dispatch.md"));
    let state = lto_rs::state::load_state(tmp.path().join(".lto/r1/state.json")).unwrap();
    assert_eq!(state.dispatch_windows[0].window_id, "w1:p1");
}

#[test]
fn herdr_dispatch_absolutizes_default_repo_for_server_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), None);
    let fake = fake_herdr(tmp.path());
    let goal = tmp.path().join("goal.md");
    fs::write(&goal, "# Goal\n\nRead the file and stop.\n").unwrap();

    let output = dispatch_command_from_default_repo(tmp.path(), &fake, &goal);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(tmp.path().join("herdr.log")).unwrap();
    let expected_cwd = tmp.path().canonicalize().unwrap();
    assert!(
        log.contains(&format!("tab create --cwd {}/.", expected_cwd.display())),
        "{log}"
    );
}

#[test]
fn herdr_dispatch_fails_closed_when_server_is_down() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), None);
    let fake = fake_herdr(tmp.path());
    let goal = tmp.path().join("goal.md");
    fs::write(&goal, "# Goal\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_lto-rs"))
        .args([
            "--repo",
            tmp.path().to_str().unwrap(),
            "dispatch-goal",
            "--run-id",
            "r1",
            "--runner",
            "codex",
            "--goal",
            goal.to_str().unwrap(),
            "--backend",
            "herdr",
            "--no-install-hooks",
            "--no-runner-constraints",
        ])
        .env("LTO_HERDR_BIN", fake)
        .env("FAKE_SERVER_DOWN", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("start herdr") || stderr.contains("default tmux backend"));
    let log = fs::read_to_string(tmp.path().join("herdr.log")).unwrap();
    assert!(!log.contains("tab create"));
}

#[test]
fn herdr_blocked_pattern_is_read_from_pane() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), None);
    let fake = fake_herdr(tmp.path());
    fs::write(tmp.path().join("capture.txt"), "Trust all and continue\n").unwrap();
    let goal = tmp.path().join("goal.md");
    fs::write(&goal, "# Goal\n").unwrap();
    let output = dispatch_command(tmp.path(), &fake, &goal);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("matched \"Trust all and continue\""),
        "{stderr}"
    );
    let state = lto_rs::state::load_state(tmp.path().join(".lto/r1/state.json")).unwrap();
    assert_eq!(state.dispatch_windows[0].status, "retained");
    assert!(
        state.dispatch_windows[0]
            .retention_reason
            .as_deref()
            .is_some_and(
                |reason| reason.contains("dispatch failed") && reason.contains("Trust all")
            )
    );
}

#[test]
fn herdr_missing_finish_target_warns_and_marks_state_cleaned() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), Some(("w1:p1", "w1:p1")));
    let fake = fake_herdr(tmp.path());
    let output = Command::new(env!("CARGO_BIN_EXE_lto-rs"))
        .args([
            "--repo",
            tmp.path().to_str().unwrap(),
            "agent-turn-completed",
            "--run-id",
            "r1",
            "--runner",
            "codex",
            "--source",
            "goal-self-report",
            "--rc",
            "0",
            "--window-id",
            "w1:p1",
        ])
        .env("LTO_HERDR_BIN", fake)
        .env("FAKE_MISSING_CLOSE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("already absent"));
    let state = lto_rs::state::load_state(tmp.path().join(".lto/r1/state.json")).unwrap();
    assert_eq!(state.dispatch_windows[0].status, "cleaned");
}

#[test]
fn herdr_finish_close_error_marks_state_retained() {
    let tmp = tempfile::tempdir().unwrap();
    write_state(tmp.path(), Some(("w1:p1", "w1:p1")));
    let fake = fake_herdr(tmp.path());
    let output = Command::new(env!("CARGO_BIN_EXE_lto-rs"))
        .args([
            "--repo",
            tmp.path().to_str().unwrap(),
            "agent-turn-completed",
            "--run-id",
            "r1",
            "--runner",
            "codex",
            "--source",
            "goal-self-report",
            "--rc",
            "0",
            "--window-id",
            "w1:p1",
        ])
        .env("LTO_HERDR_BIN", fake)
        .env("FAKE_CLOSE_ERROR", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("window w1:p1 retained"), "{stderr}");
    let state = lto_rs::state::load_state(tmp.path().join(".lto/r1/state.json")).unwrap();
    assert_eq!(state.dispatch_windows[0].status, "retained");
    assert!(
        state.dispatch_windows[0]
            .retention_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("herdr cleanup failed"))
    );
}
