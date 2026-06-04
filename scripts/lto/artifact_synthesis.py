"""Best-effort synthesis for old LTO runs without artifacts.json."""

from __future__ import annotations

from pathlib import Path

from . import state as st


def synthesize_manifest(repo: Path, run_id: str, state: dict | None = None) -> dict:
    from . import artifacts as af

    run_id = st.validate_run_id(run_id)
    run_dir = repo / ".lto" / run_id
    state = state if state is not None else st.load_state(run_dir / "state.json")
    manifest = af._empty_manifest(run_id, synthesized=True)
    task_by_artifact = _task_map_by_artifact(state or {})

    candidates: list[tuple[Path, str, str]] = [
        (run_dir / "state.json", "state_json", "machine state"),
        (run_dir / "run-state.md", "run_state_md", "human-readable run state"),
        (run_dir / "audit-ledger.md", "audit_ledger", "audit convergence ledger"),
        (run_dir / "handoff.md", "handoff", "closeout handoff"),
        (repo / "CHANGELOG.md", "changelog", "repo changelog"),
    ]
    candidates += [(p, "audit_reply", "audit reply") for p in sorted((run_dir / "audit" / "replies").glob("*.md"))]
    candidates += [(p, "audit_findings_json", "audit findings json") for p in sorted((run_dir / "audit" / "replies").glob("*.json"))]
    candidates += [(p, "decision_brief", "decision brief") for p in sorted((run_dir / "audit").glob("decision-brief-*.md"))]
    candidates += [(p, "decision_host_brief", "decision host brief") for p in sorted((run_dir / "audit").glob("decision-host-brief-*.md"))]
    candidates += [(p, "risk_discovery_reply", "risk discovery reply") for p in sorted((run_dir / "audit").glob("risk-reply-*.md"))]
    candidates += [(p, "decision_reply", "decision reply") for p in sorted((run_dir / "audit" / "decision-replies").glob("*.md"))]
    candidates += [(p, _evidence_kind(p), "execution evidence") for p in sorted((run_dir / "evidence").glob("*")) if p.is_file()]
    candidates += [(p, "judge_verdict", "judge verdict") for p in sorted((run_dir / "judge").glob("*")) if p.is_file()]

    for path, kind, summary in candidates:
        if not path.exists():
            continue
        entry = af._entry_for_path(
            repo, run_id, path, kind=kind, producer="lto.artifacts.synthesize",
            state=state, summary=summary, source="synthesized",
            task_id=task_by_artifact.get(_safe_rel(repo, path)),
        )
        af._upsert(manifest, entry)
    return manifest


def _task_map_by_artifact(state: dict) -> dict[str, str]:
    mapping: dict[str, str] = {}
    for task in state.get("tasks", []):
        task_id = task.get("id")
        if not task_id:
            continue
        for ev in task.get("evidence", []):
            for key in ("stdout_artifact", "stderr_artifact"):
                if ev.get(key):
                    mapping[ev[key]] = task_id
    return mapping


def _safe_rel(repo: Path, path: Path) -> str:
    try:
        return path.resolve().relative_to(repo.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def _evidence_kind(path: Path) -> str:
    name = path.name.lower()
    if "stderr" in name:
        return "evidence_stderr"
    if "stdout" in name:
        return "evidence_stdout"
    return "other"
