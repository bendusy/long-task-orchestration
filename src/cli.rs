use crate::audit_dispatch;
use crate::budget;
use crate::commands::{closeout, ops, recap, resume, util};
use crate::plugin;
use crate::plugin_eval_run;
use crate::state::{self, DeliveryContract, LtoState, WorkspaceSnapshot};
use anyhow::Context;
use chrono::Utc;
use clap::{Args as ClapArgs, CommandFactory, Parser, Subcommand};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const COMMANDS: &[&str] = &[
    "start",
    "check",
    "closeout",
    "resume",
    "preflight",
    "runner",
    "judge",
    "hook",
    "self-test",
    "audit",
    "next",
    "autopilot",
    "recap",
    "budget",
    "release",
    "task",
    "run",
    "collect-agent-run",
    "runs",
    "memory",
    "plugin",
];

#[derive(Debug, Parser)]
#[command(name = "lto-rs", version, about = "LTO Rust v2 core")]
pub struct Args {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "Create a new local LTO run")]
    Start {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        why: Option<String>,
        #[arg(long = "done-when")]
        done_when: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        target: Vec<String>,
        #[arg(long)]
        constraint: Vec<String>,
        #[arg(long)]
        instrument: Vec<String>,
        #[arg(long = "entropy-check")]
        entropy_check: Vec<String>,
        #[arg(long)]
        force: bool,
    },
    #[command(about = "Check run gates, phase evidence, and ledger status")]
    Check {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        strict: bool,
        #[arg(long = "to", value_parser = ["implementation", "closed"])]
        to_phase: Option<String>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Close a finished run and write handoff artifacts")]
    Closeout {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        summary: String,
        #[arg(long, default_value = "none")]
        next_action: String,
        #[arg(long, default_value = "none")]
        blocked_by: String,
        #[arg(long)]
        allow_dirty: bool,
        #[arg(long)]
        no_changelog: bool,
        #[arg(long)]
        force: bool,
    },
    #[command(about = "Print an agent resume capsule for the active run")]
    Resume {
        #[arg(long)]
        run_id: Option<String>,
    },
    #[command(about = "Probe repo, git, and runner health before work")]
    Preflight {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        record: bool,
    },
    #[command(about = "Run or dispatch one scheduler-backed task")]
    Runner(RunnerCommand),
    #[command(about = "Record or run an evidence-based judgment")]
    Judge(JudgeCommand),
    #[command(about = "Run an opt-in boundary hook")]
    Hook {
        gate: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "")]
        reason: String,
    },
    #[command(about = "Run the built-in CLI contract self-test")]
    SelfTest,
    #[command(about = "Run batch and staged job primitives")]
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    #[command(hide = true)]
    Parallel(ParallelCommand),
    #[command(hide = true)]
    Pipeline(PipelineCommand),
    #[command(about = "Dispatch and collect heterogeneous audit rounds")]
    Audit {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        auto_dispatch: bool,
        #[arg(long)]
        discover_risks: bool,
        #[arg(long)]
        allow_same_family: bool,
    },
    #[command(about = "Print deterministic next-step facts")]
    Next {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Advance safe mechanical steps under LTO gates")]
    Autopilot {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        supervised: bool,
        #[arg(long)]
        auto_exec: bool,
        #[arg(long)]
        autonomous: bool,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    #[command(about = "Render a human progress recap")]
    Recap {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        artifacts: bool,
    },
    #[command(about = "Inspect budget usage")]
    Budget {
        #[command(subcommand)]
        command: BudgetCommand,
    },
    #[command(about = "Plan a host-owned release")]
    Release {
        #[arg(long, default_value = "minor")]
        part: String,
        #[arg(long)]
        date: String,
        #[arg(long)]
        dry_run: bool,
    },
    #[command(about = "Manage tasks and run phase")]
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    #[command(hide = true)]
    TaskAdd {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        phase: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    #[command(hide = true)]
    TaskUpdate {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        phase: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        touch: Vec<String>,
    },
    #[command(hide = true)]
    Phase {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long = "set")]
        set_phase: Option<String>,
    },
    #[command(about = "Register an existing agent reply artifact")]
    CollectAgentRun {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        runner: String,
        #[arg(long)]
        reply: PathBuf,
        #[arg(long)]
        meta: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        elapsed_sec: Option<f64>,
        #[arg(long)]
        note: Option<String>,
    },
    #[command(about = "List local LTO runs")]
    Runs,
    #[command(about = "Export, publish, or resume redacted run memory")]
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    #[command(about = "Manage data-only plugins and eval packs")]
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    #[command(about = "Run multiple task commands concurrently")]
    Parallel(ParallelCommand),
    #[command(about = "Run staged commands for selected tasks")]
    Pipeline(PipelineCommand),
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    #[command(about = "Add a pending task")]
    Add {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        phase: Option<String>,
        #[arg(long)]
        command: Option<String>,
    },
    #[command(about = "Update task status, notes, phase, or touched paths")]
    Update {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        phase: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        touch: Vec<String>,
    },
    #[command(about = "Show or set the current run phase")]
    Phase {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long = "set")]
        set_phase: Option<String>,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RunnerCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long, default_value = "test")]
    kind: String,
    #[arg(long)]
    command: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long)]
    touch: Vec<String>,
    #[arg(long)]
    note: Option<String>,
    #[arg(long, default_value = "blocked")]
    status_on_fail: String,
    #[arg(long, default_value = "codex")]
    runner: String,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    job_file: Option<PathBuf>,
    #[arg(long)]
    job_id: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct JudgeCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    #[arg(long)]
    phase: Option<String>,
    #[arg(long, default_value = "codex")]
    runner: String,
    #[arg(long)]
    rerun_tests: bool,
    #[arg(long)]
    case_dir: Option<PathBuf>,
    #[arg(long)]
    brief: Option<PathBuf>,
    #[arg(long)]
    baseline_reply: Option<PathBuf>,
    #[arg(long)]
    candidate_reply: Option<PathBuf>,
    #[arg(long)]
    candidate_runner: Option<String>,
    #[arg(long)]
    judge_runner: Option<String>,
    #[arg(long)]
    execute: bool,
}

