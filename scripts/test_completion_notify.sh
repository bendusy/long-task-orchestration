#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LTO_BIN="${LTO_BIN:-$ROOT/target/debug/lto-rs}"

if ! command -v tmux >/dev/null 2>&1; then
  echo "tmux is required for completion notification e2e" >&2
  exit 2
fi

if [[ ! -x "$LTO_BIN" ]]; then
  cargo build --quiet --manifest-path "$ROOT/Cargo.toml" --bin lto-rs
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/lto-completion-notify.XXXXXX")"
SOCKET="lto-completion-notify-$$"
SESSION="completion-notify"
REPO="$TMP/repo"
TMUX_BIN="$TMP/tmux-isolated"
WAITER_PID=""

cleanup() {
  if [[ -n "$WAITER_PID" ]]; then
    kill "$WAITER_PID" 2>/dev/null || true
    wait "$WAITER_PID" 2>/dev/null || true
  fi
  "$TMUX_BIN" kill-server >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

mkdir -p "$REPO" "$TMP/bin"

cat > "$TMUX_BIN" <<EOF
#!/usr/bin/env bash
exec tmux -L "$SOCKET" "\$@"
EOF
chmod +x "$TMUX_BIN"

cat > "$TMP/bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'gpt-fake ready\n'
IFS= read -r _goal
printf 'Working\n'
"$LTO_TEST_BIN" --repo "$LTO_TEST_REPO" agent-turn-completed \
  --run-id "${LTO_RUN_ID:?}" \
  --runner codex \
  --summary "fake goal done" \
  --source codex-process-exit \
  --rc 0 \
  --bell
EOF
chmod +x "$TMP/bin/codex"

cat > "$TMP/capture-lto" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "$LTO_CAPTURE_FILE"
EOF
chmod +x "$TMP/capture-lto"

printf '# minimal completion goal\n' > "$REPO/goal.md"
"$LTO_BIN" --repo "$REPO" start \
  --run-id notify-e2e \
  --goal "completion notification e2e" \
  --target "notify sentinel and waiter" \
  --constraint "isolated tmux socket" \
  --instrument "scripts/test_completion_notify.sh" \
  --entropy-check "fail on missing signal" >/dev/null

export PATH="$TMP/bin:$PATH"
export LTO_TEST_BIN="$LTO_BIN"
export LTO_TEST_REPO="$REPO"
TARGET="$("$TMUX_BIN" new-session -d -P -F '#{session_name}:#{window_index}.#{pane_index}' -s "$SESSION")"
"$TMUX_BIN" send-keys -l -t "$TARGET" "export PATH='$TMP/bin':\$PATH"
"$TMUX_BIN" send-keys -t "$TARGET" Enter

"$LTO_BIN" --repo "$REPO" events \
  --wait \
  --event-type agent.dispatch.completed \
  --run-id notify-e2e \
  --timeout 10 > "$TMP/wait.out" &
WAITER_PID=$!

for _ in {1..50}; do
  if [[ -s "$REPO/.lto/notify-e2e/notify-endpoints.json" ]]; then
    break
  fi
  sleep 0.02
done
if [[ ! -s "$REPO/.lto/notify-e2e/notify-endpoints.json" ]]; then
  echo "events waiter did not register" >&2
  exit 1
fi

NOTIFIED="$TMP/notified.txt"
NOTIFY_CMD="printf '%s' \"\$LTO_SUMMARY\" > '$NOTIFIED'"
"$LTO_BIN" --repo "$REPO" dispatch-goal \
  --run-id notify-e2e \
  --runner codex \
  --goal "$REPO/goal.md" \
  --target "$TARGET" \
  --tmux-bin "$TMUX_BIN" \
  --ready-timeout 10 \
  --notify-cmd "$NOTIFY_CMD" > "$TMP/dispatch.out"

wait "$WAITER_PID"
WAITER_PID=""

grep -F "fake goal done" "$TMP/wait.out" >/dev/null
grep -F "wait_command=lto events --wait --event-type agent.dispatch.completed --run-id notify-e2e --timeout 600" "$TMP/dispatch.out" >/dev/null
grep -F "dispatch_and_wait=lto dispatch-and-wait" "$TMP/dispatch.out" >/dev/null
test "$(cat "$NOTIFIED")" = "fake goal done"

printf '{"cwd":"%s"}\n' "$REPO" |
  LTO_BIN="$TMP/capture-lto" \
  LTO_CAPTURE_FILE="$TMP/hook-args.txt" \
  bash "$ROOT/scripts/hooks/codex-stop-notify.sh"
grep -Fx -- "--bell" "$TMP/hook-args.txt" >/dev/null
if grep -Fx -- "--rc" "$TMP/hook-args.txt" >/dev/null; then
  echo "codex Stop hook must not fabricate rc" >&2
  exit 1
fi

grep -F '"type":"agent.dispatch.completed"' "$REPO/.lto/notify-e2e/events.jsonl" >/dev/null
printf 'completion notification e2e: PASS\n'
