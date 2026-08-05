use crate::effect::{EffectClass, EffectLevel, classify_effect};
use crate::process;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error(transparent)]
    Git(#[from] process::GitCommandError),
    #[error("worktree already leased by {owner}")]
    AlreadyLeased { owner: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeHandle {
    pub run_id: String,
    pub task_id: String,
    pub path: PathBuf,
    pub base_commit: String,
    pub keep: bool,
}

#[derive(Debug, Default)]
pub struct LeaseTable {
    leases: HashMap<PathBuf, String>,
}

impl LeaseTable {
    pub fn acquire(&mut self, path: PathBuf, owner: String) -> Result<LeaseGuard, WorktreeError> {
        if let Some(existing) = self.leases.get(&path) {
            return Err(WorktreeError::AlreadyLeased {
                owner: existing.clone(),
            });
        }
        self.leases.insert(path.clone(), owner.clone());
        Ok(LeaseGuard { path, owner })
    }

    pub fn release(&mut self, guard: LeaseGuard) {
        if self.leases.get(&guard.path) == Some(&guard.owner) {
            self.leases.remove(&guard.path);
        }
    }
}

#[derive(Debug)]
pub struct LeaseGuard {
    path: PathBuf,
    owner: String,
}

pub fn add_persistent_worktree(
    repo: &Path,
    run_id: &str,
    task_id: &str,
) -> Result<WorktreeHandle, WorktreeError> {
    process::ensure_git_repo(repo)?;
    let base_commit = process::git_stdout(repo, ["rev-parse", "HEAD"])?;
    let worktree_root = repo.join(".lto").join("worktrees").join(run_id);
    fs::create_dir_all(&worktree_root)?;
    let path = worktree_root.join(task_id);
    if path.exists() {
        return Err(WorktreeError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("worktree path already exists: {}", path.display()),
        )));
    }
    process::git(
        repo,
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            path.as_os_str(),
            OsStr::new("HEAD"),
        ],
    )?;
    Ok(WorktreeHandle {
        run_id: run_id.to_string(),
        task_id: task_id.to_string(),
        path,
        base_commit,
        keep: false,
    })
}

pub fn prune_worktree(repo: &Path, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
    if handle.keep {
        return Ok(());
    }
    if handle.path.exists() {
        let _ = process::git(
            repo,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                handle.path.as_os_str(),
            ],
        );
    }
    let _ = process::git(repo, ["worktree", "prune"]);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxResult {
    pub executed: bool,
    pub effect: EffectClass,
    pub rc: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub worktree: Option<PathBuf>,
    pub note: String,
}

pub fn run_in_ephemeral_worktree(
    repo: &Path,
    command: &str,
    allow_network: bool,
    _timeout: Duration,
) -> Result<SandboxResult, WorktreeError> {
    process::ensure_git_repo(repo)?;
    let effect = classify_effect(command);
    if effect.level == EffectLevel::NeedsSemanticJudgement {
        return Ok(SandboxResult {
            executed: false,
            effect,
            rc: None,
            stdout: String::new(),
            stderr: String::new(),
            worktree: None,
            note: "refused: needs human judgement".to_string(),
        });
    }
    if effect.level == EffectLevel::Network && !allow_network {
        return Ok(SandboxResult {
            executed: false,
            effect,
            rc: None,
            stdout: String::new(),
            stderr: String::new(),
            worktree: None,
            note: "refused: network disabled".to_string(),
        });
    }

    let temp = tempfile::Builder::new()
        .prefix("lto_wt_")
        .tempdir()
        .map_err(WorktreeError::Io)?;
    let wt = temp.path().join("wt");
    process::git(
        repo,
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            wt.as_os_str(),
            OsStr::new("HEAD"),
        ],
    )?;
    let mut env = sandboxed_env(&wt);
    let output = process::shell_command(command)
        .current_dir(&wt)
        .env_clear()
        .envs(&env)
        .output()?;
    let result = SandboxResult {
        executed: true,
        effect,
        rc: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        worktree: Some(wt.clone()),
        note: "executed in ephemeral worktree sandbox".to_string(),
    };
    env.clear();
    let _ = process::git(
        repo,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            wt.as_os_str(),
        ],
    );
    let _ = process::git(repo, ["worktree", "prune"]);
    Ok(result)
}

