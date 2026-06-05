"""Human-intervention telemetry for LTO runs.

Small, privacy-safe log for measuring avoidable human work before building
larger telemetry.  No raw stdout/stderr, no absolute paths, no secrets.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from . import state as st

_ALLOWED_TYPES = {"human_intervention", "intervention_candidate", "avoided_intervention"}
_ALLOWED_CATEGORIES = {
    "force_closeout",
    "dirty_closeout_blocked",
    "superseded_blocker",
}
_SECRET_RE = re.compile(
    r"(sk-[A-Za-z0-9_-]{12,}|sk-ant-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|"
    r"AKIA[0-9A-Z]{16}|-----BEGIN [^-]*PRIVATE KEY-----|"
    r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})"
)
_ABS_PRIVATE_PATH_RE = re.compile(r"/(?:Users|home)/[^\s:'\"]+")


def append(
    repo: Path,
    run_id: str,
    *,
    type: str,
    category: str,
    reason: str,
    source: str,
    meaningful: bool,
    avoidable: bool,
    preventable: bool,
    details: dict[str, Any] | None = None,
    dedupe_key: str | None = None,
) -> dict[str, Any]:
    """Append one privacy-safe intervention event.

    If dedupe_key already exists in this run log, return the existing event and
    avoid inflating counts on repeated judge/closeout calls.
    """
    if type not in _ALLOWED_TYPES:
        raise ValueError(f"invalid intervention type: {type}")
    if category not in _ALLOWED_CATEGORIES:
        raise ValueError(f"invalid intervention category: {category}")

    path = repo / ".lto" / run_id / "interventions.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    if dedupe_key:
        for existing in read(repo, run_id):
            if existing.get("dedupe_key") == dedupe_key:
                return existing

    event = {
        "schema_version": 1,
        "event_id": _next_event_id(path),
        "at": st.iso_now(),
        "type": type,
        "category": category,
        "source": _clean(source),
        "reason": _clean(reason),
        "meaningful": bool(meaningful),
        "avoidable": bool(avoidable),
        "preventable": bool(preventable),
        "details": _clean_obj(details or {}),
    }
    if dedupe_key:
        event["dedupe_key"] = _clean(dedupe_key)

    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")
    return event


def read(repo: Path, run_id: str) -> list[dict[str, Any]]:
    path = repo / ".lto" / run_id / "interventions.jsonl"
    if not path.exists():
        return []
    events: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(item, dict):
            events.append(item)
    return events


def summarize(repo: Path, run_id: str) -> dict[str, int]:
    events = read(repo, run_id)
    return {
        "total": len(events),
        "meaningful": sum(1 for e in events if e.get("meaningful") is True),
        "avoidable": sum(1 for e in events if e.get("avoidable") is True),
        "preventable": sum(1 for e in events if e.get("preventable") is True),
        "avoided": sum(1 for e in events if e.get("type") == "avoided_intervention"),
        "candidates": sum(1 for e in events if e.get("type") == "intervention_candidate"),
    }


def render_summary(repo: Path, run_id: str) -> str:
    s = summarize(repo, run_id)
    if s["total"] == 0:
        return "No intervention events recorded."
    return (
        f"Interventions: total={s['total']}, meaningful={s['meaningful']}, "
        f"avoidable={s['avoidable']}, preventable={s['preventable']}, "
        f"avoided={s['avoided']}, candidates={s['candidates']}."
    )


def _next_event_id(path: Path) -> int:
    if not path.exists():
        return 1
    return sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line.strip()) + 1


def _clean_obj(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(_clean(k)): _clean_obj(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_clean_obj(v) for v in value]
    if isinstance(value, (int, float, bool)) or value is None:
        return value
    return _clean(str(value))


def _clean(value: str) -> str:
    value = _SECRET_RE.sub("[REDACTED_SECRET]", value)
    value = _ABS_PRIVATE_PATH_RE.sub("[REDACTED_PATH]", value)
    return re.sub(r"\s+", " ", value).strip()[:500]
