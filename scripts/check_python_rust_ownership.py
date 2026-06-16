#!/usr/bin/env python3
"""Verify Rust-owned command ownership after Python fallback retirement."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "references" / "python-rust-ownership.json"
DOC = ROOT / "references" / "python-rust-ownership.md"


def check(condition: bool, message: str, errors: list[str]) -> None:
    if condition:
        print(f"OK   {message}")
    else:
        print(f"FAIL {message}", file=sys.stderr)
        errors.append(message)


def run(argv: list[str]) -> str:
    proc = subprocess.run(argv, cwd=str(ROOT), capture_output=True, text=True, timeout=30)
    if proc.returncode != 0:
        raise RuntimeError(f"{' '.join(argv)} failed: {proc.stderr[-1000:]}")
    return proc.stdout


def parse_rust_commands(help_text: str) -> list[str]:
    commands: list[str] = []
    in_commands = False
    for line in help_text.splitlines():
        if line == "Commands:":
            in_commands = True
            continue
        if in_commands and line == "Options:":
            break
        if not in_commands or not line.startswith("  "):
            continue
        parts = line.strip().split()
        if parts and parts[0] != "help":
            commands.append(parts[0])
    return commands


def manifest_commands(entries: list[dict[str, Any]]) -> list[str]:
    return [str(entry["command"]) for entry in entries]


def main() -> int:
    errors: list[str] = []
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    doc_text = DOC.read_text(encoding="utf-8")

    top_entries = data.get("top_level_commands", [])
    hidden_entries = data.get("hidden_compatibility_commands", [])
    plugin_entries = data.get("plugin_subcommands", [])
    top_manifest = manifest_commands(top_entries)
    hidden_manifest = manifest_commands(hidden_entries)
    plugin_manifest = manifest_commands(plugin_entries)
    rust_plugin_manifest = [
        str(entry["command"]) for entry in plugin_entries if entry.get("owner") == "rust-core"
    ]

    check(data.get("schema_version") == 1, "ownership manifest schema_version is 1", errors)
    check(len(top_manifest) == len(set(top_manifest)), "top-level ownership commands are unique", errors)
    check(len(hidden_manifest) == len(set(hidden_manifest)), "hidden compatibility commands are unique", errors)
    check(
        not (set(top_manifest) & set(hidden_manifest)),
        "hidden compatibility commands are not visible top-level commands",
        errors,
    )
    check(len(plugin_manifest) == len(set(plugin_manifest)), "plugin ownership commands are unique", errors)

    rust_top = parse_rust_commands(run(["cargo", "run", "--quiet", "--", "--help"]))
    rust_plugin = parse_rust_commands(run(["cargo", "run", "--quiet", "--", "plugin", "--help"]))

    check(sorted(rust_top) == sorted(top_manifest), "Rust top-level help matches ownership manifest", errors)
    check(sorted(rust_plugin) == sorted(rust_plugin_manifest), "Rust plugin help exposes only rust-core plugin subcommands", errors)

    for entry in top_entries:
        command = str(entry["command"])
        check(entry.get("owner") == "rust-core", f"{command} owner is rust-core", errors)
        check(entry.get("python_role") == "removed", f"{command} Python role is removed", errors)
        check(f"`{command}`" in doc_text, f"ownership doc names top-level command {command}", errors)

    for entry in hidden_entries:
        command = str(entry["command"])
        replacement = str(entry.get("replacement", ""))
        check(entry.get("owner") == "rust-core", f"hidden {command} owner is rust-core", errors)
        check(entry.get("python_role") == "removed", f"hidden {command} Python role is removed", errors)
        check(bool(replacement), f"hidden {command} declares replacement", errors)
        check(f"`{command}`" in doc_text, f"ownership doc names hidden command {command}", errors)
        check(
            f"`{replacement}`" in doc_text,
            f"ownership doc names hidden command {command} replacement",
            errors,
        )
        try:
            run(["cargo", "run", "--quiet", "--", command, "--help"])
            parses = True
        except RuntimeError:
            parses = False
        check(parses, f"hidden {command} parses through Rust help", errors)

    for entry in plugin_entries:
        command = str(entry["command"])
        owner = entry.get("owner")
        role = entry.get("python_role")
        check(owner == "rust-core", f"plugin {command} owner is rust-core", errors)
        check(role == "removed", f"plugin {command} Python role is removed", errors)
        check(f"`plugin {command}`" in doc_text, f"ownership doc names plugin command {command}", errors)

    if errors:
        print(f"\n{len(errors)} ownership failure(s)", file=sys.stderr)
        return 1
    print("\nRUST OWNERSHIP OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
