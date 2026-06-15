use crate::budget;
use crate::plugin;
use crate::state::{self, LtoState};
use clap::{CommandFactory, Parser, Subcommand};
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
        summary: Option<String>,
    },
    Resume {
        #[arg(long)]
        run_id: Option<String>,
    },
    Preflight,
    Runner,
    Judge,
    Hook,
    SelfTest,
    Parallel,
    Pipeline,
    Audit,
    Next,
    Autopilot,
    Recap,
    Budget {
        #[command(subcommand)]
        command: BudgetCommand,
    },
    Release,
    TaskAdd,
    TaskUpdate,
    Phase,
    CollectAgentRun,
    Runs,
    Memory,
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
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
    Validate { dir: PathBuf },
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
        Commands::Start { goal } => {
            let state = LtoState {
                goal: goal.unwrap_or_default(),
                ..LtoState::default()
            };
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        _ => {
            println!(
                "Rust v2 command surface is registered; this command still delegates to Python truth source until parity verification."
            );
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
}
