use crate::commands::util;
use crate::events::{self, EventRecord};
use crate::state::{self, DecisionAnchor, DecisionEntry, DecisionRecord, DecisionScope, LtoState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub run_id: Option<String>,
    pub text: String,
    pub scope_phase: Option<String>,
    pub scope_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub run_id: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct ReaffirmOptions {
    pub run_id: Option<String>,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRebaseRequired {
    pub id: String,
    pub text: String,
    pub reason: String,
    pub reaffirm_command: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionFreshnessReport {
    pub anchored: usize,
    pub legacy: usize,
    pub rebase_required: Vec<DecisionRebaseRequired>,
}

pub fn cmd_record(repo: &Path, options: RecordOptions) -> anyhow::Result<()> {
    let text = options.text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("decision text must not be empty");
    }
    let run_id = util::resolve_run_id(repo, options.run_id.as_deref())?;
    let _run_lock = util::lock_existing_run(repo, &run_id)?;
    let mut ctx = util::load_run(repo, Some(&run_id))?;
    let entries = state::parse_decision_entries(&ctx.state.user_decisions);
    let now = state::iso_now();
    let actual = util::git_status(repo);
    if actual.head == "unknown" {
        eprintln!(
            "WARN decision record: HEAD 解析失败，anchor_head 将写入 unknown；建议在 Git 仓库根运行"
        );
    }
    let record = DecisionRecord {
        id: next_id(&entries, &now),
        text,
        scope: DecisionScope {
            phase: clean_optional(options.scope_phase),
            paths: clean_paths(options.scope_paths),
        },
        anchor: DecisionAnchor {
            head: actual.head,
            phase: ctx.state.current_phase.clone(),
            recorded_at: now,
        },
        reaffirmed_at: None,
    };
    let mut updated = entries;
    updated.push(DecisionEntry::Typed(record.clone()));
    ctx.state.user_decisions = state::decision_entries_to_value(&updated);
    util::save_run_locked(&ctx)?;
    emit_decision_event(repo, &ctx.state, &record, "decision.recorded");
    println!("decision recorded: {}", record.id);
    Ok(())
}

pub fn cmd_list(repo: &Path, options: ListOptions) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    let entries = state::parse_decision_entries(&ctx.state.user_decisions);
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&entries.iter().map(entry_json).collect::<Vec<_>>())?
        );
    } else {
        render_entries_text(&entries)?;
    }
    Ok(())
}

pub fn cmd_reaffirm(repo: &Path, options: ReaffirmOptions) -> anyhow::Result<()> {
    let run_id = util::resolve_run_id(repo, options.run_id.as_deref())?;
    let _run_lock = util::lock_existing_run(repo, &run_id)?;
    let mut ctx = util::load_run(repo, Some(&run_id))?;
    let actual = util::git_status(repo);
    let now = state::iso_now();
    let entries = state::parse_decision_entries(&ctx.state.user_decisions);
    let mut updated = false;
    let mut updated_entries = entries;
    for entry in &mut updated_entries {
        match entry {
            DecisionEntry::Typed(record) if record.id == options.id => {
                record.anchor.head = actual.head;
                record.anchor.phase = ctx.state.current_phase.clone();
                record.reaffirmed_at = Some(now);
                updated = true;
                break;
            }
            DecisionEntry::Legacy(value)
                if value.get("id").and_then(Value::as_str) == Some(options.id.as_str()) =>
            {
                anyhow::bail!(
                    "cannot reaffirm legacy decision {}; record a typed decision first",
                    options.id
                );
            }
            _ => {}
        }
    }
    if !updated {
        anyhow::bail!("decision not found: {}", options.id);
    }
    let record = updated_entries
        .iter()
        .find_map(|entry| match entry {
            DecisionEntry::Typed(record) if record.id == options.id => Some(record.clone()),
            _ => None,
        })
        .expect("updated decision must remain present");
    ctx.state.user_decisions = state::decision_entries_to_value(&updated_entries);
    util::save_run_locked(&ctx)?;
    emit_decision_event(repo, &ctx.state, &record, "decision.reaffirmed");
    println!("decision reaffirmed: {}", record.id);
    Ok(())
}

pub fn freshness_report(repo: &Path, state: &LtoState) -> DecisionFreshnessReport {
    let actual = util::git_status(repo);
    freshness_report_at(repo, state, &actual.head)
}

