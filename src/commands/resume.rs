use crate::commands::util;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ResumeOptions {
    pub run_id: Option<String>,
}

pub fn cmd_resume(repo: &Path, options: ResumeOptions) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    let actual = util::git_status(repo);
    let recorded_head = if ctx.state.workspace.head.trim().is_empty() {
        "unknown"
    } else {
        ctx.state.workspace.head.as_str()
    };
    let drift = util::head_drift(repo, recorded_head, &actual.head);
    let mut warnings = Vec::new();
    let mut revalidate = Vec::new();

    match drift {
        "none" => {}
        "forward" => {
            let changed = util::git_changed_paths(repo, recorded_head, &actual.head);
            let touched = touched_files(&ctx.state.tasks);
            let related = changed
                .iter()
                .filter(|path| touched.contains(path.as_str()))
                .take(6)
                .cloned()
                .collect::<Vec<_>>();
            if !related.is_empty() {
                revalidate = revalidatable_tasks(&ctx.state.tasks);
                warnings.push(format!(
                    "HEAD advanced ({}→{}), related files changed: {}",
                    short(recorded_head),
                    short(&actual.head),
                    related.join(", ")
                ));
            } else if touched.is_empty() {
                warnings.push(format!(
                    "HEAD advanced ({}→{}), no task touched_files recorded; file drift precision unavailable",
                    short(recorded_head),
                    short(&actual.head)
                ));
            } else {
                warnings.push(format!(
                    "HEAD advanced ({}→{}), no related file changes",
                    short(recorded_head),
                    short(&actual.head)
                ));
            }
        }
        "rewrite" => {
            revalidate = revalidatable_tasks(&ctx.state.tasks);
            warnings.push(format!(
                "HEAD rewritten ({} not ancestor of {})",
                short(recorded_head),
                short(&actual.head)
            ));
        }
        "unreachable" => {
            revalidate = non_pending_tasks(&ctx.state.tasks);
            warnings.push(format!(
                "recorded HEAD {} unreachable",
                short(recorded_head)
            ));
        }
        _ => {}
    }
    if actual.dirty {
        warnings.push("worktree has uncommitted changes outside .lto".to_string());
    }
    if ctx.state.current_phase == "closed" && !revalidate.is_empty() {
        warnings.push(
            "run is closed; resume is read-only and will not reopen tasks or update recorded HEAD"
                .to_string(),
        );
        revalidate.clear();
    }

    print_capsule(repo, &ctx, &warnings, &revalidate);
    if revalidate.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "{} tasks require revalidation: {}",
            revalidate.len(),
            revalidate.join(", ")
        )
    }
}

