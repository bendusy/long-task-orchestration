"""lto phase — 查看 / 推进 run 的 current_phase。

pi 实测反馈（2026-06-10）：`recap` 始终显示 `phase: intake`，没有 phase-set
命令，phase 推进只能手动改 state.json。核实属实——`check --to` 只做 phase
evidence *报告*（且只认 implementation/closed），从不真正改 current_phase；
在此命令之前，LTO 没有任何 CLI 能推进 run 的阶段。

`lto phase --set <phase>` 是轻量推进：用 transition_phase 记录转换历史 + 同步
run-state.md。它**不带** evidence 闸门——研究/探索型 run 不需要 implementation
/closed 的重证据校验。需要带闸门的正式收尾仍走 `check --to` / `closeout`。
"""

from __future__ import annotations

import argparse
from pathlib import Path

from .. import state as st
from .. import git_state as gs
from .. import safe_emit


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    current = state.get("current_phase", "intake")

    # No --set → just report current phase + transition history (read-only).
    if not args.set_phase:
        print(f"current phase: {current}")
        transitions = state.get("phase_transitions", [])
        if transitions:
            print("transitions:")
            for t in transitions[-5:]:
                print(f"  {t.get('from')} → {t.get('to')} @ {t.get('at', '?')[:19]}")
        print(f"valid phases: {', '.join(_ordered_phases())}")
        return 0

    to_phase = args.set_phase
    if to_phase not in st.VALID_PHASES:
        raise SystemExit(
            f"invalid phase: {to_phase!r} (valid: {', '.join(_ordered_phases())})"
        )
    if to_phase == current:
        print(f"already in phase '{current}' — no change")
        return 0

    head = gs.git_head(repo) if gs.is_git_repo(repo) else "unknown"
    st.transition_phase(state, to_phase, head)
    st.save_state(state_path, state)
    st.sync_run_state_md(repo / ".lto" / run_id / "run-state.md", state)

    safe_emit(
        repo, run_id, type="phase.changed", actor_kind="host",
        phase=to_phase, object_id=run_id, object_type="run",
        summary=f"{current} → {to_phase}",
    )
    print(f"phase: {current} → {to_phase}")
    # Nudge toward the gated path for formal transitions, so this lightweight
    # command doesn't quietly become the way people skip evidence checks.
    if to_phase in ("implementation", "closed"):
        print(
            "  note: `lto check --to "
            f"{to_phase}` reports the phase-evidence checklist; "
            "`closeout` is the gated way to finish."
        )
    return 0


def _ordered_phases() -> list[str]:
    """VALID_PHASES is a set; present it in the natural lifecycle order."""
    order = ["intake", "spec", "audit", "implementation", "deploy", "observe", "closed"]
    known = [p for p in order if p in st.VALID_PHASES]
    # append any phases not in our ordering hint (forward-compat)
    extra = sorted(st.VALID_PHASES - set(order))
    return known + extra


def add_parser(subparsers) -> None:
    p = subparsers.add_parser(
        "phase",
        help="show or advance the run's current_phase (lightweight, no evidence gate)",
    )
    p.add_argument("--run-id")
    p.add_argument(
        "--set",
        dest="set_phase",
        metavar="PHASE",
        help="advance to this phase: intake|spec|audit|implementation|deploy|observe|closed",
    )
    p.set_defaults(func=run)
