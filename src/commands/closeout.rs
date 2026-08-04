use crate::commands::util;
use crate::ledger::{self, LedgerVerdict};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const HUMAN_GATE_SELF_CHECK_REMINDER: &str = "亲验自查：你的亲验脚本本身验过吗（ADDED vs MODIFIED？target 是否 active？write 是否确认成功？）";
const NO_CHANGELOG_HINT: &str =
    "use --no-changelog after commit for admin closeout without new tracked dirt";

#[derive(Debug, Clone)]
pub struct CloseoutOptions {
    pub run_id: Option<String>,
    pub summary: String,
    pub next_action: String,
    pub blocked_by: String,
    pub allow_dirty: bool,
    pub no_changelog: bool,
    pub force: bool,
    pub reverify_timeout: u64,
    pub no_reverify: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ReverifyResult {
    attempted: usize,
    passed: usize,
    failed_labels: Vec<String>,
}

pub fn cmd_closeout(repo: &Path, options: CloseoutOptions) -> anyhow::Result<()> {
    let mut ctx = util::load_run(repo, options.run_id.as_deref())?;
    let run_state_path = ctx.run_dir.join("run-state.md");
    if !run_state_path.exists() {
        anyhow::bail!("missing run-state.md: {}", run_state_path.display());
    }

    println!("human gate summary: host verification remains required");
    println!("{HUMAN_GATE_SELF_CHECK_REMINDER}");

    let reverify = enforce_gates(repo, &ctx, &options)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "gate.evaluated".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some("closeout".to_string()),
            object_type: Some("gate".to_string()),
            summary: "closeout gates passed".to_string(),
            fields: json!({
                "gate": "closeout",
                "status": "passed",
                "reverified_instruments": reverify.attempted,
                "passed_instruments": reverify.passed,
                "failed_instrument_labels": reverify.failed_labels,
            }),
            ..crate::events::EventRecord::default()
        },
    );

    let git = util::git_status(repo);
    let previous_phase = ctx.state.current_phase.clone();
    if previous_phase != "closed" {
        util::append_phase_transition(&mut ctx.state, &previous_phase, "closed", &git.head);
        crate::events::safe_emit(
            repo,
            &ctx.run_id,
            crate::events::EventRecord {
                event_type: "phase.changed".to_string(),
                actor_kind: "host".to_string(),
                phase: Some("closed".to_string()),
                summary: format!("{previous_phase} -> closed"),
                fields: json!({"from": previous_phase, "to": "closed"}),
                ..crate::events::EventRecord::default()
            },
        );
    } else {
        ctx.state.current_phase = "closed".to_string();
    }
    ctx.state.workspace.head = git.head.clone();
    ctx.state.workspace.branch = git.branch.clone();
    ctx.state.workspace.dirty_fingerprint = if git.dirty { "dirty" } else { "clean" }.to_string();
    ctx.state.blocked_by = json!(options.blocked_by);
    ctx.state.next_action = json!(options.next_action);
    util::save_run(&mut ctx)?;
    crate::events::safe_emit(
        repo,
        &ctx.run_id,
        crate::events::EventRecord {
            event_type: "run.closed".to_string(),
            actor_kind: "host".to_string(),
            phase: Some(ctx.state.current_phase.clone()),
            summary: options.summary.clone(),
            fields: json!({
                "next_action": options.next_action,
                "blocked_by": options.blocked_by,
            }),
            ..crate::events::EventRecord::default()
        },
    );

    write_closeout_section(&run_state_path, &options)?;
    util::register_artifact(
        repo,
        &ctx.run_id,
        &ctx.state_path,
        util::ArtifactMeta {
            kind: "state_json",
            producer: "lto_rs.commands.closeout",
            state: &ctx.state,
            summary: "machine state at closeout",
            tags: &["state"],
        },
    )?;
    util::register_artifact(
        repo,
        &ctx.run_id,
        &run_state_path,
        util::ArtifactMeta {
            kind: "run_state_md",
            producer: "lto_rs.commands.closeout",
            state: &ctx.state,
            summary: "human-readable state at closeout",
            tags: &["state"],
        },
    )?;

    if !options.no_changelog {
        write_changelog(repo, &ctx, &options)?;
        util::register_artifact(
            repo,
            &ctx.run_id,
            &repo.join("CHANGELOG.md"),
            util::ArtifactMeta {
                kind: "changelog",
                producer: "lto_rs.commands.closeout",
                state: &ctx.state,
                summary: "repo changelog updated",
                tags: &["closeout", "changelog"],
            },
        )?;
    }

    let artifacts = util::latest_artifacts(repo, &ctx.run_id, usize::MAX);
    let handoff = build_handoff(&ctx, &options, &git, &artifacts);
    let handoff_path = ctx.run_dir.join("handoff.md");
    fs::write(&handoff_path, handoff)?;
    util::register_artifact(
        repo,
        &ctx.run_id,
        &handoff_path,
        util::ArtifactMeta {
            kind: "handoff",
            producer: "lto_rs.commands.closeout",
            state: &ctx.state,
            summary: "closeout handoff",
            tags: &["closeout", "handoff"],
        },
    )?;
    let artifacts = util::latest_artifacts(repo, &ctx.run_id, usize::MAX);
    fs::write(
        &handoff_path,
        build_handoff(&ctx, &options, &git, &artifacts),
    )?;
    let _ = crate::telemetry::save(repo, &ctx.run_id);

    println!("{}", handoff_path.display());
    println!("interventions: none recorded by rust closeout");
    // Advisory: nudge the host to reclaim disk if .lto has grown large.
    crate::commands::prune::maybe_nudge_prune(repo);
    Ok(())
}

