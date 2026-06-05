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


# ──────────────────── cross-run aggregation ────────────────────
#
# "越用越聪明" needs memory across runs, not just within one run.  A single
# run's summary cannot answer "you have hit dirty-closeout 5 times".  These
# helpers read every .lto/<run-id>/interventions.jsonl in the current repo
# and aggregate by category.
#
# Privacy/scope: current repo only (no cross-repo aggregation).  Reads the
# same already-redacted jsonl files; adds no new data surface.

# Advice is keyed by category.  It is advisory text for the host agent, never
# a command to obey.  Categories without advice still get counted.
_FRICTION_ADVICE = {
    "dirty_closeout_blocked": (
        "Commit or stash code before closeout; use `--no-changelog` for "
        "post-commit admin closeout."
    ),
    "force_closeout": (
        "Repeated --force suggests a gate may be too strict, or these runs "
        "are genuinely exceptional. Decide which; do not normalize --force."
    ),
    "superseded_blocker": (
        "Stale blockers keep appearing on done tasks. This is the harness "
        "auto-cleaning them — informational, no action needed."
    ),
}


def aggregate_across_runs(repo: Path) -> dict[str, dict[str, Any]]:
    """Aggregate intervention events across every run in this repo's .lto/.

    Returns {category: {"runs": int, "events": int, "avoided": int,
    "candidates": int, "human": int}} where ``runs`` counts distinct run-ids
    that recorded at least one event in that category.
    """
    lto_root = repo / ".lto"
    if not lto_root.is_dir():
        return {}

    by_cat: dict[str, dict[str, Any]] = {}
    for run_dir in sorted(lto_root.iterdir()):
        if not run_dir.is_dir():
            continue
        try:
            run_id = st.validate_run_id(run_dir.name)
        except (ValueError, SystemExit):
            continue  # skip "current" symlink target name / malformed dirs
        seen_cats: set[str] = set()
        for ev in read(repo, run_id):
            cat = ev.get("category")
            if not isinstance(cat, str):
                continue
            agg = by_cat.setdefault(
                cat, {"runs": 0, "events": 0, "avoided": 0, "candidates": 0, "human": 0}
            )
            agg["events"] += 1
            if cat not in seen_cats:
                agg["runs"] += 1
                seen_cats.add(cat)
            etype = ev.get("type")
            if etype == "avoided_intervention":
                agg["avoided"] += 1
            elif etype == "intervention_candidate":
                agg["candidates"] += 1
            elif etype == "human_intervention":
                agg["human"] += 1
    return by_cat


def recurring_friction(repo: Path, *, min_runs: int = 2) -> list[dict[str, Any]]:
    """Categories that caused friction across >= min_runs distinct runs.

    Threshold is by distinct run count, not raw event count: repeated events
    within one run are one recurring pattern, not many.  ``avoided`` events are
    the harness helping (not friction the human should act on) and are excluded
    from the friction trigger, though their count is still surfaced.
    """
    out: list[dict[str, Any]] = []
    for cat, agg in aggregate_across_runs(repo).items():
        # avoided_intervention means the harness silently helped — not friction
        # the human needs to fix.  Only candidate/human events count toward the
        # "you keep hitting this" trigger.
        friction_runs = agg["candidates"] + agg["human"]
        if agg["runs"] >= min_runs and friction_runs > 0:
            out.append({
                "category": cat,
                "runs": agg["runs"],
                "events": agg["events"],
                "candidates": agg["candidates"],
                "human": agg["human"],
                "avoided": agg["avoided"],
                "advice": _FRICTION_ADVICE.get(cat, ""),
            })
    out.sort(key=lambda x: (-x["runs"], -x["events"], x["category"]))
    return out


def render_cross_run_advisory(repo: Path, *, min_runs: int = 2) -> str:
    """Markdown advisory for the host agent. Empty string if nothing recurs.

    Advisory only: it reports evidence and a hint, it does not change routing.
    """
    friction = recurring_friction(repo, min_runs=min_runs)
    if not friction:
        return ""
    lines = ["## Recurring Friction (cross-run)", ""]
    lines.append(
        "These intervention patterns recurred across multiple past runs in "
        "this repo. Advisory only — evidence and a hint, not a routing order."
    )
    lines.append("")
    for f in friction:
        lines.append(f"- **{f['category']}** — seen in {f['runs']} runs ({f['events']} events)")
        if f["advice"]:
            lines.append(f"  - Hint: {f['advice']}")
    return "\n".join(lines)


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