#[derive(Debug, ClapArgs)]
pub struct ParallelCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    task_ids: Vec<String>,
    #[arg(long)]
    phase: Option<String>,
    #[arg(long, default_value = "test")]
    kind: String,
    #[arg(long)]
    command: Option<String>,
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    #[arg(long)]
    job_file: Option<PathBuf>,
}

#[derive(Debug, ClapArgs)]
pub struct PipelineCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    task_ids: Vec<String>,
    #[arg(long)]
    phase: Option<String>,
    #[arg(long)]
    stages: Vec<String>,
    #[arg(long, default_value = "test")]
    kind: String,
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    #[arg(long)]
    continue_on_error: bool,
    #[arg(long)]
    job_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    Export {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Publish {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        am_bin: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    Resume {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        am_bin: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum BudgetCommand {
    Check {
        #[arg(long)]
        run_id: String,
        #[arg(long, default_value_t = 0)]
        tokens: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    List,
    Validate {
        dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    RenderProfile {
        dir: PathBuf,
        profile_id: String,
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long = "meta-output")]
        meta_output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    Eval {
        dir: PathBuf,
        #[arg(long = "eval-id")]
        eval_id: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    EvalRun {
        dir: PathBuf,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long = "eval-id")]
        eval_id: Option<String>,
        #[arg(long = "case")]
        only_case: Option<String>,
        #[arg(long, default_value_t = 4)]
        max_concurrency: usize,
        #[arg(long = "no-persist", default_value_t = true, action = clap::ArgAction::SetFalse)]
        persist: bool,
        #[arg(long = "runners-dir")]
        runners_dir: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    SourceNote {
        dir: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        url: String,
        #[arg(long = "claim")]
        claims: Vec<String>,
        #[arg(long = "hypothesis")]
        hypotheses: Vec<String>,
        #[arg(long = "append-manifest", conflicts_with = "no_append_manifest")]
        append_manifest: bool,
        #[arg(long = "no-append-manifest")]
        no_append_manifest: bool,
        #[arg(long)]
        json: bool,
    },
    Mount {
        dir: PathBuf,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        mounts_json: Option<PathBuf>,
    },
}

pub fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    run_args(args)
}

pub fn run_args(args: Args) -> anyhow::Result<()> {
    match args.command {
        Commands::SelfTest => {
            assert_command_count();
            println!("SELFTEST OK");
        }
        Commands::Check {
            run_id,
            strict,
            to_phase,
            json,
        } => {
            ops::cmd_check(
                &args.repo,
                ops::CheckOptions {
                    run_id,
                    strict,
                    to_phase,
                    json,
                },
            )?;
        }
        Commands::Budget {
            command: BudgetCommand::Check { run_id, tokens },
        } => {
            let path = state::state_path(&args.repo, &run_id);
            let state = state::load_state(&path)?;
            let now = state::iso_now();
            let check = budget::check_budget(Some(&state.budget), &state.started_at, tokens, &now);
            println!("{}", serde_json::to_string_pretty(&check)?);
        }
        Commands::Plugin {
            command: PluginCommand::List,
        } => {
            for path in plugin::discover_plugins(&args.repo) {
                println!("{}", path.display());
            }
        }
        Commands::Plugin {
            command: PluginCommand::Validate { dir, json: _ },
        } => {
            let validation = plugin::validate_plugin(&dir)?;
            println!("{}", serde_json::to_string_pretty(&validation)?);
            if !validation.ok {
                anyhow::bail!("plugin validation failed");
            }
        }
        Commands::Plugin {
            command:
                PluginCommand::RenderProfile {
                    dir,
                    profile_id,
                    input,
                    output,
                    meta_output,
                    json,
                },
        } => {
            let meta = plugin::render_profile(&dir, &profile_id, &input, &output)?;
            if let Some(meta_output) = meta_output {
                if let Some(parent) = meta_output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&meta_output, serde_json::to_string_pretty(&meta)? + "\n")?;
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&meta)?);
            } else {
                println!("rendered {profile_id} -> {}", output.display());
            }
        }
        Commands::Plugin {
            command:
                PluginCommand::Eval {
                    dir,
                    eval_id,
                    output,
                    json,
                },
        } => {
            let report = plugin::static_eval(&dir, eval_id.as_deref())?;
            let should_print = json || output.is_none();
            if let Some(output) = output.as_ref() {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(output, serde_json::to_string_pretty(&report)? + "\n")?;
                if !json {
                    println!(
                        "plugin eval {} -> {}",
                        if report.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                            "OK"
                        } else {
                            "FAIL"
                        },
                        output.display()
                    );
                }
            }
            if should_print {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if report.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                anyhow::bail!("plugin eval failed");
            }
        }
        Commands::Plugin {
            command:
                PluginCommand::EvalRun {
                    dir,
                    run_id,
                    eval_id,
                    only_case,
                    max_concurrency,
                    persist,
                    runners_dir,
                    output,
                    json,
                },
        } => {
            let run_id = run_id
                .or_else(|| current_run_id(&args.repo))
                .context("plugin eval-run requires --run-id or .lto/current")?;
            let report = plugin_eval_run::eval_run(
                &args.repo,
                &run_id,
                &dir,
                plugin_eval_run::EvalRunOptions {
                    eval_id: eval_id.as_deref(),
                    only_case: only_case.as_deref(),
                    max_concurrency,
                    persist,
                    runners_dir: runners_dir.as_deref(),
                },
            )?;
            let should_print = json || output.is_none();
            if let Some(output) = output.as_ref() {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(output, serde_json::to_string_pretty(&report)? + "\n")?;
                if !json {
                    println!(
                        "plugin eval-run {} -> {}",
                        if report.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                            "OK"
                        } else {
                            "FAIL"
                        },
                        output.display()
                    );
                }
            }
            if should_print {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if report.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                std::process::exit(2);
            }
        }
        Commands::Plugin {
            command:
                PluginCommand::SourceNote {
                    dir,
                    id,
                    title,
                    url,
                    claims,
                    hypotheses,
                    append_manifest,
                    no_append_manifest,
                    json,
                },
        } => {
            let append_manifest = append_manifest || !no_append_manifest;
            let path = match plugin::create_source_note(
                &dir,
                &id,
                &title,
                &url,
                &claims,
                &hypotheses,
                append_manifest,
            ) {
                Ok(path) => path,
                Err(err) => {
                    eprintln!("plugin source-note failed: {err}");
                    std::process::exit(2);
                }
            };
            let result = serde_json::json!({
                "appended_manifest": append_manifest,
                "id": id,
                "path": path,
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("source note: {}", path.display());
            }
        }
        Commands::Plugin {
            command:
                PluginCommand::Mount {
                    dir,
                    run_id,
                    mounts_json,
                },
        } => {
            let mounts_json = match mounts_json {
                Some(path) if path.is_absolute() => path,
                Some(path) => args.repo.join(path),
                None => {
                    let run_id = run_id.or_else(|| current_run_id(&args.repo)).context(
                        "plugin mount requires --run-id, .lto/current, or --mounts-json",
                    )?;
                    args.repo
                        .join(".lto")
                        .join(run_id)
                        .join("plugin-mounts.json")
                }
            };
            let entry = plugin::mount_plugin(&dir, &mounts_json)?;
            println!("{}", serde_json::to_string_pretty(&entry)?);
            println!("mounts_json: {}", mounts_json.display());
        }
        Commands::Runs => {
            let lto = args.repo.join(".lto");
            if !lto.exists() {
                println!("# LTO runs in this project (0 total)");
                return Ok(());
            }
            let mut runs = Vec::new();
            for entry in std::fs::read_dir(lto)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() && entry.path().join("state.json").exists() {
                    runs.push(entry.file_name().to_string_lossy().to_string());
                }
            }
            runs.sort();
            println!("# LTO runs in this project ({} total)", runs.len());
            for run_id in runs {
                println!("{run_id}");
            }
        }
        Commands::Audit {
            run_id,
            auto_dispatch,
            discover_risks,
            allow_same_family,
        } => {
            cmd_audit(
                &args.repo,
                AuditOptions {
                    run_id,
                    auto_dispatch,
                    discover_risks,
                    allow_same_family,
                },
            )?;
        }
        Commands::Start {
            run_id,
            goal,
            why,
            done_when,
            host,
            target,
            constraint,
            instrument,
            entropy_check,
            force,
        } => {
            let run_dir = start_run(
                &args.repo,
                StartRunOptions {
                    run_id,
                    goal: goal.unwrap_or_default(),
                    why: why.unwrap_or_default(),
                    done_when: done_when.unwrap_or_default(),
                    host: host.unwrap_or_default(),
                    delivery_contract: DeliveryContract::new(
                        target,
                        constraint,
                        instrument,
                        entropy_check,
                    ),
                    force,
                },
            )?;
            println!("{}", run_dir.display());
        }
        Commands::Recap { run_id, artifacts } => {
            recap::cmd_recap(&args.repo, recap::RecapOptions { run_id, artifacts })?;
        }
        Commands::Resume { run_id } => {
            resume::cmd_resume(&args.repo, resume::ResumeOptions { run_id })?;
        }
        Commands::Closeout {
            run_id,
            summary,
            next_action,
            blocked_by,
            allow_dirty,
            no_changelog,
            force,
        } => {
            closeout::cmd_closeout(
                &args.repo,
                closeout::CloseoutOptions {
                    run_id,
                    summary,
                    next_action,
                    blocked_by,
                    allow_dirty,
                    no_changelog,
                    force,
                },
            )?;
        }
        Commands::Preflight { run_id, record } => {
            ops::cmd_preflight(&args.repo, ops::PreflightOptions { run_id, record })?;
        }
        Commands::Runner(cmd) => {
            ops::cmd_runner(
                &args.repo,
                ops::RunnerOptions {
                    run_id: cmd.run_id,
                    task_id: cmd.task_id,
                    kind: cmd.kind,
                    command: cmd.command,
                    cwd: cmd.cwd,
                    timeout: cmd.timeout,
                    touch: cmd.touch,
                    note: cmd.note,
                    status_on_fail: cmd.status_on_fail,
                    runner: cmd.runner,
                    prompt: cmd.prompt,
                    prompt_file: cmd.prompt_file,
                    job_file: cmd.job_file,
                    job_id: cmd.job_id,
                },
            )?;
        }
        Commands::Judge(cmd) => {
            ops::cmd_judge(
                &args.repo,
                ops::JudgeOptions {
                    run_id: cmd.run_id,
                    task_id: cmd.task_id,
                    phase: cmd.phase,
                    runner: cmd.runner,
                    rerun_tests: cmd.rerun_tests,
                    case_dir: cmd.case_dir,
                    brief: cmd.brief,
                    baseline_reply: cmd.baseline_reply,
                    candidate_reply: cmd.candidate_reply,
                    candidate_runner: cmd.candidate_runner,
                    judge_runner: cmd.judge_runner,
                    execute: cmd.execute,
                },
            )?;
        }
        Commands::Hook {
            gate,
            force,
            reason,
        } => {
            ops::cmd_hook(
                &args.repo,
                ops::HookOptions {
                    gate,
                    force,
                    reason,
                },
            )?;
        }
        Commands::Run { command } => match command {
            RunCommand::Parallel(cmd) => {
                ops::cmd_parallel(
                    &args.repo,
                    ops::ParallelOptions {
                        run_id: cmd.run_id,
                        task_ids: cmd.task_ids,
                        phase: cmd.phase,
                        kind: cmd.kind,
                        command: cmd.command,
                        timeout: cmd.timeout,
                        concurrency: cmd.concurrency,
                        job_file: cmd.job_file,
                    },
                )?;
            }
            RunCommand::Pipeline(cmd) => {
                ops::cmd_pipeline(
                    &args.repo,
                    ops::PipelineOptions {
                        run_id: cmd.run_id,
                        task_ids: cmd.task_ids,
                        phase: cmd.phase,
                        stages: cmd.stages,
                        kind: cmd.kind,
                        timeout: cmd.timeout,
                        concurrency: cmd.concurrency,
                        continue_on_error: cmd.continue_on_error,
                        job_file: cmd.job_file,
                    },
                )?;
            }
        },
        Commands::Parallel(cmd) => {
            ops::cmd_parallel(
                &args.repo,
                ops::ParallelOptions {
                    run_id: cmd.run_id,
                    task_ids: cmd.task_ids,
                    phase: cmd.phase,
                    kind: cmd.kind,
                    command: cmd.command,
                    timeout: cmd.timeout,
                    concurrency: cmd.concurrency,
                    job_file: cmd.job_file,
                },
            )?;
        }
        Commands::Pipeline(cmd) => {
            ops::cmd_pipeline(
                &args.repo,
                ops::PipelineOptions {
                    run_id: cmd.run_id,
                    task_ids: cmd.task_ids,
                    phase: cmd.phase,
                    stages: cmd.stages,
                    kind: cmd.kind,
                    timeout: cmd.timeout,
                    concurrency: cmd.concurrency,
                    continue_on_error: cmd.continue_on_error,
                    job_file: cmd.job_file,
                },
            )?;
        }
        Commands::Next { run_id, json } => {
            ops::cmd_next(&args.repo, ops::NextOptions { run_id, json })?;
        }
        Commands::Autopilot {
            run_id,
            supervised: _,
            auto_exec,
            autonomous,
            timeout,
        } => {
            ops::cmd_autopilot(
                &args.repo,
                ops::AutopilotOptions {
                    run_id,
                    auto_exec,
                    autonomous,
                    timeout,
                },
            )?;
        }
        Commands::Release {
            part,
            date,
            dry_run,
        } => {
            ops::cmd_release(
                &args.repo,
                ops::ReleaseOptions {
                    part,
                    date,
                    dry_run,
                },
            )?;
        }
        Commands::Task { command } => match command {
            TaskCommand::Add {
                run_id,
                task_id,
                title,
                phase,
                command,
            } => {
                ops::cmd_task_add(
                    &args.repo,
                    ops::TaskAddOptions {
                        run_id,
                        task_id,
                        title,
                        phase,
                        command,
                    },
                )?;
            }
            TaskCommand::Update {
                run_id,
                task_id,
                status,
                phase,
                note,
                touch,
            } => {
                ops::cmd_task_update(
                    &args.repo,
                    ops::TaskUpdateOptions {
                        run_id,
                        task_id,
                        status,
                        phase,
                        note,
                        touch,
                    },
                )?;
            }
            TaskCommand::Phase { run_id, set_phase } => {
                ops::cmd_phase(&args.repo, ops::PhaseOptions { run_id, set_phase })?;
            }
        },
        Commands::TaskAdd {
            run_id,
            task_id,
            title,
            phase,
            command,
        } => {
            ops::cmd_task_add(
                &args.repo,
                ops::TaskAddOptions {
                    run_id,
                    task_id,
                    title,
                    phase,
                    command,
                },
            )?;
        }
        Commands::TaskUpdate {
            run_id,
            task_id,
            status,
            phase,
            note,
            touch,
        } => {
            ops::cmd_task_update(
                &args.repo,
                ops::TaskUpdateOptions {
                    run_id,
                    task_id,
                    status,
                    phase,
                    note,
                    touch,
                },
            )?;
        }
        Commands::Phase { run_id, set_phase } => {
            ops::cmd_phase(&args.repo, ops::PhaseOptions { run_id, set_phase })?;
        }
        Commands::CollectAgentRun {
            run_id,
            task_id,
            runner,
            reply,
            meta,
            model,
            status,
            elapsed_sec,
            note,
        } => {
            ops::cmd_collect_agent_run(
                &args.repo,
                ops::CollectAgentRunOptions {
                    run_id,
                    task_id,
                    runner,
                    reply,
                    meta,
                    model,
                    status,
                    elapsed_sec,
                    note,
                },
            )?;
        }
        Commands::Memory { command } => {
            let action = match command {
                MemoryCommand::Export { run_id, dry_run: _ } => {
                    ops::MemoryAction::Export { run_id }
                }
                MemoryCommand::Publish {
                    run_id,
                    am_bin,
                    timeout,
                } => ops::MemoryAction::Publish {
                    run_id,
                    am_bin,
                    timeout,
                },
                MemoryCommand::Resume {
                    project,
                    run_id,
                    am_bin,
                    timeout,
                } => ops::MemoryAction::Resume {
                    project,
                    run_id,
                    am_bin,
                    timeout,
                },
            };
            ops::cmd_memory(&args.repo, action)?;
        }
    }
    Ok(())
}