fn enforce_gates(
    repo: &Path,
    ctx: &util::RunContext,
    options: &CloseoutOptions,
) -> anyhow::Result<ReverifyResult> {
    let dirty = util::tracked_dirty_paths(repo);
    if !dirty.is_empty() && !options.allow_dirty {
        let sample = dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        eprintln!(
            "WARN closeout: {} tracked uncommitted change(s) detected (e.g. {}). {NO_CHANGELOG_HINT}; closeout will still refuse until code changes are committed or stashed.",
            dirty.len(),
            sample
        );
    }

    if !options.force {
        let readiness = crate::state::assess_run_readiness(
            &ctx.state.goal,
            &ctx.state.done_when,
            &ctx.state.why,
            &ctx.state.host_runtime,
        );
        if !readiness.is_ready() {
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "run readiness missing",
                json!({"missing": &readiness.missing}),
            );
            anyhow::bail!(
                "closeout refused: run readiness missing {} (use --force to override)",
                readiness.missing.join(", ")
            );
        }
        let contract = ctx.state.delivery_contract.completeness_missing();
        if !contract.is_complete() {
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "delivery contract incomplete",
                json!({"missing": &contract.missing}),
            );
            anyhow::bail!(
                "closeout refused: delivery contract incomplete: {} (use --force to override)",
                contract.missing.join(", ")
            );
        }
    }

    let ledger_path = ctx.run_dir.join("audit-ledger.md");
    if ledger_path.exists() && !options.force {
        let text = fs::read_to_string(&ledger_path)?;
        let rounds = ledger::parse_ledger(&text)?;
        let verdict = ledger::evaluate_ledger(&rounds, false);
        if let Some(diagnostics) = ledger::diagnose(&rounds) {
            eprintln!("ledger diagnostics: {}", diagnostics.summary());
            if diagnostics.suggests_entropy_review()
                && !ctx.state.delivery_contract.forced_entropy.is_empty()
            {
                eprintln!(
                    "ADVISORY review forced_entropy before changing hypothesis: {}",
                    ctx.state.delivery_contract.forced_entropy.join(" | ")
                );
            }
        }
        if !matches!(
            verdict,
            LedgerVerdict::Converged | LedgerVerdict::NoObservations
        ) {
            let sequence = ledger::ledger_sequence(&rounds);
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "audit ledger not converged",
                json!({"ledger_verdict": verdict.as_str(), "ledger_sequence": sequence}),
            );
            anyhow::bail!(
                "closeout refused: ledger verdict is {}, not CONVERGED ({}) (use --force to override)",
                verdict.as_str(),
                sequence
            );
        }
    }

    let unresolved = ctx
        .state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if unresolved > 0 && !options.force {
        emit_closeout_gate_blocked(
            repo,
            &ctx.run_id,
            "unresolved blocks",
            json!({"unresolved_blocks": unresolved}),
        );
        anyhow::bail!("closeout refused: {unresolved} unresolved blocks (use --force)");
    }

    if ctx.state.current_phase == "closed" && !options.force {
        emit_closeout_gate_blocked(
            repo,
            &ctx.run_id,
            "run already closed",
            json!({"current_phase": ctx.state.current_phase}),
        );
        anyhow::bail!("run already closed (use --force to rewrite)");
    }

    let mut reverify = ReverifyResult::default();
    if !options.force && !options.no_reverify && !ctx.state.delivery_contract.instruments.is_empty()
    {
        for instrument in &ctx.state.delivery_contract.instruments {
            let (label, command) = crate::state::split_instrument(instrument);
            let display = label.unwrap_or(command).to_string();
            reverify.attempted += 1;
            let (rc, stdout, stderr, _) =
                util::run_command_capture(repo, command, None, options.reverify_timeout)
                    .unwrap_or_else(|error| (1, String::new(), error.to_string(), 0.0));
            if rc == 0 {
                reverify.passed += 1;
                continue;
            }

            let stdout_tail = tail_lines(&stdout, 8);
            let stderr_tail = tail_lines(&stderr, 8);
            let mut failure_detail = format!("closeout reverify failed: {display} (rc={rc})");
            if !stdout_tail.is_empty() {
                failure_detail.push_str(&format!("\nstdout tail:\n{stdout_tail}"));
            }
            if !stderr_tail.is_empty() {
                failure_detail.push_str(&format!("\nstderr tail:\n{stderr_tail}"));
            }
            let refusal = format!(
                "closeout refused: delivery contract instruments failed reverify: {display} (use --force to override)\n{failure_detail}"
            );
            reverify.failed_labels.push(display);
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "delivery contract instruments failed reverify",
                json!({
                    "reverified_instruments": reverify.attempted,
                    "passed_instruments": reverify.passed,
                    "failed_instrument_labels": &reverify.failed_labels,
                }),
            );
            anyhow::bail!(refusal);
        }
        println!(
            "closeout reverify: {}/{} instruments passed",
            reverify.passed, reverify.attempted
        );
    }

    if !dirty.is_empty() && !options.allow_dirty {
        let sample = dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        emit_closeout_gate_blocked(
            repo,
            &ctx.run_id,
            "tracked worktree dirty",
            json!({"tracked_dirty_count": dirty.len(), "sample": sample}),
        );
        anyhow::bail!(
            "closeout refused: {} tracked uncommitted change(s) outside .lto (e.g. {}). Commit or stash code changes first; {NO_CHANGELOG_HINT}.",
            dirty.len(),
            sample
        );
    }
    let untracked = util::untracked_paths(repo);
    if !untracked.is_empty() && !options.allow_dirty {
        let sample = untracked
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "warning: closeout: {} untracked file(s) present (e.g. {}) -- not blocking",
            untracked.len(),
            sample
        );
    }

    let unverified = util::json_array(&ctx.state.risk_points)
        .iter()
        .filter(|risk| util::risk_is_open_unverified(risk))
        .count();
    if unverified > 0 && !options.force {
        emit_closeout_gate_blocked(
            repo,
            &ctx.run_id,
            "risk points unverified",
            json!({"unverified_risk_points": unverified}),
        );
        anyhow::bail!(
            "closeout refused: {unverified} risk points unverified (use --force to override)"
        );
    }

    if has_high_risk_task(&ctx.state.tasks) && !options.force {
        if !ledger_path.exists() {
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "missing audit ledger",
                json!({"audit_ledger": "missing"}),
            );
            anyhow::bail!(
                "closeout refused: high-risk run has no audit-ledger.md (run lto audit first, or use --force to override)"
            );
        }
        let text = fs::read_to_string(&ledger_path)?;
        if !ledger::has_real_ledger_rounds(&text) {
            emit_closeout_gate_blocked(
                repo,
                &ctx.run_id,
                "empty audit ledger",
                json!({"audit_ledger": "empty"}),
            );
            anyhow::bail!(
                "closeout refused: high-risk run has empty audit ledger (run lto audit first, or use --force to override)"
            );
        }
    }
    Ok(reverify)
}

