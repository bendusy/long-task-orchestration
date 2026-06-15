"""Dispatch affordance facts for host-facing LTO briefs.

This module is deliberately mechanical: it lists available dispatch surfaces
and best-effort runner health facts. It does not choose a mode.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any


DEFAULT_RUNNERS = ("codex", "pi", "agy", "claude")

DISPATCH_MODES: tuple[tuple[str, str], ...] = (
    (
        "scheduler",
        "batch/concurrent heterogeneous jobs; exit triage plus healthcheck gate",
    ),
    (
        "delegate.sh",
        "single cross-runtime dispatch; depends on runner health and timeout fit",
    ),
    (
        "autopilot --auto-exec",
        "safe/reversible substeps executed inside the worktree sandbox",
    ),
    (
        "Agent subagent",
        "code-reading and judgment tasks through the host tool's subagent surface",
    ),
    (
        "host direct code reading",
        "single-fact verification by the current host agent",
    ),
)


def render_dispatch_affordances(repo: Path, probe_timeout_sec: int = 5) -> list[str]:
    """Return a Markdown section with dispatch modes and runner health facts."""
    lines = [
        "### Dispatch Modes (facts only -- pick by task shape; matching is YOUR job)",
        "",
    ]
    for name, description in DISPATCH_MODES:
        lines.append(f"- `{name}`: {description}")
    lines.append("- Current runner health:")
    for entry in runner_health_facts(repo, probe_timeout_sec=probe_timeout_sec):
        lines.append(f"  - {entry}")
    lines.append("")
    return lines


def runner_health_facts(repo: Path, probe_timeout_sec: int = 5) -> list[str]:
    """Probe bundled runners and return compact, display-ready facts.

    Failures degrade to a single fact line. Brief generation must never fail
    because a runner or healthcheck is missing.
    """
    runners_dir = repo / "scripts" / "delegate" / "runners"
    healthcheck = runners_dir / "healthcheck.sh"
    if not healthcheck.exists():
        return ["healthcheck unavailable (missing scripts/delegate/runners/healthcheck.sh)"]

    env = dict(os.environ)
    env["PROBE_TIMEOUT"] = str(max(1, int(probe_timeout_sec)))
    timeout = max(5, probe_timeout_sec * len(DEFAULT_RUNNERS) + 5)
    try:
        proc = subprocess.run(
            ["bash", str(healthcheck), "--json", *DEFAULT_RUNNERS],
            cwd=str(runners_dir),
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return [f"healthcheck timed out after {timeout}s"]
    except OSError as exc:
        return [f"healthcheck failed to start: {exc}"]

    try:
        raw = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return [f"healthcheck returned invalid JSON (rc={proc.returncode})"]
    if not isinstance(raw, list):
        return [f"healthcheck returned non-list JSON (rc={proc.returncode})"]

    facts: list[str] = []
    for item in raw:
        if isinstance(item, dict):
            facts.append(_format_health_item(item))
    return facts or [f"healthcheck returned no runner rows (rc={proc.returncode})"]


def _format_health_item(item: dict[str, Any]) -> str:
    agent = str(item.get("agent") or "unknown")
    verdict = str(item.get("verdict") or "UNKNOWN")
    parts = [f"`{agent}`: {verdict}"]
    if item.get("exit") not in (None, ""):
        parts.append(f"exit={item['exit']}")
    if item.get("elapsed") not in (None, ""):
        parts.append(f"elapsed={item['elapsed']}")
    if item.get("bytes") not in (None, ""):
        parts.append(f"bytes={item['bytes']}")
    return ", ".join(parts)
