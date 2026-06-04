#!/usr/bin/env bash
# LTO bundled delegate runner.
#
# Usage:
#   delegate.sh -a <agent> -p <prompt_file> -o <reply_file> [-t <timeout_sec>]
#               [--headless] [-s <tmux_session>] [--keep-window]
#
# Agents: codex | claude | pi | agy | gemini
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNNERS_DIR="${AGENT_DELEGATE_RUNNERS:-$SCRIPT_DIR/runners}"

AGENT=""
PROMPT_FILE=""
REPLY_FILE=""
TIMEOUT_SEC=900
FORCE_HEADLESS=0
SESSION=""
KEEP_WINDOW=0
AD_CALL_DEPTH="${AD_CALL_DEPTH:-0}"
AD_MAX_CALL_DEPTH="${AD_MAX_CALL_DEPTH:-2}"
AD_HOST_AGENT="${AD_HOST_AGENT:-}"

usage() {
  sed -n '2,12p' "$0" >&2
  exit 64
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -a) AGENT="$2"; shift 2 ;;
    -p) PROMPT_FILE="$2"; shift 2 ;;
    -o) REPLY_FILE="$2"; shift 2 ;;
    -t) TIMEOUT_SEC="$2"; shift 2 ;;
    --headless) FORCE_HEADLESS=1; shift ;;
    -s) SESSION="$2"; shift 2 ;;
    --keep-window) KEEP_WINDOW=1; shift ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done

[[ -z "$AGENT" || -z "$PROMPT_FILE" || -z "$REPLY_FILE" ]] && usage
[[ -f "$PROMPT_FILE" ]] || { echo "prompt file missing: $PROMPT_FILE" >&2; exit 2; }
[[ "$TIMEOUT_SEC" =~ ^[0-9]+$ ]] || { echo "timeout must be integer seconds: $TIMEOUT_SEC" >&2; exit 64; }
[[ "$AD_CALL_DEPTH" =~ ^[0-9]+$ ]] || { echo "AD_CALL_DEPTH must be a non-negative integer" >&2; exit 64; }
[[ "$AD_MAX_CALL_DEPTH" =~ ^[0-9]+$ ]] || { echo "AD_MAX_CALL_DEPTH must be a non-negative integer" >&2; exit 64; }

case "$AGENT" in
  codex|claude|pi|agy|gemini) RUNNER="$RUNNERS_DIR/$AGENT.sh" ;;
  *) echo "unknown agent: $AGENT (expect codex|claude|pi|agy|gemini)" >&2; exit 64 ;;
esac
[[ -f "$RUNNER" ]] || { echo "runner missing: $RUNNER" >&2; exit 2; }
[[ -x "$RUNNER" ]] || chmod +x "$RUNNER"

if (( AD_CALL_DEPTH >= AD_MAX_CALL_DEPTH )); then
  echo "[$AGENT] refusing delegation: AD_CALL_DEPTH=$AD_CALL_DEPTH reached AD_MAX_CALL_DEPTH=$AD_MAX_CALL_DEPTH" >&2
  exit 65
fi
if [[ -n "$AD_HOST_AGENT" && "$AD_HOST_AGENT" == "$AGENT" ]]; then
  echo "[$AGENT] refusing same-runtime delegation from AD_HOST_AGENT=$AD_HOST_AGENT" >&2
  exit 65
fi
NEXT_CALL_DEPTH=$((AD_CALL_DEPTH + 1))

mkdir -p "$(dirname "$REPLY_FILE")"
: > "$REPLY_FILE"

run_subprocess() {
  AD_CALL_DEPTH="$NEXT_CALL_DEPTH" AD_MAX_CALL_DEPTH="$AD_MAX_CALL_DEPTH" AD_HOST_AGENT="$AGENT" \
    "$RUNNER" "$PROMPT_FILE" "$REPLY_FILE" "$TIMEOUT_SEC"
}

run_tmux() {
  local sess="$1"
  local sig="lto-dlg-${AGENT}-$$-${RANDOM}"
  local win="lto-dlg-${AGENT}-$$-${RANDOM}"
  local rcfile
  rcfile="$(mktemp "${REPLY_FILE}.rc.XXXX")"

  local payload
  payload="AD_CALL_DEPTH=$(printf '%q' "$NEXT_CALL_DEPTH") AD_MAX_CALL_DEPTH=$(printf '%q' "$AD_MAX_CALL_DEPTH") AD_HOST_AGENT=$(printf '%q' "$AGENT") $(printf '%q' "$RUNNER") $(printf '%q' "$PROMPT_FILE") $(printf '%q' "$REPLY_FILE") $(printf '%q' "$TIMEOUT_SEC"); echo \$? > $(printf '%q' "$rcfile"); tmux wait-for -S $(printf '%q' "$sig")"

  local wid
  if ! wid="$(tmux new-window -P -F '#{window_id}' -t "$sess" -n "$win" "bash -c $(printf '%q' "$payload")" 2>/dev/null)"; then
    echo "[$AGENT] tmux new-window failed" >&2
    rm -f "$rcfile"
    return 1
  fi
  [[ "$KEEP_WINDOW" -eq 1 ]] && tmux set-window-option -t "$wid" remain-on-exit on 2>/dev/null || true
  echo "[$AGENT] dispatched -> tmux ${sess}:${win} id=${wid}" >&2

  local rc=0
  if ! timeout "$((TIMEOUT_SEC + 30))s" tmux wait-for "$sig" 2>/dev/null; then
    echo "[$AGENT] wait-for timeout; killing $wid" >&2
    tmux kill-window -t "$wid" 2>/dev/null || true
    rm -f "$rcfile"
    return 124
  fi
  [[ -s "$rcfile" ]] && rc="$(<"$rcfile")"
  rm -f "$rcfile"
  [[ "$rc" -ne 0 || "$KEEP_WINDOW" -eq 1 ]] && tmux set-window-option -t "$wid" remain-on-exit on 2>/dev/null || true
  return "$rc"
}

USE_TMUX=0
if [[ "$FORCE_HEADLESS" -eq 0 ]] && command -v tmux >/dev/null 2>&1; then
  if [[ -z "$SESSION" && -n "${TMUX:-}" ]]; then
    SESSION="$(tmux display-message -p '#{session_name}' 2>/dev/null || true)"
  fi
  [[ -n "$SESSION" ]] && USE_TMUX=1
fi

RC=0
if [[ "$USE_TMUX" -eq 1 ]]; then
  run_tmux "$SESSION" || RC=$?
else
  [[ "$FORCE_HEADLESS" -eq 0 && -z "${TMUX:-}" ]] && echo "[$AGENT] not in tmux; using subprocess mode" >&2
  run_subprocess || RC=$?
fi

if [[ ! -s "$REPLY_FILE" ]]; then
  echo "[$AGENT] reply empty (rc=$RC)" >&2
  [[ "$RC" -ne 0 ]] && exit "$RC"
  exit 1
fi

BYTES="$(wc -c < "$REPLY_FILE" | tr -d ' ')"
if [[ "$RC" -ne 0 ]]; then
  echo "[$AGENT] failed: $REPLY_FILE ($BYTES bytes, rc=$RC)" >&2
  exit "$RC"
fi
echo "[$AGENT] ok: $REPLY_FILE ($BYTES bytes, rc=$RC)" >&2
