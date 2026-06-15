#!/usr/bin/env bash
# Privacy self-check for local AI coding workflows.
# Dry-run by default. --delete prompts per item; no bulk delete.
set -euo pipefail

REPO="$(pwd)"
DELETE_MODE=0
STRICT=0
INCLUDE_HISTORY=0
RUN_GITLEAKS=1
MAX_SCAN_BYTES=$((2 * 1024 * 1024))
HOME_DIR="${PRIVACY_CHECK_HOME:-$HOME}"

usage() {
  cat >&2 <<'EOF'
Usage: scripts/privacy_self_check.sh [--repo PATH] [--delete] [--strict] [--include-history] [--no-gitleaks]

Checks:
  - Claude Code privacy env/settings posture
  - local transcript/feedback/state locations
  - git tracked sensitive files and AI state dirs
  - .gitignore coverage for AI/private artifacts
  - regex scan over repo files for secrets/private paths
  - optional gitleaks run when installed

Deletion:
  Default: report only, delete nothing.
  --delete: prompt for each cleanup candidate; type exactly "delete" per item.
  Never deletes tracked files automatically.
EOF
  exit 64
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --delete) DELETE_MODE=1; shift ;;
    --strict) STRICT=1; shift ;;
    --include-history) INCLUDE_HISTORY=1; shift ;;
    --no-gitleaks) RUN_GITLEAKS=0; shift ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done

REPO="$(cd "$REPO" && pwd)"
FINDINGS=0
WARNINGS=0
CLASSIFIED_REGEX_HITS=0
DELETE_PATHS=()
DELETE_REASONS=()

section() { printf '\n## %s\n' "$1"; }
ok() { printf 'OK   %s\n' "$1"; }
warn() { printf 'WARN %s\n' "$1"; WARNINGS=$((WARNINGS + 1)); }
fail() { printf 'FAIL %s\n' "$1"; FINDINGS=$((FINDINGS + 1)); }
info() { printf 'INFO %s\n' "$1"; }
classified_regex_hit() {
  printf 'OK   classified regex test fixture: %s\n' "$1"
  CLASSIFIED_REGEX_HITS=$((CLASSIFIED_REGEX_HITS + 1))
}

is_git_repo() { git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1; }

add_delete_candidate() {
  local path="$1" reason="$2"
  [[ -e "$path" ]] || return 0
  DELETE_PATHS+=("$path")
  DELETE_REASONS+=("$reason")
}

safe_to_delete_path() {
  local path="$1" real
  real="$(python3 - <<'PY' "$path"
import os, sys
print(os.path.realpath(sys.argv[1]))
PY
)"
  case "$real" in
    /|"$HOME_DIR"|"$REPO"|"$REPO/.git") return 1 ;;
  esac
  [[ -n "$real" && "$real" != "." ]]
}

is_tracked() {
  local rel="$1"
  is_git_repo || return 1
  git -C "$REPO" ls-files --error-unmatch -- "$rel" >/dev/null 2>&1
}

rel_to_repo() {
  python3 - <<'PY' "$REPO" "$1"
import os, sys
repo=os.path.realpath(sys.argv[1])
path=os.path.realpath(sys.argv[2])
try:
    print(os.path.relpath(path, repo))
except ValueError:
    print(path)
PY
}

is_classified_regex_hit() {
  local rel="$1" hit="$2" line_no test_start
  line_no="${hit%%:*}"
  case "$rel" in
    scripts/test_*.py) return 0 ;;
    src/llm_judge.rs)
      [[ "$line_no" =~ ^[0-9]+$ ]] || return 1
      test_start="$(grep -n '^\#\[cfg(test)\]' "$REPO/$rel" | head -1 | cut -d: -f1 || true)"
      [[ -n "$test_start" && "$line_no" -ge "$test_start" ]]
      return
      ;;
  esac
  return 1
}

check_env_var() {
  local key="$1" required="$2"
  if [[ -n "${!key:-}" ]]; then
    ok "$key is set"
  elif [[ "$required" == "required" ]]; then
    fail "$key is unset"
  else
    warn "$key is unset (recommended for sensitive sessions)"
  fi
}

section "Scope"
info "repo: $REPO"
info "home: $HOME_DIR"
if is_git_repo; then ok "git repo detected"; else warn "not a git repo; git checks skipped"; fi
if [[ "$DELETE_MODE" -eq 1 ]]; then warn "delete mode enabled: each item requires exact 'delete' confirmation"; else ok "dry-run mode: delete nothing"; fi

section "Claude Code privacy env"
check_env_var CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC required
check_env_var CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY recommended
check_env_var DISABLE_TELEMETRY recommended
check_env_var DO_NOT_TRACK recommended
check_env_var CLAUDE_CODE_SKIP_PROMPT_HISTORY recommended

section "Claude Code settings files"
for settings in \
  "$HOME_DIR/.claude/settings.json" \
  "$HOME_DIR/.claude/settings.local.json" \
  "$REPO/.claude/settings.json" \
  "$REPO/.claude/settings.local.json"; do
  if [[ -f "$settings" ]]; then
    info "settings found: $settings"
    if grep -q 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC' "$settings"; then
      ok "settings mention CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"
    else
      warn "settings does not mention CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: $settings"
    fi
    if grep -q 'CLAUDE_CODE_SKIP_PROMPT_HISTORY' "$settings"; then
      ok "settings mention CLAUDE_CODE_SKIP_PROMPT_HISTORY"
    else
      warn "settings does not mention CLAUDE_CODE_SKIP_PROMPT_HISTORY: $settings"
    fi
  fi
done

section "Local AI state and transcripts"
for path in \
  "$HOME_DIR/.claude/projects" \
  "$HOME_DIR/.claude/feedback-bundles" \
  "$HOME_DIR/.codex" \
  "$HOME_DIR/.gemini" \
  "$HOME_DIR/.config/agent-skills"; do
  if [[ -e "$path" ]]; then
    count="?"
    [[ -d "$path" ]] && count="$(find "$path" -maxdepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')"
    warn "local AI state exists: $path (files≈$count)"
    case "$path" in
      *feedback-bundles*) add_delete_candidate "$path" "Claude feedback bundles may contain shared transcripts" ;;
    esac
  else
    ok "not found: $path"
  fi
done

section "Repo-local AI/private state"
for rel in .claude .codex .gemini .agy .pi .lto feedback-bundles; do
  path="$REPO/$rel"
  if [[ -e "$path" ]]; then
    if is_tracked "$rel"; then
      fail "tracked repo-local state: $rel (manual git removal required)"
    else
      warn "untracked repo-local state: $rel"
      add_delete_candidate "$path" "untracked repo-local AI/private state"
    fi
  else
    ok "repo-local state absent: $rel"
  fi
done

section ".gitignore coverage"
GITIGNORE="$REPO/.gitignore"
if [[ ! -f "$GITIGNORE" ]]; then
  warn ".gitignore missing"
else
  for pat in '.lto/' '.claude/' '.codex/' '.gemini/' '.agy/' '.pi/' 'feedback-bundles/' '*.jsonl' '*.transcript' '.env' '.env.*' '*.pem' '*.key' 'credentials.json' 'service-account*.json'; do
    if grep -Fxq "$pat" "$GITIGNORE"; then
      ok ".gitignore covers $pat"
    else
      warn ".gitignore missing $pat"
    fi
  done
fi

section "Git tracked sensitive files"
if is_git_repo; then
  mapfile -t tracked_sensitive < <(git -C "$REPO" ls-files | grep -E '(^|/)(\.lto|\.claude|\.codex|\.gemini|\.agy|\.pi)(/|$)|(^|/)\.env(\..*)?$|\.(pem|key|p12)$|credentials\.json$|service-account.*\.json$' || true)
  if [[ "${#tracked_sensitive[@]}" -eq 0 ]]; then
    ok "no tracked AI state/secrets by filename"
  else
    for f in "${tracked_sensitive[@]}"; do fail "tracked sensitive-looking file: $f"; done
  fi
fi

section "Repo regex privacy scan"
PRIVATE_HOME_RE="$(python3 - <<'PY' "$HOME_DIR"
import re, sys
print(re.escape(sys.argv[1]))
PY
)"
SCAN_PATTERNS="(${PRIVATE_HOME_RE}|sk-[A-Za-z0-9_-]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN (RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----|\b(10|172\.(1[6-9]|2[0-9]|3[01])|192\.168)\.[0-9]{1,3}\.[0-9]{1,3}\b)"
if is_git_repo; then
  mapfile -t scan_files < <(git -C "$REPO" ls-files --cached --others --exclude-standard)
else
  mapfile -t scan_files < <(find "$REPO" -type f -not -path '*/.git/*' 2>/dev/null | sed "s#^$REPO/##" || true)
fi
scan_hits=0
for rel in "${scan_files[@]}"; do
  [[ "$rel" == .git/* ]] && continue
  path="$REPO/$rel"
  [[ -f "$path" ]] || continue
  size=$(wc -c < "$path" | tr -d ' ')
  [[ "$size" -le "$MAX_SCAN_BYTES" ]] || { warn "skip large file in regex scan: $rel ($size bytes)"; continue; }
  if LC_ALL=C grep -Iq . "$path" 2>/dev/null && grep -En "$SCAN_PATTERNS" "$path" >/tmp/privacy_self_check_hits.$$ 2>/dev/null; then
    while IFS= read -r hit; do
      if is_classified_regex_hit "$rel" "$hit"; then
        classified_regex_hit "$rel:$hit"
      else
        fail "$rel:$hit"
        scan_hits=$((scan_hits + 1))
      fi
    done < /tmp/privacy_self_check_hits.$$
  fi
  rm -f /tmp/privacy_self_check_hits.$$
done
if [[ "$scan_hits" -eq 0 && "$CLASSIFIED_REGEX_HITS" -eq 0 ]]; then
  ok "regex scan clean"
elif [[ "$scan_hits" -eq 0 ]]; then
  ok "regex scan has ${CLASSIFIED_REGEX_HITS} classified test fixture hit(s), 0 unclassified"
fi

section "Gitleaks"
if [[ "$RUN_GITLEAKS" -eq 0 ]]; then
  info "gitleaks skipped by --no-gitleaks"
elif command -v gitleaks >/dev/null 2>&1; then
  if gitleaks detect --source "$REPO" --redact --no-banner >/tmp/privacy_self_check_gitleaks.$$ 2>&1; then
    ok "gitleaks clean"
  else
    fail "gitleaks found issues (redacted output below)"
    sed 's/^/  /' /tmp/privacy_self_check_gitleaks.$$
  fi
  rm -f /tmp/privacy_self_check_gitleaks.$$
else
  warn "gitleaks not installed; install with: brew install gitleaks"
fi

if [[ "$INCLUDE_HISTORY" -eq 1 ]]; then
  section "Shell history scan"
  for hist in "$HOME_DIR/.zsh_history" "$HOME_DIR/.bash_history"; do
    if [[ -f "$hist" ]]; then
      if grep -En 'sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|PRIVATE KEY|token=|api[_-]?key' "$hist" >/tmp/privacy_self_check_hist.$$ 2>/dev/null; then
        fail "possible secrets in shell history: $hist"
        sed 's/^/  /' /tmp/privacy_self_check_hist.$$ | head -20
      else
        ok "history clean by regex: $hist"
      fi
      rm -f /tmp/privacy_self_check_hist.$$
    fi
  done
else
  info "shell history scan skipped; pass --include-history to enable"
fi

section "Cleanup candidates"
if [[ "${#DELETE_PATHS[@]}" -eq 0 ]]; then
  ok "no cleanup candidates"
else
  for i in "${!DELETE_PATHS[@]}"; do
    printf 'CANDIDATE %s\n  path: %s\n  reason: %s\n' "$((i + 1))" "${DELETE_PATHS[$i]}" "${DELETE_REASONS[$i]}"
  done
fi

if [[ "$DELETE_MODE" -eq 1 && "${#DELETE_PATHS[@]}" -gt 0 ]]; then
  section "Confirmed deletion"
  for i in "${!DELETE_PATHS[@]}"; do
    path="${DELETE_PATHS[$i]}"
    reason="${DELETE_REASONS[$i]}"
    if ! safe_to_delete_path "$path"; then
      fail "refuse unsafe delete target: $path"
      continue
    fi
    rel="$(rel_to_repo "$path")"
    if [[ "$rel" != ../* && "$rel" != /* ]] && is_tracked "$rel"; then
      fail "refuse deleting tracked file/dir automatically: $rel"
      continue
    fi
    printf '\nDelete this item?\n  path: %s\n  reason: %s\nType exactly "delete" to confirm: ' "$path" "$reason"
    read -r answer || answer=""
    if [[ "$answer" == "delete" ]]; then
      rm -rf -- "$path"
      ok "deleted: $path"
    else
      info "skipped: $path"
    fi
  done
fi

section "Summary"
printf 'findings=%s warnings=%s classified_regex_hits=%s cleanup_candidates=%s\n' "$FINDINGS" "$WARNINGS" "$CLASSIFIED_REGEX_HITS" "${#DELETE_PATHS[@]}"
if [[ "$STRICT" -eq 1 && "$FINDINGS" -gt 0 ]]; then
  exit 1
fi
exit 0