fn write_closeout_section(path: &Path, options: &CloseoutOptions) -> anyhow::Result<()> {
    let content = fs::read_to_string(path)?;
    let head = content
        .split("\n## Closeout\n")
        .next()
        .unwrap_or(content.as_str())
        .trim_end();
    let closeout = format!(
        "\n\n## Closeout\n\n- closed_at: {}\n- summary: {}\n- next_action: {}\n",
        util::iso_now(),
        util::single_line(&options.summary),
        util::single_line(&options.next_action)
    );
    fs::write(path, format!("{head}{closeout}"))?;
    Ok(())
}

fn build_handoff(
    ctx: &util::RunContext,
    options: &CloseoutOptions,
    git: &util::GitStatus,
    artifacts: &[Value],
) -> String {
    let mut out = vec![
        "# LTO Handoff".to_string(),
        String::new(),
        format!("- run_id: {}", ctx.run_id),
        format!("- goal: {}", ctx.state.goal),
        "- status: closed".to_string(),
        format!("- closed_at: {}", util::iso_now()),
        format!("- git_head: {}", git.head),
        format!("- branch: {}", git.branch),
        format!("- blocked_by: {}", util::single_line(&options.blocked_by)),
        format!("- summary: {}", util::single_line(&options.summary)),
        format!("- next_action: {}", util::single_line(&options.next_action)),
        "- intervention_summary: none recorded by rust closeout".to_string(),
        format!("- token_usage: {}", token_usage_line(&ctx.state)),
        String::new(),
        "## Artifacts".to_string(),
        String::new(),
    ];
    if artifacts.is_empty() {
        out.push("- none".to_string());
    } else {
        let mut sorted = artifacts.to_vec();
        sorted.sort_by(|a, b| {
            let ak = a.get("kind").and_then(Value::as_str).unwrap_or("");
            let bk = b.get("kind").and_then(Value::as_str).unwrap_or("");
            let ap = a.get("relative_path").and_then(Value::as_str).unwrap_or("");
            let bp = b.get("relative_path").and_then(Value::as_str).unwrap_or("");
            (ak, ap).cmp(&(bk, bp))
        });
        for entry in sorted {
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("other");
            let path = entry
                .get("relative_path")
                .and_then(Value::as_str)
                .or_else(|| entry.get("run_relative_path").and_then(Value::as_str))
                .unwrap_or("?");
            let summary = entry.get("summary").and_then(Value::as_str).unwrap_or("");
            if summary.is_empty() {
                out.push(format!("- `{kind}`: `{path}`"));
            } else {
                out.push(format!("- `{kind}`: `{path}` -- {summary}"));
            }
        }
    }
    out.push(String::new());
    out.join("\n")
}

