"""lto closeout — 闭环 + 写 handoff。"""

from __future__ import annotations

import argparse
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import artifacts as af
from .. import interventions as iv
from .. import safe_emit

from .audit import _is_high_risk


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    current_file = repo / ".lto" / "current"
    if args.run_id:
        run_id = st.validate_run_id(args.run_id)
    elif current_file.exists():
        run_id = current_file.read_text(encoding="utf-8").strip()
        if run_id:
            run_id = st.validate_run_id(run_id)
        else:
            raise SystemExit("no active run")
    else:
        raise SystemExit("no active run")

    target_dir = repo / ".lto" / run_id
    state_path = target_dir / "state.json"
    md_path = target_dir / "run-state.md"

    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"missing state.json: {state_path}")
    if not md_path.exists():
        raise SystemExit(f"missing run-state.md: {md_path}")

    # Gate: ledger convergence
    ledger_path = target_dir / "audit-ledger.md"
    if ledger_path.exists() and not args.force:
        import subprocess, sys as _sys
        ledger_check = Path(__file__).resolve().parent.parent.parent / "audit_ledger_check.py"
        if ledger_check.exists():
            proc = subprocess.run(
                [_sys.executable, str(ledger_check), str(ledger_path)],
                capture_output=True, text=True,
            )
            for line in proc.stdout.splitlines():
                if line.startswith("verdict:"):
                    verdict = line.split(":", 1)[1].strip()
                    if verdict != "CONVERGED":
                        raise SystemExit(
                            f"closeout refused: ledger verdict is {verdict}, not CONVERGED "
                            "(use --force to override)"
                        )

    # Gate: unresolved blocks
    unresolved = state.get("gates", {}).get("unresolved_blocks", [])
    if unresolved and not args.force:
        raise SystemExit(f"closeout refused: {len(unresolved)} unresolved blocks (use --force)")

    # Gate: dirty worktree
    if not gs.is_git_repo(repo):
        raise SystemExit("closeout requires a git worktree")
    if gs.git_dirty(repo) and not args.allow_dirty:
        iv.append(
            repo, run_id,
            type="intervention_candidate",
            category="dirty_closeout_blocked",
            reason="closeout blocked by dirty worktree outside .lto",
            source="lto closeout",
            meaningful=False,
            avoidable=True,
            preventable=True,
            actor="gate",
            gate="closeout",
            details={"suggested_action": "commit_or_stash_then_closeout_no_changelog"},
        )
        raise SystemExit(
            "closeout refused: uncommitted changes outside .lto. "
            "Commit or stash code changes first; use --no-changelog after commit "
            "for admin closeout without new tracked dirt."
        )

    # Gate: already closed
    if state.get("current_phase") == "closed" and not args.force:
        raise SystemExit("run already closed (use --force to rewrite)")

    # Gate: risk coverage — all registered risk points must be verified
    risk_points = state.get("risk_points", [])
    if risk_points and not args.force:
        unverified = [
            rp for rp in risk_points
            if rp.get("disposition") == "open" and not rp.get("verified_by")
        ]
        if unverified:
            raise SystemExit(
                f"closeout refused: {len(unverified)} risk points unverified "
                f"(use --force to override)"
            )

    # Gate: high-risk run must have audit ledger with real rounds
    # (空 ledger 或无 ledger 的高风险 run 不能 closeout)
    if not args.force:
        has_high_risk = any(_is_high_risk(t) for t in state.get("tasks", []))
        if has_high_risk:
            if not ledger_path.exists():
                raise SystemExit(
                    "closeout refused: high-risk run has no audit-ledger.md "
                    "(run lto audit first, or use --force to override)"
                )
            if not _has_real_ledger_rounds(ledger_path.read_text(encoding="utf-8")):
                raise SystemExit(
                    "closeout refused: high-risk run has empty audit ledger "
                    "(run lto audit first, or use --force to override)"
                )

    # Force is a real human override. Log it before state changes so closeout
    # reports the intervention in the handoff summary.
    if args.force:
        iv.append(
            repo, run_id,
            type="human_intervention",
            category="force_closeout",
            reason="operator used --force to bypass one or more closeout gates",
            source="lto closeout",
            meaningful=True,
            avoidable=False,
            preventable=False,
            actor="operator",
            gate="closeout",
            details={"phase": state.get("current_phase", "unknown")},
            dedupe_key=f"closeout:force:{run_id}",
        )

    # Update state
    head = gs.git_head(repo)
    branch = gs.git_branch(repo)
    prev_phase = state.get("current_phase")
    st.transition_phase(state, "closed", head)
    if prev_phase != "closed":
        safe_emit(
            repo, run_id, type="phase.changed", actor_kind="host",
            phase="closed", object_id=run_id, object_type="run",
            summary=f"phase {prev_phase} -> closed",
            fields={"from_phase": prev_phase, "to_phase": "closed"},
        )
    state["workspace"]["head"] = head
    state["workspace"]["branch"] = branch
    state["blocked_by"] = args.blocked_by
    state["next_action"] = args.next_action

    st.save_state(state_path, state)

    # Update run-state.md with closeout section
    md_content = md_path.read_text(encoding="utf-8")
    md_content = md_content.split("\n## Closeout\n", 1)[0].rstrip()
    md_content += f"\n\n## Closeout\n\n- closed_at: {st.iso_now()}\n- summary: {st.single_line(args.summary)}\n- next_action: {st.single_line(args.next_action)}\n"
    md_path.write_text(md_content, encoding="utf-8")
    af.register_path(
        repo, run_id, state_path, kind="state_json",
        producer="lto.commands.closeout", state=state,
        summary="machine state at closeout", tags=["state"],
    )
    af.register_path(
        repo, run_id, md_path, kind="run_state_md",
        producer="lto.commands.closeout", state=state,
        summary="human-readable state at closeout", tags=["state"],
    )

    # Generate changelog unless caller is doing a post-commit administrative
    # closeout and wants to avoid creating new tracked dirt.
    if not args.no_changelog:
        _write_changelog(repo, run_id, state, args)
        if (repo / "CHANGELOG.md").exists():
            af.register_path(
                repo, run_id, repo / "CHANGELOG.md", kind="changelog",
                producer="lto.commands.closeout", state=state,
                summary="repo changelog updated", tags=["closeout", "changelog"],
            )

    interventions_path = target_dir / "interventions.jsonl"
    if interventions_path.exists():
        af.register_path(
            repo, run_id, interventions_path, kind="interventions",
            producer="lto.commands.closeout", state=state,
            summary="human intervention log", tags=["closeout", "interventions"],
        )
    intervention_summary = iv.render_summary(repo, run_id)

    # Write handoff.md from manifest, then register and rewrite once so the
    # handoff itself appears in the artifact list too.
    handoff_path = target_dir / "handoff.md"
    entries = af.load_manifest(repo, run_id, state=state).get("artifacts", [])
    handoff_path.write_text(_build_handoff(run_id, state, args, head, branch, entries, intervention_summary), encoding="utf-8")
    af.register_path(
        repo, run_id, handoff_path, kind="handoff",
        producer="lto.commands.closeout", state=state,
        summary="closeout handoff", tags=["closeout", "handoff"],
    )
    entries = af.load_manifest(repo, run_id, synthesize=False).get("artifacts", [])
    handoff_path.write_text(_build_handoff(run_id, state, args, head, branch, entries, intervention_summary), encoding="utf-8")
    af.register_path(
        repo, run_id, handoff_path, kind="handoff",
        producer="lto.commands.closeout", state=state,
        summary="closeout handoff", tags=["closeout", "handoff"],
    )

    # Emit run.closed BEFORE auto-commit (review #6) so the event line is part
    # of the committed .lto snapshot when --auto-commit is on, leaving no
    # post-commit dirt — preserving Phase 1 "no behavior change".
    safe_emit(
        repo, run_id, type="run.closed", actor_kind="host",
        phase="closed", object_id=run_id, object_type="run",
        summary=st.single_line(args.summary),
    )

    # Optionally commit produced artifacts (opt-in; default off — closeout writes
    # CHANGELOG.md and .lto, both the user's real files, so never commit silently)
    closeout_paths = [".lto"] if args.no_changelog else [".lto", "CHANGELOG.md"]
    gs.auto_commit_lto(
        repo,
        f"lto: closeout {run_id[:8]}",
        paths=closeout_paths,
        enabled=args.auto_commit,
    )

    print(target_dir / "handoff.md")
    print(intervention_summary)
    return 0


