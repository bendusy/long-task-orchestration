# Internal SOP: Layered Enterprise Audit Gate (desensitized)

> Source type: internal operational distillation
> Origin: host-provided audit model plus recurring review failures observed in local
> development sessions.
> Desensitized: contains no project names, conversation excerpts, or filesystem paths.
> Status: experimental; use as a scheduling prior, not an external-company claim.

## Problem

Heterogeneous adversarial review catches more than same-family self-review, but a
single generic "review the implementation" prompt still misses whole classes of
failure. The recurring gaps are not only code bugs: missing requirements, unstable
architecture boundaries, schema/contract drift, absent rollback, weak observability,
and acceptance based on remembered success.

## Layer Model

High-risk work should be audited across ten layers:

1. requirements
2. architecture
3. data-model
4. interface-contract
5. implementation
6. testing
7. operations-observability
8. security
9. migration-rollback
10. acceptance

This is a coverage model, not a mandatory committee. The host agent scopes the run:
high-risk changes default to all layers; small fixes may exempt irrelevant layers
with recorded reasons.

## Redline Rules

Findings become blocking redlines when they expose:

- requirement contradiction or missing acceptance criteria;
- undefined ownership or lifecycle boundary for shared state;
- unsafe data migration or unaddressed data loss;
- interface contract drift without contract tests or compatibility plan;
- implementation behavior that contradicts the spec;
- changed behavior without a regression/test-pin;
- new feature module without structured logs, healthcheck, or failure query;
- auth, tenant isolation, secret, permission, or injection issue;
- irreversible migration without dry-run, backup, rollout, or rollback evidence;
- acceptance that relies only on exit code or memory instead of artifact read-through.

## Operating Discipline

Dispatch layer auditors as read-only, cross-family runners. Merge findings by union,
not by vote. The host verifies each cited blocker first-hand, fixes or falsifies it,
then reruns affected layers until HIGH/CRITICAL redlines reach zero or the human
explicitly overrides with residual risk recorded.

## Boundary

The path does not replace LTO's host-planner boundary. It only gives the host a
layered checklist, redline vocabulary, and profile prompt for high-risk audits.
