#!/usr/bin/env bash
# Interface: claude.sh <prompt_file> <reply_file> <timeout_sec>
#
# Runs `claude -p --output-format json` so we get BOTH the reply text and a
# token usage sidecar (<reply_file>.meta.json) from one call. Output is a single
# JSON object: `result` is the reply text, `usage` carries input/output and
# cache token counts. If json parsing recognizes the result, that reply is
# authoritative (a PARSED sentinel blocks raw fallback); otherwise we fall back
# to raw stdout so ad/triad keep working if claude's json schema shifts.
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

RAW_FILE="$(mktemp)"
PARSED_FLAG="$RAW_FILE.parsed"
cleanup() { rm -f "$RAW_FILE" "$PARSED_FLAG"; }
trap cleanup EXIT

set +o pipefail
timeout "${TIMEOUT_SEC}s" claude -p --output-format json \
  --dangerously-skip-permissions \
  "$(cat "$PROMPT_FILE")" > "$RAW_FILE" 2>/dev/null
rc=$?
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

exit "$rc"