fn current_run_id(repo: &Path) -> Option<String> {
    let current = repo.join(".lto").join("current");
    std::fs::read_to_string(current)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

struct AuditOptions {
    run_id: Option<String>,
    auto_dispatch: bool,
    discover_risks: bool,
    allow_same_family: bool,
}

fn cmd_audit(repo: &Path, options: AuditOptions) -> anyhow::Result<()> {
    let run_id = options
        .run_id
        .or_else(|| current_run_id(repo))
        .context("audit requires --run-id or .lto/current")?;
    let run_dir = repo.join(".lto").join(&run_id);
    let state_path = run_dir.join("state.json");
    let state = state::load_state(&state_path)?;
    let host = effective_audit_host(&state);
    let auditors = audit_dispatch::pick_auditors_with(&host, options.allow_same_family);
    let audit_dir = run_dir.join("audit");
    fs::create_dir_all(&audit_dir)?;

    if options.discover_risks {
        dispatch_risk_discovery(
            repo, &run_id, &run_dir, &audit_dir, &state, &host, &auditors,
        )?;
    }

    let targets = high_risk_tasks(&state);
    let brief_path = audit_dir.join(format!("audit-brief-{}.md", timestamp_slug()));
    fs::write(&brief_path, build_audit_brief(&state, &host, &targets))?;
    register_run_artifact(
        repo,
        &run_id,
        &brief_path,
        &state,
        RunArtifactRecord {
            kind: "audit_brief",
            producer: "lto-rs.audit.prepare",
            summary: "audit brief",
            tags: &["audit", "brief"],
        },
    )?;

    if !options.auto_dispatch {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "rust_v2": true,
                "mode": "prepare",
                "host": host,
                "auditors": auditors,
                "brief": util::repo_relative_path(repo, &brief_path)?,
            }))?
        );
        return Ok(());
    }

    if auditors.is_empty() {
        anyhow::bail!("audit auto-dispatch has no heterogeneous auditors for host {host}");
    }
    let runners_dir = repo.join("scripts").join("delegate").join("runners");
    let results =
        audit_dispatch::submit_auto_dispatch(repo, &runners_dir, &brief_path, &auditors, &host)?;
    let replies_dir = audit_dir.join("replies");
    fs::create_dir_all(&replies_dir)?;
    let mut used = Vec::new();
    let mut counts = SeverityCounts::default();
    for result in &results {
        let reply_path = replies_dir.join(format!("reply-{}.md", result.runner));
        fs::write(&reply_path, &result.reply_text)?;
        register_run_artifact(
            repo,
            &run_id,
            &reply_path,
            &state,
            RunArtifactRecord {
                kind: "audit_reply",
                producer: "lto-rs.audit.auto_dispatch",
                summary: &format!("{} audit reply", result.runner),
                tags: &["audit", "reply"],
            },
        )?;
        used.push(result.runner.clone());
        counts.add_reply(&result.reply_text);
        println!(
            "  {}: {} exit={:?}",
            result.runner,
            result.status.as_str(),
            result.exit_code
        );
    }
    let ledger_path = run_dir.join("audit-ledger.md");
    append_audit_ledger_round(
        repo,
        &run_id,
        &ledger_path,
        &state,
        &replies_dir,
        &used,
        counts,
    )?;
    println!(
        "audit ledger: {}",
        util::repo_relative_path(repo, &ledger_path)?
    );
    Ok(())
}