fn sandboxed_env(wt_dir: &Path) -> BTreeMap<String, String> {
    let fake_home = wt_dir.join(".sandbox_home");
    let _ = fs::create_dir_all(&fake_home);
    // Security boundary: only these host variables cross into the sandbox.
    // Any new variable must be added explicitly; never restore inherited env.
    let mut env = BTreeMap::new();
    for key in ["PATH", "LANG", "LC_ALL", "TERM", "TZ", "USER", "SHELL"] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    env.insert("HOME".to_string(), fake_home.display().to_string());
    env.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
    env.insert("GIT_ASKPASS".to_string(), "true".to_string());
    env.insert(
        "GIT_CONFIG_GLOBAL".to_string(),
        fake_home.join("gitconfig-none").display().to_string(),
    );
    env.insert("GIT_CONFIG_SYSTEM".to_string(), "/dev/null".to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        process::git(tmp.path(), ["init", "-q"]).unwrap();
        process::git(tmp.path(), ["config", "user.name", "T"]).unwrap();
        process::git(tmp.path(), ["config", "user.email", "t@example.com"]).unwrap();
        fs::write(tmp.path().join("keep.txt"), "important data\n").unwrap();
        process::git(tmp.path(), ["add", "."]).unwrap();
        process::git(tmp.path(), ["commit", "-q", "-m", "init"]).unwrap();
        tmp
    }

    #[test]
    fn sandboxed_env_contains_only_allowlisted_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let env = sandboxed_env(tmp.path());
        let allowed = [
            "PATH",
            "LANG",
            "LC_ALL",
            "TERM",
            "TZ",
            "USER",
            "SHELL",
            "HOME",
            "GIT_TERMINAL_PROMPT",
            "GIT_ASKPASS",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
        ];
        assert!(env.keys().all(|key| allowed.contains(&key.as_str())));
        // Guard the assertion itself: an empty map would satisfy `all` above,
        // so pin the entries sandboxed_env must always set.
        assert_eq!(
            env.get("GIT_CONFIG_SYSTEM").map(String::as_str),
            Some("/dev/null")
        );
        assert_eq!(
            env.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert!(env.contains_key("HOME"));
        // Host secrets must never cross the boundary, whatever their name.
        for leaked in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(!env.contains_key(leaked), "{leaked} leaked into sandbox");
        }
    }

    #[test]
    fn ephemeral_write_does_not_pollute_main_worktree() {
        let repo = make_repo();
        let result = run_in_ephemeral_worktree(
            repo.path(),
            "echo sandbox-write > newfile.txt",
            true,
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(result.executed);
        assert!(!repo.path().join("newfile.txt").exists());
        assert_eq!(
            fs::read_to_string(repo.path().join("keep.txt")).unwrap(),
            "important data\n"
        );
    }

    #[test]
    fn dangerous_command_is_not_executed() {
        let repo = make_repo();
        let result =
            run_in_ephemeral_worktree(repo.path(), "rm -rf *", true, Duration::from_secs(10))
                .unwrap();
        assert!(!result.executed);
        assert_eq!(result.effect.level, EffectLevel::NeedsSemanticJudgement);
    }

    #[test]
    fn lease_table_is_exclusive() {
        let mut leases = LeaseTable::default();
        let path = PathBuf::from("/tmp/wt");
        let guard = leases.acquire(path.clone(), "job-a".to_string()).unwrap();
        assert!(matches!(
            leases.acquire(path, "job-b".to_string()),
            Err(WorktreeError::AlreadyLeased { .. })
        ));
        leases.release(guard);
    }
}
