# LTO control-loop harness spec

> **STATUS: Phase 1 passive logging plus O2 event wiring implemented.** The Rust sensor layer — append-only `events.jsonl` (known event registry enforced on write) + derived `telemetry.json` — is implemented in Rust (`src/events.rs`, `src/event_emit.rs`, `src/telemetry.rs`). O2 adds caller-side events for runner results, audit dispatch/findings, gates, budget, sandbox refusals, and judge/decision outcomes. Phase 0/0.5 remains this reviewed specification. 现状合同（signals/metrics/event log/telemetry）见 [events-telemetry-contract.md](events-telemetry-contract.md)；Issue/Claim/Barrier 等 typed workspace 目标是 future 设计，见 [control-loop-roadmap.md](control-loop-roadmap.md)，不得当现状引用。

> LTO is not an agent UI, not a PM replacement, and not a marketplace of workflows. LTO is a control harness for long AI-agent work: observe state, detect drift, limit unsafe actions, improve flow, reduce waste, and preserve evidence for future tuning.

## 1. Purpose

Long agent tasks fail through unstable feedback:

- goal drift after many turns;
- self-verification bias;
- agentic laziness and premature completion;
- unbounded fan-out or audit loops;
- hidden cost/time burn;
- stale memory and source poisoning;
- weak evidence trails that prevent future tuning.

LTO should strengthen the control loop around the host agent without taking over planning.

Primary outcomes:

1. **Work paving**: make the next valuable work visible: critical path, blockers, WIP, pending gates, and highest-risk unknowns.
2. **Performance**: reduce wall time by exposing bottlenecks, safe parallelism, retries, and stale waits.
3. **Efficiency**: reduce duplicate agent work and low-yield review loops.
4. **Cost control**: track runner calls, elapsed time, budget burn, and waste.
5. **Quality**: make findings, issues, decisions, and verification evidence traceable.
6. **Future tuning**: preserve structured run logs and telemetry so later evals can learn from real runs without turning memory into truth.

## 2. Control-theory mapping

| Control concept | LTO construct | Notes |
|---|---|---|
| Setpoint | `goal`, `why`, `done_when`, `delivery_contract`, phase exit criteria | Human/host defines target; `/goal`-style work uses target/constraint/instrument/forced-entropy fields. |
| Plant | Real work being done in repo, docs, tests, deployment | Messy external system. |
| Sensors | `state.json`, `artifacts.json`, runner stdout/stderr, tests, git state, issue/finding logs | Sensors can be noisy. |
| Observer / estimator | `check`, `judge`, `audit`, privacy scan, schema parsers | Estimates truth; never truth itself. |
| Advisory brief (host-decided) | `next`, `recap`, `autopilot --supervised` | Surfaces signals and candidate facts; host decides. The decision engine exists in Rust internals, but historical `autopilot --decide` is not exposed by the current CLI. |
| Actuators | `runner`, `run parallel`, `run pipeline`, delegate runners, worktree exec | Must be bounded. |
| Actuator limiter | `PermissionPolicy`, worktree sandbox, budget, timeout, concurrency, human approval | Prevents runaway action. |
| Supervisor | Human + host agent | Final authority. |
| Final stabilization | `closeout`, changelog, handoff, validation evidence | Locks what happened. |

Invariant:

```text
LTO may observe, summarize, constrain, and propose.
LTO must not silently decide, route, promote, deploy, or erase.
```

## 3. Feedback loops

### 3.1 Stability loop

Goal: reduce drift and premature completion.

Signals:

- goal/done_when mismatch;
- open high/critical issues;
- unverified findings;
- stale blockers;
- missing test evidence;
- dirty worktree;
- phase gate violations.

Control actions:

- `next` flags drift and missing evidence;
- `judge` blocks closeout;
- `audit` only on high-risk / unresolved uncertainty;
- human gate for semantic ambiguity.

### 3.2 Flow/performance loop

Goal: improve wall-clock throughput without losing control.

Signals:

- task cycle time;
- blocked age;
- WIP count;
- dependency chain length;
- runner elapsed seconds;
- retry count;
- queue/wait time;
- parallelizable candidates;
- fan-out barrier readiness.

Control actions:

- split oversized tasks;
- freeze scope when WIP exceeds limit;
- propose safe fan-out only for independent tasks;
- surface critical path;
- recommend resolving oldest/highest-severity blocker first.

### 3.3 Cost loop

Goal: reduce token/model/runtime waste.

Signals:

- runner invocation count;
- elapsed seconds by runner;
- timeout/rate-limit count;
- duplicate findings;
- low accepted-finding ratio;
- audit rounds;
- estimated token/cost when available.

Control actions:

- budget hard stop;
- max audit rounds;
- max fan-out width;
- reuse frozen evidence;
- prefer static validation before runtime eval;
- mark low-yield runner/profile as observation, not automatic ban.

### 3.4 Quality loop

Goal: reduce rework and false confidence.

Signals:

- accepted/rejected finding ratio;
- issue reopen count;
- parse/schema success;
- test rerun pass/fail;
- judge reversal;
- false positive evidence;
- missed known blocker in eval cases.

Control actions:

- require structured outputs where possible;
- convert findings to issue lifecycle;
- require verification evidence before resolving;
- keep judged metrics distinct from deterministic metrics.

### 3.5 Learning loop

Goal: tune harness based on real runs without memory poisoning.

Pipeline:

```text
run telemetry -> observation -> hypothesis -> eval case -> promoted heuristic or rejected note
```

Rules:

- observations are not facts;
- external articles remain claims until tested;
- worker performance is advisory only;
- no auto-routing based on historical stats;
- promotion requires evidence and human approval.

## 4. Signals and metrics

已迁至 [events-telemetry-contract.md](events-telemetry-contract.md)。

## 5. Run log and telemetry design

已迁至 [events-telemetry-contract.md](events-telemetry-contract.md)。

## 6. Typed workspace objects

已迁至 [control-loop-roadmap.md](control-loop-roadmap.md)（design/future，非现状）。

## 7. Actuator limits

Every actuator must be bounded.

| Actuator | Required limits |
|---|---|
| `runner` | timeout, cwd, touched files, permission snapshot |
| `audit --auto-dispatch` | runner health, timeout, max auditors, no same-runtime self-audit |
| `run parallel` / `run pipeline` | concurrency, shell command list, rc capture |
| `autopilot --auto-exec` | worktree sandbox, safe command allowlist, no push/deploy/delete |
| future `eval-run` | plan-only default, execute explicit, budget, max concurrency, frozen evidence |
| future fan-out | barrier object, max workers, synthesis requirement, contradiction report |

## 8. Work paving briefs

`next` should evolve from task facts toward control facts:

```text
Current State
- phase: implementation
- WIP: 3 (limit 2) -> over limit
- critical path: T2 -> T5 -> closeout
- oldest blocker: T3, 7h
- open high issues: ISSUE-002
- budget: 12/20 runner calls used
- duplicate audit findings last round: 60%

Candidate Actions
1. Resolve ISSUE-002 before new work
2. Split T5; it blocks closeout and touches 8 files
3. Stop fan-out; last round low yield and high duplication
```

No automatic route. Host decides.

## 9. Anti-patterns

- UI-first Cockpit clone.
- PM coordinator daemon.
- Automatic worker selection from historical stats.
- Global KB treated as fact.
- Loop until issues empty.
- Adding metrics without actionability.
- Letting telemetry include raw private output.
- Optimizing for speed while hiding quality regressions.
- Treating judged metrics as deterministic truth.
- Creating issue noise without lifecycle/closeout gates.

## 10. Implementation plan

已迁至 [control-loop-roadmap.md](control-loop-roadmap.md)（design/future，非现状）。

## 11. Review questions

已迁至 [control-loop-roadmap.md](control-loop-roadmap.md)。

## 12. Non-goals

- No UI.
- No central server.
- No PM coordinator.
- No auto-routing.
- No global worker pool allocator.
- No automatic promotion.
- No raw transcript telemetry export.
- No long autonomous loop.