fn dispatch_risk_discovery(
    repo: &Path,
    run_id: &str,
    run_dir: &Path,
    audit_dir: &Path,
    state: &LtoState,
    host: &str,
    auditors: &[String],
) -> anyhow::Result<()> {
    let Some(discoverer) = audit_dispatch::pick_healthy_discoverer(repo, auditors, host) else {
        anyhow::bail!("risk discovery has no healthy heterogeneous discoverer for host {host}");
    };
    let brief_path = audit_dir.join(format!("risk-brief-{}.md", timestamp_slug()));
    fs::write(&brief_path, build_risk_brief(state, host))?;
    register_run_artifact(
        repo,
        run_id,
        &brief_path,
        state,
        RunArtifactRecord {
            kind: "audit_brief",
            producer: "lto-rs.audit.discover_risks",
            summary: "risk discovery brief",
            tags: &["audit", "risk", "brief"],
        },
    )?;
    let runners_dir = repo.join("scripts").join("delegate").join("runners");
    let job = audit_dispatch::build_risk_discovery_job(&brief_path, &discoverer, host);
    let results = crate::scheduler::Scheduler::new(repo, runners_dir).submit_blocking(vec![job])?;
    let Some(result) = results.first() else {
        anyhow::bail!("risk discovery returned no result");
    };
    if result.status != crate::agent_job::JobStatus::Ok {
        anyhow::bail!(
            "risk discovery runner {} returned {} exit={:?}: {}",
            result.runner,
            result.status.as_str(),
            result.exit_code,
            result.error
        );
    }
    let reply_path = audit_dir.join(format!("risk-reply-{}-{}.md", discoverer, timestamp_slug()));
    fs::write(&reply_path, &result.reply_text)?;
    register_run_artifact(
        repo,
        run_id,
        &reply_path,
        state,
        RunArtifactRecord {
            kind: "risk_discovery_reply",
            producer: "lto-rs.audit.discover_risks",
            summary: &format!("{discoverer} risk discovery reply"),
            tags: &["audit", "risk", "reply"],
        },
    )?;
    let risks = parse_findings_or_empty(&result.reply_text);
    if risks.is_empty() {
        println!("risk discovery: {discoverer} reported no structured risks");
        return Ok(());
    }
    let state_path = run_dir.join("state.json");
    let mut state = state::load_state(&state_path)?;
    append_discovered_risk_points(&mut state, risks);
    state::save_state(&state_path, &state)?;
    util::sync_run_state_md(&run_dir.join("run-state.md"), &state)?;
    println!("risk discovery: {discoverer} added risk points");
    Ok(())
}

