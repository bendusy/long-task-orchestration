use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[test]
fn python_proxy_and_rust_match_three_golden_ledgers() {
    for (fixture, strict, expected_rc) in [
        ("terminal-zero.md", false, 0),
        ("rebound.md", false, 1),
        ("stalled.md", true, 1),
    ] {
        let path = fixture_path(fixture);
        let rust = rust_check(&path, strict);
        let python = python_check(&path, strict);
        assert_eq!(rust.status.code(), Some(expected_rc), "rust {fixture}");
        assert_eq!(python.status.code(), Some(expected_rc), "python {fixture}");
        assert_eq!(python.stdout, rust.stdout, "stdout drift for {fixture}");
        assert!(stderr(&python).contains("compatibility proxy"));
    }
}

#[test]
fn ledger_only_strictness_and_errors_preserve_legacy_exit_contract() {
    let stalled = fixture_path("stalled.md");
    assert_eq!(rust_check(&stalled, false).status.code(), Some(0));
    assert_eq!(rust_check(&stalled, true).status.code(), Some(1));

    let missing = fixture_path("missing.md");
    let rust = rust_check(&missing, false);
    let python = python_check(&missing, false);
    assert_eq!(rust.status.code(), Some(2));
    assert_eq!(python.status.code(), Some(2));
    assert!(stderr(&rust).contains("ERROR"));
    assert!(stderr(&python).contains("ERROR"));
}

#[test]
fn python_self_test_is_a_rust_owned_golden_check() {
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/audit_ledger_check.py"))
        .arg("self-test")
        .env("LTO_BIN", env!("CARGO_BIN_EXE_lto-rs"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("verdict: CONVERGED"));
}

fn rust_check(path: &Path, strict: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lto-rs"));
    command.args(["check", "--ledger"]).arg(path);
    if strict {
        command.arg("--strict");
    }
    command.output().unwrap()
}

fn python_check(path: &Path, strict: bool) -> Output {
    let mut command = Command::new("python3");
    command
        .arg(repo_root().join("scripts/audit_ledger_check.py"))
        .arg(path)
        .env("LTO_BIN", env!("CARGO_BIN_EXE_lto-rs"));
    if strict {
        command.arg("--strict");
    }
    command.output().unwrap()
}

fn fixture_path(name: &str) -> PathBuf {
    repo_root().join("tests/fixtures/audit-ledger").join(name)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
