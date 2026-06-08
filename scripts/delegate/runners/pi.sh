#!/usr/bin/env bash
# Interface: pi.sh <prompt_file> <reply_file> <timeout_sec>
#
# Runs pi in --mode json so we can extract BOTH the reply text and a token
# usage sidecar (<reply_file>.meta.json) from one call. Reply text is the
# concatenation of `text` blocks in the final assistant message_end event;
# token usage comes from that event's usage{input,output,totalTokens}.
# If json parsing yields no reply, falls back to raw stdout so ad/triad keep
# working even if pi's json schema shifts.
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

RAW_FILE="$(mktemp)"
cleanup() { rm -f "$RAW_FILE" "$RAW_FILE.parsed"; }
trap cleanup EXIT

# pipefail off for this call so a downstream tool in a pipe can't mask pi's rc.
set +o pipefail
timeout "${TIMEOUT_SEC}s" pi -p --mode json \
  --provider deepseek --model deepseek-v4-pro \
  "$(cat "$PROMPT_FILE")" > "$RAW_FILE" 2>/dev/null
rc=$?
set -o pipefail

# Parse NDJSON: reply = text blocks of the LAST assistant message_end; sidecar =
# that same event's usage. Only `message_end` is matched (not `turn_end`) so
# reply and usage always come from one coherent event — turn_end can repeat
# usage with missing input/output and would otherwise clobber good data (B2).
# A PARSED sentinel file tells the bash side "json parsing ran and is
# authoritative", so an empty/whitespace reply does NOT fall back to raw
# NDJSON (which would pollute downstream — B1).
PARSED_FLAG="$RAW_FILE.parsed"
if command -v python3 >/dev/null 2>&1; then
  python3 - "$RAW_FILE" "$REPLY_FILE" "$REPLY_FILE.meta.json" "$PARSED_FLAG" <<'PYPI' 2>/dev/null || true
import json, sys
raw, reply_file, meta_file, parsed_flag = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
reply, usage, saw_assistant = None, None, False
try:
    with open(raw, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except Exception:
                continue
            if not isinstance(ev, dict):
                continue
            msg = ev.get("message")
            if ev.get("type") == "message_end" and isinstance(msg, dict) \
               and msg.get("role") == "assistant":
                saw_assistant = True
                content = msg.get("content")
                texts = []
                if isinstance(content, list):
                    texts = [b.get("text", "") for b in content
                             if isinstance(b, dict) and b.get("type") == "text"]
                reply = "".join(texts).strip()  # keep last assistant message
                u = msg.get("usage")
                usage = u if isinstance(u, dict) else None  # paired with this reply
except Exception:
    sys.exit(0)
# Sentinel: json parsing recognized at least one assistant message → its reply
# (even if empty) is authoritative; bash must NOT fall back to raw NDJSON.
if saw_assistant:
    open(parsed_flag, "w").close()
    with open(reply_file, "w", encoding="utf-8") as fh:
        fh.write(reply or "")
if isinstance(usage, dict):
    ti, to, tt = usage.get("input"), usage.get("output"), usage.get("totalTokens")
    meta = {}
    if isinstance(ti, int) and ti >= 0:
        meta["tokens_in"] = ti
    if isinstance(to, int) and to >= 0:
        meta["tokens_out"] = to
    # pi's totalTokens includes cache/reasoning; prefer it for the rollup.
    if isinstance(tt, int) and tt >= 0:
        meta["tokens"] = tt
    elif "tokens_in" in meta and "tokens_out" in meta:
        meta["tokens"] = meta["tokens_in"] + meta["tokens_out"]
    if meta:
        with open(meta_file, "w", encoding="utf-8") as fh:
            json.dump(meta, fh)
PYPI
fi

# raw fallback: only when json parsing did NOT recognize an assistant message
# (no PARSED sentinel). If parsing ran and the assistant reply was genuinely
# empty/whitespace, we keep that empty reply rather than polluting downstream
# with raw NDJSON (B1). Sentinel absent → schema mismatch / no python3 → raw.
if [[ ! -f "$PARSED_FLAG" && ! -s "$REPLY_FILE" && -s "$RAW_FILE" ]]; then
  cp "$RAW_FILE" "$REPLY_FILE"
fi

exit "$rc"
