use crate::audit_dispatch;
use crate::audit_ledger;
use crate::budget;
use crate::commands::{closeout, contract, ledger_check, ops, recap, resume, util};
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
    "contract",
    "check",
    "closeout",
    "resume",
    "preflight",
    "runner",
    "dispatch-and-wait",
    "dispatch-goal",
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
    "prune",
    "memory",
    "agent-turn-completed",
    "plugin",
    "events",
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
        #[arg(
            long,
            value_name = "[LABEL::]CMD",
            help = "Measurement command; use LABEL::CMD for a stable label, or CMD without ::"
        )]
        instrument: Vec<String>,
        #[arg(long = "entropy-check")]
        entropy_check: Vec<String>,
        #[arg(long)]
        force: bool,
    },
    #[command(about = "Update run readiness metadata and delivery contract")]
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },
    #[command(about = "Check run gates, phase evidence, and ledger status")]
    Check {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with_all = ["run_id", "to_phase", "json"]
        )]
        ledger: Option<PathBuf>,
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
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Run or dispatch one scheduler-backed task",
        long_about = "Run or dispatch one scheduler-backed task. Choose exactly one work mode:\n\
\n  \
--command \"<shell>\"   Run a shell command as evidence. REQUIRES --task-id.\n  \
--prompt \"<text>\"     Send a prompt to --runner (or --prompt-file <path>).\n  \
--job-file <path>     Run a pre-built job spec (with --job-id).\n\
\n\
The runner backend is --runner (default codex; see its help for the full set). \
Use --runner tmux for an interactive real-TUI session, or prefer `lto dispatch-goal`.\n\
\n\
Examples:\n  \
lto runner --task-id T1 --command \"cargo test\"          # evidence run\n  \
lto runner --task-id T1 --runner tmux --prompt \"fix X\"   # interactive dispatch\n  \
lto runner --job-file job.json --job-id J1                # run a job spec\n\
\n\
See also: lto dispatch-goal (goal-file dispatch), lto events --wait (await completion)."
    )]
    Runner(Box<RunnerCommand>),
    #[command(
        about = "Dispatch a goal file AND block until the agent completes (dispatch-goal + events --wait)",
        long_about = "One-step convenience: dispatch a goal to an external agent, then block until \
its agent.dispatch.completed event fires (primary: goal-self-report; optional side-channels: \
Codex Stop/update_goal proof or pi/agy process-exit with a real rc), and \
print a summary. Equivalent to `lto dispatch-goal ...` followed by `lto events --wait \
--event-type agent.dispatch.completed`, but in a single call.\n\
\n\
Examples:\n  \
lto dispatch-and-wait --runner codex --goal goal.md               # dispatch + wait (default 600s)\n  \
lto dispatch-and-wait --runner pi --goal goal.md --timeout 1200   # longer wait\n\
\n\
After it returns, register the reply with `lto collect-agent-run` if you need it as audit evidence.\n\
The single-step `lto dispatch-goal` and `lto events --wait` remain available for finer control."
    )]
    DispatchAndWait(DispatchAndWaitCommand),
    #[command(
        about = "Dispatch a goal file to codex, pi, or agy through tmux",
        long_about = "Dispatch a goal file to an external agent (codex/pi/agy) in a real tmux TUI. \
With no --target/--new-window it opens a visible window in your current tmux session. \
Primary completion is goal-self-report (agent runs lto agent-turn-completed --source \
goal-self-report); Codex Stop/update_goal and process-exit remain optional side-channels.\n\
\n\
Examples:\n  \
lto dispatch-goal --runner codex --goal goal.md          # dispatch into current tmux\n  \
lto dispatch-goal --runner agy --goal goal.md --new-window\n  \
lto dispatch-and-wait --runner codex --goal goal.md      # dispatch AND block until done\n\
\n\
See also: lto dispatch-and-wait (one-step dispatch+wait), lto events --wait (await), \
lto collect-agent-run (register the reply)."
    )]
    DispatchGoal(DispatchGoalCommand),
    #[command(
        about = "Record or run an evidence-based judgment",
        long_about = "Record an evidence-based judgment (strong/adequate/weak/none), or run an \
LLM judge over frozen evidence. State mode just records; LLM mode needs a runner + evidence.\n\
\n\
Examples:\n  \
lto judge --task-id T1 --verdict adequate --note \"tests pass\"   # record\n  \
lto judge --task-id T1 --runner codex --rerun-tests             # LLM judge\n\
\n\
See also: lto audit (cross-family audit), lto check (gate status)."
    )]
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
    #[command(
        about = "Dispatch and collect heterogeneous audit rounds",
        long_about = "Dispatch and collect cross-family audit rounds. --auto-dispatch picks healthy \
runners of a different family than the author and dispatches them; --discover-risks first surfaces \
risk points to audit. Fails closed if no healthy heterogeneous runner is available.\n\
\n\
Examples:\n  \
lto audit --auto-dispatch                                  # auto-pick + dispatch auditors\n  \
lto audit --auto-dispatch --prefer-runner codex --prefer-runner agy   # order the pool\n  \
lto audit --discover-risks                                 # surface risks first\n\
\n\
See also: lto events --wait (collect replies), lto judge (single judgment)."
    )]
    Audit {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        auto_dispatch: bool,
        #[arg(long)]
        discover_risks: bool,
        #[arg(long)]
        allow_same_family: bool,
        /// Restrict and order the cross-family auditor pool (repeatable).
        /// Keeps slow heavy-thinking runners off the closeout critical path,
        /// e.g. `--prefer-runner codex --prefer-runner agy` drops pi.
        #[arg(long = "prefer-runner")]
        prefer_runner: Vec<String>,
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
        #[arg(long = "worker-runner", default_value = "auto", value_parser = ["auto", "sandbox", "tmux"])]
        worker_runner: String,
        #[arg(long)]
        target: Option<String>,
        #[arg(long = "tmux-bin")]
        tmux_bin: Option<String>,
        #[arg(long = "ready-timeout")]
        ready_timeout: Option<u64>,
    },
    #[command(about = "Render a human progress recap")]
    Recap {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        artifacts: bool,
        #[arg(long)]
        mine: bool,
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
        #[arg(long = "instrument-ref")]
        instrument_ref: Option<String>,
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
    #[command(
        about = "Register an existing agent reply artifact",
        long_about = "Register a reply an external agent already produced into the run's agent_runs, \
so audit/judge can use it as evidence. --status defaults to `returned`.\n\
\n\
Example:\n  \
lto collect-agent-run --task-id T1 --runner agy --reply reply-agy.md --status ok\n\
\n\
See also: lto dispatch-goal (produce the reply), lto audit (use it as evidence)."
    )]
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
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(crate::agent_job::JOB_STATUS_INPUT_VALUES))]
        status: Option<String>,
        #[arg(long)]
        elapsed_sec: Option<f64>,
        #[arg(long)]
        note: Option<String>,
    },
    #[command(about = "List local LTO runs")]
    Runs,
    #[command(
        about = "Reclaim disk from finished runs (keeps state index, never touches active runs)",
        long_about = "Reclaim disk by removing bulk logs (events.jsonl, live/, audit/, dispatch/) \
from finished runs, while KEEPING the lightweight history index (state.json, run-state.md, \
artifacts.json). Only runs with phase=closed and older than --older-than days are eligible; \
active/unfinished runs are never touched. Dry-run by default — pass --yes to actually delete.\n\n\
Examples:\n  \
lto prune                         # dry-run: show what 30d+ closed runs would free\n  \
lto prune --yes                   # actually reclaim\n  \
lto prune --older-than 7 --yes    # prune closed runs older than 7 days\n  \
lto prune --keep-last 10 --yes    # keep the 10 most recent closed runs\n  \
lto prune --run-id <id> --yes     # prune one specific closed run"
    )]
    Prune {
        /// Show what would be reclaimed without deleting (default; use --yes to delete).
        #[arg(long)]
        dry_run: bool,
        /// Actually delete the bulk artifacts (turns off the default dry-run).
        #[arg(long)]
        yes: bool,
        /// Only prune closed runs older than this many days.
        #[arg(long, default_value_t = 30)]
        older_than: i64,
        /// Keep the most recent N closed runs untouched even if they qualify.
        #[arg(long, default_value_t = 0)]
        keep_last: usize,
        /// Prune one specific run (skips age/keep-last gates; still refuses active runs).
        #[arg(long)]
        run_id: Option<String>,
    },
    #[command(about = "Export, publish, or resume redacted run memory")]
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    #[command(
        hide = true,
        about = "Route an agent lifecycle event from a hook or process wrapper"
    )]
    AgentTurnCompleted(AgentTurnCompletedCommand),
    #[command(about = "Manage data-only plugins and eval packs")]
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },
    #[command(about = "Block until a matching run event appears")]
    Events(EventsCommand),
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
        #[arg(long = "instrument-ref")]
        instrument_ref: Option<String>,
    },
    #[command(about = "Update task status, notes, phase, or touched paths")]
    Update {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        task_id: String,
        /// New task status: pending | in_progress | blocked | done | skipped.
        #[arg(long, value_parser = crate::commands::util::VALID_TASK_STATUSES.to_vec())]
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

#[derive(Debug, Subcommand)]
pub enum ContractCommand {
    #[command(about = "Merge typed metadata and delivery contract fields into a run")]
    Set {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long = "done-when")]
        done_when: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        target: Vec<String>,
        #[arg(long)]
        constraint: Vec<String>,
        #[arg(
            long,
            value_name = "[LABEL::]CMD",
            conflicts_with = "replace_instrument",
            help = "Measurement command; use LABEL::CMD for a stable label, or CMD without ::"
        )]
        instrument: Vec<String>,
        #[arg(
            long = "replace-instrument",
            value_name = "[LABEL::]CMD",
            conflicts_with = "instrument",
            help = "Replace all instruments; use this to repair invalid legacy instrument values"
        )]
        replace_instrument: Vec<String>,
        #[arg(long = "entropy-check")]
        entropy_check: Vec<String>,
    },
}

