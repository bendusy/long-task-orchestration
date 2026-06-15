# Privacy self-check

> Local AI coding privacy is not one switch. Check cloud telemetry, local transcripts, repo artifacts, git tracking, secret patterns, and cleanup candidates separately.

Run from the repo:

```bash
scripts/privacy_self_check.sh --repo .
```

Default mode is dry-run:

- prints findings and warnings;
- deletes nothing;
- reports cleanup candidates;
- exits 0 unless `--strict` is used and findings exist.

## Checks

The script checks:

1. Claude Code privacy environment variables:
   - `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`
   - `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`
   - `DISABLE_TELEMETRY`
   - `DO_NOT_TRACK`
   - `CLAUDE_CODE_SKIP_PROMPT_HISTORY`
2. Claude settings files under `~/.claude/` and repo `.claude/`.
3. Local AI state locations, including Claude projects and feedback bundles.
4. Repo-local AI/private state directories:
   - `.claude/`, `.codex/`, `.gemini/`, `.agy/`, `.pi/`, `.lto/`, `feedback-bundles/`
5. `.gitignore` coverage for AI state, transcripts, env files, keys, and credentials.
6. Git-tracked sensitive-looking files.
7. Regex scan for private home paths, private IPs, common API key/token patterns, and private key blocks.
8. `gitleaks` scan if installed.
9. Optional shell history scan with `--include-history`.

Regex hits in explicit redaction tests are still printed, but classified as test
fixtures instead of counted as unclassified findings. The classifier is narrow:
`scripts/test_*.py` and Rust lines inside `#[cfg(test)]` modules. A matching
secret or private path anywhere else remains a finding and fails `--strict`.

## Cleanup mode

Deletion is opt-in:

```bash
scripts/privacy_self_check.sh --repo . --delete
```

For each candidate, the script prints:

- path;
- reason;
- confirmation prompt.

It only deletes that one item if you type exactly:

```text
delete
```

Anything else skips the item. There is no yes-all mode.

The script refuses obviously unsafe targets such as `/`, `$HOME`, repo root, and `.git`. It also refuses to delete tracked files automatically; tracked secrets require manual `git rm` / history cleanup.

## Sensitive sessions

Before sensitive Claude Code sessions:

```bash
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
export CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY=1
export DISABLE_TELEMETRY=1
export DO_NOT_TRACK=1
export CLAUDE_CODE_SKIP_PROMPT_HISTORY=1
```

Then run:

```bash
scripts/privacy_self_check.sh --repo . --strict --include-history
```

## Cleanup cautions

- `.lto/` contains local run evidence and recovery state. Delete only after closeout/export, and only if you no longer need resume/audit evidence.
- `~/.claude/projects` may contain useful resume history. Delete only when privacy beats continuity.
- `~/.claude/feedback-bundles` may contain feedback archives. Inspect before deleting.
- Shell history cleanup is intentionally report-only; edit history manually after reviewing lines.

## CI use

For CI, avoid touching `$HOME` and disable gitleaks if unavailable:

```bash
PRIVACY_CHECK_HOME=/tmp/empty-home \
  scripts/privacy_self_check.sh --repo . --strict --no-gitleaks
```
