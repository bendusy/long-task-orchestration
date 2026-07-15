use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const RUN_ID: &str = "c2-run";

#[test]
fn start_rejects_missing_goal_before_any_lto_write() {
    assert_start_rejected_before_write(
        &["start", "--run-id", RUN_ID, "--done-when", "tests pass"],
        &["--goal"],
    );
}

#[test]
fn start_rejects_missing_done_when_before_any_lto_write() {
    assert_start_rejected_before_write(
        &["start", "--run-id", RUN_ID, "--goal", "ship C2"],
        &["--done-when"],
    );
}

#[test]
fn start_rejects_both_missing_readiness_fields_before_any_lto_write() {
    assert_start_rejected_before_write(&["start", "--run-id", RUN_ID], &["--goal", "--done-when"]);
}

#[test]
fn start_treats_whitespace_readiness_fields_as_missing() {
    for (args, missing) in [
        (
            vec![
                "start",
                "--run-id",
                RUN_ID,
                "--goal",
                "   \t",
                "--done-when",
                "tests pass",
            ],
            "--goal",
        ),
        (
            vec![
                "start",
                "--run-id",
                RUN_ID,
                "--goal",
                "ship C2",
                "--done-when",
                "  \t ",
            ],
            "--done-when",
        ),
    ] {
        assert_start_rejected_before_write(&args, &[missing]);
    }
}

#[test]
fn start_force_does_not_bypass_readiness() {
    assert_start_rejected_before_write(
        &["start", "--run-id", RUN_ID, "--goal", "ship C2", "--force"],
        &["--done-when"],
    );
}

#[test]
fn start_accepts_empty_delivery_contract() {
    let repo = RepoFixture::new();
    let output = repo.start(&[]);
    assert_success(&output, "start with an empty delivery contract");

    let state = repo.state();
    assert!(
        state.get("delivery_contract").is_none()
            || delivery_values(&state, "targets").is_empty()
                && delivery_values(&state, "constraints").is_empty()
                && delivery_values(&state, "instruments").is_empty()
                && delivery_values(&state, "forced_entropy").is_empty()
    );
}

#[test]
fn start_rejects_target_without_instrument_and_names_only_missing_flag() {
    assert_partial_start_rejected(
        &["--target", "ship measurable behavior"],
        "--instrument",
        "--target",
    );
}

#[test]
fn start_rejects_instrument_without_target_and_names_only_missing_flag() {
    assert_partial_start_rejected(
        &["--instrument", "cargo test --locked --all-targets"],
        "--target",
        "--instrument",
    );
}

#[test]
fn start_rejects_optional_only_contract_before_any_lto_write() {
    for (flag, value) in [
        ("--constraint", "bounded scope"),
        ("--entropy-check", "change hypothesis"),
    ] {
        let repo = RepoFixture::new();
        let output = repo.start(&["--host", "codex", flag, value]);
        assert_failure(&output, "start with optional-only delivery contract");
        let stderr = stderr(&output);
        assert!(stderr.contains("--target"), "{stderr}");
        assert!(stderr.contains("--instrument"), "{stderr}");
        assert!(
            !repo.root.join(".lto").exists(),
            "optional-only contract wrote .lto before validation"
        );
    }
}

#[test]
fn start_rejects_empty_labeled_instruments_before_any_lto_write() {
    for invalid_instrument in ["label::", "::"] {
        let repo = RepoFixture::new();
        let output = repo.start(&[
            "--host",
            "codex",
            "--target",
            "ship measurable behavior",
            "--instrument",
            invalid_instrument,
        ]);
        assert_failure(&output, "start with an empty labeled instrument");
        let stderr = stderr(&output);
        assert!(stderr.contains("--instrument"), "{stderr}");
        assert!(!stderr.contains("--target"), "{stderr}");
        assert!(
            !repo.root.join(".lto").exists(),
            "invalid labeled instrument wrote .lto before validation"
        );
    }
}

#[test]
fn start_accepts_paired_contract_without_optional_sections_and_warns() {
    let repo = RepoFixture::new();
    let output = repo.start(&[
        "--host",
        "codex",
        "--target",
        "ship measurable behavior",
        "--instrument",
        "cargo test --locked --all-targets",
    ]);
    assert_success(&output, "start with paired target and instrument");
    let stderr = stderr(&output);
    assert!(stderr.contains("WARN"), "expected WARN, got:\n{stderr}");
    assert!(stderr.contains("--constraint"), "{stderr}");
    assert!(stderr.contains("--entropy-check"), "{stderr}");
}