pub fn freshness_report_at(
    repo: &Path,
    state: &LtoState,
    actual_head: &str,
) -> DecisionFreshnessReport {
    let mut report = DecisionFreshnessReport::default();
    for entry in state::parse_decision_entries(&state.user_decisions) {
        match entry {
            DecisionEntry::Legacy(_) => report.legacy += 1,
            DecisionEntry::Typed(record) => {
                report.anchored += 1;
                let mut reasons = Vec::new();
                if record.anchor.phase != state.current_phase {
                    reasons.push(format!(
                        "phase drift ({} -> {})",
                        fallback(&record.anchor.phase),
                        fallback(&state.current_phase)
                    ));
                }
                let drift = util::head_drift(repo, &record.anchor.head, actual_head);
                if let Some(reason) = head_reason(drift, &record.anchor.head, actual_head) {
                    reasons.push(reason);
                }
                if !reasons.is_empty() {
                    report.rebase_required.push(DecisionRebaseRequired {
                        id: record.id.clone(),
                        text: record.text,
                        reason: reasons.join("; "),
                        reaffirm_command: format!("lto decision reaffirm --id {}", record.id),
                    });
                }
            }
        }
    }
    report
}

pub fn render_freshness_text(report: &DecisionFreshnessReport) -> String {
    let mut lines = Vec::new();
    if report.rebase_required.is_empty() {
        lines.push(format!("decisions: {} anchored, fresh", report.anchored));
    } else {
        lines.push(format!(
            "decisions: {} anchored, {} require rebase",
            report.anchored,
            report.rebase_required.len()
        ));
        for item in &report.rebase_required {
            lines.push(format!(
                "DECISION_REBASE_REQUIRED id={} text={} reason={} reaffirm={}",
                item.id,
                util::single_line(&item.text),
                item.reason,
                item.reaffirm_command
            ));
        }
    }
    if report.legacy > 0 {
        lines.push(format!(
            "decisions: {} legacy; 无锚点（legacy），建议补录",
            report.legacy
        ));
    }
    lines.join("\n")
}

pub fn freshness_json(report: &DecisionFreshnessReport) -> Value {
    serde_json::to_value(report).expect("decision freshness report is serializable")
}

fn emit_decision_event(repo: &Path, state: &LtoState, record: &DecisionRecord, event_type: &str) {
    let _ = events::safe_emit(
        repo,
        &state.run_id,
        EventRecord {
            event_type: event_type.to_string(),
            actor_kind: "host".to_string(),
            phase: Some(state.current_phase.clone()),
            object_id: Some(record.id.clone()),
            object_type: Some("decision".to_string()),
            summary: format!("{event_type}: {}", record.id),
            fields: json!({
                "decision_id": record.id,
                "scope_phase": record.scope.phase,
                "scope_paths": record.scope.paths,
                "anchor_head": record.anchor.head,
                "anchor_phase": record.anchor.phase,
            }),
            ..EventRecord::default()
        },
    );
}

fn entry_json(entry: &DecisionEntry) -> Value {
    match entry {
        DecisionEntry::Typed(record) => json!({"kind": "typed", "record": record}),
        DecisionEntry::Legacy(value) => json!({
            "kind": "legacy",
            "value": value,
            "status": "无锚点（legacy），建议补录",
        }),
    }
}

fn render_entries_text(entries: &[DecisionEntry]) -> anyhow::Result<()> {
    if entries.is_empty() {
        println!("decisions: none");
        return Ok(());
    }
    for entry in entries {
        match entry {
            DecisionEntry::Typed(record) => println!(
                "{}: {} [scope_phase={} scope_paths={} anchor_head={} anchor_phase={} reaffirmed_at={}]",
                record.id,
                util::single_line(&record.text),
                record.scope.phase.as_deref().unwrap_or("none"),
                if record.scope.paths.is_empty() {
                    "none".to_string()
                } else {
                    record.scope.paths.join(",")
                },
                short_head(&record.anchor.head),
                record.anchor.phase,
                record.reaffirmed_at.as_deref().unwrap_or("none")
            ),
            DecisionEntry::Legacy(value) => println!(
                "无锚点（legacy），建议补录: {}",
                serde_json::to_string(value)?
            ),
        }
    }
    Ok(())
}

fn next_id(entries: &[DecisionEntry], now: &str) -> String {
    let slug = now
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let base = format!("decision-{slug}");
    if !entries
        .iter()
        .any(|entry| entry_id(entry) == Some(base.as_str()))
    {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !entries
            .iter()
            .any(|entry| entry_id(entry) == Some(candidate.as_str()))
        {
            return candidate;
        }
    }
    unreachable!("decision id suffix space is finite")
}