#[derive(Debug, ClapArgs)]
pub struct RunnerCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    task_id: Option<String>,
    /// What kind of work this run records: test | build | eval | research | lint | custom.
    /// Defaults to `test`. Used for classification/telemetry, not execution.
    #[arg(long, default_value = "test")]
    kind: String,
    #[arg(long)]
    command: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Max seconds the runner may run before it is killed. Defaults to 300.
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long)]
    touch: Vec<String>,
    #[arg(long)]
    note: Option<String>,
    /// Stable delivery-contract instrument label (or generated sha256 reference).
    #[arg(long = "instrument-ref", requires = "command")]
    instrument_ref: Option<String>,
    /// Task status to record when the runner exits non-zero. Defaults to
    /// `blocked` (NOT `failed`) — a blocked task is a gate the host must clear,
    /// not a permanent failure. Pass `--status-on-fail failed` if you want a
    /// hard failure instead.
    #[arg(long, default_value = "blocked", value_parser = ["blocked", "failed"])]
    status_on_fail: String,
    /// Runner backend (default `codex`): codex/pi/agy/gemini/claude are headless
    /// delegate scripts; `tmux` is an interactive real-TUI session with
    /// completion detection. Prefer `--runner tmux` (or `lto dispatch-goal`)
    /// when dispatching an external agent so the host/user can watch it;
    /// headless runners are for shell evidence capture and tmux-unavailable/CI
    /// fallback. Validated against the known set (same as dispatch-goal).
    #[arg(
        long,
        default_value = "codex",
        value_parser = ["codex", "pi", "agy", "gemini", "claude", "tmux"]
    )]
    runner: String,
    /// Explicitly allow a write-capable non-tmux runner. Prefer dispatch-goal
    /// for development work so the TUI remains visible and observable.
    #[arg(long)]
    allow_headless_write: bool,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    job_file: Option<PathBuf>,
    #[arg(long)]
    job_id: Option<String>,
    #[arg(long)]
    target: Option<String>,
    /// tmux completion detection: `signal` (one-shot command → tmux wait-for,
    /// zero polling; best for `codex exec`-style commands), `sentinel` (agent
    /// touches a done-file the host polls; for interactive TUIs), or `fire`
    /// (fire-and-forget, no completion wait). Defaults to the runner's own mode.
    #[arg(long = "tmux-mode", value_parser = ["signal", "sentinel", "fire"])]
    tmux_mode: Option<String>,
    #[arg(long)]
    sentinel: Option<PathBuf>,
    #[arg(long = "tmux-session")]
    tmux_session: Option<String>,
    #[arg(long = "new-window")]
    new_window: bool,
    #[arg(long = "new-session")]
    new_session: bool,
    #[arg(long = "window-name")]
    window_name: Option<String>,
    #[arg(long = "ready-pattern")]
    ready_pattern: Vec<String>,
    #[arg(long = "skip-prompt")]
    skip_prompt: Vec<String>,
    #[arg(long = "ready-timeout")]
    ready_timeout: Option<u64>,
    #[arg(long = "tmux-bin")]
    tmux_bin: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct DispatchGoalCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long, value_parser = ["codex", "pi", "agy"])]
    runner: String,
    #[arg(long)]
    goal: PathBuf,
    #[arg(long)]
    target: Option<String>,
    #[arg(long = "new-window")]
    new_window: bool,
    #[arg(long = "window-name")]
    window_name: Option<String>,
    /// Preserve an LTO-created window after successful completion.
    #[arg(long = "keep-window")]
    keep_window: bool,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long = "tmux-session")]
    tmux_session: Option<String>,
    #[arg(long = "tmux-bin")]
    tmux_bin: Option<String>,
    #[arg(long = "ready-timeout")]
    ready_timeout: Option<u64>,
    /// Host notification command persisted on the run and executed when the
    /// dispatch completion fires. Untrusted summary text is exposed via $LTO_SUMMARY.
    #[arg(long = "notify-cmd")]
    notify_cmd: Option<String>,
    #[arg(long = "no-install-hooks")]
    no_install_hooks: bool,
    #[arg(long = "uninstall-hooks")]
    uninstall_hooks: bool,
    /// Skip per-runner behavioral-constraints injection into the dispatched
    /// goal (built-in codex block and ~/.config/lto/constraints/<runner>.md
    /// overrides; dir overridable via $LTO_CONSTRAINTS_DIR).
    #[arg(long = "no-runner-constraints")]
    no_runner_constraints: bool,
}

