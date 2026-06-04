"""证据记录：task 执行产出的结构化证据。"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def iso_now() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def record_evidence(
    kind: str,
    command: str | list[str],
    cwd: str,
    rc: int,
    head_before: str,
    head_after: str | None = None,
    stdout_artifact: str | None = None,
    stderr_artifact: str | None = None,
    summary: str = "",
    verified_by: str = "runner",
    started_at: str | None = None,
    ended_at: str | None = None,
) -> dict[str, Any]:
    """构造一条 evidence 记录。"""
    if head_after is None:
        head_after = head_before

    return {
        "kind": kind,
        "command": command if isinstance(command, str) else " ".join(command),
        "argv": command if isinstance(command, list) else [command],
        "cwd": cwd,
        "rc": rc,
        "started_at": started_at or iso_now(),
        "ended_at": ended_at or iso_now(),
        "head_before": head_before,
        "head_after": head_after,
        "stdout_artifact": stdout_artifact,
        "stderr_artifact": stderr_artifact,
        "summary": summary,
        "verified_by": verified_by,
    }


def evidence_summary(evidence: dict[str, Any]) -> str:
    """单条 evidence 的人类可读摘要。"""
    status = "PASS" if evidence["rc"] == 0 else f"FAIL(rc={evidence['rc']})"
    return f"[{evidence['kind']}] {status} {evidence.get('summary', evidence.get('command', '')[:80])}"


def artifacts_from_evidence(evidence_list: list[dict[str, Any]]) -> list[str]:
    """从 evidence 列表提取 artifact 路径列表。"""
    artifacts = []
    for ev in evidence_list:
        for key in ("stdout_artifact", "stderr_artifact"):
            if ev.get(key):
                artifacts.append(ev[key])
    return artifacts
