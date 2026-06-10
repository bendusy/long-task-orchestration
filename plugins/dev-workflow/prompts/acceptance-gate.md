# Acceptance Gate — six-gate checklist (host self-check)

Walk this checklist at the acceptance stage of the feature-dev path. All six gates
must hold **simultaneously**; any exemption must record an explicit reason in the
run state. This is a host self-check, not a runner dispatch — no JSON output is
required; record per-gate evidence in your own words.

## Gate 1 — scripts green

The project's own verification scripts (registry / lint / CI-class checks) all pass.
Run them now, in this session; do not rely on a remembered earlier pass.

## Gate 2 — artifacts read first-hand

Key deliverables were opened and read. Exit code 0 alone never counts as
verification: the artifact's actual content must have been inspected.

## Gate 3 — adversarial findings converged

The findings union from the implementation audit is fully processed: every item
adopted (and fixed) or rejected with a falsification note, with no remaining
blocker or high-severity finding open.

## Gate 4 — docs synced

README, changelog, and interface/reference docs match the code as shipped. If a
docs-sync loop ran, its drift register is closed.

## Gate 5 — experience captured

This task's pitfalls and decisions are recorded — in the memory system, a decision
record, or the handoff document. Any one of those locations satisfies the gate;
none of them satisfies it implicitly.

## Gate 6 — observability present

New feature modules ship the observability triad (see
`prompts/observability-module.md`): structured log schema, doctor/healthcheck
entry point, and failure-query commands. Small fixes may be exempted, but the
exemption and its reason must be recorded explicitly.

---

Done means: gates 1–6 all pass at the same time, and every exemption carries a
recorded reason. If any gate fails and cannot be reasonably exempted, return to
the converge stage instead of negotiating the gate down.
