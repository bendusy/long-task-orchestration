#!/usr/bin/env bash
set +e

payload="${TMPDIR:-/tmp}/lto-codex-stop-$$.json"
cat > "$payload" 2>/dev/null || true

lto_bin="${LTO_BIN:-lto}"
repo="${LTO_REPO:-.}"
if command -v python3 >/dev/null 2>&1; then
  detected_repo="$(python3 - "$payload" <<'PY' 2>/dev/null
import json
import pathlib
import sys

try:
    data = json.loads(pathlib.Path(sys.argv[1]).read_text())
except Exception:
    sys.exit(0)

cwd = data.get("cwd") or data.get("workspace") or data.get("repo") or data.get("repo_root")
if not isinstance(cwd, str) or not cwd:
    sys.exit(0)

path = pathlib.Path(cwd).expanduser()
if not path.is_absolute():
    path = pathlib.Path.cwd() / path
for candidate in [path, *path.parents]:
    if (candidate / ".lto").is_dir():
        print(candidate)
        break
PY
)"
  if [ -n "$detected_repo" ]; then
    repo="$detected_repo"
  fi
fi

args=(--repo "$repo" agent-turn-completed --runner codex --payload-file "$payload" --source codex-stop-hook --bell)
if [ -n "${LTO_RUN_ID:-}" ]; then
  args+=(--run-id "$LTO_RUN_ID")
fi
if [ -n "${LTO_WINDOW_ID:-}" ]; then
  args+=(--window-id "$LTO_WINDOW_ID")
fi

"$lto_bin" "${args[@]}" >/dev/null 2>&1 || true

rm -f "$payload" 2>/dev/null || true
exit 0
