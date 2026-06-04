"""lto check — 校验 run-state 完整性。"""

from __future__ import annotations

import argparse, sys
import json
from pathlib import Path

from .. import state as st
from .. import git_state as gs


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    target_dir = repo / ".lto" / run_id
    errors: list[str] = []
    warnings: list[str] = []

    # Check state.json exists and is valid
    state_path = target_dir / "state.json"
    state = None
    if not state_path.exists():
        errors.append(f"missing {state_path}")
    else:
        state = st.load_state(state_path)
        if state is None:
            errors.append(f"cannot parse {state_path}")
        else:
            _check_state(state, repo, errors, warnings, args.strict)

    # Check run-state.md (optional under new state.json model)
    md_path = target_dir / "run-state.md"
    if not md_path.exists() and not state_path.exists():
        errors.append(f"missing both state.json and run-state.md")

    # Check audit-ledger (only if present, not mandatory)
    ledger_path = target_dir / "audit-ledger.md"
    ledger_status = None
    if ledger_path.exists():
        ledger_status = _check_ledger(
            ledger_path, errors, warnings, args.strict, emit=not args.json,
        )

    phase_report = None
    if args.to_phase and state is not None:
        phase_report = _phase_report(
            state, repo, run_id, args.to_phase, ledger_status, ledger_path, args.strict,
        )
        if args.strict:
            for check in phase_report["checks"]:
                if check["required"] and check["status"] == "missing":
                    errors.append(f"phase evidence missing: {check['id']}: {check['detail']}")

    if args.json:
        output = phase_report or {"run_id": run_id, "check": {"errors": errors, "warnings": warnings}}
        if phase_report:
            output["check"] = {"errors": errors, "warnings": warnings}
        print(json.dumps(output, ensure_ascii=False, sort_keys=True))
        return 1 if errors else 0

    for warning in warnings:
        print(f"WARN {warning}", file=sys.stderr)
    for error in errors:
        print(f"ERROR {error}", file=sys.stderr)
    if phase_report:
        _print_phase_report(phase_report)

    if errors:
        return 1
    print(f"OK {target_dir}")
    return 0


def _check_state(state: dict, repo: Path, errors: list, warnings: list, strict: bool) -> None:
    # Phase validity
    phase = state.get("current_phase", "")
    if phase and phase not in st.VALID_PHASES:
        errors.append(f"invalid current_phase: {phase}")

    # Git anchor
    ws = state.get("workspace", {})
    recorded_head = ws.get("head", "unknown")
    actual_head = gs.git_head(repo)

    if strict:
        if not gs.is_git_repo(repo):
            errors.append("strict check requires a git worktree")
        elif not recorded_head or recorded_head == "unknown" or not actual_head:
            errors.append("strict check requires a real git HEAD anchor")
        elif recorded_head and not gs.git_commit_exists(repo, recorded_head):
            errors.append(f"recorded git HEAD not a commit: {recorded_head}")
        elif phase != "closed" and recorded_head != actual_head:
            drift = gs.head_drift(repo, recorded_head)
            if drift in ("rewrite", "unreachable"):
                errors.append(f"git HEAD {drift}: {recorded_head[:8]}→{actual_head[:8]}")

    if (
        phase != "closed"
        and gs.is_git_repo(repo)
        and recorded_head
        and recorded_head != "unknown"
        and actual_head
        and recorded_head != actual_head
    ):
        drift = gs.head_drift(repo, recorded_head)
        if drift == "forward":
            file_drift = gs.task_file_drift(repo, recorded_head, actual_head, state)
            if file_drift["missing_touched_files"]:
                warnings.append("no task touched_files recorded; file drift precision unavailable")
            if file_drift["changed_paths"]:
                sample = ", ".join(file_drift["changed_paths"][:8])
                suffix = "" if len(file_drift["changed_paths"]) <= 8 else ", ..."
                msg = f"related task files changed since recorded HEAD: {sample}{suffix}"
                if strict:
                    errors.append(msg)
                else:
                    warnings.append(msg)

    if phase != "closed" and gs.git_dirty(repo):
        msg = "worktree has uncommitted changes outside .lto"
        if strict:
            errors.append(msg)
        else:
            warnings.append(msg)

    # Handoff
    if phase == "closed":
        handoff = repo / ".lto" / state.get("run_id", "") / "handoff.md"
        if not handoff.exists() or not handoff.read_text(encoding="utf-8").strip():
            errors.append("closed run missing non-empty handoff.md")


def _check_ledger(
    ledger_path: Path, errors: list, warnings: list, strict: bool, *, emit: bool = True,
) -> dict:
    status = _ledger_status(ledger_path, strict)
    if emit and status.get("verdict"):
        print(f"ledger: {status['verdict']}")
    if status.get("error"):
        msg = f"ledger check failed: {status['error']}"
        if strict:
            errors.append(msg)
        else:
            warnings.append(msg)
    elif status.get("rc") == 1 and strict:
        errors.append("ledger not converging (strict mode)")
    return status


def _ledger_status(ledger_path: Path, strict: bool) -> dict:
    try:
        import audit_ledger_check as alc
        text = ledger_path.read_text(encoding="utf-8")
        if "## Round Summary" not in text:
            return {"exists": True, "error": f"no Round Summary section in {ledger_path}"}
        rounds = alc.extract_rounds(text)
        verdict, reason = alc.evaluate(rounds, strict)
        return {
            "exists": True,
            "has_rounds": bool(rounds),
            "verdict": verdict,
            "reason": reason,
            "rc": alc.verdict_exit_code(verdict),
        }
    except Exception as exc:
        return {"exists": ledger_path.exists(), "error": str(exc)}