#[derive(Debug, ClapArgs)]
pub struct DispatchAndWaitCommand {
    #[command(flatten)]
    dispatch: DispatchGoalCommand,
    /// Max seconds to wait for the completion event after dispatch. Defaults to 600.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
}

#[derive(Debug, ClapArgs)]
pub struct AgentTurnCompletedCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long, default_value = "codex")]
    runner: String,
    #[arg(long = "payload-file")]
    payload_file: Option<PathBuf>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long = "session-id")]
    session_id: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    rc: Option<i32>,
    /// Immutable tmux window id inherited from dispatch-goal (for example @42).
    #[arg(long = "window-id")]
    window_id: Option<String>,
    #[arg(long, default_value = "hook")]
    source: String,
    /// Ring the terminal/tmux bell on completion (off by default).
    #[arg(long)]
    bell: bool,
    /// Host notification command run on completion, with {summary}/{rc}/{run_id}/{runner}
    /// placeholders, e.g. an iaf progress call. LTO does not hardcode any notifier.
    #[arg(long = "notify-cmd")]
    notify_cmd: Option<String>,
}

#[derive(Debug, ClapArgs)]
pub struct EventsCommand {
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long)]
    wait: bool,
    #[arg(long)]
    event_type: Option<String>,
    #[arg(long)]
    after: Option<u64>,
    #[arg(long, default_value_t = 300)]
    timeout: u64,
    #[arg(long)]
    json: bool,
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
            ledger,
            strict,
            to_phase,
            json,
        } => {
            if let Some(path) = ledger {
                match ledger_check::evaluate_path(&path, strict) {
                    Ok(report) => {
                        print!("{}", report.render());
                        let exit_code = report.exit_code();
                        if exit_code != 0 {
                            std::process::exit(exit_code);
                        }
                    }
                    Err(err) => {
                        eprintln!("ERROR {err:#}");
                        std::process::exit(2);
                    }
                }
            } else {
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
        }
        Commands::Budget {
            command: BudgetCommand::Check { run_id, tokens },
        } => {
            let path = state::state_path(&args.repo, &run_id);
            let state = state::load_state(&path)?;
            let now = state::iso_now();
            let check = budget::check_budget(Some(&state.budget), &state.started_at, tokens, &now);
            crate::event_emit::emit_budget_event(&args.repo, &run_id, &check, "budget_check");
            let _ = crate::telemetry::save(&args.repo, &run_id);
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
            let _ = crate::telemetry::save(&args.repo, &run_id);
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
                    let (size, phase) = crate::commands::prune::run_size_and_phase(&entry.path());
                    runs.push((entry.file_name().to_string_lossy().to_string(), size, phase));
                }
            }
            runs.sort_by(|a, b| a.0.cmp(&b.0));
            println!("# LTO runs in this project ({} total)", runs.len());
            for (run_id, size, phase) in runs {
                println!(
                    "{:<8} {:>9}  {run_id}",
                    phase,
                    crate::commands::prune::format_bytes(size)
                );
            }
        }
        Commands::Prune {
            dry_run,
            yes,
            older_than,
            keep_last,
            run_id,
        } => {
            crate::commands::prune::cmd_prune(
                &args.repo,
                crate::commands::prune::PruneOptions {
                    dry_run,
                    yes,
                    older_than_days: older_than,
                    keep_last,
                    run_id,
                },
            )?;
        }
        Commands::Audit {
            run_id,
            auto_dispatch,
            discover_risks,
            allow_same_family,
            prefer_runner,
        } => {
            cmd_audit(
                &args.repo,
                AuditOptions {
                    run_id,
                    auto_dispatch,
                    discover_risks,
                    allow_same_family,
                    prefer_runner,
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
            let goal = goal.unwrap_or_default();
            let why = why.unwrap_or_default();
            let done_when = done_when.unwrap_or_default();
            let host = host
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let delivery_contract =
                DeliveryContract::new(target, constraint, instrument, entropy_check);
            let readiness = state::assess_run_readiness(&goal, &done_when, &why, &host);
            if !readiness.is_ready() {
                anyhow::bail!(
                    "需补充: {}\n（信息不足禁猜：没有完成标准的 run 无法判收敛，recap/closeout 都会退化）",
                    format_flag_hints(&readiness.missing)
                );
            }
            let contract_assessment = delivery_contract.completeness_missing();
            if !contract_assessment.is_complete() {
                anyhow::bail!(
                    "需补充: {}\n（信息不足禁猜：目标与测量手段必须成对，目标必须有可验证的测量手段）",
                    format_flag_hints(&contract_assessment.missing)
                );
            }
            for flag in readiness.advisory {
                eprintln!("WARN 需补充: {}", flag_hint(flag));
            }
            for flag in contract_assessment.advisory {
                eprintln!("WARN delivery contract 可补充: {}", flag_hint(flag));
            }
            let run_dir = start_run(
                &args.repo,
                StartRunOptions {
                    run_id,
                    goal,
                    why,
                    done_when,
                    host,
                    delivery_contract,
                    force,
                },
            )?;
            println!("{}", run_dir.display());
        }
        Commands::Contract { command } => match command {
            ContractCommand::Set {
                run_id,
                goal,
                done_when,
                host,
                target,
                constraint,
                instrument,
                replace_instrument,
                entropy_check,
            } => {
                contract::cmd_contract_set(
                    &args.repo,
                    contract::ContractSetOptions {
                        run_id,
                        goal,
                        done_when,
                        host,
                        targets: target,
                        constraints: constraint,
                        instruments: instrument,
                        replacement_instruments: replace_instrument,
                        entropy_checks: entropy_check,
                    },
                )?;
            }
        },
        Commands::Recap {
            run_id,
            artifacts,
            mine,
        } => {
            recap::cmd_recap(
                &args.repo,
                recap::RecapOptions {
                    run_id,
                    artifacts,
                    mine,
                },
            )?;
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
        Commands::Preflight {
            run_id,
            record,
            json,
        } => {
            ops::cmd_preflight(
                &args.repo,
                ops::PreflightOptions {
                    run_id,
                    record,
                    json,
                },
            )?;
        }
        Commands::Runner(cmd) => {
            let cmd = *cmd;
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
                    instrument_ref: cmd.instrument_ref,
                    status_on_fail: cmd.status_on_fail,
                    runner: cmd.runner,
                    allow_headless_write: cmd.allow_headless_write,
                    prompt: cmd.prompt,
                    prompt_file: cmd.prompt_file,
                    job_file: cmd.job_file,
                    job_id: cmd.job_id,
                    tmux_target: cmd.target,
                    tmux_mode: cmd.tmux_mode,
                    tmux_sentinel: cmd.sentinel,
                    tmux_session: cmd.tmux_session,
                    tmux_new_window: cmd.new_window,
                    tmux_new_session: cmd.new_session,
                    tmux_window_name: cmd.window_name,
                    tmux_ready_patterns: cmd.ready_pattern,
                    tmux_skip_prompts: cmd.skip_prompt,
                    tmux_ready_timeout_sec: cmd.ready_timeout,
                    tmux_bin: cmd.tmux_bin,
                },
            )?;
        }
        Commands::DispatchGoal(cmd) => {
            crate::dispatch_goal::cmd_dispatch_goal(
                &args.repo,
                crate::dispatch_goal::DispatchGoalOptions {
                    run_id: cmd.run_id,
                    runner: cmd.runner,
                    goal: cmd.goal,
                    target: cmd.target,
                    new_window: cmd.new_window,
                    window_name: cmd.window_name,
                    keep_window: cmd.keep_window,
                    cwd: cmd.cwd,
                    tmux_session: cmd.tmux_session,
                    tmux_bin: cmd.tmux_bin,
                    ready_timeout_sec: cmd.ready_timeout,
                    notify_cmd: cmd.notify_cmd,
                    no_install_hooks: cmd.no_install_hooks,
                    uninstall_hooks: cmd.uninstall_hooks,
                    no_runner_constraints: cmd.no_runner_constraints,
                },
            )?;
        }
        Commands::DispatchAndWait(cmd) => {
            let d = cmd.dispatch;
            // Resolve the run id before dispatch so we can wait on it after.
            let run_id = d
                .run_id
                .clone()
                .or_else(|| current_run_id(&args.repo))
                .context("dispatch-and-wait requires --run-id or .lto/current")?;
            crate::dispatch_goal::cmd_dispatch_goal(
                &args.repo,
                crate::dispatch_goal::DispatchGoalOptions {
                    run_id: Some(run_id.clone()),
                    runner: d.runner,
                    goal: d.goal,
                    target: d.target,
                    new_window: d.new_window,
                    window_name: d.window_name,
                    keep_window: d.keep_window,
                    cwd: d.cwd,
                    tmux_session: d.tmux_session,
                    tmux_bin: d.tmux_bin,
                    ready_timeout_sec: d.ready_timeout,
                    notify_cmd: d.notify_cmd,
                    no_install_hooks: d.no_install_hooks,
                    uninstall_hooks: d.uninstall_hooks,
                    no_runner_constraints: d.no_runner_constraints,
                },
            )?;
            println!(
                "\nwaiting up to {}s for agent.dispatch.completed on run {run_id} ...",
                cmd.timeout
            );
            match crate::events::wait_for(
                &args.repo,
                &run_id,
                "agent.dispatch.completed",
                None,
                std::time::Duration::from_secs(cmd.timeout),
            )? {
                Some(event) => {
                    let summary = event
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(no summary)");
                    let runner = event
                        .get("fields")
                        .and_then(|f| f.get("runner"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let rc = event
                        .get("fields")
                        .and_then(|fields| fields.get("rc"))
                        .and_then(|value| value.as_i64());
                    if rc != Some(0) {
                        anyhow::bail!(
                            "dispatch completed without success (runner={runner}, rc={}); window retained for troubleshooting",
                            rc.map(|value| value.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        );
                    }
                    println!("DONE runner={runner} rc=0 summary={summary}");
                    println!(
                        "Register the reply as evidence with: lto collect-agent-run --run-id {run_id} --runner {runner} --reply <path>"
                    );
                }
                None => {
                    crate::dispatch_goal::retain_latest_dispatch_window(
                        &args.repo,
                        &run_id,
                        &format!("dispatch-and-wait timeout after {}s", cmd.timeout),
                    );
                    anyhow::bail!(
                        "TIMEOUT after {}s; the agent may still be running and its window was retained. Check with `lto events --wait --run-id {run_id}` or `tmux` directly.",
                        cmd.timeout
                    );
                }
            }
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
            worker_runner,
            target,
            tmux_bin,
            ready_timeout,
        } => {
            ops::cmd_autopilot(
                &args.repo,
                ops::AutopilotOptions {
                    run_id,
                    auto_exec,
                    autonomous,
                    timeout,
                    worker_runner,
                    tmux_target: target,
                    tmux_bin,
                    tmux_ready_timeout_sec: ready_timeout,
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
                instrument_ref,
            } => {
                ops::cmd_task_add(
                    &args.repo,
                    ops::TaskAddOptions {
                        run_id,
                        task_id,
                        title,
                        phase,
                        command,
                        instrument_ref,
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
            instrument_ref,
        } => {
            ops::cmd_task_add(
                &args.repo,
                ops::TaskAddOptions {
                    run_id,
                    task_id,
                    title,
                    phase,
                    command,
                    instrument_ref,
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
        Commands::AgentTurnCompleted(cmd) => {
            crate::agent_turn::cmd_agent_turn_completed(
                &args.repo,
                crate::agent_turn::AgentTurnOptions {
                    run_id: cmd.run_id,
                    runner: cmd.runner,
                    payload_file: cmd.payload_file,
                    cwd: cmd.cwd,
                    session_id: cmd.session_id,
                    summary: cmd.summary,
                    rc: cmd.rc,
                    window_id: cmd.window_id,
                    source: cmd.source,
                    bell: cmd.bell,
                    notify_cmd: cmd.notify_cmd,
                },
            )?;
        }
        Commands::Events(cmd) => {
            let run_id = cmd
                .run_id
                .or_else(|| current_run_id(&args.repo))
                .context("events requires --run-id or .lto/current")?;
            crate::events::cmd_events(
                &args.repo,
                &run_id,
                cmd.wait,
                cmd.event_type,
                cmd.after,
                cmd.timeout,
                cmd.json,
            )?;
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
    prefer_runner: Vec<String>,
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
    if host.eq_ignore_ascii_case("unknown") {
        eprintln!(
            "WARN host runtime is unknown; 需补充: lto contract set --run-id {run_id} --host \"<runtime>\"（unknown 不做同族排除）"
        );
    }
    let auditors = audit_dispatch::pick_auditors_preferred(
        &host,
        options.allow_same_family,
        &options.prefer_runner,
    );
    let audit_dir = run_dir.join("audit");
    fs::create_dir_all(&audit_dir)?;
    crate::event_emit::emit_audit_dispatched(repo, &run_id, &host, &auditors, "prepare", None);

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
    crate::event_emit::emit_audit_dispatched(
        repo,
        &run_id,
        &host,
        &auditors,
        "auto_dispatch",
        None,
    );
    let results = audit_dispatch::submit_auto_dispatch(
        repo,
        &runners_dir,
        &brief_path,
        &auditors,
        &host,
        &run_id,
    )?;
    crate::event_emit::emit_runner_results_checked(
        repo,
        &run_id,
        Some(state.current_phase.as_str()),
        None,
        "audit.auto_dispatch",
        &results,
    )?;
    let mut run_ctx = util::load_run(repo, Some(&run_id))?;
    util::append_agent_results_to_state(&mut run_ctx.state, None, &results)?;
    util::save_run(&mut run_ctx)?;
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
        if result.status == crate::agent_job::JobStatus::Ok {
            used.push(result.runner.clone());
            counts.add_reply(&result.reply_text);
            let findings = parse_findings_or_empty(&result.reply_text);
            crate::event_emit::emit_audit_findings(
                repo,
                &run_id,
                &result.runner,
                &findings,
                "audit.auto_dispatch",
            );
        }
        println!(
            "  {}: {} exit={:?}",
            result.runner,
            result.status.as_str(),
            result.exit_code
        );
    }
    used.sort();
    used.dedup();
    let coverage = targets
        .iter()
        .filter_map(|target| target.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(",");
    let ledger_path = run_dir.join("audit-ledger.md");
    append_audit_ledger_round(
        repo,
        &run_id,
        &ledger_path,
        AuditLedgerRoundInput {
            state: &state,
            replies_dir: &replies_dir,
            auditors: &used,
            coverage: &coverage,
            counts,
        },
    )?;
    println!(
        "audit ledger: {}",
        util::repo_relative_path(repo, &ledger_path)?
    );
    let _ = crate::telemetry::save(repo, &run_id);
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
        crate::event_emit::emit_audit_dispatched(
            repo,
            run_id,
            host,
            auditors,
            "risk_discovery_unhealthy",
            None,
        );
        anyhow::bail!("risk discovery has no healthy heterogeneous discoverer for host {host}");
    };
    crate::event_emit::emit_audit_dispatched(
        repo,
        run_id,
        host,
        auditors,
        "risk_discovery",
        Some(&discoverer),
    );
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
    let mut job = audit_dispatch::build_risk_discovery_job(&brief_path, &discoverer, host);
    job.meta.insert("run_id".to_string(), json!(run_id));
    // Session reuse (backlog ⑪ 治本): same stable per-(run, auditor) session id as
    // auto-dispatch, so a discoverer reused across rounds warms the prompt cache.
    job.env.insert(
        "LTO_SESSION_ID".to_string(),
        audit_dispatch::audit_session_id(run_id, &discoverer),
    );
    let jobs = vec![job];
    crate::event_emit::emit_runner_started_jobs(
        repo,
        run_id,
        Some(state.current_phase.as_str()),
        None,
        "audit.risk_discovery",
        &jobs,
    );
    let results =
        match crate::scheduler::Scheduler::new(repo, runners_dir).submit_blocking(jobs.clone()) {
            Ok(results) => results,
            Err(err) => {
                crate::event_emit::emit_runner_submission_failed_jobs(
                    repo,
                    run_id,
                    Some(state.current_phase.as_str()),
                    None,
                    "audit.risk_discovery",
                    &jobs,
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
    let Some(result) = results.first() else {
        anyhow::bail!("risk discovery returned no result");
    };
    crate::event_emit::emit_runner_results_checked(
        repo,
        run_id,
        Some(state.current_phase.as_str()),
        None,
        "audit.risk_discovery",
        &results,
    )?;
    let mut run_ctx = util::load_run(repo, Some(run_id))?;
    util::append_agent_results_to_state(&mut run_ctx.state, None, &results)?;
    util::save_run(&mut run_ctx)?;
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
    crate::event_emit::emit_audit_findings(
        repo,
        run_id,
        &result.runner,
        &risks,
        "audit.risk_discovery",
    );
    if risks.is_empty() {
        println!("risk discovery: {discoverer} reported no structured risks");
        let _ = crate::telemetry::save(repo, run_id);
        return Ok(());
    }
    let state_path = run_dir.join("state.json");
    let mut state = state::load_state(&state_path)?;
    append_discovered_risk_points(&mut state, risks);
    util::save_state_preserving_c2(&state_path, run_id, &mut state)?;
    util::sync_run_state_md(&run_dir.join("run-state.md"), &state)?;
    let _ = crate::telemetry::save(repo, run_id);
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
        let mut risk_point = json!({
            "id": format!("RP-auto-{next}"),
            "source": "risk-agent",
            "claim": claim,
            "evidence_to_check": risk.get("evidence_to_check").and_then(Value::as_str).unwrap_or(""),
            "severity": risk.get("severity").and_then(Value::as_str).unwrap_or("medium"),
            "status": "open",
            "disposition": "open",
            "recorded_at": util::iso_now(),
        });
        if let Some(reported_confidence) = risk
            .get("reported_confidence")
            .filter(|value| !value.is_null())
        {
            risk_point["reported_confidence"] = reported_confidence.clone();
        }
        if let Some(invalidated_when) = risk
            .get("invalidated_when")
            .filter(|value| !value.is_null())
        {
            risk_point["invalidated_when"] = invalidated_when.clone();
        }
        risk_points.push(risk_point);
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
        let Some(findings) = parse_structured_findings(text) else {
            self.high += text.matches("high").count() as u64;
            self.critical += text.matches("critical").count() as u64;
            return;
        };
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

struct AuditLedgerRoundInput<'a> {
    state: &'a LtoState,
    replies_dir: &'a Path,
    auditors: &'a [String],
    coverage: &'a str,
    counts: SeverityCounts,
}

fn parse_findings_or_empty(text: &str) -> Vec<Value> {
    parse_structured_findings(text).unwrap_or_default()
}

fn parse_structured_findings(text: &str) -> Option<Vec<Value>> {
    if let Some(findings) = crate::audit::parse_findings_text(text) {
        return Some(
            findings
                .into_iter()
                .filter_map(|finding| serde_json::to_value(finding).ok())
                .collect(),
        );
    }
    let raw = parse_json_findings(text)?;
    if raw.is_empty() {
        return Some(Vec::new());
    }
    let findings = crate::audit::parse_valid_findings_values(&raw);
    if findings.is_empty() {
        return None;
    }
    Some(
        findings
            .into_iter()
            .filter_map(|finding| serde_json::to_value(finding).ok())
            .collect(),
    )
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
    input: AuditLedgerRoundInput<'_>,
) -> anyhow::Result<()> {
    let AuditLedgerRoundInput {
        state,
        replies_dir,
        auditors,
        coverage,
        counts,
    } = input;
    let artifact = util::repo_relative_path(repo, replies_dir)?;
    let outcome = audit_ledger::append(
        ledger_path,
        audit_ledger::AppendInput {
            artifact: &artifact,
            auditors,
            coverage,
            high: counts.high,
            critical: counts.critical,
            minor: counts.minor,
        },
    )?;
    crate::event_emit::emit_audit_round_recorded(
        repo,
        run_id,
        &outcome.label,
        counts.high,
        counts.critical,
        counts.minor,
    );
    crate::event_emit::emit_audit_ledger_evaluated(
        repo,
        run_id,
        &outcome.label,
        outcome.verdict.as_str(),
        outcome.diagnostics.terminal.as_str(),
        outcome.diagnostics.oscillation.as_str(),
    );
    register_run_artifact(
        repo,
        run_id,
        ledger_path,
        state,
        RunArtifactRecord {
            kind: "audit_ledger",
            producer: "lto-rs.audit.collect",
            summary: &format!("audit ledger updated {}", outcome.label),
            tags: &["audit", "ledger"],
        },
    )?;
    Ok(())
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
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
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
        "For each finding, report your self-assessed confidence and the evidence that would invalidate the claim. reported_confidence is uncalibrated metadata, not severity or probability.".to_string(),
        String::new(),
        "```json".to_string(),
        r#"[{"severity":"high","claim":"...","reported_confidence":{"level":"high","rationale":"..."},"invalidated_when":"...","evidence_to_check":"...","file":"..."}]"#.to_string(),
        "```".to_string(),
    ]);
    lines.join("\n")
}

fn build_risk_brief(state: &LtoState, host: &str) -> String {
    format!(
        "# LTO Risk Discovery Brief\n\n- goal: {}\n- host_runtime: {}\n- phase: {}\n\nRead current state and recent changed files. Return only new high/critical/medium risks as JSON findings. For each finding, include reported_confidence with level/rationale and invalidated_when; confidence is uncalibrated review metadata, not severity or probability. Return [] if no new risks.\n",
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

fn format_flag_hints(flags: &[&str]) -> String {
    flags
        .iter()
        .map(|flag| flag_hint(flag))
        .collect::<Vec<_>>()
        .join(" ")
}

fn flag_hint(flag: &str) -> String {
    match flag {
        "--goal" => "--goal \"<一句话目标>\"".to_string(),
        "--done-when" => "--done-when \"<怎么算做完>\"".to_string(),
        "--why" => "--why \"<为什么要做>\"".to_string(),
        "--host" => "--host \"<当前 host runtime>\"（当前按 unknown 记录）".to_string(),
        "--target" => "--target \"<可验证目标>\"".to_string(),
        "--constraint" => "--constraint \"<交付约束>\"".to_string(),
        "--instrument" => "--instrument \"<测量命令>\"".to_string(),
        "--entropy-check" => "--entropy-check \"<停滞时的换假设检查>\"".to_string(),
        _ => flag.to_string(),
    }
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
        assert_eq!(COMMANDS.len(), 27);
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
    fn audit_empty_findings_do_not_fall_back_to_prose_severity_words() {
        let mut counts = SeverityCounts::default();
        counts.add_reply("```json\n[]\n```\nThe `critical` column precedes `high`.");
        assert_eq!(counts.high, 0);
        assert_eq!(counts.critical, 0);
        assert_eq!(counts.minor, 0);
    }

    #[test]
    fn structured_findings_fallback_keeps_only_typed_normalized_items() {
        let findings = parse_structured_findings(
            r#"[{"severity":"high","claim":"valid","reported_confidence":"High"},{"severity":"INVALID","claim":"bad"}]"#,
        )
        .unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["claim"], "valid");
        assert_eq!(findings[0]["reported_confidence"]["level"], "high");
    }

    #[test]
    fn audit_unstructured_legacy_reply_keeps_severity_word_fallback() {
        let mut counts = SeverityCounts::default();
        counts.add_reply("high issue and critical issue");
        assert_eq!(counts.high, 1);
        assert_eq!(counts.critical, 1);
        assert_eq!(counts.minor, 0);
    }

    #[test]
    fn collect_agent_run_status_values_are_registered() {
        Args::try_parse_from([
            "lto-rs",
            "collect-agent-run",
            "--task-id",
            "T1",
            "--runner",
            "codex",
            "--reply",
            "reply.md",
            "--status",
            "returned",
        ])
        .unwrap();
        let err = Args::try_parse_from([
            "lto-rs",
            "collect-agent-run",
            "--task-id",
            "T1",
            "--runner",
            "codex",
            "--reply",
            "reply.md",
            "--status",
            "returnedd",
        ])
        .unwrap_err()
        .to_string();
        assert!(err.contains("possible values"));
        assert!(err.contains("rate_limited"));
        assert!(err.contains("returned"));
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
        Args::try_parse_from(["lto-rs", "check", "--ledger", "ledger.md", "--strict"]).unwrap();
        assert!(Args::try_parse_from(["lto-rs", "check", "--to", "deploy"]).is_err());
        for conflict in ["--run-id", "--to", "--json"] {
            let mut argv = vec!["lto-rs", "check", "--ledger", "ledger.md", conflict];
            if conflict != "--json" {
                argv.push(if conflict == "--run-id" {
                    "r1"
                } else {
                    "closed"
                });
            }
            assert!(
                Args::try_parse_from(argv).is_err(),
                "{conflict} must conflict with --ledger"
            );
        }
    }

    #[test]
    fn preflight_json_flag_is_registered() {
        Args::try_parse_from(["lto-rs", "preflight", "--json"]).unwrap();
        Args::try_parse_from([
            "lto-rs",
            "preflight",
            "--run-id",
            "r1",
            "--record",
            "--json",
        ])
        .unwrap();
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
    fn contract_set_flags_are_registered() {
        Args::try_parse_from([
            "lto-rs",
            "contract",
            "set",
            "--run-id",
            "r1",
            "--goal",
            "ship",
            "--done-when",
            "tests pass",
            "--host",
            "codex",
            "--target",
            "first",
            "--target",
            "second",
            "--constraint",
            "bounded",
            "--instrument",
            "smoke::cargo test",
            "--entropy-check",
            "change hypothesis",
        ])
        .unwrap();
        Args::try_parse_from([
            "lto-rs",
            "contract",
            "set",
            "--replace-instrument",
            "smoke::cargo test",
        ])
        .unwrap();
        assert!(
            Args::try_parse_from([
                "lto-rs",
                "contract",
                "set",
                "--instrument",
                "true",
                "--replace-instrument",
                "smoke::true",
            ])
            .is_err()
        );
    }

    #[test]
    fn start_rejects_missing_readiness_before_writing() {
        let cases = [
            (
                vec!["--goal", "ship"],
                vec!["--done-when"],
                "missing done-when",
            ),
            (
                vec!["--done-when", "tests pass"],
                vec!["--goal"],
                "missing goal",
            ),
            (
                vec!["--force"],
                vec!["--goal", "--done-when"],
                "force does not bypass readiness",
            ),
        ];
        for (suffix, missing, label) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path();
            let mut argv = vec!["lto-rs", "--repo", repo.to_str().unwrap(), "start"];
            argv.extend(suffix);
            let err = run_args(Args::try_parse_from(argv).unwrap()).unwrap_err();
            let message = err.to_string();
            for flag in missing {
                assert!(message.contains(flag), "{label}: {message}");
            }
            assert!(!repo.join(".lto").exists(), "{label} wrote .lto");
        }
    }

    #[test]
    fn start_rejects_unpaired_contract_before_writing() {
        for (flag, value, missing) in [
            ("--target", "ship", "--instrument"),
            ("--instrument", "cargo test", "--target"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path();
            let args = Args::try_parse_from([
                "lto-rs",
                "--repo",
                repo.to_str().unwrap(),
                "start",
                "--goal",
                "ship",
                "--done-when",
                "tests pass",
                flag,
                value,
            ])
            .unwrap();
            let err = run_args(args).unwrap_err();
            assert!(err.to_string().contains(missing), "{err:#}");
            assert!(!repo.join(".lto").exists());
        }
    }

    #[test]
    fn start_defaults_missing_host_to_unknown_for_an_empty_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        crate::process::git(repo, ["init"]).unwrap();
        let args = Args::try_parse_from([
            "lto-rs",
            "--repo",
            repo.to_str().unwrap(),
            "start",
            "--run-id",
            "unknown-host",
            "--goal",
            "ship",
            "--done-when",
            "tests pass",
        ])
        .unwrap();
        run_args(args).unwrap();

        let state = state::load_state(repo.join(".lto/unknown-host/state.json")).unwrap();
        assert_eq!(state.host_runtime, "unknown");
        assert!(state.delivery_contract.is_empty());
    }

    #[test]
    fn effective_audit_host_preserves_recorded_unknown() {
        let state = LtoState {
            host_runtime: "unknown".to_string(),
            ..LtoState::default()
        };
        assert_eq!(effective_audit_host(&state), "unknown");
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
                    "severity": "critical",
                    "reported_confidence": {
                        "level": "high",
                        "rationale": "source inspection"
                    },
                    "invalidated_when": "gate reads a different source"
                }),
                json!({"claim": "   "}),
            ],
        );
        assert_eq!(added, 1);
        let risks = state.risk_points.as_array().unwrap();
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0]["status"], "open");
        assert_eq!(risks[0]["disposition"], "open");
        assert_eq!(risks[0]["reported_confidence"]["level"], "high");
        assert_eq!(
            risks[0]["invalidated_when"],
            "gate reads a different source"
        );
        assert!(risks[0].get("reported_confidence").is_some());
        assert!(risks[0].get("invalidated_when").is_some());
    }

    #[test]
    fn finding_metadata_isolation_keeps_audit_gate_counts() {
        let mut baseline = SeverityCounts::default();
        baseline
            .add_reply(r#"[{"severity":"high","claim":"A"},{"severity":"medium","claim":"B"}]"#);
        let mut enriched = SeverityCounts::default();
        enriched.add_reply(
            r#"[{"severity":"high","claim":"A","reported_confidence":{"level":"low","rationale":"uncertain"},"invalidated_when":"counterexample"},{"severity":"medium","claim":"B","reported_confidence":"high","invalidated_when":"source changes"}]"#,
        );
        assert_eq!(
            (baseline.high, baseline.critical, baseline.minor),
            (enriched.high, enriched.critical, enriched.minor)
        );
    }

    #[test]
    fn commands_markdown_tracks_rust_command_contract() {
        let doc =
            std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("COMMANDS.md"))
                .unwrap();
        // Derived from COMMANDS so adding a command does not require editing two
        // hardcoded numbers here and in COMMANDS.md independently.
        let business = COMMANDS.len();
        assert!(doc.contains(&format!("Command count: {}.", business + 1)));
        assert!(doc.contains(&format!("{business} Rust-owned business")));
        assert!(doc.contains("clap built-in `help`"));
        for command in COMMANDS {
            assert!(
                doc.contains(&format!("| `{command}`")),
                "COMMANDS.md missing {command}"
            );
        }
    }
}
