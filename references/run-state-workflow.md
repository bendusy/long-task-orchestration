# LTO run-state workflow

Use `scripts/lto_run.py` when a long task needs durable repo-local state instead
of chat-memory coordination.

## Start

Inside `agent-skills`, run from the repo root:

```bash
# minimal: run-state.md only (default)
python3 skills/long-task-orchestration/scripts/lto_run.py start \
  --goal "short task goal" \
  --host codex \
  --request "original user request"

# audit: run-state + preflight + audit-ledger
python3 skills/long-task-orchestration/scripts/lto_run.py start \
  --goal "spec audit task" \
  --host codex \
  --profile audit

# deploy: full all three (deployment-safe)
python3 skills/long-task-orchestration/scripts/lto_run.py start \
  --goal "deploy workflow" \
  --host codex \
  --profile deploy
```

When the target repo is not `agent-skills`, call this script by absolute path:

```bash
python3 /Users/ben/Projects/agent-skills/skills/long-task-orchestration/scripts/lto_run.py \
  --repo /path/to/target/repo \
  start --goal "short task goal" --host codex
```

This creates `.lto/<run-id>/` with artifacts per profile:
- `minimal`: `run-state.md` only
- `audit`: `run-state.md` + `preflight.md` + `audit-ledger.md`
- `deploy`: all three (same as audit)

It also writes `.lto/current`, so later commands can omit `--run-id`.

## Check

```bash
python3 skills/long-task-orchestration/scripts/lto_run.py check
```

Use strict mode before claiming the run is current:

```bash
python3 skills/long-task-orchestration/scripts/lto_run.py check --strict
```

Strict mode fails when:

- required state fields are blank
- required artifacts are missing
- the target is not a git worktree
- recorded git HEAD differs from the current repo HEAD outside `.lto`
- the worktree has uncommitted changes outside `.lto`
- a closed run has no `handoff.md`

When non-strict mode sees git HEAD drift or dirty files outside `.lto`, it
prints a warning instead of failing. `.lto` changes are ignored by drift checks
because the state files themselves may be committed after the code commit.

## Closeout

```bash
python3 skills/long-task-orchestration/scripts/lto_run.py closeout \
  --summary "what changed and how it was verified" \
  --next-action "none"
```

Closeout updates `run-state.md`, marks `current_phase: closed`, refreshes git
HEAD/branch, appends a closeout note, and writes `.lto/<run-id>/handoff.md`.
It refuses to run when required artifacts are missing, when `preflight.md`
has no `preflight_verdict`, when `audit-ledger.md` has no latest
HIGH/CRITICAL count or close/continue verdict, or when there are uncommitted
changes outside `.lto` unless `--allow-dirty` is explicitly used. Running
closeout on an already closed run requires `--force`; force rewrites the
existing closeout section instead of appending duplicate entries.

## Self-Test

The script has offline smoke coverage:

```bash
python3 skills/long-task-orchestration/scripts/lto_run.py self-test
```

Run it after editing the script or templates.
