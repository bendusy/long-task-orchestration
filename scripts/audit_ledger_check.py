#!/usr/bin/env python3
from __future__ import annotations

import argparse, re, sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SKILL_DIR = SCRIPT_DIR.parent

# Round Summary header columns in templates/audit-ledger.md.
ROUND_COL = 0
HIGH_COL = 3
CRITICAL_COL = 4

VERDICT_CONVERGED = "CONVERGED"
VERDICT_CONVERGING = "CONVERGING"
VERDICT_REBOUND = "REBOUND"
VERDICT_STALLED = "STALLED"


class LedgerError(Exception):
    """Raised on usage / parse errors that must map to exit code 2."""


def validate_run_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,95}", value):
        raise LedgerError(f"invalid run id: {value!r}")
    if value in {".", ".."} or ".." in value:
        raise LedgerError(f"invalid run id: {value!r}")
    return value


def resolve_ledger_path(args: argparse.Namespace) -> Path:
    if args.path:
        return Path(args.path)
    if args.run_id:
        run_id = validate_run_id(args.run_id)
        repo = (args.repo or Path.cwd()).resolve()
        return repo / ".lto" / run_id / "audit-ledger.md"
    raise LedgerError("provide a ledger path or --run-id")


def split_cells(line: str) -> list[str]:
    stripped = line.strip()
    if stripped.startswith("|"):
        stripped = stripped[1:]
    if stripped.endswith("|"):
        stripped = stripped[:-1]
    return [cell.strip() for cell in stripped.split("|")]


def is_separator_row(cells: list[str]) -> bool:
    joined = "".join(cells)
    return bool(joined) and set(joined) <= set("-: ")


def is_header_row(cells: list[str]) -> bool:
    return any(cell.strip().lower() == "round" for cell in cells)


def parse_count(raw: str, round_label: str, column: str) -> int:
    # Tolerate a+b accumulation forms used in the ledger (e.g. critical "1+2").
    parts = [p.strip() for p in raw.split("+") if p.strip()]
    total = 0
    for part in parts:
        if not re.fullmatch(r"\d+", part):
            raise LedgerError(f"non-numeric {column} count in {round_label}: {raw!r}")
        total += int(part)
    return total


def extract_rounds(text: str) -> list[dict]:
    rounds: list[dict] = []
    in_summary = False
    for line in text.splitlines():
        if line.startswith("## "):
            in_summary = line.strip().lower() == "## round summary"
            continue
        if not in_summary:
            continue
        if "|" not in line:
            continue
        cells = split_cells(line)
        if len(cells) <= CRITICAL_COL:
            continue
        if is_separator_row(cells) or is_header_row(cells):
            continue
        round_label = cells[ROUND_COL] or f"row{len(rounds) + 1}"
        high_raw = cells[HIGH_COL]
        crit_raw = cells[CRITICAL_COL]
        # Empty count cells mean the round is not filled yet; skip entirely.
        if not high_raw and not crit_raw:
            continue
        high = parse_count(high_raw, round_label, "high")
        critical = parse_count(crit_raw, round_label, "critical")
        rounds.append({"round": round_label, "high": high, "critical": critical, "blocker": high + critical})
    return rounds


def evaluate(rounds: list[dict], strict: bool) -> tuple[str, str | None]:
    if not rounds:
        # No filled rounds means no outstanding blockers — treat as converged
        # so a run that never entered heterogeneous audit can still close out.
        return VERDICT_CONVERGED, None
    blockers = [r["blocker"] for r in rounds]
    for i in range(1, len(blockers)):
        if blockers[i] > blockers[i - 1]:
            reason = (
                f"rebound at {rounds[i]['round']}: "
                f"blocker {blockers[i - 1]} -> {blockers[i]}"
            )
            return VERDICT_REBOUND, reason
        if strict and blockers[i] == blockers[i - 1] and blockers[i] > 0:
            reason = (
                f"stalled at {rounds[i]['round']}: "
                f"blocker flat at {blockers[i]} for two rounds"
            )
            return VERDICT_STALLED, reason
    if blockers[-1] == 0:
        return VERDICT_CONVERGED, None
    return VERDICT_CONVERGING, None


ALL_VERDICTS = (VERDICT_CONVERGED, VERDICT_CONVERGING, VERDICT_REBOUND, VERDICT_STALLED)


def verdict_exit_code(verdict: str) -> int:
    if verdict in (VERDICT_REBOUND, VERDICT_STALLED):
        return 1
    if verdict in (VERDICT_CONVERGED, VERDICT_CONVERGING):
        return 0
    # An unmapped verdict must not silently pass as healthy; surface it.
    raise LedgerError(f"unknown verdict: {verdict!r}")


def report(rounds: list[dict], verdict: str, reason: str | None) -> int:
    if not rounds:
        print("no filled rounds yet")
        print(f"verdict: {verdict}")
        return 0
    sequence = " -> ".join(f"{r['round']}={r['blocker']}" for r in rounds)
    print(f"blocker sequence: {sequence}")
    print(f"verdict: {verdict}")
    if reason:
        print(reason, file=sys.stderr)
    return verdict_exit_code(verdict)


