use crate::agent_job::{
    AgentJob, AgentResult, Budget, JobStatus, Pattern, RetryPolicy, TaskSize,
    readonly_intent_to_policy,
};
use crate::budget::{self, BudgetStatus};
use crate::commands::util;
use crate::ledger::{self, LedgerDiagnostics, LedgerVerdict};
use crate::llm_judge;
use crate::process::shell_single_quote;
use crate::scheduler::Scheduler;
use crate::worktree;
use anyhow::Context;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PreflightOptions {
    pub run_id: Option<String>,
    pub record: bool,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreflightRunReadiness {
    run_id: String,
    missing: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PreflightRunOutcome {
    run: Option<util::RunContext>,
    record_error: Option<anyhow::Error>,
}

impl PreflightRunReadiness {
    fn is_ready(&self) -> bool {
        self.missing.is_empty()
    }

    fn to_json(&self) -> Value {
        json!({
            "run_id": self.run_id,
            "ok": self.is_ready(),
            "missing": self.missing,
            "warnings": self.warnings,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub run_id: Option<String>,
    pub strict: bool,
    pub to_phase: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CheckOutcome {
    pub run_id: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub phase_report: Option<Value>,
    ledger: Option<LedgerCheck>,
}

#[derive(Debug, Clone)]
struct LedgerCheck {
    has_rounds: bool,
    verdict: Option<LedgerVerdict>,
    sequence: Option<String>,
    diagnostics: Option<LedgerDiagnostics>,
    advisory: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub kind: String,
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,
    pub timeout: u64,
    pub touch: Vec<String>,
    pub note: Option<String>,
    pub instrument_ref: Option<String>,
    pub status_on_fail: String,
    pub runner: String,
    pub allow_headless_write: bool,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub job_file: Option<PathBuf>,
    pub job_id: Option<String>,
    pub tmux_target: Option<String>,
    pub tmux_mode: Option<String>,
    pub tmux_sentinel: Option<PathBuf>,
    pub tmux_session: Option<String>,
    pub tmux_new_window: bool,
    pub tmux_new_session: bool,
    pub tmux_window_name: Option<String>,
    pub tmux_ready_patterns: Vec<String>,
    pub tmux_skip_prompts: Vec<String>,
    pub tmux_ready_timeout_sec: Option<u64>,
    pub tmux_bin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JudgeOptions {
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub phase: Option<String>,
    pub runner: String,
    pub rerun_tests: bool,
    pub case_dir: Option<PathBuf>,
    pub brief: Option<PathBuf>,
    pub baseline_reply: Option<PathBuf>,
    pub candidate_reply: Option<PathBuf>,
    pub candidate_runner: Option<String>,
    pub judge_runner: Option<String>,
    pub execute: bool,
}

#[derive(Debug, Clone)]
pub struct NextOptions {
    pub run_id: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct AutopilotOptions {
    pub run_id: Option<String>,
    pub auto_exec: bool,
    pub autonomous: bool,
    pub timeout: u64,
    pub worker_runner: String,
    pub tmux_target: Option<String>,
    pub tmux_bin: Option<String>,
    pub tmux_ready_timeout_sec: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ReleaseOptions {
    pub part: String,
    pub date: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub enum MemoryAction {
    Export {
        run_id: Option<String>,
    },
    Publish {
        run_id: Option<String>,
        am_bin: Option<String>,
        timeout: u64,
    },
    Resume {
        project: Option<String>,
        run_id: Option<String>,
        am_bin: Option<String>,
        timeout: u64,
    },
}

#[derive(Debug, Clone)]
pub struct TaskAddOptions {
    pub run_id: Option<String>,
    pub task_id: String,
    pub title: String,
    pub phase: Option<String>,
    pub command: Option<String>,
    pub instrument_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskUpdateOptions {
    pub run_id: Option<String>,
    pub task_id: String,
    pub status: Option<String>,
    pub phase: Option<String>,
    pub note: Option<String>,
    pub touch: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PhaseOptions {
    pub run_id: Option<String>,
    pub set_phase: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CollectAgentRunOptions {
    pub run_id: Option<String>,
    pub task_id: String,
    pub runner: String,
    pub reply: PathBuf,
    pub meta: Option<PathBuf>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub elapsed_sec: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HookOptions {
    pub gate: String,
    pub force: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ParallelOptions {
    pub run_id: Option<String>,
    pub task_ids: Vec<String>,
    pub phase: Option<String>,
    pub kind: String,
    pub command: Option<String>,
    pub timeout: u64,
    pub concurrency: usize,
    pub job_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub run_id: Option<String>,
    pub task_ids: Vec<String>,
    pub phase: Option<String>,
    pub stages: Vec<String>,
    pub kind: String,
    pub timeout: u64,
    pub concurrency: usize,
    pub continue_on_error: bool,
    pub job_file: Option<PathBuf>,
}

/// Best-effort check for an executable on PATH (no spawn). Used for advisory
/// tool probes like hs, so a missing tool never blocks or fails preflight.
fn which_in_path(cmd: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(cmd);
        candidate.is_file() && is_executable(&candidate)
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn cmd_preflight(repo: &Path, options: PreflightOptions) -> anyhow::Result<()> {
    let selected_run_id = select_preflight_run_id(repo, options.run_id.as_deref());
    let runners = util::KNOWN_RUNNERS
        .iter()
        .map(|runner| (*runner).to_string())
        .collect::<Vec<_>>();
    let runners_dir = repo.join("scripts").join("delegate").join("runners");
    let scheduler = Scheduler::new(repo, &runners_dir);
    let health = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(scheduler.healthcheck_checked(&runners));

    let sandbox_write = check_write(repo);
    let git_repo = crate::process::ensure_git_repo(repo).is_ok();
    let mut checks = vec![
        json!({"name": "sandbox_write", "pass": sandbox_write, "detail": if sandbox_write { "can write to repo" } else { "write failed" }}),
        json!({"name": "git_repo", "pass": git_repo, "detail": if git_repo { "git repo" } else { "not a git repo" }}),
    ];
    match health {
        Ok(map) => {
            for runner in &runners {
                checks.push(json!({
                    "name": format!("runner:{runner}"),
                    "pass": map.get(runner).copied().unwrap_or(false),
                    "detail": if map.get(runner).copied().unwrap_or(false) { "OK" } else { "unhealthy" },
                }));
            }
        }
        Err(err) => {
            checks.push(
                json!({"name": "runner_healthcheck", "pass": false, "detail": err.to_string()}),
            );
        }
    }
    // hs (Hybrid Search router) is the preferred entry for external docs/API
    // lookups when dispatched agents research a capability. It is an optional
    // host tool, not an LTO dependency, so this is advisory only (`advisory:
    // true`) and never counts toward the pass/fail gate.
    let hs_present = which_in_path("hs");
    checks.push(json!({
        "name": "tool:hs",
        "pass": hs_present,
        "advisory": true,
        "detail": if hs_present {
            "available (route external docs/API lookups through hs, then verify locally)"
        } else {
            "not found (optional: install hs for cross-checked external research)"
        },
    }));

    // Only non-advisory checks gate pass/fail.
    let gating = |check: &Value| {
        !check
            .get("advisory")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let pass = checks
        .iter()
        .filter(|check| gating(check))
        .all(|check| check.get("pass").and_then(Value::as_bool).unwrap_or(false));
    let passed = checks
        .iter()
        .filter(|check| gating(check))
        .filter(|check| check.get("pass").and_then(Value::as_bool).unwrap_or(false))
        .count();
    let gating_total = checks.iter().filter(|check| gating(check)).count();
    let run = selected_run_id.and_then(|run_id| {
        load_preflight_run(
            repo,
            run_id.as_deref(),
            options.record,
            sandbox_write,
            pass,
            &checks,
        )
    });
    let outcome = match run {
        Ok(outcome) => outcome,
        Err(error) => {
            if options.json {
                let report = preflight_run_error_json(
                    &checks,
                    pass,
                    repo,
                    options.run_id.as_deref(),
                    &error,
                );
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_preflight_environment(&checks, pass, passed, gating_total);
                print_preflight_run_error(
                    &requested_preflight_run_id(repo, options.run_id.as_deref()),
                    &error,
                );
                crate::commands::prune::maybe_nudge_prune(repo);
            }
            return Err(error);
        }
    };
    let readiness = outcome.run.as_ref().map(assess_preflight_run);

    if options.json {
        let mut report = json!({
            "environment": {
                "ok": pass,
                "checks": checks,
            }
        });
        if let Some(readiness) = &readiness {
            report["run_readiness"] = readiness.to_json();
        }
        if let Some(error) = &outcome.record_error {
            report["record_error"] = json!(error.to_string());
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_preflight_environment(&checks, pass, passed, gating_total);
        if let Some(readiness) = &readiness {
            print_preflight_run_readiness(readiness);
        }
        // Advisory: nudge the host to reclaim disk if .lto has grown large.
        crate::commands::prune::maybe_nudge_prune(repo);
    }

    let readiness_ok = readiness
        .as_ref()
        .is_none_or(PreflightRunReadiness::is_ready);
    if let Some(error) = outcome.record_error {
        return Err(error);
    }
    if pass && readiness_ok {
        Ok(())
    } else {
        anyhow::bail!("preflight failed")
    }
}

fn select_preflight_run_id(
    repo: &Path,
    explicit_run_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if let Some(run_id) = explicit_run_id {
        return Ok(Some(crate::state::validate_run_id(run_id)?.to_string()));
    }
    let current = repo.join(".lto").join("current");
    match fs::symlink_metadata(&current) {
        Ok(_) => util::resolve_run_id(repo, None).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", current.display())),
    }
}

fn load_preflight_run(
    repo: &Path,
    selected_run_id: Option<&str>,
    record: bool,
    sandbox_write: bool,
    pass: bool,
    checks: &[Value],
) -> anyhow::Result<PreflightRunOutcome> {
    let Some(run_id) = selected_run_id else {
        return Ok(PreflightRunOutcome {
            run: None,
            record_error: None,
        });
    };
    if !record {
        return util::load_run(repo, Some(run_id)).map(|run| PreflightRunOutcome {
            run: Some(run),
            record_error: None,
        });
    }

    let _run_lock = util::lock_existing_run(repo, run_id)?;
    let mut ctx = util::load_run(repo, Some(run_id))?;
    ctx.state.environment_snapshot.sandbox = if sandbox_write { "ok" } else { "fail" }.to_string();
    ctx.state.environment_snapshot.network = "unknown".to_string();
    ctx.state.environment_snapshot.captured_at = util::iso_now();
    ctx.state.environment_snapshot.extra.insert(
        "preflight_verdict".to_string(),
        json!(if pass { "pass" } else { "fail" }),
    );
    ctx.state
        .environment_snapshot
        .extra
        .insert("checks".to_string(), Value::Array(checks.to_vec()));
    let record_error = util::save_run_locked(&ctx).err();
    Ok(PreflightRunOutcome {
        run: Some(ctx),
        record_error,
    })
}

fn preflight_run_error_json(
    checks: &[Value],
    pass: bool,
    repo: &Path,
    explicit_run_id: Option<&str>,
    error: &anyhow::Error,
) -> Value {
    let run_id = requested_preflight_run_id(repo, explicit_run_id);
    json!({
        "environment": {
            "ok": pass,
            "checks": checks,
        },
        "run_readiness": {
            "run_id": run_id,
            "ok": false,
            "missing": [],
            "warnings": [],
            "error": error.to_string(),
        },
        "error": error.to_string(),
    })
}

fn requested_preflight_run_id(repo: &Path, explicit_run_id: Option<&str>) -> String {
    explicit_run_id
        .map(str::to_string)
        .or_else(|| {
            fs::read_to_string(repo.join(".lto").join("current"))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "(active)".to_string())
}

fn assess_preflight_run(ctx: &util::RunContext) -> PreflightRunReadiness {
    let base = crate::state::assess_run_readiness(
        &ctx.state.goal,
        &ctx.state.done_when,
        &ctx.state.why,
        &ctx.state.host_runtime,
    );
    let contract = ctx.state.delivery_contract.completeness_missing();
    PreflightRunReadiness {
        run_id: ctx.run_id.clone(),
        missing: base
            .missing
            .into_iter()
            .chain(contract.missing)
            .map(str::to_string)
            .collect(),
        warnings: base
            .advisory
            .into_iter()
            .chain(contract.advisory)
            .map(str::to_string)
            .collect(),
    }
}

fn print_preflight_environment(checks: &[Value], pass: bool, passed: usize, total: usize) {
    println!(
        "=== LTO Preflight ({}: {passed}/{total}) ===",
        if pass { "pass" } else { "fail" }
    );
    for check in checks {
        let ok = check.get("pass").and_then(Value::as_bool).unwrap_or(false);
        let advisory = check
            .get("advisory")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let label = if ok {
            "OK"
        } else if advisory {
            "INFO"
        } else {
            "FAIL"
        };
        println!(
            "  {} {}: {}",
            label,
            check.get("name").and_then(Value::as_str).unwrap_or("?"),
            check.get("detail").and_then(Value::as_str).unwrap_or("")
        );
    }
}

fn print_preflight_run_readiness(readiness: &PreflightRunReadiness) {
    println!(
        "=== LTO run_readiness ({}) ===",
        if readiness.is_ready() { "pass" } else { "fail" }
    );
    println!("  run_id: {}", readiness.run_id);
    for flag in &readiness.missing {
        println!("  MISSING {flag}");
    }
    for flag in &readiness.warnings {
        println!("  WARN {flag}");
    }
}

fn print_preflight_run_error(run_id: &str, error: &anyhow::Error) {
    println!("=== LTO run_readiness (fail) ===");
    println!("  run_id: {run_id}");
    println!("  ERROR {error}");
}

pub fn cmd_check(repo: &Path, options: CheckOptions) -> anyhow::Result<()> {
    let outcome = collect_check(repo, &options);
    if options.json {
        println!("{}", serde_json::to_string_pretty(&outcome_json(&outcome))?);
    } else {
        for warning in &outcome.warnings {
            eprintln!("WARN {warning}");
        }
        for error in &outcome.errors {
            eprintln!("ERROR {error}");
        }
        if let Some(status) = &outcome.ledger {
            if let Some(verdict) = status.verdict {
                println!("ledger verdict: {}", verdict.as_str());
            }
            if let Some(diagnostics) = status.diagnostics {
                println!("ledger diagnostics: {}", diagnostics.summary());
            }
            if let Some(advisory) = &status.advisory {
                eprintln!("ADVISORY {advisory}");
            }
        }
        if let Some(report) = &outcome.phase_report {
            print_phase_report(report);
        }
        if outcome.errors.is_empty() {
            let run_id = if outcome.run_id.is_empty() {
                "(unknown)"
            } else {
                &outcome.run_id
            };
            println!("OK {}", repo.join(".lto").join(run_id).display());
        }
    }
    if let Some(report) = &outcome.phase_report
        && !outcome.run_id.is_empty()
    {
        crate::event_emit::emit_gate_evaluated(repo, &outcome.run_id, "phase_check", report);
        let _ = crate::telemetry::save(repo, &outcome.run_id);
    }
    if !outcome.errors.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn collect_check(repo: &Path, options: &CheckOptions) -> CheckOutcome {
    let mut outcome = CheckOutcome::default();
    let run_id = match util::resolve_run_id(repo, options.run_id.as_deref()) {
        Ok(run_id) => run_id,
        Err(err) => {
            outcome.errors.push(err.to_string());
            options.run_id.clone().unwrap_or_default()
        }
    };
    outcome.run_id = run_id.clone();
    let run_dir = repo.join(".lto").join(&run_id);
    let state_path = run_dir.join("state.json");
    let md_path = run_dir.join("run-state.md");
    let state_exists = state_path.exists();
    let md_exists = md_path.exists();
    let mut state = None;
    if !state_exists && !md_exists {
        outcome.errors.push(format!(
            "missing both {} and {}",
            state_path.display(),
            md_path.display()
        ));
    } else if !state_exists {
        outcome
            .errors
            .push(format!("missing {}", state_path.display()));
    } else {
        match crate::state::load_state(&state_path) {
            Ok(loaded) => {
                collect_state_checks(
                    repo,
                    &run_dir,
                    &loaded,
                    options.strict,
                    options.to_phase.is_none(),
                    &mut outcome,
                );
                state = Some(loaded);
            }
            Err(err) => outcome
                .errors
                .push(format!("cannot parse {}: {err}", state_path.display())),
        }
    }

    let mut ledger_status = collect_ledger_check(&run_dir, options.strict, &mut outcome);
    if let (Some(status), Some(state)) = (ledger_status.as_mut(), state.as_ref())
        && status
            .diagnostics
            .is_some_and(LedgerDiagnostics::suggests_entropy_review)
        && !state.delivery_contract.forced_entropy.is_empty()
    {
        status.advisory = Some(format!(
            "review forced_entropy before changing hypothesis: {}",
            state.delivery_contract.forced_entropy.join(" | ")
        ));
    }
    if let (Some(target), Some(state)) = (options.to_phase.as_deref(), state.as_ref()) {
        let report = phase_report(repo, &run_dir, state, target, ledger_status.as_ref());
        if options.strict {
            for check in report
                .get("checks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let required = check
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let missing = check.get("status").and_then(Value::as_str) == Some("missing");
                if required && missing {
                    outcome.errors.push(format!(
                        "phase evidence missing: {}: {}",
                        check.get("id").and_then(Value::as_str).unwrap_or("?"),
                        check.get("detail").and_then(Value::as_str).unwrap_or("")
                    ));
                }
            }
        }
        outcome.phase_report = Some(report);
    }
    outcome.ledger = ledger_status;
    outcome
}

fn outcome_json(outcome: &CheckOutcome) -> Value {
    let mut output = outcome.phase_report.clone().unwrap_or_else(|| {
        json!({
            "run_id": outcome.run_id,
        })
    });
    output["check"] = json!({
        "errors": outcome.errors,
        "warnings": outcome.warnings,
    });
    if let Some(status) = &outcome.ledger {
        let mut ledger = json!({
            "has_rounds": status.has_rounds,
            "verdict": status.verdict.map(LedgerVerdict::as_str),
            "sequence": status.sequence,
            "error": status.error,
        });
        if let Some(diagnostics) = status.diagnostics {
            ledger["diagnostics"] = json!({
                "sample_sufficiency": diagnostics.sample_sufficiency.as_str(),
                "terminal": diagnostics.terminal.as_str(),
                "direction": diagnostics.direction.as_str(),
                "oscillation": diagnostics.oscillation.as_str(),
                "envelope": diagnostics.envelope.as_str(),
                "confidence": diagnostics.confidence.as_str(),
            });
        }
        if let Some(advisory) = &status.advisory {
            ledger["advisory"] = json!(advisory);
        }
        output["ledger"] = ledger;
    }
    output
}

fn collect_state_checks(
    repo: &Path,
    run_dir: &Path,
    state: &crate::state::LtoState,
    strict: bool,
    include_c2_issues: bool,
    outcome: &mut CheckOutcome,
) {
    if !util::VALID_PHASES.contains(&state.current_phase.as_str()) {
        outcome
            .errors
            .push(format!("invalid current_phase: {}", state.current_phase));
    }
    if include_c2_issues {
        collect_c2_state_checks(state, strict, outcome);
    }
    collect_git_anchor_checks(repo, state, strict, outcome);
    if dirty_outside_lto(repo) {
        let msg = "worktree has uncommitted changes outside .lto".to_string();
        if strict {
            outcome.errors.push(msg);
        } else {
            outcome.warnings.push(msg);
        }
    }
    if state.current_phase == "closed" && !non_empty_file(&run_dir.join("handoff.md")) {
        outcome
            .errors
            .push("closed run missing non-empty handoff.md".to_string());
    }
}

fn collect_c2_state_checks(
    state: &crate::state::LtoState,
    strict: bool,
    outcome: &mut CheckOutcome,
) {
    let readiness = crate::state::assess_run_readiness(
        &state.goal,
        &state.done_when,
        &state.why,
        &state.host_runtime,
    );
    if !readiness.missing.is_empty() {
        push_strict_issue(
            outcome,
            strict,
            format!("run readiness missing: {}", readiness.missing.join(", ")),
        );
    }
    if !readiness.advisory.is_empty() {
        outcome.warnings.push(format!(
            "run readiness advisory: {}",
            readiness.advisory.join(", ")
        ));
    }

    let contract = state.delivery_contract.completeness_missing();
    if !contract.missing.is_empty() {
        push_strict_issue(
            outcome,
            strict,
            format!(
                "delivery contract incomplete: {}",
                contract.missing.join(", ")
            ),
        );
    }
    if contract.present && !contract.advisory.is_empty() {
        outcome.warnings.push(format!(
            "delivery contract advisory: {}",
            contract.advisory.join(", ")
        ));
    }
}

fn push_strict_issue(outcome: &mut CheckOutcome, strict: bool, message: String) {
    if strict {
        outcome.errors.push(message);
    } else {
        outcome.warnings.push(message);
    }
}

fn collect_git_anchor_checks(
    repo: &Path,
    state: &crate::state::LtoState,
    strict: bool,
    outcome: &mut CheckOutcome,
) {
    if crate::process::ensure_git_repo(repo).is_err() {
        let msg = "strict check requires a git worktree".to_string();
        if strict {
            outcome.errors.push(msg);
        } else {
            outcome.warnings.push("not a git worktree".to_string());
        }
        return;
    }
    let recorded = state.workspace.head.as_str();
    let actual = util::git_status(repo).head;
    if recorded.is_empty() || recorded == "unknown" || actual.is_empty() || actual == "unknown" {
        let msg = "strict check requires a real git HEAD anchor".to_string();
        if strict {
            outcome.errors.push(msg);
        } else {
            outcome
                .warnings
                .push("missing real git HEAD anchor".to_string());
        }
        return;
    }
    if !util::commit_exists(repo, recorded) {
        let msg = format!("recorded git HEAD not a commit: {recorded}");
        if strict {
            outcome.errors.push(msg);
        } else {
            outcome.warnings.push(msg);
        }
        return;
    }
    if state.current_phase != "closed" && recorded != actual {
        let drift = util::head_drift(repo, recorded, &actual);
        let msg = format!(
            "git HEAD {drift}: {} -> {}",
            truncate(recorded, 8),
            truncate(&actual, 8)
        );
        if strict {
            outcome.errors.push(msg);
        } else {
            outcome.warnings.push(msg);
        }
    }
}

fn collect_ledger_check(
    run_dir: &Path,
    strict: bool,
    outcome: &mut CheckOutcome,
) -> Option<LedgerCheck> {
    let ledger_path = run_dir.join("audit-ledger.md");
    if !ledger_path.exists() {
        outcome.warnings.push("no audit-ledger.md".to_string());
        return None;
    }
    let status = match fs::read_to_string(&ledger_path)
        .map_err(anyhow::Error::from)
        .and_then(|text| ledger::parse_ledger(&text))
    {
        Ok(rounds) => {
            let verdict = ledger::evaluate_ledger(&rounds, strict);
            let sequence = ledger::ledger_sequence(&rounds);
            LedgerCheck {
                has_rounds: !rounds.is_empty(),
                verdict: Some(verdict),
                sequence: (!sequence.is_empty()).then_some(sequence),
                diagnostics: ledger::diagnose(&rounds),
                advisory: None,
                error: None,
            }
        }
        Err(err) => LedgerCheck {
            has_rounds: false,
            verdict: None,
            sequence: None,
            diagnostics: None,
            advisory: None,
            error: Some(err.to_string()),
        },
    };
    match (&status.error, &status.verdict, status.has_rounds) {
        (Some(err), _, _) => {
            let msg = format!("ledger check failed: {err}");
            if strict {
                outcome.errors.push(msg);
            } else {
                outcome.warnings.push(msg);
            }
        }
        (None, Some(LedgerVerdict::NoObservations), false) => outcome
            .warnings
            .push("ledger exists but has no filled rounds".to_string()),
        (None, Some(verdict), true) if *verdict != LedgerVerdict::Converged => {
            let msg = match status.sequence.as_deref() {
                Some(sequence) => {
                    format!("ledger not converged: {} ({sequence})", verdict.as_str())
                }
                None => format!("ledger not converged: {}", verdict.as_str()),
            };
            if strict {
                outcome.errors.push(msg);
            } else {
                outcome.warnings.push(msg);
            }
        }
        _ => {}
    }
    Some(status)
}

fn phase_report(
    repo: &Path,
    run_dir: &Path,
    state: &crate::state::LtoState,
    target: &str,
    ledger_status: Option<&LedgerCheck>,
) -> Value {
    let current = state.current_phase.as_str();
    let mut checks = Vec::new();
    add_phase_check(
        &mut checks,
        "phase_direction",
        if phase_direction(current, target) == "backward" {
            "warn"
        } else {
            "ok"
        },
        false,
        format!(
            "{current} -> {target} ({})",
            phase_direction(current, target)
        ),
    );
    let unresolved = unresolved_blocks(state);
    let open_risks = open_unverified_risks(state);
    match target {
        "implementation" => {
            add_run_readiness_phase_checks(&mut checks, state);
            add_delivery_contract_phase_check(&mut checks, state);
            let count = unresolved.len() + open_risks.len();
            add_phase_check(
                &mut checks,
                "no_unresolved_blocks",
                if count == 0 { "ok" } else { "missing" },
                true,
                if count == 0 {
                    "none".to_string()
                } else {
                    format!("{count} unresolved item(s)")
                },
            );
            add_ledger_phase_check(&mut checks, run_dir, ledger_status, false);
            let tasks = util::json_array(&state.tasks);
            add_phase_check(
                &mut checks,
                "tasks_present",
                if tasks.is_empty() { "warn" } else { "ok" },
                false,
                if tasks.is_empty() {
                    "no tasks found".to_string()
                } else {
                    format!("{} task(s)", tasks.len())
                },
            );
        }
        "closed" => {
            add_run_readiness_phase_checks(&mut checks, state);
            add_delivery_contract_phase_check(&mut checks, state);
            let open_tasks = util::json_array(&state.tasks)
                .iter()
                .filter(|task| {
                    !matches!(
                        task.get("status").and_then(Value::as_str),
                        Some("done" | "skipped")
                    )
                })
                .count();
            add_phase_check(
                &mut checks,
                "no_open_tasks",
                if open_tasks == 0 { "ok" } else { "missing" },
                true,
                if open_tasks == 0 {
                    "none".to_string()
                } else {
                    format!("{open_tasks} open task(s)")
                },
            );
            add_phase_check(
                &mut checks,
                "no_unresolved_blocks",
                if unresolved.is_empty() {
                    "ok"
                } else {
                    "missing"
                },
                true,
                if unresolved.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} unresolved gate block(s)", unresolved.len())
                },
            );
            add_phase_check(
                &mut checks,
                "risk_points_verified",
                if open_risks.is_empty() {
                    "ok"
                } else {
                    "missing"
                },
                true,
                if open_risks.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} open unverified risk point(s)", open_risks.len())
                },
            );
            let done_without_evidence = done_tasks_without_evidence(state);
            add_phase_check(
                &mut checks,
                "done_tasks_have_evidence",
                if done_without_evidence.is_empty() {
                    "ok"
                } else {
                    "missing"
                },
                true,
                if done_without_evidence.is_empty() {
                    "all done tasks carry evidence".to_string()
                } else {
                    format!(
                        "done task(s) missing evidence: {}",
                        done_without_evidence.join(", ")
                    )
                },
            );
            add_ledger_phase_check(&mut checks, run_dir, ledger_status, true);
            let handoff = run_dir.join("handoff.md");
            add_phase_check(
                &mut checks,
                "handoff_exists",
                if non_empty_file(&handoff) {
                    "ok"
                } else {
                    "missing"
                },
                true,
                if non_empty_file(&handoff) {
                    "exists".to_string()
                } else {
                    "missing non-empty handoff.md".to_string()
                },
            );
            let manifest = run_dir.join("artifacts.json");
            add_phase_check(
                &mut checks,
                "artifact_manifest_exists",
                if manifest.exists() { "ok" } else { "warn" },
                false,
                util::repo_relative_path(repo, &manifest)
                    .unwrap_or_else(|_| "missing artifacts.json".to_string()),
            );
        }
        _ => {}
    }
    let evidence_status = if checks.iter().any(|check| {
        matches!(
            check.get("status").and_then(Value::as_str),
            Some("missing" | "warn")
        )
    }) {
        "attention_required"
    } else {
        "all_required_present"
    };
    json!({
        "run_id": state.run_id,
        "target_phase": target,
        "current_phase": current,
        "phase_direction": phase_direction(current, target),
        "evidence_status": evidence_status,
        "human_gate_required": true,
        "checks": checks,
    })
}

fn add_run_readiness_phase_checks(checks: &mut Vec<Value>, state: &crate::state::LtoState) {
    let assessment = crate::state::assess_run_readiness(
        &state.goal,
        &state.done_when,
        &state.why,
        &state.host_runtime,
    );
    let detail = if assessment.missing.is_empty() {
        "goal and done_when present".to_string()
    } else {
        format!("missing {}", assessment.missing.join(", "))
    };
    add_phase_check(
        checks,
        "run_readiness",
        if assessment.is_ready() {
            "ok"
        } else {
            "missing"
        },
        true,
        detail,
    );
    if !assessment.advisory.is_empty() {
        add_phase_check(
            checks,
            "run_readiness_advisory",
            "warn",
            false,
            format!("advisory {}", assessment.advisory.join(", ")),
        );
    }
}

fn add_delivery_contract_phase_check(checks: &mut Vec<Value>, state: &crate::state::LtoState) {
    let contract = &state.delivery_contract;
    let assessment = contract.completeness_missing();
    if !assessment.present {
        return;
    }
    let detail = if assessment.missing.is_empty() {
        format!(
            "targets={}, constraints={}, instruments={}, forced_entropy={}",
            contract.targets.len(),
            contract.constraints.len(),
            contract.instruments.len(),
            contract.forced_entropy.len()
        )
    } else {
        format!("missing {}", assessment.missing.join(", "))
    };
    add_phase_check(
        checks,
        "delivery_contract_complete",
        if assessment.is_complete() {
            "ok"
        } else {
            "missing"
        },
        true,
        detail,
    );
    if !assessment.advisory.is_empty() {
        add_phase_check(
            checks,
            "delivery_contract_advisory",
            "warn",
            false,
            format!("advisory {}", assessment.advisory.join(", ")),
        );
    }
}

fn add_phase_check(
    checks: &mut Vec<Value>,
    id: &str,
    status: &str,
    required: bool,
    detail: String,
) {
    checks.push(json!({
        "id": id,
        "status": status,
        "required": required,
        "detail": util::single_line(&detail),
    }));
}

fn add_ledger_phase_check(
    checks: &mut Vec<Value>,
    run_dir: &Path,
    ledger_status: Option<&LedgerCheck>,
    required: bool,
) {
    let (status, detail) = match ledger_status {
        None => ("warn", "no audit-ledger.md".to_string()),
        Some(LedgerCheck {
            error: Some(err), ..
        }) => ("warn", err.clone()),
        Some(LedgerCheck {
            has_rounds: false, ..
        }) => ("warn", "ledger exists but has no filled rounds".to_string()),
        Some(LedgerCheck {
            verdict: Some(verdict),
            ..
        }) if *verdict == LedgerVerdict::Converged => {
            ("ok", "audit-ledger.md: CONVERGED".to_string())
        }
        Some(LedgerCheck {
            verdict: Some(verdict),
            ..
        }) => (
            if required { "missing" } else { "warn" },
            format!(
                "{}: {}",
                run_dir.join("audit-ledger.md").display(),
                verdict.as_str()
            ),
        ),
        _ => ("warn", "unknown ledger status".to_string()),
    };
    add_phase_check(
        checks,
        "audit_ledger_converged_if_present",
        status,
        false,
        detail,
    );
}

fn print_phase_report(report: &Value) {
    println!(
        "=== LTO Phase Evidence: {} ({}) ===",
        report
            .get("target_phase")
            .and_then(Value::as_str)
            .unwrap_or("?"),
        report
            .get("evidence_status")
            .and_then(Value::as_str)
            .unwrap_or("?")
    );
    for check in report
        .get("checks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let status = match check
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("warn")
        {
            "ok" => "OK",
            "missing" => "MISSING",
            _ => "WARN",
        };
        let scope = if check
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "required"
        } else {
            "advisory"
        };
        println!(
            "  {status} {scope} {}: {}",
            check.get("id").and_then(Value::as_str).unwrap_or("?"),
            check.get("detail").and_then(Value::as_str).unwrap_or("")
        );
    }
    println!("  HUMAN human_gate_required: true");
}

fn phase_direction(current: &str, target: &str) -> &'static str {
    let rank = |phase: &str| {
        util::VALID_PHASES
            .iter()
            .position(|item| *item == phase)
            .unwrap_or(usize::MAX)
    };
    let current_rank = rank(current);
    let target_rank = rank(target);
    if current_rank == usize::MAX || target_rank == usize::MAX {
        "unknown"
    } else if current_rank < target_rank {
        "forward"
    } else if current_rank == target_rank {
        "same"
    } else {
        "backward"
    }
}

fn unresolved_blocks(state: &crate::state::LtoState) -> Vec<Value> {
    state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn open_unverified_risks(state: &crate::state::LtoState) -> Vec<Value> {
    util::json_array(&state.risk_points)
        .iter()
        .filter(|risk| util::risk_is_open_unverified(risk))
        .cloned()
        .collect()
}

fn done_tasks_without_evidence(state: &crate::state::LtoState) -> Vec<String> {
    util::json_array(&state.tasks)
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("done"))
        .filter(|task| {
            task.get("evidence")
                .and_then(Value::as_array)
                .is_none_or(|items| items.is_empty())
        })
        .map(|task| {
            task.get("id")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        })
        .collect()
}

fn dirty_outside_lto(repo: &Path) -> bool {
    !util::tracked_dirty_paths(repo).is_empty() || !util::untracked_paths(repo).is_empty()
}

fn non_empty_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false)
}

pub fn cmd_runner(repo: &Path, options: RunnerOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let mut jobs = load_jobs(job_file)?;
        if options.allow_headless_write {
            for job in &mut jobs {
                job.permission_policy.allow_headless_write = true;
            }
        }
        let run_id = job_file_run_id(repo, options.run_id.as_deref(), &jobs)?;
        return run_job_file(repo, jobs, run_id, "runner.job_file");
    }
    let command_as_tmux_prompt =
        options.runner == "tmux" && options.command.is_some() && options.task_id.is_none();
    if options.prompt.is_some() || options.prompt_file.is_some() || command_as_tmux_prompt {
        let run_id = util::resolve_run_id(repo, options.run_id.as_deref())
            .unwrap_or_else(|_| "rust-runner".to_string());
        let mut run_ctx = util::load_run(repo, Some(&run_id)).ok();
        let inline_prompt = options.prompt.clone().or_else(|| {
            if command_as_tmux_prompt {
                options.command.clone()
            } else {
                None
            }
        });
        let mut permission_policy = readonly_intent_to_policy(&options.runner);
        if options.allow_headless_write {
            permission_policy.allow_headless_write = true;
        }
        let job = AgentJob {
            job_id: options
                .job_id
                .clone()
                .unwrap_or_else(|| "runner-1".to_string()),
            prompt_ref: inline_prompt
                .clone()
                .or_else(|| {
                    options
                        .prompt_file
                        .as_ref()
                        .map(|path| path.display().to_string())
                })
                .unwrap_or_default(),
            runner: options.runner.clone(),
            prompt_is_inline: inline_prompt.is_some(),
            model: None,
            env: BTreeMap::new(),
            permission_policy,
            isolation: "none".to_string(),
            output_schema: None,
            parent_pattern: Pattern::Linear,
            budget: Budget {
                timeout_sec: options.timeout,
                max_tokens: None,
            },
            retry_policy: RetryPolicy::default(),
            verifier_of: None,
            children: Vec::new(),
            task_type: Some("runner".to_string()),
            size: TaskSize::Small,
            test_cmd: None,
            needs_worktree: false,
            meta: runner_job_meta(&options, &run_id),
        };
        let jobs = vec![job];
        let phase = run_ctx.as_ref().map(|ctx| ctx.state.current_phase.as_str());
        crate::event_emit::emit_runner_started_jobs(
            repo,
            &run_id,
            phase,
            None,
            "runner.prompt",
            &jobs,
        );
        let results = match submit_jobs(repo, jobs.clone()) {
            Ok(results) => results,
            Err(err) => {
                crate::event_emit::emit_runner_submission_failed_jobs(
                    repo,
                    &run_id,
                    None,
                    None,
                    "runner.prompt",
                    &jobs,
                    &err.to_string(),
                );
                return Err(err);
            }
        };
        if let Some(ctx) = &mut run_ctx {
            emit_and_record_runner_results(repo, ctx, None, "runner.prompt", &results)?;
        } else {
            crate::event_emit::emit_runner_results(
                repo,
                &run_id,
                None,
                None,
                "runner.prompt",
                &results,
            );
            let _ = crate::telemetry::save(repo, &run_id);
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    warn_if_tmux_flags_ignored(&options);
    run_task_command(repo, options)
}

/// Nudge (CLAUDE.md 原则1): tmux pipeline flags are silently dropped on the
/// headless path. Warn the caller instead of swallowing them, and point at the
/// preferred interactive dispatch, without changing the default runner.
fn warn_if_tmux_flags_ignored(options: &RunnerOptions) {
    if options.runner == "tmux" {
        return;
    }
    let has_tmux_flags = options.tmux_target.is_some()
        || options.tmux_mode.is_some()
        || options.tmux_sentinel.is_some()
        || options.tmux_session.is_some()
        || options.tmux_new_window
        || options.tmux_new_session
        || options.tmux_window_name.is_some()
        || !options.tmux_ready_patterns.is_empty()
        || !options.tmux_skip_prompts.is_empty()
        || options.tmux_ready_timeout_sec.is_some();
    if has_tmux_flags {
        eprintln!(
            "warning: --tmux-* flags are ignored with --runner {} (headless); \
             use --runner tmux or `lto dispatch-goal` for an interactive session",
            options.runner
        );
    }
}

pub fn cmd_judge(repo: &Path, options: JudgeOptions) -> anyhow::Result<()> {
    if options.case_dir.is_some()
        || options.brief.is_some()
        || options.baseline_reply.is_some()
        || options.candidate_reply.is_some()
    {
        return cmd_llm_judge(repo, options);
    }
    cmd_state_judge(repo, options)
}

pub fn cmd_next(repo: &Path, options: NextOptions) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    let facts = analyze_state(repo, &ctx.state);
    let drift = util::head_drift(
        repo,
        &ctx.state.workspace.head,
        &util::git_status(repo).head,
    );
    let route = route_next(&facts);
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "drift": drift,
                "facts": facts,
                "route": route,
            }))?
        );
        return Ok(());
    }
    println!("{}", decision_brief(&ctx.state, &facts));
    println!();
    println!(
        "# Route: {}",
        route["action"]
            .as_str()
            .unwrap_or("escalate")
            .to_ascii_uppercase()
    );
    println!("  unambiguous={}", route["unambiguous"]);
    println!("  reason: {}", route["reason"].as_str().unwrap_or(""));
    if let Some(cmd) = route.get("cmd").and_then(Value::as_str) {
        println!("  cmd: {cmd}");
    }
    if let Some(pattern) = route.get("pattern").and_then(Value::as_str) {
        println!("  pattern: {pattern}");
    }
    Ok(())
}

pub fn cmd_autopilot(repo: &Path, options: AutopilotOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    ctx.state.budget.turns_used = ctx.state.budget.turns_used.saturating_add(1);
    util::save_run(&mut ctx)?;
    let rollup = util::token_rollup(&ctx.state);
    let budget = budget::check_budget(
        Some(&ctx.state.budget),
        &ctx.state.started_at,
        rollup.total_tokens,
        &util::iso_now(),
    );
    crate::event_emit::emit_budget_event(repo, &ctx.run_id, &budget, "autopilot");
    if budget.overall == BudgetStatus::Exceeded {
        let _ = crate::telemetry::save(repo, &ctx.run_id);
        println!("# LTO Autopilot -- budget gate BLOCKED");
        println!("AUTOPILOT_STATUS: NEEDS_CONFIRM");
        return Ok(());
    }
    if options.autonomous {
        let report = autonomous_gate(repo, &ctx.state);
        println!("# LTO Autopilot -- autonomous");
        println!(
            "  operational_reliability: {}",
            if report.operational_reliability.passes() {
                "PASS"
            } else {
                "FAIL"
            }
        );
        println!("    reason: {}", report.operational_reliability.reason);
        for warning in &report.operational_reliability.warnings {
            println!("    WARN {warning}");
        }
        println!(
            "  current_run_observability: {}",
            serde_json::to_value(report.current_run_observability.status)?
                .as_str()
                .unwrap_or("missing")
        );
        println!("    reason: {}", report.current_run_observability.reason);
        match report.current_run_observability.status {
            crate::run_observability::ObservabilityStatus::SignalDeclared => {
                println!(
                    "  已声明未证实: {}",
                    report.current_run_observability.reason
                );
                println!(
                    "  missing evidence: {}",
                    report.current_run_observability.missing.join(", ")
                );
            }
            crate::run_observability::ObservabilityStatus::Missing => {
                println!(
                    "  missing: {}",
                    report.current_run_observability.missing.join(", ")
                );
            }
            crate::run_observability::ObservabilityStatus::ObservableVerified => {}
        }
        println!("  gate_report: {}", serde_json::to_string(&report)?);
        if !report.passes() {
            println!("  fallback: supervised");
            println!("AUTOPILOT_STATUS: NEEDS_CONFIRM");
            return Ok(());
        }
    }
    let curr_digest = progress_digest(&ctx);
    let prev_digest = ctx
        .state
        .gates
        .get("autopilot_last_digest")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let (progressed, progress_reason) = has_progressed(&prev_digest, &curr_digest);
    let facts = analyze_state(repo, &ctx.state);
    let route = route_next(&facts);
    println!("{}", decision_brief(&ctx.state, &facts));
    println!();
    println!(
        "# LTO Autopilot -- {}",
        if options.auto_exec || options.autonomous {
            "supervised (auto-exec)"
        } else {
            "supervised (brief-only)"
        }
    );
    println!(
        "  progress since last check: {} ({progress_reason})",
        if progressed { "YES" } else { "STALLED" }
    );
    println!(
        "  route: {}",
        route["action"].as_str().unwrap_or("escalate")
    );
    println!("  reason: {}", route["reason"].as_str().unwrap_or(""));
    println!(
        "  worker carrier: {}",
        select_worker_carrier(&options).as_str()
    );
    if !progressed && !prev_digest.as_object().is_none_or(Map::is_empty) {
        println!("AUTOPILOT_STATUS: STALLED");
        update_autopilot_digest(&mut ctx)?;
        return Ok(());
    }
    if options.auto_exec || options.autonomous {
        auto_exec_tasks(repo, &mut ctx, &options)?;
    } else {
        println!(
            "  suggested cmd: {}",
            route.get("cmd").and_then(Value::as_str).unwrap_or("(none)")
        );
        println!("AUTOPILOT_STATUS: NEEDS_HOST");
    }
    update_autopilot_digest(&mut ctx)?;
    util::save_run(&mut ctx)?;
    Ok(())
}

pub fn cmd_release(repo: &Path, options: ReleaseOptions) -> anyhow::Result<()> {
    let version_path = repo.join("VERSION");
    let changelog_path = repo.join("CHANGELOG.md");
    let old = fs::read_to_string(&version_path)
        .with_context(|| format!("no VERSION file at {}", version_path.display()))?
        .trim()
        .to_string();
    let new = bump_version(&old, &options.part)?;
    let tag = format!("v{new}");
    fs::read_to_string(&changelog_path)
        .with_context(|| format!("no CHANGELOG.md at {}", changelog_path.display()))?;
    println!("# lto release plan: {old} -> {new} (tag {tag})");
    println!("  VERSION: {old} -> {new}");
    println!("  CHANGELOG: Unreleased -> v{new} -- {}", options.date);
    println!("  host git commands:");
    for cmd in util::git_add_plan_commands(&tag) {
        println!("    {cmd}");
    }
    if options.dry_run {
        println!("  (dry-run -- nothing written)");
    } else {
        println!(
            "  Rust 6a does not write VERSION/CHANGELOG or .git; run the host plan above after verification."
        );
    }
    Ok(())
}

pub fn cmd_memory(repo: &Path, action: MemoryAction) -> anyhow::Result<()> {
    match action {
        MemoryAction::Export { run_id } => {
            let ctx = util::load_run(repo, run_id.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&memory_projection(repo, &ctx)?)?
            );
            Ok(())
        }
        MemoryAction::Publish {
            run_id,
            am_bin,
            timeout,
        } => {
            let ctx = util::load_run(repo, run_id.as_deref())?;
            let projection = memory_projection(repo, &ctx)?;
            match publish_am(&projection, am_bin.as_deref(), timeout) {
                Ok(output) => println!("{output}"),
                Err(err) => {
                    eprintln!(
                        "warning: am/ANIMEM memory unavailable; local .lto remains source of truth. ({err})"
                    );
                    println!("{}", serde_json::to_string_pretty(&projection)?);
                }
            }
            Ok(())
        }
        MemoryAction::Resume {
            project,
            run_id,
            am_bin,
            timeout,
        } => {
            let project_key = project.unwrap_or_else(|| {
                repo.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("project")
                    .to_string()
            });
            match resume_am(&project_key, am_bin.as_deref(), timeout) {
                Ok(output) if !output.trim().is_empty() => println!("{output}"),
                Ok(_) => {}
                Err(err) => eprintln!(
                    "warning: am/ANIMEM memory unavailable; using local .lto only. ({err})"
                ),
            }
            let ctx = util::load_run(repo, run_id.as_deref())?;
            print_memory_capsule(repo, &ctx)?;
            Ok(())
        }
    }
}

pub fn cmd_task_add(repo: &Path, options: TaskAddOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let task_id = options.task_id.clone();
    let title = options.title.clone();
    if util::json_array(&ctx.state.tasks)
        .iter()
        .any(|task| task.get("id").and_then(Value::as_str) == Some(task_id.as_str()))
    {
        anyhow::bail!("task id already exists: {}", task_id);
    }
    let phase = options
        .phase
        .clone()
        .unwrap_or_else(|| ctx.state.current_phase.clone());
    ensure_valid_phase(&phase)?;
    let instrument_ref = options
        .instrument_ref
        .as_deref()
        .map(|reference| {
            crate::run_observability::validate_instrument_ref(
                &ctx.state.delivery_contract,
                reference,
            )
        })
        .transpose()?;
    let mut task = json!({
        "id": options.task_id,
        "title": options.title,
        "status": "pending",
        "phase": phase,
        "depends_on": [],
        "last_update": util::iso_now(),
        "touched_files": [],
        "commands_run": [],
        "evidence": [],
        "blockers": [],
        "assumptions": [],
        "retry_count": 0,
        "retry_by_command": {},
    });
    if let Some(command) = options.command {
        task["planned_command"] = json!(command);
    }
    if let Some(instrument_ref) = instrument_ref {
        task["instrument_ref"] = json!(instrument_ref);
    }
    util::json_array_mut(&mut ctx.state.tasks).push(task);
    util::save_run(&mut ctx)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "task.created".to_string(),
            actor_kind: "host".to_string(),
            phase: Some(phase.clone()),
            task_id: Some(task_id.clone()),
            object_id: Some(task_id.clone()),
            object_type: Some("task".to_string()),
            summary: title.clone(),
            ..crate::events::EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    println!("task {} added to phase '{}': {}", task_id, phase, title);
    Ok(())
}

pub fn cmd_task_update(repo: &Path, options: TaskUpdateOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    if options.status.is_none()
        && options.phase.is_none()
        && options.note.is_none()
        && options.touch.is_empty()
    {
        anyhow::bail!(
            "task update is a no-op: pass at least one of --status / --phase / --note / --touch"
        );
    }
    let task = find_task_mut(&mut ctx.state.tasks, &options.task_id)?;
    let mut changes = Vec::new();
    if let Some(status) = &options.status {
        ensure_valid_status(status)?;
        task["status"] = json!(status);
        changes.push(format!("status={status}"));
    }
    if let Some(phase) = &options.phase {
        ensure_valid_phase(phase)?;
        task["phase"] = json!(phase);
        changes.push(format!("phase={phase}"));
    }
    if let Some(note) = &options.note {
        util::append_to_object_array(
            task,
            "evidence",
            json!({"kind": "manual", "summary": note, "recorded_at": util::iso_now()}),
        );
        changes.push("note".to_string());
    }
    if !options.touch.is_empty() {
        let touched = task
            .as_object_mut()
            .expect("task object")
            .entry("touched_files")
            .or_insert_with(|| Value::Array(Vec::new()));
        let touched = util::json_array_mut(touched);
        for path in &options.touch {
            if !touched
                .iter()
                .any(|value| value.as_str() == Some(path.as_str()))
            {
                touched.push(json!(path));
            }
        }
        changes.push(format!("touched+{}", options.touch.len()));
    }
    task["last_update"] = json!(util::iso_now());
    let event_phase = task
        .get("phase")
        .and_then(Value::as_str)
        .map(str::to_string);
    util::save_run(&mut ctx)?;
    if let Some(status) = &options.status {
        crate::events::safe_emit(
            repo,
            &ctx.run_id,
            crate::events::EventRecord {
                event_type: "task.status_changed".to_string(),
                actor_kind: "host".to_string(),
                phase: event_phase,
                task_id: Some(options.task_id.clone()),
                object_id: Some(options.task_id.clone()),
                object_type: Some("task".to_string()),
                summary: format!("task {} -> {}", options.task_id, status),
                fields: json!({"status": status}),
                ..crate::events::EventRecord::default()
            },
        );
    }
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    println!("task {} updated: {}", options.task_id, changes.join(", "));
    Ok(())
}

pub fn cmd_phase(repo: &Path, options: PhaseOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let current = ctx.state.current_phase.clone();
    let Some(to_phase) = options.set_phase else {
        println!("current phase: {current}");
        let transitions = util::json_array(&ctx.state.phase_transitions);
        if !transitions.is_empty() {
            println!("transitions:");
            for item in transitions
                .iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                println!(
                    "  {} -> {} @ {}",
                    item.get("from").and_then(Value::as_str).unwrap_or("?"),
                    item.get("to").and_then(Value::as_str).unwrap_or("?"),
                    item.get("at")
                        .and_then(Value::as_str)
                        .map(|value| value.chars().take(19).collect::<String>())
                        .unwrap_or_else(|| "?".to_string())
                );
            }
        }
        println!("valid phases: {}", util::VALID_PHASES.join(", "));
        return Ok(());
    };
    ensure_valid_phase(&to_phase)?;
    if to_phase == current {
        println!("already in phase '{current}' -- no change");
        return Ok(());
    }
    let head = util::git_status(repo).head;
    util::append_phase_transition(&mut ctx.state, &current, &to_phase, &head);
    util::save_run(&mut ctx)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "phase.changed".to_string(),
            actor_kind: "host".to_string(),
            phase: Some(to_phase.clone()),
            summary: format!("{current} -> {to_phase}"),
            fields: json!({"from": current, "to": to_phase}),
            ..crate::events::EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    util::sync_run_state_md(&ctx.run_dir.join("run-state.md"), &ctx.state)?;
    println!("phase: {current} -> {to_phase}");
    if matches!(to_phase.as_str(), "implementation" | "closed") {
        println!(
            "  note: `lto check --to {to_phase}` reports the phase-evidence checklist; `closeout` is the gated way to finish."
        );
    }
    Ok(())
}

pub fn cmd_collect_agent_run(repo: &Path, options: CollectAgentRunOptions) -> anyhow::Result<()> {
    if !util::KNOWN_RUNNERS.contains(&options.runner.as_str()) {
        anyhow::bail!("unknown runner: {}", options.runner);
    }
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let task = find_task_mut(&mut ctx.state.tasks, &options.task_id)?;
    let reply_path = absolutize(repo, &options.reply);
    let reply_text = util::read_to_string_lossy(&reply_path)
        .with_context(|| format!("reply file not found: {}", reply_path.display()))?;
    let meta_path = options
        .meta
        .as_ref()
        .map(|path| absolutize(repo, path))
        .unwrap_or_else(|| {
            reply_path.with_file_name(format!(
                "{}.meta.json",
                reply_path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("reply")
            ))
        });
    let meta = fs::read_to_string(meta_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    let status = options.status.unwrap_or_else(|| {
        if reply_text.trim().is_empty() {
            "failed".to_string()
        } else {
            "ok".to_string()
        }
    });
    let parsed_status = util::parse_status(&status)?;
    let canonical_status = parsed_status.as_str();
    let mut cost = BTreeMap::new();
    for key in ["tokens", "tokens_in", "tokens_out"] {
        if let Some(value) = meta.get(key).and_then(Value::as_u64) {
            cost.insert(key.to_string(), json!(value));
        }
    }
    if let Some(elapsed) = options.elapsed_sec {
        cost.insert("elapsed_sec".to_string(), json!(elapsed));
    }
    let result = AgentResult {
        job_id: options.task_id.clone(),
        runner: options.runner.clone(),
        model: options.model,
        status: parsed_status,
        exit_code: None,
        findings: Vec::new(),
        reply_text,
        cost,
        permissions: BTreeMap::new(),
        artifacts: vec![
            util::repo_relative_path(repo, &reply_path)
                .unwrap_or_else(|_| reply_path.display().to_string()),
        ],
        attempts: 1,
        error: if parsed_status == JobStatus::Ok {
            String::new()
        } else {
            options
                .note
                .clone()
                .unwrap_or_else(|| "empty or failed reply".to_string())
        },
        task_type: None,
        size: TaskSize::Unknown,
        merge_review: None,
    };
    let result_value = serde_json::to_value(&result)?;
    let agent_runs = util::json_object_mut(&mut ctx.state.agent_runs);
    let entries = agent_runs
        .entry(options.task_id.clone())
        .or_insert_with(|| Value::Array(Vec::new()));
    util::json_array_mut(entries).push(result_value);
    util::append_to_object_array(
        task,
        "evidence",
        json!({
            "kind": "manual",
            "summary": format!("collected {} dispatch{}", options.runner, options.note.as_ref().map(|n| format!(": {n}")).unwrap_or_default()),
            "recorded_at": util::iso_now(),
        }),
    );
    task["last_update"] = json!(util::iso_now());
    // Emit event BEFORE saving state. If safe_emit fails (events.lock timeout,
    // hard-stop reached, disk full, etc.), bail before state is written. This
    // prevents permanent divergence between state.agent_runs (read by
    // autonomous_gate) and events.jsonl (read by cross_run_evidence).
    // safe_emit remains fail-closed (⑫) — the caller now reacts instead of
    // silently ignoring the failure.
    let emitted = crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "runner.finished".to_string(),
            actor_kind: "runner".to_string(),
            actor_id: Some(options.runner.clone()),
            phase: Some(ctx.state.current_phase.clone()),
            task_id: Some(options.task_id.clone()),
            object_id: Some(options.task_id.clone()),
            object_type: Some("task".to_string()),
            summary: format!("collected {} status={canonical_status}", options.runner),
            fields: json!({
                "runner": options.runner.clone(),
                "model": result.model,
                "status": canonical_status,
                "tokens": meta.get("tokens").cloned().unwrap_or(Value::Null),
                "elapsed_sec": options.elapsed_sec,
            }),
            ..crate::events::EventRecord::default()
        },
    );
    if emitted.is_none() {
        anyhow::bail!(
            "event emit failed for runner.finished (task {}); state not saved to keep state/events consistent",
            options.task_id
        );
    }
    util::save_run(&mut ctx)?;
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    println!(
        "collected {} run for task {}: status={canonical_status}",
        options.runner, options.task_id
    );
    Ok(())
}

pub fn cmd_hook(repo: &Path, options: HookOptions) -> anyhow::Result<()> {
    match options.gate.as_str() {
        "pre-commit" => hook_pre_commit(repo, options),
        "pre-deploy" => hook_pre_deploy(repo),
        "pre-closeout" => hook_pre_closeout(repo),
        other => anyhow::bail!("unknown gate: {other}"),
    }
}

/// Submit jobs loaded from a `--job-file` and record the whole lifecycle:
/// started event, submission-failure event, then results either into run state
/// or as a bare event when the run cannot be loaded.
///
/// `context` is the event source label that distinguishes the three callers
/// (`runner.job_file`, `run.parallel`, `run.pipeline`) — it is their only
/// difference. Prompt/judge/autopilot submissions do NOT belong here: they
/// carry a phase, escalate on failure, or convert errors into a held state.
fn run_job_file(
    repo: &Path,
    jobs: Vec<AgentJob>,
    run_id: Option<String>,
    context: &str,
) -> anyhow::Result<()> {
    if let Some(run_id) = &run_id {
        crate::event_emit::emit_runner_started_jobs(repo, run_id, None, None, context, &jobs);
    }
    let results = match submit_jobs(repo, jobs.clone()) {
        Ok(results) => results,
        Err(err) => {
            if let Some(run_id) = &run_id {
                crate::event_emit::emit_runner_submission_failed_jobs(
                    repo,
                    run_id,
                    None,
                    None,
                    context,
                    &jobs,
                    &err.to_string(),
                );
            }
            return Err(err);
        }
    };
    if let Some(run_id) = run_id {
        if let Ok(mut ctx) = util::load_run(repo, Some(&run_id)) {
            emit_and_record_runner_results(repo, &mut ctx, None, context, &results)?;
        } else {
            crate::event_emit::emit_runner_results(repo, &run_id, None, None, context, &results);
            let _ = crate::telemetry::save(repo, &run_id);
        }
    }
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

pub fn cmd_parallel(repo: &Path, options: ParallelOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let jobs = load_jobs(job_file)?;
        let run_id = job_file_run_id(repo, options.run_id.as_deref(), &jobs)?;
        return run_job_file(repo, jobs, run_id, "run.parallel");
    }
    run_many_task_commands(repo, options)
}

pub fn cmd_pipeline(repo: &Path, options: PipelineOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let jobs = load_jobs(job_file)?;
        let run_id = job_file_run_id(repo, options.run_id.as_deref(), &jobs)?;
        return run_job_file(repo, jobs, run_id, "run.pipeline");
    }
    run_pipeline_task_commands(repo, options)
}

fn submit_jobs(repo: &Path, jobs: Vec<AgentJob>) -> anyhow::Result<Vec<AgentResult>> {
    let scheduler = Scheduler::new(repo, repo.join("scripts").join("delegate").join("runners"));
    Ok(scheduler.submit_blocking(jobs)?)
}

fn load_jobs(path: &Path) -> anyhow::Result<Vec<AgentJob>> {
    let text = fs::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&text)?;
    if value.is_array() {
        Ok(serde_json::from_value(value)?)
    } else if let Some(jobs) = value.get("jobs") {
        Ok(serde_json::from_value(jobs.clone())?)
    } else {
        Ok(vec![serde_json::from_value(value)?])
    }
}

fn common_job_run_id(jobs: &[AgentJob]) -> Option<String> {
    let mut run_id = None;
    for job in jobs {
        let Some(candidate) = job.meta.get("run_id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(existing) = &run_id
            && existing != candidate
        {
            return None;
        }
        run_id = Some(candidate.to_string());
    }
    run_id
}

fn job_file_run_id(
    repo: &Path,
    explicit_run_id: Option<&str>,
    jobs: &[AgentJob],
) -> anyhow::Result<Option<String>> {
    if explicit_run_id.is_some() {
        return util::resolve_run_id(repo, explicit_run_id).map(Some);
    }
    Ok(common_job_run_id(jobs))
}

fn emit_and_record_runner_results(
    repo: &Path,
    ctx: &mut util::RunContext,
    task_id: Option<&str>,
    context: &str,
    results: &[AgentResult],
) -> anyhow::Result<()> {
    crate::event_emit::emit_runner_results_checked(
        repo,
        &ctx.run_id,
        Some(ctx.state.current_phase.as_str()),
        task_id,
        context,
        results,
    )?;
    util::append_agent_results_to_state(&mut ctx.state, task_id, results)?;
    util::save_run(ctx)?;
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    Ok(())
}

fn runner_job_meta(options: &RunnerOptions, run_id: &str) -> BTreeMap<String, Value> {
    let mut meta = BTreeMap::from([("run_id".to_string(), json!(run_id))]);
    if options.runner != "tmux" {
        return meta;
    }
    if let Some(value) = &options.tmux_target {
        meta.insert("tmux_target".to_string(), json!(value));
    }
    if let Some(value) = &options.tmux_mode {
        meta.insert("tmux_mode".to_string(), json!(value));
    }
    if let Some(value) = &options.tmux_sentinel {
        meta.insert(
            "tmux_sentinel".to_string(),
            json!(value.display().to_string()),
        );
    }
    if let Some(value) = &options.tmux_session {
        meta.insert("tmux_session".to_string(), json!(value));
    }
    if options.tmux_new_window {
        meta.insert("tmux_new_window".to_string(), json!(true));
    }
    if options.tmux_new_session {
        meta.insert("tmux_new_session".to_string(), json!(true));
    }
    if let Some(value) = &options.tmux_window_name {
        meta.insert("tmux_window_name".to_string(), json!(value));
    }
    if !options.tmux_ready_patterns.is_empty() {
        meta.insert(
            "tmux_ready_patterns".to_string(),
            json!(options.tmux_ready_patterns),
        );
    }
    if !options.tmux_skip_prompts.is_empty() {
        meta.insert(
            "tmux_skip_prompts".to_string(),
            json!(options.tmux_skip_prompts),
        );
    }
    if let Some(value) = options.tmux_ready_timeout_sec {
        meta.insert("tmux_ready_timeout_sec".to_string(), json!(value));
    }
    if let Some(value) = &options.tmux_bin {
        meta.insert("tmux_bin".to_string(), json!(value));
    }
    meta
}

fn run_task_command(repo: &Path, options: RunnerOptions) -> anyhow::Result<()> {
    let task_id = options
        .task_id
        .as_ref()
        .context("--task-id is required when --command is used")?
        .clone();
    let command = options
        .command
        .as_ref()
        .context("--command is required without --prompt/--job-file")?
        .clone();
    ensure_valid_kind(&options.kind)?;
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let inherited_ref = find_task_mut(&mut ctx.state.tasks, &task_id)?
        .get("instrument_ref")
        .and_then(Value::as_str)
        .map(str::to_string);
    let instrument_ref = crate::run_observability::resolve_instrument_ref(
        &ctx.state.delivery_contract,
        options.instrument_ref.as_deref(),
        inherited_ref.as_deref(),
        &command,
    )?;
    let task = find_task_mut(&mut ctx.state.tasks, &task_id)?;
    let cwd = options.cwd.as_deref().unwrap_or(repo);
    let head_before = util::git_status(repo).head;
    let (rc, stdout, stderr, elapsed) =
        util::run_command_capture(repo, &command, Some(cwd), options.timeout)?;
    let head_after = util::git_status(repo).head;
    let mut evidence = json!({
        "kind": options.kind,
        "command": command,
        "cwd": cwd.display().to_string(),
        "rc": rc,
        "started_at": Value::Null,
        "ended_at": util::iso_now(),
        "head_before": head_before,
        "head_after": head_after,
        "verified_by": "runner",
        "summary": options.note.clone().unwrap_or_else(|| format!("{}: {}", options.kind, if rc == 0 { "PASS" } else if rc == 124 { "TIMEOUT" } else { "FAIL" })),
        "stdout_tail": tail_lines(&stdout, 20),
        "stderr_tail": tail_lines(&stderr, 20),
        "elapsed_sec": elapsed,
    });
    if let Some(instrument_ref) = &instrument_ref {
        evidence["instrument_ref"] = json!(instrument_ref);
    }
    util::append_to_object_array(task, "evidence", evidence.clone());
    append_unique_strings(task, "touched_files", &options.touch);
    append_string(task, "commands_run", &command);
    if rc == 0 {
        task["status"] = json!("done");
        resolve_blockers(task, &evidence);
        if options.kind == "test" {
            ctx.state.gates["last_tested_head"] = json!(head_after);
        }
    } else {
        let retry_count = bump_retry(task, &command);
        task["status"] = json!(options.status_on_fail);
        util::append_to_object_array(
            task,
            "blockers",
            json!({"reason": format!("command failed (rc={rc})"), "command": command, "evidence_kind": options.kind, "at": util::iso_now()}),
        );
        ctx.state.last_failure = json!(format!(
            "{task_id}: {} rc={rc} (retry {retry_count})",
            options.kind
        ));
    }
    task["last_update"] = json!(util::iso_now());
    util::save_run(&mut ctx)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "runner.finished".to_string(),
            actor_kind: "runner".to_string(),
            actor_id: Some("lto-runner".to_string()),
            phase: Some(ctx.state.current_phase.clone()),
            task_id: Some(task_id.clone()),
            object_id: Some(task_id.clone()),
            object_type: Some("task".to_string()),
            summary: format!("{} rc={rc}", options.kind),
            fields: {
                let mut fields = json!({
                "kind": options.kind,
                "command_hash": format!("{:x}", sha2::Sha256::digest(command.as_bytes())),
                "rc": rc,
                "status": if rc == 0 { "ok" } else if rc == 124 { "timeout" } else { "failed" },
                "elapsed_sec": elapsed,
                "timeout": rc == 124,
                });
                if let Some(instrument_ref) = &instrument_ref {
                    fields["instrument_ref"] = json!(instrument_ref);
                }
                fields
            },
            ..crate::events::EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    println!(
        "{} [{}] {} rc={rc}",
        if rc == 0 { "PASS" } else { "FAIL" },
        options.kind,
        command
    );
    if rc == 0 {
        Ok(())
    } else {
        anyhow::bail!("runner command failed rc={rc}")
    }
}

fn cmd_state_judge(repo: &Path, options: JudgeOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let head = util::git_status(repo).head;
    let tasks = filter_tasks(
        &ctx.state.tasks,
        options.task_id.as_deref(),
        options.phase.as_deref(),
    );
    if tasks.is_empty() {
        println!(
            "no tasks to judge (phase={})",
            options
                .phase
                .unwrap_or_else(|| ctx.state.current_phase.clone())
        );
        return Ok(());
    }
    let mut test_results = Vec::new();
    if options.rerun_tests {
        for task in &tasks {
            for evidence in task
                .get("evidence")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                if evidence.get("kind").and_then(Value::as_str) == Some("test")
                    && evidence.get("rc").and_then(Value::as_i64) == Some(0)
                    && let Some(command) = evidence.get("command").and_then(Value::as_str)
                {
                    let (rc, _, _, _) = util::run_command_capture(repo, command, Some(repo), 120)?;
                    test_results.push(json!({"command": command, "result": if rc == 0 { "pass" } else { "fail" }, "rc": rc}));
                }
            }
        }
    }
    let verdict = build_state_verdict(&tasks, &test_results, &head, &options);
    let judge_dir = ctx.run_dir.join("judge");
    fs::create_dir_all(&judge_dir)?;
    let path = judge_dir.join(format!(
        "judge-{}-{}.yaml",
        ctx.state.current_phase,
        util::now_for_filename()
    ));
    fs::write(&path, &verdict)?;
    util::register_artifact(
        repo,
        &ctx.run_id,
        &path,
        util::ArtifactMeta {
            kind: "judge_verdict",
            producer: "lto_rs.commands.judge",
            state: &ctx.state,
            summary: "judge verdict",
            tags: &["judge", "verdict"],
        },
    )?;
    ctx.state.gates["last_reviewed_head"] = json!(head);
    util::save_run(&mut ctx)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "gate.evaluated".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some("judge".to_string()),
            object_type: Some("gate".to_string()),
            summary: format!(
                "judge verdict for {}",
                options.phase.unwrap_or_else(|| "all".to_string())
            ),
            fields: json!({
                "gate": "judge",
                "verdict_path": util::repo_relative_path(repo, &path).unwrap_or_else(|_| path.display().to_string()),
                "head": head,
            }),
            ..crate::events::EventRecord::default()
        },
    );
    let _ = crate::telemetry::save(repo, &ctx.run_id);
    println!("{verdict}");
    Ok(())
}

