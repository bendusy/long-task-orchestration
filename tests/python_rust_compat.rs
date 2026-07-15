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

    let phase_check = run_ok(
        Command::new(lto_rs).args([
            "--repo",
            work.to_str().unwrap(),
            "check",
            "--run-id",
            "legacy-fixture-run",
            "--to",
            "implementation",
            "--strict",
            "--json",
        ]),
        "strict implementation check for legacy fixture",
    );
    let phase_json: Value = serde_json::from_str(&phase_check).unwrap();
    assert!(phase_json["check"]["errors"].as_array().unwrap().is_empty());
    assert!(
        !phase_json["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| {
                check["id"] == "delivery_contract_complete" && check["status"] == "missing"
            })
    );

    #[cfg(unix)]
    {
        install_passing_healthcheck(&work);
        let state_path = work
            .join(".lto")
            .join("legacy-fixture-run")
            .join("state.json");
        let state_before = fs::read(&state_path).unwrap();
        let preflight = run_ok(
            Command::new(lto_rs).args([
                "--repo",
                work.to_str().unwrap(),
                "preflight",
                "--json",
                "--run-id",
                "legacy-fixture-run",
            ]),
            "preflight readiness for legacy fixture",
        );
        let preflight_json: Value = serde_json::from_str(&preflight).unwrap();
        assert_eq!(preflight_json["environment"]["ok"], true);
        assert_eq!(preflight_json["run_readiness"]["ok"], true);
        assert_eq!(
            preflight_json["run_readiness"]["missing"],
            serde_json::json!([])
        );
        assert!(preflight_json["run_readiness"]["warnings"].is_array());
        assert_eq!(fs::read(state_path).unwrap(), state_before);
    }
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

#[cfg(unix)]
fn install_passing_healthcheck(repo: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let runners = repo.join("scripts").join("delegate").join("runners");
    fs::create_dir_all(&runners).unwrap();
    let script = runners.join("healthcheck.sh");
    fs::write(
        &script,
        r#"#!/usr/bin/env bash
set -euo pipefail
shift
printf '['
first=1
for runner in "$@"; do
  if [ "$first" -eq 0 ]; then printf ','; fi
  first=0
  printf '{"agent":"%s","verdict":"OK"}' "$runner"
done
printf ']'
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(script, permissions).unwrap();
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
