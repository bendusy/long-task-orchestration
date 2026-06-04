#!/usr/bin/env bash
# Interface: claude.sh <prompt_file> <reply_file> <timeout_sec>
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" claude -p \
  --bare --dangerously-skip-permissions \
  "$(cat "$PROMPT_FILE")" > "$REPLY_FILE"