fn token_usage_line(state: &crate::state::LtoState) -> String {
    let rollup = util::token_rollup(state);
    if rollup.runs_total == 0 {
        return "no agent runs".to_string();
    }
    let elapsed = if rollup.total_elapsed_sec > 0.0 {
        format!(", {:.0}s total", rollup.total_elapsed_sec)
    } else {
        String::new()
    };
    if rollup.total_tokens == 0 {
        return format!(
            "unmetered ({} runs, no runner reported tokens{})",
            rollup.runs_total, elapsed
        );
    }
    let mut parts = rollup
        .by_runner
        .iter()
        .filter(|(_, slot)| slot.tokens > 0)
        .map(|(runner, slot)| (runner.clone(), slot.tokens))
        .collect::<Vec<_>>();
    parts.sort_by_key(|part| std::cmp::Reverse(part.1));
    let by = parts
        .iter()
        .map(|(runner, tokens)| format!("{runner}={tokens}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} total (in={}, out={}; {}/{} runs metered{}; {})",
        rollup.total_tokens,
        rollup.tokens_in,
        rollup.tokens_out,
        rollup.runs_with_tokens,
        rollup.runs_total,
        elapsed,
        by
    )
}

fn write_changelog(
    repo: &Path,
    ctx: &util::RunContext,
    options: &CloseoutOptions,
) -> anyhow::Result<()> {
    let path = repo.join("CHANGELOG.md");
    let mut lines = vec![
        format!("## {}", fallback(&ctx.state.goal, "unknown")),
        String::new(),
        format!("- **Run ID**: `{}`", ctx.run_id),
        format!("- **Closed**: {}", util::iso_now()),
        format!("- **Summary**: {}", util::single_line(&options.summary)),
        String::new(),
    ];
    let tasks = util::json_array(&ctx.state.tasks);
    if !tasks.is_empty() {
        lines.push("### Tasks".to_string());
        lines.push(String::new());
        for task in tasks {
            let id = task.get("id").and_then(Value::as_str).unwrap_or("?");
            let title = task.get("title").and_then(Value::as_str).unwrap_or(id);
            let status = task
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            lines.push(format!("- **{id}**: {title} ({status})"));
            for evidence in task
                .get("evidence")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let kind = evidence.get("kind").and_then(Value::as_str).unwrap_or("?");
                let summary = evidence
                    .get("summary")
                    .and_then(Value::as_str)
                    .or_else(|| evidence.get("command").and_then(Value::as_str))
                    .unwrap_or("");
                // Evidence without an rc (manual notes, collected agent runs)
                // carries no pass/fail verdict — marking it FAIL misreported
                // every successful manual registration in the changelog.
                let marker = match evidence.get("rc").and_then(Value::as_i64) {
                    Some(0) => "PASS",
                    Some(_) => "FAIL",
                    None => "NOTE",
                };
                lines.push(format!("  - {marker} [{kind}] {}", truncate(summary, 80)));
            }
            for blocker in task
                .get("blockers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let reason = blocker
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                lines.push(format!("  - blocked: {reason}"));
            }
        }
        lines.push(String::new());
    }
    if options.blocked_by != "none" {
        lines.push(format!("**Blocked by**: {}", options.blocked_by));
        lines.push(String::new());
    }
    if options.next_action != "none" {
        lines.push(format!("**Next**: {}", options.next_action));
        lines.push(String::new());
    }
    let section = lines.join("\n") + "\n";
    if path.exists() {
        let existing = fs::read_to_string(&path)?;
        if existing.starts_with('#') {
            if let Some(pos) = existing.find('\n') {
                let (title, rest) = existing.split_at(pos + 1);
                fs::write(&path, format!("{title}\n{section}{rest}"))?;
            } else {
                fs::write(&path, format!("{existing}\n\n{section}"))?;
            }
        } else {
            fs::write(&path, format!("{section}{existing}"))?;
        }
    } else {
        fs::write(&path, format!("# Changelog\n\n{section}"))?;
    }
    Ok(())
}