#[test]
fn start_accepts_complete_delivery_contract() {
    let repo = RepoFixture::new();
    let output = repo.start(&[
        "--host",
        "codex",
        "--target",
        "ship measurable behavior",
        "--constraint",
        "macOS and Linux first",
        "--instrument",
        "cargo test --locked --all-targets",
        "--entropy-check",
        "change hypothesis after a stall",
    ]);
    assert_success(&output, "start with complete delivery contract");

    let state = repo.state();
    assert_eq!(
        delivery_values(&state, "targets"),
        vec!["ship measurable behavior"]
    );
    assert_eq!(
        delivery_values(&state, "constraints"),
        vec!["macOS and Linux first"]
    );
    assert_eq!(
        delivery_values(&state, "instruments"),
        vec!["cargo test --locked --all-targets"]
    );
    assert_eq!(
        delivery_values(&state, "forced_entropy"),
        vec!["change hypothesis after a stall"]
    );
}

#[test]
fn start_defaults_unknown_host_and_emits_advisory() {
    let repo = RepoFixture::new();
    let output = repo.start(&[]);
    assert_success(&output, "start without an explicit host");
    assert_eq!(repo.state()["host_runtime"], "unknown");
    let stderr = stderr(&output);
    assert!(stderr.contains("WARN"), "expected host advisory:\n{stderr}");
    assert!(stderr.contains("--host"), "{stderr}");
}

#[test]
fn audit_unknown_host_warns_and_keeps_the_full_auditor_pool() {
    let repo = RepoFixture::new();
    let start = repo.start(&[]);
    assert_success(&start, "start without an explicit host");

    let audit = repo.lto(&["audit", "--run-id", RUN_ID]);
    assert_success(&audit, "prepare audit with unknown host");
    let stderr = stderr(&audit);
    assert!(stderr.contains("host runtime is unknown"), "{stderr}");
    assert!(stderr.contains("--host"), "{stderr}");
    let report = parse_json_stdout(&audit, "audit prepare with unknown host");
    assert_eq!(report["host"], "unknown");
    assert_eq!(report["auditors"], json!(["codex", "pi", "agy"]));
}

#[test]
fn contract_set_backfills_empty_contract_and_emits_typed_event() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    let mut legacy_state = repo.state();
    assert!(legacy_state.get("delivery_contract").is_none());
    legacy_state["goal"] = json!("");
    legacy_state["done_when"] = json!("");
    legacy_state["host_runtime"] = json!("");
    repo.write_state(&legacy_state);

    let output = repo.lto(&[
        "contract",
        "set",
        "--goal",
        "repaired legacy goal",
        "--done-when",
        "legacy acceptance passes",
        "--host",
        "codex",
        "--target",
        "ship measurable behavior",
        "--constraint",
        "macOS and Linux first",
        "--instrument",
        "smoke::cargo test --locked --all-targets",
        "--entropy-check",
        "change hypothesis after a stall",
    ]);
    assert_success(&output, "contract set backfill");

    let state = repo.state();
    assert_eq!(state["goal"], "repaired legacy goal");
    assert_eq!(state["done_when"], "legacy acceptance passes");
    assert_eq!(state["host_runtime"], "codex");
    assert_eq!(
        delivery_values(&state, "targets"),
        vec!["ship measurable behavior"]
    );
    assert_eq!(
        delivery_values(&state, "instruments"),
        vec!["smoke::cargo test --locked --all-targets"]
    );
    assert!(
        repo.events()
            .iter()
            .any(|event| { event.get("type").and_then(Value::as_str) == Some("contract.updated") })
    );

    let check = repo.check_implementation();
    assert_success(&check, "strict check after contract backfill");
    let report = parse_json_stdout(&check, "strict check after contract backfill");
    let delivery = delivery_check(&report).expect("delivery contract check after backfill");
    assert_eq!(delivery["status"], "ok");
    assert_eq!(delivery["required"], true);
}

