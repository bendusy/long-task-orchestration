# LTO plugin boundary v0

> Plugins let LTO try external ideas without turning core into a preset marketplace. Core stays harness primitives; plugins stay data-only, provenance-rich, and opt-in.

## 1. Invariant

LTO core owns deterministic primitives:

- `state.json` / `artifacts.json`
- `AgentJob` / `AgentResult`
- `Scheduler` / runner contracts
- `PermissionPolicy`
- `runner` / `audit` / `judge` / `next` / `recap` / `closeout`

A plugin may only compile into those existing contracts. It must not introduce new execution semantics, hidden routing, background daemons, arbitrary code, or permission escalation.

## 2. Why this exists

External articles often contain useful harness ideas: model-specific profiles, better audit prompts, eval patterns, tool naming conventions, or output schemas. Directly editing LTO core for each idea causes:

- plugin/preset sprawl;
- authority laundering ("blog said it" becomes "LTO guarantees it");
- runtime compatibility debt;
- untested prompt changes masquerading as architecture;
- core drift away from harness primitives.

Plugin v0 creates a quarantine lane:

```text
source note → experimental path/profile → eval evidence → blessed plugin or rejected tombstone
```

Promotion to core is rare. Most wins should remain plugins.

## 3. Object model

### Source note

A source note captures provenance and falsifiable hypotheses. It is inert.

```json
{
  "id": "note.langchain.deepagents-profiles.2026-06-04",
  "url": "https://www.langchain.com/blog/tuning-deep-agents-different-models",
  "claims": [
    {"id": "c1", "text": "Per-model harness profiles improve agent eval results", "status": "unverified"}
  ],
  "hypotheses": [
    {"id": "h1", "text": "Codex audit improves when prompts ask it to batch reads first"}
  ]
}
```

### Path plugin

A path plugin is a playbook fragment. It may suggest primitive sequences, but host agent remains planner.

### Runtime profile

A runtime profile is a declarative overlay for one runner/model/intent. It may set prompt refs, output schema refs, budget hints, and allowed env keys. It may not grant permissions by itself.

### Eval pack

An eval pack is the evidence harness for a plugin. It compares a baseline path/profile against the candidate and records parse rate, blocker quality, false positives, timeout rate, cost, and safety regressions.

## 4. Directory contract

```text
plugins/<plugin-id>/
  plugin.json
  sources/
    *.json
    *.md
  paths/
    *.json
  profiles/
    *.json
  prompts/
    *.md
  schemas/
    *.json
  eval/
    *.json
```

All v0 plugin files are data or Markdown. No executable plugin code.

## 5. `plugin.json` v0

Required fields:

```json
{
  "id": "deep-agent-profiles",
  "version": "0.1.0",
  "stage": "experimental",
  "kind": "path-plugin",
  "description": "Model/runtime profile experiments from deep-agent harness tuning",
  "source_notes": ["sources/langchain-tuning-deep-agents.json"],
  "provides": {
    "paths": ["paths/model-profile-ab-test.json"],
    "profiles": ["profiles/codex-audit-readonly.json"],
    "evals": ["eval/profile-ab-cases.json"]
  },
  "security": {
    "executable_code": false,
    "max_sandbox": "read-only",
    "env_allowlist": ["CODEX_MODEL", "CODEX_PROFILE", "CODEX_JSON"],
    "requires_human_approval_for": ["workspace-write", "danger-full-access", "network"]
  },
  "default_enabled": false
}
```

Allowed `stage`: `experimental`, `blessed`, `rejected`.
Allowed `kind`: `path-plugin`.
Allowed `max_sandbox`: `read-only`, `workspace-write`, `danger-full-access`.

## 6. Mount lock

Mounting a plugin writes run-scoped evidence:

```text
.lto/<run-id>/plugin-mounts.json
```

Each entry records:

- plugin id/version/stage;
- plugin path;
- manifest hash;
- source note paths;
- profile/path/eval refs;
- approved max sandbox;
- who/what approved mount;
- timestamp.

The lock is evidence, not execution. It lets closeout answer: "Which experimental idea influenced this run?"

## 7. Security gates

### Load/validate gate

`lto plugin validate <dir>` rejects:

- missing or malformed `plugin.json`;
- invalid id/version/stage/kind;
- `security.executable_code != false`;
- missing provided files;
- absolute paths or `..` traversal;
- env keys outside safe pattern;
- source notes missing;
- JSON files that do not parse.

### Mount gate

`lto plugin mount <dir>` rejects invalid plugin and writes a mount lock. It does not auto-apply profiles.

### Pre-exec gate

Existing `PermissionPolicy` remains source of truth:

- `workspace-write` requires reason;
- `danger-full-access` requires reason and `user_approved=True`;
- runner env conflicts fail validation.

A plugin can lower permission ceilings. It cannot raise them.

### Post-exec gate

For plugin-influenced jobs, host should verify (✅ = checked by `eval-run` deterministic metrics):

- reply is not pointer-only ✅ (`pointer_only` metric);
- output schema parse succeeded ✅ (`parse_ok` metric);
- no local private paths in public artifacts ✅ (`private_path_leak` metric);
- permission snapshot matches mount approval ✅ (`permission_violation` metric, sandbox-rank based).

## 8. Runtime profile strategy

### Codex

- default `read-only`;
- prompt via stdin;
- final answer via `-o` reply file;
- no trust in transcript resume;
- profile may set `CODEX_PROFILE`/`CODEX_MODEL`, not sandbox escalation.

### Pi / DeepSeek

- timeout budget higher for thinking models;
- distinguish CLI headless from internal Agent tools;
- do not assume worktree isolation unless observed.

### Agy / Gemini

- useful for adversarial framing;
- require output contract gate because some runs produce pointer-only replies;
- v0 write tasks should remain disabled unless separately proven. agy/gemini 无法兑现 read-only（agy 无 read-only 档），scheduler validate 时 fail-closed 拒绝其 read-only 审计。

### Claude

- strong reflection profile candidate;
- healthcheck auth before counting as auditor;
- prompt should force direct file/test observation.

## 9. Promotion rules

Experimental → blessed requires:

- at least N=5 eval or real runs;
- no P0/P1 safety regression;
- parse rate not worse than baseline;
- false-positive rate not worse than baseline;
- blocker capture not worse than baseline;
- cost/time regression justified and recorded;
- human approval.

Blessed → core requires stricter evidence: multiple independent plugins reveal the same missing primitive that cannot be expressed with current contracts, or the maintainer explicitly promotes a generic harness primitive. `delivery_contract` is a core primitive because it defines deliverable target/constraint/instrument/forced-entropy fields for any long run; plugin paths may still provide domain-specific playbooks around it.

Rejected plugins stay tombstoned with `stage: rejected` and `rejection_reason`.

## 10. v0 non-goals

Do not implement yet:

- plugin marketplace;
- URL auto-ingestion;
- arbitrary Python/JS plugin code;
- middleware framework;
- dynamic tool installation;
- plugin dependency DAG;
- automatic profile selection;
- automatic promotion to core;
- one-click "best path" routing.

Rust v2 owns the plugin path: list, validate, render-profile, eval, mount, source-note creation, and real eval-run. Host agent still decides. Python remains only as an explicit compatibility fallback until the formal removal gate deletes the fallback tree.

## 11. Implemented v0 commands

```bash
# Discover and validate data-only plugins
lto plugin list
lto plugin validate plugins/deep-agent-profiles --json

# Create an inert source note and optionally append it to plugin.json
lto plugin source-note plugins/deep-agent-profiles \
  --id note.example.article \
  --title "Interesting Article" \
  --url "https://example.com/article" \
  --claim "Article claims X improves Y" \
  --hypothesis "Test whether X improves parse_rate" \
  --append-manifest
# source-note writes sources/<id>.json; pass --append-manifest to add it to plugin.json.

# Render a profile into a normal prompt file; execution still goes through runner/AgentJob
lto plugin render-profile plugins/deep-agent-profiles codex-audit-readonly-v1 \
  --input brief.md \
  --output rendered-brief.md \
  --meta-output rendered-brief.meta.json

# Static eval-pack validation (metadata only, no model run)
lto plugin eval plugins/deep-agent-profiles --json

# Mount plugin provenance into the active run; does not auto-apply profiles
lto plugin mount plugins/deep-agent-profiles --approved-by host

# Real baseline-vs-candidate A/B run with deterministic metrics (v0)
# Compiles each eval-pack case into two AgentJobs and runs them via the scheduler.
lto plugin eval-run plugins/deep-agent-profiles --run-id <run-id> --json
```

Rust `plugin eval` stays deliberately static: it checks declared eval packs, profile references, metrics, and safety metadata. Rust `plugin eval-run` (v0) does the real model A/B: it compiles each case into a baseline (bare brief) and candidate (profile-injected) AgentJob, runs both through the normal scheduler/runner primitives, and records deterministic metrics + evidence under `.lto/<run-id>/plugin-eval/<case-id>/`. Automatic promotion remains deferred (see §12).

## 12. Real eval runner boundary

Rust `plugin eval-run` follows [`plugin-real-eval-runner.md`](./plugin-real-eval-runner.md): it is a **sub-LTO-run compiler**, not a new workflow engine. Rust-owned `plugin source-note` may create inert provenance files, but it does not run, route, promote, or raise permissions. The rules below are the boundary eval-run stays inside; automatic promotion remains human-gated and is declared as deferred in each run report.

Design rules:

- article/source claims stay `unverified` until turned into falsifiable hypotheses and frozen evidence cases;
- live or historical evidence must be copied, hashed, and redacted before candidate jobs see it;
- deterministic metrics and judged metrics must be separated;
- parallel/swarm evaluation is opt-in, bounded, and only allowed for cases declaring `parallelizable=true`;
- promotion remains human-gated and cannot be triggered by plugin metadata.