fn append_discovered_risk_points(state: &mut LtoState, risks: Vec<Value>) -> usize {
    let risk_points = util::json_array_mut(&mut state.risk_points);
    let mut next = risk_points.len() + 1;
    let mut added = 0;
    for risk in risks {
        let claim = risk
            .get("claim")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if claim.is_empty() {
            continue;
        }
        risk_points.push(json!({
            "id": format!("RP-auto-{next}"),
            "source": "risk-agent",
            "claim": claim,
            "evidence_to_check": risk.get("evidence_to_check").and_then(Value::as_str).unwrap_or(""),
            "severity": risk.get("severity").and_then(Value::as_str).unwrap_or("medium"),
            "status": "open",
            "disposition": "open",
            "recorded_at": util::iso_now(),
        }));
        next += 1;
        added += 1;
    }
    added
}

#[derive(Debug, Clone, Copy, Default)]
struct SeverityCounts {
    high: u64,
    critical: u64,
    minor: u64,
}

impl SeverityCounts {
    fn add_reply(&mut self, text: &str) {
        let findings = parse_findings_or_empty(text);
        if findings.is_empty() {
            self.high += text.matches("high").count() as u64;
            self.critical += text.matches("critical").count() as u64;
            return;
        }
        for finding in findings {
            match finding
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "critical" => self.critical += 1,
                "high" => self.high += 1,
                "medium" | "low" => self.minor += 1,
                _ => {}
            }
        }
    }
}

fn parse_findings_or_empty(text: &str) -> Vec<Value> {
    if let Some(findings) = crate::audit::parse_findings_text(text) {
        return findings
            .into_iter()
            .filter_map(|finding| serde_json::to_value(finding).ok())
            .collect();
    }
    parse_json_findings(text).unwrap_or_default()
}

