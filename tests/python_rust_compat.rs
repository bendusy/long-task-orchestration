use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn fixed_legacy_run_fixture_is_readable_by_rust_recap_resume_and_check() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    fs::create_dir_all(&work).unwrap();
    init_git_repo(&work);
    install_legacy_fixture(&work, "legacy-fixture-run");

    let lto_rs = env!("CARGO_BIN_EXE_lto-rs");
    let recap = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "recap",
            "--run-id",
            "legacy-fixture-run",
        ]),
        "rust recap legacy fixture",
    );
    assert!(recap.contains("legacy Python run compatibility"));
    assert!(recap.contains("已完成 1 项"));

    let resume = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "resume",
            "--run-id",
            "legacy-fixture-run",
        ]),
        "rust resume legacy fixture",
    );
    assert!(resume.contains("Run ID: legacy-fixture-run"));
    assert!(resume.contains("Tasks: OLD1:done"));

    let check = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "check",
            "--run-id",
            "legacy-fixture-run",
            "--json",
        ]),
        "rust check legacy fixture",
    );
    let json: Value = serde_json::from_str(&check).unwrap();
    assert_eq!(json["run_id"], "legacy-fixture-run");
    assert!(json["check"]["errors"].as_array().unwrap().is_empty());
    assert!(json["check"]["warnings"].is_array());
    assert_eq!(json["ledger"]["verdict"], "CONVERGED");
    assert_eq!(
        json["ledger"]["diagnostics"]["confidence"],
        "low (no lineage)"
    );
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

fn install_legacy_fixture(repo: &Path, run_id: &str) {
    let run_dir = repo.join(".lto").join(run_id);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(repo.join(".lto").join("current"), format!("{run_id}\n")).unwrap();
    let fixture = include_str!("fixtures/legacy-run/state.json");
    fs::write(
        run_dir.join("state.json"),
        fixture.replace("__HEAD__", &git_head(repo)),
    )
    .unwrap();
    fs::write(
        run_dir.join("audit-ledger.md"),
        include_str!("fixtures/legacy-run/audit-ledger.md"),
    )
    .unwrap();
}

fn git_head(repo: &Path) -> String {
    run_ok(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo),
        "git rev-parse HEAD",
    )
    .trim()
    .to_string()
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
