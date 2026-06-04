#!/usr/bin/env bash
# Interface: pi.sh <prompt_file> <reply_file> <timeout_sec>
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" pi -p \
  --provider deepseek --model deepseek-v4-pro \
  "$(cat "$PROMPT_FILE")" > "$REPLY_FILE"
