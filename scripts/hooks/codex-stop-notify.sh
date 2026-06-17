#!/usr/bin/env bash
set +e

payload="${TMPDIR:-/tmp}/lto-codex-stop-$$.json"
cat > "$payload" 2>/dev/null || true

lto_bin="${LTO_BIN:-lto}"
repo="${LTO_REPO:-.}"

args=(--repo "$repo" agent-turn-completed --runner codex --payload-file "$payload" --source codex-stop-hook)
if [ -n "${LTO_RUN_ID:-}" ]; then
  args+=(--run-id "$LTO_RUN_ID")
fi

"$lto_bin" "${args[@]}" >/dev/null 2>&1 || true

rm -f "$payload" 2>/dev/null || true
exit 0
