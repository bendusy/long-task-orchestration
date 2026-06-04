"""Projection from local LTO state/artifacts into memory-safe records."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
from pathlib import Path
from typing import Any

from . import artifacts as af
from . import git_state as gs
from . import state as st

SCHEMA_VERSION = 1
MAX_TEXT = 240
MAX_GOAL = 160

SECRET_RE = re.compile(
    r"(?i)(api[_-]?key|token|secret|password|authorization)\s*[:=]\s*['\"]?[^\s'\"]+"
)
ABS_PATH_RE = re.compile(
    r"(/Users/[^\s]+|/Volumes/[^\s]+|/home/[^\s]+|/private/[^\s]+|[A-Za-z]:\\\\[^\s]+)"
)


def build_projection(repo: Path, run_id: str) -> dict[str, Any]:
    repo = repo.resolve()
    run_id = st.validate_run_id(run_id)
    run_dir = repo / ".lto" / run_id
    state_path = run_dir / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    manifest = af.load_manifest(repo, run_id, state=state)
    records: list[dict[str, Any]] = [
        _project_snapshot(repo, run_id),
        _run_snapshot(repo, run_id, state),
    ]
    records.extend(_task_records(state, repo))
    records.extend(_artifact_records(manifest, repo))
    records.append(_workflow_routing_placeholder(repo, run_id))
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "lto_memory_projection",
        "project_key": _project_key(repo),
        "repo_path": _redact_path(str(repo)),
        "run_id": run_id,
        "generated_at": st.iso_now(),
        "records": records,
    }


def _project_snapshot(repo: Path, run_id: str) -> dict[str, Any]:
    dirty_count, samples = _dirty_summary(repo)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "project_snapshot",
        "project_key": _project_key(repo),
        "repo_path": _redact_path(str(repo)),
        "aliases": _project_aliases(repo),
        "git": {
            "branch": gs.git_branch(repo),
            "head": gs.git_head(repo),
            "dirty": dirty_count > 0,
            "dirty_count": dirty_count,
            "dirty_path_samples": samples,
        },
        "active_lto_run": run_id,
        "latest_closed_lto_run": _latest_closed_run(repo),
        "updated_at": st.iso_now(),
    }


def _run_snapshot(repo: Path, run_id: str, state: dict[str, Any]) -> dict[str, Any]:
    run_dir = repo / ".lto" / run_id
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "lto_run_snapshot",
        "project_key": _project_key(repo),
        "run_id": run_id,
        "request_hash": _hash_text(state.get("original_user_request", "")),
        "goal_redacted": _redact_text(state.get("goal", ""), max_len=MAX_GOAL),
        "why_redacted": _redact_text(state.get("why", "")),
        "done_when_redacted": _redact_text(state.get("done_when", "")),
        "phase": state.get("current_phase", "unknown"),
        "status": _run_status(state),
        "host_runtime": state.get("host_runtime", "unknown"),
        "state_path": f".lto/{run_id}/state.json",
        "manifest_path": f".lto/{run_id}/artifacts.json",
        "audit_ledger_path": f".lto/{run_id}/audit-ledger.md",
        "task_counts": _task_counts(state),
        "next_action_redacted": _redact_text(state.get("next_action", "")),
        "blocked_by": _redact_text(state.get("blocked_by", "none"), max_len=120),
        "artifact_hash": _file_hash(run_dir / "artifacts.json"),
        "state_hash": _file_hash(run_dir / "state.json"),
        "updated_at": st.iso_now(),
    }


def _task_records(state: dict[str, Any], repo: Path) -> list[dict[str, Any]]:
    project_key = _project_key(repo)
    run_id = state.get("run_id", "unknown")
    records = []
    for task in state.get("tasks", []) or []:
        records.append({
            "schema_version": SCHEMA_VERSION,
            "kind": "lto_task_memory",
            "project_key": project_key,
            "run_id": run_id,
            "task_id": task.get("id", ""),
            "title": _redact_text(task.get("title", ""), max_len=160),
            "status": task.get("status", "pending"),
            "phase": task.get("phase", state.get("current_phase", "unknown")),
            "depends_on": list(task.get("depends_on", []) or []),
            "touched_files": _safe_rel_paths(task.get("touched_files", []) or []),
            "commands_run": [_redact_command(c) for c in task.get("commands_run", []) or []],
            "evidence_refs": _evidence_refs(task),
            "blockers": [_redact_text(_blocker_text(b)) for b in task.get("blockers", []) or []],
            "assumptions": [_redact_text(a) for a in task.get("assumptions", []) or []],
            "last_update": task.get("last_update", ""),
        })
    return records


def _artifact_records(manifest: dict[str, Any], repo: Path) -> list[dict[str, Any]]:
    records = []
    for item in manifest.get("artifacts", []) or []:
        records.append({
            "schema_version": SCHEMA_VERSION,
            "kind": "lto_artifact_memory",
            "project_key": _project_key(repo),
            "run_id": manifest.get("run_id", ""),
            "artifact_id": item.get("id", ""),
            "artifact_kind": item.get("kind", "other"),
            "relative_path": item.get("relative_path", ""),
            "producer": item.get("producer", ""),
            "host_runtime": item.get("host_runtime", "unknown"),
            "runner": item.get("runner"),
            "task_id": item.get("task_id"),
            "phase": item.get("phase", "unknown"),
            "summary": _redact_text(item.get("summary", "")),
            "sha256": item.get("sha256", ""),
            "volatile": bool(item.get("volatile", False)),
            "tags": list(item.get("tags", []) or []),
            "created_at": item.get("created_at", ""),
        })
    return records


def _workflow_routing_placeholder(repo: Path, run_id: str) -> dict[str, Any]:
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "workflow_routing_memory",
        "project_key": _project_key(repo),
        "run_id": run_id,
        "schema_only": True,
        "note": "schema placeholder; publish skips this record",
    }


def _project_key(repo: Path) -> str:
    return repo.name or "unknown"


def _project_aliases(repo: Path) -> list[str]:
    aliases = [repo.name]
    if repo.name == "long-task-orchestration":
        aliases.append("lto")
    return sorted(set(a for a in aliases if a))


def _dirty_summary(repo: Path) -> tuple[int, list[str]]:
    try:
        out = subprocess.check_output(
            ["git", "status", "--porcelain", "--", ".", ":(exclude).lto"],
            cwd=repo, text=True, stderr=subprocess.DEVNULL,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return 0, []
    paths = [line[3:].strip() for line in out.splitlines() if len(line) > 3]
    return len(paths), [_redact_path(p) for p in paths[:5]]


def _latest_closed_run(repo: Path) -> str | None:
    base = repo / ".lto"
    if not base.exists():
        return None
    candidates = []
    for sp in base.glob("*/state.json"):
        state = st.load_state(sp)
        if state and state.get("current_phase") == "closed":
            candidates.append((state.get("started_at", ""), state.get("run_id", sp.parent.name)))
    return sorted(candidates)[-1][1] if candidates else None


def _task_counts(state: dict[str, Any]) -> dict[str, int]:
    counts = {status: 0 for status in sorted(st.VALID_TASK_STATUSES)}
    for task in state.get("tasks", []) or []:
        status = task.get("status", "pending")
        counts[status] = counts.get(status, 0) + 1
    return counts


def _run_status(state: dict[str, Any]) -> str:
    if state.get("current_phase") == "closed":
        return "closed"
    if state.get("blocked_by") and state.get("blocked_by") != "none":
        return "blocked"
    return "active"


def _file_hash(path: Path) -> str:
    if not path.exists():
        return ""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _hash_text(value: str) -> str:
    return hashlib.sha256((value or "").encode("utf-8")).hexdigest() if value else ""


def _redact_text(value: Any, *, max_len: int = MAX_TEXT) -> str:
    text = st.single_line(str(value or ""))
    text = SECRET_RE.sub("[redacted-secret]", text)
    text = ABS_PATH_RE.sub("[redacted-path]", text)
    return text[:max_len] + ("..." if len(text) > max_len else "")


def _redact_path(value: str) -> str:
    path = Path(value)
    if path.is_absolute() or ".." in path.parts:
        return "[redacted-path]"
    parts = path.parts
    return "/".join(parts[-2:]) if len(parts) > 2 else path.as_posix()


def _safe_rel_paths(values: list[Any]) -> list[str]:
    safe = []
    for value in values[:20]:
        if not isinstance(value, str):
            continue
        path = Path(value)
        if path.is_absolute() or ".." in path.parts:
            continue
        safe.append(path.as_posix())
    return safe


def _redact_command(command: str) -> str:
    return _redact_text(command, max_len=160)


def _blocker_text(blocker: Any) -> str:
    if isinstance(blocker, dict):
        return blocker.get("reason") or json.dumps(blocker, ensure_ascii=False)
    return str(blocker)


def _evidence_refs(task: dict[str, Any]) -> list[str]:
    refs = []
    for item in task.get("evidence", []) or []:
        if isinstance(item, dict):
            ref = item.get("artifact_id") or item.get("path") or item.get("kind")
            if ref:
                refs.append(_redact_text(ref, max_len=160))
    return refs[:20]
