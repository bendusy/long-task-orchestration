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
    let _ = process::git_output(&worktree.path, ["add", "-N", "."]);
    let diff = process::git_stdout(
        &worktree.path,
        ["diff", "--binary", worktree.base_commit.as_str(), "--"],
    )?;
    let test_result = match test_cmd {
        Some(cmd) => run_test_command(&worktree.path, cmd)?,
        None => TestResult {
            status: TestStatus::NotRun,
            command: None,
            stdout: String::new(),
            stderr: String::new(),
        },
    };
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
}