fn parse_json_findings(text: &str) -> Option<Vec<Value>> {
    serde_json::from_str::<Value>(text.trim())
        .ok()
        .and_then(findings_from_value)
        .or_else(|| {
            text.split("```json").skip(1).find_map(|tail| {
                let body = tail.split("```").next()?.trim();
                serde_json::from_str::<Value>(body)
                    .ok()
                    .and_then(findings_from_value)
            })
        })
}

fn findings_from_value(value: Value) -> Option<Vec<Value>> {
    if let Some(items) = value.as_array() {
        return Some(items.clone());
    }
    value.get("findings")?.as_array().cloned()
}

fn append_audit_ledger_round(
    repo: &Path,
    run_id: &str,
    ledger_path: &Path,
    state: &LtoState,
    replies_dir: &Path,
    auditors: &[String],
    counts: SeverityCounts,
) -> anyhow::Result<()> {
    ensure_audit_ledger(ledger_path)?;
    let content = fs::read_to_string(ledger_path)?;
    let label = next_audit_round_label(&content);
    let artifact = util::repo_relative_path(repo, replies_dir)?;
    let row = format!(
        "| {label} | {artifact} | {} | {} | {} | {} | {} | open |",
        auditors.join(" "),
        counts.high,
        counts.critical,
        counts.minor,
        if label == "R1" { "start" } else { "flat" },
    );
    let updated = if label == "R1" {
        let placeholder = "| R1 |  |  |  |  |  | start | open |";
        if content.contains(placeholder) {
            content.replacen(placeholder, &row, 1)
        } else {
            insert_audit_round_row(&content, &row)
        }
    } else {
        insert_audit_round_row(&content, &row)
    };
    fs::write(ledger_path, updated)?;
    register_run_artifact(
        repo,
        run_id,
        ledger_path,
        state,
        RunArtifactRecord {
            kind: "audit_ledger",
            producer: "lto-rs.audit.collect",
            summary: &format!("audit ledger updated {label}"),
            tags: &["audit", "ledger"],
        },
    )?;
    Ok(())
}

fn ensure_audit_ledger(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, include_str!("../templates/audit-ledger.md"))?;
    Ok(())
}

fn next_audit_round_label(content: &str) -> String {
    let max_round = content
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("| R"))
        .filter_map(|tail| tail.split('|').next()?.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    if content.contains("| R1 |  |  |  |  |  | start | open |") || max_round == 0 {
        "R1".to_string()
    } else {
        format!("R{}", max_round + 1)
    }
}

fn insert_audit_round_row(content: &str, row: &str) -> String {
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let idx = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("| R"))
        .map(|idx| idx + 1)
        .unwrap_or(lines.len());
    lines.insert(idx, row.to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

struct RunArtifactRecord<'a> {
    kind: &'a str,
    producer: &'a str,
    summary: &'a str,
    tags: &'a [&'a str],
}

fn register_run_artifact(
    repo: &Path,
    run_id: &str,
    path: &Path,
    state: &LtoState,
    record: RunArtifactRecord<'_>,
) -> anyhow::Result<()> {
    util::register_artifact(
        repo,
        run_id,
        path,
        util::ArtifactMeta {
            kind: record.kind,
            producer: record.producer,
            state,
            summary: record.summary,
            tags: record.tags,
        },
    )
}

fn effective_audit_host(state: &LtoState) -> String {
    if !state.host_runtime.trim().is_empty() {
        return state.host_runtime.trim().to_string();
    }
    std::env::var("LTO_HOST_RUNTIME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "codex".to_string())
}

fn high_risk_tasks(state: &LtoState) -> Vec<Value> {
    state
        .tasks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            let status = task
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            status != "skipped"
        })
        .collect()
}

fn build_audit_brief(state: &LtoState, host: &str, targets: &[Value]) -> String {
    let mut lines = vec![
        "# LTO Heterogeneous Audit Brief".to_string(),
        String::new(),
        format!("- goal: {}", state.goal),
        format!("- host_runtime: {host}"),
        format!("- phase: {}", state.current_phase),
        String::new(),
        "## Audit Targets".to_string(),
        String::new(),
    ];
    for task in targets {
        lines.push(format!(
            "### {}: {}",
            task.get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>"),
            task.get("title").and_then(Value::as_str).unwrap_or("")
        ));
        if let Some(files) = task.get("touched_files").and_then(Value::as_array) {
            let files = files
                .iter()
                .filter_map(Value::as_str)
                .take(12)
                .collect::<Vec<_>>();
            if !files.is_empty() {
                lines.push(format!("- touched: {}", files.join(", ")));
            }
        }
        if let Some(evidence) = task
            .get("evidence")
            .and_then(Value::as_array)
            .and_then(|v| v.last())
        {
            lines.push(format!("- latest evidence: {}", compact_json(evidence)));
        }
        lines.push(String::new());
    }
    lines.extend([
        "## Required Output".to_string(),
        String::new(),
        "Return the strongest blockers first. End with a JSON findings list.".to_string(),
        "Use severity critical/high/medium/low. Use [] if there are no findings.".to_string(),
        String::new(),
        "```json".to_string(),
        r#"[{"severity":"high","claim":"...","evidence_to_check":"...","file":"..."}]"#.to_string(),
        "```".to_string(),
    ]);
    lines.join("\n")
}