def _build_handoff(
    run_id: str, state: dict, args: argparse.Namespace, head: str, branch: str,
    entries: list[dict], intervention_summary: str,
) -> str:
    header = "\n".join([
        "# LTO Handoff",
        "",
        f"- run_id: {run_id}",
        f"- goal: {state.get('goal', '?')}",
        "- status: closed",
        f"- closed_at: {st.iso_now()}",
        f"- git_head: {head}",
        f"- branch: {branch}",
        f"- blocked_by: {args.blocked_by}",
        f"- summary: {st.single_line(args.summary)}",
        f"- next_action: {st.single_line(args.next_action)}",
        f"- intervention_summary: {intervention_summary}",
        f"- token_usage: {_token_usage_line(state)}",
        "",
    ])
    ordered = sorted(entries, key=lambda e: (e.get("kind", ""), e.get("relative_path", "")))
    return header + af.render_markdown(ordered, title="Artifacts") + "\n"


def _token_usage_line(state: dict) -> str:
    """Compact per-run token rollup for the handoff header (machine-friendly)."""
    roll = st.token_rollup(state)
    if roll["runs_total"] == 0:
        return "no agent runs"
    if roll["total_tokens"] == 0:
        return f"unmetered ({roll['runs_total']} runs, no runner reported tokens)"
    by = ", ".join(
        f"{r}={s['tokens']}"
        for r, s in sorted(roll["by_runner"].items(), key=lambda kv: -kv[1]["tokens"])
        if s["tokens"] > 0
    )
    return (
        f"{roll['total_tokens']} total "
        f"(in={roll['tokens_in']}, out={roll['tokens_out']}; "
        f"{roll['runs_with_tokens']}/{roll['runs_total']} runs metered; {by})"
    )


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("closeout", help="mark run closed and write handoff.md")
    p.add_argument("--run-id")
    p.add_argument("--summary", required=True)
    p.add_argument("--next-action", default="none")
    p.add_argument("--blocked-by", default="none")
    p.add_argument("--allow-dirty", action="store_true")
    p.add_argument("--no-changelog", action="store_true",
                   help="skip CHANGELOG.md update for post-commit/admin closeout")
    p.add_argument("--auto-commit", action="store_true",
                   help="commit generated closeout files (opt-in; default off, uses repo git identity)")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=run)


