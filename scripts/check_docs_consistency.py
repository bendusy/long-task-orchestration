#!/usr/bin/env python3
"""Fail on active documentation drift for the Rust v2 release path."""

from __future__ import annotations

import sys
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def check(condition: bool, message: str, errors: list[str]) -> None:
    if condition:
        print(f"OK   {message}")
    else:
        print(f"FAIL {message}", file=sys.stderr)
        errors.append(message)


def contains_any(text: str, needles: list[str]) -> list[str]:
    return [needle for needle in needles if needle in text]


def cargo_package_version(cargo_toml: str) -> str | None:
    in_package = False
    for raw in cargo_toml.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            in_package = line == "[package]"
            continue
        if in_package and line.startswith("version"):
            _, _, value = line.partition("=")
            return value.strip().strip('"')
    return None


def rust_owned_commands(cli_rs: str) -> list[str]:
    match = re.search(r'pub const COMMANDS:\s*&\[&str\]\s*=\s*&\[(?P<body>.*?)\];', cli_rs, re.S)
    if not match:
        return []
    return re.findall(r'"([^"]+)"', match.group("body"))


def commands_doc_rows(commands_md: str) -> list[str]:
    return re.findall(r"^\|\s*`([^` ]+)`", commands_md, flags=re.MULTILINE)


def commands_doc_count(commands_md: str) -> int | None:
    match = re.search(r"Command count:\s*(\d+)", commands_md)
    return int(match.group(1)) if match else None