fn cmd_llm_judge(repo: &Path, options: JudgeOptions) -> anyhow::Result<()> {
    let case_dir = options
        .case_dir
        .clone()
        .unwrap_or_else(|| repo.join(".lto").join("judge-case"));
    let brief = read_required_path(options.brief.as_ref(), "--brief")?;
    let baseline = read_required_path(options.baseline_reply.as_ref(), "--baseline-reply")?;
    let candidate = read_required_path(options.candidate_reply.as_ref(), "--candidate-reply")?;
    let candidate_runner = options
        .candidate_runner
        .as_deref()
        .context("--candidate-runner is required for llm judge mode")?;
    let frozen = llm_judge::freeze_evidence(&case_dir, &brief, &baseline, &candidate)?;
    let plan = llm_judge::plan_judge_dispatch(
        repo,
        case_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("case"),
        candidate_runner,
        &frozen,
        options.judge_runner.as_deref(),
        Some(&repo.join("scripts").join("delegate").join("runners")),
    );
    if !options.execute {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    let run_ctx = if options.run_id.is_some() || repo.join(".lto").join("current").exists() {
        Some(util::load_run(repo, options.run_id.as_deref())?)
    } else {
        None
    };
    match plan {
        llm_judge::JudgeDispatchPlan::Ready { runner, job, .. } => {
            let mut job = *job;
            if let Some(ctx) = &run_ctx {
                job.meta.insert("run_id".to_string(), json!(ctx.run_id));
            }
            let jobs = vec![job];
            if let Some(ctx) = &run_ctx {
                crate::event_emit::emit_runner_started_jobs(
                    repo,
                    &ctx.run_id,
                    Some(ctx.state.current_phase.as_str()),
                    None,
                    "judge.llm",
                    &jobs,
                );
            }
            let results = match submit_jobs(repo, jobs.clone()) {
                Ok(results) => results,
                Err(err) => {
                    if let Some(ctx) = &run_ctx {
                        crate::event_emit::emit_runner_submission_failed_jobs(
                            repo,
                            &ctx.run_id,
                            Some(ctx.state.current_phase.as_str()),
                            None,
                            "judge.llm",
                            &jobs,
                            &err.to_string(),
                        );
                        crate::event_emit::emit_decision_escalated(
                            repo,
                            &ctx.run_id,
                            "judge scheduler failed",
                            json!({
                                "case_id": case_dir.file_name().and_then(|value| value.to_str()).unwrap_or("case"),
                                "judge_runner": runner,
                                "error": err.to_string(),
                            }),
                        );
                        let _ = crate::telemetry::save(repo, &ctx.run_id);
                    }
                    return Err(err);
                }
            };
            if let Some(ctx) = &run_ctx {
                crate::event_emit::emit_runner_results_checked(
                    repo,
                    &ctx.run_id,
                    Some(ctx.state.current_phase.as_str()),
                    None,
                    "judge.llm",
                    &results,
                )?;
                let mut updated_ctx = ctx.clone();
                util::append_agent_results_to_state(&mut updated_ctx.state, None, &results)?;
                util::save_run(&mut updated_ctx)?;
            }
            let Some(result) = results.first() else {
                if let Some(ctx) = &run_ctx {
                    crate::event_emit::emit_decision_escalated(
                        repo,
                        &ctx.run_id,
                        "judge result missing",
                        json!({
                            "case_id": case_dir.file_name().and_then(|value| value.to_str()).unwrap_or("case"),
                            "judge_runner": runner,
                        }),
                    );
                    let _ = crate::telemetry::save(repo, &ctx.run_id);
                }
                anyhow::bail!("judge produced no result");
            };
            let parsed = llm_judge::parse_judge_reply(&result.reply_text);
            let hash = llm_judge::freeze_verdict(
                &case_dir,
                &frozen.evidence_hash,
                Some(&runner),
                util::status_str(result),
                parsed.as_ref(),
                if result.error.is_empty() {
                    None
                } else {
                    Some(&result.error)
                },
            )?;
            if let Some(ctx) = &run_ctx {
                let case_id = case_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("case");
                if let Some(judgment) = &parsed {
                    crate::event_emit::emit_decision_voted(
                        repo,
                        &ctx.run_id,
                        &runner,
                        "llm_judge",
                        json!({
                            "case_id": case_id,
                            "judge_runner": runner,
                            "status": util::status_str(result),
                            "blocker_quality": format!("{:?}", judgment.blocker_quality).to_ascii_lowercase(),
                            "false_positive_suspected": judgment.false_positive_suspected,
                            "evidence_hash": frozen.evidence_hash,
                        }),
                    );
                } else {
                    crate::event_emit::emit_decision_escalated(
                        repo,
                        &ctx.run_id,
                        "judge reply did not parse",
                        json!({
                            "case_id": case_id,
                            "judge_runner": runner,
                            "result_status": result.status.as_str(),
                        }),
                    );
                }
                let _ = crate::telemetry::save(repo, &ctx.run_id);
            }
            println!("judge verdict: {hash}");
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        skipped => {
            let payload = serde_json::to_value(&skipped)?;
            if let Some(ctx) = &run_ctx {
                crate::event_emit::emit_judge_skipped(
                    repo,
                    &ctx.run_id,
                    case_dir
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("case"),
                    payload.get("reason").and_then(Value::as_str),
                );
                let _ = crate::telemetry::save(repo, &ctx.run_id);
            }
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn analyze_state(repo: &Path, state: &crate::state::LtoState) -> Value {
    let tasks = util::json_array(&state.tasks);
    let mut counts = Map::new();
    let mut blocked = Vec::new();
    let mut pending = Vec::new();
    let mut in_progress = Vec::new();
    let mut done = Vec::new();
    for task in tasks {
        let status = task
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let count = counts.get(status).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(status.to_string(), json!(count));
        let summary = json!({
            "id": task.get("id").and_then(Value::as_str).unwrap_or(""),
            "title": task.get("title").and_then(Value::as_str).unwrap_or(""),
            "status": status,
        });
        match status {
            "blocked" => blocked.push(summary),
            "pending" => pending.push(summary),
            "in_progress" => in_progress.push(summary),
            "done" => done.push(summary),
            _ => {}
        }
    }
    let actual_head = util::git_status(repo).head;
    let gates = &state.gates;
    let unresolved = gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let unverified = util::json_array(&state.risk_points)
        .iter()
        .filter(|risk| util::risk_is_open_unverified(risk))
        .count();
    let has_tasks = !tasks.is_empty();
    let done_count = counts.get("done").and_then(Value::as_u64).unwrap_or(0);
    let skipped_count = counts.get("skipped").and_then(Value::as_u64).unwrap_or(0);
    json!({
        "phase": state.current_phase,
        "has_tasks": has_tasks,
        "task_counts": counts,
        "total_tasks": tasks.len(),
        "all_done": has_tasks && done_count as usize == tasks.len(),
        "all_non_skipped_done": has_tasks && (done_count + skipped_count) as usize == tasks.len(),
        "blocked": blocked,
        "pending": pending,
        "in_progress": in_progress,
        "done": done,
        "unverified_risk_points": unverified,
        "has_high_risk_unreviewed": has_high_risk_task(&state.tasks),
        "last_failure": state.last_failure,
        "gates": {
            "last_tested_head": gates.get("last_tested_head").cloned().unwrap_or(Value::Null),
            "last_reviewed_head": gates.get("last_reviewed_head").cloned().unwrap_or(Value::Null),
            "actual_head": actual_head,
            "has_unresolved": !unresolved.is_empty(),
            "unresolved_blocks": unresolved,
        },
    })
}

fn route_next(facts: &Value) -> Value {
    let phase = facts.get("phase").and_then(Value::as_str).unwrap_or("");
    let has_tasks = facts
        .get("has_tasks")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let all_done = facts
        .get("all_non_skipped_done")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let blocked_count = facts
        .get("blocked")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let pending_count = facts
        .get("pending")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let in_progress_count = facts
        .get("in_progress")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let gates = facts.get("gates").unwrap_or(&Value::Null);
    let has_unresolved = gates
        .get("has_unresolved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let unverified = facts
        .get("unverified_risk_points")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if !has_tasks {
        return json!({
            "action": "escalate",
            "unambiguous": false,
            "reason": format!("phase '{phase}' has no tasks -- cannot auto-advance, host LLM must decide"),
        });
    }
    if all_done && !has_unresolved && unverified == 0 && util::VALID_PHASES.contains(&phase) {
        return json!({
            "action": "run",
            "argv": ["closeout", "--summary", "all tasks done (lto next)"],
            "cmd": "lto closeout --summary \"all tasks done (lto next)\"",
            "pattern": "linear",
            "unambiguous": true,
            "reason": "all tasks done, gates clear",
        });
    }
    if all_done && util::VALID_PHASES.contains(&phase) {
        return json!({
            "action": "run",
            "argv": ["judge", "--phase", phase],
            "cmd": format!("lto judge --phase {phase}"),
            "pattern": "judge",
            "unambiguous": true,
            "reason": format!("all tasks in phase '{phase}' done, needs judgement"),
        });
    }
    let mut parts = Vec::new();
    if blocked_count > 0 {
        parts.push(format!("{blocked_count} blocked tasks"));
    }
    if pending_count > 0 {
        parts.push(format!("{pending_count} pending tasks"));
    }
    if in_progress_count > 0 {
        parts.push(format!("{in_progress_count} in-progress tasks"));
    }
    json!({
        "action": "escalate",
        "unambiguous": false,
        "reason": if parts.is_empty() { "no unambiguous routing matches".to_string() } else { format!("ambiguous state: {}", parts.join(", ")) },
    })
}

fn decision_brief(state: &crate::state::LtoState, facts: &Value) -> String {
    let mut lines = vec![
        "# LTO Decision Brief".to_string(),
        String::new(),
        "This brief is deterministic state only. The host agent must decide the next pattern."
            .to_string(),
        String::new(),
        "## Current State".to_string(),
        String::new(),
        format!("- **Goal**: {}", state.goal),
        format!(
            "- **Phase**: {}",
            facts["phase"].as_str().unwrap_or("unknown")
        ),
        format!(
            "- **Tasks**: {} total (done={}, in_progress={}, blocked={}, pending={})",
            facts["total_tasks"],
            facts["task_counts"]
                .get("done")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            facts["task_counts"]
                .get("in_progress")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            facts["task_counts"]
                .get("blocked")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            facts["task_counts"]
                .get("pending")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        format!(
            "- **Unverified risk points**: {}",
            facts["unverified_risk_points"].as_u64().unwrap_or(0)
        ),
        String::new(),
        "## Candidate Actions".to_string(),
        String::new(),
    ];
    if facts["blocked"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
        > 0
    {
        lines.push("1. **Fix blocked tasks** -- `linear` or `fan-out`".to_string());
    }
    if facts["pending"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
        > 0
        || facts["in_progress"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
            > 0
    {
        lines.push("2. **Pursue remaining work** -- `linear` or `fan-out`".to_string());
    }
    if facts["all_non_skipped_done"].as_bool().unwrap_or(false) {
        lines.push("3. **Judge or closeout** -- gates determine the next primitive".to_string());
    }
    lines.push(String::new());
    lines.push("---".to_string());
    lines.push("Facts only; host reasoning remains authoritative.".to_string());
    lines.join("\n")
}

fn auto_exec_tasks(
    repo: &Path,
    ctx: &mut util::RunContext,
    options: &AutopilotOptions,
) -> anyhow::Result<()> {
    let mut executed = 0;
    let mut held = 0;
    let mut failed = 0;
    let mut retry_blocked = 0;
    let mut agent_results = Vec::<(String, AgentResult)>::new();
    let carrier = select_worker_carrier(options);
    for task in util::json_array_mut(&mut ctx.state.tasks) {
        let status = task
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();
        if !matches!(status.as_str(), "pending" | "in_progress") {
            continue;
        }
        let command = task
            .get("commands_run")
            .and_then(Value::as_array)
            .and_then(|items| items.last())
            .and_then(Value::as_str)
            .or_else(|| task.get("planned_command").and_then(Value::as_str))
            .map(str::to_string);
        let Some(command) = command else {
            continue;
        };
        let retry_count = task.get("retry_count").and_then(Value::as_u64).unwrap_or(0);
        if retry_count >= 3 {
            retry_blocked += 1;
            let id = task.get("id").and_then(Value::as_str).unwrap_or("?");
            crate::event_emit::emit_sandbox_rejected(
                repo,
                &ctx.run_id,
                id,
                &worktree::SandboxResult {
                    executed: false,
                    effect: crate::effect::classify_effect(&command),
                    rc: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    worktree: None,
                    note: format!("refused: retry_count={retry_count} >= 3"),
                },
            );
            println!("    [{id}] SKIP -- retry_count={retry_count} >= 3 (needs human)");
            continue;
        }
        let id = task
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        match carrier {
            WorkerCarrier::Sandbox => {
                let result = worktree::run_in_ephemeral_worktree(
                    repo,
                    &command,
                    !options.autonomous,
                    Duration::from_secs(options.timeout),
                )?;
                if !result.executed {
                    held += 1;
                    println!("    [{id}] HELD -- {}", result.note);
                    crate::event_emit::emit_sandbox_rejected(repo, &ctx.run_id, &id, &result);
                    continue;
                }
                executed += 1;
                let rc = result.rc.unwrap_or(1);
                println!("    [{id}] rc={rc} -- {}", truncate(&command, 80));
                util::append_to_object_array(
                    task,
                    "evidence",
                    json!({
                        "kind": "test",
                        "command": command,
                        "cwd": result.worktree.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
                        "rc": rc,
                        "summary": format!("autopilot sandbox: {}", if rc == 0 { "PASS" } else { "FAIL" }),
                        "verified_by": "autopilot",
                        "ended_at": util::iso_now(),
                        "carrier": "sandbox",
                    }),
                );
                if rc == 0 {
                    task["status"] = json!("done");
                    let evidence = task
                        .get("evidence")
                        .and_then(Value::as_array)
                        .and_then(|items| items.last())
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    resolve_blockers(task, &evidence);
                } else {
                    failed += 1;
                    bump_retry(task, &command);
                    task["status"] = json!("blocked");
                }
                task["last_update"] = json!(util::iso_now());
            }
            WorkerCarrier::Tmux => {
                let outcome = run_tmux_autopilot_worker(
                    repo,
                    &ctx.run_id,
                    ctx.state.current_phase.as_str(),
                    task,
                    &command,
                    options,
                )?;
                match outcome {
                    TmuxWorkerOutcome::Held { reason, result } => {
                        if let Some(result) = result {
                            agent_results.push((id.clone(), result));
                        }
                        held += 1;
                        println!("    [{id}] HELD -- {reason}");
                    }
                    TmuxWorkerOutcome::Ran { rc, job_id, result } => {
                        agent_results.push((id.clone(), result));
                        executed += 1;
                        println!("    [{id}] tmux rc={rc} job={job_id}");
                        if rc == 0 {
                            task["status"] = json!("done");
                            let evidence = task
                                .get("evidence")
                                .and_then(Value::as_array)
                                .and_then(|items| items.last())
                                .cloned()
                                .unwrap_or_else(|| json!({}));
                            resolve_blockers(task, &evidence);
                        } else {
                            failed += 1;
                            bump_retry(task, &command);
                            task["status"] = json!("blocked");
                        }
                        task["last_update"] = json!(util::iso_now());
                    }
                }
            }
        }
    }
    println!(
        "AUTOPILOT_STATUS: {}",
        if retry_blocked > 0 {
            "NEEDS_HUMAN"
        } else if failed > 0 {
            "NEEDS_HOST"
        } else if held > 0 {
            "NEEDS_CONFIRM"
        } else {
            "DONE"
        }
    );
    println!(
        "auto-exec: executed={executed} held={held} failed={failed} retry_blocked={retry_blocked}"
    );
    for (task_id, result) in agent_results {
        util::append_agent_results_to_state(&mut ctx.state, Some(&task_id), &[result])?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerCarrier {
    Sandbox,
    Tmux,
}

impl WorkerCarrier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Tmux => "tmux",
        }
    }
}

enum TmuxWorkerOutcome {
    Held {
        reason: String,
        result: Option<AgentResult>,
    },
    Ran {
        rc: i32,
        job_id: String,
        result: AgentResult,
    },
}

fn select_worker_carrier(options: &AutopilotOptions) -> WorkerCarrier {
    match options.worker_runner.as_str() {
        "sandbox" => WorkerCarrier::Sandbox,
        "tmux" => WorkerCarrier::Tmux,
        _ => {
            let tmux_bin = options.tmux_bin.as_deref().unwrap_or("tmux");
            let has_target = options.tmux_target.is_some();
            let has_tmux_env = std::env::var("TMUX")
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
                || std::env::var("TMUX_PANE")
                    .ok()
                    .is_some_and(|value| !value.trim().is_empty());
            if (has_target || has_tmux_env) && tmux_binary_available(tmux_bin) {
                WorkerCarrier::Tmux
            } else {
                WorkerCarrier::Sandbox
            }
        }
    }
}

fn tmux_binary_available(tmux_bin: &str) -> bool {
    Command::new(tmux_bin)
        .arg("-V")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_tmux_autopilot_worker(
    repo: &Path,
    run_id: &str,
    phase: &str,
    task: &mut Value,
    command: &str,
    options: &AutopilotOptions,
) -> anyhow::Result<TmuxWorkerOutcome> {
    let task_id = task
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("task")
        .to_string();
    let safe_id = sanitize_worker_id(&task_id);
    let job_id = format!("autopilot-{safe_id}");
    let contract_path = repo
        .join(".lto")
        .join(run_id)
        .join("live")
        .join(format!("{safe_id}.worker.json"));
    if let Some(parent) = contract_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(&contract_path);
    let prompt = tmux_worker_prompt(repo, command, &contract_path, &task_id)?;
    let mut meta = BTreeMap::from([
        ("run_id".to_string(), json!(run_id)),
        ("tmux_mode".to_string(), json!("signal")),
        (
            "tmux_window_name".to_string(),
            json!(format!("lto-{safe_id}")),
        ),
    ]);
    if let Some(target) = &options.tmux_target {
        meta.insert("tmux_target".to_string(), json!(target));
    }
    if let Some(tmux_bin) = &options.tmux_bin {
        meta.insert("tmux_bin".to_string(), json!(tmux_bin));
    }
    if let Some(timeout) = options.tmux_ready_timeout_sec {
        meta.insert("tmux_ready_timeout_sec".to_string(), json!(timeout));
    }
    let job = AgentJob {
        job_id: job_id.clone(),
        prompt_ref: prompt,
        runner: "tmux".to_string(),
        prompt_is_inline: true,
        model: None,
        env: BTreeMap::new(),
        permission_policy: readonly_intent_to_policy("tmux"),
        isolation: "none".to_string(),
        output_schema: None,
        parent_pattern: Pattern::Linear,
        budget: Budget {
            timeout_sec: options.timeout,
            max_tokens: None,
        },
        retry_policy: RetryPolicy {
            max_retries: 0,
            ..RetryPolicy::default()
        },
        verifier_of: None,
        children: Vec::new(),
        task_type: Some("autopilot_worker".to_string()),
        size: TaskSize::Small,
        test_cmd: None,
        needs_worktree: false,
        meta,
    };
    let jobs = vec![job];
    crate::event_emit::emit_runner_started_jobs(
        repo,
        run_id,
        None,
        Some(&task_id),
        "autopilot.tmux_worker",
        &jobs,
    );
    let results = match submit_jobs(repo, jobs.clone()) {
        Ok(results) => {
            crate::event_emit::emit_runner_results_checked(
                repo,
                run_id,
                Some(phase),
                Some(&task_id),
                "autopilot.tmux_worker",
                &results,
            )?;
            results
        }
        Err(err) => {
            crate::event_emit::emit_runner_submission_failed_jobs(
                repo,
                run_id,
                None,
                Some(&task_id),
                "autopilot.tmux_worker",
                &jobs,
                &err.to_string(),
            );
            return Ok(TmuxWorkerOutcome::Held {
                reason: format!("tmux worker submission failed: {err}"),
                result: None,
            });
        }
    };
    let Some(result) = results.into_iter().next() else {
        return Ok(TmuxWorkerOutcome::Held {
            reason: "tmux worker returned no result".to_string(),
            result: None,
        });
    };
    if result.status != JobStatus::Ok {
        let reason = format!("tmux worker {}: {}", result.status.as_str(), result.error);
        util::append_to_object_array(
            task,
            "evidence",
            tmux_worker_evidence(command, &contract_path, &job_id, &result, 1, &reason),
        );
        return Ok(TmuxWorkerOutcome::Held {
            reason,
            result: Some(result),
        });
    }
    let contract = read_tmux_worker_contract(&contract_path)?;
    let rc = contract
        .as_ref()
        .and_then(|value| value.get("rc"))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(1);
    let summary = if contract.is_some() {
        format!(
            "autopilot tmux worker: {}",
            if rc == 0 { "PASS" } else { "FAIL" }
        )
    } else {
        "autopilot tmux worker: missing completion contract".to_string()
    };
    util::append_to_object_array(
        task,
        "evidence",
        tmux_worker_evidence(command, &contract_path, &job_id, &result, rc, &summary),
    );
    Ok(TmuxWorkerOutcome::Ran { rc, job_id, result })
}

fn tmux_worker_prompt(
    repo: &Path,
    command: &str,
    contract_path: &Path,
    task_id: &str,
) -> anyhow::Result<String> {
    let parent = contract_path
        .parent()
        .context("worker contract path has no parent")?;
    let task_json = serde_json::to_string(task_id)?;
    let inner = format!(
        "cd {repo}\n\
         bash -lc {command}\n\
         lto_worker_rc=$?\n\
         mkdir -p {parent}\n\
         printf '{{\"task_id\":%s,\"rc\":%s,\"carrier\":\"tmux\"}}\\n' {task_json} \"$lto_worker_rc\" > {contract}\n\
         exit 0",
        repo = shell_single_quote(&repo.display().to_string()),
        command = shell_single_quote(command),
        parent = shell_single_quote(&parent.display().to_string()),
        task_json = shell_single_quote(&task_json),
        contract = shell_single_quote(&contract_path.display().to_string()),
    );
    Ok(format!("bash -lc {}", shell_single_quote(&inner)))
}

fn read_tmux_worker_contract(path: &Path) -> anyhow::Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn tmux_worker_evidence(
    command: &str,
    contract_path: &Path,
    job_id: &str,
    result: &AgentResult,
    rc: i32,
    summary: &str,
) -> Value {
    json!({
        "kind": "worker",
        "carrier": "tmux",
        "command": command,
        "job_id": job_id,
        "runner_status": result.status.as_str(),
        "runner_error": result.error,
        "contract": contract_path.display().to_string(),
        "rc": rc,
        "summary": summary,
        "verified_by": "autopilot",
        "ended_at": util::iso_now(),
        "reply_tail": tail_lines(&result.reply_text, 20),
    })
}

fn sanitize_worker_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "task".to_string()
    } else {
        sanitized
    }
}

fn update_autopilot_digest(ctx: &mut util::RunContext) -> anyhow::Result<()> {
    let digest = progress_digest(ctx);
    ctx.state.gates["autopilot_last_digest"] = digest.clone();
    let gates = util::json_object_mut(&mut ctx.state.gates);
    let high_water = gates
        .entry("progress_high_water".to_string())
        .or_insert_with(|| json!({"done": 0, "verified_risks": 0}));
    let high_water = util::json_object_mut(high_water);
    let done = digest.get("done").and_then(Value::as_u64).unwrap_or(0);
    let verified = digest
        .get("verified_risks")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let prev_done = high_water.get("done").and_then(Value::as_u64).unwrap_or(0);
    let prev_verified = high_water
        .get("verified_risks")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    high_water.insert("done".to_string(), json!(prev_done.max(done)));
    high_water.insert(
        "verified_risks".to_string(),
        json!(prev_verified.max(verified)),
    );
    util::save_run(ctx)
}

fn progress_digest(ctx: &util::RunContext) -> Value {
    let tasks = util::json_array(&ctx.state.tasks);
    let done = tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("done"))
        .count();
    let blocked = tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("blocked"))
        .collect::<Vec<_>>();
    let blocked_fp = blocked
        .iter()
        .filter_map(|task| {
            let id = task.get("id").and_then(Value::as_str)?;
            Some((id.to_string(), json!(failure_fingerprint(task))))
        })
        .collect::<Map<_, _>>();
    let rc0_evidence = tasks
        .iter()
        .filter_map(|task| {
            let id = task.get("id").and_then(Value::as_str)?;
            let count = task
                .get("evidence")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|evidence| evidence.get("rc").and_then(Value::as_i64) == Some(0))
                        .count()
                })
                .unwrap_or(0);
            Some((id.to_string(), json!(count)))
        })
        .collect::<Map<_, _>>();
    let ledger_blockers = ctx
        .state
        .gates
        .get("ledger_blockers")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let verified_risks = util::json_array(&ctx.state.risk_points)
        .iter()
        .filter(|risk| util::risk_is_verified(risk))
        .count();
    let projection = json!({
        "phase": ctx.state.current_phase,
        "tasks": tasks.iter().map(task_digest_projection).collect::<Vec<_>>(),
        "unresolved_blocks": unresolved_blocks(&ctx.state),
        "risk_points": ctx.state.risk_points,
        "ledger_blockers": ledger_blockers,
    });
    let artifact_hash = fs::read(ctx.run_dir.join("artifacts.json"))
        .map(|bytes| format!("{:x}", Sha256::digest(&bytes)))
        .unwrap_or_default();
    json!({
        "state_hash": format!("{:x}", Sha256::digest(serde_json::to_vec(&projection).unwrap_or_default())),
        "artifact_hash": artifact_hash,
        "done": done,
        "blocked_count": blocked.len(),
        "blocked_fp": blocked_fp,
        "rc0_evidence": rc0_evidence,
        "ledger_blockers": ledger_blockers,
        "verified_risks": verified_risks,
    })
}

fn task_digest_projection(task: &Value) -> Value {
    json!({
        "id": task.get("id").cloned().unwrap_or(Value::Null),
        "status": task.get("status").cloned().unwrap_or(Value::Null),
        "commands_run": task.get("commands_run").cloned().unwrap_or(Value::Null),
        "blockers": task.get("blockers").cloned().unwrap_or(Value::Null),
        "retry_count": task.get("retry_count").cloned().unwrap_or(Value::Null),
        "failure_fp": failure_fingerprint(task),
        "rc0_count": task
            .get("evidence")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter(|evidence| evidence.get("rc").and_then(Value::as_i64) == Some(0)).count())
            .unwrap_or(0),
    })
}

fn has_progressed(prev: &Value, curr: &Value) -> (bool, String) {
    let Some(prev_obj) = prev.as_object() else {
        return (true, "first step (no baseline)".to_string());
    };
    if prev_obj.is_empty() {
        return (true, "first step (no baseline)".to_string());
    }
    let prev_done = json_u64_field(prev, "done");
    let curr_done = json_u64_field(curr, "done");
    if curr_done > prev_done {
        return (true, format!("done {prev_done}->{curr_done}"));
    }
    let prev_ledger = json_u64_field(prev, "ledger_blockers");
    let curr_ledger = json_u64_field(curr, "ledger_blockers");
    if curr_ledger < prev_ledger {
        return (
            true,
            format!("ledger blockers {prev_ledger}->{curr_ledger}"),
        );
    }
    let prev_risks = json_u64_field(prev, "verified_risks");
    let curr_risks = json_u64_field(curr, "verified_risks");
    if curr_risks > prev_risks {
        return (true, format!("verified risks {prev_risks}->{curr_risks}"));
    }
    let prev_blocked = json_u64_field(prev, "blocked_count");
    let curr_blocked = json_u64_field(curr, "blocked_count");
    if curr_blocked < prev_blocked && blocked_task_got_success(prev, curr) {
        return (
            true,
            format!("blocked {prev_blocked}->{curr_blocked} with passing evidence"),
        );
    }
    if failure_fingerprints_changed(prev, curr) {
        return (true, "failure fingerprint changed".to_string());
    }
    if prev.get("artifact_hash").and_then(Value::as_str)
        != curr.get("artifact_hash").and_then(Value::as_str)
    {
        return (true, "artifact manifest changed".to_string());
    }
    (
        false,
        "no monotone improvement; same progress digest".to_string(),
    )
}

fn blocked_task_got_success(prev: &Value, curr: &Value) -> bool {
    let prev_blocked = prev
        .get("blocked_fp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let curr_blocked = curr
        .get("blocked_fp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for id in prev_blocked
        .keys()
        .filter(|id| !curr_blocked.contains_key(*id))
    {
        let prev_count = prev
            .get("rc0_evidence")
            .and_then(|value| value.get(id))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let curr_count = curr
            .get("rc0_evidence")
            .and_then(|value| value.get(id))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if curr_count > prev_count {
            return true;
        }
    }
    false
}

fn failure_fingerprints_changed(prev: &Value, curr: &Value) -> bool {
    let prev_blocked = prev
        .get("blocked_fp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let curr_blocked = curr
        .get("blocked_fp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    prev_blocked.iter().any(|(id, prev_fp)| {
        curr_blocked
            .get(id)
            .is_some_and(|curr_fp| curr_fp != prev_fp)
    })
}

fn failure_fingerprint(task: &Value) -> String {
    let Some(failed) = task
        .get("evidence")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .rev()
                .find(|evidence| evidence.get("rc").and_then(Value::as_i64).unwrap_or(0) != 0)
        })
    else {
        return String::new();
    };
    let rc = failed.get("rc").and_then(Value::as_i64).unwrap_or(1);
    let stderr_tail = failed
        .get("stderr_tail")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    format!(
        "{:x}",
        Sha256::digest(format!("{rc}\n{stderr_tail}").as_bytes())
    )
}

fn json_u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

const AUTONOMOUS_MIN_AGENT_RUNS: u64 = 5;
const AUTONOMOUS_MIN_AGENT_RESULTS: u64 = 10;
const AUTONOMOUS_MIN_EVIDENCE_RUNS: usize = 5;
const AUTONOMOUS_MIN_EVIDENCE_RUNS_SUM: usize = 10;
const AUTONOMOUS_RECENT_COMPLETIONS: usize = 20;
const AUTONOMOUS_FAILURE_RATE_MIN_SAMPLES: usize = 5;
const AUTONOMOUS_FAILURE_RATE_LIMIT: f64 = 0.5;
const AUTONOMOUS_COLD_FAILURE_STREAK: usize = 3;
const AUTONOMOUS_FAILURE_WARNING_STREAK: usize = 2;

fn autonomous_gate(
    repo: &Path,
    state: &crate::state::LtoState,
) -> crate::autonomous_gate::GateReport {
    crate::autonomous_gate::GateReport {
        operational_reliability: operational_reliability(repo),
        current_run_observability: crate::run_observability::assess(state),
    }
}

fn operational_reliability(repo: &Path) -> crate::autonomous_gate::ReliabilityReport {
    let mut runs = 0_u64;
    let mut results = 0_u64;
    let Ok(entries) = fs::read_dir(repo.join(".lto")) else {
        return crate::autonomous_gate::ReliabilityReport::fail("no .lto directory");
    };
    for entry in entries.flatten() {
        let path = entry.path().join("state.json");
        if !path.exists() {
            continue;
        }
        if let Ok(state) = crate::state::load_state(&path) {
            let count = util::iter_agent_runs(&state.agent_runs).len() as u64;
            if count > 0 {
                runs += 1;
                results += count;
            }
        }
    }
    if runs < AUTONOMOUS_MIN_AGENT_RUNS || results < AUTONOMOUS_MIN_AGENT_RESULTS {
        return crate::autonomous_gate::ReliabilityReport::fail(format!(
            "autonomous requires >={AUTONOMOUS_MIN_AGENT_RUNS} real agent-run runs and >={AUTONOMOUS_MIN_AGENT_RESULTS} results; current {runs}/{results}"
        ));
    }
    let evidence = match crate::telemetry::cross_run_evidence(repo) {
        Ok(evidence) => evidence,
        Err(err) => {
            return crate::autonomous_gate::ReliabilityReport::fail(format!(
                "cross-run evidence unavailable: {err}"
            ));
        }
    };
    if evidence.entries.is_empty() {
        return crate::autonomous_gate::ReliabilityReport::fail(
            "cross-run evidence has no runner.finished or agent.dispatch.completed entries",
        );
    }
    let evidence_distinct_runs = evidence
        .entries
        .iter()
        .map(|entry| entry.distinct_runs)
        .sum::<usize>();
    if evidence.run_count < AUTONOMOUS_MIN_EVIDENCE_RUNS
        || evidence_distinct_runs < AUTONOMOUS_MIN_EVIDENCE_RUNS_SUM
    {
        return crate::autonomous_gate::ReliabilityReport::fail(format!(
            "autonomous requires cross-run evidence >={AUTONOMOUS_MIN_EVIDENCE_RUNS} runs and >={AUTONOMOUS_MIN_EVIDENCE_RUNS_SUM} distinct-runs sum; current {}/{}",
            evidence.run_count, evidence_distinct_runs
        ));
    }
    if evidence
        .entries
        .iter()
        .all(|entry| entry.subjective_non_measurement)
    {
        return crate::autonomous_gate::ReliabilityReport::fail(
            "cross-run evidence is only subjective/non-measurement runs",
        );
    }
    let recent = crate::autonomous_gate::assess_recent_reliability(
        &evidence,
        AUTONOMOUS_RECENT_COMPLETIONS,
        AUTONOMOUS_FAILURE_RATE_MIN_SAMPLES,
        AUTONOMOUS_FAILURE_RATE_LIMIT,
        AUTONOMOUS_COLD_FAILURE_STREAK,
        AUTONOMOUS_FAILURE_WARNING_STREAK,
    );
    if let Some(failure) = recent.failure {
        return crate::autonomous_gate::ReliabilityReport::fail(failure);
    }
    crate::autonomous_gate::ReliabilityReport::pass(
        format!(
            "{runs} state-agent-run runs / {results} results; evidence runs={} distinct-runs sum={evidence_distinct_runs}",
            evidence.run_count
        ),
        recent.warnings,
    )
}

fn memory_projection(repo: &Path, ctx: &util::RunContext) -> anyhow::Result<Value> {
    let state_bytes = fs::read(&ctx.state_path)?;
    let artifact_path = ctx.run_dir.join("artifacts.json");
    let artifact_bytes = fs::read(&artifact_path).unwrap_or_default();
    let repo_name = repo
        .canonicalize()
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .or_else(|| {
            repo.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "project".to_string());
    let tasks = util::json_array(&ctx.state.tasks)
        .iter()
        .map(|task| {
            json!({
                "id": task.get("id").and_then(Value::as_str).unwrap_or(""),
                "title": task.get("title").and_then(Value::as_str).unwrap_or(""),
                "status": task.get("status").and_then(Value::as_str).unwrap_or(""),
                "phase": task.get("phase").and_then(Value::as_str).unwrap_or(""),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "schema_version": 1,
        "project": repo_name,
        "records": [{
            "kind": "lto_run_snapshot",
            "run_id": ctx.run_id,
            "request_hash": format!("sha256:{}", sha256_hex(ctx.state.original_user_request.as_bytes())),
            "goal_redacted": redact_short(&ctx.state.goal, 240),
            "why_redacted": redact_short(&ctx.state.why, 240),
            "done_when_redacted": redact_short(&ctx.state.done_when, 240),
            "phase": ctx.state.current_phase,
            "delivery_contract": delivery_contract_projection(&ctx.state.delivery_contract),
            "state_hash": format!("sha256:{}", sha256_hex(&state_bytes)),
            "artifact_hash": format!("sha256:{}", sha256_hex(&artifact_bytes)),
            "tasks": tasks,
            "source": "local .lto",
        }]
    }))
}

fn delivery_contract_projection(contract: &crate::state::DeliveryContract) -> Value {
    json!({
        "present": !contract.is_empty(),
        "complete": !contract.is_empty() && contract.is_complete(),
        "target_count": contract.targets.len(),
        "constraint_count": contract.constraints.len(),
        "instrument_count": contract.instruments.len(),
        "forced_entropy_count": contract.forced_entropy.len(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn redact_short(value: &str, max_len: usize) -> String {
    let redacted = llm_judge::redact_text(value);
    let line = util::single_line(&redacted);
    if line.chars().count() <= max_len {
        line
    } else {
        format!("{}...", line.chars().take(max_len).collect::<String>())
    }
}

fn publish_am(projection: &Value, am_bin: Option<&str>, timeout: u64) -> anyhow::Result<String> {
    let bin = am_bin.unwrap_or("am");
    let mut child = Command::new(bin)
        .args(["ingest", "-f", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {bin}"))?;
    {
        let stdin = child.stdin.as_mut().context("am stdin unavailable")?;
        stdin.write_all(serde_json::to_string(projection)?.as_bytes())?;
    }
    wait_child_output(child, timeout)
}

fn resume_am(project: &str, am_bin: Option<&str>, timeout: u64) -> anyhow::Result<String> {
    let bin = am_bin.unwrap_or("am");
    let child = Command::new(bin)
        .args(["search", "技术", project])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {bin}"))?;
    wait_child_output(child, timeout)
}

fn wait_child_output(mut child: std::process::Child, timeout: u64) -> anyhow::Result<String> {
    let started = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            let _ = child.kill();
            anyhow::bail!("command timed out after {timeout}s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn print_memory_capsule(repo: &Path, ctx: &util::RunContext) -> anyhow::Result<()> {
    let projection = memory_projection(repo, ctx)?;
    let run = projection
        .get("records")
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    println!("=== LTO MEMORY LOCAL CAPSULE ===");
    println!("Run ID: {}", ctx.run_id);
    println!("Goal: {}", ctx.state.goal);
    println!("Phase: {}", ctx.state.current_phase);
    println!("Recorded Head: {}", truncate(&ctx.state.workspace.head, 12));
    println!(
        "Current Head: {}",
        truncate(&util::git_status(repo).head, 12)
    );
    println!(
        "Tasks: {}",
        util::json_array(&ctx.state.tasks)
            .iter()
            .rev()
            .take(5)
            .map(|task| format!(
                "{}:{}",
                task.get("id").and_then(Value::as_str).unwrap_or("?"),
                task.get("status").and_then(Value::as_str).unwrap_or("?")
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "Projection Drift: state_hash={} artifact_hash={}",
        truncate(
            run.get("state_hash").and_then(Value::as_str).unwrap_or(""),
            12
        ),
        truncate(
            run.get("artifact_hash")
                .and_then(Value::as_str)
                .unwrap_or(""),
            12
        )
    );
    println!("Local .lto remains source of truth; memory resume did not modify files.");
    println!("================================");
    Ok(())
}

fn run_many_task_commands(repo: &Path, options: ParallelOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let task_ids = select_task_ids(
        &ctx.state.tasks,
        &options.task_ids,
        options.phase.as_deref(),
    );
    if task_ids.is_empty() {
        println!("no tasks to run");
        return Ok(());
    }
    println!(
        "◆ LTO Parallel: {} tasks ({} concurrent)",
        task_ids.len(),
        options.concurrency.min(task_ids.len()).max(1)
    );
    let mut passed = 0;
    let total = task_ids.len();
    for task_id in task_ids {
        let command = task_command(&ctx.state.tasks, &task_id)
            .or_else(|| options.command.clone())
            .unwrap_or_else(|| "true".to_string());
        let runner_options = RunnerOptions {
            run_id: Some(ctx.run_id.clone()),
            task_id: Some(task_id.clone()),
            kind: options.kind.clone(),
            command: Some(command),
            cwd: None,
            timeout: options.timeout,
            touch: Vec::new(),
            note: None,
            instrument_ref: None,
            status_on_fail: "blocked".to_string(),
            runner: "codex".to_string(),
            allow_headless_write: false,
            prompt: None,
            prompt_file: None,
            job_file: None,
            job_id: None,
            tmux_target: None,
            tmux_mode: None,
            tmux_sentinel: None,
            tmux_session: None,
            tmux_new_window: false,
            tmux_new_session: false,
            tmux_window_name: None,
            tmux_ready_patterns: Vec::new(),
            tmux_skip_prompts: Vec::new(),
            tmux_ready_timeout_sec: None,
            tmux_bin: None,
        };
        match run_task_command(repo, runner_options) {
            Ok(()) => {
                passed += 1;
                println!("  OK {task_id}");
            }
            Err(err) => println!("  FAIL {task_id}: {err}"),
        }
        ctx = util::load_run(repo, Some(&ctx.run_id))?;
    }
    println!("◆ {passed}/{total} passed");
    if passed == total {
        Ok(())
    } else {
        anyhow::bail!("parallel failed: {passed}/{total} passed")
    }
}

fn run_pipeline_task_commands(repo: &Path, options: PipelineOptions) -> anyhow::Result<()> {
    if options.stages.is_empty() {
        anyhow::bail!("no stages specified; use --stages 'cmd1' 'cmd2' ...");
    }
    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    let task_ids = select_task_ids(
        &ctx.state.tasks,
        &options.task_ids,
        options.phase.as_deref(),
    );
    if task_ids.is_empty() {
        println!("no items to pipeline");
        return Ok(());
    }
    println!(
        "◆ LTO Pipeline: {} items x {} stages ({} concurrent)",
        task_ids.len(),
        options.stages.len(),
        options.concurrency.min(task_ids.len()).max(1)
    );
    let mut passed = 0;
    let mut total = 0;
    for task_id in task_ids {
        let mut task_ok = true;
        for (idx, stage) in options.stages.iter().enumerate() {
            total += 1;
            let command = stage.replace("{task_id}", &task_id);
            let runner_options = RunnerOptions {
                run_id: Some(ctx.run_id.clone()),
                task_id: Some(task_id.clone()),
                kind: options.kind.clone(),
                command: Some(command),
                cwd: None,
                timeout: options.timeout,
                touch: Vec::new(),
                note: Some(format!("stage {idx}")),
                instrument_ref: None,
                status_on_fail: "blocked".to_string(),
                runner: "codex".to_string(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            };
            if run_task_command(repo, runner_options).is_ok() {
                passed += 1;
            } else {
                task_ok = false;
                if !options.continue_on_error {
                    break;
                }
            }
        }
        println!("  {} {task_id}", if task_ok { "OK" } else { "FAIL" });
    }
    println!("◆ {passed}/{total} stages passed");
    if passed == total {
        Ok(())
    } else {
        anyhow::bail!("pipeline failed: {passed}/{total} stages passed")
    }
}

fn hook_pre_commit(repo: &Path, options: HookOptions) -> anyhow::Result<()> {
    if std::env::var("LTO_HOOK_MODE").unwrap_or_else(|_| "warn".to_string()) == "off" {
        return Ok(());
    }
    let Ok(ctx) = util::load_run(repo, None) else {
        return Ok(());
    };
    let unresolved = ctx
        .state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if unresolved > 0 && !options.force {
        anyhow::bail!("LTO: BLOCKED -- {unresolved} unresolved blocks");
    }
    let head = util::git_status(repo).head;
    let last_reviewed = ctx
        .state
        .gates
        .get("last_reviewed_head")
        .and_then(Value::as_str);
    if last_reviewed.is_some_and(|value| value != head) {
        eprintln!("LTO: no review for current HEAD");
    }
    if options.force && !options.reason.is_empty() {
        eprintln!("LTO hook forced: {}", options.reason);
    }
    Ok(())
}

fn hook_pre_deploy(repo: &Path) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, None).context("LTO: no active run, deploy blocked")?;
    if ctx.state.current_phase == "closed" {
        anyhow::bail!("LTO: run closed, deploy blocked");
    }
    let unresolved = ctx
        .state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if unresolved > 0 {
        anyhow::bail!("LTO: {unresolved} unresolved blocks, deploy blocked");
    }
    println!("LTO: pre-deploy OK");
    Ok(())
}

fn hook_pre_closeout(repo: &Path) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, None)?;
    let unresolved = ctx
        .state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if unresolved > 0 {
        anyhow::bail!("LTO: check failed, closeout blocked");
    }
    println!("LTO: pre-closeout OK");
    Ok(())
}

fn check_write(repo: &Path) -> bool {
    let path = repo.join(".lto").join(".preflight_test");
    let result = fs::create_dir_all(path.parent().unwrap_or(repo))
        .and_then(|_| fs::write(&path, "ok"))
        .and_then(|_| fs::remove_file(&path));
    result.is_ok()
}

fn bump_version(version: &str, part: &str) -> anyhow::Result<String> {
    let pieces = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("VERSION not semver x.y.z: {version:?}"))?;
    if pieces.len() != 3 {
        anyhow::bail!("VERSION not semver x.y.z: {version:?}");
    }
    let (major, minor, patch) = (pieces[0], pieces[1], pieces[2]);
    match part {
        "major" => Ok(format!("{}.0.0", major + 1)),
        "minor" => Ok(format!("{}.{}.0", major, minor + 1)),
        "patch" => Ok(format!("{}.{}.{}", major, minor, patch + 1)),
        other => anyhow::bail!("invalid --part: {other:?}"),
    }
}

fn ensure_valid_phase(phase: &str) -> anyhow::Result<()> {
    if util::VALID_PHASES.contains(&phase) {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid phase: {phase:?} (valid: {})",
            util::VALID_PHASES.join(", ")
        )
    }
}

fn ensure_valid_status(status: &str) -> anyhow::Result<()> {
    if util::VALID_TASK_STATUSES.contains(&status) {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid status: {status:?} (valid: {})",
            util::VALID_TASK_STATUSES.join(", ")
        )
    }
}

fn ensure_valid_kind(kind: &str) -> anyhow::Result<()> {
    if util::VALID_EVIDENCE_KINDS.contains(&kind) {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid evidence kind: {kind:?} (valid: {})",
            util::VALID_EVIDENCE_KINDS.join(", ")
        )
    }
}

fn find_task_mut<'a>(tasks: &'a mut Value, task_id: &str) -> anyhow::Result<&'a mut Value> {
    util::json_array_mut(tasks)
        .iter_mut()
        .find(|task| task.get("id").and_then(Value::as_str) == Some(task_id))
        .with_context(|| format!("no such task: {task_id}"))
}

fn filter_tasks(tasks: &Value, task_id: Option<&str>, phase: Option<&str>) -> Vec<Value> {
    util::json_array(tasks)
        .iter()
        .filter(|task| {
            if let Some(task_id) = task_id {
                return task.get("id").and_then(Value::as_str) == Some(task_id);
            }
            if let Some(phase) = phase {
                return task.get("phase").and_then(Value::as_str) == Some(phase);
            }
            matches!(
                task.get("status").and_then(Value::as_str),
                Some("done" | "in_progress")
            )
        })
        .cloned()
        .collect()
}

fn build_state_verdict(
    tasks: &[Value],
    test_results: &[Value],
    head: &str,
    options: &JudgeOptions,
) -> String {
    let has_failures = test_results
        .iter()
        .any(|result| result.get("result").and_then(Value::as_str) != Some("pass"));
    let has_active_blockers = tasks.iter().any(|task| {
        task.get("blockers")
            .and_then(Value::as_array)
            .is_some_and(|blockers| !blockers.is_empty())
    });
    let verdict = if has_failures || has_active_blockers {
        "fail"
    } else {
        "pass"
    };
    let mut lines = vec![
        "# LTO Judge Verdict".to_string(),
        String::new(),
        format!("verdict: {verdict}"),
        format!("reviewed_head: {head}"),
        format!("runner: {}", options.runner),
        format!("phase: {}", options.phase.as_deref().unwrap_or("auto")),
        format!("tasks_reviewed: {}", tasks.len()),
        String::new(),
        "## Test Rerun Results".to_string(),
    ];
    for result in test_results {
        lines.push(format!(
            "- command: {}",
            result.get("command").and_then(Value::as_str).unwrap_or("")
        ));
        lines.push(format!(
            "  result: {} (rc={})",
            result.get("result").and_then(Value::as_str).unwrap_or("?"),
            result.get("rc").and_then(Value::as_i64).unwrap_or(-1)
        ));
    }
    lines.push(String::new());
    lines.push("## Must Fix".to_string());
    for task in tasks {
        for blocker in task
            .get("blockers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            lines.push(format!(
                "- task: {}",
                task.get("id").and_then(Value::as_str).unwrap_or("?")
            ));
            lines.push(format!(
                "  reason: {}",
                blocker
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Should Fix".to_string());
    lines.push(String::new());
    lines.push("## Scope Drift".to_string());
    lines.push(String::new());
    lines.push("## Residual Risks".to_string());
    lines.push(String::new());
    lines.push(format!(
        "next_action: {}",
        if verdict == "fail" {
            "fix_and_rerun"
        } else {
            "commit_allowed"
        }
    ));
    lines.join("\n")
}

fn read_required_path(path: Option<&PathBuf>, flag: &str) -> anyhow::Result<String> {
    let path = path.with_context(|| format!("{flag} is required"))?;
    fs::read_to_string(path).with_context(|| format!("failed to read {flag}: {}", path.display()))
}

fn tail_lines(text: &str, count: usize) -> Vec<String> {
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.len() > count {
        lines = lines.split_off(lines.len() - count);
    }
    lines
}

fn append_unique_strings(task: &mut Value, key: &str, values: &[String]) {
    let object = task.as_object_mut().expect("task object");
    let slot = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let array = util::json_array_mut(slot);
    for value in values {
        if !array
            .iter()
            .any(|item| item.as_str() == Some(value.as_str()))
        {
            array.push(json!(value));
        }
    }
}

fn append_string(task: &mut Value, key: &str, value: &str) {
    let object = task.as_object_mut().expect("task object");
    let slot = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    util::json_array_mut(slot).push(json!(value));
}

fn resolve_blockers(task: &mut Value, evidence: &Value) {
    let blockers = task
        .get("blockers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if blockers.is_empty() {
        task["blockers"] = json!([]);
        return;
    }
    let ended_at = evidence
        .get("ended_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    for blocker in blockers {
        let mut resolved = blocker;
        if let Some(object) = resolved.as_object_mut() {
            object.insert("resolved_at".to_string(), json!(ended_at));
            object.insert("resolved_by".to_string(), json!("runner_success"));
            object.insert(
                "superseded_by".to_string(),
                json!({
                    "kind": evidence.get("kind").cloned().unwrap_or(Value::Null),
                    "command": evidence.get("command").cloned().unwrap_or(Value::Null),
                    "ended_at": ended_at,
                }),
            );
        }
        util::append_to_object_array(task, "resolved_blockers", resolved);
    }
    task["blockers"] = json!([]);
}

fn bump_retry(task: &mut Value, command: &str) -> u64 {
    let fingerprint = command_fingerprint(command);
    let object = task.as_object_mut().expect("task object");
    let retry_map = object
        .entry("retry_by_command".to_string())
        .or_insert_with(|| json!({}));
    let retry_object = util::json_object_mut(retry_map);
    let current = retry_object
        .get(&fingerprint)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    retry_object.insert(fingerprint, json!(current));
    object.insert("retry_count".to_string(), json!(current));
    current
}

fn command_fingerprint(command: &str) -> String {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

fn absolutize(repo: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn has_high_risk_task(tasks: &Value) -> bool {
    let keywords = [
        "auth",
        "authorization",
        "authentication",
        "permission",
        "secret",
        "token",
        "migration",
        "database",
        "deploy",
        "persistence",
        "security",
        "payment",
        "callback",
        "webhook",
        "tenant",
    ];
    util::json_array(tasks).iter().any(|task| {
        let haystack = serde_json::to_string(task)
            .unwrap_or_default()
            .to_ascii_lowercase();
        keywords.iter().any(|keyword| haystack.contains(keyword))
    })
}

fn select_task_ids(tasks: &Value, explicit: &[String], phase: Option<&str>) -> Vec<String> {
    util::json_array(tasks)
        .iter()
        .filter(|task| {
            if !explicit.is_empty() {
                return task
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| explicit.iter().any(|wanted| wanted == id));
            }
            if let Some(phase) = phase {
                return task.get("phase").and_then(Value::as_str) == Some(phase)
                    && matches!(
                        task.get("status").and_then(Value::as_str),
                        Some("pending" | "in_progress")
                    );
            }
            matches!(
                task.get("status").and_then(Value::as_str),
                Some("pending" | "in_progress")
            )
        })
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn task_command(tasks: &Value, task_id: &str) -> Option<String> {
    util::json_array(tasks)
        .iter()
        .find(|task| task.get("id").and_then(Value::as_str) == Some(task_id))
        .and_then(|task| {
            task.get("commands_run")
                .and_then(Value::as_array)
                .and_then(|items| items.last())
                .and_then(Value::as_str)
                .or_else(|| task.get("planned_command").and_then(Value::as_str))
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, DeliveryContract, LtoState, WorkspaceSnapshot};
    use std::process::Command;

    #[test]
    fn which_in_path_finds_executable_and_misses_absent() {
        // A ubiquitous executable that is on PATH in any CI/dev shell.
        assert!(which_in_path("sh"), "sh must resolve on PATH");
        // A name that cannot exist as a bare command.
        assert!(!which_in_path("lto-definitely-not-a-real-tool-xyz"));
    }

    struct Harness {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path().join("repo");
            fs::create_dir_all(&repo).unwrap();
            Self { _tmp: tmp, repo }
        }

        fn init_git(&self) {
            git(&self.repo, &["init"]);
            git(&self.repo, &["config", "user.email", "lto@example.test"]);
            git(&self.repo, &["config", "user.name", "LTO Test"]);
            fs::write(self.repo.join("README.md"), "repo\n").unwrap();
            git(&self.repo, &["add", "README.md"]);
            git(&self.repo, &["commit", "-m", "init"]);
        }

        fn write_state(&self, state: LtoState) {
            self.write_state_as("r1", state);
            fs::write(self.repo.join(".lto").join("current"), "r1\n").unwrap();
        }

        fn write_state_as(&self, run_id: &str, mut state: LtoState) {
            state.run_id = run_id.to_string();
            let run_dir = self.repo.join(".lto").join(run_id);
            fs::create_dir_all(&run_dir).unwrap();
            state::save_state(run_dir.join("state.json"), &state).unwrap();
            fs::write(
                run_dir.join("run-state.md"),
                "- current_phase: intake\n- next_command_or_question: none\n- blocked_by: none\n",
            )
            .unwrap();
        }

        fn state(&self) -> LtoState {
            state::load_state(self.repo.join(".lto").join("r1").join("state.json")).unwrap()
        }
    }

    fn base_state() -> LtoState {
        LtoState {
            run_id: "r1".to_string(),
            goal: "ops commands".to_string(),
            why: "exercise command behavior".to_string(),
            done_when: "assertions pass".to_string(),
            host_runtime: "codex".to_string(),
            current_phase: "implementation".to_string(),
            workspace: WorkspaceSnapshot {
                head: "unknown".to_string(),
                ..WorkspaceSnapshot::default()
            },
            gates: json!({}),
            tasks: json!([]),
            blocked_by: json!("none"),
            next_action: json!("continue"),
            ..LtoState::default()
        }
    }

    #[test]
    fn cmd_task_add_main_path_appends_pending_task_with_command() {
        let h = Harness::new();
        h.write_state(base_state());
        cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                title: "write tests".into(),
                phase: Some("implementation".into()),
                command: Some("cargo test".into()),
                instrument_ref: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert_eq!(task["id"], "T1");
        assert_eq!(task["status"], "pending");
        assert_eq!(task["commands_run"], json!([]));
        assert_eq!(task["planned_command"], "cargo test");

        let err = cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                title: "duplicate".into(),
                phase: None,
                command: None,
                instrument_ref: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn planned_command_is_not_double_counted_after_a_real_run() {
        let h = Harness::new();
        h.write_state(base_state());
        cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                title: "run once".into(),
                phase: Some("implementation".into()),
                command: Some("true".into()),
                instrument_ref: None,
            },
        )
        .unwrap();
        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: Some("T1".into()),
                kind: "test".into(),
                command: Some("true".into()),
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert_eq!(task["commands_run"], json!(["true"]));
        assert_eq!(task["planned_command"], "true");
    }

    #[test]
    fn task_add_accepts_only_current_contract_instrument_refs() {
        let h = Harness::new();
        let mut state = base_state();
        state.delivery_contract = DeliveryContract::new(
            vec!["tests".into()],
            vec![],
            vec!["tests::printf ok".into()],
            vec![],
        );
        h.write_state(state);

        cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                title: "run tests".into(),
                phase: Some("implementation".into()),
                command: Some("printf ok".into()),
                instrument_ref: Some("tests".into()),
            },
        )
        .unwrap();
        assert_eq!(h.state().tasks[0]["instrument_ref"], "tests");

        let err = cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T2".into(),
                title: "unknown signal".into(),
                phase: Some("implementation".into()),
                command: None,
                instrument_ref: Some("unknown".into()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn runner_auto_links_matching_contract_command_in_evidence() {
        let h = Harness::new();
        let mut state = base_state();
        state.delivery_contract = DeliveryContract::new(
            vec!["tests".into()],
            vec![],
            vec!["tests::printf ok".into()],
            vec![],
        );
        state.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "commands_run": [],
            "evidence": [],
            "blockers": []
        }]);
        h.write_state(state);

        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: Some("T1".into()),
                kind: "test".into(),
                command: Some("printf \"ok\"".into()),
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            },
        )
        .unwrap();

        assert_eq!(h.state().tasks[0]["evidence"][0]["instrument_ref"], "tests");
    }

    #[test]
    fn task_command_falls_back_to_planned_command_before_any_run() {
        let h = Harness::new();
        h.write_state(base_state());
        cmd_task_add(
            &h.repo,
            TaskAddOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                title: "not yet run".into(),
                phase: Some("implementation".into()),
                command: Some("echo hi".into()),
                instrument_ref: None,
            },
        )
        .unwrap();
        let state = h.state();
        assert_eq!(
            task_command(&state.tasks, "T1"),
            Some("echo hi".to_string())
        );
    }

    #[test]
    fn cmd_phase_main_path_records_transition_and_syncs_run_state() {
        let h = Harness::new();
        h.write_state(base_state());
        cmd_phase(
            &h.repo,
            PhaseOptions {
                run_id: Some("r1".into()),
                set_phase: Some("deploy".into()),
            },
        )
        .unwrap();
        let state = h.state();
        assert_eq!(state.current_phase, "deploy");
        assert_eq!(util::json_array(&state.phase_transitions).len(), 1);
        let md = fs::read_to_string(h.repo.join(".lto").join("r1").join("run-state.md")).unwrap();
        assert!(md.contains("- current_phase: deploy"));
    }

    #[test]
    fn cmd_release_main_path_validates_version_and_changelog_without_writing() {
        let h = Harness::new();
        fs::write(h.repo.join("VERSION"), "1.2.3\n").unwrap();
        fs::write(h.repo.join("CHANGELOG.md"), "# Changelog\n").unwrap();
        cmd_release(
            &h.repo,
            ReleaseOptions {
                part: "minor".into(),
                date: "2026-06-15".into(),
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(h.repo.join("VERSION")).unwrap(),
            "1.2.3\n"
        );
        let err = cmd_release(
            &h.repo,
            ReleaseOptions {
                part: "bad".into(),
                date: "2026-06-15".into(),
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid --part"));
    }

    #[test]
    fn collect_check_reports_all_strict_closed_errors_without_early_return() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.workspace.head = util::git_status(&h.repo).head;
        state.tasks = json!([{"id": "T1", "status": "pending"}]);
        state.gates = json!({"unresolved_blocks": [{"id": "B1"}]});
        state.risk_points = json!([{"id": "R1", "status": "open"}]);
        h.write_state(state);

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("closed".into()),
                json: true,
            },
        );
        let joined = outcome.errors.join("\n");
        assert!(joined.contains("no_open_tasks"));
        assert!(joined.contains("no_unresolved_blocks"));
        assert!(joined.contains("risk_points_verified"));
        assert!(joined.contains("handoff_exists"));
    }

    #[test]
    fn collect_check_reports_missing_state_and_run_state_once() {
        let h = Harness::new();
        fs::create_dir_all(h.repo.join(".lto").join("broken")).unwrap();

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("broken".into()),
                strict: false,
                to_phase: None,
                json: true,
            },
        );

        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].contains("missing both"));
    }

    #[test]
    fn strict_phase_check_reports_c2_gaps_only_through_phase_evidence() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.done_when.clear();
        state.workspace.head = util::git_status(&h.repo).head;
        h.write_state(state);

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("implementation".into()),
                json: true,
            },
        );

        assert!(
            outcome
                .errors
                .iter()
                .any(|error| error.contains("phase evidence missing: run_readiness"))
        );
        assert!(
            !outcome
                .errors
                .iter()
                .any(|error| error.starts_with("run readiness missing:"))
        );
        assert_eq!(
            outcome
                .errors
                .iter()
                .filter(|error| error.contains("--done-when"))
                .count(),
            1
        );
    }

    #[test]
    fn collect_check_accepts_clean_closed_run_with_handoff() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.current_phase = "closed".to_string();
        state.workspace.head = util::git_status(&h.repo).head;
        state.tasks = json!([{
            "id": "T1",
            "status": "done",
            "evidence": [{"kind": "manual", "summary": "host verified", "rc": 0}]
        }]);
        state.gates = json!({});
        state.risk_points = json!([]);
        h.write_state(state);
        fs::write(h.repo.join(".lto").join("r1").join("handoff.md"), "done\n").unwrap();

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("closed".into()),
                json: true,
            },
        );
        assert_eq!(outcome.errors, Vec::<String>::new());
    }

    #[test]
    fn collect_check_rejects_closed_done_task_without_evidence() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.current_phase = "closed".to_string();
        state.workspace.head = util::git_status(&h.repo).head;
        state.tasks = json!([{"id": "T1", "status": "done"}]);
        state.gates = json!({});
        state.risk_points = json!([]);
        h.write_state(state);
        fs::write(h.repo.join(".lto").join("r1").join("handoff.md"), "done\n").unwrap();

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("closed".into()),
                json: true,
            },
        );
        let joined = outcome.errors.join("\n");
        assert!(joined.contains("done_tasks_have_evidence"));
        assert!(joined.contains("T1"));
    }

    #[test]
    fn collect_check_gates_partial_delivery_contract_when_present() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.workspace.head = util::git_status(&h.repo).head;
        state.delivery_contract = DeliveryContract::new(
            vec!["ship installable Rust wrapper".into()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        h.write_state(state);

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("implementation".into()),
                json: true,
            },
        );
        let joined = outcome.errors.join("\n");
        assert!(joined.contains("delivery_contract_complete"));
        assert!(joined.contains("--instrument"));
        assert!(!joined.contains("--target"));
        assert!(!joined.contains("--constraint"));
        let report = outcome.phase_report.unwrap();
        let checks = report["checks"].as_array().unwrap();
        assert!(checks.iter().any(|check| {
            check["id"] == "delivery_contract_complete" && check["status"] == "missing"
        }));
    }

    #[test]
    fn collect_check_accepts_paired_contract_and_reports_optional_advisories() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.workspace.head = util::git_status(&h.repo).head;
        state.delivery_contract = DeliveryContract::new(
            vec!["ship installable Rust wrapper".into()],
            Vec::new(),
            vec!["cargo test --locked --all-targets".into()],
            Vec::new(),
        );
        h.write_state(state);

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("implementation".into()),
                json: true,
            },
        );

        assert_eq!(outcome.errors, Vec::<String>::new());
        let report = outcome.phase_report.unwrap();
        let checks = report["checks"].as_array().unwrap();
        assert!(checks.iter().any(|check| {
            check["id"] == "delivery_contract_complete" && check["status"] == "ok"
        }));
        let advisory = checks
            .iter()
            .find(|check| check["id"] == "delivery_contract_advisory")
            .expect("delivery contract advisory");
        assert_eq!(advisory["status"], "warn");
        assert_eq!(advisory["required"], false);
        assert!(
            advisory["detail"]
                .as_str()
                .unwrap()
                .contains("--constraint")
        );
        assert!(
            advisory["detail"]
                .as_str()
                .unwrap()
                .contains("--entropy-check")
        );
    }

    #[test]
    fn collect_check_requires_base_readiness_for_implementation_and_closed() {
        for target in ["implementation", "closed"] {
            let h = Harness::new();
            h.init_git();
            let mut state = base_state();
            state.done_when.clear();
            state.workspace.head = util::git_status(&h.repo).head;
            h.write_state(state);

            let outcome = collect_check(
                &h.repo,
                &CheckOptions {
                    run_id: Some("r1".into()),
                    strict: true,
                    to_phase: Some(target.into()),
                    json: true,
                },
            );

            let report = outcome.phase_report.unwrap();
            let check = report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|check| check["id"] == "run_readiness")
                .expect("run readiness check");
            assert_eq!(check["status"], "missing", "target={target}");
            assert_eq!(check["required"], true, "target={target}");
            assert!(
                check["detail"].as_str().unwrap().contains("--done-when"),
                "target={target}"
            );
        }
    }

    #[test]
    fn collect_check_accepts_complete_delivery_contract() {
        let h = Harness::new();
        h.init_git();
        let mut state = base_state();
        state.workspace.head = util::git_status(&h.repo).head;
        state.delivery_contract = DeliveryContract::new(
            vec!["ship installable Rust wrapper".into()],
            vec!["macOS/Linux first".into()],
            vec!["cargo test --locked --all-targets".into()],
            vec!["verify Rust default and legacy fixture separately".into()],
        );
        h.write_state(state);

        let outcome = collect_check(
            &h.repo,
            &CheckOptions {
                run_id: Some("r1".into()),
                strict: true,
                to_phase: Some("implementation".into()),
                json: true,
            },
        );
        assert_eq!(outcome.errors, Vec::<String>::new());
        let report = outcome.phase_report.unwrap();
        let checks = report["checks"].as_array().unwrap();
        assert!(checks.iter().any(|check| {
            check["id"] == "delivery_contract_complete" && check["status"] == "ok"
        }));
    }

    #[test]
    fn runner_failure_increments_retry_by_command_fingerprint() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "commands_run": ["exit 7"],
            "evidence": [],
            "blockers": [],
            "retry_count": 0,
            "retry_by_command": {}
        }]);
        h.write_state(state);

        for _ in 0..3 {
            let err = cmd_runner(
                &h.repo,
                RunnerOptions {
                    run_id: Some("r1".into()),
                    task_id: Some("T1".into()),
                    kind: "test".into(),
                    command: Some("exit 7".into()),
                    cwd: None,
                    timeout: 5,
                    touch: Vec::new(),
                    note: None,
                    instrument_ref: None,
                    status_on_fail: "blocked".into(),
                    runner: "codex".into(),
                    allow_headless_write: false,
                    prompt: None,
                    prompt_file: None,
                    job_file: None,
                    job_id: None,
                    tmux_target: None,
                    tmux_mode: None,
                    tmux_sentinel: None,
                    tmux_session: None,
                    tmux_new_window: false,
                    tmux_new_session: false,
                    tmux_window_name: None,
                    tmux_ready_patterns: Vec::new(),
                    tmux_skip_prompts: Vec::new(),
                    tmux_ready_timeout_sec: None,
                    tmux_bin: None,
                },
            )
            .unwrap_err();
            assert!(err.to_string().contains("runner command failed"));
        }
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert_eq!(task["retry_count"], json!(3));
        let retries = task["retry_by_command"].as_object().unwrap();
        assert_eq!(retries.len(), 1);
        assert_eq!(retries.values().next().unwrap(), &json!(3));
    }

    #[test]
    fn runner_success_moves_active_blockers_to_resolved_blockers() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "blocked",
            "commands_run": ["true"],
            "evidence": [],
            "blockers": [{"reason": "previous failure"}],
            "retry_count": 1,
            "retry_by_command": {}
        }]);
        h.write_state(state);

        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: Some("T1".into()),
                kind: "test".into(),
                command: Some("true".into()),
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert_eq!(task["status"], json!("done"));
        assert!(task["blockers"].as_array().unwrap().is_empty());
        assert_eq!(task["resolved_blockers"].as_array().unwrap().len(), 1);
        assert_eq!(
            task["resolved_blockers"][0]["resolved_by"],
            "runner_success"
        );
    }

    #[test]
    fn cmd_runner_tmux_command_uses_scheduler_path_and_emits_events() {
        let h = Harness::new();
        h.write_state(base_state());
        write_ok_healthcheck(&h.repo);
        let fake_tmux = write_fake_tmux(&h.repo);

        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: None,
                kind: "manual".into(),
                command: Some("echo tmux-ok".into()),
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "tmux".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: Some("tmux-smoke".into()),
                tmux_target: Some("sess:1.0".into()),
                tmux_mode: Some("signal".into()),
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: Some(15),
                tmux_bin: Some(fake_tmux.display().to_string()),
            },
        )
        .unwrap();

        let events =
            fs::read_to_string(h.repo.join(".lto").join("r1").join("events.jsonl")).unwrap();
        let event_rows = events
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(event_rows.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("runner.started")
                && event
                    .get("actor")
                    .and_then(|actor| actor.get("id"))
                    .and_then(Value::as_str)
                    == Some("tmux")
        }));
        assert!(event_rows.iter().any(|event| {
            event.get("type").and_then(Value::as_str) == Some("runner.finished")
                && event
                    .get("actor")
                    .and_then(|actor| actor.get("id"))
                    .and_then(Value::as_str)
                    == Some("tmux")
        }));
        let tmux_log = fs::read_to_string(h.repo.join("tmux-log.jsonl")).unwrap();
        assert!(tmux_log.contains("[\"load-buffer\""));
        assert!(tmux_log.contains("[\"paste-buffer\""));
        assert!(tmux_log.contains("[\"wait-for\""));
    }

    fn write_ok_healthcheck(repo: &Path) {
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
        make_executable(&script);
    }

    fn write_fake_codex_runner(repo: &Path) {
        let runners = repo.join("scripts").join("delegate").join("runners");
        fs::create_dir_all(&runners).unwrap();
        let script = runners.join("codex.sh");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
set -euo pipefail
prompt_file="$1"
reply_file="$2"
printf 'fake codex saw %s\n' "$(head -n 1 "$prompt_file")" > "$reply_file"
"#,
        )
        .unwrap();
        make_executable(&script);
    }

    fn write_inline_codex_job_file(repo: &Path, job_id: &str) -> PathBuf {
        let path = repo.join(format!("{job_id}.json"));
        fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "job_id": job_id,
                "runner": "codex",
                "prompt_ref": format!("prompt for {job_id}"),
                "prompt_is_inline": true,
                "task_type": "implementation"
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn write_workspace_codex_job_file(repo: &Path, job_id: &str) -> PathBuf {
        let path = repo.join(format!("{job_id}.json"));
        fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "job_id": job_id,
                "runner": "codex",
                "prompt_ref": format!("prompt for {job_id}"),
                "prompt_is_inline": true,
                "permission_policy": {
                    "sandbox": "workspace-write",
                    "reason": "test implementation"
                },
                "task_type": "implementation"
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    #[test]
    fn autopilot_tmux_worker_runs_pending_tasks_and_uses_contracts() {
        let h = Harness::new();
        write_ok_healthcheck(&h.repo);
        let fake_tmux = write_fake_tmux_worker(&h.repo);
        let mut state = base_state();
        state.tasks = json!([
            {
                "id": "T1",
                "status": "pending",
                "commands_run": ["printf one"],
                "evidence": [],
                "blockers": [],
                "retry_count": 0,
                "retry_by_command": {}
            },
            {
                "id": "T2",
                "status": "pending",
                "commands_run": ["printf two"],
                "evidence": [],
                "blockers": [],
                "retry_count": 0,
                "retry_by_command": {}
            }
        ]);
        h.write_state(state);

        cmd_autopilot(
            &h.repo,
            AutopilotOptions {
                run_id: Some("r1".into()),
                auto_exec: true,
                autonomous: false,
                timeout: 5,
                worker_runner: "tmux".into(),
                tmux_target: Some("sess:1.0".into()),
                tmux_bin: Some(fake_tmux.display().to_string()),
                tmux_ready_timeout_sec: Some(15),
            },
        )
        .unwrap();

        let state = h.state();
        let tasks = util::json_array(&state.tasks);
        assert_eq!(tasks[0]["status"], "done");
        assert_eq!(tasks[1]["status"], "done");
        for task in tasks {
            let evidence = task["evidence"].as_array().unwrap().last().unwrap();
            assert_eq!(evidence["kind"], "worker");
            assert_eq!(evidence["carrier"], "tmux");
            assert_eq!(evidence["rc"], 0);
            assert!(
                task["last_update"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            let contract = PathBuf::from(evidence["contract"].as_str().unwrap());
            assert!(contract.exists());
        }
        let log = fs::read_to_string(h.repo.join("tmux-log.jsonl")).unwrap();
        assert!(log.contains("printf one"));
        assert!(log.contains("printf two"));
        let events =
            fs::read_to_string(h.repo.join(".lto").join("r1").join("events.jsonl")).unwrap();
        assert!(events.contains("autopilot.tmux_worker"));
        assert!(events.contains("runner.started"));
        assert!(events.contains("runner.finished"));
        assert!(events.contains("\"phase\":\"implementation\""));
        let agent_runs = util::iter_agent_runs(&state.agent_runs);
        assert_eq!(agent_runs.len(), 2);
        assert!(agent_runs.iter().all(|result| {
            result.runner == "tmux" && result.task_type.as_deref() == Some("autopilot_worker")
        }));
    }

    #[test]
    fn cmd_runner_prompt_scheduler_path_records_agent_run() {
        let h = Harness::new();
        h.write_state(base_state());
        write_ok_healthcheck(&h.repo);
        write_fake_codex_runner(&h.repo);

        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: None,
                kind: "manual".into(),
                command: None,
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: Some("hello from prompt".into()),
                prompt_file: None,
                job_id: Some("prompt-job".into()),
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
                job_file: None,
            },
        )
        .unwrap();

        let state = h.state();
        let agent_runs = util::iter_agent_runs(&state.agent_runs);
        assert_eq!(agent_runs.len(), 1);
        assert_eq!(agent_runs[0].job_id, "prompt-job");
        assert_eq!(agent_runs[0].runner, "codex");
        assert_eq!(agent_runs[0].status, JobStatus::Ok);
        let events =
            fs::read_to_string(h.repo.join(".lto").join("r1").join("events.jsonl")).unwrap();
        assert!(events.contains("runner.finished"));
        assert!(events.contains("runner.prompt"));
    }

    #[test]
    fn cmd_runner_job_file_requires_headless_write_override() {
        let h = Harness::new();
        h.init_git();
        h.write_state(base_state());
        write_ok_healthcheck(&h.repo);
        write_fake_codex_runner(&h.repo);
        let job_file = write_workspace_codex_job_file(&h.repo, "write-job-file");

        let options = RunnerOptions {
            run_id: Some("r1".into()),
            task_id: None,
            kind: "manual".into(),
            command: None,
            cwd: None,
            timeout: 5,
            touch: Vec::new(),
            note: None,
            instrument_ref: None,
            status_on_fail: "blocked".into(),
            runner: "codex".into(),
            allow_headless_write: false,
            prompt: None,
            prompt_file: None,
            job_file: Some(job_file),
            job_id: None,
            tmux_target: None,
            tmux_mode: None,
            tmux_sentinel: None,
            tmux_session: None,
            tmux_new_window: false,
            tmux_new_session: false,
            tmux_window_name: None,
            tmux_ready_patterns: Vec::new(),
            tmux_skip_prompts: Vec::new(),
            tmux_ready_timeout_sec: None,
            tmux_bin: None,
        };
        let err = cmd_runner(&h.repo, options.clone()).unwrap_err();
        assert!(err.to_string().contains("lto dispatch-goal --runner codex"));

        cmd_runner(
            &h.repo,
            RunnerOptions {
                allow_headless_write: true,
                ..options
            },
        )
        .unwrap();
    }

    #[test]
    fn job_file_scheduler_paths_record_agent_runs_with_explicit_run_id() {
        let h = Harness::new();
        h.write_state(base_state());
        write_ok_healthcheck(&h.repo);
        write_fake_codex_runner(&h.repo);

        let runner_job = write_inline_codex_job_file(&h.repo, "runner-job-file");
        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: None,
                kind: "manual".into(),
                command: None,
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
                job_file: Some(runner_job),
            },
        )
        .unwrap();

        let parallel_job = write_inline_codex_job_file(&h.repo, "parallel-job-file");
        cmd_parallel(
            &h.repo,
            ParallelOptions {
                run_id: Some("r1".into()),
                task_ids: Vec::new(),
                phase: None,
                kind: "manual".into(),
                command: None,
                timeout: 5,
                concurrency: 1,
                job_file: Some(parallel_job),
            },
        )
        .unwrap();

        let pipeline_job = write_inline_codex_job_file(&h.repo, "pipeline-job-file");
        cmd_pipeline(
            &h.repo,
            PipelineOptions {
                run_id: Some("r1".into()),
                task_ids: Vec::new(),
                phase: None,
                stages: Vec::new(),
                kind: "manual".into(),
                timeout: 5,
                concurrency: 1,
                continue_on_error: false,
                job_file: Some(pipeline_job),
            },
        )
        .unwrap();

        let state = h.state();
        let mut job_ids = util::iter_agent_runs(&state.agent_runs)
            .into_iter()
            .map(|result| result.job_id)
            .collect::<Vec<_>>();
        job_ids.sort();
        assert_eq!(
            job_ids,
            vec!["parallel-job-file", "pipeline-job-file", "runner-job-file"]
        );

        let events = crate::events::read(&h.repo, "r1").unwrap();
        let mut started_contexts = events
            .iter()
            .filter(|event| event["type"] == "runner.started")
            .filter_map(|event| event["fields"]["context"].as_str())
            .collect::<Vec<_>>();
        started_contexts.sort();
        assert_eq!(
            started_contexts,
            vec!["run.parallel", "run.pipeline", "runner.job_file"]
        );
    }

    #[test]
    fn job_file_submission_failure_emits_lifecycle_event() {
        let h = Harness::new();
        h.write_state(base_state());
        let job_file = write_inline_codex_job_file(&h.repo, "invalid-runner-job-file");
        let mut jobs = load_jobs(&job_file).unwrap();
        jobs[0].runner = "unknown".into();

        let err = run_job_file(&h.repo, jobs, Some("r1".into()), "runner.job_file").unwrap_err();
        assert!(err.to_string().contains("unknown runner"));

        let events = crate::events::read(&h.repo, "r1").unwrap();
        let submission_failed = events
            .iter()
            .find(|event| {
                event["type"] == "runner.finished" && event["fields"]["submission_failed"] == true
            })
            .unwrap();
        assert_eq!(submission_failed["fields"]["context"], "runner.job_file");
    }

    #[test]
    fn autopilot_tmux_worker_blocks_on_nonzero_contract_rc() {
        let h = Harness::new();
        write_ok_healthcheck(&h.repo);
        let fake_tmux = write_fake_tmux_worker(&h.repo);
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "commands_run": ["exit 7"],
            "evidence": [],
            "blockers": [],
            "retry_count": 0,
            "retry_by_command": {}
        }]);
        h.write_state(state);

        cmd_autopilot(
            &h.repo,
            AutopilotOptions {
                run_id: Some("r1".into()),
                auto_exec: true,
                autonomous: false,
                timeout: 5,
                worker_runner: "tmux".into(),
                tmux_target: Some("sess:1.0".into()),
                tmux_bin: Some(fake_tmux.display().to_string()),
                tmux_ready_timeout_sec: Some(15),
            },
        )
        .unwrap();

        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert_eq!(task["status"], "blocked");
        assert_eq!(task["retry_count"], 1);
        let evidence = task["evidence"].as_array().unwrap().last().unwrap();
        assert_eq!(evidence["kind"], "worker");
        assert_eq!(evidence["carrier"], "tmux");
        assert_eq!(evidence["rc"], 7);
        let agent_runs = util::iter_agent_runs(&state.agent_runs);
        assert_eq!(agent_runs.len(), 1);
        assert_eq!(agent_runs[0].runner, "tmux");
        assert_eq!(agent_runs[0].status, JobStatus::Ok);
        assert!(
            task["last_update"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
    }

    #[test]
    fn tmux_worker_prompt_preserves_quoted_command_contract() {
        let h = Harness::new();
        let contract = h
            .repo
            .join(".lto")
            .join("r1")
            .join("live")
            .join("quoted.worker.json");
        let prompt =
            tmux_worker_prompt(&h.repo, "printf '%s\\n' \"it'works\"", &contract, "T'1").unwrap();

        let status = Command::new("bash")
            .arg("-lc")
            .arg(&prompt)
            .status()
            .unwrap();
        assert!(status.success());

        let value = read_tmux_worker_contract(&contract).unwrap().unwrap();
        assert_eq!(value["task_id"], "T'1");
        assert_eq!(value["rc"], 0);
        assert_eq!(value["carrier"], "tmux");
    }

    #[test]
    fn autopilot_skips_auto_exec_when_retry_limit_reached() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "commands_run": ["true"],
            "evidence": [],
            "blockers": [],
            "retry_count": 3,
            "retry_by_command": {}
        }]);
        h.write_state(state);

        cmd_autopilot(
            &h.repo,
            AutopilotOptions {
                run_id: Some("r1".into()),
                auto_exec: true,
                autonomous: false,
                timeout: 5,
                worker_runner: "sandbox".into(),
                tmux_target: None,
                tmux_bin: None,
                tmux_ready_timeout_sec: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert!(task["evidence"].as_array().unwrap().is_empty());
        assert_eq!(task["status"], "pending");
    }

    #[test]
    fn autopilot_stall_digest_blocks_auto_exec_on_second_same_state() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "commands_run": ["touch SHOULD_NOT_EXIST"],
            "evidence": [],
            "blockers": [],
            "retry_count": 0,
            "retry_by_command": {}
        }]);
        h.write_state(state);
        let mut ctx = util::load_run(&h.repo, Some("r1")).unwrap();
        ctx.state.gates["autopilot_last_digest"] = progress_digest(&ctx);
        util::save_run(&mut ctx).unwrap();

        cmd_autopilot(
            &h.repo,
            AutopilotOptions {
                run_id: Some("r1".into()),
                auto_exec: true,
                autonomous: false,
                timeout: 5,
                worker_runner: "sandbox".into(),
                tmux_target: None,
                tmux_bin: None,
                tmux_ready_timeout_sec: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = &util::json_array(&state.tasks)[0];
        assert!(task["evidence"].as_array().unwrap().is_empty());
        assert!(!h.repo.join("SHOULD_NOT_EXIST").exists());
    }

    #[test]
    fn progress_digest_treats_same_failed_task_as_stalled() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{
            "id": "T1",
            "status": "blocked",
            "commands_run": ["false"],
            "evidence": [{"rc": 1, "stderr_tail": ["same failure"]}],
            "blockers": [{"reason": "failed"}],
            "retry_count": 1,
            "retry_by_command": {}
        }]);
        h.write_state(state);
        let ctx = util::load_run(&h.repo, Some("r1")).unwrap();
        let digest = progress_digest(&ctx);
        let (progressed, reason) = has_progressed(&digest, &digest);
        assert!(!progressed, "{reason}");
    }

    fn write_autonomous_gate_run(
        h: &Harness,
        run_id: &str,
        first_status: &str,
        second_status: &str,
        subjective: bool,
    ) {
        let mut state = base_state();
        state.agent_runs = json!({
            format!("{run_id}-a"): [{
                "job_id": format!("{run_id}-a"),
                "runner": "codex",
                "model": "gpt-5",
                "status": first_status,
                "task_type": "implementation"
            }],
            format!("{run_id}-b"): [{
                "job_id": format!("{run_id}-b"),
                "runner": "codex",
                "model": "gpt-5",
                "status": second_status,
                "task_type": "review"
            }]
        });
        h.write_state_as(run_id, state);
        for (job_id, task_id, status) in [
            (format!("{run_id}-a"), "impl-task", first_status),
            (format!("{run_id}-b"), "test-task", second_status),
        ] {
            crate::events::emit(
                &h.repo,
                run_id,
                crate::events::EventRecord {
                    event_type: "runner.finished".to_string(),
                    actor_kind: "runner".to_string(),
                    actor_id: Some("codex".to_string()),
                    phase: Some("implementation".to_string()),
                    task_id: Some(task_id.to_string()),
                    object_id: Some(job_id),
                    object_type: Some("runner_job".to_string()),
                    fields: json!({
                        "runner": "codex",
                        "model": "gpt-5",
                        "status": status,
                        "elapsed_sec": 1.0,
                    }),
                    ..crate::events::EventRecord::default()
                },
            )
            .unwrap();
        }
        if subjective {
            crate::events::emit(
                &h.repo,
                run_id,
                crate::events::EventRecord {
                    event_type: "decision.voted".to_string(),
                    actor_kind: "auditor".to_string(),
                    actor_id: Some("codex".to_string()),
                    summary: "subjective vote".to_string(),
                    ..crate::events::EventRecord::default()
                },
            )
            .unwrap();
        }
    }

    fn write_gate_count_only_run(h: &Harness, run_id: &str) {
        let mut state = base_state();
        state.agent_runs = json!({
            format!("{run_id}-a"): [{
                "job_id": format!("{run_id}-a"),
                "runner": "codex",
                "status": "ok"
            }],
            format!("{run_id}-b"): [{
                "job_id": format!("{run_id}-b"),
                "runner": "codex",
                "status": "ok"
            }]
        });
        h.write_state_as(run_id, state);
    }

    #[test]
    fn autonomous_gate_blocks_when_evidence_data_is_missing() {
        let h = Harness::new();
        for index in 0..5 {
            write_gate_count_only_run(&h, &format!("r{index}"));
        }

        let report = operational_reliability(&h.repo);
        let ok = report.passes();
        let reason = report.reason;

        assert!(!ok);
        assert!(reason.contains("cross-run evidence has no"));
    }

    #[test]
    fn autonomous_gate_blocks_high_failure_rate_even_when_counts_pass() {
        let h = Harness::new();
        for index in 0..5 {
            let status = if index < 3 { "failed" } else { "ok" };
            write_autonomous_gate_run(&h, &format!("r{index}"), status, "ok", false);
        }

        let report = operational_reliability(&h.repo);
        let ok = report.passes();
        let reason = report.reason;

        assert!(!ok);
        assert!(reason.contains("failure_rate=60.0%"), "{reason}");
    }

    #[test]
    fn autonomous_gate_does_not_block_one_old_timeout_or_rate_limit() {
        let h = Harness::new();
        for index in 0..5 {
            let first = if index == 0 { "rate_limited" } else { "ok" };
            let second = if index == 1 { "timeout" } else { "ok" };
            write_autonomous_gate_run(&h, &format!("r{index}"), first, second, false);
        }

        let report = operational_reliability(&h.repo);
        let ok = report.passes();
        let reason = report.reason;

        assert!(ok, "{reason}");
    }

    #[test]
    fn autonomous_gate_blocks_when_only_one_run_has_evidence() {
        let h = Harness::new();
        write_autonomous_gate_run(&h, "r0", "ok", "ok", false);
        for index in 1..5 {
            write_gate_count_only_run(&h, &format!("r{index}"));
        }

        let report = operational_reliability(&h.repo);
        let ok = report.passes();
        let reason = report.reason;

        assert!(!ok);
        assert!(
            reason.contains("cross-run evidence >=5 runs and >=10 distinct-runs sum"),
            "{reason}"
        );
        assert!(reason.contains("current 1/2"), "{reason}");
    }

    #[test]
    fn autonomous_gate_passes_with_counts_and_clean_objective_evidence() {
        let h = Harness::new();
        for index in 0..5 {
            write_autonomous_gate_run(&h, &format!("r{index}"), "ok", "ok", false);
        }
        let before = fs::read_to_string(h.repo.join(".lto").join("r0").join("state.json")).unwrap();

        let report = operational_reliability(&h.repo);
        let ok = report.passes();
        let reason = report.reason;

        assert!(ok, "{reason}");
        assert!(
            reason.contains("evidence runs=5 distinct-runs sum=10"),
            "{reason}"
        );
        let after = fs::read_to_string(h.repo.join(".lto").join("r0").join("state.json")).unwrap();
        assert_eq!(before, after, "autonomous_gate must stay read-only");
    }

    #[test]
    fn autonomous_gate_still_blocks_pure_subjective_evidence() {
        let h = Harness::new();
        for index in 0..5 {
            write_autonomous_gate_run(&h, &format!("r{index}"), "ok", "ok", true);
        }

        let report = operational_reliability(&h.repo);

        assert!(!report.passes());
        assert!(
            report.reason.contains("only subjective/non-measurement"),
            "{}",
            report.reason
        );
    }

    #[test]
    fn autonomous_missing_current_signal_does_not_execute_pending_task() {
        let h = Harness::new();
        for index in 0..5 {
            write_autonomous_gate_run(&h, &format!("r{index}"), "ok", "ok", false);
        }
        let mut current = state::load_state(h.repo.join(".lto/r1/state.json")).unwrap();
        current.tasks = json!([{
            "id": "T1",
            "status": "pending",
            "planned_command": "touch autonomous-should-not-run",
            "commands_run": [],
            "evidence": []
        }]);
        h.write_state(current);

        cmd_autopilot(
            &h.repo,
            AutopilotOptions {
                run_id: Some("r1".into()),
                auto_exec: false,
                autonomous: true,
                timeout: 5,
                worker_runner: "sandbox".into(),
                tmux_target: None,
                tmux_bin: None,
                tmux_ready_timeout_sec: None,
            },
        )
        .unwrap();

        let state = h.state();
        assert_eq!(state.tasks[0]["status"], "pending");
        assert!(state.tasks[0]["evidence"].as_array().unwrap().is_empty());
        assert!(!h.repo.join("autonomous-should-not-run").exists());
    }

    #[test]
    fn cmd_memory_export_main_path_reads_local_run_without_sink() {
        let h = Harness::new();
        let mut state = base_state();
        state.delivery_contract = DeliveryContract::new(
            vec!["secret target token=SECRET".into()],
            vec!["budget".into()],
            vec!["private command /Users/example/private/eval.sh".into()],
            vec!["entropy".into()],
        );
        h.write_state(state);
        cmd_memory(
            &h.repo,
            MemoryAction::Export {
                run_id: Some("r1".into()),
            },
        )
        .unwrap();
        let projection =
            memory_projection(&h.repo, &util::load_run(&h.repo, Some("r1")).unwrap()).unwrap();
        let record = &projection["records"][0];
        assert_eq!(record["run_id"], "r1");
        assert_eq!(record["source"], "local .lto");
        assert!(record.get("goal").is_none());
        assert!(
            record["request_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert_eq!(record["delivery_contract"]["complete"], true);
        let text = serde_json::to_string(&projection).unwrap();
        assert!(!text.contains("token=SECRET"));
        assert!(!text.contains("/Users/example/private"));
    }

    #[test]
    fn build_state_verdict_fails_when_any_task_has_blockers() {
        let tasks = json!([
            {"id": "T1"},
            {"id": "T2", "blockers": [{"reason": "still blocked"}]}
        ]);
        let options = JudgeOptions {
            run_id: None,
            task_id: None,
            phase: None,
            runner: "codex".into(),
            rerun_tests: false,
            case_dir: None,
            brief: None,
            baseline_reply: None,
            candidate_reply: None,
            candidate_runner: None,
            judge_runner: None,
            execute: false,
        };

        let verdict = build_state_verdict(tasks.as_array().unwrap(), &[], "head", &options);

        assert!(verdict.contains("verdict: fail"), "{verdict}");
        assert!(verdict.contains("reason: still blocked"), "{verdict}");
    }

    #[test]
    fn cmd_judge_main_path_writes_verdict_and_review_gate() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([{"id": "T1", "status": "done", "phase": "implementation"}]);
        h.write_state(state);
        cmd_judge(
            &h.repo,
            JudgeOptions {
                run_id: Some("r1".into()),
                task_id: None,
                phase: Some("implementation".into()),
                runner: "codex".into(),
                rerun_tests: false,
                case_dir: None,
                brief: None,
                baseline_reply: None,
                candidate_reply: None,
                candidate_runner: None,
                judge_runner: None,
                execute: false,
            },
        )
        .unwrap();
        let judge_dir = h.repo.join(".lto").join("r1").join("judge");
        assert!(fs::read_dir(judge_dir).unwrap().next().is_some());
        assert!(h.state().gates.get("last_reviewed_head").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn cmd_runner_main_path_records_pass_and_failure_evidence() {
        let h = Harness::new();
        let mut state = base_state();
        state.tasks = json!([
            {"id": "T1", "status": "pending", "commands_run": [], "evidence": [], "blockers": []},
            {"id": "T2", "status": "pending", "commands_run": [], "evidence": [], "blockers": []}
        ]);
        h.write_state(state);
        cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: Some("T1".into()),
                kind: "test".into(),
                command: Some("printf ok".into()),
                cwd: None,
                timeout: 5,
                touch: vec!["src/lib.rs".into()],
                note: Some("unit smoke".into()),
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            },
        )
        .unwrap();
        let state = h.state();
        let task = util::json_array(&state.tasks)
            .iter()
            .find(|task| task["id"] == "T1")
            .unwrap();
        assert_eq!(task["status"], "done");
        assert_eq!(task["evidence"][0]["rc"], 0);
        assert_eq!(task["touched_files"][0], "src/lib.rs");

        let err = cmd_runner(
            &h.repo,
            RunnerOptions {
                run_id: Some("r1".into()),
                task_id: Some("T2".into()),
                kind: "test".into(),
                command: Some("exit 7".into()),
                cwd: None,
                timeout: 5,
                touch: Vec::new(),
                note: None,
                instrument_ref: None,
                status_on_fail: "blocked".into(),
                runner: "codex".into(),
                allow_headless_write: false,
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
                tmux_target: None,
                tmux_mode: None,
                tmux_sentinel: None,
                tmux_session: None,
                tmux_new_window: false,
                tmux_new_session: false,
                tmux_window_name: None,
                tmux_ready_patterns: Vec::new(),
                tmux_skip_prompts: Vec::new(),
                tmux_ready_timeout_sec: None,
                tmux_bin: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("rc=7"));
        let state = h.state();
        let task = util::json_array(&state.tasks)
            .iter()
            .find(|task| task["id"] == "T2")
            .unwrap();
        assert_eq!(task["status"], "blocked");
        assert!(
            task["blockers"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("rc=7")
        );
    }

    #[test]
    fn preflight_readiness_combines_base_contract_and_advisory_flags() {
        let state = LtoState {
            run_id: "r1".into(),
            goal: "ship".into(),
            done_when: " ".into(),
            host_runtime: "unknown".into(),
            delivery_contract: DeliveryContract::new(
                vec!["unmeasured target".into()],
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            ..LtoState::default()
        };
        let ctx = util::RunContext {
            run_id: "r1".into(),
            run_dir: PathBuf::from(".lto/r1"),
            state_path: PathBuf::from(".lto/r1/state.json"),
            state,
        };

        let readiness = assess_preflight_run(&ctx);

        assert_eq!(readiness.missing, vec!["--done-when", "--instrument"]);
        assert_eq!(
            readiness.warnings,
            vec!["--why", "--host", "--constraint", "--entropy-check"]
        );
        assert!(!readiness.is_ready());
    }

    #[test]
    fn preflight_run_error_json_preserves_environment_result() {
        let h = Harness::new();
        let error =
            load_preflight_run(&h.repo, Some("missing-run"), false, true, true, &[]).unwrap_err();
        let checks = vec![json!({"name": "sandbox_write", "pass": true})];

        let report = preflight_run_error_json(&checks, true, &h.repo, Some("missing-run"), &error);

        assert_eq!(report["environment"]["ok"], true);
        assert_eq!(report["environment"]["checks"], Value::Array(checks));
        assert!(report["environment"].get("skipped").is_none());
        assert_eq!(report["run_readiness"]["run_id"], "missing-run");
        assert_eq!(report["run_readiness"]["ok"], false);
        assert!(report["error"].as_str().unwrap().contains("missing-run"));
        assert!(!h.repo.join(".lto").exists());
    }

    #[cfg(unix)]
    #[test]
    fn cmd_preflight_main_path_records_environment_snapshot() {
        use std::os::unix::fs::PermissionsExt;

        let h = Harness::new();
        h.init_git();
        h.write_state(base_state());
        let runners = h.repo.join("scripts").join("delegate").join("runners");
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
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        cmd_preflight(
            &h.repo,
            PreflightOptions {
                run_id: Some("r1".into()),
                record: true,
                json: false,
            },
        )
        .unwrap();
        let state = h.state();
        assert_eq!(state.environment_snapshot.sandbox, "ok");
        assert_eq!(
            state.environment_snapshot.extra["preflight_verdict"],
            "pass"
        );
        // sandbox_write + git_repo + one advisory tool:hs check, plus one per runner.
        let recorded_checks = state.environment_snapshot.extra["checks"]
            .as_array()
            .unwrap();
        assert_eq!(recorded_checks.len(), util::KNOWN_RUNNERS.len() + 3);
        // The hs probe is present and marked advisory so it never gates.
        let hs = recorded_checks
            .iter()
            .find(|c| c.get("name").and_then(Value::as_str) == Some("tool:hs"))
            .expect("tool:hs check recorded");
        assert_eq!(hs.get("advisory").and_then(Value::as_bool), Some(true));
    }

    fn write_fake_tmux(repo: &Path) -> PathBuf {
        let bin = repo.join("fake-tmux");
        let log = repo.join("tmux-log.jsonl");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
log = pathlib.Path(r'''{log}''')
args = sys.argv[1:]
with log.open("a") as f:
    f.write(json.dumps(args) + "\n")
if args and args[0] == "capture-pane":
    print("tmux capture ok")
sys.exit(0)
"#,
            log = log.display(),
        );
        fs::write(&bin, script).unwrap();
        make_executable(&bin);
        bin
    }

    fn write_fake_tmux_worker(repo: &Path) -> PathBuf {
        let bin = repo.join("fake-tmux-worker");
        let log = repo.join("tmux-log.jsonl");
        let buffer = repo.join("tmux-buffer.txt");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, re, sys
log = pathlib.Path(r'''{log}''')
buffer = pathlib.Path(r'''{buffer}''')
args = sys.argv[1:]
stdin_data = sys.stdin.read() if args and args[0] == "load-buffer" else None
with log.open("a") as f:
    f.write(json.dumps(args + (["stdin", stdin_data] if stdin_data is not None else [])) + "\n")
if args and args[0] == "capture-pane":
    print("ready")
    sys.exit(0)
if args and args[0] == "load-buffer":
    buffer.write_text(stdin_data or "")
    sys.exit(0)
if args and args[0] == "paste-buffer":
    text = buffer.read_text() if buffer.exists() else ""
    match = re.search(r"(/[^\s']+\.worker\.json)", text)
    if match:
        rc = 7 if "exit 7" in text else 0
        pathlib.Path(match.group(1)).write_text(json.dumps({{"rc": rc, "carrier": "tmux"}}))
    sys.exit(0)
if args and args[0] == "wait-for":
    sys.exit(0)
sys.exit(0)
"#,
            log = log.display(),
            buffer = buffer.display(),
        );
        fs::write(&bin, script).unwrap();
        make_executable(&bin);
        bin
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn collect_agent_run_emits_runner_and_model_fields() {
        let h = Harness::new();
        h.write_state(LtoState {
            tasks: json!([
                {"id": "T1", "status": "pending", "commands_run": [], "evidence": [], "blockers": []}
            ]),
            ..base_state()
        });
        fs::write(h.repo.join("reply.txt"), "collected output\n").unwrap();

        cmd_collect_agent_run(
            &h.repo,
            CollectAgentRunOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                runner: "codex".into(),
                reply: PathBuf::from("reply.txt"),
                meta: None,
                model: Some("gpt-5".into()),
                status: Some("ok".into()),
                elapsed_sec: Some(12.0),
                note: None,
            },
        )
        .unwrap();

        let events =
            fs::read_to_string(h.repo.join(".lto").join("r1").join("events.jsonl")).unwrap();
        let finished = events
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|event| event.get("type").and_then(Value::as_str) == Some("runner.finished"))
            .unwrap();
        assert_eq!(finished["fields"]["runner"], json!("codex"));
        assert_eq!(finished["fields"]["model"], json!("gpt-5"));
    }

    #[test]
    fn collect_agent_run_accepts_returned_status_alias() {
        let h = Harness::new();
        h.write_state(LtoState {
            tasks: json!([
                {"id": "T1", "status": "pending", "commands_run": [], "evidence": [], "blockers": []}
            ]),
            ..base_state()
        });
        fs::write(h.repo.join("reply.txt"), "collected output\n").unwrap();

        cmd_collect_agent_run(
            &h.repo,
            CollectAgentRunOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                runner: "codex".into(),
                reply: PathBuf::from("reply.txt"),
                meta: None,
                model: None,
                status: Some("returned".into()),
                elapsed_sec: None,
                note: None,
            },
        )
        .unwrap();

        let state = h.state();
        let agent_runs = util::iter_agent_runs(&state.agent_runs);
        assert_eq!(agent_runs.len(), 1);
        assert_eq!(agent_runs[0].status, JobStatus::Ok);
        assert!(agent_runs[0].error.is_empty());
        let events =
            fs::read_to_string(h.repo.join(".lto").join("r1").join("events.jsonl")).unwrap();
        assert!(events.contains("\"status\":\"ok\""));
        assert!(!events.contains("\"status\":\"returned\""));
    }

    #[test]
    fn collect_agent_run_bails_when_events_emit_fails_preventing_state_events_divergence() {
        // BUG-7: When safe_emit fails (e.g. hard-stop reached), the command
        // must bail BEFORE saving state. Otherwise state.agent_runs and
        // events.jsonl diverge permanently: autonomous_gate (reads state)
        // and cross_run_evidence (reads events) report different run counts.
        let h = Harness::new();
        h.write_state(LtoState {
            tasks: json!([
                {"id": "T1", "status": "pending", "commands_run": [], "evidence": [], "blockers": []}
            ]),
            ..base_state()
        });
        // Create a reply file so read_to_string_lossy won't fail.
        fs::write(h.repo.join("reply.txt"), "collected output\n").unwrap();
        // Fill events.jsonl to HARD_STOP_AT to trigger safe_emit hard-stop.
        let events_dir = h.repo.join(".lto").join("r1");
        let mut lines = String::new();
        for i in 0..crate::events::HARD_STOP_AT {
            lines.push_str(&format!("{{\"event_id\":{i}}}\n"));
        }
        fs::write(events_dir.join("events.jsonl"), &lines).unwrap();
        let err = cmd_collect_agent_run(
            &h.repo,
            CollectAgentRunOptions {
                run_id: Some("r1".into()),
                task_id: "T1".into(),
                runner: "codex".into(),
                reply: PathBuf::from("reply.txt"),
                meta: None,
                model: Some("gpt-5".into()),
                status: Some("ok".into()),
                elapsed_sec: Some(12.0),
                note: None,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("event emit failed"),
            "expected 'event emit failed' in error, got: {err}"
        );
        // State must NOT have the agent run — save_run was never called.
        let state = h.state();
        assert!(
            state.agent_runs.get("T1").is_none(),
            "agent_runs should be empty for T1 (state not saved), got: {:?}",
            state.agent_runs
        );
    }
}
