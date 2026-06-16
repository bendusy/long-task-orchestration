#!/usr/bin/env bash
# Interface: claude.sh <prompt_file> <reply_file> <timeout_sec>
#
# Runs `claude -p --output-format json` so we get BOTH the reply text and a
# token usage sidecar (<reply_file>.meta.json) from one call. Output is a single
# JSON object: `result` is the reply text, `usage` carries input/output and
# cache token counts. If json parsing recognizes the result, that reply is
# authoritative (a PARSED sentinel blocks raw fallback); otherwise we fall back
# to raw stdout so ad/triad keep working if claude's json schema shifts.
#
# read-only contract (references/runner-readonly-contract.md §7):
#   实测：claude `--allowedTools` 不硬裁工具集（Write/Bash 仍在工具列表），真正拦写
#   的是 `--permission-mode plan` 的软约束。所以 read-only 时两者必须同传：
#     LTO_PERM_SANDBOX=read-only → --allowedTools <LTO_PERM_TOOLS> --permission-mode plan
#   非 read-only（或缺省裸调用）维持历史 --dangerously-skip-permissions 行为。
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

RAW_FILE="$(mktemp)"
PARSED_FLAG="$RAW_FILE.parsed"
cleanup() { rm -f "$RAW_FILE" "$PARSED_FLAG"; }
trap cleanup EXIT

SANDBOX="${LTO_PERM_SANDBOX:-danger-full-access}"
JOB_ID="${LTO_JOB_ID:-}"
PERM_TOOLS="${LTO_PERM_TOOLS:-}"

# Build the permission argv from the job-level sandbox intent.
PERM_ARGV=()
PERM_MECH="skip-permissions"
if [[ "$SANDBOX" == "read-only" ]]; then
  # plan mode is the real enforcer; allowedTools narrows the declared set.
  PERM_ARGV=(--allowedTools "${PERM_TOOLS:-Read,Grep,Glob,WebFetch}" --permission-mode plan)
  PERM_MECH="tool-allowlist+plan"
else
  PERM_ARGV=(--dangerously-skip-permissions)
fi

# Lean context (backlog ⑪): LTO sets LTO_LEAN_CONTEXT=1 for one-shot review jobs
# (audit/judge). claude cold-loads ~19k tokens of settings/skills/memory/hooks by
# default; --setting-sources '' drops user/project/local settings (skills, memory,
# CLAUDE.md, hooks) → ~2.5k tokens (~7.5x). read-only is enforced by --permission-mode
# plan + --allowedTools (above), NOT by settings, so dropping settings is safe here.
LEAN_ARGV=()
if [[ "${LTO_LEAN_CONTEXT:-0}" == "1" ]]; then
  LEAN_ARGV=(--setting-sources "")
fi

# stdout via tee: stored in RAW_FILE (for reply/token parse) AND streamed to
# this process's stdout so LTO scheduler's Popen captures it into the live log.
# PIPESTATUS[0] keeps claude's real rc (not tee's).
set +o pipefail
timeout "${TIMEOUT_SEC}s" claude -p --output-format json \
  ${PERM_ARGV[@]+"${PERM_ARGV[@]}"} \
  ${LEAN_ARGV[@]+"${LEAN_ARGV[@]}"} \
  "$(cat "$PROMPT_FILE")" 2>/dev/null | tee "$RAW_FILE"
rc=${PIPESTATUS[0]}
set -o pipefail

if command -v python3 >/dev/null 2>&1; then
  python3 - "$RAW_FILE" "$REPLY_FILE" "$REPLY_FILE.meta.json" "$PARSED_FLAG" <<'PYCLA' 2>/dev/null || true
import json, sys
raw, reply_file, meta_file, parsed_flag = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
try:
    data = json.loads(open(raw, encoding="utf-8").read())
except Exception:
    sys.exit(0)
if not isinstance(data, dict) or "result" not in data:
    sys.exit(0)  # not the expected envelope → leave for raw fallback
# result is authoritative once recognized (even if empty) → block raw fallback.
open(parsed_flag, "w").close()
result = data.get("result")
with open(reply_file, "w", encoding="utf-8") as fh:
    fh.write(result if isinstance(result, str) else "")
u = data.get("usage")
if isinstance(u, dict):
    ti = u.get("input_tokens")
    to = u.get("output_tokens")
    cc = u.get("cache_creation_input_tokens") or 0
    cr = u.get("cache_read_input_tokens") or 0
    meta = {}
    if isinstance(ti, int) and ti >= 0:
        meta["tokens_in"] = ti
    if isinstance(to, int) and to >= 0:
        meta["tokens_out"] = to
    # Roll up everything claude actually consumed (prompt + output + cache),
    # mirroring pi's totalTokens-includes-cache convention.
    parts = [v for v in (ti, to, cc, cr) if isinstance(v, int) and v >= 0]
    if parts:
        meta["tokens"] = sum(parts)
    if meta:
        with open(meta_file, "w", encoding="utf-8") as fh:
            json.dump(meta, fh)
PYCLA
fi

# raw fallback only when json parsing did NOT recognize the result envelope.
if [[ ! -f "$PARSED_FLAG" && ! -s "$REPLY_FILE" && -s "$RAW_FILE" ]]; then
  cp "$RAW_FILE" "$REPLY_FILE"
fi

# perm sidecar (RC3: job_id 绑定 + 原子 rename)。
if [[ -n "$JOB_ID" ]]; then
  PERM_FILE="$REPLY_FILE.perm.json"
  PERM_TMP="$(mktemp "${PERM_FILE}.XXXXXX")"
  printf '{"job_id":"%s","runner":"claude","readonly_mechanism":"%s","enforced_argv":"%s","sandbox":"%s","tools":"%s"}\n' \
    "$JOB_ID" "$PERM_MECH" "${PERM_ARGV[*]}" "$SANDBOX" "${PERM_TOOLS}" > "$PERM_TMP"
  mv -f "$PERM_TMP" "$PERM_FILE"
fi

exit "$rc"
