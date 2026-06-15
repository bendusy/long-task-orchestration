# LTO Run State Template

> 用途：每个长任务一份，路径建议 `.lto/<run-id>/run-state.md`。它是 /compact、resume、后台派工回收后的真源，不是事后总结。

## Identity

- run_id:
- feature / goal:
- started_at:
- host_runtime:
- repo:
- initial_user_request:
- current_phase: intake | spec | audit | implementation | deploy | observe | closed
- current_git_head:
- current_branch:

## Host Preconditions

- sandbox / approval:
- network:
- tmux/session:
- memory-flow / MCP:
- production access:
- child runner write roots:

## Active Delegations

| id | purpose | runner | command | pid/window | timeout | reply | exit | status |
|---|---|---|---|---|---:|---|---:|---|
| d1 |  |  |  |  |  |  |  | planned |

Status values: planned, running, returned, failed, superseded, abandoned.

## Phase Ledger

| phase | entry condition | exit evidence | user decision | status |
|---|---|---|---|---|
| intake |  |  |  | open |
| spec |  |  |  | pending |
| audit |  |  |  | pending |
| implementation |  |  |  | pending |
| deploy |  |  |  | pending |
| observe |  |  |  | pending |

## Decision Slugs

| decision | slug / ADR | commit | reason it matters |
|---|---|---|---|
|  |  |  |  |

## Evidence Snapshot

Keep this short and evidence-first. Update after every resume or phase gate.

- architecture_alignment:
- first_principles:
- simplification_dedupe:
- value_measurement:
- documentation_alignment:
- historical_cleanup:
- clean_worktree:
- rebuild_package:
- code layer:
- runtime layer:
- persistence layer:

## Next Action

- blocked_by:
- next_command_or_question:
- owner:
- due / wakeup:
