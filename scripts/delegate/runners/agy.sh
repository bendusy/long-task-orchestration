#!/usr/bin/env bash
# Interface: agy.sh <prompt_file> <reply_file> <timeout_sec>
#
# NOTE: no token sidecar. The agy (Antigravity) CLI's --print mode emits only
# the reply text — no usage/token output, no --json/--output-format flag, and
# --log-file carries OAuth/auth-token errors, not usage tokens. So eval-run's
# token_metering_available is False for agy by design (CLI limitation), and the
# scheduler's optional sidecar reader simply finds no <reply>.meta.json.
# Revisit if a future agy release exposes per-call usage.
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

timeout "${TIMEOUT_SEC}s" agy \
  --dangerously-skip-permissions \
  --print-timeout "${TIMEOUT_SEC}s" \
  -p "$(cat "$PROMPT_FILE")" > "$REPLY_FILE"
