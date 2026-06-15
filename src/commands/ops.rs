use crate::agent_job::{
    AgentJob, AgentResult, Budget, Pattern, RetryPolicy, TaskSize, readonly_intent_to_policy,
};
use crate::budget::{self, BudgetStatus};
use crate::commands::util;
use crate::llm_judge;
use crate::scheduler::Scheduler;
use crate::worktree;
use anyhow::Context;
use serde_json::{Map, Value, json};
use sha2::Digest;
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
    pub status_on_fail: String,
    pub runner: String,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub job_file: Option<PathBuf>,
    pub job_id: Option<String>,
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

pub fn cmd_preflight(repo: &Path, options: PreflightOptions) -> anyhow::Result<()> {
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
    let pass = checks
        .iter()
        .all(|check| check.get("pass").and_then(Value::as_bool).unwrap_or(false));
    let passed = checks
        .iter()
        .filter(|check| check.get("pass").and_then(Value::as_bool).unwrap_or(false))
        .count();
    println!(
        "=== LTO Preflight ({}: {}/{}) ===",
        if pass { "pass" } else { "fail" },
        passed,
        checks.len()
    );
    for check in &checks {
        println!(
            "  {} {}: {}",
            if check.get("pass").and_then(Value::as_bool).unwrap_or(false) {
                "OK"
            } else {
                "FAIL"
            },
            check.get("name").and_then(Value::as_str).unwrap_or("?"),
            check.get("detail").and_then(Value::as_str).unwrap_or("")
        );
    }
    if options.record
        && let Ok(mut ctx) = util::load_run(repo, options.run_id.as_deref())
    {
        ctx.state.environment_snapshot.sandbox =
            if sandbox_write { "ok" } else { "fail" }.to_string();
        ctx.state.environment_snapshot.network = "unknown".to_string();
        ctx.state.environment_snapshot.captured_at = util::iso_now();
        ctx.state.environment_snapshot.extra.insert(
            "preflight_verdict".to_string(),
            json!(if pass { "pass" } else { "fail" }),
        );
        ctx.state
            .environment_snapshot
            .extra
            .insert("checks".to_string(), Value::Array(checks));
        util::save_run(&ctx)?;
    }
    if pass {
        Ok(())
    } else {
        anyhow::bail!("preflight failed")
    }
}

