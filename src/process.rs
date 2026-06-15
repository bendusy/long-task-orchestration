use std::ffi::OsStr;
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

pub fn ensure_git_repo(repo: &Path) -> Result<(), GitCommandError> {
    let output = git_output(repo, ["rev-parse", "--is-inside-work-tree"])?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true" {
        Ok(())
    } else {
        Err(GitCommandError::NotGitRepo(repo.display().to_string()))
    }
}

pub fn git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<(), GitCommandError> {
    let output = git_output(repo, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GitCommandError::NonZero {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn git_stdout<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String, GitCommandError> {
    let output = git_output(repo, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(GitCommandError::NonZero {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<Output, std::io::Error> {
    Command::new("git").args(args).current_dir(repo).output()
}

pub fn command_with_args<I, S>(program: &str, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd
}