#[test]
fn contract_set_replaces_invalid_legacy_instruments_through_typed_cli() {
    let repo = RepoFixture::new();
    repo.start_ok(&[
        "--host",
        "codex",
        "--target",
        "legacy measurable target",
        "--instrument",
        "initial::true",
    ]);
    let mut state = repo.state();
    state["delivery_contract"]["instruments"] = json!(["legacy-label::"]);
    repo.write_state(&state);

    let output = repo.lto(&[
        "contract",
        "set",
        "--run-id",
        RUN_ID,
        "--replace-instrument",
        "repaired::true",
    ]);
    assert_success(&output, "replace invalid legacy instrument");
    assert_eq!(
        delivery_values(&repo.state(), "instruments"),
        vec!["repaired::true"]
    );
    let check = repo.check_implementation();
    assert_success(&check, "strict check after legacy instrument repair");
    let report = parse_json_stdout(&check, "strict check after legacy instrument repair");
    assert_eq!(
        delivery_check(&report).expect("delivery check")["status"],
        "ok"
    );
}

#[test]
fn contract_set_rejects_half_contracts_without_writing_state_or_event() {
    for (flag, value, expected_missing, unexpected_flag) in [
        ("--target", "unmeasured target", "--instrument", "--target"),
        (
            "--instrument",
            "cargo test --locked --all-targets",
            "--target",
            "--instrument",
        ),
    ] {
        let repo = RepoFixture::new();
        repo.start_ok(&["--host", "codex"]);
        let state_before = fs::read(repo.state_path()).unwrap();
        let events_before = fs::read(repo.events_path()).unwrap();

        let output = repo.lto(&["contract", "set", "--run-id", RUN_ID, flag, value]);
        assert_failure(&output, "contract set with a half contract");
        let stderr = stderr(&output);
        assert!(stderr.contains(expected_missing), "{stderr}");
        assert!(!stderr.contains(unexpected_flag), "{stderr}");
        assert_eq!(fs::read(repo.state_path()).unwrap(), state_before);
        assert_eq!(fs::read(repo.events_path()).unwrap(), events_before);
    }
}

#[test]
fn contract_set_rejects_optional_only_contract_without_writing() {
    for (flag, value) in [
        ("--constraint", "bounded scope"),
        ("--entropy-check", "change hypothesis"),
    ] {
        let repo = RepoFixture::new();
        repo.start_ok(&["--host", "codex"]);
        let state_before = fs::read(repo.state_path()).unwrap();
        let events_before = fs::read(repo.events_path()).unwrap();

        let output = repo.lto(&["contract", "set", "--run-id", RUN_ID, flag, value]);

        assert_failure(&output, "contract set with optional-only contract");
        let stderr = stderr(&output);
        assert!(stderr.contains("--target"), "{stderr}");
        assert!(stderr.contains("--instrument"), "{stderr}");
        assert_eq!(fs::read(repo.state_path()).unwrap(), state_before);
        assert_eq!(fs::read(repo.events_path()).unwrap(), events_before);
    }
}

#[test]
fn contract_set_rejects_empty_labeled_instrument_without_writing_state_or_event() {
    let repo = RepoFixture::new();
    repo.start_ok(&[
        "--host",
        "codex",
        "--target",
        "baseline target",
        "--instrument",
        "baseline::true",
    ]);
    let state_before = fs::read(repo.state_path()).unwrap();
    let events_before = fs::read(repo.events_path()).unwrap();

    for invalid_instrument in ["label::", "::"] {
        let output = repo.lto(&[
            "contract",
            "set",
            "--run-id",
            RUN_ID,
            "--instrument",
            invalid_instrument,
        ]);
        assert_failure(&output, "contract set with an empty labeled instrument");
        let stderr = stderr(&output);
        assert!(stderr.contains("--instrument"), "{stderr}");
        assert!(!stderr.contains("--target"), "{stderr}");
        assert_eq!(fs::read(repo.state_path()).unwrap(), state_before);
        assert_eq!(fs::read(repo.events_path()).unwrap(), events_before);
    }
}

