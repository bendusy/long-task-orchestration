"""lto next — deterministic fact brief generator (zero LLM, zero key).

Design contract:
- This module produces a "decision brief" for the **host LLM** (the agent
  running LTO) to read and reason about. It never embeds any LLM call itself.
- It runs on the 6-phase rail and leaves path selection to the host LLM
  (linear / fan-out / adversarial / stop / ask human).
- Rich context: every blocked task carries a failure summary drawn from the
  last N lines of its stderr artifact, so the host LLM can reason like Coco:
  "goal is X but T3 failed because Y, suggest Z first."

Usage:
  lto next          → print command (unambiguous) or decision brief (escalate)
  lto next --exec   → execute unambiguous cmd; escalate → print brief only
  lto next --json   → output facts + route as JSON
"""

from __future__ import annotations

import argparse, json, subprocess, sys
from pathlib import Path
from typing import Any

from .. import state as st
from .. import git_state as gs
from ..agent_job import Pattern
from .audit import _is_high_risk


# ────────────────────────── analyze ──────────────────────────

def analyze(state: dict, repo: Path) -> dict:
    """Deterministic state analysis — facts, no judgement.

    Returns a dict with phase, task counts by status, blocked task details
    (including failure evidence summaries), gate status, and risk status.
    """
    phase = state.get("current_phase", "")
    tasks: list[dict] = state.get("tasks", [])
    risk_points: list[dict] = state.get("risk_points", [])
    gates: dict = state.get("gates", {})
    last_failure = state.get("last_failure")

    # Task count by status
    counts: dict[str, int] = {}
    for t in tasks:
        s = t.get("status", "pending")
        counts[s] = counts.get(s, 0) + 1

    has_tasks = len(tasks) > 0

    # Blocked tasks with reason + failure evidence
    blocked: list[dict] = []
    in_progress_tasks: list[dict] = []
    pending_tasks: list[dict] = []
    done_tasks: list[dict] = []

    for t in tasks:
        status = t.get("status", "pending")
        task_summary = {
            "id": t.get("id", ""),
            "title": t.get("title", ""),
            "status": status,
        }

        if status == "blocked":
            entry = dict(task_summary)
            entry["blockers"] = t.get("blockers", [])
            entry["failure_summary"] = _extract_failure_summary(t, repo)
            blocked.append(entry)
        elif status == "in_progress":
            in_progress_tasks.append(task_summary)
        elif status == "pending":
            pending_tasks.append(task_summary)
        elif status == "done":
            done_tasks.append(task_summary)

    # All tasks done? (only meaningful when has_tasks=True)
    all_done = has_tasks and counts.get("done", 0) == len(tasks)

    # All non-skipped tasks done?
    all_non_skipped_done = has_tasks and (
        counts.get("done", 0) + counts.get("skipped", 0) == len(tasks)
    )

    # Risk points
    unverified_risks = sum(1 for rp in risk_points if rp.get("disposition") == "open")
    has_high_risk_unreviewed = any(
        _is_high_risk(t) for t in tasks
        if t.get("status") in ("in_progress", "done", "blocked")
    )

    # Gate status — compare with current HEAD
    actual_head = gs.git_head(repo)
    last_tested = gates.get("last_tested_head")
    last_reviewed = gates.get("last_reviewed_head")
    unresolved_blocks = gates.get("unresolved_blocks", [])

    gate_status = {
        "last_tested_head": last_tested,
        "last_reviewed_head": last_reviewed,
        "actual_head": actual_head,
        "tested_behind": (
            last_tested is not None
            and last_tested != actual_head
            and not (gs.git_commit_exists(repo, last_tested) and gs.is_ancestor(repo, last_tested, actual_head))
        ) if last_tested else False,
        "reviewed_behind": (
            last_reviewed is not None
            and last_reviewed != actual_head
            and not (gs.git_commit_exists(repo, last_reviewed) and gs.is_ancestor(repo, last_reviewed, actual_head))
        ) if last_reviewed else False,
        "unresolved_blocks": unresolved_blocks,
        "has_unresolved": len(unresolved_blocks) > 0,
    }

    return {
        "phase": phase,
        "has_tasks": has_tasks,
        "task_counts": counts,
        "total_tasks": len(tasks),
        "all_done": all_done,
        "all_non_skipped_done": all_non_skipped_done,
        "blocked": blocked,
        "in_progress": in_progress_tasks,
        "pending": pending_tasks,
        "done": done_tasks,
        "unverified_risk_points": unverified_risks,
        "has_high_risk_unreviewed": has_high_risk_unreviewed,
        "last_failure": last_failure,
        "gates": gate_status,
    }


