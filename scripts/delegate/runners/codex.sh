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
# RH1: CODEX_SANDBOX 由 scheduler 在 spawn 时以隔离 env 显式注入（_effective_env），
# 不从用户 shell/父进程继承——与「不走继承 env」统一为「scheduler 构造侧注入」。
CODEX_SANDBOX="${CODEX_SANDBOX:-read-only}"
case "$CODEX_SANDBOX" in
  read-only|workspace-write|danger-full-access) ;;
  *)
    # Codex host sessions may export implementation-specific sandbox names
    # such as "seatbelt". Those are not valid `codex exec -s` values; fallback
    # keeps standalone healthcheck/manual runner calls read-only instead of
    # failing before auth/network can be probed.
    CODEX_SANDBOX="read-only"
    ;;
esac
JOB_ID="${LTO_JOB_ID:-}"

if [[ ! -f "$PROMPT_FILE" ]]; then
  echo "codex runner: prompt file not found: $PROMPT_FILE" >&2
  exit 2
fi

if ! command -v "$CODEX_BIN" >/dev/null 2>&1; then
  echo "codex runner: Codex CLI not found ($CODEX_BIN)" >&2
  exit 127
fi

# Codex CLI flags change over time; probe exec help before relying on -C/-s/-o.
# Bounded by its own 10s timeout: this probe runs BEFORE the main `timeout
# ${TIMEOUT_SEC}s` guard, so without it a hung `codex exec --help` (e.g. auth
# prompt waiting on stdin in an odd env) would only be caught by the scheduler's
# outer subprocess timeout. timeout exit 124 still trips the `!` failure branch.
if ! timeout 10s "$CODEX_BIN" exec --help </dev/null >/dev/null 2>&1; then
  echo "codex runner: 'codex exec --help' failed or timed out; CLI unavailable or unauthenticated" >&2
  exit 127
fi

OUT_FILE="$(mktemp)"
ERR_FILE="$(mktemp)"
cleanup() {
  rm -f "$OUT_FILE" "$ERR_FILE"
}
trap cleanup EXIT

# Lean context (backlog ⑪): LTO sets LTO_LEAN_CONTEXT=1 for one-shot review jobs.
# codex has NO safe context-only flag: --ignore-user-config also drops the user's
# API-key/auth config (probed → 401 Unauthorized), and --ignore-rules only skips
# AGENTS.md, not the bulk of the cold load. So codex gracefully degrades: it
# ignores LTO_LEAN_CONTEXT and runs normally (still heterogeneous, still works,
# just no token saving). Revisit if codex adds a context-only flag.
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
# stdout 过 tee：既存进 OUT_FILE（供 reply/token 解析），又透传到本进程 stdout，
# 让 LTO scheduler 的 Popen 能流式捕获进 live log（可观测）。PIPESTATUS[0]
# 取 codex 的真实 rc，不被 tee 的退出码掩盖。
timeout "${TIMEOUT_SEC}s" "$CODEX_BIN" "${args[@]}" < "$PROMPT_FILE" 2> "$ERR_FILE" | tee "$OUT_FILE"
rc=${PIPESTATUS[0]}
set -e 2>/dev/null || true

# Fallback: if -o produced no final message but stdout has content, keep stdout
# so scheduler can distinguish empty-output bugs from real Codex replies.
if [[ ! -s "$REPLY_FILE" && -s "$OUT_FILE" ]]; then
  cp "$OUT_FILE" "$REPLY_FILE"
fi

# Token sidecar (best-effort): when --json is on, parse the last turn.completed
# usage from stdout and write <reply>.meta.json. Scheduler reads it optionally;
# absence/failure is non-fatal and never affects rc. Needs python3 + CODEX_JSON=1.
if [[ "${CODEX_JSON:-0}" == "1" ]] && command -v python3 >/dev/null 2>&1; then
  python3 - "$OUT_FILE" "$REPLY_FILE.meta.json" <<'PYMETA' 2>/dev/null || true
import json, sys
out_file, meta_file = sys.argv[1], sys.argv[2]
# codex exec 非交互模式实测为单 turn（1 个 turn.completed），usage 即该次完整值。
# 为防御未来多 turn（若 codex 改成增量 usage），这里累加所有 turn 的 in/out。
ti_sum = to_sum = 0
seen = False
try:
    with open(out_file, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except Exception:
                continue
            u = ev.get("usage") if isinstance(ev, dict) else None
            if not isinstance(u, dict):
                continue
            i, o = u.get("input_tokens"), u.get("output_tokens")
            if isinstance(i, int) and i >= 0:
                ti_sum += i; seen = True
            if isinstance(o, int) and o >= 0:
                to_sum += o; seen = True
except Exception:
    sys.exit(0)
if not seen:
    sys.exit(0)
ti = ti_sum
to = to_sum
meta = {}
if isinstance(ti, int) and ti >= 0:
    meta["tokens_in"] = ti
if isinstance(to, int) and to >= 0:
    meta["tokens_out"] = to
if "tokens_in" in meta and "tokens_out" in meta:
    meta["tokens"] = meta["tokens_in"] + meta["tokens_out"]
if meta:
    with open(meta_file, "w", encoding="utf-8") as fh:
        json.dump(meta, fh)
PYMETA
fi

if [[ -s "$ERR_FILE" ]]; then
  cat "$ERR_FILE" >&2
fi

# perm sidecar (RC3: job_id 绑定 + 原子 rename)。codex 用 sandbox-rank 机制。
if [[ -n "$JOB_ID" ]]; then
  PERM_FILE="$REPLY_FILE.perm.json"
  PERM_TMP="$(mktemp "${PERM_FILE}.XXXXXX")"
  printf '{"job_id":"%s","runner":"codex","readonly_mechanism":"sandbox-rank","enforced_argv":"-s %s","sandbox":"%s"}\n' \
    "$JOB_ID" "$CODEX_SANDBOX" "$CODEX_SANDBOX" > "$PERM_TMP"
  mv -f "$PERM_TMP" "$PERM_FILE"
fi

exit "$rc"
