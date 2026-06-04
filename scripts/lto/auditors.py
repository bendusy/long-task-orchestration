"""Shared auditor selection and structured findings parsing primitives."""

from __future__ import annotations

import json
import re
from pathlib import Path


_VALID_SEVERITIES = {"critical", "high", "medium", "low"}

_FAMILY = {
    "claude": "anthropic",
    "anthropic": "anthropic",
    "codex": "openai",
    "gpt": "openai",
    "openai": "openai",
    "pi": "deepseek",
    "deepseek": "deepseek",
    "agy": "google",
    "gemini": "google",
    "google": "google",
}


def _is_findings_list(data) -> bool:
    """Return True when data is a non-empty findings list with valid severities."""
    if not isinstance(data, list) or len(data) == 0:
        return False
    for item in data:
        if not isinstance(item, dict):
            return False
        sev = str(item.get("severity", "")).lower()
        if sev not in _VALID_SEVERITIES:
            return False
    return True


def parse_findings_text(text: str) -> list[dict] | None:
    """Parse structured JSON findings from text.

    Accepts either a whole-file JSON findings list or the first valid
    ```json ... ``` block. Empty arrays intentionally return None to preserve
    existing audit fallback behavior.
    """
    if not text:
        return None

    try:
        data = json.loads(text)
        if _is_findings_list(data):
            return data
    except (json.JSONDecodeError, ValueError):
        pass

    json_blocks = re.findall(r"```json\s*\n(.*?)\n```", text, re.DOTALL)
    for block in json_blocks:
        try:
            data = json.loads(block)
            if _is_findings_list(data):
                return data
        except (json.JSONDecodeError, ValueError):
            continue

    return None


def _parse_structured_reply(reply_path: Path) -> list[dict] | None:
    """Parse a reply file as structured JSON findings."""
    text = reply_path.read_text(encoding="utf-8", errors="replace")
    return parse_findings_text(text)


def _family(runtime: str) -> str:
    rl = runtime.lower()
    for key, fam in _FAMILY.items():
        if key in rl:
            return fam
    return rl


def _same_family(a: str, b: str) -> bool:
    return _family(a) == _family(b)


def _pick_auditors(host: str) -> list[str]:
    """Pick heterogeneous auditors from the supported runtime pool."""
    pool = ["codex", "pi", "agy"]
    picked = [a for a in pool if not _same_family(a, host)]
    return picked or pool


def _runtime_from_filename(name: str) -> str | None:
    nl = name.lower()
    for key in ("codex", "claude", "gemini", "agy", "pi", "gpt", "deepseek"):
        if key in nl:
            return key
    return None
