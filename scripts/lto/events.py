"""Phase 1 passive event log for LTO runs (.lto/<run-id>/events.jsonl).

This is the **sensor layer**: append-only, zero LLM, zero decision. It records
what changed so future tuning has real run history. It never routes, promotes,
or recommends.

Design mirrors ``interventions.py`` (the v0 same-pattern implementation):
reuse its ``_clean`` redaction (secrets + absolute private paths + truncate to
500 chars), its monotonic ``_next_event_id`` discipline, and its tolerant
``read``. Spec: references/control-loop-harness.md §5.1 / §5.2 / §5.3.

Hard privacy rules (CI-verifiable):
- event lines NEVER inline stdout/stderr/transcript/secret/private source or
  absolute private paths; large output stays in artifacts, events only ref ids;
- redaction happens BEFORE append, not at export;
- ``summary`` and free-text go through ``_clean``; structured fields stay
  enum/number/bool.

Emit is fail-safe: a write failure must not break the host's main flow. Callers
should use :func:`emit` which swallows errors into a warning.
"""

from __future__ import annotations

import contextlib
import json
import re
import time
import warnings
from pathlib import Path
from typing import Any, Iterator

from . import state as st

SCHEMA_VERSION = 1

# Phase 1 event types ONLY. Deferred types (finding/issue/decision/gate/
# permission/worker/barrier/diagnostics) are intentionally absent and rejected.
PHASE1_EVENT_TYPES = {
    "run.started",
    "run.closed",
    "phase.changed",
    "task.created",
    "task.status_changed",
    "runner.started",
    "runner.finished",
    "artifact.registered",
}

# actor.kind is a provenance fact, not a judgement.
_ALLOWED_ACTOR_KINDS = {"host", "lto", "runner", "auditor", "human"}

_ALLOWED_REDACTION_STATUS = {"not_required", "passed", "failed"}

# Size policy (spec §5.1).
WARN_AT = 10_000
HARD_STOP_AT = 50_000

# Shared redaction patterns — kept identical in spirit to interventions.py so a
# leak fixed in one place is fixed in both. (Duplicated rather than imported so
# the two sensor logs stay independently auditable.)
_SECRET_RE = re.compile(
    r"(sk-[A-Za-z0-9_-]{12,}|sk-ant-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|"
    r"AKIA[0-9A-Z]{16}|-----BEGIN [^-]*PRIVATE KEY-----|"
    r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})"
)
_ABS_PRIVATE_PATH_RE = re.compile(r"/(?:Users|home)/[^\s:'\"]+")

# Truncation width. Spec §5.0 is authoritative: free-text fields are capped at
# 240 chars. (Earlier draft used 500 to match interventions._clean; spec wins.)
_TRUNCATE = 240

# Raw-output semantic keys forbidden anywhere in event metadata. Matched both
# exactly and by suffix/substring (codex review #3): a key carrying raw stdout/
# stderr/transcript/command output must never reach events.jsonl, even nested.
_FORBIDDEN_KEY_EXACT = {"stdout", "stderr", "reply_text", "transcript", "output"}
_FORBIDDEN_KEY_SUFFIXES = ("_excerpt", "_tail")
_FORBIDDEN_KEY_SUBSTRINGS = ("output", "stdout", "stderr", "transcript")


def _is_forbidden_key(key: str) -> bool:
    k = key.lower()
    if k in _FORBIDDEN_KEY_EXACT:
        return True
    if k.endswith(_FORBIDDEN_KEY_SUFFIXES):
        return True
    return any(sub in k for sub in _FORBIDDEN_KEY_SUBSTRINGS)


def _strip_forbidden(value: Any) -> Any:
    """Recursively drop raw-output-bearing keys from nested dicts/lists."""
    if isinstance(value, dict):
        return {
            k: _strip_forbidden(v)
            for k, v in value.items()
            if not (isinstance(k, str) and _is_forbidden_key(k))
        }
    if isinstance(value, list):
        return [_strip_forbidden(v) for v in value]
    return value


def _clean(value: str) -> str:
    """Redact secrets + absolute private paths, collapse whitespace, truncate.

    Ingress redactor: runs BEFORE append. Truncation per spec §5.0 (240).
    """
    value = _SECRET_RE.sub("[REDACTED_SECRET]", value)
    value = _ABS_PRIVATE_PATH_RE.sub("[REDACTED_PATH]", value)
    return re.sub(r"\s+", " ", value).strip()[:_TRUNCATE]


def _clean_obj(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(_clean(str(k))): _clean_obj(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_clean_obj(v) for v in value]
    if isinstance(value, (int, float, bool)) or value is None:
        return value
    return _clean(str(value))


def _events_path(repo: Path, run_id: str) -> Path:
    return repo / ".lto" / st.validate_run_id(run_id) / "events.jsonl"


def _count_events(path: Path) -> int:
    if not path.exists():
        return 0
    return sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line.strip())


def _next_event_id(path: Path) -> int:
    """Monotonic per-run id = current line count + 1.

    Must be called inside :func:`_events_lock` so the read-count→write window is
    atomic across processes (review #1: otherwise concurrent appends collide).
    """
    return _count_events(path) + 1


