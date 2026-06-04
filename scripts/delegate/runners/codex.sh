#!/usr/bin/env bash
# Interface: codex.sh <prompt_file> <reply_file> <timeout_sec>
#
# Non-interactive Codex runner for LTO/ad-style delegation.
# Defaults to read-only; set CODEX_SANDBOX=workspace-write only for tasks
# explicitly allowed to edit files. Avoid danger-full-access unless the outer
# environment is already sandboxed and the user explicitly approved it.
set -uo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: codex.sh <prompt_file> <reply_file> [timeout_sec]" >&2
  exit 2
fi

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"
CODEX_BIN="${CODEX_BIN:-codex}"
CODEX_WORKDIR="${CODEX_WORKDIR:-$PWD}"
CODEX_SANDBOX="${CODEX_SANDBOX:-read-only}"

if [[ ! -f "$PROMPT_FILE" ]]; then
  echo "codex runner: prompt file not found: $PROMPT_FILE" >&2
  exit 2
fi

if ! command -v "$CODEX_BIN" >/dev/null 2>&1; then
  echo "codex runner: Codex CLI not found ($CODEX_BIN)" >&2
  exit 127
fi

# Codex CLI flags change over time; probe exec help before relying on -C/-s/-o.
if ! "$CODEX_BIN" exec --help >/dev/null 2>&1; then
  echo "codex runner: 'codex exec --help' failed; CLI unavailable or unauthenticated" >&2
  exit 127
fi

OUT_FILE="$(mktemp)"
ERR_FILE="$(mktemp)"
cleanup() {
  rm -f "$OUT_FILE" "$ERR_FILE"
}
trap cleanup EXIT

args=(exec --skip-git-repo-check -C "$CODEX_WORKDIR" -s "$CODEX_SANDBOX")

if [[ -n "${CODEX_MODEL:-}" ]]; then
  args+=(-m "$CODEX_MODEL")
fi
if [[ -n "${CODEX_PROFILE:-}" ]]; then
  args+=(-p "$CODEX_PROFILE")
fi
if [[ "${CODEX_JSON:-0}" == "1" ]]; then
  args+=(--json)
fi

# Image input is rarely used by LTO, but Codex requires explicit attachment.
# Accept comma-separated CODEX_IMAGES="a.png,b.png" or colon-separated paths.
if [[ -n "${CODEX_IMAGES:-}" ]]; then
  IFS=',:' read -r -a image_paths <<< "$CODEX_IMAGES"
  for img in "${image_paths[@]}"; do
    [[ -n "$img" ]] && args+=(-i "$img")
  done
fi

# Long prompts go through stdin ('-') to avoid shell argv/quoting limits.
# -o writes the final assistant message to the reply file; stdout is retained
# only as fallback for older/odd Codex behavior.
args+=(-o "$REPLY_FILE" -)

set +e
timeout "${TIMEOUT_SEC}s" "$CODEX_BIN" "${args[@]}" < "$PROMPT_FILE" > "$OUT_FILE" 2> "$ERR_FILE"
rc=$?
set -e 2>/dev/null || true

# Fallback: if -o produced no final message but stdout has content, keep stdout
# so scheduler can distinguish empty-output bugs from real Codex replies.
if [[ ! -s "$REPLY_FILE" && -s "$OUT_FILE" ]]; then
  cp "$OUT_FILE" "$REPLY_FILE"
fi

if [[ -s "$ERR_FILE" ]]; then
  cat "$ERR_FILE" >&2
fi

exit "$rc"
