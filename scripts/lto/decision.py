#!/usr/bin/env python3
"""decision.py - dual-track convergence engine for LTO autonomous escalation.

Design contract:
- Zero LLM embeds, zero API keys. This module spawns heterogeneous agents,
  tallies votes, merges findings, and produces a decision brief for the
  HOST agent to read and reason about.
- Closed output schema for spawned agents: only pick_task:<id>, pick_pattern:<enum>,
  or needs_human. No freeform.
- Injection defense: task_id returned by agent is looked up in state.json to
  retrieve the actual command. Agent-returned command strings are NEVER executed.

Budget:
  Real token counting is available when a runner writes a <reply>.meta.json
  token sidecar (codex does so under CODEX_JSON=1; scheduler merges it into
  AgentResult.cost.tokens). When absent, budget falls back to an estimate:
  rounds-remaining × estimated tokens-per-round. Either way declared honestly
  in the brief footer.

Tracks:
  direction → spawn 3 heterogeneous agents → tally_votes (2/3 majority)
  review    → spawn 3 heterogeneous agents → merge_findings (union, no voting)
  both      → run both tracks, dual-section brief
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from lto import state as st
from lto import artifacts as af
from lto.agent_job import AgentJob, AgentResult, Budget, Pattern
from lto.auditors import _pick_auditors, parse_findings_text
from lto.decision_brief import (
    build_decision_brief_v2,
    build_budget_exhausted_brief,
    build_needs_human_brief,
)
from lto import agent_exec


# ──────────────────────── constants ────────────────────────────

# Closed output schema for direction-track agents (G3: no freeform)
DIRECTION_OUTPUT_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "decision": {
            "type": "string",
            "enum": ["pick_task", "pick_pattern", "needs_human"],
        },
        "value": {"type": "string"},
        "reasoning": {"type": "string"},
    },
    "required": ["decision", "value", "reasoning"],
}

# Closed output schema for review-track agents (findings, not votes)
REVIEW_OUTPUT_SCHEMA: dict[str, Any] = {
    "type": "array",
    "items": {
        "type": "object",
        "properties": {
            "severity": {
                "type": "string",
                "enum": ["critical", "high", "medium", "low"],
            },
            "claim": {"type": "string"},
            "evidence_to_check": {"type": "string"},
            "file": {"type": "string"},
        },
        "required": ["severity", "claim"],
    },
}

# Approximate token budget per agent round (declared in brief footer as estimate)
# Based on: ~4K prompt + ~2K reply ≈ 6K tokens per agent, 3 agents ≈ 18K per round.
_EST_TOKENS_PER_AGENT = 6_000
_EST_TOKENS_PER_ROUND = _EST_TOKENS_PER_AGENT * 3  # 3 agents

# Escalate point tracking key in state metadata
_ESCALATE_POINT_KEY = "decision_escalate_points"


# ──────────────────────── public API ────────────────────────────

def run_decision(
    repo: Path,
    run_id: str,
    facts: dict,
    state: dict,
    decision_kind: str,
    budget_remaining: int,
    *,
    runners_dir: Path | None = None,
) -> dict[str, Any]:
    """Spawn tri-partite heterogeneous agents, converge or merge.

    Parameters
    ----------
    repo:
        Repository root.
    run_id:
        LTO run identifier.
    facts:
        Output of next.analyze(state, repo).
    state:
        Full state dict (may be mutated to record escalate points).
    decision_kind:
        "direction" (vote), "review" (union merge), or "both" (run both tracks).
    budget_remaining:
        Estimated tokens remaining in the autopilot budget. Approximate unless
        runners emit token sidecars (then cost.tokens carries real usage).
    runners_dir:
        Override runners directory (testing).

    Returns
    -------
    {
        "status": "converged" | "needs_info" | "needs_human",
        "kind": str,
        "result": dict | list | None,
        "dissent": dict | list,
        "brief": str,  # Markdown for host LLM to read
        "dispatched_to": list[str],
        "budget_consumed_est": int,
    }
    """
    host = state.get("host_runtime", "unknown")
    auditors = _pick_auditors(host)

    # ── budget check ──
    rounds_needed = 2 if decision_kind == "both" else 1
    est_cost = _EST_TOKENS_PER_ROUND * rounds_needed
    if budget_remaining < est_cost:
        return _budget_exhausted_result(decision_kind, auditors, budget_remaining, est_cost)

    # ── escalate-point dedup (G5: same escalate point max 1 spawn) ──
    escalate_key = _build_escalate_key(facts)
    if _has_spawned_before(state, escalate_key):
        return _needs_human_result(
            decision_kind, auditors,
            "same escalate point already spawned once - refusing re-spawn (G5 limit)"
        )

    # ── dispatch ──
    dispatched: list[str] = []
    budget_spent = 0

    direction_result = None
    review_result = None

    if decision_kind in ("direction", "both"):
        direction_result = _run_direction_track(
            repo, run_id, facts, state, auditors, runners_dir
        )
        dispatched = direction_result.get("dispatched_to", auditors)
        budget_spent += _EST_TOKENS_PER_ROUND

        # FIX-4: need >= 2 valid reviewers for multi-perspective
        dir_valid = sum(
            1 for r in direction_result.get("results", [])
            if r.status == "ok" and (r.reply_text or "").strip()
        )
        if dir_valid < 2:
            _record_spawn(state, escalate_key)
            return _needs_human_result(
                decision_kind, auditors,
                f"direction track: 有效异构审者不足（实际 {dir_valid} 家），无法构成多视角，请人工决策"
            )

    if decision_kind in ("review", "both"):
        review_result = _run_review_track(
            repo, run_id, facts, state, auditors, runners_dir
        )
        if not dispatched:
            dispatched = review_result.get("dispatched_to", auditors)
        budget_spent += _EST_TOKENS_PER_ROUND

        # FIX-4: need >= 2 valid reviewers for multi-perspective
        rev_valid = sum(
            1 for r in review_result.get("results", [])
            if r.status == "ok" and (r.reply_text or "").strip()
        )
        if rev_valid < 2:
            _record_spawn(state, escalate_key)
            return _needs_human_result(
                decision_kind, auditors,
                f"review track: 有效异构审者不足（实际 {rev_valid} 家），无法构成多视角，请人工决策"
            )

    # ── record escalate point ──
    _record_spawn(state, escalate_key)

    # ── compose result ──
    return _compose_result(
        decision_kind, direction_result, review_result,
        dispatched, budget_spent, facts, state, host,
    )


# ──────────────────────── direction track (voting) ──────────────

def _run_direction_track(
    repo: Path,
    run_id: str,
    facts: dict,
    state: dict,
    auditors: list[str],
    runners_dir: Path | None,
) -> dict:
    """Spawn 3 agents with closed direction schema, tally votes."""
    brief_path = _write_direction_brief(repo, run_id, facts, state)

    jobs = []
    for auditor in auditors:
        job = AgentJob(
            job_id=f"decision-dir-{auditor}",
            prompt_ref=str(brief_path),
            runner=auditor,
            output_schema=DIRECTION_OUTPUT_SCHEMA,
            budget=Budget(timeout_sec=300),
            parent_pattern=Pattern.ADVERSARIAL.value,
            meta={"host": state.get("host_runtime", "?"), "track": "direction"},
        )
        jobs.append(job)

    results = agent_exec.spawn_agents(repo, run_id, jobs, persist=False, runners_dir=runners_dir)
    _persist_decision_replies(repo, run_id, "direction", results, state)

    # Parse structured replies
    parsed: list[dict] = []
    for r in results:
        d = _parse_direction_reply(r)
        if d:
            parsed.append(d)

    # Tally — pass whitelist of valid task IDs from state (FIX-1)
    task_ids = {t["id"] for t in state.get("tasks", []) if "id" in t}
    tally = tally_votes(parsed, valid_task_ids=task_ids) if parsed else {}
    return {
        "track": "direction",
        "dispatched_to": auditors,
        "results": results,
        "parsed": parsed,
        "tally": tally,
    }


def _write_direction_brief(repo: Path, run_id: str, facts: dict, state: dict) -> Path:
    """Write a direction-decision brief for spawned agents."""
    target_dir = repo / ".lto" / run_id / "audit"
    target_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    brief_path = target_dir / f"decision-brief-dir-{ts}.md"

    tasks = state.get("tasks", [])
    goal = state.get("goal", "?")
    phase = facts.get("phase", "?")

    lines = [
        "# Direction Decision Brief",
        "",
        f"- **Goal**: {goal}",
        f"- **Phase**: {phase}",
        f"- **Host runtime**: {state.get('host_runtime', '?')}",
        "",
        "## Context",
        "",
        f"The autopilot has encountered divergence and needs a direction decision.",
        f"There are {facts.get('task_counts', {}).get('blocked', 0)} blocked tasks "
        f"and {facts.get('task_counts', {}).get('pending', 0)} pending tasks.",
        "",
        "## Available Tasks",
        "",
    ]

    for t in tasks:
        status = t.get("status", "?")
        tid = t.get("id", "?")
        title = t.get("title", "?")
        command = t.get("command", "")
        lines.append(f"- **{tid}** [{status}]: {title}")
        if command:
            lines.append(f"  - Command: `{command[:120]}`")
        blockers = t.get("blockers", [])
        for b in blockers:
            lines.append(f"  - Blocker: {b.get('reason', '?')}")

    lines += [
        "",
        "## Available Patterns",
        "",
        "- `linear`: sequential execution, one task at a time",
        "- `fan-out`: parallel independent tasks",
        "- `adversarial`: each action with independent verifier",
        "- `tournament`: multiple agents compete, best wins",
        "- `loop`: iterate until stop condition met",
        "",
        "## Instructions",
        "",
        "You are a direction-decision agent. Based on the state above, choose ONE:",
        "",
        "1. **pick_task**: pick the most critical task to work on next.",
        "   Value format: the task ID (e.g. `T1`, `T2`).",
        "2. **pick_pattern**: choose the execution pattern.",
        "   Value: one of `linear`, `fan-out`, `adversarial`, `tournament`, `loop`.",
        "3. **needs_human**: this situation requires human judgement.",
        "   Value: brief explanation of why.",
        "",
        "## Output Format",
        "",
        "Reply with valid JSON matching this schema:",
        "",
        "```json",
        json.dumps(DIRECTION_OUTPUT_SCHEMA, indent=2),
        "```",
        "",
        "Be decisive. Explain your reasoning concisely.",
    ]

    brief_path.write_text("\n".join(lines), encoding="utf-8")
    af.register_path(
        repo, run_id, brief_path, kind="decision_brief",
        producer="lto.decision.direction_brief", state=state,
        summary="direction decision brief", tags=["decision", "brief"],
    )
    return brief_path


def _persist_decision_replies(
    repo: Path, run_id: str, track: str, results: list[AgentResult], state: dict,
) -> None:
    ts = st.iso_now().replace(":", "-")[:19]
    for result in results:
        rel = f"audit/decision-replies/{track}-reply-{result.runner}-{ts}.md"
        path = af.write_text(
            repo, run_id, rel, result.reply_text or "",
            kind="decision_reply", producer="lto.decision",
            state=state, summary=f"{result.runner} {track} decision reply",
            job_id=result.job_id, runner=result.runner,
            consumed_by=["decision.merge"], tags=["decision", "reply", track],
        )
        if path not in result.artifacts:
            result.artifacts.append(path)


def _parse_direction_reply(result: AgentResult) -> dict | None:
    """Parse a direction-track agent reply into {decision, value, reasoning, source}."""
    text = result.reply_text or ""
    if not text:
        return None

    # Try whole-file JSON
    try:
        data = json.loads(text)
        if isinstance(data, dict) and "decision" in data:
            data["source"] = result.runner
            return data
    except (json.JSONDecodeError, ValueError):
        pass

    # Try ```json fence
    import re
    blocks = re.findall(r'```json\s*\n(.*?)\n```', text, re.DOTALL)
    for block in blocks:
        try:
            data = json.loads(block)
            if isinstance(data, dict) and "decision" in data:
                data["source"] = result.runner
                return data
        except (json.JSONDecodeError, ValueError):
            continue

    return None


def tally_votes(parsed_replies: list[dict], *, valid_task_ids: set[str] | None = None) -> dict[str, Any]:
    """Count votes for direction decisions (2/3 majority rule).

    Parameters
    ----------
    parsed_replies:
        List of {decision, value, reasoning, source} from agents.
    valid_task_ids:
        Whitelist of legal task IDs from state.tasks. pick_task values
        not in this set are rejected (injection defense, FIX-1).
        If None, task ID validation is skipped (backward compat).

    Returns
    -------
    {
        "majority_pick": str | None,        # winning pick (if 2/3+)
        "majority_count": int,
        "total_voters": int,                # valid voters only
        "supermajority_met": bool,          # True if >= 2/3
        "votes": list[dict],                # valid votes with source
        "minority": list[dict],             # dissenting votes
        "invalid_votes": list[dict],        # rejected votes (bad task_id / pattern)
        "invalid_votes_count": int,
        "needs_human_votes": int,
        "needs_info": bool,                 # tie, full disagreement, or any needs_human
    }
    """
    _LEGAL_PATTERNS = ("linear", "fan-out", "adversarial", "tournament", "loop")

    if not parsed_replies:
        return {
            "majority_pick": None,
            "majority_count": 0,
            "total_voters": 0,
            "supermajority_met": False,
            "votes": [],
            "minority": [],
            "invalid_votes": [],
            "invalid_votes_count": 0,
            "needs_human_votes": 0,
            "needs_info": True,
        }

    # ── filter invalid votes (FIX-1: injection defense) ──
    invalid_votes: list[dict] = []
    valid_replies: list[dict] = []

    for reply in parsed_replies:
        decision = reply.get("decision", "")
        value = reply.get("value", "")

        if decision == "pick_task":
            if valid_task_ids is not None and value not in valid_task_ids:
                invalid_votes.append(dict(reply))
                continue
        elif decision == "pick_pattern":
            if value not in _LEGAL_PATTERNS:
                invalid_votes.append(dict(reply))
                continue
        # needs_human always valid (no value whitelist needed)

        valid_replies.append(dict(reply))

    total = len(valid_replies)

    votes: list[dict] = []
    needs_human_votes = 0

    for reply in valid_replies:
        source = reply.get("source", "?")
        decision = reply.get("decision", "")
        value = reply.get("value", "")
        reasoning = reply.get("reasoning", "")

        vote = {
            "source": source,
            "decision": decision,
            "value": value,
            "reasoning": reasoning,
        }
        votes.append(vote)

        if decision == "needs_human":
            needs_human_votes += 1

    # Count pick_task and pick_pattern votes by value
    # Union: "pick_task:T1" and "pick_pattern:linear" are in same tally pool
    # but we separate by decision type for clarity
    pick_counts: dict[str, int] = {}
    for v in votes:
        if v["decision"] in ("pick_task", "pick_pattern"):
            key = f"{v['decision']}:{v['value']}"
            pick_counts[key] = pick_counts.get(key, 0) + 1

    # Find majority (>= 2/3)
    threshold = 2  # 2 out of 3
    majority_pick = None
    majority_count = 0
    for key, count in pick_counts.items():
        if count >= threshold and count > majority_count:
            majority_pick = key
            majority_count = count

    supermajority_met = majority_count >= threshold

    # Minority = valid votes not matching majority pick
    minority_picks: list[dict] = []
    if majority_pick:
        for v in votes:
            key = f"{v['decision']}:{v['value']}"
            if key != majority_pick and v["decision"] != "needs_human":
                minority_picks.append(v)

    # needs_info conditions:
    #   - supermajority not met (tie, full disagreement)
    #   - any needs_human vote (FIX-2: one-vote veto)
    needs_info = (not supermajority_met) or (needs_human_votes >= 1)

    return {
        "majority_pick": majority_pick,
        "majority_count": majority_count,
        "total_voters": total,
        "supermajority_met": supermajority_met,
        "votes": votes,
        "minority": minority_picks,
        "invalid_votes": invalid_votes,
        "invalid_votes_count": len(invalid_votes),
        "needs_human_votes": needs_human_votes,
        "needs_info": needs_info,
    }


# ──────────────────────── review track (union merge) ────────────

def _run_review_track(
    repo: Path,
    run_id: str,
    facts: dict,
    state: dict,
    auditors: list[str],
    runners_dir: Path | None,
) -> dict:
    """Spawn 3 agents with review schema, union-merge findings."""
    brief_path = _write_review_brief(repo, run_id, facts, state)

    jobs = []
    for auditor in auditors:
        job = AgentJob(
            job_id=f"decision-rev-{auditor}",
            prompt_ref=str(brief_path),
            runner=auditor,
            output_schema=REVIEW_OUTPUT_SCHEMA,
            budget=Budget(timeout_sec=300),
            parent_pattern=Pattern.ADVERSARIAL.value,
            meta={"host": state.get("host_runtime", "?"), "track": "review"},
        )
        jobs.append(job)

    results = agent_exec.spawn_agents(repo, run_id, jobs, persist=False, runners_dir=runners_dir)
    _persist_decision_replies(repo, run_id, "review", results, state)

    # Merge findings (union)
    merged = merge_findings(results)

    return {
        "track": "review",
        "dispatched_to": auditors,
        "results": results,
        "merged_findings": merged,
    }


def _write_review_brief(repo: Path, run_id: str, facts: dict, state: dict) -> Path:
    """Write a review/risk-discovery brief for spawned agents."""
    target_dir = repo / ".lto" / run_id / "audit"
    target_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    brief_path = target_dir / f"decision-brief-rev-{ts}.md"

    tasks = state.get("tasks", [])
    goal = state.get("goal", "?")
    phase = facts.get("phase", "?")

    lines = [
        "# Review / Risk Discovery Brief",
        "",
        f"- **Goal**: {goal}",
        f"- **Phase**: {phase}",
        f"- **Host runtime**: {state.get('host_runtime', '?')}",
        "",
        "## Current State",
        "",
        f"Blocked tasks: {facts.get('task_counts', {}).get('blocked', 0)}",
        f"Pending tasks: {facts.get('task_counts', {}).get('pending', 0)}",
        f"In-progress tasks: {facts.get('task_counts', {}).get('in_progress', 0)}",
        "",
        "## Tasks",
        "",
    ]

    for t in tasks:
        status = t.get("status", "?")
        tid = t.get("id", "?")
        title = t.get("title", "?")
        touched = t.get("touched_files", [])
        lines.append(f"- **{tid}** [{status}]: {title}")
        if touched:
            lines.append(f"  - Files: {', '.join(touched[:8])}")

    lines += [
        "",
        "## Instructions",
        "",
        "You are a review / risk-discovery agent. Your job is to find issues,",
        "risks, and problems - NOT to vote on a direction.",
        "",
        "Focus areas:",
        "- Concurrency / race conditions",
        "- Missing error handling / rollback",
        "- API contract breaks",
        "- Security / auth gaps",
        "- State corruption / partial failure risks",
        "- Unregistered risk points the developer may have missed",
        "",
        "Each finding you report will be UNION-merged with findings from other agents.",
        "Your job is coverage - find what others might miss. Do NOT hold back.",
        "",
        "## Output Format",
        "",
        "Reply with a JSON array of findings:",
        "",
        "```json",
        json.dumps(REVIEW_OUTPUT_SCHEMA, indent=2),
        "```",
        "",
        "No findings → output empty array `[]`.",
    ]

    brief_path.write_text("\n".join(lines), encoding="utf-8")
    af.register_path(
        repo, run_id, brief_path, kind="decision_brief",
        producer="lto.decision.review_brief", state=state,
        summary="review decision brief", tags=["decision", "brief"],
    )
    return brief_path


def merge_findings(results: list[AgentResult]) -> list[dict]:
    """Union-merge structured findings from review-track agents.

    Each finding gets a `source` field indicating which agent found it.
    Findings are NOT deduplicated - the host agent does semantic dedup.
    """
    all_findings: list[dict] = []

    for r in results:
        source = r.runner

        # Try structured findings first
        if r.findings:
            for f in r.findings:
                f = dict(f)
                f["source"] = source
                all_findings.append(f)
            continue

        # Try parsing reply text as structured JSON
        parsed = parse_findings_text(r.reply_text or "")
        if parsed is not None:
            for f in parsed:
                f = dict(f)
                f["source"] = source
                all_findings.append(f)
            continue

        # Fallback: raw reply as one finding (skip empty/trivial replies like "[]")
        reply = (r.reply_text or "").strip()
        if reply and reply not in ("[]", "{}", "null", ""):
            all_findings.append({
                "severity": "medium",
                "claim": f"[unparsed reply from {source}]",
                "evidence_to_check": reply[:500],
                "file": "",
                "source": source,
            })

    return all_findings

# ──────────────────────── escalate-point dedup ──────────────────

def _build_escalate_key(facts: dict) -> str:
    """Build a stable key from the escalate context.

    Uses: phase + blocked task IDs + pending task IDs.
    This key identifies "same situation" to prevent re-spawn loops (G5).
    """
    phase = facts.get("phase", "?")
    blocked_ids = sorted(t["id"] for t in facts.get("blocked", []))
    pending_ids = sorted(t["id"] for t in facts.get("pending", []))
    return f"{phase}|blocked={','.join(blocked_ids)}|pending={','.join(pending_ids)}"


def _has_spawned_before(state: dict, escalate_key: str) -> bool:
    """Check if this escalate point was already spawned in this run."""
    points: dict = state.get(_ESCALATE_POINT_KEY, {})
    return escalate_key in points


def _record_spawn(state: dict, escalate_key: str) -> None:
    """Record that we spawned for this escalate point."""
    state.setdefault(_ESCALATE_POINT_KEY, {})
    state[_ESCALATE_POINT_KEY][escalate_key] = st.iso_now()


# ──────────────────────── result composition ────────────────────

def _compose_result(
    decision_kind: str,
    direction_result: dict | None,
    review_result: dict | None,
    dispatched: list[str],
    budget_spent: int,
    facts: dict,
    state: dict,
    host: str,
) -> dict[str, Any]:
    """Compose final result dict from track outputs."""
    # Determine status
    direction_converged = False
    direction_needs_info = False

    if direction_result:
        tally = direction_result.get("tally", {})
        direction_converged = tally.get("supermajority_met", False)
        direction_needs_info = tally.get("needs_info", False)

    # Compose status
    # FIX-2: needs_info (incl. any needs_human vote) has priority over supermajority
    # FIX-3: review track always converges if enough valid reviewers; empty findings = clean
    if decision_kind == "direction":
        if direction_needs_info:
            status = "needs_info"
        elif direction_converged:
            status = "converged"
        else:
            status = "needs_info"
    elif decision_kind == "review":
        # FIX-3: empty findings = clean review = converged
        # needs_info only from FIX-4 (insufficient reviewers, handled upstream)
        status = "converged"
    else:  # both
        if direction_needs_info:
            status = "needs_info"
        elif direction_converged:
            status = "converged"
        else:
            status = "needs_info"

    # Compose result payload
    result_payload: Any = None
    if decision_kind == "direction" and direction_result:
        tally = direction_result.get("tally", {})
        if tally.get("supermajority_met"):
            result_payload = {
                "pick": tally["majority_pick"],
                "count": tally["majority_count"],
                "total": tally["total_voters"],
            }
    elif decision_kind == "review" and review_result:
        result_payload = review_result.get("merged_findings", [])
    elif decision_kind == "both":
        payload: dict[str, Any] = {}
        if direction_result:
            tally = direction_result.get("tally", {})
            payload["direction"] = {
                "converged": tally.get("supermajority_met", False),
                "pick": tally.get("majority_pick"),
                "votes": tally.get("votes", []),
                "minority": tally.get("minority", []),
            }
        if review_result:
            payload["review"] = {
                "findings": review_result.get("merged_findings", []),
            }
        result_payload = payload

    # Compose dissent
    dissent: Any = None
    if direction_result:
        tally = direction_result.get("tally", {})
        dissent = {
            "minority_votes": tally.get("minority", []),
            "needs_human_votes": tally.get("needs_human_votes", 0),
        }
    if review_result:
        merged = review_result.get("merged_findings", [])
        if dissent is None:
            dissent = {"findings_for_host_judgment": merged}
        else:
            dissent["findings_for_host_judgment"] = merged

    # Build brief
    brief = build_decision_brief_v2(
        decision_kind=decision_kind,
        direction_result=direction_result,
        review_result=review_result,
        facts=facts,
        state=state,
        host=host,
        dispatched=dispatched,
        status=status,
        budget_spent=budget_spent,
    )

    return {
        "status": status,
        "kind": decision_kind,
        "result": result_payload,
        "dissent": dissent,
        "brief": brief,
        "dispatched_to": dispatched,
        "budget_consumed_est": budget_spent,
    }


# Brief builder - see lto.decision_brief (split to keep decision.py under line budget)


# ──────────────────────── edge-case results ─────────────────────

def _budget_exhausted_result(
    decision_kind: str, auditors: list[str], budget_remaining: int, est_cost: int,
) -> dict[str, Any]:
    return {
        "status": "needs_human", "kind": decision_kind, "result": None,
        "dissent": {"reason": "budget_exhausted"},
        "brief": build_budget_exhausted_brief(decision_kind, auditors, budget_remaining, est_cost),
        "dispatched_to": [], "budget_consumed_est": 0,
    }


def _needs_human_result(
    decision_kind: str, auditors: list[str], reason: str,
) -> dict[str, Any]:
    return {
        "status": "needs_human", "kind": decision_kind, "result": None,
        "dissent": {"reason": reason},
        "brief": build_needs_human_brief(decision_kind, auditors, reason),
        "dispatched_to": [], "budget_consumed_est": 0,
    }
