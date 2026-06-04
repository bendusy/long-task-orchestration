"""命令执行内核：执行一条命令 → 存 artifact → 造 evidence。

runner / parallel / pipeline 三个执行器共用本模块，避免三套不一致的
evidence 契约。本模块只负责「执行 + 落 artifact + 造 evidence dict」，
不碰 state.json 更新（更新逻辑各执行器自己做，粒度不同）。
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

from . import state as st
from . import git_state as gs
from . import evidence as ev
from . import artifacts as af


def save_artifact(repo: Path, run_id: str, task_id: str, suffix: str, content: str) -> str | None:
    """把命令输出落成 artifact 文件，返回相对仓库根的路径；空内容返回 None。"""
    if not content:
        return None
    artifact_dir = repo / ".lto" / run_id / "evidence"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    safe_suffix = suffix.replace("/", "-").replace("\\", "-").replace(" ", "-")
    # Path(...).name 强制截断任何残留路径分隔符，纵深防止穿越
    filename = Path(f"{task_id}-{ts}-{safe_suffix}.txt").name
    path = artifact_dir / filename
    path.write_text(content, encoding="utf-8")
    kind = "evidence_stderr" if "stderr" in safe_suffix.lower() else "evidence_stdout"
    af.register_path(
        repo, run_id, path, kind=kind, producer="lto.exec.save_artifact",
        task_id=task_id, summary=f"{task_id} {safe_suffix}", tags=["evidence"],
    )
    return str(path.relative_to(repo))


def run_command(
    repo: Path,
    run_id: str,
    task_id: str,
    *,
    kind: str,
    command: str,
    cwd: Path,
    timeout: int,
    verified_by: str,
    summary: str,
    artifact_suffix: str = "",
) -> tuple[int, dict[str, Any]]:
    """执行一条 shell 命令，落 stdout/stderr artifact，返回 (rc, evidence)。

    超时返回 rc=124 的 evidence（不抛异常，交给调用方处理 task 状态）。
    """
    head_before = gs.git_head(repo)
    started_at = st.iso_now()
    suffix_prefix = f"{artifact_suffix}-" if artifact_suffix else ""

    try:
        proc = subprocess.run(
            command, shell=True, cwd=cwd,
            capture_output=True, text=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        ended_at = st.iso_now()
        evidence = ev.record_evidence(
            kind=kind, command=command, cwd=str(cwd), rc=124,
            head_before=head_before, summary=f"timeout after {timeout}s",
            verified_by=verified_by, started_at=started_at, ended_at=ended_at,
        )
        return 124, evidence

    ended_at = st.iso_now()
    head_after = gs.git_head(repo)
    stdout_artifact = save_artifact(repo, run_id, task_id, f"{suffix_prefix}stdout", proc.stdout)
    stderr_artifact = save_artifact(repo, run_id, task_id, f"{suffix_prefix}stderr", proc.stderr)

    evidence = ev.record_evidence(
        kind=kind, command=command, cwd=str(cwd), rc=proc.returncode,
        head_before=head_before, head_after=head_after,
        stdout_artifact=stdout_artifact, stderr_artifact=stderr_artifact,
        summary=summary, verified_by=verified_by,
        started_at=started_at, ended_at=ended_at,
    )
    return proc.returncode, evidence