fn build_risk_brief(state: &LtoState, host: &str) -> String {
    format!(
        "# LTO Risk Discovery Brief\n\n- goal: {}\n- host_runtime: {}\n- phase: {}\n\nRead current state and recent changed files. Return only new high/critical/medium risks as JSON findings. Return [] if no new risks.\n",
        state.goal, host, state.current_phase
    )
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn timestamp_slug() -> String {
    Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

struct StartRunOptions {
    run_id: Option<String>,
    goal: String,
    why: String,
    done_when: String,
    host: String,
    delivery_contract: DeliveryContract,
    force: bool,
}

fn start_run(repo: &Path, options: StartRunOptions) -> anyhow::Result<PathBuf> {
    let StartRunOptions {
        run_id,
        goal,
        why,
        done_when,
        host,
        delivery_contract,
        force,
    } = options;
    let run_id = run_id.unwrap_or_else(|| default_run_id(&goal));
    state::validate_run_id(&run_id)?;
    let run_dir = repo.join(".lto").join(&run_id);
    if run_dir.exists() && !force {
        anyhow::bail!(
            "run already exists: {} (use --force to overwrite)",
            run_dir.display()
        );
    }

    fs::create_dir_all(&run_dir)?;
    let git = util::git_status(repo);
    let mut state = LtoState {
        run_id: run_id.clone(),
        goal: goal.clone(),
        why,
        done_when,
        host_runtime: host,
        workspace: WorkspaceSnapshot {
            repo_root: repo.display().to_string(),
            branch: git.branch,
            head: git.head,
            dirty_fingerprint: if git.dirty { "dirty" } else { "clean" }.to_string(),
            ..WorkspaceSnapshot::default()
        },
        original_user_request: goal,
        artifacts: json!({"manifest": format!(".lto/{run_id}/artifacts.json")}),
        ..LtoState::default()
    };
    state.delivery_contract = delivery_contract;
    state.started_at = state::iso_now();

    let state_path = run_dir.join("state.json");
    state::save_state(&state_path, &state)?;

    let run_state_path = run_dir.join("run-state.md");
    let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
        .join("run-state.md");
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("failed to read {}", template_path.display()))?;
    fs::write(&run_state_path, template)?;
    util::sync_run_state_md(&run_state_path, &state)?;

    fs::create_dir_all(repo.join(".lto"))?;
    fs::write(repo.join(".lto").join("current"), format!("{run_id}\n"))?;
    crate::events::safe_emit(
        repo,
        &run_id,
        crate::events::EventRecord {
            event_type: "run.started".to_string(),
            actor_kind: "host".to_string(),
            actor_id: if state.host_runtime.is_empty() {
                None
            } else {
                Some(state.host_runtime.clone())
            },
            phase: Some(state.current_phase.clone()),
            summary: state.goal.clone(),
            fields: json!({"why": state.why, "done_when": state.done_when}),
            ..crate::events::EventRecord::default()
        },
    );
    util::register_artifact(
        repo,
        &run_id,
        &state_path,
        util::ArtifactMeta {
            kind: "state_json",
            producer: "lto-rs.start",
            state: &state,
            summary: "machine state",
            tags: &["state"],
        },
    )?;
    util::register_artifact(
        repo,
        &run_id,
        &run_state_path,
        util::ArtifactMeta {
            kind: "run_state_md",
            producer: "lto-rs.start",
            state: &state,
            summary: "human-readable run state",
            tags: &["state"],
        },
    )?;
    let _ = crate::telemetry::save(repo, &run_id);
    Ok(run_dir)
}

fn default_run_id(goal: &str) -> String {
    let slug = slugify(goal);
    let digest = format!("{:x}", Sha256::digest(goal.as_bytes()));
    format!(
        "{}-{}-{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        slug,
        &digest[..8]
    )
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.trim().to_ascii_lowercase().chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            ch
        } else {
            '-'
        };
        if ch == '-' {
            if !last_dash && !out.is_empty() {
                out.push(ch);
            }
            last_dash = true;
        } else {
            out.push(ch);
            last_dash = false;
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "task".to_string()
    } else {
        out
    }
}