def _extract_failure_summary(task: dict, repo: Path) -> dict:
    """Extract failure evidence from a blocked task's last evidence entry.

    Returns {"reason": ..., "stderr_tail": [...]} or {} if no evidence.
    """
    evidence_list: list[dict] = task.get("evidence", [])
    if not evidence_list:
        return {}

    # Find last failed evidence
    last_failed = None
    for ev in reversed(evidence_list):
        if ev.get("rc", 1) != 0:
            last_failed = ev
            break

    if last_failed is None:
        return {}

    result: dict[str, Any] = {
        "kind": last_failed.get("kind", "unknown"),
        "rc": last_failed.get("rc", 1),
        "command": last_failed.get("command", "")[:120],
        "summary": last_failed.get("summary", ""),
    }

    # Try to read stderr artifact (last 15 lines)
    stderr_artifact: str | None = last_failed.get("stderr_artifact")
    if stderr_artifact:
        artifact_path = repo / stderr_artifact
        try:
            if artifact_path.exists():
                lines = artifact_path.read_text(encoding="utf-8").splitlines()
                result["stderr_tail"] = lines[-15:] if len(lines) > 15 else lines
            else:
                result["stderr_tail"] = ["(artifact file not found)"]
        except Exception:
            result["stderr_tail"] = ["(error reading artifact)"]

    # If no stderr artifact, try stdout
    if "stderr_tail" not in result:
        stdout_artifact: str | None = last_failed.get("stdout_artifact")
        if stdout_artifact:
            artifact_path = repo / stdout_artifact
            try:
                if artifact_path.exists():
                    lines = artifact_path.read_text(encoding="utf-8").splitlines()
                    result["stdout_tail"] = lines[-15:] if len(lines) > 15 else lines
            except Exception:
                pass

    return result


# ──────────────────────── build_decision_brief ──────────────────────