def _has_real_ledger_rounds(ledger_text: str) -> bool:
    """Check whether the ledger has real Round rows (not just placeholder R1
    with empty count cells).  Mirrors the filtering logic in
    audit_ledger_check.extract_rounds: rows with empty high *and* critical
    columns are skipped.
    """
    for line in ledger_text.splitlines():
        stripped = line.strip()
        if not (stripped.startswith("| R") or stripped.startswith("|R")):
            continue
        # Split cells: strip leading/trailing |, split on |
        cells = [c.strip() for c in stripped.strip("|").split("|")]
        if len(cells) < 5:
            continue
        # High = col 3, Critical = col 4 (0-indexed after stripping leading |)
        high_cell = cells[3] if len(cells) > 3 else ""
        crit_cell = cells[4] if len(cells) > 4 else ""
        if not high_cell and not crit_cell:
            continue  # placeholder row, skip
        if any(c.isdigit() for c in high_cell + crit_cell):
            return True
    return False


def _write_changelog(repo: Path, run_id: str, state: dict, args) -> None:
    """Generate human-friendly changelog from state.json evidence."""
    changelog_path = repo / "CHANGELOG.md"
    goal = state.get("goal", "unknown")
    tasks = state.get("tasks", [])

    lines = [
        f"## {goal}",
        "",
        f"- **Run ID**: `{run_id}`",
        f"- **Closed**: {st.iso_now()}",
        f"- **Summary**: {st.single_line(args.summary)}",
        "",
    ]

    if tasks:
        lines.append("### Tasks")
        lines.append("")
        for task in tasks:
            status_icon = {"done": "✅", "blocked": "🚧", "in_progress": "🔄", "pending": "⏸", "skipped": "⏭"}.get(task["status"], "❓")
            lines.append(f"- {status_icon} **{task['id']}**: {task['title']} ({task['status']})")
            for ev_entry in task.get("evidence", []):
                icon = "✅" if ev_entry.get("rc") == 0 else "❌"
                lines.append(f"  - {icon} [{ev_entry.get('kind', '?')}] {ev_entry.get('summary', ev_entry.get('command', '')[:80])}")
            for blocker in task.get("blockers", []):
                lines.append(f"  - 🚧 blocked: {blocker.get('reason', 'unknown')}")
        lines.append("")

    if state.get("blocked_by") and state["blocked_by"] != "none":
        lines.append(f"**Blocked by**: {state['blocked_by']}")
        lines.append("")

    if state.get("next_action") and state["next_action"] != "none":
        lines.append(f"**Next**: {state['next_action']}")
        lines.append("")

    # Prepend to existing CHANGELOG.md or create new
    if changelog_path.exists():
        existing = changelog_path.read_text(encoding="utf-8")
        # Insert after title line
        title_end = existing.find("\n")
        if existing.startswith("#") and title_end > 0:
            changelog_path.write_text(
                existing[:title_end + 1] + "\n" + "\n".join(lines) + "\n" + existing[title_end + 1:],
                encoding="utf-8",
            )
        else:
            changelog_path.write_text("\n".join(lines) + "\n" + existing, encoding="utf-8")
    else:
        changelog_path.write_text(
            "# Changelog\n\n" + "\n".join(lines) + "\n",
            encoding="utf-8",
        )
    # Note: committing CHANGELOG.md is handled by the caller via auto_commit_lto
    # (opt-in). This function only writes the file.
