"""lto preflight — 默认只 stdout；--record 写 state.json。

Only repo write and git status are hard requirements. Network and MCP checks are
optional probes: missing ANIMEM/memory-flow/MCP or corporate offline networks
must not make core LTO look broken.
"""

from __future__ import annotations

import argparse, os, socket, subprocess, sys
from pathlib import Path
from datetime import datetime, timezone


def collect_checks(repo: Path) -> tuple[list[dict], str]:
    """探活当前环境，返回 (checks, verdict)。供 preflight 命令和 start --profile deploy 复用。"""
    checks: list[dict] = []

    # Sandbox check: can we write files?
    sandbox_ok = _check_write(repo)
    checks.append({"name": "sandbox_write", "pass": sandbox_ok, "detail": "can write to repo" if sandbox_ok else "write failed"})

    # Optional network check. Defaults are configurable and non-blocking.
    net_host = os.getenv("LTO_PREFLIGHT_NETWORK_HOST", "8.8.8.8")
    net_port = _env_int("LTO_PREFLIGHT_NETWORK_PORT", 53)
    net_ok = _check_network(net_host, net_port)
    checks.append({
        "name": "network",
        "pass": net_ok,
        "required": False,
        "detail": f"{net_host}:{net_port} reachable" if net_ok else f"{net_host}:{net_port} unreachable or blocked",
    })

    # Git check
    git_ok = (repo / ".git").is_dir()
    checks.append({"name": "git_repo", "pass": git_ok, "detail": "git repo" if git_ok else "not a git repo"})

    # Optional MCP check. Set LTO_PREFLIGHT_MCP_PORTS=8787:name,18080:asr
    # to probe local services on a specific machine.
    mcp_services = _check_mcp()
    checks.append({"name": "mcp_services", "pass": bool(mcp_services), "required": False,
                   "raw": mcp_services,
                   "detail": ", ".join(mcp_services) if mcp_services else "none configured/detected"})

    # Tmux check
    in_tmux = "TMUX" in os.environ
    checks.append({
        "name": "tmux", "pass": in_tmux, "required": False,
        "detail": "in tmux" if in_tmux else "not in tmux; subprocess fallback available",
    })

    required = [c for c in checks if c.get("required", True)]
    verdict = "pass" if all(c["pass"] for c in required) else "fail"
    return checks, verdict


def run(args: argparse.Namespace) -> int:
    checks, verdict = collect_checks(args.repo)
    required = [c for c in checks if c.get("required", True)]
    passed = sum(1 for c in required if c["pass"])
    total = len(required)

    print(f"=== LTO Preflight ({verdict}: {passed}/{total}) ===")
    for c in checks:
        icon = "✅" if c["pass"] else ("ℹ️" if not c.get("required", True) else "❌")
        print(f"  {icon} {c['name']}: {c['detail']}")

    # Write snapshot to state.json if --record
    if args.record:
        _record_snapshot(args.repo, checks, verdict)

    return 0 if verdict == "pass" else 1


def _check_write(repo: Path) -> bool:
    try:
        test_file = repo / ".lto" / ".preflight_test"
        test_file.parent.mkdir(parents=True, exist_ok=True)
        test_file.write_text("ok")
        test_file.unlink()
        return True
    except Exception:
        return False


def _check_network(host: str, port: int) -> bool:
    try:
        with socket.create_connection((host, port), timeout=3):
            pass
        return True
    except OSError:
        return False


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.getenv(name, str(default)))
    except ValueError:
        return default


def _check_mcp() -> list[str]:
    services = []
    raw = os.getenv("LTO_PREFLIGHT_MCP_PORTS", "")
    if not raw:
        return services
    for item in raw.split(","):
        if not item.strip():
            continue
        port_s, _, name = item.partition(":")
        try:
            port = int(port_s)
        except ValueError:
            continue
        label = name or f"port-{port}"
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                pass
            services.append(label)
        except OSError:
            pass
    return services


def _record_snapshot(repo: Path, checks: list[dict], verdict: str, run_id: str | None = None) -> None:
    from .. import state as st
    if run_id is None:
        current_file = repo / ".lto" / "current"
        if not current_file.exists():
            return
        run_id = current_file.read_text(encoding="utf-8").strip()
    state_path = repo / ".lto" / run_id / "state.json"
    state = st.load_state(state_path)
    if state is None:
        return

    mcp = next((c.get("raw", []) for c in checks if c["name"] == "mcp_services"), [])
    # Merge into the existing snapshot (preserves default_state keys like write_roots,
    # so a recorded snapshot is a superset of the placeholder, never a subset).
    snap = state.get("environment_snapshot") or {}
    snap.update({
        "sandbox": "ok" if any(c["name"] == "sandbox_write" and c["pass"] for c in checks) else "fail",
        "network": "ok" if any(c["name"] == "network" and c["pass"] for c in checks) else "fail",
        "mcp_services": mcp,
        "verdict": verdict,
        "captured_at": datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds"),
    })
    state["environment_snapshot"] = snap
    st.save_state(state_path, state)


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("preflight", help="probe environment health; --record writes state.json")
    p.add_argument("--record", action="store_true", help="write snapshot to state.json")
    p.set_defaults(func=run)
