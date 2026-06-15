use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn python_written_run_is_readable_by_rust_recap_resume_and_check() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    fs::create_dir_all(&work).unwrap();
    init_git_repo(&work);

    let python = python_bin();
    let lto_py = repo_root.join("scripts").join("lto_run.py");
    run_ok(
        Command::new(&python).arg(&lto_py).args([
            "--repo",
            work.to_str().unwrap(),
            "start",
            "--run-id",
            "compat-run",
            "--goal",
            "compatibility run",
            "--why",
            "verify rust can read python state",
            "--done-when",
            "rust recap resume check pass",
        ]),
        "python start",
    );
    run_ok(
        Command::new(&python).arg(&lto_py).args([
            "--repo",
            work.to_str().unwrap(),
            "task-add",
            "--run-id",
            "compat-run",
            "--task-id",
            "T1",
            "--title",
            "record evidence",
            "--command",
            "git status --short",
        ]),
        "python task-add",
    );
    run_ok(
        Command::new(&python).arg(&lto_py).args([
            "--repo",
            work.to_str().unwrap(),
            "runner",
            "--run-id",
            "compat-run",
            "--task-id",
            "T1",
            "--kind",
            "test",
            "--command",
            "git status --short",
        ]),
        "python runner",
    );

    let lto_rs = env!("CARGO_BIN_EXE_lto-rs");
    let recap = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "recap",
            "--run-id",
            "compat-run",
        ]),
        "rust recap",
    );
    assert!(recap.contains("compatibility run"));
    assert!(recap.contains("已完成 1 项"));

    let resume = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "resume",
            "--run-id",
            "compat-run",
        ]),
        "rust resume",
    );
    assert!(resume.contains("Run ID: compat-run"));
    assert!(resume.contains("Tasks: T1:done"));

    let check = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "check",
            "--run-id",
            "compat-run",
            "--json",
        ]),
        "rust check",
    );
    let json: Value = serde_json::from_str(&check).unwrap();
    assert_eq!(json["run_id"], "compat-run");
    assert!(json["check"]["errors"].as_array().unwrap().is_empty());
    assert!(json["check"]["warnings"].is_array());
}

fn init_git_repo(repo: &Path) {
    run_ok(
        Command::new("git").args(["init"]).current_dir(repo),
        "git init",
    );
    run_ok(
        Command::new("git")
            .args(["config", "user.email", "lto@example.test"])
            .current_dir(repo),
        "git config email",
    );
    run_ok(
        Command::new("git")
            .args(["config", "user.name", "LTO Test"])
            .current_dir(repo),
        "git config name",
    );
    fs::write(repo.join("README.md"), "compat\n").unwrap();
    run_ok(
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(repo),
        "git add",
    );
    run_ok(
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo),
        "git commit",
    );
}

fn python_bin() -> String {
    for candidate in ["python3", "python"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return candidate.to_string();
        }
    }
    "python3".to_string()
}

fn run_ok(cmd: &mut Command, label: &str) -> String {
    let output = cmd.output().unwrap_or_else(|err| panic!("{label}: {err}"));
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
