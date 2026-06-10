"""Audit parser and auditor-selection self-tests."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path

from lto.auditors import (
    _normalize_severity,
    _parse_structured_reply,
    _pick_auditors,
    _same_family,
    parse_findings_text,
)
from lto.commands.audit import _build_brief, _scan_severity


class _Counter:
    def __init__(self) -> None:
        self.passed = 0
        self.total = 0

    def ok(self, cond: bool, label: str) -> None:
        self.total += 1
        if cond:
            self.passed += 1
            print(f"  OK {label}")
        else:
            print(f"  FAIL {label}")


def run() -> tuple[int, int]:
    c = _Counter()

    print("\n[S1] _parse_structured_reply: whole-file JSON")
    with tempfile.TemporaryDirectory() as d:
        dp = Path(d)
        (dp / "valid.json").write_text(json.dumps([
            {"severity": "critical", "claim": "missing lock"},
            {"severity": "high", "claim": "rollback gap"},
        ]), encoding="utf-8")
        c.ok(len(_parse_structured_reply(dp / "valid.json") or []) == 2,
             "S1a whole-file JSON parsed")
        (dp / "empty.json").write_text("[]", encoding="utf-8")
        c.ok(_parse_structured_reply(dp / "empty.json") is None,
             "S1b empty array stays None for review findings")
        (dp / "plain.md").write_text("plain text", encoding="utf-8")
        c.ok(_parse_structured_reply(dp / "plain.md") is None,
             "S1c plain text returns None")

    print("\n[S2] _parse_structured_reply: fenced JSON")
    with tempfile.TemporaryDirectory() as d:
        dp = Path(d)
        (dp / "fenced.md").write_text(
            "preamble\n```json\n"
            "[{\"severity\":\"high\",\"claim\":\"edge gap\",\"file\":\"x.py\"}]\n"
            "```\n", encoding="utf-8")
        parsed = _parse_structured_reply(dp / "fenced.md") or []
        c.ok(len(parsed) == 1 and parsed[0]["severity"] == "high",
             "S2a JSON fence parsed")
        (dp / "multi.md").write_text(
            "```json\n[{\"severity\":\"low\",\"claim\":\"minor\"}]\n```\n"
            "```json\n{\"not\":\"findings\"}\n```\n", encoding="utf-8")
        parsed2 = _parse_structured_reply(dp / "multi.md") or []
        c.ok(len(parsed2) == 1 and parsed2[0]["severity"] == "low",
             "S2b first valid findings fence used")
        (dp / "bad.md").write_text(
            "```json\n[{\"severity\":\"CRITICAL!!!\",\"claim\":\"bad\"}]\n```\n",
            encoding="utf-8")
        c.ok(_parse_structured_reply(dp / "bad.md") is None,
             "S2c invalid severity returns None")

    print("\n[S3] structured severity counting")
    with tempfile.TemporaryDirectory() as d:
        dp = Path(d)
        (dp / "reply-codex.md").write_text(
            "```json\n[{\"severity\":\"critical\",\"claim\":\"x\"},"
            "{\"severity\":\"high\",\"claim\":\"y\"}]\n```\n", encoding="utf-8")
        (dp / "reply-pi.md").write_text(
            "```json\n[{\"severity\":\"high\",\"claim\":\"z\"}]\n```\n",
            encoding="utf-8")
        (dp / "reply-agy.md").write_text(
            "```json\n[]\n```\nNo critical problems.", encoding="utf-8")
        structured = []
        fallback = []
        for p in sorted(dp.iterdir()):
            parsed = _parse_structured_reply(p)
            if parsed is None:
                fallback.append(p)
            else:
                structured.extend(parsed)
        critical = sum(1 for f in structured if f["severity"] == "critical")
        high = sum(1 for f in structured if f["severity"] == "high")
        c.ok((critical, high) == (1, 2), "S3a structured counts exact")
        _fb_high, fb_critical = _scan_severity(fallback)
        c.ok(fb_critical >= 1, "S3b legacy regex false-positive demonstrated")

    print("\n[S4] mixed structured + text")
    with tempfile.TemporaryDirectory() as d:
        dp = Path(d)
        (dp / "reply-codex.md").write_text(
            "```json\n[{\"severity\":\"critical\",\"claim\":\"x\"}]\n```\n",
            encoding="utf-8")
        (dp / "reply-pi.md").write_text("plain review", encoding="utf-8")
        parsed_count = sum(
            1 for p in dp.iterdir() if _parse_structured_reply(p) is not None
        )
        c.ok(parsed_count == 1, "S4a one structured reply, one fallback")

    print("\n[S5] _build_brief includes output schema")
    brief = _build_brief(
        {"goal": "test", "host_runtime": "claude", "current_phase": "audit"},
        [{"id": "T1", "title": "test task"}],
    )
    c.ok("```json" in brief, "S5a JSON code block present")
    c.ok('"severity"' in brief and '"claim"' in brief, "S5b schema fields present")

    print("\n[S11] yh质检 cap findings adapter (Chinese severity + object wrap)")
    # yh gov-doc-verify --json 输出：顶层对象裹 findings[]，severity 中文，
    # location 是对象。LTO 侧自映射到四档 + 提 location.file 到顶层。
    yh_output = json.dumps({
        "pass": False,
        "issues": [{"type": "标题序号", "severity": "严重"}],
        "findings": [
            {"severity": "严重", "claim": "标题序号层级错",
             "location": {"file": "doc.docx", "paragraph": 3}, "source_cap": "gov-doc-verify", "rule": "标题序号"},
            {"severity": "警告", "claim": "混淆字",
             "location": {"file": "doc.docx", "paragraph": 12}, "source_cap": "gov-doc-verify", "rule": "混淆字"},
            {"severity": "提示", "claim": "建议补落款",
             "location": {"file": "doc.docx"}, "source_cap": "gov-doc-verify", "rule": "落款"},
        ],
        "summary": {"严重": 1, "警告": 1, "提示": 1},
    }, ensure_ascii=False)
    yf = parse_findings_text(yh_output) or []
    c.ok(len(yf) == 3, "S11a yh object-wrapped findings extracted (3)")
    c.ok([f["severity"] for f in yf] == ["critical", "high", "low"],
         "S11b Chinese severity 严重/警告/提示 → critical/high/low")
    c.ok(yf[0].get("file") == "doc.docx" and yf[2].get("file") == "doc.docx",
         "S11c nested location.file lifted to top level")
    c.ok(yf[0].get("source_cap") == "gov-doc-verify" and yf[0].get("rule") == "标题序号",
         "S11d source_cap/rule passthrough for ledger")
    # yh output inside a ```json fence (agent may wrap it)
    fenced = "审计：\n```json\n" + yh_output + "\n```\n"
    c.ok(len(parse_findings_text(fenced) or []) == 3, "S11e yh object inside fence parsed")
    # empty findings object stays None (preserve text fallback)
    c.ok(parse_findings_text(json.dumps({"pass": True, "findings": []})) is None,
         "S11f empty findings object returns None")
    # bare English array path must not regress
    c.ok((parse_findings_text(json.dumps([{"severity": "high", "claim": "x"}])) or [{}])[0].get("severity") == "high",
         "S11g bare English array path intact")
    # unknown severity still rejected (no dirty data smuggling)
    c.ok(_normalize_severity("garbage") == "garbage"
         and parse_findings_text(json.dumps([{"severity": "garbage", "claim": "x"}])) is None,
         "S11h unknown severity rejected")

    print("\n[S7/S10] auditor family selection")
    for host, expected, forbidden in (
        ("codex", {"pi", "agy"}, "codex"),
        ("pi", {"codex", "agy"}, "pi"),
    ):
        auditors = set(_pick_auditors(host))
        c.ok(forbidden not in auditors, f"{host} host excludes same runtime")
        c.ok(expected.issubset(auditors), f"{host} host keeps heterogenous auditors")
    discoverer = _pick_auditors("claude")[0]
    c.ok(discoverer in ("codex", "pi", "agy"), "claude host picks valid auditor")
    c.ok(not _same_family(_pick_auditors("codex")[0], "codex"),
         "discoverer differs from host family")

    return c.passed, c.total