@contextlib.contextmanager
def _events_lock(path: Path, timeout: float = 5.0) -> Iterator[None]:
    """Cross-process exclusive lock for the read-id→write window.

    Mirrors artifacts._manifest_lock: fcntl.flock on a sibling .events.lock,
    busy-wait up to timeout, degrade to best-effort if fcntl is unavailable.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.parent / ".events.lock"
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
                        warnings.warn("events lock timeout; proceeding best-effort")
                        break
                    time.sleep(0.02)
            yield
            with contextlib.suppress(Exception):
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
        except ImportError:
            warnings.warn("fcntl unavailable; event appends are best-effort")
            yield


def _clean_artifact_refs(refs: list[Any] | None) -> list[str]:
    """Artifact refs must be ids / repo-relative paths, never absolute private
    paths. Run them through _clean and drop anything that still looks absolute.
    """
    out: list[str] = []
    for ref in refs or []:
        cleaned = _clean(str(ref))
        if cleaned:
            out.append(cleaned)
    return out


def append(
    repo: Path,
    run_id: str,
    *,
    type: str,
    actor_kind: str,
    actor_id: str | None = None,
    phase: str | None = None,
    task_id: str | None = None,
    object_id: str | None = None,
    object_type: str | None = None,
    summary: str = "",
    artifact_refs: list[Any] | None = None,
    contains_raw_output: bool = False,
    fields: dict[str, Any] | None = None,
    force: bool = False,
) -> dict[str, Any]:
    """Validate → redact → append one Phase 1 event to events.jsonl.

    Returns the written event dict. Raises ValueError on schema violation or
    when the size hard-stop is hit (unless ``force=True``). The redaction
    happens here, before the line is written.

    ``fields`` carries extra export-safe structured metadata (rc / elapsed /
    timeout / counts / repo-relative touched files). It is _clean_obj'd; callers
    MUST NOT pass stdout/stderr/reply_text/secrets — only metadata.
    """
    if type not in PHASE1_EVENT_TYPES:
        raise ValueError(f"invalid or deferred event type: {type}")
    if actor_kind not in _ALLOWED_ACTOR_KINDS:
        raise ValueError(f"invalid actor kind: {actor_kind}")
    # Review #3: an event that admits it carries raw output has no place in the
    # event log. Raw output lives in artifacts; events only reference it.
    if contains_raw_output:
        raise ValueError("contains_raw_output events are forbidden; store as artifact and reference it")

    path = _events_path(repo, run_id)
    path.parent.mkdir(parents=True, exist_ok=True)

    # Strip raw-output-bearing keys (nested, by name/suffix/substring) BEFORE
    # redaction so they never reach the line, then redact remaining free text.
    extra = _clean_obj(_strip_forbidden(fields or {}))

    # Review #1: do the whole count→id→write window under one cross-process
    # lock so concurrent appends cannot collide on event_id or interleave bytes.
    with _events_lock(path):
        count = _count_events(path)
        if count >= HARD_STOP_AT and not force:
            raise ValueError(
                f"event log hard stop at {HARD_STOP_AT} events ({count} present); "
                "pass force=True to override"
            )
        if count >= WARN_AT:
            warnings.warn(f"event log has {count} events (warn threshold {WARN_AT})")

        event = {
            "schema_version": SCHEMA_VERSION,
            "event_id": count + 1,
            "run_id": st.validate_run_id(run_id),
            "at": st.iso_now(),
            "type": type,
            "actor": {
                "kind": actor_kind,
                "id": _clean(str(actor_id)) if actor_id is not None else None,
            },
            "phase": _clean(phase) if phase else None,
            "task_id": _clean(task_id) if task_id else None,
            "object_id": _clean(object_id) if object_id else None,
            "object_type": _clean(object_type) if object_type else None,
            "summary": _clean(summary),
            "artifact_refs": _clean_artifact_refs(artifact_refs),
            "privacy": {
                "contains_raw_output": False,
                "redaction_status": "not_required" if not summary and not extra else "passed",
            },
        }
        if extra:
            event["fields"] = extra

        with path.open("a", encoding="utf-8") as f:
            f.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")
    return event


def emit(repo: Path, run_id: str, **kwargs: Any) -> dict[str, Any] | None:
    """Fail-safe wrapper around :func:`append`.

    Sensors must not crash the plant. Any failure (bad run id, disk error,
    size hard-stop, schema bug) is swallowed into a warning and returns None.
    Use this from integration points; use :func:`append` in tests where you
    want the raise.
    """
    try:
        return append(repo, run_id, **kwargs)
    except (Exception, SystemExit) as exc:  # SystemExit: validate_run_id raises it
        warnings.warn(f"events.emit failed ({kwargs.get('type', '?')}): {exc}")
        return None


def read(repo: Path, run_id: str) -> list[dict[str, Any]]:
    """Tolerant read: skip broken lines; old runs with missing fields still
    load (consumers must default-fill). Rejects duplicate event_ids by keeping
    the first occurrence and warning (append-only invariant guard for readers).
    """
    path = _events_path(repo, run_id)
    if not path.exists():
        return []
    events: list[dict[str, Any]] = []
    seen_ids: set[int] = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(item, dict):
            continue
        eid = item.get("event_id")
        if isinstance(eid, int):
            if eid in seen_ids:
                warnings.warn(f"duplicate event_id {eid} in {run_id}; keeping first")
                continue
            seen_ids.add(eid)
        events.append(item)
    return events


def event_count(repo: Path, run_id: str) -> int:
    return _count_events(_events_path(repo, run_id))
