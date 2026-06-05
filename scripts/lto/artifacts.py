"""Artifact manifest helpers for LTO runs.

Schema v1 is intentionally small: repo-relative paths plus enough producer
metadata for another host runtime to resume without reading the transcript.
Future schema upgrades are a known debt; keep load tolerant and write strict.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import tempfile
import time
import warnings
from pathlib import Path
from typing import Any, Iterator

from . import state as st
from .artifact_synthesis import synthesize_manifest

SCHEMA_VERSION = 1

KNOWN_KINDS = {
    "state_json", "run_state_md", "audit_ledger", "audit_brief",
    "audit_reply", "audit_findings_json", "risk_discovery_reply",
    "decision_brief", "decision_reply", "decision_host_brief",
    "decision_record",
    "evidence_stdout", "evidence_stderr", "judge_verdict", "handoff",
    "changelog", "interventions", "preflight_snapshot", "other",
}

RUN_OUTSIDE_ALLOWLIST = {"CHANGELOG.md"}


def manifest_path(repo: Path, run_id: str) -> Path:
    return repo / ".lto" / st.validate_run_id(run_id) / "artifacts.json"


def load_manifest(
    repo: Path,
    run_id: str,
    *,
    synthesize: bool = True,
    persist_synthesized: bool = False,
    state: dict | None = None,
) -> dict[str, Any]:
    path = manifest_path(repo, run_id)
    if path.exists():
        return _normalize_manifest(json.loads(path.read_text(encoding="utf-8")), run_id)
    if not synthesize:
        return _empty_manifest(run_id)

    manifest = synthesize_manifest(repo, run_id, state)
    if persist_synthesized and (state or {}).get("current_phase") != "closed":
        save_manifest(repo, run_id, manifest)
    return manifest


def save_manifest(repo: Path, run_id: str, manifest: dict[str, Any]) -> None:
    path = manifest_path(repo, run_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    manifest = _normalize_manifest(manifest, run_id)
    manifest["updated_at"] = st.iso_now()
    with _manifest_lock(path):
        _atomic_write_json(path, manifest)


def init_manifest(repo: Path, run_id: str, state: dict) -> dict[str, Any]:
    state["artifacts"] = {"manifest": f".lto/{run_id}/artifacts.json"}
    manifest = _empty_manifest(run_id)
    save_manifest(repo, run_id, manifest)

    run_dir = repo / ".lto" / run_id
    for rel, kind, summary in (
        ("state.json", "state_json", "machine state"),
        ("run-state.md", "run_state_md", "human-readable run state"),
        ("audit-ledger.md", "audit_ledger", "audit convergence ledger"),
    ):
        path = run_dir / rel
        if path.exists():
            register_path(
                repo, run_id, path, kind=kind, producer="lto.commands.start",
                state=state, summary=summary,
            )
    return load_manifest(repo, run_id, synthesize=False)


def register_path(
    repo: Path,
    run_id: str,
    path: Path | str,
    *,
    kind: str,
    producer: str,
    state: dict | None = None,
    summary: str = "",
    task_id: str | None = None,
    job_id: str | None = None,
    runner: str | None = None,
    phase: str | None = None,
    consumed_by: list[str] | None = None,
    tags: list[str] | None = None,
) -> dict[str, Any]:
    mpath = manifest_path(repo, run_id)
    with _manifest_lock(mpath):
        manifest = (
            _read_manifest_unlocked(mpath, run_id)
            if mpath.exists()
            else synthesize_manifest(repo, run_id, state)
        )
        entry = _entry_for_path(
            repo, run_id, path, kind=kind, producer=producer, state=state,
            summary=summary, task_id=task_id, job_id=job_id, runner=runner,
            phase=phase, consumed_by=consumed_by, tags=tags,
        )
        _upsert(manifest, entry)
        manifest["updated_at"] = st.iso_now()
        _atomic_write_json(mpath, manifest)
        return entry


def write_text(
    repo: Path,
    run_id: str,
    run_relative_path: str,
    content: str,
    *,
    kind: str,
    producer: str,
    state: dict | None = None,
    summary: str = "",
    **meta: Any,
) -> str:
    rel = Path(run_relative_path)
    if rel.is_absolute() or ".." in rel.parts:
        raise ValueError(f"invalid run-relative artifact path: {run_relative_path!r}")
    path = repo / ".lto" / st.validate_run_id(run_id) / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    register_path(repo, run_id, path, kind=kind, producer=producer, state=state, summary=summary, **meta)
    return str(path.relative_to(repo))


def recent(repo: Path, run_id: str, *, limit: int = 8, kinds: set[str] | None = None) -> list[dict[str, Any]]:
    manifest = load_manifest(repo, run_id)
    entries = manifest.get("artifacts", [])
    if kinds is not None:
        entries = [e for e in entries if e.get("kind") in kinds]
    return sorted(entries, key=lambda e: e.get("created_at", ""), reverse=True)[:limit]


def render_markdown(entries: list[dict[str, Any]], *, title: str = "Artifacts") -> str:
    lines = [f"## {title}", ""]
    if not entries:
        lines.append("- none")
        return "\n".join(lines)
    for e in entries:
        path = e.get("relative_path", "?")
        summary = e.get("summary", "")
        suffix = f" — {summary}" if summary else ""
        marker = " (synthesized)" if e.get("source") == "synthesized" else ""
        lines.append(f"- `{e.get('kind', 'other')}`: `{path}`{suffix}{marker}")
    return "\n".join(lines)


def _empty_manifest(run_id: str, *, synthesized: bool = False) -> dict[str, Any]:
    now = st.iso_now()
    return {
        "schema_version": SCHEMA_VERSION,
        "run_id": st.validate_run_id(run_id),
        "synthesized": synthesized,
        "created_at": now,
        "updated_at": now,
        "artifacts": [],
    }


def _normalize_manifest(manifest: dict[str, Any], run_id: str) -> dict[str, Any]:
    base = _empty_manifest(run_id, synthesized=bool(manifest.get("synthesized", False)))
    base.update({k: v for k, v in manifest.items() if k != "artifacts"})
    base["schema_version"] = SCHEMA_VERSION
    base["run_id"] = st.validate_run_id(run_id)
    base["artifacts"] = list(manifest.get("artifacts", []))
    return base


def _read_manifest_unlocked(path: Path, run_id: str) -> dict[str, Any]:
    if path.exists():
        return _normalize_manifest(json.loads(path.read_text(encoding="utf-8")), run_id)
    return _empty_manifest(run_id)


def _entry_for_path(repo: Path, run_id: str, path: Path | str, **meta: Any) -> dict[str, Any]:
    kind = _normalize_kind(str(meta.pop("kind")))
    rel_path, run_rel = _validate_artifact_path(repo, run_id, path, kind)
    state = meta.pop("state", None) or {}
    volatile = kind == "changelog" or bool(meta.pop("volatile", False))
    entry = {
        "id": "af_" + hashlib.sha1(f"{kind}|{rel_path}".encode("utf-8")).hexdigest()[:16],
        "kind": kind,
        "relative_path": rel_path,
        "run_relative_path": run_rel,
        "producer": meta.pop("producer"),
        "host_runtime": state.get("host_runtime", "unknown"),
        "runner": meta.pop("runner", None),
        "task_id": meta.pop("task_id", None),
        "job_id": meta.pop("job_id", None),
        "phase": meta.pop("phase", None) or state.get("current_phase", "unknown"),
        "source": meta.pop("source", "registered"),
        "volatile": volatile,
        "created_at": st.iso_now(),
        "summary": st.single_line(meta.pop("summary", "")),
        "consumed_by": list(meta.pop("consumed_by", []) or []),
        "tags": list(meta.pop("tags", []) or []),
    }
    target = repo / rel_path
    if target.exists() and not volatile:
        data = target.read_bytes()
        entry["bytes"] = len(data)
        entry["sha256"] = hashlib.sha256(data).hexdigest()
    return entry


def _normalize_kind(kind: str) -> str:
    if kind in KNOWN_KINDS:
        return kind
    warnings.warn(f"unknown artifact kind {kind!r}; storing as 'other'")
    return "other"


def _validate_artifact_path(repo: Path, run_id: str, path: Path | str, kind: str) -> tuple[str, str]:
    repo = repo.resolve()
    path = Path(path)
    if not path.is_absolute():
        path = repo / path
    resolved = path.resolve()
    try:
        rel = resolved.relative_to(repo).as_posix()
    except ValueError as exc:
        raise ValueError(f"artifact path outside repo: {path}") from exc
    run_prefix = f".lto/{st.validate_run_id(run_id)}/"
    if rel in RUN_OUTSIDE_ALLOWLIST and kind == "changelog":
        return rel, rel
    if kind == "decision_record" and _is_decision_record_path(rel):
        return rel, rel
    if not rel.startswith(run_prefix):
        raise ValueError(f"artifact path outside run dir: {rel}")
    return rel, rel[len(run_prefix):]


def _is_decision_record_path(rel: str) -> bool:
    prefix = "docs/decisions/"
    if not rel.startswith(prefix) or not rel.endswith(".md"):
        return False
    name = rel[len(prefix):]
    return bool(name) and "/" not in name


def _upsert(manifest: dict[str, Any], entry: dict[str, Any]) -> None:
    key = (entry["kind"], entry["relative_path"])
    entries = manifest.setdefault("artifacts", [])
    for idx, current in enumerate(entries):
        if (current.get("kind"), current.get("relative_path")) == key:
            merged = {**current, **entry}
            if current.get("created_at"):
                merged["created_at"] = current["created_at"]
            entries[idx] = merged
            return
    entries.append(entry)


def _atomic_write_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


@contextlib.contextmanager
def _manifest_lock(path: Path, timeout: float = 5.0) -> Iterator[None]:
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.parent / ".artifacts.lock"
    with open(lock_path, "a+", encoding="utf-8") as lock_file:
        try:
            import fcntl
            deadline = time.monotonic() + timeout
            while True:
                try:
                    fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        warnings.warn("artifact manifest lock timeout; proceeding best-effort")
                        break
                    time.sleep(0.05)
            yield
            with contextlib.suppress(Exception):
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
        except ImportError:
            warnings.warn("fcntl unavailable; artifact manifest writes are best-effort")
            yield
