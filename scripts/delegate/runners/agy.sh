#!/usr/bin/env bash
# Interface: agy.sh <prompt_file> <reply_file> <timeout_sec>
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" agy \
  --dangerously-skip-permissions \
  --print-timeout "${TIMEOUT_SEC}s" \
  -p "$(cat "$PROMPT_FILE")" > "$REPLY_FILE"
