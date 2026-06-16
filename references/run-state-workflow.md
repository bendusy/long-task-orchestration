# LTO run-state workflow

Rust-only command reference for `.lto/<run-id>/` state workflows. The former
Python fallback command reference was retired in v0.5.0; command truth now lives
in `COMMANDS.md`, `src/cli.rs`, and `references/rust-migration-release.md`.

## Start

Inside the repository, run from the root:

```bash
# minimal: state.json + run-state.md (default)
lto start \
  --goal "short task goal" \
  --host codex \
  --request "original user request" \
  --why "why this run exists (for human recap after long gaps)" \
  --done-when "how you'll know it's finished (recap data source)"

# with audit ledger (only INITIALISES the ledger; run `audit` to fill+converge it)
lto start \
  --goal "spec audit task" \
  --host codex \
  --with-audit

# deploy profile: audit 超集，额外落 preflight 环境快照进 state.json
lto start \
  --goal "deploy task" \
  --host codex \
  --profile deploy

# opt-in: install LTO pre-commit gate into .git/hooks (skips if husky/pre-commit detected)
#   add --install-hooks ; NOT installed by default
```

`--why` / `--done-when` feed `recap`'s human-facing view. `--install-hooks` is
opt-in (default off). `--with-audit` only creates `audit-ledger.md`; the actual
adversarial audit + convergence runs via the `audit` command.

Before entering implementation or optimization, record four evidence lines in
`run-state.md` or task evidence:

- `architecture_alignment`: layer, module boundary, and existing pattern being reused.
- `first_principles`: real constraint, user value, or root cause that justifies the change.
- `simplification_dedupe`: what was deleted, merged, reused, or why new abstraction is necessary.
- `value_measurement`: baseline, metric, pass threshold, and post-change measurement command/result.

Optimization without measurement is only a hypothesis; it is not closeout evidence.

Before closeout, release, or long handoff, record four closure evidence lines:

- `documentation_alignment`: docs checked/updated so they match the final architecture and command surface.
- `historical_cleanup`: stale paths, legacy notes, obsolete runs, or compatibility leftovers removed, archived, or explicitly marked historical.
- `clean_worktree`: clean `git status --short` before packaging, or a named human-approved residual dirt list.
- `rebuild_package`: final rebuild/repackage command and result after the repo reached its final state.

Packaging before the last edit is not release evidence; rebuild from the final state.

Optional **budget caps** (all default unlimited → zero break for runs that omit
them): `--max-turns N` / `--max-tokens N` / `--deadline ISO8601`. See the Budget
section below for the graded-brake semantics.

When the target repo is not current directory, pass `--repo` before the command:

```bash
lto --repo /path/to/target/repo start --goal "short task goal" --host codex
```

After `bash scripts/install.sh`, the global wrapper is shorter:

```bash
lto --repo /path/to/target/repo start --goal "short task goal" --host codex
lto check --repo /path/to/target/repo
```

The wrapper is sentinel-managed and points at the current
`long-task-orchestration` checkout. If the repo moves, rerun `scripts/install.sh`.

This creates `.lto/<run-id>/` with:
- `state.json` — machine-readable state (source of truth)
- `run-state.md` — human-readable state
- `audit-ledger.md` — only when `--with-audit` is set

It also writes `.lto/current`, so later commands can omit `--run-id`.

**Git hook install is opt-in** (2026-06-03): pass `--install-hooks` to add the LTO
pre-commit gate into `.git/hooks/pre-commit`. It is **not** installed by default,
and is **skipped with a warning** when husky / pre-commit framework / an existing
custom pre-commit hook is detected, to avoid clobbering your setup.

## Task-Add

After `start`, add the tasks the run will work on. A task is the unit that
`runner` / `next` / `audit` operate on — `runner` does NOT auto-create them.

```bash
lto --repo . task add \
  --task-id T1 \
  --title "给 login 加判空校验" \
  --command "pytest tests/test_auth.py -x"   # optional: planned command (runner/autopilot use it)
```

