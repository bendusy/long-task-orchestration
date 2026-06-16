#!/usr/bin/env bash
# Healthcheck real runtime runners before dispatch.
#
# Usage:
#   healthcheck.sh [agent1 agent2 ...]
#   healthcheck.sh --json
#   PROBE_TIMEOUT=120 healthcheck.sh
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROBE_TIMEOUT="${PROBE_TIMEOUT:-90}"
JSON=0
AGENTS=()

for arg in "$@"; do
  case "$arg" in
    --json) JSON=1 ;;
    *) AGENTS+=("$arg") ;;
  esac
done
[[ ${#AGENTS[@]} -eq 0 ]] && AGENTS=(codex pi agy claude)

PROMPT_FILE="$(mktemp)"
REPLY_FILE="$(mktemp)"
trap 'rm -f "$PROMPT_FILE" "$REPLY_FILE"' EXIT
printf 'Only answer the result of this expression: 1+1=\n' > "$PROMPT_FILE"

verdict() {
  local rc="$1" bytes="$2"
  if [[ "$rc" -eq 0 && "$bytes" -gt 0 ]]; then
    echo OK
  elif [[ "$rc" -eq 0 && "$bytes" -eq 0 ]]; then
    echo EMPTY
  elif [[ "$rc" -eq 124 ]]; then
    echo TIMEOUT
  else
    echo "ERROR(rc=$rc)"
  fi
}

results=()
for agent in "${AGENTS[@]}"; do
  if [[ "$agent" == "tmux" ]]; then
    start=$SECONDS
    if command -v tmux >/dev/null 2>&1 && tmux -V >/dev/null 2>&1; then
      rc=0
      bytes=1
    else
      rc=127
      bytes=0
    fi
    elapsed=$((SECONDS - start))
    results+=("$agent|$rc|${elapsed}s|$bytes|$(verdict "$rc" "$bytes")")
    continue
  fi
  runner="$SCRIPT_DIR/$agent.sh"
  if [[ ! -f "$runner" ]]; then
    results+=("$agent|-|-|0|MISSING")
    continue
  fi
  : > "$REPLY_FILE"
  start=$SECONDS
  set +e
  bash "$runner" "$PROMPT_FILE" "$REPLY_FILE" "$PROBE_TIMEOUT" >/dev/null 2>&1
  rc=$?
  set -e 2>/dev/null || true
  elapsed=$((SECONDS - start))
  bytes="$(wc -c < "$REPLY_FILE" 2>/dev/null | tr -d ' ')"
  bytes="${bytes:-0}"
  results+=("$agent|$rc|${elapsed}s|$bytes|$(verdict "$rc" "$bytes")")
done

if [[ "$JSON" -eq 1 ]]; then
  printf '['
  first=1
  for row in "${results[@]}"; do
    IFS='|' read -r agent rc elapsed bytes vd <<< "$row"
    [[ "$first" -eq 0 ]] && printf ','
    printf '{"agent":"%s","exit":"%s","elapsed":"%s","bytes":"%s","verdict":"%s"}' \
      "$agent" "$rc" "$elapsed" "$bytes" "$vd"
    first=0
  done
  printf ']\n'
else
  printf '%-8s %-6s %-8s %-8s %s\n' RUNNER EXIT ELAPSED BYTES VERDICT
  printf '%-8s %-6s %-8s %-8s %s\n' ------ ---- ------- ----- -------
  for row in "${results[@]}"; do
    IFS='|' read -r agent rc elapsed bytes vd <<< "$row"
    printf '%-8s %-6s %-8s %-8s %s\n' "$agent" "$rc" "$elapsed" "$bytes" "$vd"
  done
fi

for row in "${results[@]}"; do
  [[ "$row" == *"|OK" ]] && exit 0
done
exit 1
