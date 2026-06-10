# Observability Module — triad checklist (host self-check)

Cross-cutting pattern: every healthy project converges on the same three
observability pieces. New feature modules must ship all three; this checklist
backs acceptance gate 6 (see `prompts/acceptance-gate.md`). Host self-check —
no JSON output required.

## 1. Structured log schema

- Events are written as machine-parseable structured records (e.g. JSONL),
  append-only.
- The schema is documented: field names, types, and what each event means.
- A human never has to regex free-form prose to reconstruct what happened.

## 2. doctor / healthcheck entry point

- One command shows the module's health: dependencies present, config valid,
  state files readable.
- It is the documented first command for troubleshooting.
- It exits non-zero on a detected problem and says which check failed.

## 3. Failure-query commands

- A query entry point answers "what failed recently" without log spelunking —
  fails / recent / stats-class commands over the structured log.
- Output is specific enough to start a diagnosis (which step, when, what error).

## Acceptance shape

The triad passes acceptance when:

- the logs can be parsed by a machine (demonstrate: parse one real log file);
- doctor is a single command (demonstrate: run it, observe its verdict);
- the failure query can answer "what failed recently" (demonstrate: ask it after
  a real or injected failure).

Demonstrations count; descriptions do not. A module whose observability exists
only in its README has not shipped the triad.

## Exemption rule

Small fixes (no new module, no new external behavior) may skip the triad — but
the exemption and its reason must be recorded explicitly in the run state.
"Observability later" without a recorded reason is the anti-pattern this
checklist exists to stop.
