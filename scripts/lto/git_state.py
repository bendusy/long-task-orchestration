"""Git 状态查询：HEAD、dirty、branch、ancestor 检查。"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any


def run(cmd: list[str], cwd: Path) -> str:
    try:
        return subprocess.check_output(cmd, cwd=cwd, text=True, stderr=subprocess.DEVNULL).strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return ""


def git_value(repo: Path, *args: str) -> str:
    return run(["git", *args], repo)


def is_git_repo(repo: Path) -> bool:
    return git_value(repo, "rev-parse", "--is-inside-work-tree") == "true"


def git_dirty(repo: Path, exclude_lto: bool = True) -> bool:
    args = ["git", "status", "--porcelain", "--", "."]
    if exclude_lto:
        args.append(":(exclude).lto")
    return bool(run(args, repo))


def git_head(repo: Path) -> str:
    return git_value(repo, "rev-parse", "HEAD") or "unknown"


def git_branch(repo: Path) -> str:
    return git_value(repo, "branch", "--show-current") or "unknown"


def git_commit_exists(repo: Path, ref: str) -> bool:
    cmd = ["git", "cat-file", "-e", f"{ref}^{{commit}}"]
    return subprocess.run(cmd, cwd=repo, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def is_ancestor(repo: Path, ancestor: str, descendant: str) -> bool:
    """检查 ancestor 是否为 descendant 的祖先 commit。"""
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant],
        cwd=repo, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return result.returncode == 0


def git_identity(repo: Path) -> tuple[str, str]:
    """读取仓库真实 git identity，缺失则返回空串。"""
    return git_value(repo, "config", "user.name"), git_value(repo, "config", "user.email")


def auto_commit_lto(
    repo: Path,
    message: str,
    paths: list[str] | None = None,
    *,
    enabled: bool = False,
) -> dict[str, object]:
    """可选地把指定路径的改动提交到 git。

    设计原则（修正 handoff P0-2/P0-3 的越权问题）：
    - 默认 enabled=False：不自动 commit，只打印提示，把提交权还给用户。
    - 不伪造 author 身份：用仓库真实 git identity；缺失则拒绝 commit。
    - 失败不静默吞：returncode 非零时打印并在返回值里标记。

    返回 {"action": "...", "ok": bool, "detail": str}，action ∈
    {"disabled", "no-changes", "no-identity", "committed", "failed", "not-git"}。
    """
    paths = paths or [".lto"]
    if not is_git_repo(repo):
        return {"action": "not-git", "ok": False, "detail": "not a git worktree"}

    # 是否有目标路径的改动（staged 或 unstaged）
    staged = subprocess.run(
        ["git", "diff", "--cached", "--quiet", "--", *paths], cwd=repo, capture_output=True
    )
    unstaged = subprocess.run(
        ["git", "diff", "--quiet", "--", *paths], cwd=repo, capture_output=True
    )
    untracked = run(["git", "ls-files", "--others", "--exclude-standard", "--", *paths], repo)
    has_changes = staged.returncode != 0 or unstaged.returncode != 0 or bool(untracked)
    if not has_changes:
        return {"action": "no-changes", "ok": True, "detail": "nothing to commit"}

    if not enabled:
        joined = " ".join(paths)
        print(
            f"[lto] {joined} 有未提交改动；如需提交请运行："
            f"git add {joined} && git commit -m \"{message}\"",
        )
        return {"action": "disabled", "ok": True, "detail": "auto-commit disabled (opt-in)"}

    name, email = git_identity(repo)
    if not name or not email:
        print(
            f"[lto] 跳过 auto-commit：仓库未配置 git user.name/email，"
            f"不伪造身份。请手动提交 {' '.join(paths)}。"
        )
        return {"action": "no-identity", "ok": False, "detail": "missing git identity"}

    add = subprocess.run(["git", "add", "--", *paths], cwd=repo, capture_output=True, text=True)
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip()
        print(f"[lto] auto-commit 失败（git add，rc={add.returncode}）：{detail}")
        return {"action": "failed", "ok": False, "detail": detail}

    commit = subprocess.run(
        ["git", "commit", "-m", message], cwd=repo, capture_output=True, text=True
    )
    if commit.returncode != 0:
        detail = (commit.stderr or commit.stdout).strip()
        print(f"[lto] auto-commit 失败（git commit，rc={commit.returncode}）：{detail}")
        return {"action": "failed", "ok": False, "detail": detail}
    return {"action": "committed", "ok": True, "detail": message}


def head_drift(repo: Path, recorded_head: str) -> str:
    """检测 HEAD 漂移类型。

    返回：'none' | 'forward' | 'rewrite' | 'unreachable'
    - none: HEAD 不变
    - forward: 旧 HEAD 是当前 HEAD 祖先（正常前进）
    - rewrite: 旧 HEAD 不可达但 commit 存在（rebase/reset）
    - unreachable: 旧 HEAD 不存在
    """
    actual = git_head(repo)
    if not recorded_head or recorded_head == "unknown":
        return "none"
    if recorded_head == actual:
        return "none"
    if not git_commit_exists(repo, recorded_head):
        return "unreachable"
    if is_ancestor(repo, recorded_head, actual):
        return "forward"
    return "rewrite"


def task_touched_files(state: dict[str, Any]) -> list[str]:
    """Return safe repo-relative paths recorded in task touched_files."""
    paths: list[str] = []
    seen: set[str] = set()
    for task in state.get("tasks", []) or []:
        for raw in task.get("touched_files", []) or []:
            if not isinstance(raw, str) or not raw.strip():
                continue
            path = Path(raw)
            if path.is_absolute() or ".." in path.parts:
                continue
            rel = path.as_posix()
            if rel not in seen:
                seen.add(rel)
                paths.append(rel)
    return paths


def changed_paths_between(repo: Path, old_head: str, new_head: str, paths: list[str]) -> list[str]:
    """Return changed paths between two commits, restricted to pathspecs."""
    if not paths:
        return []
    result = run(["git", "diff", "--name-only", old_head, new_head, "--", *paths], repo)
    return [line for line in result.splitlines() if line]


def task_file_drift(repo: Path, old_head: str, new_head: str, state: dict[str, Any]) -> dict[str, Any]:
    """Shared commit-to-commit drift detector for task-owned files.

    The detector is intentionally bounded to task touched_files. It does not
    inspect the whole repo and does not inspect dirty worktree changes.
    """
    tasks = list(state.get("tasks", []) or [])
    touched = task_touched_files(state)
    changed = changed_paths_between(repo, old_head, new_head, touched)
    return {
        "has_tasks": bool(tasks),
        "touched_files": touched,
        "missing_touched_files": bool(tasks) and not touched,
        "changed_paths": changed,
    }