`--task-id` must be unique (duplicate is rejected). `--phase` defaults to the
current phase. Then run it via `runner --task-id T1 --command "..."`.

## Resume

Recover from a previous session:

```bash
lto resume
```

Prints a context capsule (phase, tasks, last failure, next action).
Validates git HEAD: forward drift with unrelated changes is OK, rewrite triggers
revalidation. Returns exit code 2 when tasks need revalidation.

For forward HEAD drift, `resume` compares changed commit paths against task
`touched_files`. Related changes mark done/in-progress tasks pending. If tasks
exist but no `touched_files` are recorded, it warns that file drift precision is
unavailable instead of guessing across the whole repo.

## Memory Projection (optional ANIMEM / memory-flow)

LTO core does **not** require ANIMEM, memory-flow, MCP, PostgreSQL, or any
private memory service. Local `.lto/<run-id>/state.json` and `artifacts.json`
remain the source of truth.

Use memory projection only when you want cross-runtime/cross-project discovery:

```bash
# Pure local, redacted JSON. No network, no ANIMEM required.
lto memory export \
  --run-id <run-id> --dry-run

# Try memory-flow/ANIMEM discovery, then always print local-first capsule.
# If no sink is configured, prints a warning and degrades to local .lto.
lto memory resume \
  --project agent-skills --run-id <run-id>

# Explicit publish only. Requires MEMORY_FLOW_URL + MEMORY_FLOW_TOKEN or flags.
lto memory publish \
  --run-id <run-id>
```

Projection privacy rules:

- `original_user_request` is hash-only; raw text is not projected.
- `goal` / `why` / `done_when` / `next_action` / artifact summaries are capped
  and redacted.
- `agent_runs`, `decision_escalate_points`, raw runner output, source file
  bodies, secrets, env values, and private document bodies are not projected.
- Dirty worktree details are `dirty_count` plus capped/redacted samples.

`lto memory resume` is read-only. It never overwrites `.lto/current`,
`state.json`, or tasks. If remote hashes differ from local state, report drift;
local files win.

## Preflight

Probe environment health (stdout only, no file):

```bash
lto preflight
lto preflight --record  # also write to state.json
```

## Runner

Execute a single task and auto-record evidence:

```bash
lto runner \
  --task-id T1 \
  --kind test \
  --command "pytest tests/test_auth.py -x" \
  --touch src/auth.py \
  --note "验证登录修复"
```

On success: task.status=done, evidence recorded, gates.last_tested_head updated.
On failure: task.status=blocked, blocker recorded, state.last_failure set,
retry_count bumped (per command fingerprint).

Other flags: `--status-on-fail {blocked,in_progress}` (default blocked),
`--cwd`, `--timeout`, `--auto-commit` (opt-in commit of .lto state).

## Judge

Read-only review of runner output, outputs YAML verdict:

```bash
# Review entire phase
lto judge --phase implementation

# Review single high-risk task
lto judge --task-id T5

# Rerun recorded tests
lto judge --phase implementation --rerun-tests
```

Saves verdict to `.lto/<run-id>/judge/judge-<phase>-<ts>.yaml`.
Other flags: `--since <git-base>` (diff review base), `--runner <name>`
(auditor agent name, default codex), `--auto-commit` (opt-in commit of .lto state).
Updates `gates.last_reviewed_head`.

## Hook

Boundary gate checks for irreversible actions:

```bash
lto hook pre-commit
lto hook pre-deploy
lto hook pre-closeout

# Force override
lto hook pre-commit --force --reason "docs-only"
```

Environment variable `LTO_HOOK_MODE` controls pre-commit behavior:
- `off` — disabled
- `warn` (default) — warn only (except unresolved blocks)
- `block` — warn also blocks

## Check

```bash
lto check
lto check --strict
lto check --to implementation
lto check --to closed --strict
lto check --to implementation --json
```

Validates state.json integrity, git HEAD anchor, dirty worktree, handoff
completeness, and optional audit-ledger convergence.

When HEAD advanced normally, `check` uses the same task `touched_files`
commit-to-commit drift detector as `resume`. Default mode warns on related task
file changes; `--strict` returns rc 1. It does not mutate state. Dirty worktree
changes are still handled by the existing dirty warning/error and are not
intersected with `touched_files` in this pass.

`--to implementation|closed` adds a read-only phase-entry evidence report. It
does not transition state and does not approve the phase; the report always
includes `human_gate_required: true`.

Targets covered in this first version:

| Target | Required evidence under `--strict` | Advisory evidence |
|---|---|---|
| `implementation` | no unresolved gate blocks or open unverified risks; filled audit ledger is `CONVERGED` when present | task list present, phase direction |
| `closed` | no open tasks (`status` not in `done`/`skipped`); no unresolved gate blocks; risk points verified or closed; filled audit ledger is `CONVERGED` when present | artifact manifest, handoff if already closed, phase direction |

Default mode reports missing phase evidence but keeps rc 0 when the base
`check` passes. `--strict` upgrades missing required evidence to rc 1.
`--json` prints one JSON object to stdout and suppresses text/WARN output so
other host runtimes can parse it directly.

The four development evidence lines and four closure evidence lines are host
contracts today. Rust `check` enforces the machine-verifiable gates; record the
remaining host-judgment evidence in run-state/task evidence and let
`judge`/human review treat missing fields as closeout blockers.

## Closeout

```bash
lto closeout \
  --summary "what changed and how it was verified" \
  --next-action "none"
```

Closeout updates state.json (phase→closed), syncs run-state.md, writes
handoff.md, and renders its Artifacts section from `.lto/<run-id>/artifacts.json`.
Refuses when: ledger not converged, unresolved blocks exist,
uncommitted changes outside .lto, or run already closed (use `--force`).
Also refuses if a high-risk task has no/empty audit ledger, or if there are
unverified `risk_points` (use `--force` / `--allow-dirty` to override).

Add `--auto-commit` to commit `.lto` + CHANGELOG.md (opt-in, uses repo git
identity, default off).

## Parallel / Pipeline (shell command batching)

These batch-run **shell commands** (not agent fan-out — same names as
pi-dynamic-workflows but different semantics).

```bash
L="cargo run --quiet --"

# parallel: run many tasks' shell verify commands concurrently, record evidence
$L run parallel --phase implementation --concurrency 4 --command "pytest -x"

# pipeline: each task runs sequential stages ({task_id} placeholder), items concurrent
$L run pipeline --phase implementation --stages "ruff check {task_id}" "pytest -k {task_id}"
```

Each records evidence via the shared `exec.run_command` kernel. `--auto-commit`
opt-in. Real **agent fan-out** is `audit --auto-dispatch` / `--discover-risks`.
stdout/stderr artifacts are registered in `.lto/<run-id>/artifacts.json` using
repo-relative paths.

## Audit (adversarial heterogeneous review)

```bash
$L audit --auto-dispatch        # auto-dispatch heterogeneous auditors (≠ host family) + collect
$L audit --discover-risks       # spawn agent to find unregistered risk points (source=risk-agent)
$L audit                        # write brief + print dispatch instructions (manual)
$L audit --collect <reply-dir>  # collect replies → heterogeneity check + blocker count + converge
```

Auditors emit structured JSON findings (severity is a field, not a regex scan).
`--collect` rejects same-family auditors (use `--allow-same-family` to override).

## Next (fact router — zero LLM)

```bash
$L next            # print decision brief (escalate) or unambiguous cmd suggestion
$L next --exec     # execute unambiguous routes (closeout/judge/resume); escalate → print only
$L next --json     # facts + route as JSON
```

Analyzes state, gives the host LLM a rich decision brief (goal + blocked task
failure summaries). It does not choose a complete workflow or preset. Decisions
stay with the host. Empty phases never auto-advance.

## Autopilot (constrained harness)

```bash
$L autopilot --supervised               # brief + route, escalate to host (default)
$L autopilot --supervised --auto-exec    # auto-run safe/reversible task commands in worktree sandbox
$L autopilot --auto-exec --worker-runner tmux --target <session:window.pane>
                                        # dispatch one tmux worker per pending task
```

`--auto-exec` runs commands in an isolated git worktree (rm -rf only nuks the
worktree; env-isolated HOME/credentials). Dangerous ops (rm -rf / git push /
DROP / sudo / curl|sh / escape paths) are HELD for human confirm. Retry≥3 skips,
stall detection reverts to brief-only. Autopilot can run safe substeps and collect
decision evidence, but the host agent remains planner. With `--worker-runner tmux`,
autopilot uses the scheduler-backed tmux runner as a bounded worker carrier: each
pending task gets its own worker dispatch and must write a
`.lto/<run>/live/*.worker.json` completion contract. `state.tasks` changes only
from that contract `rc`, not from the worker saying it is done. `--autonomous` is implemented
as a **mechanical evidence gate + mechanical execution** — it never spawns a decision
agent and never reflects (LTO emits facts; the host reflects). It reads cross-run
mining to gate on accumulated real dispatch data (falls back to supervised when
insufficient), then mechanically runs safe substeps; escalate/dangerous/push/network
stay with the human. Mutually exclusive with `--decide`.

## Recap (human-facing review)

```bash
$L recap            # what you set out to do / why / how long / where you are / what's next
$L recap --artifacts  # same recap plus recent artifact paths
```

Unlike `resume` (feeds the AI: git head / task ids), `recap` is for **humans** —
plain-language answers after a long gap. Uses `state.json` + `--why`/`--done-when`.
Artifact paths are opt-in to keep the default human recap low-noise.

## Budget (run-level contract)

```bash
$L budget check                        # per-dimension used/limit/status
$L budget extend --max-tokens 2000000  # raise a cap (human action)
```

Run-level budget caps are an **optional contract** set at `start`
(`--max-turns` / `--max-tokens` / `--deadline`); all default unlimited, so runs
that omit them behave exactly as before. Enforcement is **graded**:

- **Soft warning** at `warn_ratio` (default 0.8): a `⚠️ budget: …` fact line
  appears in `next`'s Decision Brief and `recap`. Zero block — it is a fact, not
  a recommendation; matching it to your decision stays the host's job.
- **Hard brake** at 100%: `autopilot` runs a budget gate before every
  auto-advance. Any dimension over limit → fail-closed `NEEDS_CONFIRM`, no
  auto-exec. Unlock only by explicit `lto budget extend` or re-`start`.

`turns_used` counts **autopilot auto-advance calls only** — human manual ops
(`runner`/`audit`/`next`) never consume a turn; the contract constrains
automation, not the human. `budget extend` cannot shrink a cap below the
already-used amount (anti self-lock). Measurement lives in `budget.py`
(pure: token total + current time injected by the caller); autopilot executes
the brake — measurement and enforcement stay separated, like `next` (facts) vs
`autopilot` (action).

## Artifact Manifest

Every new run creates `.lto/<run-id>/artifacts.json`. It indexes run artifacts
with repo-relative paths: state/run-state, audit briefs/replies, decision briefs,
shell evidence, judge verdicts, decision records, handoff, and volatile
repo-level `CHANGELOG.md`.
`resume` prints recent artifacts for the next host agent. Old runs without a
manifest are synthesized best-effort in memory; closed runs are not dirtied by
read-only synthesis.

`decision_record` is the only additional run-outside artifact kind: paths must
match `docs/decisions/*.md`, and both `relative_path` and `run_relative_path`
store the full repo-relative path.

## Decision Records

```bash
python3 scripts/write_decision.py \
  --repo . \
  --run-id <id> \
  --title "why keep wrapper opt in" \
  --slug "keep-wrapper-opt-in" \
  --context "..." \
  --decision "..." \
  --consequences "..."
```

The helper writes `docs/decisions/YYYY-MM-DD-<slug>.md`, appends
`state.user_decisions`, and registers the ADR as `decision_record` in the
artifact manifest. It does not call memory-flow directly.

## Self-Test

```bash
lto self-test
```

Covers: start, resume, check, preflight, hook pre-commit, closeout, and
gate regression (non-converged ledger rejection).
