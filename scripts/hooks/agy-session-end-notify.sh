#!/usr/bin/env bash
# LTO agy completion hook — mechanical completion signal for
# `dispatch-goal --runner agy` running in a real tmux TUI (agy is Gemini-CLI
# based; its SessionEnd hook fires when the agy session ends). On fire this
# calls `lto agent-turn-completed`, which writes the event, wakes any
# `lto events --wait` waiter, and (with --bell) rings the tmux bell as a
# human-visible fallback if the wake path is missed.
#
# Best-effort: never fail the agy session. Any error exits 0.
#
# Environment (LTO_BIN/LTO_RUN_ID injected by the dispatcher's hook command;
# LTO_REPO_FALLBACK is the repo the dispatch installed the hook for):
set +e

lto_bin="${LTO_BIN:-lto}"
repo="${LTO_REPO_FALLBACK:-${LTO_REPO:-.}}"

args=(--repo "$repo" agent-turn-completed --runner agy --source agy-session-end-hook --bell)
if [ -n "${LTO_RUN_ID:-}" ]; then
  args+=(--run-id "$LTO_RUN_ID")
fi

"$lto_bin" "${args[@]}" >/dev/null 2>&1 || true
exit 0
