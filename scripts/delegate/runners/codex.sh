#!/usr/bin/env bash
# Interface: codex.sh <prompt_file> <reply_file> <timeout_sec>
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" codex exec \
  --skip-git-repo-check \
  "$(cat "$PROMPT_FILE")" 2>/dev/null > "$REPLY_FILE"
