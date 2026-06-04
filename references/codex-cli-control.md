# Codex CLI control contract for LTO

> Distilled from `sugarforever/01coder-agent-skills/skills/codex-cli` and adapted for LTO's harness role. This is not a general Codex skill; it is the subset LTO needs when Codex is a host or an audit/delegation runner.

## 1. Source of truth: installed CLI help

Before using Codex as runner, verify the real binary and current flags:

```bash
command -v codex
codex --version
codex exec --help
```

If any fail, mark Codex runner unavailable. Do **not** simulate a Codex answer. Codex CLI flags change; `codex exec --help` beats stale docs.

## 2. Non-interactive runner shape

Use `codex exec`, not interactive `codex`, for LTO delegation:

```bash
codex exec -C "$PWD" -s read-only -o /tmp/reply.md - < /tmp/prompt.md
```

LTO runner contract:

- prompt enters via stdin (`-`) to avoid argv length and quoting failures;
- final answer goes to a reply file via `-o` / `--output-last-message`;
- scheduler judges by exit code + reply bytes, not by vibes;
- stdout is fallback only; stderr stays diagnostic evidence;
- timeout `124` means timeout, not empty output.

## 3. Sandbox defaults

Default sandbox is `read-only`:

```bash
CODEX_SANDBOX=read-only scripts/delegate/runners/codex.sh prompt.md reply.md 300
```

Use `workspace-write` only when Codex is explicitly expected to edit files:

```bash
CODEX_SANDBOX=workspace-write scripts/delegate/runners/codex.sh prompt.md reply.md 900
```

Avoid `danger-full-access` unless user explicitly approved and the outer environment is already controlled. LTO must never equate "long task" with "full disk permission".

## 4. Prompt shape for delegated Codex work

For non-trivial jobs, shape prompt as:

```text
Goal:
Context:
Constraints:
Done when:
Output format:
```

Audit/review jobs should include:

- do not ask follow-up questions;
- do not modify files unless explicitly authorized;
- emit findings with severity as data, not prose keywords;
- state uncertainty and evidence paths;
- exit normally only after writing final answer.

Complex implementation: ask for a `read-only` plan first, then run a separate `workspace-write` job after human/host approval.

## 5. Resume is not state truth

Codex `exec resume <session-id>` can continue a thread, but LTO must still rebuild truth from disk:

```bash
git status --short
python3 scripts/lto_run.py resume
python3 scripts/lto_run.py next
```

Do not trust transcript memory like "background task still running" or "already fixed". Trust `.lto/<run-id>/state.json`, `artifacts.json`, git SHA, reply files, and current filesystem.

## 6. File and image inputs

For repo files, point Codex at the workspace with `-C "$PWD"` and name files in prompt. For non-repo logs or long text, pipe via stdin.

Images must be attached explicitly:

```bash
codex exec -C "$PWD" -s read-only -i before.png -i after.png \
  "Compare these UI states against the spec"
```

LTO's default runner supports `CODEX_IMAGES="a.png,b.png"` for rare visual audit jobs. Keep image jobs read-only.

## 7. Generated images are out of scope for LTO closeout

Codex may generate images under `${CODEX_HOME:-$HOME/.codex}/generated_images/<session-id>/` and may not print the path. If an LTO task ever asks Codex to generate project assets, the host must locate the real file, verify dimensions if relevant, and copy the asset into the project before referencing it. Do not leave required assets only under Codex home.

## 8. Runner env knobs

Standalone LTO ships `scripts/delegate/runners/codex.sh` with these controls:

| Env | Default | Use |
|---|---:|---|
| `CODEX_BIN` | `codex` | alternate binary/fake binary in tests |
| `CODEX_WORKDIR` | `$PWD` | value for `codex exec -C` |
| `CODEX_SANDBOX` | `read-only` | `read-only` / `workspace-write` / `danger-full-access` |
| `CODEX_MODEL` | unset | optional `-m` model |
| `CODEX_PROFILE` | unset | optional `-p` profile |
| `CODEX_JSON` | `0` | add `--json`; reply file remains final answer sink |
| `CODEX_IMAGES` | unset | comma/colon-separated image attachments |

## 9. Job-level env and permission guard

LTO's scheduler can pass env per `AgentJob` instead of relying on process-global exports:

```python
AgentJob(
    job_id="impl-review",
    runner="codex",
    prompt_ref="...",
    prompt_is_inline=True,
    env={"CODEX_PROFILE": "lto"},
    permission_policy=PermissionPolicy(
        sandbox="read-only",
        reason="audit only; no file edits",
        user_approved=False,
    ),
)
```

Guard rules:

- default Codex sandbox is `read-only`, even if parent process has `CODEX_SANDBOX` set;
- `workspace-write` requires `permission_policy.reason`;
- `danger-full-access` requires `permission_policy.reason` and `user_approved=True`;
- `env["CODEX_SANDBOX"]` must match `permission_policy.sandbox` or validation fails;
- scheduler persists a safe permission snapshot in `AgentResult.permissions` (`sandbox`, `reason`, `user_approved`, `env_keys`) so closeout can audit why a runner had write power.

Use this pattern for host-agent judgment:

| Job intent | `permission_policy.sandbox` |
|---|---|
| review / audit / plan / critique | `read-only` |
| approved implementation/edit pass | `workspace-write` with reason |
| externally sandboxed emergency only | `danger-full-access` with explicit approval |

## 10. Failure handling

Report Codex failures plainly:

- missing binary: runner unavailable;
- auth/network/model refresh error: include stderr tail;
- exit `0` + empty reply: output contract failure;
- exit `124`: timeout, increase budget or reduce prompt;
- rate-limit text on non-zero exit: retry/backoff if policy allows.

After any Codex write task, inspect git diff. Never claim Codex changed files solely from its final message.
