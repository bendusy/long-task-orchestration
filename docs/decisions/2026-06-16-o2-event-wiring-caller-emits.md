# O2 event wiring keeps scheduler run-agnostic

- status: accepted
- date: 2026-06-16
- lto_run: 20260616-065656-o2-event-wiring-for-silent-subsystems-22fec2ac
- slug: o2-event-wiring-caller-emits

## Context

O2 observability must wire runner, audit, gate, budget, sandbox, judge, and decision moments into events.jsonl. The existing events writer rejects unknown event types on write, and Scheduler::submit has no run_id contract because it is shared by audit, run parallel/pipeline, plugin eval-run, and judge dispatch.

## Decision

Expand the production event type whitelist before emitting O2 events by replacing the Phase 1-only list with KNOWN_EVENT_TYPES. Keep Scheduler run-agnostic and event-free; caller layers that already know repo/run_id emit runner.started before submit, runner.finished/retry/healthcheck from AgentResult after submit, and submission-failed runner.finished records on SchedulerError.

## Consequences

O2 event writes fail fast on typo types but allow the new O2 families. Scheduler remains a generic execution primitive with no .lto protocol coupling. Event ownership is explicit at command/plugin/audit call boundaries, and submit failures no longer leave orphaned runner.started events.
