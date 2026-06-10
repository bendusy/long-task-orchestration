#!/usr/bin/env bash
# Interface: agy.sh <prompt_file> <reply_file> <timeout_sec>
#
# read-only contract (references/runner-readonly-contract.md §7):
#   agy `--sandbox` 实测兑现的是 workspace-write（工作区内可写、仅封工作区外），
#   NOT read-only。agy 无 read-only 档。所以：
#     - LTO_PERM_SANDBOX=read-only         → 不该到这（scheduler/validate 已拒 agy
#                                            承接 read-only）；防御性 fail-closed 退出。
#     - LTO_PERM_SANDBOX=workspace-write    → `--sandbox`（恰好兑现 workspace-write）
#     - LTO_PERM_SANDBOX=danger-full-access → `--dangerously-skip-permissions`
#   缺省（无 env）维持历史 full-access 行为以不破坏 ad/triad 裸调用。
#
# NOTE: no token sidecar. The agy CLI's --print mode emits only reply text.
set -uo pipefail

PROMPT_FILE="$1"
REPLY_FILE="$2"
TIMEOUT_SEC="${3:-900}"

SANDBOX="${LTO_PERM_SANDBOX:-danger-full-access}"
JOB_ID="${LTO_JOB_ID:-}"

# Select the permission flag from the job-level sandbox intent.
case "$SANDBOX" in
  read-only)
    # agy cannot enforce read-only — must not run a read-only job.
    echo "agy.sh: refusing read-only job (agy --sandbox is workspace-write, not read-only)" >&2
    exit 64
    ;;
  workspace-write)
    PERM_FLAG="--sandbox"
    ;;
  danger-full-access)
    PERM_FLAG="--dangerously-skip-permissions"
    ;;
  *)
    echo "agy.sh: unknown LTO_PERM_SANDBOX=$SANDBOX" >&2
    exit 64
    ;;
esac

# stdout via tee: written to REPLY_FILE AND streamed for live log.
# PIPESTATUS[0] keeps agy's real rc (not tee's).
set +o pipefail
timeout "${TIMEOUT_SEC}s" agy \
  "$PERM_FLAG" \
  --print-timeout "${TIMEOUT_SEC}s" \
  -p "$(cat "$PROMPT_FILE")" | tee "$REPLY_FILE"
rc=${PIPESTATUS[0]}
set -o pipefail

# perm sidecar (RC3: job_id 绑定 + 原子 rename)。仅回传 scheduler 构造侧看不到的
# 运行时事实（runner 实际接受的 flag）。scheduler 仍以自己构造的 argv 为权威。
if [[ -n "$JOB_ID" ]]; then
  PERM_FILE="$REPLY_FILE.perm.json"
  PERM_TMP="$(mktemp "${PERM_FILE}.XXXXXX")"
  printf '{"job_id":"%s","runner":"agy","readonly_mechanism":"sandbox-flag","enforced_flag":"%s","sandbox":"%s"}\n' \
    "$JOB_ID" "$PERM_FLAG" "$SANDBOX" > "$PERM_TMP"
  mv -f "$PERM_TMP" "$PERM_FILE"
fi

exit "$rc"