def cmd_check(args: argparse.Namespace) -> int:
    path = resolve_ledger_path(args)
    if not path.exists():
        raise LedgerError(f"ledger not found: {path}")
    text = path.read_text(encoding="utf-8")
    if "## Round Summary" not in text:
        raise LedgerError(f"no Round Summary section in {path}")
    rounds = extract_rounds(text)
    verdict, reason = evaluate(rounds, args.strict)
    return report(rounds, verdict, reason)


def _assert_case(name: str, text: str, strict: bool, want_verdict: str, want_rc: int) -> None:
    rounds = extract_rounds(text)
    verdict, _ = evaluate(rounds, strict)
    rc = verdict_exit_code(verdict)
    if verdict != want_verdict or rc != want_rc:
        raise AssertionError(f"{name}: got {verdict}/rc{rc}, want {want_verdict}/rc{want_rc}")


def _ledger(*rows: str) -> str:
    header = [
        "## Round Summary",
        "",
        "| round | artifact | auditors | high | critical | minor | trend | status |",
        "|---|---|---|---:|---:|---:|---|---|",
    ]
    return "\n".join(header + list(rows)) + "\n"


def cmd_self_test(_: argparse.Namespace) -> int:
    descending = _ledger(
        "| R1 | spec v0 | codex pi agy | 3 | 5 | 2 | start | open |",
        "| R2 | spec v1 | codex pi agy | 2 | 1+2 | 1 | down | open |",
        "| R3 | spec v2 | codex pi | 0 | 0 | 3 | down | open |",
    )
    _assert_case("descending-to-zero", descending, False, VERDICT_CONVERGED, 0)

    still_going = _ledger(
        "| R1 | spec v0 | codex pi agy | 3 | 5 | 2 | start | open |",
        "| R2 | spec v1 | codex pi agy | 1 | 1 | 1 | down | open |",
    )
    _assert_case("descending-not-zero", still_going, False, VERDICT_CONVERGING, 0)

    rebound = _ledger(
        "| R1 | spec v0 | codex pi agy | 1 | 1 | 0 | start | open |",
        "| R2 | spec v1 | codex pi agy | 1 | 0 | 0 | down | open |",
        "| R3 | spec v2 | codex pi agy | 2 | 1 | 0 | rebound | open |",
    )
    _assert_case("rebound", rebound, False, VERDICT_REBOUND, 1)

    stalled = _ledger(
        "| R1 | spec v0 | codex pi agy | 2 | 1 | 0 | start | open |",
        "| R2 | spec v1 | codex pi agy | 1 | 1 | 0 | down | open |",
        "| R3 | spec v2 | codex pi agy | 1 | 1 | 0 | flat | open |",
    )
    _assert_case("stalled-strict", stalled, True, VERDICT_STALLED, 1)
    # Without --strict the same flat ledger must not hard-fail.
    _assert_case("stalled-nonstrict", stalled, False, VERDICT_CONVERGING, 0)

    placeholder = _ledger("| R1 |  |  |  |  |  | start | open |")
    placeholder_rounds = extract_rounds(placeholder)
    assert placeholder_rounds == [], f"placeholder rounds should be empty: {placeholder_rounds}"
    # An unfilled ledger has no outstanding blockers -> CONVERGED (closeout-safe).
    _assert_case("empty-placeholder", placeholder, False, VERDICT_CONVERGED, 0)

    # A filled-but-not-zero ledger must stay CONVERGING so closeout can refuse it.
    _assert_case("descending-not-zero-vs-empty", still_going, False, VERDICT_CONVERGING, 0)

    print("LEDGERCHECK SELFTEST OK")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Check LTO audit-ledger blocker convergence.")
    sub = parser.add_subparsers(dest="command")

    check = sub.add_parser("check", help="evaluate convergence of an audit-ledger.md")
    check.add_argument("path", nargs="?", help="path to audit-ledger.md (takes priority)")
    check.add_argument("--run-id", help="resolve .lto/<run-id>/audit-ledger.md")
    check.add_argument("--repo", type=Path, default=None, help="repo root for --run-id")
    check.add_argument("--strict", action="store_true", help="treat two flat rounds (>0) as STALLED")
    check.set_defaults(func=cmd_check)

    self_test = sub.add_parser("self-test", help="run offline assertions")
    self_test.set_defaults(func=cmd_self_test)
    return parser


def main(argv: list[str] | None = None) -> int:
    # Bare path / flags default to the check command for ergonomic usage.
    raw = list(sys.argv[1:] if argv is None else argv)
    if raw and raw[0] not in {"check", "self-test", "-h", "--help"}:
        raw = ["check", *raw]
    parser = build_parser()
    args = parser.parse_args(raw)
    if not getattr(args, "func", None):
        parser.print_help(sys.stderr)
        return 2
    try:
        return args.func(args)
    except LedgerError as exc:
        print(f"ERROR {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