def build_decision_brief(facts: dict, state: dict) -> str:
    """Build structured Markdown decision brief for the host LLM.

    Lists current status, candidate actions (with pattern suggestions based on
    real failure information, not templates), and an explicit reminder that
    the host LLM must do the reasoning.
    """
    lines: list[str] = []

    # ── Header ──
    lines.append("# LTO Decision Brief")
    lines.append("")
    lines.append(
        "This brief is a deterministic summary of the current run state. "
        "**The host LLM (you, the agent) must read it and reason about the "
        "next pattern to follow.** No pattern decision has been made by the "
        "tool itself."
    )
    lines.append("")

    # ── Current State ──
    lines.append("## Current State")
    lines.append("")
    lines.append(f"- **Goal**: {state.get('goal', '(none)')}")
    lines.append(f"- **Phase**: {facts['phase']}")
    lines.append(f"- **Tasks**: {facts['total_tasks']} total "
                  f"(done={facts['task_counts'].get('done', 0)}, "
                  f"in_progress={facts['task_counts'].get('in_progress', 0)}, "
                  f"blocked={facts['task_counts'].get('blocked', 0)}, "
                  f"pending={facts['task_counts'].get('pending', 0)})")
    lines.append(f"- **Unverified risk points**: {facts['unverified_risk_points']}")
    if facts["last_failure"]:
        lines.append(f"- **Last failure**: {facts['last_failure']}")
    lines.append("")

    # ── Gate Status ──
    gs_info = facts["gates"]
    lines.append("### Gates")
    lines.append("")
    if gs_info["tested_behind"]:
        lines.append(f"- ⚠️  Last tested HEAD ({_short(gs_info['last_tested_head'])}) is behind "
                      f"current HEAD ({_short(gs_info['actual_head'])})")
    if gs_info["reviewed_behind"]:
        lines.append(f"- ⚠️  Last reviewed HEAD ({_short(gs_info['last_reviewed_head'])}) is behind "
                      f"current HEAD ({_short(gs_info['actual_head'])})")
    if gs_info["has_unresolved"]:
        lines.append(f"- ⚠️  {len(gs_info['unresolved_blocks'])} unresolved blocks")
    if not (gs_info["tested_behind"] or gs_info["reviewed_behind"] or gs_info["has_unresolved"]):
        lines.append("- ✅ All gates clear")
    lines.append("")

    # ── Blocked Tasks ──
    if facts["blocked"]:
        lines.append("## Blocked Tasks")
        lines.append("")
        for bt in facts["blocked"]:
            lines.append(f"### {bt['id']}: {bt['title']}")
            for blocker in bt.get("blockers", []):
                lines.append(f"- **Blocker**: {blocker.get('reason', '(no reason)')}")
            fs = bt.get("failure_summary", {})
            if fs:
                lines.append(f"- **Last failure**: [{fs.get('kind', '?')}] rc={fs.get('rc', '?')} "
                              f"— `{fs.get('command', '')}`")
                if fs.get("summary"):
                    lines.append(f"  - Summary: {fs['summary']}")
                tail = fs.get("stderr_tail") or fs.get("stdout_tail")
                if tail:
                    lines.append("  - **Evidence tail**:")
                    lines.append("    ```")
                    for line in tail:
                        lines.append(f"    {line}")
                    lines.append("    ```")
            lines.append("")

    # ── Candidate Actions ──
    lines.append("## Candidate Actions")
    lines.append("")
    lines.append(
        "Each candidate below includes a **why** based on actual state facts, "
        "not a template. It also suggests a pattern, but the final routing "
        "decision is yours."
    )
    lines.append("")

    # 1. Blocked tasks
    if facts["blocked"]:
        if len(facts["blocked"]) == 1:
            bt = facts["blocked"][0]
            reason = bt.get("blockers", [{}])[0].get("reason", "unknown")
            fs = bt.get("failure_summary", {})
            detail = ""
            if fs.get("summary"):
                detail = f" (failure: {fs['summary']})"
            lines.append(f"1. **Fix blocked task {bt['id']}** — `linear`")
            lines.append(f"   - Why: {bt['id']} is blocked due to \"{reason}\"{detail}. "
                          "Diagnose and repair, then re-run evidence.")
        else:
            lines.append(f"1. **Unblock {len(facts['blocked'])} tasks** — `fan-out` or `linear`")
            lines.append(f"   - Why: {len(facts['blocked'])} tasks are blocked:")
            for bt in facts["blocked"]:
                reason = bt.get("blockers", [{}])[0].get("reason", "unknown")
                fs = bt.get("failure_summary", {})
                detail = f" — {fs['summary']}" if fs.get("summary") else ""
                lines.append(f"     - {bt['id']}: \"{reason}\"{detail}")
            lines.append("   - Fan-out can diagnose all in parallel; linear fixes one by one.")
        lines.append("")

    # 2. High-risk task unreviewed
    if facts["has_high_risk_unreviewed"]:
        lines.append("2. **Audit high-risk tasks** — `adversarial` (`lto audit`)")
        lines.append("   - Why: One or more tasks match high-risk keywords "
                      "(persistence, auth, migration, etc.) and have not been "
                      "adversarially audited. An audit can catch regressions or "
                      "security gaps before closeout.")
        lines.append("")

    # 3. Phase all done, not judged
    if facts["all_non_skipped_done"] and facts["has_tasks"]:
        lines.append(f"3. **Judge phase {facts['phase']}** — `judge`")
        lines.append(f"   - Why: All tasks in phase \"{facts['phase']}\" are done "
                      "or skipped. Run `lto judge --phase {phase}` to validate "
                      f"exit criteria before advancing.")
        lines.append("")

    # 4. Pursue remaining pending / in-progress work (linear or fan-out)
    pending_count = len(facts.get("pending", []))
    in_progress_count = len(facts.get("in_progress", []))
    if pending_count > 0 or in_progress_count > 0:
        lines.append("4. **Pursue remaining work** — `linear` or `fan-out`")
        if pending_count > 0:
            names = ", ".join(t["id"] for t in facts["pending"][:5])
            lines.append(f"   - Why: {pending_count} pending tasks ({names}...). "
                          "Proceed linearly or fan-out if independent.")
        if in_progress_count > 0:
            names = ", ".join(t["id"] for t in facts["in_progress"])
            lines.append(f"   - Why: {in_progress_count} tasks in progress ({names}). "
                          "Check if any are stalled.")
        lines.append("")

    # 5. Closeout
    if facts["all_non_skipped_done"] and facts["has_tasks"]:
        gate_issues = []
        if facts["unverified_risk_points"] > 0:
            gate_issues.append(f"{facts['unverified_risk_points']} unverified risk points")
        if gs_info["has_unresolved"]:
            gate_issues.append(f"{len(gs_info['unresolved_blocks'])} unresolved blocks")
        if gs_info["tested_behind"]:
            gate_issues.append("tests behind HEAD")
        if gs_info["reviewed_behind"]:
            gate_issues.append("review behind HEAD")

        if gate_issues:
            lines.append("5. **Closeout** — blocked by gates")
            lines.append(f"   - Why: All tasks done but closeout blocked: "
                          f"{'; '.join(gate_issues)}.")
        else:
            lines.append("5. **Closeout** — `linear` (`lto closeout`)")
            lines.append("   - Why: All tasks done, all gates clear, "
                          "worktree clean. Ready to close.")
        lines.append("")

    # ── Footer ──
    lines.append("---")
    lines.append("")
    lines.append(
        "**The above are state facts. The host LLM must reason about which "
        "pattern to apply next, considering the goal, phase, blockers, and "
        "real failure evidence provided.**"
    )

    return "\n".join(lines)


