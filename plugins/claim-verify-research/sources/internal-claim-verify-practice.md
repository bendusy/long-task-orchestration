# Source note: Internal claim-verify practice

URL: internal:lto-release/long-task-orchestration/audit-ledger-observations-2026
Captured: 2026-06-10

## Background

During LTO audit runs (2026-05 to 2026-06), three recurring failure modes appeared:

1. **Authority laundering** — a second LLM agreeing with the first was counted as corroborating evidence. It is not. Evidence must be reproducible from a path:line or a deterministic command, not from another model's agreement.
2. **Confirm-first bias** — auditors asked "does this claim hold?" tend to find supporting evidence first and stop. Refute-first framing ("try to refute this; default to refuted if uncertain") lowers false-positive verdicts.
3. **Single-angle search blindness** — research tasks routed to one search backend missed claims that a second angle (different entity framing, different time window, or different source type) would have surfaced.

## LTO adaptation

This plugin captures the claim-verify pattern as a data-only playbook:

- **Decomposer** (Claude): break vague external claims into falsifiable hypotheses with measurable metrics.
- **Evidence refuter** (Codex): read repo files / command outputs; try to refute each hypothesis with direct evidence; report explicit confidence.
- **Completeness critic** (Pi): after primary verification, enumerate any claim not yet verified or refuted; flag gaps.

## Boundary

These patterns are hypotheses until evaluated. The plugin does not grant write permission, does not change LTO convergence logic, and does not auto-promote verdicts to core state.

All evidence used to upgrade a claim must be frozen (path:line or deterministic command output captured and redacted). "Another model also agrees" is explicitly not evidence.
