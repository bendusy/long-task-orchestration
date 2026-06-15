use crate::audit_dispatch;
use crate::budget;
use crate::commands::{closeout, ops, recap, resume};
use crate::plugin;
use crate::state::{self, LtoState};
use anyhow::Context;
use clap::{Args as ClapArgs, CommandFactory, Parser, Subcommand};
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
    "parallel",
    "pipeline",
    "audit",
    "next",
    "autopilot",
    "recap",
    "budget",
    "release",
    "task-add",
    "task-update",
    "phase",
    "collect-agent-run",
    "runs",
    "memory",
    "plugin",
];

#[derive(Debug, Parser)]
#[command(name = "lto-rs", about = "LTO Rust v2 core")]
pub struct Args {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Start {
        #[arg(long)]
        goal: Option<String>,
    },
    Check {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
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
    Resume {
        #[arg(long)]
        run_id: Option<String>,
    },
    Preflight {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        record: bool,
    },
    Runner(RunnerCommand),
    Judge(JudgeCommand),
    Hook {
        gate: String,
        #[arg(long)]
        force: bool,
        #[arg(long, default_value = "")]
        reason: String,
    },
    SelfTest,
    Parallel(ParallelCommand),
    Pipeline(PipelineCommand),
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
    Next {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
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
    Recap {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        artifacts: bool,
    },
    Budget {
        #[command(subcommand)]
        command: BudgetCommand,
    },
    Release {
        #[arg(long, default_value = "minor")]
        part: String,
        #[arg(long)]
        date: String,
        #[arg(long)]
        dry_run: bool,
    },
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
    Phase {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long = "set")]
        set_phase: Option<String>,
    },
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
    Runs,
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
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
        Commands::Check { run_id, json } => {
            let run_id = run_id
                .or_else(|| current_run_id(&args.repo))
                .unwrap_or_default();
            let path = state::state_path(&args.repo, &run_id);
            let state = state::load_state(&path)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "run_id": state.run_id,
                        "current_phase": state.current_phase,
                        "goal": state.goal,
                        "rust_v2": true
                    }))?
                );
            } else {
                println!("run_id: {}", state.run_id);
                println!("phase: {}", state.current_phase);
                println!("goal: {}", state.goal);
            }
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
            command: PluginCommand::Validate { dir },
        } => {
            let validation = plugin::validate_plugin(&dir)?;
            println!("{}", serde_json::to_string_pretty(&validation)?);
            if !validation.ok {
                anyhow::bail!("plugin validation failed");
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
                return Ok(());
            }
            for entry in std::fs::read_dir(lto)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() && entry.path().join("state.json").exists() {
                    println!("{}", entry.file_name().to_string_lossy());
                }
            }
        }
        Commands::Audit {
            run_id,
            auto_dispatch,
            discover_risks,
            allow_same_family,
        } => {
            let host = run_id
                .or_else(|| current_run_id(&args.repo))
                .and_then(|id| state::load_state(state::state_path(&args.repo, &id)).ok())
                .map(|state| state.host_runtime)
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let auditors = audit_dispatch::pick_auditors_with(&host, allow_same_family);
            let severity = if discover_risks {
                vec!["high", "critical", "medium"]
            } else {
                vec!["critical", "high", "medium", "low"]
            };
            let discoverer = if discover_risks {
                audit_dispatch::pick_healthy_discoverer(&args.repo, &auditors, &host)
            } else {
                None
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "rust_v2": true,
                    "mode": if discover_risks {
                        "discover_risks"
                    } else if auto_dispatch {
                        "auto_dispatch"
                    } else {
                        "prepare"
                    },
                    "host": host,
                    "auditors": auditors,
                    "discoverer": discoverer,
                    "severity": severity,
                    "scheduler_healthcheck": "used_for_discover_risks"
                }))?
            );
        }
        Commands::Start { goal } => {
            let state = LtoState {
                goal: goal.unwrap_or_default(),
                ..LtoState::default()
            };
            println!("{}", serde_json::to_string_pretty(&state)?);
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
        assert_eq!(COMMANDS.len(), 24);
    }

    #[test]
    fn audit_flags_are_registered() {
        Args::try_parse_from(["lto-rs", "audit", "--auto-dispatch"]).unwrap();
        Args::try_parse_from(["lto-rs", "audit", "--discover-risks"]).unwrap();
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
    fn commands_markdown_tracks_rust_command_contract() {
        let doc =
            std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("COMMANDS.md"))
                .unwrap();
        assert!(doc.contains("Command count: 24."));
        for command in COMMANDS {
            assert!(
                doc.contains(&format!("| `{command}`")),
                "COMMANDS.md missing {command}"
            );
        }
    }
}
