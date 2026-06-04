#!/usr/bin/env bash
# Interface: gemini.sh <prompt_file> <reply_file> <timeout_sec>
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" gemini -p "$(cat "$PROMPT_FILE")" > "$REPLY_FILE"