#[test]
fn contract_set_appends_repeated_values_and_repairs_scalar_metadata() {
    let repo = RepoFixture::new();
    repo.start_ok(&[
        "--host",
        "codex",
        "--why",
        "initial repair reason",
        "--target",
        "baseline target",
        "--instrument",
        "baseline::true",
    ]);

    let output = repo.lto(&[
        "contract",
        "set",
        "--run-id",
        RUN_ID,
        "--goal",
        "repaired goal",
        "--done-when",
        "repaired acceptance passes",
        "--host",
        "pi",
        "--target",
        "second target",
        "--target",
        "third target",
        "--instrument",
        "smoke::cargo test",
        "--instrument",
        "lint::cargo clippy -- -D warnings",
    ]);
    assert_success(&output, "contract set repeated append and scalar repair");

    let state = repo.state();
    assert_eq!(state["goal"], "repaired goal");
    assert_eq!(state["done_when"], "repaired acceptance passes");
    assert_eq!(state["host_runtime"], "pi");
    assert_eq!(
        delivery_values(&state, "targets"),
        vec!["baseline target", "second target", "third target"]
    );
    assert_eq!(
        delivery_values(&state, "instruments"),
        vec![
            "baseline::true",
            "smoke::cargo test",
            "lint::cargo clippy -- -D warnings"
        ]
    );
    let run_state = fs::read_to_string(repo.run_state_path()).unwrap();
    assert!(
        run_state.contains("- feature / goal: repaired goal"),
        "{run_state}"
    );
    assert!(run_state.contains("- host_runtime: pi"), "{run_state}");
    assert!(
        run_state.contains("- delivery_targets: baseline target | second target | third target"),
        "{run_state}"
    );
    assert!(
        run_state.contains(
            "- delivery_instruments: baseline::true | smoke::cargo test | lint::cargo clippy -- -D warnings"
        ),
        "{run_state}"
    );
    assert!(
        run_state.contains("- done_when: repaired acceptance passes"),
        "{run_state}"
    );
    if include_str!("../templates/run-state.md")
        .lines()
        .any(|line| line.trim_start().starts_with("- why:"))
    {
        assert!(
            run_state.contains("- why: initial repair reason"),
            "{run_state}"
        );
    }
}

#[test]
fn concurrent_contract_sets_preserve_both_appends_and_events() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);

    let first = repo
        .lto_command(&[
            "contract",
            "set",
            "--run-id",
            RUN_ID,
            "--target",
            "concurrent target one",
            "--instrument",
            "one::true",
        ])
        .spawn()
        .unwrap();
    let second = repo
        .lto_command(&[
            "contract",
            "set",
            "--run-id",
            RUN_ID,
            "--target",
            "concurrent target two",
            "--instrument",
            "two::true",
        ])
        .spawn()
        .unwrap();

    let first_output = first.wait_with_output().unwrap();
    let second_output = second.wait_with_output().unwrap();
    assert_success(&first_output, "first concurrent contract set");
    assert_success(&second_output, "second concurrent contract set");

    let state = repo.state();
    let mut targets = delivery_values(&state, "targets");
    targets.sort_unstable();
    assert_eq!(
        targets,
        vec!["concurrent target one", "concurrent target two"]
    );
    let mut instruments = delivery_values(&state, "instruments");
    instruments.sort_unstable();
    assert_eq!(instruments, vec!["one::true", "two::true"]);
    assert_eq!(
        repo.events()
            .iter()
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some("contract.updated")
            })
            .count(),
        2
    );
}