fn emit_closeout_gate_blocked(repo: &Path, run_id: &str, reason: &str, fields: Value) {
    crate::events::safe_emit(
        repo,
        run_id,
        crate::events::EventRecord {
            event_type: "gate.evaluated".to_string(),
            actor_kind: "lto".to_string(),
            object_id: Some("closeout".to_string()),
            object_type: Some("gate".to_string()),
            summary: format!("closeout gates failed: {reason}"),
            fields: json!({
                "gate": "closeout",
                "status": "failed",
                "reason": reason,
                "detail": fields,
            }),
            ..crate::events::EventRecord::default()
        },
    );
    crate::event_emit::emit_gate_blocked(repo, run_id, "closeout", reason, fields);
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

fn fallback<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn tail_lines(value: &str, max: usize) -> String {
    let mut lines = value.lines().rev().take(max).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LtoState, WorkspaceSnapshot};
    use serde_json::json;
    use std::process::Command;

    fn options(force: bool) -> CloseoutOptions {
        CloseoutOptions {
            run_id: None,
            summary: "shipped the work".to_string(),
            next_action: "none".to_string(),
            blocked_by: "none".to_string(),
            allow_dirty: false,
            no_changelog: false,
            force,
            reverify_timeout: 300,
            no_reverify: false,
        }
    }

    fn ctx(repo: &Path) -> util::RunContext {
        let run_id = "r1".to_string();
        let run_dir = repo.join(".lto").join(&run_id);
        fs::create_dir_all(&run_dir).unwrap();
        let state = LtoState {
            run_id: run_id.clone(),
            goal: "closeout gates".to_string(),
            why: "prove closeout safety".to_string(),
            done_when: "all closeout gates pass".to_string(),
            host_runtime: "codex".to_string(),
            current_phase: "implementation".to_string(),
            workspace: WorkspaceSnapshot {
                head: "abc123".to_string(),
                branch: "feat".to_string(),
                ..WorkspaceSnapshot::default()
            },
            gates: json!({}),
            tasks: json!([]),
            risk_points: json!([]),
            ..LtoState::default()
        };
        let state_path = run_dir.join("state.json");
        crate::state::save_state(&state_path, &state).unwrap();
        util::RunContext {
            state_path,
            run_id,
            run_dir,
            state,
        }
    }

    fn unconverged_ledger() -> &'static str {
        r#"
## Round Summary

| Round | Total | Medium | High | Critical |
|---|---:|---:|---:|---:|
| r1 | 1 | 0 | 1 | 0 |
"#
    }

    #[test]
    fn enforce_gates_rejects_unconverged_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx(tmp.path());
        fs::write(ctx.run_dir.join("audit-ledger.md"), unconverged_ledger()).unwrap();
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("ledger verdict is CONVERGING"));
    }

    #[test]
    fn enforce_gates_rejects_unresolved_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        let marker = tmp.path().join("unresolved-should-not-exist");
        ctx.state.gates = json!({"unresolved_blocks": [{"id": "B1"}]});
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec![format!("side-effect::touch {}", marker.display())],
            Vec::new(),
        );
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("1 unresolved blocks"));
        eprintln!(
            "assertion: unresolved gate left {} nonexistent",
            marker.display()
        );
        assert!(!marker.exists(), "unresolved gate ran the instrument");
    }

    #[test]
    fn enforce_gates_rejects_missing_run_readiness() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.done_when.clear();

        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();

        assert!(err.to_string().contains("run readiness missing"));
        assert!(err.to_string().contains("--done-when"));
    }

    #[test]
    fn enforce_gates_rejects_optional_only_delivery_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            Vec::new(),
            vec!["bounded".into()],
            Vec::new(),
            Vec::new(),
        );

        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();

        assert!(err.to_string().contains("delivery contract incomplete"));
        assert!(err.to_string().contains("--target"));
        assert!(err.to_string().contains("--instrument"));
    }

    #[test]
    fn enforce_gates_rejects_failed_instrument_reverify() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        let marker = tmp.path().join("later-instrument-should-not-exist");
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec![
                "fail::printf 'stdout diagnostic\\n'; printf 'stderr diagnostic\\n' >&2; exit 1"
                    .into(),
                format!("later::touch {}", marker.display()),
            ],
            Vec::new(),
        );

        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        eprintln!("{err}");

        let message = err.to_string();
        assert!(message.contains("fail"));
        assert!(message.contains("stdout diagnostic"));
        assert!(message.contains("stderr diagnostic"));
        eprintln!(
            "assertion: first failure left {} nonexistent",
            marker.display()
        );
        assert!(!marker.exists(), "instrument after first failure ran");
        let events = crate::events::read(tmp.path(), &ctx.run_id).unwrap();
        let gate = events
            .iter()
            .find(|event| event["type"] == "gate.evaluated")
            .unwrap();
        assert_eq!(gate["fields"]["detail"]["reverified_instruments"], 1);
        assert_eq!(gate["fields"]["detail"]["passed_instruments"], 0);
    }

    #[test]
    fn enforce_gates_accepts_successful_instrument_reverify() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec!["ok::true".into()],
            Vec::new(),
        );

        let result = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap();

        assert_eq!(result.attempted, 1);
        assert_eq!(result.passed, 1);
        assert!(result.failed_labels.is_empty());
    }

    #[test]
    fn enforce_gates_preserves_empty_instrument_behavior() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx(tmp.path());

        let result = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap();

        assert_eq!(result, ReverifyResult::default());
    }

    #[test]
    fn no_reverify_skips_failed_instrument() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec!["fail::exit 1".into()],
            Vec::new(),
        );
        let mut options = options(false);
        options.no_reverify = true;

        let result = enforce_gates(tmp.path(), &ctx, &options).unwrap();

        assert_eq!(result, ReverifyResult::default());
    }

    #[test]
    fn reverify_timeout_rejects_slow_instrument() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec!["slow::sleep 2".into()],
            Vec::new(),
        );
        let mut options = options(false);
        options.reverify_timeout = 1;

        let err = enforce_gates(tmp.path(), &ctx, &options).unwrap_err();

        assert!(err.to_string().contains("slow"));
    }

    #[test]
    fn enforce_gates_rejects_already_closed_run() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        let marker = tmp.path().join("closed-should-not-exist");
        ctx.state.current_phase = "closed".to_string();
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec![format!("side-effect::touch {}", marker.display())],
            Vec::new(),
        );
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("run already closed"));
        eprintln!(
            "assertion: closed gate left {} nonexistent",
            marker.display()
        );
        assert!(!marker.exists(), "closed gate ran the instrument");
    }

    #[test]
    fn enforce_gates_rejects_tracked_dirty_paths_outside_lto() {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        fs::write(tmp.path().join("tracked.txt"), "changed\n").unwrap();
        Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let ctx = ctx(tmp.path());
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("tracked uncommitted change"));
    }

    #[test]
    fn human_gate_self_check_reminder_covers_the_three_host_checks() {
        assert!(HUMAN_GATE_SELF_CHECK_REMINDER.contains("亲验"));
        assert!(HUMAN_GATE_SELF_CHECK_REMINDER.contains("ADDED vs MODIFIED"));
        assert!(HUMAN_GATE_SELF_CHECK_REMINDER.contains("target 是否 active"));
        assert!(HUMAN_GATE_SELF_CHECK_REMINDER.contains("write 是否确认成功"));
    }

    #[test]
    fn dirty_worktree_warning_exposes_the_no_changelog_hint_before_refusal() {
        assert!(NO_CHANGELOG_HINT.contains("use --no-changelog after commit"));
        let refusal = format!("closeout refused: tracked changes; {NO_CHANGELOG_HINT}.");
        assert!(refusal.contains(NO_CHANGELOG_HINT));
    }

    #[test]
    fn enforce_gates_rejects_open_unverified_risk_points() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.risk_points = json!([{"id": "R1", "status": "open"}]);
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("risk points unverified"));
    }

    #[test]
    fn enforce_gates_rejects_high_risk_missing_or_empty_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.tasks = json!([{"id": "T1", "title": "deploy database migration"}]);
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("has no audit-ledger.md"));

        fs::write(ctx.run_dir.join("audit-ledger.md"), "## Round Summary\n").unwrap();
        let err = enforce_gates(tmp.path(), &ctx, &options(false)).unwrap_err();
        assert!(err.to_string().contains("empty audit ledger"));
    }

    #[test]
    fn enforce_gates_allows_low_risk_empty_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx(tmp.path());
        fs::write(
            ctx.run_dir.join("audit-ledger.md"),
            "## Round Summary\n| Round | Total | Medium | High | Critical |\n|---|---:|---:|---:|---:|\n",
        )
        .unwrap();

        enforce_gates(tmp.path(), &ctx, &options(false)).unwrap();
    }

    #[test]
    fn force_overrides_non_dirty_closeout_gates() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.current_phase = "closed".to_string();
        ctx.state.gates = json!({"unresolved_blocks": [{"id": "B1"}]});
        ctx.state.risk_points = json!([{"id": "R1", "disposition": "open"}]);
        ctx.state.tasks = json!([{"id": "T1", "title": "deploy database migration"}]);
        ctx.state.delivery_contract = crate::state::DeliveryContract::new(
            vec!["ship".into()],
            Vec::new(),
            vec!["fail::exit 1".into()],
            Vec::new(),
        );
        fs::write(ctx.run_dir.join("audit-ledger.md"), unconverged_ledger()).unwrap();
        assert_eq!(
            enforce_gates(tmp.path(), &ctx, &options(true)).unwrap(),
            ReverifyResult::default()
        );
    }

    #[test]
    fn build_handoff_includes_tokens_and_sorted_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.agent_runs = json!({
            "job1": [{
                "job_id": "job1",
                "runner": "codex",
                "status": "ok",
                "cost": {"tokens_in": 10, "tokens_out": 20, "elapsed_sec": 2.0}
            }]
        });
        let git = util::GitStatus {
            head: "abcdef123456".to_string(),
            branch: "feat".to_string(),
            dirty: false,
        };
        let handoff = build_handoff(
            &ctx,
            &options(false),
            &git,
            &[
                json!({"kind": "zeta", "relative_path": "z.md", "summary": "last"}),
                json!({"kind": "alpha", "relative_path": "a.md", "summary": "first"}),
            ],
        );
        assert!(handoff.contains("- token_usage: 30 total"));
        assert!(handoff.find("`alpha`").unwrap() < handoff.find("`zeta`").unwrap());
    }

    #[test]
    fn write_changelog_inserts_run_summary_and_task_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("CHANGELOG.md"), "# Changelog\n\nold\n").unwrap();
        let mut ctx = ctx(tmp.path());
        ctx.state.tasks = json!([{
            "id": "T1",
            "title": "write tests",
            "status": "done",
            "evidence": [
                {"kind": "test", "summary": "cargo test", "rc": 0},
                {"kind": "test", "summary": "cargo clippy", "rc": 101},
                {"kind": "manual", "summary": "collected pi dispatch"}
            ],
            "blockers": [{"reason": "none now"}]
        }]);
        write_changelog(tmp.path(), &ctx, &options(false)).unwrap();
        let text = fs::read_to_string(tmp.path().join("CHANGELOG.md")).unwrap();
        assert!(text.starts_with("# Changelog\n\n## closeout gates"));
        assert!(text.contains("- **T1**: write tests (done)"));
        assert!(text.contains("PASS [test] cargo test"));
        assert!(text.contains("FAIL [test] cargo clippy"));
        // rc-less evidence carries no verdict: it must not be reported as FAIL.
        assert!(text.contains("NOTE [manual] collected pi dispatch"));
        assert!(text.contains("blocked: none now"));
    }
}