fn print_capsule(repo: &Path, ctx: &util::RunContext, warnings: &[String], revalidate: &[String]) {
    let state = &ctx.state;
    let head = if state.workspace.head.trim().is_empty() {
        "unknown"
    } else {
        state.workspace.head.as_str()
    };
    let task_summary = task_summary(&state.tasks);
    let last_failure = value_as_line(&state.last_failure, 120);
    let next_action = value_as_line(&state.next_action, 120);
    let blocked = state.blocked_by.as_str().unwrap_or("none");
    let unresolved = state
        .gates
        .get("unresolved_blocks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();

    println!("=== LTO ACTIVE SESSION ===");
    println!("Run ID: {}", state.run_id);
    println!("Goal: {}", fallback(&state.goal, "?"));
    println!("Phase: {}", fallback(&state.current_phase, "unknown"));
    println!(
        "Head: {} ({})",
        short(head),
        fallback(&state.workspace.dirty_fingerprint, "?")
    );
    println!("Tasks: {task_summary}");
    if !last_failure.is_empty() {
        println!("Last Failure: {last_failure}");
    }
    println!("Next: {next_action}");
    if !blocked.is_empty() && blocked != "none" {
        println!("Blocked: {blocked}");
    }
    if unresolved > 0 {
        println!("Unresolved Blocks: {unresolved}");
    }

    let artifacts = util::latest_artifacts(repo, &ctx.run_id, 6);
    if !artifacts.is_empty() {
        println!("Recent Artifacts:");
        for entry in artifacts {
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("other");
            let path = entry
                .get("relative_path")
                .and_then(Value::as_str)
                .or_else(|| entry.get("run_relative_path").and_then(Value::as_str))
                .unwrap_or("?");
            let summary = entry.get("summary").and_then(Value::as_str).unwrap_or("");
            let source = if entry.get("source").and_then(Value::as_str) == Some("synthesized") {
                " (synthesized)"
            } else {
                ""
            };
            if summary.is_empty() {
                println!("  - {kind}: {path}{source}");
            } else {
                println!("  - {kind}: {path} — {}{source}", truncate(summary, 80));
            }
        }
    }

    if !warnings.is_empty() {
        println!();
        println!("Warnings:");
        for warning in warnings {
            println!("  ⚠ {warning}");
        }
    }
    if !revalidate.is_empty() {
        println!();
        println!(
            "⚠ {} tasks require revalidation: {}",
            revalidate.len(),
            revalidate
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let gap = util::max_session_gap_hours(state);
    if gap >= 24.0 {
        println!();
        println!(
            "⏳ 距上次活动约 {} 小时（{} 天）。建议先给用户跑 `lto recap`——隔这么久，人可能忘了在做什么、为什么。",
            gap as u64,
            gap as u64 / 24
        );
    }
    println!("===========================");
}

fn task_summary(tasks: &Value) -> String {
    let tasks = util::json_array(tasks);
    if tasks.is_empty() {
        return "none".to_string();
    }
    tasks
        .iter()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|task| {
            let id = task.get("id").and_then(Value::as_str).unwrap_or("?");
            let status = task
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            format!("{id}:{status}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn touched_files(tasks: &Value) -> BTreeSet<String> {
    util::json_array(tasks)
        .iter()
        .flat_map(|task| {
            task.get("touched_files")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn revalidatable_tasks(tasks: &Value) -> Vec<String> {
    util::json_array(tasks)
        .iter()
        .filter(|task| {
            matches!(
                task.get("status").and_then(Value::as_str),
                Some("in_progress" | "done")
            )
        })
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn non_pending_tasks(tasks: &Value) -> Vec<String> {
    util::json_array(tasks)
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) != Some("pending"))
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn value_as_line(value: &Value, limit: usize) -> String {
    let raw = value.as_str().map(str::to_string).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    });
    truncate(&util::single_line(&raw), limit)
}

fn fallback<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn short(value: &str) -> String {
    value.chars().take(12).collect()
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, LtoState, WorkspaceSnapshot};
    use serde_json::json;
    use std::fs;
    use std::process::Command;

    struct GitHarness {
        _tmp: tempfile::TempDir,
        repo: std::path::PathBuf,
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

        fn write_commit(&self, file: &str, text: &str, msg: &str) -> String {
            fs::write(self.repo.join(file), text).unwrap();
            git(&self.repo, &["add", file]);
            git(&self.repo, &["commit", "-m", msg]);
            head(&self.repo)
        }

        fn write_state(&self, recorded_head: &str, tasks: serde_json::Value) {
            let run_dir = self.repo.join(".lto").join("r1");
            fs::create_dir_all(&run_dir).unwrap();
            fs::write(self.repo.join(".lto").join("current"), "r1\n").unwrap();
            let state = LtoState {
                run_id: "r1".to_string(),
                goal: "resume drift".to_string(),
                current_phase: "implementation".to_string(),
                workspace: WorkspaceSnapshot {
                    head: recorded_head.to_string(),
                    dirty_fingerprint: "clean".to_string(),
                    ..WorkspaceSnapshot::default()
                },
                tasks,
                next_action: json!("continue"),
                blocked_by: json!("none"),
                ..LtoState::default()
            };
            state::save_state(run_dir.join("state.json"), &state).unwrap();
        }
    }

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

    #[test]
    fn cmd_resume_main_path_has_no_revalidation_when_head_matches() {
        let h = GitHarness::new();
        let first = h.write_commit("a.txt", "a\n", "first");
        h.write_state(
            &first,
            json!([{"id": "T1", "status": "done", "touched_files": ["a.txt"]}]),
        );
        cmd_resume(
            &h.repo,
            ResumeOptions {
                run_id: Some("r1".into()),
            },
        )
        .unwrap();
    }

    #[test]
    fn cmd_resume_forward_drift_revalidates_related_done_tasks() {
        let h = GitHarness::new();
        let first = h.write_commit("a.txt", "a\n", "first");
        h.write_state(
            &first,
            json!([{"id": "T1", "status": "done", "touched_files": ["a.txt"]}]),
        );
        h.write_commit("a.txt", "changed\n", "change touched");
        let err = cmd_resume(
            &h.repo,
            ResumeOptions {
                run_id: Some("r1".into()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("T1"));
    }

    #[test]
    fn cmd_resume_rewrite_drift_revalidates_done_tasks() {
        let h = GitHarness::new();
        let first = h.write_commit("base.txt", "base\n", "base");
        let main_branch = String::from_utf8(
            Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(&h.repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(&h.repo, &["checkout", "-b", "side"]);
        let side = h.write_commit("side.txt", "side\n", "side");
        git(&h.repo, &["checkout", &main_branch]);
        assert_eq!(head(&h.repo), first);
        h.write_state(
            &side,
            json!([{"id": "T1", "status": "done", "touched_files": ["side.txt"]}]),
        );
        h.write_commit("main.txt", "main\n", "main");
        let err = cmd_resume(
            &h.repo,
            ResumeOptions {
                run_id: Some("r1".into()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("T1"));
    }

    #[test]
    fn cmd_resume_unreachable_recorded_head_revalidates_non_pending_tasks() {
        let h = GitHarness::new();
        h.write_commit("a.txt", "a\n", "first");
        h.write_state(
            "1111111111111111111111111111111111111111",
            json!([
                {"id": "T1", "status": "done"},
                {"id": "T2", "status": "pending"}
            ]),
        );
        let err = cmd_resume(
            &h.repo,
            ResumeOptions {
                run_id: Some("r1".into()),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("T1"));
        assert!(!err.to_string().contains("T2"));
    }
}
