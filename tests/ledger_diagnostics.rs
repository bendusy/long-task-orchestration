use serde_json::Value;
use std::fs;
use std::process::{Command, Output};

#[test]
fn check_json_reports_alternating_diagnostics_without_changing_gate_result() {
    let fixture = Fixture::new();
    fixture.write_ledger(&[5, 2, 4, 1, 3]);

    let output = fixture.check_json();
    assert!(output.status.success(), "{}", stderr(&output));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["check"]["errors"].as_array().unwrap().is_empty());
    assert_eq!(json["ledger"]["diagnostics"]["oscillation"], "alternating");
    assert_eq!(json["ledger"]["diagnostics"]["envelope"], "shrinking");
    assert_eq!(
        json["ledger"]["diagnostics"]["confidence"],
        "advisory (lineage recorded)"
    );
    assert!(
        json["ledger"]["advisory"]
            .as_str()
            .unwrap()
            .contains("change hypothesis")
    );
}

#[test]
fn check_json_keeps_terminal_zero_verdict_and_suppresses_single_rebound_advisory() {
    let fixture = Fixture::new();
    fixture.write_ledger(&[1, 2, 0]);

    let output = fixture.check_json();
    assert!(output.status.success(), "{}", stderr(&output));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ledger"]["verdict"], "CONVERGED");
    assert_eq!(
        json["ledger"]["diagnostics"]["oscillation"],
        "single_rebound"
    );
    assert_eq!(json["ledger"]["diagnostics"]["terminal"], "zero");
    assert!(json["ledger"].get("advisory").is_none());
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        run_ok(Command::new("git").args(["init", "-q"]).current_dir(&repo));
        run_ok(
            Command::new("git")
                .args(["config", "user.email", "lto@example.test"])
                .current_dir(&repo),
        );
        run_ok(
            Command::new("git")
                .args(["config", "user.name", "LTO Test"])
                .current_dir(&repo),
        );
        run_ok(
            Command::new("git")
                .args(["commit", "--allow-empty", "-q", "-m", "init"])
                .current_dir(&repo),
        );
        run_ok(Command::new(env!("CARGO_BIN_EXE_lto-rs")).args([
            "--repo",
            repo.to_str().unwrap(),
            "start",
            "--run-id",
            "r1",
            "--goal",
            "diagnostics test",
            "--entropy-check",
            "change hypothesis",
        ]));
        Self { _tmp: tmp, repo }
    }

    fn write_ledger(&self, blockers: &[u64]) {
        let mut rows = String::new();
        for (index, blockers) in blockers.iter().enumerate() {
            rows.push_str(&format!(
                "| R{} | reply | agy pi | T1 | {} | 0 | 0 | flat | open |\n",
                index + 1,
                blockers
            ));
        }
        let ledger = format!(
            "## Round Summary\n| round | artifact | auditors | coverage | high | critical | minor | trend | status |\n|---|---|---|---|---:|---:|---:|---|---|\n{rows}"
        );
        fs::write(self.run_dir().join("audit-ledger.md"), ledger).unwrap();
    }

    fn check_json(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lto-rs"))
            .args([
                "--repo",
                self.repo.to_str().unwrap(),
                "check",
                "--run-id",
                "r1",
                "--json",
            ])
            .output()
            .unwrap()
    }

    fn run_dir(&self) -> std::path::PathBuf {
        self.repo.join(".lto").join("r1")
    }
}

fn run_ok(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