pub fn cmd_runner(repo: &Path, options: RunnerOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let jobs = load_jobs(job_file)?;
        let results = submit_jobs(repo, jobs)?;
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    if options.prompt.is_some() || options.prompt_file.is_some() {
        let run_id = util::resolve_run_id(repo, options.run_id.as_deref())
            .unwrap_or_else(|_| "rust-runner".to_string());
        let job = AgentJob {
            job_id: options.job_id.unwrap_or_else(|| "runner-1".to_string()),
            prompt_ref: options
                .prompt
                .clone()
                .or_else(|| {
                    options
                        .prompt_file
                        .as_ref()
                        .map(|path| path.display().to_string())
                })
                .unwrap_or_default(),
            runner: options.runner.clone(),
            prompt_is_inline: options.prompt.is_some(),
            model: None,
            env: BTreeMap::new(),
            permission_policy: readonly_intent_to_policy(&options.runner),
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
            meta: BTreeMap::from([("run_id".to_string(), json!(run_id))]),
        };
        let results = submit_jobs(repo, vec![job])?;
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    run_task_command(repo, options)
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
    util::save_run(&ctx)?;
    let rollup = util::token_rollup(&ctx.state);
    let budget = budget::check_budget(
        Some(&ctx.state.budget),
        &ctx.state.started_at,
        rollup.total_tokens,
        &util::iso_now(),
    );
    if budget.overall == BudgetStatus::Exceeded {
        println!("# LTO Autopilot -- budget gate BLOCKED");
        println!("AUTOPILOT_STATUS: NEEDS_CONFIRM");
        return Ok(());
    }
    if options.autonomous {
        let (ok, reason) = autonomous_gate(repo);
        println!("# LTO Autopilot -- autonomous");
        println!("  evidence gate: {}", if ok { "PASS" } else { "BLOCKED" });
        println!("  reason: {reason}");
        if !ok {
            println!("AUTOPILOT_STATUS: NEEDS_CONFIRM");
            return Ok(());
        }
    }
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
        "  route: {}",
        route["action"].as_str().unwrap_or("escalate")
    );
    println!("  reason: {}", route["reason"].as_str().unwrap_or(""));
    if options.auto_exec || options.autonomous {
        auto_exec_tasks(repo, &mut ctx, options.timeout, options.autonomous)?;
    } else {
        println!(
            "  suggested cmd: {}",
            route.get("cmd").and_then(Value::as_str).unwrap_or("(none)")
        );
        println!("AUTOPILOT_STATUS: NEEDS_HOST");
    }
    util::save_run(&ctx)?;
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
    if util::json_array(&ctx.state.tasks)
        .iter()
        .any(|task| task.get("id").and_then(Value::as_str) == Some(options.task_id.as_str()))
    {
        anyhow::bail!("task id already exists: {}", options.task_id);
    }
    let phase = options
        .phase
        .clone()
        .unwrap_or_else(|| ctx.state.current_phase.clone());
    ensure_valid_phase(&phase)?;
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
    });
    if let Some(command) = options.command {
        task["commands_run"] = json!([command]);
    }
    util::json_array_mut(&mut ctx.state.tasks).push(task);
    util::save_run(&ctx)?;
    println!(
        "task {} added to phase '{}': {}",
        options.task_id, phase, options.title
    );
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
            "task-update is a no-op: pass at least one of --status / --phase / --note / --touch"
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
    util::save_run(&ctx)?;
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
    util::save_run(&ctx)?;
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
        status: util::parse_status(&status)?,
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
        error: if status == "ok" {
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
    let result_value = serde_json::to_value(result)?;
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
    util::save_run(&ctx)?;
    println!(
        "collected {} run for task {}: status={status}",
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

pub fn cmd_parallel(repo: &Path, options: ParallelOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let jobs = load_jobs(job_file)?;
        let results = submit_jobs(repo, jobs)?;
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    run_many_task_commands(repo, options)
}

pub fn cmd_pipeline(repo: &Path, options: PipelineOptions) -> anyhow::Result<()> {
    if let Some(job_file) = &options.job_file {
        let jobs = load_jobs(job_file)?;
        let results = submit_jobs(repo, jobs)?;
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
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
    let task = find_task_mut(&mut ctx.state.tasks, &task_id)?;
    let cwd = options.cwd.as_deref().unwrap_or(repo);
    let head_before = util::git_status(repo).head;
    let (rc, stdout, stderr, elapsed) =
        util::run_command_capture(repo, &command, Some(cwd), options.timeout)?;
    let head_after = util::git_status(repo).head;
    let evidence = json!({
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
    util::append_to_object_array(task, "evidence", evidence.clone());
    append_unique_strings(task, "touched_files", &options.touch);
    append_string(task, "commands_run", &command);
    if rc == 0 {
        task["status"] = json!("done");
        task["blockers"] = json!([]);
        if options.kind == "test" {
            ctx.state.gates["last_tested_head"] = json!(head_after);
        }
    } else {
        task["status"] = json!(options.status_on_fail);
        util::append_to_object_array(
            task,
            "blockers",
            json!({"reason": format!("command failed (rc={rc})"), "command": command, "evidence_kind": options.kind, "at": util::iso_now()}),
        );
        ctx.state.last_failure = json!(format!("{task_id}: {} rc={rc}", options.kind));
    }
    task["last_update"] = json!(util::iso_now());
    util::save_run(&ctx)?;
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
    util::save_run(&ctx)?;
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
    match plan {
        llm_judge::JudgeDispatchPlan::Ready { runner, job, .. } => {
            let results = submit_jobs(repo, vec![*job])?;
            let result = results.first().context("judge produced no result")?;
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
            println!("judge verdict: {hash}");
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        skipped => {
            println!("{}", serde_json::to_string_pretty(&skipped)?);
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
        .filter(|risk| risk.get("disposition").and_then(Value::as_str) == Some("open"))
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
    timeout: u64,
    autonomous: bool,
) -> anyhow::Result<()> {
    let mut executed = 0;
    let mut held = 0;
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
            .map(str::to_string);
        let Some(command) = command else {
            continue;
        };
        let result = worktree::run_in_ephemeral_worktree(
            repo,
            &command,
            !autonomous,
            Duration::from_secs(timeout),
        )?;
        let id = task.get("id").and_then(Value::as_str).unwrap_or("?");
        if !result.executed {
            held += 1;
            println!("    [{id}] HELD -- {}", result.note);
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
            }),
        );
        task["status"] = json!(if rc == 0 { "done" } else { "blocked" });
    }
    println!(
        "AUTOPILOT_STATUS: {}",
        if held > 0 { "NEEDS_CONFIRM" } else { "DONE" }
    );
    println!("auto-exec: executed={executed} held={held}");
    Ok(())
}

fn autonomous_gate(repo: &Path) -> (bool, String) {
    let mut runs = 0_u64;
    let mut results = 0_u64;
    let Ok(entries) = fs::read_dir(repo.join(".lto")) else {
        return (false, "no .lto directory".to_string());
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
    if runs >= 5 && results >= 10 {
        (true, format!("{runs} run / {results} results"))
    } else {
        (
            false,
            format!(
                "autonomous requires >=5 real agent-run runs and >=10 results; current {runs}/{results}"
            ),
        )
    }
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
            "goal": ctx.state.goal,
            "phase": ctx.state.current_phase,
            "state_hash": format!("{:x}", sha2::Sha256::digest(&state_bytes)),
            "artifact_hash": format!("{:x}", sha2::Sha256::digest(&artifact_bytes)),
            "tasks": tasks,
            "source": "local .lto",
        }]
    }))
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
            status_on_fail: "blocked".to_string(),
            runner: "codex".to_string(),
            prompt: None,
            prompt_file: None,
            job_file: None,
            job_id: None,
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
                status_on_fail: "blocked".to_string(),
                runner: "codex".to_string(),
                prompt: None,
                prompt_file: None,
                job_file: None,
                job_id: None,
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
    let active_blockers = tasks
        .iter()
        .flat_map(|task| {
            task.get("blockers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let verdict = if has_failures || !active_blockers.is_empty() {
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
        .and_then(|task| task.get("commands_run").and_then(Value::as_array))
        .and_then(|items| items.last())
        .and_then(Value::as_str)
        .map(str::to_string)
}
