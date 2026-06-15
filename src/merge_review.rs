use crate::process;
use crate::worktree::WorktreeHandle;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Passed,
    Failed(i32),
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub status: TestStatus,
    pub command: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffWithTests {
    pub base_commit: String,
    pub diff: String,
    pub test_result: TestResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestGateAction {
    Batchable,
    ImmediateReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditOpinion {
    pub source: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeReview {
    pub diff: DiffWithTests,
    pub touched_paths: Vec<String>,
    pub suggest_audit: bool,
    pub audit_opinion: Option<AuditOpinion>,
}

pub fn emit_diff(
    worktree: &WorktreeHandle,
    test_cmd: Option<&str>,
) -> anyhow::Result<DiffWithTests> {
    let test_result = match test_cmd {
        Some(cmd) => run_test_command(&worktree.path, cmd)?,
        None => TestResult {
            status: TestStatus::NotRun,
            command: None,
            stdout: String::new(),
            stderr: String::new(),
        },
    };
    let diff = diff_against_base(worktree)?;
    Ok(DiffWithTests {
        base_commit: worktree.base_commit.clone(),
        diff,
        test_result,
    })
}

pub fn build_merge_review(diff: DiffWithTests) -> MergeReview {
    let touched_paths = touched_paths_from_diff(&diff.diff);
    let suggest_audit = touched_paths.iter().any(|path| {
        path.contains("security")
            || path.contains("auth")
            || path.contains("migration")
            || path.ends_with(".sql")
    }) || !matches!(diff.test_result.status, TestStatus::Passed);
    MergeReview {
        diff,
        touched_paths,
        suggest_audit,
        audit_opinion: None,
    }
}

pub fn test_gate_action(diff: &DiffWithTests) -> TestGateAction {
    match diff.test_result.status {
        TestStatus::Failed(_) => TestGateAction::ImmediateReport,
        TestStatus::Passed | TestStatus::NotRun => TestGateAction::Batchable,
    }
}

pub fn merge_worktree(
    repo: &Path,
    worktree: &WorktreeHandle,
    message: &str,
) -> anyhow::Result<String> {
    let has_changes = has_uncommitted_changes(&worktree.path)?;
    if has_changes {
        process::git(&worktree.path, ["add", "-A"])?;
        process::git(&worktree.path, ["commit", "-m", message])?;
    }
    let head = process::git_stdout(&worktree.path, ["rev-parse", "HEAD"])?;
    if head == worktree.base_commit {
        anyhow::bail!("worktree has no changes to merge");
    }
    process::git(repo, ["merge", "--no-ff", head.as_str(), "-m", message])?;
    Ok(head)
}

fn diff_against_base(worktree: &WorktreeHandle) -> anyhow::Result<String> {
    let _ = process::git_output(&worktree.path, ["add", "-N", "."]);
    if has_uncommitted_changes(&worktree.path)? {
        return Ok(process::git_stdout(
            &worktree.path,
            ["diff", "--binary", worktree.base_commit.as_str(), "--"],
        )?);
    }
    let range = format!("{}...HEAD", worktree.base_commit);
    Ok(process::git_stdout(
        &worktree.path,
        ["diff", "--binary", range.as_str(), "--"],
    )?)
}

fn has_uncommitted_changes(path: &Path) -> anyhow::Result<bool> {
    Ok(!process::git_stdout(path, ["status", "--porcelain"])?.is_empty())
}

fn run_test_command(worktree: &Path, command: &str) -> anyhow::Result<TestResult> {
    let output = process::shell_command(command)
        .current_dir(worktree)
        .output()?;
    Ok(TestResult {
        status: if output.status.success() {
            TestStatus::Passed
        } else {
            TestStatus::Failed(output.status.code().unwrap_or(-1))
        },
        command: Some(command.to_string()),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn touched_paths_from_diff(diff: &str) -> Vec<String> {
    diff.lines()
        .filter_map(|line| line.strip_prefix("diff --git a/"))
        .filter_map(|rest| rest.split_once(" b/").map(|(_, path)| path))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_opinion_is_optional_and_not_part_of_default_review() {
        let diff = DiffWithTests {
            base_commit: "abc".to_string(),
            diff: "diff --git a/auth.rs b/auth.rs\n".to_string(),
            test_result: TestResult {
                status: TestStatus::Passed,
                command: Some("true".to_string()),
                stdout: String::new(),
                stderr: String::new(),
            },
        };
        let review = build_merge_review(diff);
        assert!(review.suggest_audit);
        assert!(review.audit_opinion.is_none());
    }

    #[test]
    fn emit_diff_includes_uncommitted_worktree_changes_and_test_status() {
        let repo = make_repo();
        let handle = crate::worktree::add_persistent_worktree(repo.path(), "r1", "t1").unwrap();
        std::fs::write(handle.path.join("feature.txt"), "new feature\n").unwrap();

        let diff = emit_diff(&handle, Some("true")).unwrap();

        assert!(diff.diff.contains("feature.txt"));
        assert_eq!(diff.test_result.status, TestStatus::Passed);
        assert_eq!(test_gate_action(&diff), TestGateAction::Batchable);
        crate::worktree::prune_worktree(repo.path(), &handle).unwrap();
    }

    #[test]
    fn failing_test_gate_reports_immediately_but_keeps_diff() {
        let repo = make_repo();
        let handle = crate::worktree::add_persistent_worktree(repo.path(), "r2", "t2").unwrap();
        std::fs::write(handle.path.join("broken.txt"), "broken\n").unwrap();

        let diff = emit_diff(&handle, Some("false")).unwrap();

        assert!(diff.diff.contains("broken.txt"));
        assert!(matches!(diff.test_result.status, TestStatus::Failed(_)));
        assert_eq!(test_gate_action(&diff), TestGateAction::ImmediateReport);
        crate::worktree::prune_worktree(repo.path(), &handle).unwrap();
    }

    #[test]
    fn merge_worktree_commits_uncommitted_changes_then_shells_out_git_merge() {
        let repo = make_repo();
        let handle = crate::worktree::add_persistent_worktree(repo.path(), "r3", "t3").unwrap();
        std::fs::write(handle.path.join("merged.txt"), "merged\n").unwrap();

        let merged_head = merge_worktree(repo.path(), &handle, "merge test worktree").unwrap();

        assert_ne!(merged_head, handle.base_commit);
        assert_eq!(
            std::fs::read_to_string(repo.path().join("merged.txt")).unwrap(),
            "merged\n"
        );
        crate::worktree::prune_worktree(repo.path(), &handle).unwrap();
    }

    fn make_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        process::git(tmp.path(), ["init", "-q"]).unwrap();
        process::git(tmp.path(), ["config", "user.name", "T"]).unwrap();
        process::git(tmp.path(), ["config", "user.email", "t@example.com"]).unwrap();
        std::fs::write(tmp.path().join("base.txt"), "base\n").unwrap();
        process::git(tmp.path(), ["add", "."]).unwrap();
        process::git(tmp.path(), ["commit", "-q", "-m", "init"]).unwrap();
        tmp
    }
}
