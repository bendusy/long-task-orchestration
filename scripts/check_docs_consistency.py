#!/usr/bin/env python3
"""Fail on active documentation drift for the Rust v2 release path."""

from __future__ import annotations

import sys
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
    check("Rust v2 is the current default core path" in protocol, "protocol doc states Rust v2 default core path", errors)
    check("Python is an explicit legacy" in protocol and "fallback" in protocol, "protocol doc states explicit Python fallback", errors)

    readme = read("README.md")
    install = read("INSTALL.md")
    agents = read("AGENTS.md")
    rust_release = read("references/rust-migration-release.md")
    oss_req = read("references/open-source-delivery-requirements.md")
    release_workflow = read(".github/workflows/rust-v2.yml")

    check("references/open-source-delivery-requirements.md" in readme, "README links open-source delivery requirements", errors)
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

    run_state = read("references/run-state-workflow.md")
    check(run_state.startswith("# LTO run-state workflow\n\nLegacy Python command reference."), "run-state workflow is explicitly legacy", errors)
    check("python3 scripts/lto_run.py" in run_state, "legacy run-state workflow still documents Python fallback behavior", errors)

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

    if errors:
        print(f"\n{len(errors)} documentation consistency failure(s)", file=sys.stderr)
        return 1
    print("\nDOCS CONSISTENCY OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