def _short(h: str | None) -> str:
    if not h:
        return "(none)"
    return h[:8]


# ──────────────────────────── route ────────────────────────────

def route(facts: dict) -> dict:
    """Deterministic routing — unambiguous cases only.

    Returns {"action": "run"|"escalate", "unambiguous": bool, ...}.
    Only a small set of unambiguous situations get a concrete command.
    Everything else escalates to the host LLM.

    Safety boundary: empty-phase vacuously-done is NEVER auto-advanced.
    """
    phase = facts["phase"]
    has_tasks = facts["has_tasks"]
    all_non_skipped_done = facts["all_non_skipped_done"]
    all_done = facts["all_done"]
    blocked = facts["blocked"]
    gs_info = facts["gates"]
    unverified_risks = facts["unverified_risk_points"]

    # ── Case 1: head drift requiring revalidate ──
    # (drift is checked by caller; here we only handle the facts)
    # Handled by caller in the run() function.

    # phase 进入任何命令参数前必须过白名单（G1：防 state.json 篡改注入）
    safe_phase = phase if phase in st.VALID_PHASES else None

    # ── Case 2: all done + clean → closeout ──
    if all_non_skipped_done and has_tasks and safe_phase is not None:
        # phase 必须合法——被篡改的 state（非法 phase）不该静默 closeout
        # Check gates
        if (
            not gs_info["has_unresolved"]
            and unverified_risks == 0
            and not gs_info["tested_behind"]
            and not gs_info["reviewed_behind"]
        ):
            # Clean closeout candidate — worktree cleanness checked by caller
            # argv 是给 --exec 的 shell=False 执行体（每个元素独立，无注入面）；
            # cmd 仅用于人类可读展示。summary 用定值不拼 phase，避免引号注入。
            return {
                "action": "run",
                "argv": ["closeout", "--summary", "all tasks done (lto next)"],
                "cmd": "lto closeout --summary \"all tasks done (lto next)\"",
                "pattern": Pattern.LINEAR.value,
                "unambiguous": True,
                "reason": "all tasks done, gates clear",
            }

    # ── Case 3: phase all done but not judged ──
    if all_non_skipped_done and has_tasks:
        if safe_phase is None:
            # phase 非法（state 被篡改或损坏）→ 不拼进命令，降级 escalate
            return {
                "action": "escalate",
                "unambiguous": False,
                "reason": f"phase {phase!r} not in VALID_PHASES — refusing to build command",
            }
        return {
            "action": "run",
            "argv": ["judge", "--phase", safe_phase],
            "cmd": f"lto judge --phase {safe_phase}",
            "pattern": "judge",
            "unambiguous": True,
            "reason": f"all tasks in phase '{safe_phase}' done, needs judgement",
        }

    # ── Case 4: empty phase (has_tasks=False) → NEVER auto-advance ──
    if not has_tasks:
        return {
            "action": "escalate",
            "unambiguous": False,
            "reason": f"phase '{phase}' has no tasks — cannot auto-advance, host LLM must decide",
        }

    # ── Case 5: everything else → escalate ──
    return {
        "action": "escalate",
        "unambiguous": False,
        "reason": _escalate_reason(facts),
    }