#[cfg(unix)]
#[test]
fn preflight_record_does_not_overwrite_concurrent_contract_update() {
    use std::thread;
    use std::time::Duration;

    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    repo.install_blocking_healthcheck();

    let preflight = repo
        .lto_command(&["preflight", "--run-id", RUN_ID, "--record"])
        .spawn()
        .unwrap();
    let started = repo.root.join(".preflight-health-started");
    for _ in 0..500 {
        if started.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let health_started = started.exists();
    let contract_output = repo.lto(&[
        "contract",
        "set",
        "--run-id",
        RUN_ID,
        "--target",
        "concurrent target",
        "--instrument",
        "concurrent::true",
    ]);
    fs::write(repo.root.join(".preflight-health-release"), "release\n").unwrap();
    let preflight_output = preflight.wait_with_output().unwrap();

    assert!(health_started, "preflight healthcheck did not start");
    assert_success(&contract_output, "contract update during preflight");
    assert_success(
        &preflight_output,
        "recording preflight after contract update",
    );
    let state = repo.state();
    assert_eq!(
        delivery_values(&state, "targets"),
        vec!["concurrent target"]
    );
    assert_eq!(
        delivery_values(&state, "instruments"),
        vec!["concurrent::true"]
    );
    assert_eq!(state["environment_snapshot"]["preflight_verdict"], "pass");
}

#[cfg(unix)]
#[test]
fn runner_completion_does_not_overwrite_concurrent_contract_update() {
    use std::thread;
    use std::time::Duration;

    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    let task_add = repo.lto(&[
        "task",
        "add",
        "--run-id",
        RUN_ID,
        "--task-id",
        "T1",
        "--title",
        "blocking runner",
    ]);
    assert_success(&task_add, "add runner task");
    let runner_script = repo.install_blocking_runner();
    let runner = repo
        .lto_command(&[
            "runner",
            "--run-id",
            RUN_ID,
            "--task-id",
            "T1",
            "--command",
            runner_script.to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    let started = repo.root.join(".runner-started");
    for _ in 0..500 {
        if started.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let runner_started = started.exists();
    let contract_output = repo.lto(&[
        "contract",
        "set",
        "--run-id",
        RUN_ID,
        "--target",
        "runner-safe target",
        "--instrument",
        "runner-safe::true",
    ]);
    fs::write(repo.root.join(".runner-release"), "release\n").unwrap();
    let runner_output = runner.wait_with_output().unwrap();

    assert!(runner_started, "blocking runner did not start");
    assert_success(&contract_output, "contract update during runner");
    assert_success(&runner_output, "runner completion after contract update");
    let state = repo.state();
    assert_eq!(
        delivery_values(&state, "targets"),
        vec!["runner-safe target"]
    );
    assert_eq!(
        delivery_values(&state, "instruments"),
        vec!["runner-safe::true"]
    );
    let task = state["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "T1")
        .unwrap();
    assert_eq!(task["status"], "done");
    assert!(!task["evidence"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn preflight_text_reports_active_and_explicit_run_without_recording() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    repo.install_passing_healthcheck();
    let state_before = fs::read(repo.state_path()).unwrap();

    for args in [vec!["preflight"], vec!["preflight", "--run-id", RUN_ID]] {
        let output = repo.lto(&args);
        assert_success(&output, "preflight text readiness");
        let stdout = stdout(&output);
        assert!(stdout.contains("run_readiness"), "{stdout}");
        assert!(stdout.contains(RUN_ID), "{stdout}");
    }
    assert_eq!(fs::read(repo.state_path()).unwrap(), state_before);
}

#[cfg(unix)]
#[test]
fn preflight_json_reports_environment_and_active_or_explicit_readiness() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    repo.install_passing_healthcheck();

    for args in [
        vec!["preflight", "--json"],
        vec!["preflight", "--json", "--run-id", RUN_ID],
    ] {
        let output = repo.lto(&args);
        assert_success(&output, "preflight JSON readiness");
        let report = parse_json_stdout(&output, "preflight JSON readiness");
        assert_eq!(report["environment"]["ok"], true);
        assert!(report["environment"]["checks"].is_array());
        assert_eq!(report["run_readiness"]["ok"], true);
        assert_eq!(report["run_readiness"]["missing"], json!([]));
        assert!(report["run_readiness"]["warnings"].is_array());
    }
}

#[cfg(unix)]
#[test]
fn preflight_json_omits_readiness_when_no_run_is_selected() {
    let repo = RepoFixture::new();
    repo.install_passing_healthcheck();

    let output = repo.lto(&["preflight", "--json"]);
    assert_success(&output, "preflight JSON without a run");
    let report = parse_json_stdout(&output, "preflight JSON without a run");
    assert_eq!(report["environment"]["ok"], true);
    assert!(report["environment"]["checks"].is_array());
    assert!(report.get("run_readiness").is_none());
}

#[cfg(unix)]
#[test]
fn preflight_explicit_missing_run_fails_in_text_and_json_modes() {
    let repo = RepoFixture::new();
    repo.install_passing_healthcheck();

    let text_output = repo.lto(&["preflight", "--run-id", "missing-run"]);
    assert_failure(&text_output, "text preflight with explicit missing run");
    let combined = format!("{}\n{}", stdout(&text_output), stderr(&text_output));
    assert!(combined.contains("missing-run"), "{combined}");
    assert!(stdout(&text_output).contains("LTO Preflight"));
    assert_eq!(repo.healthcheck_call_count(), 1);

    let json_output = repo.lto(&["preflight", "--json", "--run-id", "missing-run"]);
    assert_failure(&json_output, "JSON preflight with explicit missing run");
    let report = parse_json_stdout(&json_output, "JSON preflight with explicit missing run");
    assert!(report.to_string().contains("missing-run"), "{report}");
    assert_eq!(report["environment"]["ok"], true);
    assert!(report["environment"]["checks"].is_array());
    assert!(report["environment"].get("skipped").is_none());
    assert_eq!(repo.healthcheck_call_count(), 2);
}

#[cfg(unix)]
#[test]
fn preflight_json_records_environment_before_failing_incomplete_readiness() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    repo.install_passing_healthcheck();
    let mut state = repo.state();
    state["done_when"] = json!("  ");
    state["delivery_contract"] = json!({
        "schema_version": 1,
        "targets": ["unmeasured target"]
    });
    repo.write_state(&state);

    let output = repo.lto(&["preflight", "--json", "--record"]);
    assert_failure(&output, "preflight with incomplete run readiness");
    let report = parse_json_stdout(&output, "preflight JSON with incomplete run");
    assert_eq!(report["environment"]["ok"], true);
    assert_eq!(report["run_readiness"]["ok"], false);
    let missing = string_array(&report["run_readiness"]["missing"]);
    assert!(missing.contains(&"--done-when".to_string()), "{missing:?}");
    assert!(missing.contains(&"--instrument".to_string()), "{missing:?}");
    assert!(!missing.contains(&"--target".to_string()), "{missing:?}");
    assert!(report["run_readiness"]["warnings"].is_array());

    let recorded = repo.state();
    assert_eq!(recorded["environment_snapshot"]["sandbox"], "ok");
    assert_eq!(
        recorded["environment_snapshot"]["preflight_verdict"],
        "pass"
    );
    assert!(recorded["environment_snapshot"]["checks"].is_array());
    assert!(
        recorded["environment_snapshot"]["captured_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[cfg(unix)]
#[test]
fn preflight_json_keeps_environment_report_when_record_persistence_fails() {
    use std::os::unix::fs::PermissionsExt;

    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    repo.install_passing_healthcheck();
    let run_dir = repo.root.join(".lto").join(RUN_ID);
    fs::write(run_dir.join(".state.lock"), "").unwrap();
    let original_mode = fs::metadata(&run_dir).unwrap().permissions().mode();
    let mut read_only = fs::metadata(&run_dir).unwrap().permissions();
    read_only.set_mode(0o555);
    fs::set_permissions(&run_dir, read_only).unwrap();

    let output = repo.lto(&["preflight", "--json", "--record", "--run-id", RUN_ID]);

    let mut restored = fs::metadata(&run_dir).unwrap().permissions();
    restored.set_mode(original_mode);
    fs::set_permissions(&run_dir, restored).unwrap();
    assert_failure(&output, "JSON preflight with a state persistence failure");
    let report = parse_json_stdout(&output, "JSON preflight persistence failure");
    assert_eq!(report["environment"]["ok"], true);
    assert!(report["environment"]["checks"].is_array());
    assert_eq!(report["run_readiness"]["run_id"], RUN_ID);
    assert_eq!(report["run_readiness"]["ok"], true);
    assert!(
        report["record_error"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[test]
fn check_strict_accepts_empty_contract_like_start() {
    assert_strict_contract_accepted(&[]);
}

#[test]
fn check_strict_accepts_paired_contract_like_start() {
    assert_strict_contract_accepted(&[
        "--target",
        "paired target",
        "--instrument",
        "paired::cargo test",
    ]);
}

#[test]
fn check_strict_accepts_complete_contract_like_start() {
    assert_strict_contract_accepted(&[
        "--target",
        "complete target",
        "--constraint",
        "bounded scope",
        "--instrument",
        "complete::cargo test",
        "--entropy-check",
        "change hypothesis",
    ]);
}

#[test]
fn check_strict_rejects_target_without_instrument_like_start() {
    assert_strict_contract_rejected(
        json!({"schema_version": 1, "targets": ["target only"]}),
        "--instrument",
        "--target",
    );
}

#[test]
fn check_strict_rejects_instrument_without_target_like_start() {
    assert_strict_contract_rejected(
        json!({"schema_version": 1, "instruments": ["instrument only"]}),
        "--target",
        "--instrument",
    );
}

#[test]
fn check_strict_rejects_empty_labeled_instruments_like_start() {
    for invalid_instrument in ["label::", "::"] {
        assert_strict_contract_rejected(
            json!({
                "schema_version": 1,
                "targets": ["target with invalid labeled instrument"],
                "instruments": [invalid_instrument]
            }),
            "--instrument",
            "--target",
        );
    }
}

#[test]
fn check_strict_without_phase_rejects_missing_readiness_and_optional_only_contract() {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    let mut state = repo.state();
    state["goal"] = json!("");
    state["done_when"] = json!("");
    state["delivery_contract"] = json!({
        "schema_version": 1,
        "constraints": ["bounded scope"]
    });
    repo.write_state(&state);

    let output = repo.lto(&["check", "--run-id", RUN_ID, "--strict", "--json"]);

    assert_failure(&output, "strict check without --to");
    let report = parse_json_stdout(&output, "strict check without --to");
    let errors = report["check"]["errors"].to_string();
    for flag in ["--goal", "--done-when", "--target", "--instrument"] {
        assert!(errors.contains(flag), "missing {flag} in {errors}");
    }
}

fn assert_strict_contract_accepted(contract_args: &[&str]) {
    let repo = RepoFixture::new();
    let mut args = vec!["--host", "codex"];
    args.extend_from_slice(contract_args);
    repo.start_ok(&args);

    let output = repo.check_implementation();
    assert_success(&output, "strict implementation check for accepted start");
    let report = parse_json_stdout(&output, "strict implementation check");
    assert!(
        delivery_check(&report)
            .and_then(|check| check.get("status"))
            .and_then(Value::as_str)
            != Some("missing"),
        "{}",
        stdout(&output)
    );
}

fn assert_strict_contract_rejected(contract: Value, expected_missing: &str, unexpected_flag: &str) {
    let repo = RepoFixture::new();
    repo.start_ok(&["--host", "codex"]);
    let mut state = repo.state();
    state["delivery_contract"] = contract;
    repo.write_state(&state);

    let output = repo.check_implementation();
    assert_failure(&output, "strict implementation check for partial contract");
    let report = parse_json_stdout(&output, "strict partial contract check");
    let check = delivery_check(&report).expect("delivery contract check");
    assert_eq!(check["status"], "missing");
    let detail = check["detail"].as_str().unwrap_or_default();
    assert!(detail.contains(expected_missing), "{detail}");
    assert!(!detail.contains(unexpected_flag), "{detail}");
}

fn assert_start_rejected_before_write(args: &[&str], missing_flags: &[&str]) {
    let repo = RepoFixture::new();
    let output = repo.lto(args);
    assert_failure(&output, "start readiness rejection");
    let stderr = stderr(&output);
    assert!(stderr.contains("需补充"), "{stderr}");
    for flag in missing_flags {
        assert!(stderr.contains(flag), "missing {flag} in:\n{stderr}");
    }
    assert!(
        !repo.root.join(".lto").exists(),
        "invalid start wrote .lto before validation"
    );
}

fn assert_partial_start_rejected(extra: &[&str], missing: &str, present: &str) {
    let repo = RepoFixture::new();
    let mut args = vec![
        "start",
        "--run-id",
        RUN_ID,
        "--goal",
        "ship C2",
        "--done-when",
        "tests pass",
        "--host",
        "codex",
    ];
    args.extend_from_slice(extra);
    let output = repo.lto(&args);
    assert_failure(&output, "start partial delivery contract");
    let stderr = stderr(&output);
    assert!(stderr.contains(missing), "{stderr}");
    assert!(!stderr.contains(present), "{stderr}");
    assert!(
        !repo.root.join(".lto").exists(),
        "partial contract wrote .lto before validation"
    );
}

struct RepoFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl RepoFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        run_ok(Command::new("git").args(["init", "-q"]).current_dir(&root));
        run_ok(
            Command::new("git")
                .args(["config", "user.email", "lto@example.test"])
                .current_dir(&root),
        );
        run_ok(
            Command::new("git")
                .args(["config", "user.name", "LTO Test"])
                .current_dir(&root),
        );
        run_ok(
            Command::new("git")
                .args(["commit", "--allow-empty", "-q", "-m", "init"])
                .current_dir(&root),
        );
        Self { _tmp: tmp, root }
    }

    fn lto_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lto-rs"));
        command
            .arg("--repo")
            .arg(&self.root)
            .args(args)
            .env_remove("LTO_HOST_RUNTIME")
            .env("C2_HEALTHCHECK_ROOT", &self.root);
        command
    }

    fn lto(&self, args: &[&str]) -> Output {
        self.lto_command(args).output().unwrap()
    }

    fn start(&self, extra: &[&str]) -> Output {
        let mut args = vec![
            "start",
            "--run-id",
            RUN_ID,
            "--goal",
            "ship C2",
            "--done-when",
            "tests pass",
        ];
        args.extend_from_slice(extra);
        self.lto(&args)
    }

    fn start_ok(&self, extra: &[&str]) {
        let output = self.start(extra);
        assert_success(&output, "fixture start");
    }

    fn check_implementation(&self) -> Output {
        self.lto(&[
            "check",
            "--run-id",
            RUN_ID,
            "--to",
            "implementation",
            "--strict",
            "--json",
        ])
    }

    fn state_path(&self) -> PathBuf {
        self.root.join(".lto").join(RUN_ID).join("state.json")
    }

    fn events_path(&self) -> PathBuf {
        self.root.join(".lto").join(RUN_ID).join("events.jsonl")
    }

    fn run_state_path(&self) -> PathBuf {
        self.root.join(".lto").join(RUN_ID).join("run-state.md")
    }

    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.state_path()).unwrap()).unwrap()
    }

    fn write_state(&self, state: &Value) {
        fs::write(
            self.state_path(),
            serde_json::to_string_pretty(state).unwrap() + "\n",
        )
        .unwrap();
    }

    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.events_path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[cfg(unix)]
    fn install_passing_healthcheck(&self) {
        use std::os::unix::fs::PermissionsExt;

        let runners = self.root.join("scripts").join("delegate").join("runners");
        fs::create_dir_all(&runners).unwrap();
        let script = runners.join("healthcheck.sh");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
set -euo pipefail
printf 'called\n' >> "$C2_HEALTHCHECK_ROOT/.healthcheck-calls"
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

    #[cfg(unix)]
    fn install_blocking_healthcheck(&self) {
        use std::os::unix::fs::PermissionsExt;

        let runners = self.root.join("scripts").join("delegate").join("runners");
        fs::create_dir_all(&runners).unwrap();
        let script = runners.join("healthcheck.sh");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
set -euo pipefail
touch "$C2_HEALTHCHECK_ROOT/.preflight-health-started"
while [ ! -f "$C2_HEALTHCHECK_ROOT/.preflight-health-release" ]; do sleep 0.01; done
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

    #[cfg(unix)]
    fn install_blocking_runner(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = self.root.join("blocking-runner.sh");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
set -euo pipefail
touch "$C2_HEALTHCHECK_ROOT/.runner-started"
while [ ! -f "$C2_HEALTHCHECK_ROOT/.runner-release" ]; do sleep 0.01; done
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        script
    }

    fn healthcheck_call_count(&self) -> usize {
        fs::read_to_string(self.root.join(".healthcheck-calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

fn delivery_values<'a>(state: &'a Value, key: &str) -> Vec<&'a str> {
    state
        .get("delivery_contract")
        .and_then(|contract| contract.get(key))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn delivery_check(report: &Value) -> Option<&Value> {
    report["checks"]
        .as_array()?
        .iter()
        .find(|check| check["id"] == "delivery_contract_complete")
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("JSON string array")
        .iter()
        .map(|item| item.as_str().expect("string array item").to_string())
        .collect()
}

fn parse_json_stdout(output: &Output, label: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{label} did not emit pure JSON: {err}\nstdout:\n{}\nstderr:\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_failure(output: &Output, label: &str) {
    assert!(
        !output.status.success(),
        "{label} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn run_ok(command: &mut Command) {
    let output = command.output().unwrap();
    assert_success(&output, "fixture command");
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
