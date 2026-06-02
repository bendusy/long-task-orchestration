# LTO Preflight Template

> 用途：异构审计、后台长任务、部署前先填。路径建议 `.lto/<run-id>/preflight.md`。宣称范围必须等于实测范围。

## Scope

- run_id:
- task:
- host_runtime:
- requested_auditors:
- planned_timeout:

## Host Profile

| item | value | evidence | verdict |
|---|---|---|---|
| interactive or exec |  |  |  |
| sandbox |  |  |  |
| approval policy |  |  |  |
| network |  |  |  |
| tmux/session |  |  |  |
| child write roots |  |  |  |
| MCP/memory visible in child |  |  |  |

Verdict values: pass, fail, degraded, not_applicable.

## Runner Health

Use `skills/agent-delegate/scripts/runners/healthcheck.sh --json` when agent-delegate is available.

| runner | exit | elapsed | bytes | verdict | action |
|---|---:|---:|---:|---|---|
| codex |  |  |  |  |  |
| pi |  |  |  |  |  |
| agy |  |  |  |  |  |
| claude |  |  |  |  |  |

Runner verdict rules:
- exit=0 + bytes>0 -> OK
- exit=0 + bytes=0 -> EMPTY
- exit=124 -> TIMEOUT
- non-zero -> ERROR

## Degradation Decision

- actual_auditors:
- omitted_auditors_and_reason:
- timeout_adjustments:
- safety_notes:
- claim_to_user:

## Go / No-Go

- preflight_verdict:
- required_user_decision:
- next_step:
