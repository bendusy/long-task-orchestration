#!/usr/bin/env bash
# Run several bundled delegate runners in parallel and collect replies.
#
# Usage:
#   triad.sh -p <prompt_file> -d <reply_dir> [-a "codex pi agy"] [-t <timeout>]
#            [--headless] [-s <tmux_session>] [--keep-window]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DELEGATE="$SCRIPT_DIR/delegate.sh"

PROMPT_FILE=""
REPLY_DIR=""
AGENTS_RAW="codex pi agy"
TIMEOUT_SEC=900
FORCE_HEADLESS=0
SESSION=""
KEEP_WINDOW=0

usage() {
  sed -n '2,9p' "$0" >&2
  exit 64
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p) PROMPT_FILE="$2"; shift 2 ;;
    -d) REPLY_DIR="$2"; shift 2 ;;
    -a) AGENTS_RAW="$2"; shift 2 ;;
    -t) TIMEOUT_SEC="$2"; shift 2 ;;
    --headless) FORCE_HEADLESS=1; shift ;;
    -s) SESSION="$2"; shift 2 ;;
    --keep-window) KEEP_WINDOW=1; shift ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done

[[ -z "$PROMPT_FILE" || -z "$REPLY_DIR" ]] && usage
[[ -f "$PROMPT_FILE" ]] || { echo "prompt missing: $PROMPT_FILE" >&2; exit 2; }
[[ "$TIMEOUT_SEC" =~ ^[0-9]+$ ]] || { echo "timeout must be integer seconds: $TIMEOUT_SEC" >&2; exit 64; }
[[ -x "$DELEGATE" ]] || chmod +x "$DELEGATE"

mkdir -p "$REPLY_DIR"
read -r -a AGENTS <<< "$AGENTS_RAW"

echo "[triad] dispatching: ${AGENTS[*]} (timeout=${TIMEOUT_SEC}s)" >&2
PIDS=()
for agent in "${AGENTS[@]}"; do
  (
    reply="$REPLY_DIR/$agent.md"
    rcfile="$REPLY_DIR/$agent.exit"
    args=(-a "$agent" -p "$PROMPT_FILE" -o "$reply" -t "$TIMEOUT_SEC")
    [[ "$FORCE_HEADLESS" -eq 1 ]] && args+=(--headless)
    [[ -n "$SESSION" ]] && args+=(-s "$SESSION")
    [[ "$KEEP_WINDOW" -eq 1 ]] && args+=(--keep-window)
    if "$DELEGATE" "${args[@]}"; then
      echo 0 > "$rcfile"
    else
      echo $? > "$rcfile"
    fi
  ) &
  PIDS+=("$!")
done

for pid in "${PIDS[@]}"; do
  wait "$pid" || true
done

FAIL=0
for agent in "${AGENTS[@]}"; do
  reply="$REPLY_DIR/$agent.md"
  rcfile="$REPLY_DIR/$agent.exit"
  rc="??"
  [[ -f "$rcfile" ]] && rc="$(<"$rcfile")"
  bytes=0
  [[ -s "$reply" ]] && bytes="$(wc -c < "$reply" | tr -d ' ')"
  if [[ "$rc" == "0" && "$bytes" -gt 0 ]]; then
    echo "[triad] $agent: OK ($bytes bytes, rc=$rc)"
  else
    echo "[triad] $agent: FAIL ($bytes bytes, rc=$rc)" >&2
    FAIL=1
  fi
done

exit "$FAIL"
