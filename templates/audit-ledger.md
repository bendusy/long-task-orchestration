# LTO Audit Ledger Template

> 用途：每轮异构审计回来后立刻更新，路径建议 `.lto/<run-id>/audit-ledger.md`。它记录 blocker 是否单调下降，不记录泛泛建议。

## Round Summary

| round | artifact | auditors | coverage | high | critical | minor | trend | status |
|---|---|---|---|---:|---:|---:|---|---|
| R1 |  |  |  |  |  |  | start | open |

Trend values: start, down, flat, rebound, closed.

## Blocker Register

| id | round_seen | severity | source | claim | evidence_to_check | disposition | fix_or_rebuttal | status |
|---|---|---|---|---|---|---|---|---|
| B001 | R1 | high |  |  |  | needs_verification |  | open |

Disposition values: accepted, rejected, needs_verification, deferred_minor.
Status values: open, fixed, rebutted, backlog, superseded.

## Verification Notes

Each adopted or rejected blocker needs first-hand evidence.

### B001

- claim:
- first-hand evidence:
- result: accepted | rejected | deferred
- patch / follow-up:

## Rebound Handling

- Did HIGH+CRITICAL increase this round?
- If yes, stop normal iteration and run debug/re-scope:
- If flat for two rounds, challenge the requirement or audit standard:

## Closure Gate

- latest HIGH+CRITICAL count:
- remaining minor items:
- user decision required:
- close / continue verdict:
