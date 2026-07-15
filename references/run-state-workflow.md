# LTO run-state workflow

Rust-only command reference for `.lto/<run-id>/` state workflows. The former
Python fallback command reference was retired in v0.5.0; command truth now lives
in `COMMANDS.md`, `src/cli.rs`, and `references/rust-migration-release.md`.

## Start

Inside the repository, run from the root:

```bash
# minimal: state.json + run-state.md
lto start \
  --goal "short task goal" \
  --host codex \
  --why "why this run exists (for human recap after long gaps)" \
  --done-when "how you'll know it's finished (recap data source)"

# /goal 型长交付：delivery contract 四件套（target/constraint/instrument/entropy-check）
lto start \
  --goal "提升检索召回" \
  --done-when "hidden eval recall 达标且审计收敛" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h" \
  --instrument "hidden-eval::python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"
```

参数真源是 `lto start --help`（`src/cli.rs` `Start`）：`--run-id/--goal/--why/--done-when/
--host/--target/--constraint/--instrument/--entropy-check/--force`。

- `--goal` 与 `--done-when` 必须提供非空值；缺失时在任何 `.lto` 写入前硬失败。
- `--why` 与 `--host` 是 advisory；缺失会告警，缺失 host 按 `unknown` 记录。
- 四个 contract section 全空是合法 ordinary run。一旦任一 section 非空，
  `--target` 与 `--instrument` 必须同时非空，否则硬失败；`--constraint` 与
  `--entropy-check` 缺失只产生可选完善告警。
- Instrument 语法是 `[LABEL::]CMD`，`CMD` 必须非空；没有 `::` 时整个值就是命令。

已有 run 通过 typed mutation 入口修补，不要直接编辑 `state.json`：

```bash
lto contract set --run-id <run-id> \
  --goal "repaired goal" \
  --done-when "repaired acceptance" \
  --host codex \
  --target "hidden eval recall >= 95%" \
  --instrument "hidden-eval::python3 eval/search_recall.py --hidden"
```

`contract set` 替换 goal/done-when/host，重复参数追加 delivery fields，并在任何
写入前校验合并后的 readiness 与 target ↔ instrument 配对关系。`audit-ledger.md`
由首轮 `lto audit` 派工创建（不是 `start`）；boundary gates 按需通过
`lto hook <gate>` 运行（见 `hooks.md`，不写入 `.git/hooks`）。

若 legacy state 已含 `label::` 这类没有命令的非法 instrument，用重复的
`--replace-instrument "label::command"` 显式替换全部 instruments；它与追加型
`--instrument` 互斥。成功结果会同步 `state.json`、`run-state.md` 和
`contract.updated` event；后置持久化失败时回滚 state/run-state，不留下可重复追加的部分提交。

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

Budget readouts are currently exposed through `lto budget check`. Historical
notes described `start --max-turns/--max-tokens/--deadline` and `budget extend`,
but those flags/subcommands are not part of the current Rust CLI.

When the target repo is not current directory, pass `--repo` before the command:

```bash
lto --repo /path/to/target/repo start \
  --goal "short task goal" --done-when "acceptance criteria" --host codex
```

After `bash scripts/install.sh`, the global wrapper is shorter:

```bash
lto --repo /path/to/target/repo start \
  --goal "short task goal" --done-when "acceptance criteria" --host codex
lto --repo /path/to/target/repo check
```

The wrapper is sentinel-managed and points at the current
`long-task-orchestration` checkout. If the repo moves, rerun `scripts/install.sh`.

This creates `.lto/<run-id>/` with:
- `state.json` — machine-readable state (source of truth)
- `run-state.md` — human-readable state
- `audit-ledger.md` — created later, by the first `lto audit` dispatch round

It also writes `.lto/current`, so later commands can omit `--run-id`.

**Boundary hooks are opt-in and on-demand**: `lto hook <gate> [--force] [--reason]`
runs a pre-commit/pre-deploy/pre-closeout style gate when you invoke it. The CLI
does not install anything into `.git/hooks`（早期版本的 `.git/hooks` 安装器已移除，
避免撞 husky / pre-commit framework）。

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

Environment health is always evaluated. With an active run, or an explicit
`--run-id`, preflight also emits a separate `run_readiness` result for hard
goal/done-when requirements and target ↔ instrument pairing, plus advisory
why/host/constraint/entropy-check gaps. An explicitly selected missing run is an
error after the environment result has been evaluated and emitted; with no explicit
or active run, output remains environment-only.

```bash
lto preflight                         # environment + active run readiness, if any
lto preflight --run-id <run-id>       # environment + this run's readiness
lto preflight --json                  # output shape only; no persistence side effect
lto preflight --record                # persist environment snapshot only
lto preflight --json --record         # the two flags remain independent
```

`--record` never turns readiness into persisted state and does not control whether
readiness is reported. Conversely, `--json` changes serialization only. If snapshot
persistence fails, JSON still contains the evaluated environment/readiness sections,
adds top-level `record_error`, and exits nonzero.

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
`--cwd`, `--timeout`. LTO never commits for you——`.lto` 状态提交是 host 动作。

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
(auditor agent name, default codex).
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
| `implementation` | base `run_readiness` (`goal` + `done_when`); target ↔ instrument pairing when a contract is present; no unresolved gate blocks or open unverified risks; filled audit ledger is `CONVERGED` when present | why/host and constraint/entropy-check gaps, task list present, phase direction |
| `closed` | base `run_readiness`; target ↔ instrument pairing when a contract is present; no open tasks (`status` not in `done`/`skipped`); no unresolved gate blocks; risk points verified or closed; filled audit ledger is `CONVERGED` when present | why/host and constraint/entropy-check gaps, artifact manifest, handoff if already closed, phase direction |

Default mode reports missing phase evidence but keeps rc 0 when the base
`check` passes. `--strict` upgrades missing required evidence to rc 1.
`--json` prints one JSON object to stdout and suppresses text/WARN output so
other host runtimes can parse it directly.

Even without `--to`, run-mode `check --strict` enforces base readiness and the
non-empty-contract target/instrument requirements. `closeout` repeats those hard
checks before archival so legacy or externally edited state cannot bypass C2.

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

closeout writes CHANGELOG.md but never commits——提交 `.lto` 相关产物与 CHANGELOG
是 host 的显式动作（提交权在你手里）。

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

Each records evidence via the shared `exec.run_command` kernel.
Real **agent fan-out** is `audit --auto-dispatch` / `--discover-risks`.
stdout/stderr artifacts are registered in `.lto/<run-id>/artifacts.json` using
repo-relative paths.

## Audit (adversarial heterogeneous review)

```bash
$L audit --auto-dispatch        # auto-dispatch heterogeneous auditors (≠ host family) + collect
$L audit --discover-risks       # spawn agent to find unregistered risk points (source=risk-agent)
$L audit                        # write brief + print dispatch instructions (manual)
$L collect-agent-run --task-id T1 --runner codex --reply reply-codex.md
```

Auditors emit structured JSON findings (severity is a field, not a regex scan).
Current Rust CLI has no `audit --collect <dir>`; manually produced runner replies
must be registered with `collect-agent-run` or as explicit artifacts/evidence.

## Next (fact router — zero LLM)

```bash
$L next            # print decision brief (escalate) or unambiguous cmd suggestion
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
stay with the human. Historical `--decide` flags are not exposed by the current
Rust CLI.

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
```

Current budget support is a readout/check surface. Earlier design notes described
run-level budget caps set at `start` (`--max-turns` / `--max-tokens` /
`--deadline`) and a `budget extend` unlock command, but those are not in the
current Rust CLI. If restored, the intended enforcement model remains graded:

- **Soft warning** at `warn_ratio` (default 0.8): a `⚠️ budget: …` fact line
  appears in `next`'s Decision Brief and `recap`. Zero block — it is a fact, not
  a recommendation; matching it to your decision stays the host's job.
- **Hard brake** at 100%: `autopilot` runs a budget gate before every
  auto-advance. Any dimension over limit → fail-closed `NEEDS_CONFIRM`, no
  auto-exec. Unlock mechanics need a future CLI design because `budget extend`
  is not currently implemented.

`turns_used` counts **autopilot auto-advance calls only** — human manual ops
(`runner`/`audit`/`next`) never consume a turn; the contract constrains
automation, not the human. Measurement lives in `src/budget.rs`
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
