"""Shared auditor selection and structured findings parsing primitives."""

from __future__ import annotations

import json
import re
from pathlib import Path

from .agent_job import PermissionPolicy


_VALID_SEVERITIES = {"critical", "high", "medium", "low"}

# yh 质检 cap（gov-doc-verify / govdocx-qc / chengpi-gate）按约定输出中文
# severity（"严重"/"警告"/"提示"），由 LTO 侧自映射到四档。映射保守：yh 的
# "警告"不全等于 LTO 的 high（yh 阈值偏宽，chengpi-gate 曾 22 violations 仍
# pass），但归一后由 LTO 自己的 --high/--critical 闸门决定收敛标准。
_SEVERITY_ALIASES = {
    "严重": "critical", "致命": "critical", "阻断": "critical", "blocker": "critical",
    "警告": "high", "高危": "high", "高风险": "high",
    "提示": "low", "建议": "low", "info": "low", "提醒": "low",
    "中": "medium", "中危": "medium", "warning": "medium",
}


def _normalize_severity(value) -> str:
    """Map a raw severity (English or yh's Chinese) to LTO's four-tier scale.
    Unknown values pass through lowercased so _is_findings_list can reject them."""
    raw = str(value or "").strip()
    low = raw.lower()
    if low in _VALID_SEVERITIES:
        return low
    return _SEVERITY_ALIASES.get(raw, low)


def _normalize_finding(item: dict) -> dict:
    """Normalize one finding in place-ish: map severity to LTO scale and lift
    yh's nested location.{file,...} up to a top-level `file` for ledger use.
    Returns a new dict; original is not mutated."""
    out = dict(item)
    out["severity"] = _normalize_severity(item.get("severity"))
    loc = item.get("location")
    if isinstance(loc, dict):
        # keep the structured location, but surface `file` at top level (where
        # LTO's ledger and existing findings expect it) if not already set.
        if not out.get("file") and loc.get("file"):
            out["file"] = loc["file"]
    return out

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
    """Return True when data is a non-empty findings list with valid severities.
    Severity is normalized first so yh's Chinese severities (严重/警告/提示) pass."""
    if not isinstance(data, list) or len(data) == 0:
        return False
    for item in data:
        if not isinstance(item, dict):
            return False
        if _normalize_severity(item.get("severity")) not in _VALID_SEVERITIES:
            return False
    return True


def _findings_from_any(data) -> list[dict] | None:
    """Extract a findings list from either a bare array or an object that wraps
    one under a `findings` key (yh质检 cap 的 --json 输出是 {pass, issues,
    findings:[...], summary})."""
    if _is_findings_list(data):
        return [_normalize_finding(item) for item in data]
    if isinstance(data, dict):
        inner = data.get("findings")
        if _is_findings_list(inner):
            return [_normalize_finding(item) for item in inner]
    return None


def parse_findings_text(text: str) -> list[dict] | None:
    """Parse structured JSON findings from text.

    Accepts a bare JSON findings list, an object wrapping one under `findings`
    (yh质检 cap output), or the first valid ```json ... ``` block. Empty arrays
    intentionally return None to preserve existing audit fallback behavior.
    """
    if not text:
        return None

    try:
        found = _findings_from_any(json.loads(text))
        if found is not None:
            return found
    except (json.JSONDecodeError, ValueError):
        pass

    json_blocks = re.findall(r"```json\s*\n(.*?)\n```", text, re.DOTALL)
    for block in json_blocks:
        try:
            found = _findings_from_any(json.loads(block))
            if found is not None:
                return found
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


def readonly_intent_policy(runner: str) -> PermissionPolicy:
    """审计/评审/judge 这类只读意图派工的统一权限收口。

    评审是只读意图，但 §7 实测 agy 无 read-only 档（--sandbox=workspace-write 开关）。
    保留 agy 这一异构视角（union-merge 不投票、一个不漏），给它 workspace-write，
    越权风险靠 agy --sandbox 的工作区外封锁 + perm sidecar 监控兜底。
    其余 runner 一律 read-only（codex sandbox 档 / claude+pi tool-allowlist）。
    """
    if runner == "agy":
        return PermissionPolicy(
            sandbox="workspace-write",
            reason="agy has no read-only sandbox (§7); workspace-write is the "
            "minimal enforceable level to keep agy's heterogeneous review lens",
        )
    return PermissionPolicy(sandbox="read-only")


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
