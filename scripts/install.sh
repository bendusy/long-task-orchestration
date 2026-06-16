#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LTO_BIN_DIR_CONFIG="${LTO_BIN_DIR:-$HOME/.local/bin}"
LTO_WRAPPER_SENTINEL="# long-task-orchestration managed lto wrapper"
LTO_RS_TARGET="$REPO_ROOT/target/release/lto-rs"

CHECK_ONLY=0
if [ "${1:-}" = "--check" ]; then
  CHECK_ONLY=1
fi

fail=0
warn=0

need() {
  if command -v "$1" >/dev/null 2>&1; then
    printf "  [OK]   %s\n" "$1"
  else
    printf "  [MISS] %s — %s\n" "$1" "$2" >&2
    fail=$((fail+1))
  fi
}

optional() {
  if command -v "$1" >/dev/null 2>&1; then
    printf "  [OK]   %s\n" "$1"
  else
    printf "  [OPT]  %s — %s\n" "$1" "$2"
  fi
}

report_lto_wrapper() {
  local bin_dir="$LTO_BIN_DIR_CONFIG"
  local wrapper="$bin_dir/lto"
  echo "==> LTO wrapper"
  printf "  [INFO] bin dir: %s\n" "$bin_dir"
  if [ ! -e "$wrapper" ]; then
    printf "  [MISS] %s\n" "$wrapper"
    return
  fi
  if grep -Fq "$LTO_WRAPPER_SENTINEL" "$wrapper" 2>/dev/null; then
    local rust_target
    local default_runtime
    rust_target="$(grep '^LTO_RS_BIN=' "$wrapper" 2>/dev/null | head -n 1 | cut -d= -f2- || true)"
    default_runtime="$(grep '^LTO_DEFAULT_RUNTIME=' "$wrapper" 2>/dev/null | head -n 1 | cut -d= -f2- || true)"
    printf "  [OK]   managed wrapper: %s\n" "$wrapper"
    if [ -n "$default_runtime" ]; then
      printf "         default: %s\n" "$default_runtime"
    fi
    if [ -n "$rust_target" ]; then
      printf "         rust: %s\n" "$rust_target"
    fi
  else
    printf "  [WARN] unmanaged lto exists: %s\n" "$wrapper"
    warn=$((warn+1))
  fi
}

build_lto_rs() {
  echo "==> Rust binary"
  if [ ! -x "$LTO_RS_TARGET" ]; then
    printf "  [INFO] building %s\n" "$LTO_RS_TARGET"
  else
    printf "  [INFO] refreshing %s\n" "$LTO_RS_TARGET"
  fi
  cargo build --release --locked --bin lto-rs
  if [ ! -x "$LTO_RS_TARGET" ]; then
    printf "  [MISS] Rust binary was not produced: %s\n" "$LTO_RS_TARGET" >&2
    fail=$((fail+1))
    return
  fi
  printf "  [OK]   %s\n" "$LTO_RS_TARGET"
}

install_lto_wrapper() {
  local bin_dir="$LTO_BIN_DIR_CONFIG"
  mkdir -p "$bin_dir"
  local bin_abs
  bin_abs="$(cd "$bin_dir" && pwd)"
  local wrapper="$bin_abs/lto"

  echo "==> LTO wrapper: $wrapper"
  if [ -e "$wrapper" ] && ! grep -Fq "$LTO_WRAPPER_SENTINEL" "$wrapper" 2>/dev/null; then
    printf "  [SKIP] lto — unmanaged file already exists: %s\n" "$wrapper" >&2
    fail=$((fail+1))
    return
  fi

  {
    echo "#!/usr/bin/env bash"
    echo "$LTO_WRAPPER_SENTINEL"
    echo "set -euo pipefail"
    printf "LTO_RS_BIN=%q\n" "$LTO_RS_TARGET"
    echo "LTO_DEFAULT_RUNTIME=rust"
    cat <<'WRAPPER'
runtime="${LTO_DEFAULT_RUNTIME:-rust}"
if [ "${1:-}" = "--use-rust" ]; then
  runtime=rust
  shift
fi
if [ "${1:-}" = "--use-python" ]; then
  echo "lto: Python fallback was removed in v0.5.0; use the Rust CLI directly" >&2
  exit 64
fi
if [ "${LTO_USE_PYTHON:-0}" = "1" ]; then
  echo "lto: LTO_USE_PYTHON is no longer supported; Python fallback was removed in v0.5.0" >&2
  exit 64
elif [ "${LTO_USE_RUST:-}" = "1" ]; then
  runtime=rust
fi
if [ "$runtime" = "rust" ]; then
  if [ ! -x "$LTO_RS_BIN" ]; then
    echo "lto: Rust binary is missing or not executable: $LTO_RS_BIN" >&2
    echo "lto: build it with: cargo build --release --bin lto-rs" >&2
    exit 1
  fi
  exec "$LTO_RS_BIN" "$@"
fi
echo "lto: unsupported runtime '$runtime'; only rust is available" >&2
exit 64
WRAPPER
  } > "$wrapper"
  chmod +x "$wrapper"
  printf "  [OK]   %s\n" "$wrapper"

  case ":$PATH:" in
    *":$bin_abs:"*) ;;
    *)
      printf "  [WARN] %s is not in PATH; add it to run lto directly\n" "$bin_abs" >&2
      warn=$((warn+1))
      ;;
  esac
}

echo "==> Required CLIs"
need cargo "Rust v2 CLI build requires Cargo"
need bash "installer and wrapper require bash"
need git "LTO state, drift checks, and worktree sandbox require git"

echo "==> Optional CLIs"
optional tmux "needed by bundled delegate tmux fan-out"
optional codex "OpenAI-family runtime/auditor"
optional claude "Anthropic-family runtime/auditor"
optional pi "DeepSeek-family runtime/auditor"
optional agy "Gemini-family runtime/auditor"

echo "==> Optional memory sink"
if [ -n "${MEMORY_FLOW_URL:-}" ]; then
  printf "  [OK]   MEMORY_FLOW_URL configured\n"
else
  printf "  [OPT]  MEMORY_FLOW_URL — set only for optional artifact-memory publish/resume\n"
fi

if [ $fail -gt 0 ]; then
  echo "==> preflight failed: fail=$fail warn=$warn" >&2
  exit 1
fi

if [ "$CHECK_ONLY" -eq 1 ]; then
  report_lto_wrapper
  echo "==> --check mode: skip install. warn=$warn"
  exit 0
fi

build_lto_rs
install_lto_wrapper

if [ $fail -gt 0 ]; then
  echo "==> install finished with errors: fail=$fail warn=$warn" >&2
  exit 2
fi
echo "==> install ok. warn=$warn"