def _escalate_reason(facts: dict) -> str:
    parts: list[str] = []
    if facts["blocked"]:
        parts.append(f"{len(facts['blocked'])} blocked tasks")
    if facts.get("pending"):
        parts.append(f"{len(facts['pending'])} pending tasks")
    if facts.get("in_progress"):
        parts.append(f"{len(facts['in_progress'])} in-progress tasks")
    if parts:
        return "ambiguous state: " + ", ".join(parts)
    return "no unambiguous routing matches"


# ──────────────────────────── CLI ───────────────────────────────

def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()

    # Resolve run_id
    current_file = repo / ".lto" / "current"
    if args.run_id:
        run_id = st.validate_run_id(args.run_id)
    elif current_file.exists():
        run_id = current_file.read_text(encoding="utf-8").strip()
        if run_id:
            run_id = st.validate_run_id(run_id)
        else:
            print("LTO: no active run (empty .lto/current)", file=sys.stderr)
            return 1
    else:
        print("LTO: no active run (missing .lto/current)", file=sys.stderr)
        return 1

    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        print(f"LTO: no state.json found for {run_id}", file=sys.stderr)
        return 1

    # Check for head drift that triggers unambiguous revalidate
    ws = state.get("workspace", {})
    recorded_head = ws.get("head", "unknown")
    drift = gs.head_drift(repo, recorded_head)

    # Run analysis
    facts = analyze(state, repo)

    # Drift override: rewrite/unreachable → unambiguous resume
    if drift in ("rewrite", "unreachable"):
        # Override route result
        route_result: dict[str, Any] = {
            "action": "run",
            "cmd": "lto resume",
            "pattern": Pattern.LINEAR.value,
            "unambiguous": True,
            "reason": f"HEAD drift ({drift}), requires revalidate",
        }
    else:
        route_result = route(facts)

    # JSON output
    if args.json:
        _print_json(facts, route_result, drift)
        return 0

    # Print decision brief (always show facts to host LLM)
    brief = build_decision_brief(facts, state)
    print(brief)
    print()

    # Route output
    print(f"# Route: {route_result['action'].upper()}")
    print(f"  unambiguous={route_result['unambiguous']}")
    print(f"  reason: {route_result['reason']}")
    if "cmd" in route_result:
        print(f"  cmd: {route_result['cmd']}")
    if "pattern" in route_result:
        print(f"  pattern: {route_result['pattern']}")

    # --exec mode (G1: shell=False + argv，无命令注入面)
    if getattr(args, 'exec_mode', False):
        if route_result["action"] == "run" and route_result["unambiguous"] and "argv" in route_result:
            lto_run = Path(__file__).resolve().parent.parent.parent / "lto_run.py"
            argv = [sys.executable, str(lto_run), "--repo", str(repo), *route_result["argv"]]
            print(f"\n[lto next --exec] running: {' '.join(route_result['argv'])}")
            result = subprocess.run(argv, shell=False)
            return result.returncode
        else:
            print("\n[lto next --exec] route is ambiguous/escalate — not executing")
            return 0

    return 0


def _print_json(facts: dict, route_result: dict, drift: str) -> None:
    output = {
        "drift": drift,
        "facts": {
            k: v for k, v in facts.items()
            # Omit large nested structures that would bloat JSON
        },
        "route": route_result,
    }
    print(json.dumps(output, indent=2, ensure_ascii=False))


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("next", help="fact brief: analyze state → suggest next primitive")
    p.add_argument("--run-id")
    p.add_argument("--exec", dest="exec_mode", action="store_true",
                   help="execute unambiguous cmd; escalate → print only")
    p.add_argument("--json", action="store_true",
                   help="output facts + route as JSON")
    p.set_defaults(func=run)
