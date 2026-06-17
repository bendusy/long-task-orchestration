#!/usr/bin/env bash
# release_preflight.sh — 发版前确定性检查闸门。
#
# 把"每次发版踩的坑"固化成自动检查,host 发版前一键跑,全绿才发。
# 覆盖 CI 的全部检查 + 发版特有检查(版本三处一致 / 隐私扫描 / tag 安全)。
#
# 用法:
#   bash scripts/release_preflight.sh              # 检查当前状态(发版前自检)
#   bash scripts/release_preflight.sh --version X.Y.Z   # 额外校验目标版本号在三处一致
#
# 退出码: 0=全绿可发 / 1=有阻断项,别发
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TARGET_VERSION=""
[[ "${1:-}" == "--version" ]] && TARGET_VERSION="${2:-}"

FAIL=0
pass() { printf '  \033[32mOK\033[0m   %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=1; }
sec()  { printf '\n== %s ==\n' "$1"; }

# ---- 1. 版本三处一致(本次踩的坑:漏改 VERSION)----
sec "版本一致性(Cargo.toml / VERSION / Cargo.lock)"
CARGO_VER=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
FILE_VER=$(tr -d '[:space:]' < VERSION 2>/dev/null)
LOCK_VER=$(grep -A1 'name = "lto-rs"' Cargo.lock 2>/dev/null | grep version | sed 's/.*"\(.*\)"/\1/')
echo "  Cargo.toml=$CARGO_VER  VERSION=$FILE_VER  Cargo.lock=$LOCK_VER"
if [[ "$CARGO_VER" == "$FILE_VER" && "$CARGO_VER" == "$LOCK_VER" ]]; then
  pass "三处版本一致 ($CARGO_VER)"
else
  fail "版本不一致 — Cargo.toml/VERSION/Cargo.lock 必须相同(CI check_docs_consistency 会挂)"
fi
if [[ -n "$TARGET_VERSION" ]]; then
  [[ "$CARGO_VER" == "$TARGET_VERSION" ]] && pass "匹配目标版本 $TARGET_VERSION" || fail "当前 $CARGO_VER ≠ 目标 $TARGET_VERSION"
fi

# ---- 2. 隐私/命名族扫描(本次踩的坑:GOAL spec 带 yh 字样)----
sec "隐私扫描(开源 repo 零私有领域痕迹)"
# 排除本脚本自身(它的扫描正则含这些词,自指会误报)
PRIV_RE='yihub|办文|呈批|gov-doc|chengpi|\byh\b'
HITS=$(git grep -iEln "$PRIV_RE" -- . ':!scripts/release_preflight.sh' 2>/dev/null | wc -l | tr -d ' ')
[[ "$HITS" == "0" ]] && pass "yh 命名族零命中" || { fail "yh 命名族命中 $HITS 文件:"; git grep -iEln "$PRIV_RE" -- . ':!scripts/release_preflight.sh' 2>/dev/null | sed 's/^/      /'; }

# ---- 3. 凭据扫描 ----
sec "凭据扫描"
CRED=$(git grep -iEn 'api[_-]?key.*=.*["\x27][A-Za-z0-9]{20}|password.*=.*["\x27][^"\x27]{8}' -- . 2>/dev/null | grep -viE 'test|example|redact|placeholder|//' | wc -l | tr -d ' ')
[[ "$CRED" == "0" ]] && pass "无硬编码凭据" || fail "疑似硬编码凭据 $CRED 处"

# ---- 4. CI 全部确定性检查(本地复现,别只跑 cargo test)----
sec "红线(本地复现 CI 全部检查)"
run() { local name="$1"; shift; if "$@" >/tmp/preflight_$$.log 2>&1; then pass "$name"; else fail "$name"; tail -5 /tmp/preflight_$$.log | sed 's/^/      /'; fi; }
run "cargo fmt --check"       cargo fmt --all --check
run "cargo clippy -D warnings" cargo clippy --locked --all-targets -- -D warnings
run "cargo test --locked"     cargo test --locked
run "check_docs_consistency"  python3 scripts/check_docs_consistency.py
run "check_python_rust_ownership" python3 scripts/check_python_rust_ownership.py
rm -f /tmp/preflight_$$.log

# ---- 5. self-test ----
sec "二进制 self-test"
if cargo run --quiet --release -- self-test >/tmp/preflight_st_$$.log 2>&1 && grep -q "SELFTEST OK" /tmp/preflight_st_$$.log; then
  pass "self-test OK"
else fail "self-test 未过"; fi
rm -f /tmp/preflight_st_$$.log

# ---- 6. 工作树 + 分支安全 ----
sec "git 状态"
DIRTY=$(git status --short | wc -l | tr -d ' ')
[[ "$DIRTY" == "0" ]] && pass "工作树干净" || { fail "工作树有 $DIRTY 个未提交改动(发版前先 commit)"; git status --short | sed 's/^/      /'; }
if git rev-parse --abbrev-ref @{u} >/dev/null 2>&1; then
  git merge-base --is-ancestor "$(git rev-parse @{u})" HEAD 2>/dev/null && pass "本地领先远端可 fast-forward" || fail "与远端分叉,push 前先 rebase/核查"
fi

# ---- 总结 ----
echo ""
if [[ "$FAIL" == "0" ]]; then
  printf '\033[32m=== PREFLIGHT 全绿,可发版 ===\033[0m\n'
  exit 0
else
  printf '\033[31m=== PREFLIGHT 有阻断项,修完再发 ===\033[0m\n'
  exit 1
fi
