# LTO project instructions

## Project identity

LTO is a **control harness for long AI-agent work**.

It is not:

- an agent UI;
- a PM/coordinator replacement;
- a workflow marketplace;
- an auto-routing system;
- a memory system that treats old notes as truth.

The host agent remains the planner. LTO provides state, evidence, feedback, gates, actuator limits, and run logs.

## Core control-loop principles

### 1. Host remains controller-in-chief

LTO may observe, summarize, constrain, and propose. It must not silently decide, route, promote, deploy, push, or erase.

`next` and `autopilot --supervised` produce control briefs. The host/human decides.

### 2. Observability before control

Do not automate a behavior until LTO can measure it.

Preferred order:

```text
observe -> log -> derive signal -> propose action -> gate -> execute -> record outcome
```

### 3. Negative feedback first

LTO should first correct drift, surface blockers, cap cost, and stop unsafe actions.

Avoid positive-feedback loops such as:

- “runner was fast once, route more work to it”;
- “article sounds right, promote it to core”;
- “audit found issues, keep spawning audits until empty”.

### 4. Every actuator is bounded

Any command that executes work must have explicit limits:

- timeout;
- cwd / worktree boundary;
- permission snapshot;
- max concurrency;
- max rounds;
- max budget where applicable;
- human gate for irreversible actions.

### 5. Sensors are fallible

`judge`, `audit`, schema parsers, privacy scans, and external articles are evidence, not truth.

Separate:

- deterministic metrics;
- oracle-assisted metrics;
- judged metrics.

Never treat judged metrics as ground truth.

### 6. Work paving beats UI

Do not build a Cockpit UI. Strengthen CLI/file primitives that help the host pave work:

- critical path;
- WIP count;
- blocker age;
- issue lifecycle;
- budget burn;
- fan-out barriers;
- worker observations;
- closeout gates.

### 7. Run logs are tuning fuel

LTO must preserve structured run telemetry for future tuning:

- append-only `events.jsonl` style run log;
- derived `telemetry.json` snapshots;
- worker observations;
- finding/issue/decision provenance;
- budget/time/quality signals.

Logs must pass ingress redaction before write. They reference artifacts, not inline raw transcripts, secrets, private source contents, or absolute private paths. `telemetry.json` persists derived signals only; route-like recommendations belong in ephemeral `next` briefs, not persistent telemetry.

### 8. External viewpoints enter as hypotheses

Articles, tweets, framework claims, and model-specific advice enter via:

```text
source note -> unverified claim -> falsifiable hypothesis -> eval evidence -> promote/reject
```

No direct core changes from article authority.

### 9. Typed shared workspace, not PM platform

Absorb Cockpit-like ideas as typed workspace primitives, not as a coordinator daemon:

- Task;
- AgentJob;
- AgentResult;
- Artifact;
- Finding;
- Issue;
- ResearchNote;
- Claim;
- Hypothesis;
- Decision;
- WorkerObservation;
- Barrier.

Workers may produce proposed findings/reports/research artifacts. Host/LTO gates decide what becomes accepted issue/decision/state.

### 10. Privacy is part of control

Telemetry and logs must be redaction-aware. Never commit `.lto/`, transcripts, feedback bundles, secrets, or private local paths.

Run privacy checks before push when changing logs, telemetry, artifacts, plugins, or delegation code.

## Current design references

Read these before changing related areas:

- `references/control-loop-harness.md` — control-loop harness spec and run telemetry design.
- `references/plugin-boundary.md` — data-only plugin boundary.
- `references/plugin-real-eval-runner.md` — real eval as sub-LTO-run compiler.
- `references/privacy-self-check.md` — privacy scan and confirmed cleanup.
- `references/workflow-playbook.md` — host-agent playbook philosophy.

## Implementation posture

When implementing new LTO capabilities:

1. Prefer passive logging before behavior changes.
2. Add schema/tests before automation.
3. Keep commands resumable and auditable.
4. Do not add UI/server/global daemon unless explicitly approved later.
5. Do not let historical telemetry auto-route workers; surface it as advisory evidence.
6. Preserve backwards compatibility for existing `.lto/<run-id>/state.json` runs.