PHASE_ORDER = {
    "intake": 0,
    "spec": 1,
    "audit": 2,
    "implementation": 3,
    "deploy": 4,
    "observe": 5,
    "closed": 6,
}


def _phase_report(
    state: dict,
    repo: Path,
    run_id: str,
    target: str,
    ledger_status: dict | None,
    ledger_path: Path,
    strict: bool,
) -> dict:
    checks: list[dict] = []

    def add(cid: str, label: str, status: str, required: bool, detail: str) -> None:
        checks.append({
            "id": cid, "label": label, "status": status,
            "required": required, "detail": st.single_line(detail),
        })

    current = state.get("current_phase", "unknown")
    direction = _phase_direction(current, target)
    add("phase_direction", "Current phase vs target", "ok" if direction != "backward" else "warn",
        False, f"{current} -> {target} ({direction})")

    gate_unresolved = list(state.get("gates", {}).get("unresolved_blocks", []) or [])
    unverified_open_risks = [
        rp for rp in state.get("risk_points", [])
        if rp.get("disposition") == "open" and not rp.get("verified_by")
    ]

    if target == "implementation":
        unresolved_count = len(gate_unresolved) + len(unverified_open_risks)
        add("no_unresolved_blocks", "No unresolved gate blocks or open risks",
            "ok" if unresolved_count == 0 else "missing",
            True, "none" if unresolved_count == 0 else f"{unresolved_count} unresolved item(s)")
        _add_ledger_check(add, ledger_status, ledger_path, strict_required=strict)
        tasks = state.get("tasks", [])
        add("tasks_present", "Tasks are planned", "ok" if tasks else "warn",
            False, f"{len(tasks)} task(s)" if tasks else "no tasks found")
    elif target == "closed":
        open_tasks = [
            t for t in state.get("tasks", [])
            if t.get("status") not in ("done", "skipped")
        ]
        add("no_open_tasks", "No open tasks", "ok" if not open_tasks else "missing",
            True, "none" if not open_tasks else f"{len(open_tasks)} open task(s)")
        add("no_unresolved_blocks", "No unresolved gate blocks", "ok" if not gate_unresolved else "missing",
            True, "none" if not gate_unresolved else f"{len(gate_unresolved)} unresolved gate block(s)")
        add("risk_points_verified", "Risk points verified or closed",
            "ok" if not unverified_open_risks else "missing",
            True, "none" if not unverified_open_risks
            else f"{len(unverified_open_risks)} open unverified risk point(s)")
        _add_ledger_check(add, ledger_status, ledger_path, strict_required=strict)
        manifest = repo / ".lto" / run_id / "artifacts.json"
        add("artifact_manifest_exists", "Artifact manifest exists", "ok" if manifest.exists() else "warn",
            False, str(manifest.relative_to(repo)) if manifest.exists() else "missing artifacts.json")
        handoff = repo / ".lto" / run_id / "handoff.md"
        add("handoff_exists_if_already_closed", "Handoff exists if already closed",
            "ok" if current != "closed" or handoff.exists() else "warn",
            False, "not closed yet" if current != "closed" else ("exists" if handoff.exists() else "missing"))

    evidence_status = (
        "attention_required"
        if any(c["status"] in ("missing", "warn") for c in checks)
        else "all_required_present"
    )
    return {
        "run_id": run_id,
        "target_phase": target,
        "current_phase": current,
        "phase_direction": direction,
        "evidence_status": evidence_status,
        "human_gate_required": True,
        "checks": checks,
    }


def _add_ledger_check(add, ledger_status: dict | None, ledger_path: Path, *, strict_required: bool) -> None:
    if ledger_status is None:
        add("audit_ledger_converged_if_present", "Audit ledger converged if present",
            "warn", False, "no audit-ledger.md")
        return
    if ledger_status.get("error"):
        add("audit_ledger_converged_if_present", "Audit ledger converged if present",
            "warn", False, ledger_status["error"])
        return
    if not ledger_status.get("has_rounds"):
        add("audit_ledger_converged_if_present", "Audit ledger converged if present",
            "warn", False, "ledger exists but has no filled rounds")
        return
    verdict = ledger_status.get("verdict", "unknown")
    ok = verdict == "CONVERGED"
    status = "ok" if ok else ("missing" if strict_required else "warn")
    add("audit_ledger_converged_if_present", "Audit ledger converged if present",
        status, strict_required,
        f"{ledger_path.name}: {verdict}")


def _phase_direction(current: str, target: str) -> str:
    if current not in PHASE_ORDER or target not in PHASE_ORDER:
        return "unknown"
    if PHASE_ORDER[current] < PHASE_ORDER[target]:
        return "forward"
    if PHASE_ORDER[current] == PHASE_ORDER[target]:
        return "same"
    return "backward"


def _print_phase_report(report: dict) -> None:
    print(
        f"=== LTO Phase Evidence: {report['target_phase']} "
        f"({report['evidence_status']}) ==="
    )
    for check in report["checks"]:
        if check["status"] == "ok":
            icon = "OK"
        elif check["status"] == "missing":
            icon = "MISSING"
        else:
            icon = "WARN"
        scope = "required" if check["required"] else "advisory"
        print(f"  {icon} {scope} {check['id']}: {check['detail']}")
    print("  HUMAN human_gate_required: true")


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("check", help="validate run-state directory")
    p.add_argument("--run-id")
    p.add_argument("--strict", action="store_true")
    p.add_argument("--to", dest="to_phase", choices=("implementation", "closed"),
                   help="also report phase-entry evidence for the target phase")
    p.add_argument("--json", action="store_true", help="print JSON only")
    p.set_defaults(func=run)