fn assert_command_count() {
    let clap_count = Args::command()
        .get_subcommands()
        .filter(|cmd| COMMANDS.contains(&cmd.get_name()))
        .count();
    assert_eq!(clap_count, COMMANDS.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_subcommand_count_matches_contract() {
        assert_command_count();
        assert_eq!(COMMANDS.len(), 21);
    }

    #[test]
    fn visible_commands_have_short_help() {
        for cmd in Args::command().get_subcommands() {
            let name = cmd.get_name();
            if COMMANDS.contains(&name) {
                let about = cmd
                    .get_about()
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                assert!(!about.trim().is_empty(), "{name} is missing short help");
            }
        }
    }

    #[test]
    fn clap_version_matches_package_version() {
        assert_eq!(
            Args::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn audit_flags_are_registered() {
        Args::try_parse_from(["lto-rs", "audit", "--auto-dispatch"]).unwrap();
        Args::try_parse_from(["lto-rs", "audit", "--discover-risks"]).unwrap();
    }

    #[test]
    fn grouped_run_commands_and_legacy_aliases_are_registered() {
        Args::try_parse_from([
            "lto-rs",
            "run",
            "parallel",
            "--task-ids",
            "T1",
            "--command",
            "cargo test",
        ])
        .unwrap();
        Args::try_parse_from([
            "lto-rs",
            "run",
            "pipeline",
            "--task-ids",
            "T1",
            "--stages",
            "fmt",
        ])
        .unwrap();
        Args::try_parse_from(["lto-rs", "parallel", "--task-ids", "T1"]).unwrap();
        Args::try_parse_from(["lto-rs", "pipeline", "--stages", "fmt"]).unwrap();
    }

    #[test]
    fn grouped_task_commands_and_legacy_aliases_are_registered() {
        Args::try_parse_from([
            "lto-rs",
            "task",
            "add",
            "--task-id",
            "T1",
            "--title",
            "Grouped task",
        ])
        .unwrap();
        Args::try_parse_from([
            "lto-rs",
            "task",
            "update",
            "--task-id",
            "T1",
            "--status",
            "done",
        ])
        .unwrap();
        Args::try_parse_from(["lto-rs", "task", "phase", "--set", "implementation"]).unwrap();
        Args::try_parse_from(["lto-rs", "task-add", "--task-id", "T1", "--title", "Old"]).unwrap();
        Args::try_parse_from([
            "lto-rs",
            "task-update",
            "--task-id",
            "T1",
            "--status",
            "done",
        ])
        .unwrap();
        Args::try_parse_from(["lto-rs", "phase", "--set", "implementation"]).unwrap();
    }

    #[test]
    fn check_flags_are_registered() {
        Args::try_parse_from(["lto-rs", "check", "--strict", "--to", "closed", "--json"]).unwrap();
        Args::try_parse_from(["lto-rs", "check", "--to", "implementation"]).unwrap();
        assert!(Args::try_parse_from(["lto-rs", "check", "--to", "deploy"]).is_err());
    }

    #[test]
    fn start_accepts_python_migration_metadata() {
        Args::try_parse_from([
            "lto-rs",
            "start",
            "--run-id",
            "rust-start",
            "--goal",
            "ship rust",
            "--why",
            "retire python fallback risk",
            "--done-when",
            "release binaries exist",
            "--host",
            "codex",
            "--target",
            "users can run lto without Python",
            "--constraint",
            "macOS/Linux first; Windows paused",
            "--instrument",
            "cargo test --locked --all-targets",
            "--entropy-check",
            "verify wrapper and legacy fixture separately",
            "--force",
        ])
        .unwrap();
    }

    #[test]
    fn start_persists_run_state_and_current_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        crate::process::git(repo, ["init"]).unwrap();
        let run_dir = start_run(
            repo,
            StartRunOptions {
                run_id: Some("r1".to_string()),
                goal: "ship rust".to_string(),
                why: "retire python fallback risk".to_string(),
                done_when: "release binaries exist".to_string(),
                host: "codex".to_string(),
                delivery_contract: DeliveryContract::new(
                    vec!["users can run lto without Python".to_string()],
                    vec!["macOS/Linux first; Windows paused".to_string()],
                    vec!["cargo test --locked --all-targets".to_string()],
                    vec!["verify wrapper and legacy fixture separately".to_string()],
                ),
                force: false,
            },
        )
        .unwrap();
        assert!(run_dir.join("state.json").exists());
        assert!(run_dir.join("run-state.md").exists());
        assert!(run_dir.join("artifacts.json").exists());
        assert_eq!(
            std::fs::read_to_string(repo.join(".lto").join("current")).unwrap(),
            "r1\n"
        );
        let state = state::load_state(run_dir.join("state.json")).unwrap();
        assert_eq!(state.goal, "ship rust");
        assert_eq!(state.why, "retire python fallback risk");
        assert_eq!(state.done_when, "release binaries exist");
        assert_eq!(state.host_runtime, "codex");
        assert_eq!(
            state.delivery_contract.targets,
            vec!["users can run lto without Python"]
        );
        assert!(state.delivery_contract.is_complete());
        let run_state = std::fs::read_to_string(run_dir.join("run-state.md")).unwrap();
        assert!(run_state.contains("- delivery_targets: users can run lto without Python"));
        assert!(run_state.contains("- delivery_instruments: cargo test --locked --all-targets"));
    }

    #[test]
    fn plugin_mount_command_accepts_explicit_mount_lock_path() {
        Args::try_parse_from([
            "lto-rs",
            "plugin",
            "mount",
            "plugins/dev-workflow",
            "--mounts-json",
            ".lto/r1/plugin-mounts.json",
        ])
        .unwrap();
    }

    #[test]
    fn plugin_static_commands_are_registered() {
        Args::try_parse_from([
            "lto-rs",
            "plugin",
            "render-profile",
            "plugins/deep-agent-profiles",
            "codex-audit-readonly-v1",
            "--input",
            "brief.md",
            "--output",
            "rendered.md",
            "--meta-output",
            "rendered.meta.json",
            "--json",
        ])
        .unwrap();
        Args::try_parse_from([
            "lto-rs",
            "plugin",
            "validate",
            "plugins/deep-agent-profiles",
            "--json",
        ])
        .unwrap();
        Args::try_parse_from([
            "lto-rs",
            "plugin",
            "eval",
            "plugins/deep-agent-profiles",
            "--eval-id",
            "profile-ab-cases-v1",
            "--output",
            "eval.json",
            "--json",
        ])
        .unwrap();
    }

    #[test]
    fn plugin_source_note_defaults_to_appending_manifest() {
        let args = Args::try_parse_from([
            "lto-rs",
            "plugin",
            "source-note",
            "plugins/deep-agent-profiles",
            "--id",
            "x.note",
            "--title",
            "A source",
            "--url",
            "https://example.test/source",
        ])
        .unwrap();
        let Commands::Plugin {
            command:
                PluginCommand::SourceNote {
                    append_manifest,
                    no_append_manifest,
                    ..
                },
        } = args.command
        else {
            panic!("expected plugin source-note");
        };
        assert!(append_manifest || !no_append_manifest);
    }

    #[test]
    fn plugin_source_note_can_disable_manifest_append() {
        let args = Args::try_parse_from([
            "lto-rs",
            "plugin",
            "source-note",
            "plugins/deep-agent-profiles",
            "--id",
            "x.note",
            "--title",
            "A source",
            "--url",
            "https://example.test/source",
            "--no-append-manifest",
        ])
        .unwrap();
        let Commands::Plugin {
            command:
                PluginCommand::SourceNote {
                    append_manifest,
                    no_append_manifest,
                    ..
                },
        } = args.command
        else {
            panic!("expected plugin source-note");
        };
        assert!(!append_manifest && no_append_manifest);
        assert!(
            Args::try_parse_from([
                "lto-rs",
                "plugin",
                "source-note",
                "plugins/deep-agent-profiles",
                "--id",
                "x.note",
                "--title",
                "A source",
                "--url",
                "https://example.test/source",
                "--append-manifest",
                "--no-append-manifest",
            ])
            .is_err()
        );
    }

    #[test]
    fn discovered_risk_points_default_to_open_disposition() {
        let mut state = LtoState::default();
        let added = append_discovered_risk_points(
            &mut state,
            vec![
                json!({
                    "claim": "closeout gate misses auto risks",
                    "evidence_to_check": "state.json risk_points",
                    "severity": "critical"
                }),
                json!({"claim": "   "}),
            ],
        );
        assert_eq!(added, 1);
        let risks = state.risk_points.as_array().unwrap();
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0]["status"], "open");
        assert_eq!(risks[0]["disposition"], "open");
    }

    #[test]
    fn commands_markdown_tracks_rust_command_contract() {
        let doc =
            std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("COMMANDS.md"))
                .unwrap();
        assert!(doc.contains("Command count: 22."));
        assert!(doc.contains("21 Rust-owned business"));
        assert!(doc.contains("clap built-in `help`"));
        for command in COMMANDS {
            assert!(
                doc.contains(&format!("| `{command}`")),
                "COMMANDS.md missing {command}"
            );
        }
    }
}