def main() -> int:
    errors: list[str] = []

    protocol = read("references/protocol-and-language-strategy.md")
    old_language_claims = [
        "Now:      Python core",
        "Python core, because protocols are still changing",
        "### Python now",
        "### Go later",
        "Go shadow CLI",
        "### Rust only for narrow components",
        "Rust is not currently worth the cost for LTO core",
        "Decision: keep Python as primary implementation",
        "Keep shipping small protocol-backed improvements in Python",
    ]
    stale_protocol = contains_any(protocol, old_language_claims)
    check(not stale_protocol, f"protocol language roadmap has no stale claims: {stale_protocol}", errors)
    check("Rust v2 is the current core path" in protocol, "protocol doc states Rust v2 core path", errors)
    check("Python fallback was removed" in protocol, "protocol doc states Python fallback removal", errors)

    readme = read("README.md")
    install = read("INSTALL.md")
    agents = read("AGENTS.md")
    rust_release = read("references/rust-migration-release.md")
    oss_req = read("references/open-source-delivery-requirements.md")
    release_workflow = read(".github/workflows/rust-v2.yml")
    cli_rs = read("src/cli.rs")
    commands_md = read("COMMANDS.md")
    version = read("VERSION").strip()
    cargo_version = cargo_package_version(read("Cargo.toml"))
    rust_commands = rust_owned_commands(cli_rs)
    documented_commands = commands_doc_rows(commands_md)
    documented_count = commands_doc_count(commands_md)
    help_row_count = len(rust_commands) + 1  # clap adds the built-in `help` pseudo-command.

    check("references/open-source-delivery-requirements.md" in readme, "README links open-source delivery requirements", errors)
    check(cargo_version == version, f"Cargo.toml package version matches VERSION ({cargo_version!r} vs {version!r})", errors)
    check(bool(rust_commands), "src/cli.rs COMMANDS contract is parseable", errors)
    check(documented_count == help_row_count, f"COMMANDS.md command count matches Rust help rows ({documented_count!r} vs {help_row_count})", errors)
    check(sorted(documented_commands) == sorted(rust_commands), "COMMANDS.md table rows match Rust-owned commands", errors)
    check("clap built-in `help`" in commands_md, "COMMANDS.md explains the generated help pseudo-command", errors)
    check("二进制下载是 release-gated" in readme, "README gates binary downloads on release assets", errors)
    check("Rust 二进制安装是 release-gated" in install, "INSTALL gates binary installs on release assets", errors)
    check("Binary installation is release-gated" in rust_release, "release doc gates binary availability on live assets", errors)
    check("Verify current GitHub Releases" in rust_release, "release doc requires live release verification", errors)
    check("shasum -a 256 -c" in rust_release and "./lto-rs self-test" in rust_release, "release doc requires checksum and self-test", errors)
    check("Treat binary availability as release-gated" in oss_req, "open-source requirements preserve release asset gate", errors)
    check("Verify packaged binary" in release_workflow, "release workflow verifies packaged binary before upload", errors)
    check("Verify uploaded release asset" in release_workflow, "release workflow verifies uploaded GitHub Release asset", errors)
    check("gh release download" in release_workflow, "release workflow downloads uploaded assets for verification", errors)
    check("lto-rs\" self-test" in release_workflow, "release workflow self-tests unpacked assets", errors)
    check("(cd dist && shasum -a 256 -c" in release_workflow, "release workflow verifies package checksum from dist directory", errors)
    check("macos-15-intel" in release_workflow, "release workflow uses current Intel macOS runner label", errors)
    check("macos-13" not in release_workflow, "release workflow avoids retired macos-13 release runner", errors)
    check("musl-tools" in release_workflow, "release workflow installs musl toolchain for Linux asset", errors)
    check("max-parallel: 1" in release_workflow, "release workflow serializes GitHub Release asset upload/verify", errors)

    for rel, text in [
        ("README.md", readme),
        ("INSTALL.md", install),
        ("AGENTS.md", agents),
        ("references/rust-migration-release.md", rust_release),
    ]:
        check("Windows" in text and ("paused" in text or "暂" in text), f"{rel} states Windows native support is paused", errors)

    active_command_docs = [
        "references/execution-loop.md",
        "references/engineering-map.md",
        "references/codex-cli-control.md",
        "references/long-loop-state.md",
    ]
    for rel in active_command_docs:
        text = read(rel)
        check("python3 scripts/lto_run.py" not in text, f"{rel} does not teach Python as active entrypoint", errors)
        check("`lto_run.py" not in text, f"{rel} table/prose uses lto wrapper rather than lto_run.py command names", errors)

    active_runtime_docs = [
        "references/onboarding.md",
        "references/execution-loop.md",
        "references/plugin-real-eval-runner.md",
    ]
    for rel in active_runtime_docs:
        text = read(rel)
        stale_modules = contains_any(text, ["agent_exec.py", "agent_job.py", "scheduler.py", "autopilot.py"])
        check(not stale_modules, f"{rel} does not document retired Python runtime modules: {stale_modules}", errors)

    plugin_real = read("references/plugin-real-eval-runner.md")
    check(
        "Python fallback was removed in v0.5.0" in plugin_real,
        "plugin real eval runner doc records Python fallback removal",
        errors,
    )

    run_state = read("references/run-state-workflow.md")
    check("python3 scripts/lto_run.py" not in run_state, "run-state workflow no longer documents Python fallback commands", errors)
    check("lto start" in run_state or "cargo run --quiet -- start" in run_state, "run-state workflow documents Rust CLI commands", errors)

    forbidden_public_claims = [
        "Windows native support is supported",
        "Windows native release is supported",
        "downloadable binaries are available without verification",
        "GitHub Releases provide downloadable binaries without verification",
    ]
    public_docs = [
        "README.md",
        "INSTALL.md",
        "AGENTS.md",
        "CLAUDE.md",
        "SKILL.md",
        "references/rust-migration-release.md",
        "references/open-source-delivery-requirements.md",
    ]
    for rel in public_docs:
        hits = contains_any(read(rel), forbidden_public_claims)
        check(not hits, f"{rel} has no false platform/release claim: {hits}", errors)
        stale_python = contains_any(read(rel), ["--use-python", "LTO_USE_PYTHON", "scripts/lto_run.py"])
        check(not stale_python, f"{rel} has no active Python fallback instructions: {stale_python}", errors)

    if errors:
        print(f"\n{len(errors)} documentation consistency failure(s)", file=sys.stderr)
        return 1
    print("\nDOCS CONSISTENCY OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