fn entry_id(entry: &DecisionEntry) -> Option<&str> {
    match entry {
        DecisionEntry::Typed(record) => Some(record.id.as_str()),
        DecisionEntry::Legacy(value) => value.get("id").and_then(Value::as_str),
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clean_paths(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect()
}

fn head_reason(drift: &str, recorded: &str, actual: &str) -> Option<String> {
    match drift {
        "none" => None,
        "forward" => Some(format!(
            "HEAD advanced ({} -> {})",
            short_head(recorded),
            short_head(actual)
        )),
        "rewrite" => Some(format!(
            "HEAD rewritten ({} not ancestor of {})",
            short_head(recorded),
            short_head(actual)
        )),
        "unreachable" => Some(format!(
            "recorded HEAD {} unreachable",
            short_head(recorded)
        )),
        _ => Some(format!("HEAD drift: {drift}")),
    }
}

fn short_head(value: &str) -> String {
    value.chars().take(12).collect()
}

fn fallback(value: &str) -> &str {
    if value.trim().is_empty() {
        "unknown"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, WorkspaceSnapshot};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct GitHarness {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
    }

    impl GitHarness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path().join("repo");
            fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init"]);
            git(&repo, &["config", "user.email", "lto@example.test"]);
            git(&repo, &["config", "user.name", "LTO Test"]);
            Self { _tmp: tmp, repo }
        }

        fn write_commit(&self, file: &str, text: &str, message: &str) -> String {
            fs::write(self.repo.join(file), text).unwrap();
            git(&self.repo, &["add", file]);
            git(&self.repo, &["commit", "-m", message]);
            head(&self.repo)
        }

        fn write_state(&self, state: LtoState) {
            let run_dir = self.repo.join(".lto").join("r1");
            fs::create_dir_all(&run_dir).unwrap();
            fs::write(self.repo.join(".lto").join("current"), "r1\n").unwrap();
            state::save_state(run_dir.join("state.json"), &state).unwrap();
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn head(repo: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    fn base_state(head: &str) -> LtoState {
        LtoState {
            run_id: "r1".into(),
            goal: "decision test".into(),
            current_phase: "implementation".into(),
            workspace: WorkspaceSnapshot {
                head: head.into(),
                ..WorkspaceSnapshot::default()
            },
            ..LtoState::default()
        }
    }

    fn typed_record(id: &str, head: &str, phase: &str) -> DecisionRecord {
        DecisionRecord {
            id: id.into(),
            text: format!("decision {id}"),
            scope: DecisionScope::default(),
            anchor: DecisionAnchor {
                head: head.into(),
                phase: phase.into(),
                recorded_at: "before".into(),
            },
            reaffirmed_at: None,
        }
    }

    #[test]
    fn record_writes_anchor_and_event() {
        let h = GitHarness::new();
        let head = h.write_commit("a.txt", "a\n", "base");
        h.write_state(base_state(&head));

        cmd_record(
            &h.repo,
            RecordOptions {
                run_id: Some("r1".into()),
                text: "Keep the host gate".into(),
                scope_phase: Some("implementation".into()),
                scope_paths: vec!["src/state.rs".into()],
            },
        )
        .unwrap();

        let saved = state::load_state(h.repo.join(".lto/r1/state.json")).unwrap();
        let entries = state::parse_decision_entries(&saved.user_decisions);
        let record = entries[0].as_record().unwrap();
        assert_eq!(record.text, "Keep the host gate");
        assert_eq!(record.anchor.head, head);
        assert_eq!(record.anchor.phase, "implementation");
        assert_eq!(record.scope.paths, vec!["src/state.rs"]);
        let events = events::read(&h.repo, "r1").unwrap();
        assert_eq!(events[0]["type"], "decision.recorded");
    }

    #[test]
    fn record_allows_unknown_head_and_persists_unknown_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("not-a-git-repo");
        fs::create_dir_all(repo.join(".lto/r1")).unwrap();
        fs::write(repo.join(".lto/current"), "r1\n").unwrap();
        state::save_state(repo.join(".lto/r1/state.json"), &base_state("unknown")).unwrap();

        cmd_record(
            &repo,
            RecordOptions {
                run_id: Some("r1".into()),
                text: "Keep fail-safe recording".into(),
                scope_phase: None,
                scope_paths: Vec::new(),
            },
        )
        .unwrap();

        let saved = state::load_state(repo.join(".lto/r1/state.json")).unwrap();
        let entries = state::parse_decision_entries(&saved.user_decisions);
        let record = entries[0].as_record().unwrap();
        assert_eq!(record.anchor.head, "unknown");
    }

    #[test]
    fn list_json_marks_typed_and_legacy_entries() {
        let entries = [
            DecisionEntry::Legacy(json!({"old": true})),
            DecisionEntry::Typed(DecisionRecord {
                id: "d-1".into(),
                text: "typed".into(),
                scope: DecisionScope::default(),
                anchor: DecisionAnchor {
                    head: "a".repeat(40),
                    phase: "audit".into(),
                    recorded_at: "now".into(),
                },
                reaffirmed_at: None,
            }),
        ];
        let output = entries.iter().map(entry_json).collect::<Vec<_>>();
        assert_eq!(output[0]["kind"], "legacy");
        assert_eq!(output[0]["status"], "无锚点（legacy），建议补录");
        assert_eq!(output[1]["kind"], "typed");
    }

    #[test]
    fn reaffirm_updates_anchor_and_timestamp() {
        let h = GitHarness::new();
        let head = h.write_commit("a.txt", "a\n", "base");
        let mut state = base_state(&head);
        state.user_decisions = json!([DecisionRecord {
            id: "d-1".into(),
            text: "typed".into(),
            scope: DecisionScope::default(),
            anchor: DecisionAnchor {
                head: "b".repeat(40),
                phase: "audit".into(),
                recorded_at: "before".into(),
            },
            reaffirmed_at: None,
        }]);
        h.write_state(state);

        cmd_reaffirm(
            &h.repo,
            ReaffirmOptions {
                run_id: Some("r1".into()),
                id: "d-1".into(),
            },
        )
        .unwrap();

        let saved = state::load_state(h.repo.join(".lto/r1/state.json")).unwrap();
        let record = state::parse_decision_entries(&saved.user_decisions)[0]
            .as_record()
            .unwrap()
            .clone();
        assert_eq!(record.anchor.head, head);
        assert_eq!(record.anchor.phase, "implementation");
        assert!(record.reaffirmed_at.is_some());
        assert_eq!(
            events::read(&h.repo, "r1").unwrap()[0]["type"],
            "decision.reaffirmed"
        );
        let fresh = freshness_report(&h.repo, &saved);
        assert_eq!(fresh.anchored, 1);
        assert!(fresh.rebase_required.is_empty());
    }

    #[test]
    fn freshness_reports_head_forward_drift() {
        let h = GitHarness::new();
        let first = h.write_commit("a.txt", "a\n", "base");
        let mut state = base_state(&first);
        state.user_decisions = json!([typed_record("d-head", &first, "implementation")]);
        h.write_state(state);
        h.write_commit("b.txt", "b\n", "advance");

        let report = freshness_report(
            &h.repo,
            &state::load_state(h.repo.join(".lto/r1/state.json")).unwrap(),
        );

        assert_eq!(report.anchored, 1);
        assert_eq!(report.rebase_required.len(), 1);
        assert!(report.rebase_required[0].reason.contains("HEAD advanced"));
        assert!(render_freshness_text(&report).contains("DECISION_REBASE_REQUIRED"));
    }

    #[test]
    fn freshness_reports_phase_drift_without_ttl() {
        let h = GitHarness::new();
        let head = h.write_commit("a.txt", "a\n", "base");
        let mut state = base_state(&head);
        state.user_decisions = json!([typed_record("d-phase", &head, "audit")]);
        h.write_state(state);

        let saved = state::load_state(h.repo.join(".lto/r1/state.json")).unwrap();
        let report = freshness_report(&h.repo, &saved);

        assert_eq!(report.rebase_required.len(), 1);
        assert!(report.rebase_required[0].reason.contains("phase drift"));
    }

    #[test]
    fn freshness_groups_legacy_entries_without_blocking() {
        let h = GitHarness::new();
        let head = h.write_commit("a.txt", "a\n", "base");
        let mut state = base_state(&head);
        state.user_decisions = json!([{"legacy": true}, "old scalar"]);
        h.write_state(state);

        let saved = state::load_state(h.repo.join(".lto/r1/state.json")).unwrap();
        let report = freshness_report(&h.repo, &saved);

        assert_eq!(report.anchored, 0);
        assert_eq!(report.legacy, 2);
        assert!(report.rebase_required.is_empty());
        assert!(render_freshness_text(&report).contains("无锚点（legacy），建议补录"));
    }
}
