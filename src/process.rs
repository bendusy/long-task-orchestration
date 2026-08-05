use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Output};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitCommandError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a git worktree: {0}")]
    NotGitRepo(String),
    #[error("git {args} failed: {stderr}")]
    NonZero { args: String, stderr: String },
}

pub fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Wrap a value in POSIX single quotes for safe interpolation into a shell
/// command line. Embedded single quotes are closed, escaped, and reopened.
///
/// Shared by every path that builds a shell line for an external agent
/// (dispatch, turn completion, tmux send-keys) — the escaping is security
/// relevant, so it must have exactly one definition.
pub fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

pub fn ensure_git_repo(repo: &Path) -> Result<(), GitCommandError> {
    let output = git_output(repo, ["rev-parse", "--is-inside-work-tree"])?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true" {
        Ok(())
    } else {
        Err(GitCommandError::NotGitRepo(repo.display().to_string()))
    }
}

pub fn git<I, S>(repo: &Path, args: I) -> Result<(), GitCommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = collect_args(args);
    let output = git_output_args(repo, &args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GitCommandError::NonZero {
            args: display_args(&args),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn git_stdout<I, S>(repo: &Path, args: I) -> Result<String, GitCommandError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = collect_args(args);
    let output = git_output_args(repo, &args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(GitCommandError::NonZero {
            args: display_args(&args),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn git_output<I, S>(repo: &Path, args: I) -> Result<Output, std::io::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = collect_args(args);
    git_output_args(repo, &args)
}

fn collect_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect()
}

fn git_output_args(repo: &Path, args: &[OsString]) -> Result<Output, std::io::Error> {
    Command::new("git").args(args).current_dir(repo).output()
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn shell_single_quote_round_trips_shell_values() {
        for input in ["it's", "`cmd`", "$(rm -rf /)", "line one\nline two"] {
            let quoted = shell_single_quote(input);
            let output = Command::new("sh")
                .args(["-c", &format!("echo {quoted}")])
                .output()
                .unwrap();
            assert!(output.status.success());
            let rendered = String::from_utf8(output.stdout).unwrap();
            assert_eq!(rendered.strip_suffix('\n').unwrap(), input);
        }
    }

    #[test]
    fn shell_single_quote_is_single_quoted() {
        let quoted = shell_single_quote("value");
        assert_eq!(quoted, "'value'");
    }

    #[test]
    fn ensure_git_repo_accepts_initialized_repo() {
        let repo = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        ensure_git_repo(repo.path()).unwrap();
    }

    #[test]
    fn ensure_git_repo_rejects_non_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            ensure_git_repo(dir.path()),
            Err(GitCommandError::NotGitRepo(_))
        ));
    }
}
