# Internal SOP: Feature Development Lifecycle (desensitized)

> Source type: internal operational distillation  
> Origin: mined from the host user's development session logs across multiple projects.
> Desensitized — this note keeps the recurring patterns only; it contains no project
> names, no conversation excerpts, and no filesystem paths.  
> Status: experimental — claims unverified at scale

## The six-stage chain

Mid-task playbooks (review / debug / migration / claim-verify / research) assume the
task is already shaped. The recurring real chain from "an idea" to "a versioned
release" is longer:

```
[specify]    concept -> spec v1 -> heterogeneous co-design review -> spec v2
[dispatch]   sub-agents implement in an isolated worktree; host plans and audits;
             no questions back to the human after dispatch
[impl-audit] heterogeneous adversarial audit of the implementation;
             findings are union-merged — no voting, not one dropped
[converge]   per-blocker fix loop; exit condition is the test-pin:
             audit-raised invariants must land as regression tests
[acceptance] six gates checked simultaneously (scripts green, artifacts read,
             audit converged, docs synced, experience captured, observability present)
[release]    changelog + docs alignment + experience capture + privacy self-check
             + human-confirmed push
```

The host agent may enter, skip, or exit at any stage. This is a scheduling prior,
not a state machine.

## Hidden rules (recurring corrections distilled from the logs)

1. **Models are not pinned.** Designs that hard-code a model or runner name get
   rejected. Profiles declare capability and family; the host picks the runner.
2. **Verifier > Planner.** Physical verification beats plan discussion. Exit code 0
   alone never counts; artifacts must be read first-hand.
3. **No questions after dispatch.** Resolve clarifications before dispatching;
   give the full stop conditions in the prompt.
4. **Capture is a delivery action.** Recording lessons and decisions ranks with
   the push itself; it is not optional.
5. **Live lookup beats model memory.** Claims about current APIs / versions /
   prices must be checked against live sources.
6. **Commits are checkpoints.** Commit at every convergence point so drift can be
   rolled back.

## Observability: a cross-cutting pattern

Across all mined projects the same triad recurs:

1. **Structured logs are the source of truth** (machine-parseable, append-only);
2. **doctor / healthcheck is the entry point** (one command to see health);
3. **failure-query commands** answer "what failed recently" without log spelunking.

Observability was historically backfilled after "feature complete" — but feature
complete is not acceptance. The SOP therefore makes observability acceptance gate
number six: new feature modules must ship the triad; small fixes may be exempted
with an explicitly recorded reason.

## Direction disagreements

Findings-union loops never converge on taste or direction disputes. Those need a
separate route: classify the disagreement (evidence-decidable vs taste/direction),
decide evidence-decidable disputes by dispatching heterogeneous verification, and
escalate taste/direction disputes to a human by default. A needs_human signal from
any auditor escalates immediately and cannot be outvoted.
