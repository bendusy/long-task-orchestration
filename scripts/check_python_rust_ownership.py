#!/usr/bin/env python3
"""Verify Rust-owned command ownership after Python fallback retirement."""

from __future__ import annotations

import ast
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "references" / "python-rust-ownership.json"
DOC = ROOT / "references" / "python-rust-ownership.md"
AUDIT_LEDGER_PROXY = "scripts/audit_ledger_check.py"


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


def assigned_names(tree: ast.AST) -> set[str]:
    names: set[str] = set()
    for node in ast.walk(tree):
        targets: list[ast.expr] = []
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        for target in targets:
            if isinstance(target, ast.Name):
                names.add(target.id)
    return names


def check_audit_ledger_proxy(entry: dict[str, Any], doc_text: str, errors: list[str]) -> None:
    path = str(entry.get("path", ""))
    replacement = str(entry.get("replacement", ""))
    check(entry.get("owner") == "rust-core", f"{path} semantic owner is rust-core", errors)
    check(entry.get("python_role") == "exec-proxy", f"{path} Python role is exec-proxy", errors)
    check(replacement == "lto check --ledger", f"{path} delegates to lto check --ledger", errors)
    check(
        entry.get("compatibility_window") == "one-version",
        f"{path} compatibility window is one-version",
        errors,
    )
    check(f"`{path}`" in doc_text, f"ownership doc names proxy {path}", errors)
    check(f"`{replacement}`" in doc_text, f"ownership doc names proxy replacement {replacement}", errors)

    proxy_path = ROOT / path
    check(proxy_path.is_file(), f"compatibility proxy exists: {path}", errors)
    if not proxy_path.is_file():
        return
    source = proxy_path.read_text(encoding="utf-8")
    try:
        tree = ast.parse(source, filename=path)
    except SyntaxError as exc:
        check(False, f"compatibility proxy parses as Python: {exc}", errors)
        return

    functions = {
        node.name
        for node in ast.walk(tree)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    forbidden_functions = {
        "split_cells",
        "is_separator_row",
        "is_header_row",
        "parse_count",
        "parse_ledger",
        "extract_rounds",
        "evaluate",
        "evaluate_ledger",
        "verdict_exit_code",
        "report",
        "_assert_case",
        "_ledger",
    }
    check(
        not (functions & forbidden_functions),
        f"{path} has no ledger parser/evaluator functions",
        errors,
    )

    constants = assigned_names(tree)
    forbidden_constants = {
        name
        for name in constants
        if name.startswith("VERDICT_")
        or name in {"ALL_VERDICTS", "ROUND_COL", "HIGH_COL", "CRITICAL_COL"}
    }
    check(not forbidden_constants, f"{path} has no verdict/parser constants", errors)

    verdict_literals = {"CONVERGED", "CONVERGING", "REBOUND", "STALLED", "NO_OBSERVATIONS"}
    string_literals = {
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    }
    check(
        not (string_literals & verdict_literals),
        f"{path} has no local verdict or exit-code mapping",
        errors,
    )

    calls = [node.func for node in ast.walk(tree) if isinstance(node, ast.Call)]
    imported_modules = {
        alias.name
        for node in ast.walk(tree)
        if isinstance(node, ast.Import)
        for alias in node.names
    }
    reads_ledger = any(
        isinstance(func, ast.Name) and func.id == "open"
        or isinstance(func, ast.Attribute) and func.attr in {"open", "read", "read_text"}
        for func in calls
    )
    uses_execvp = any(
        isinstance(func, ast.Attribute)
        and func.attr == "execvp"
        and isinstance(func.value, ast.Name)
        and func.value.id == "os"
        for func in calls
    )
    check(not reads_ledger, f"{path} does not read or parse ledger contents", errors)
    check("subprocess" not in imported_modules, f"{path} does not translate Rust exit codes", errors)
    check(uses_execvp, f"{path} delegates with os.execvp", errors)
    check(
        {"check", "--ledger"}.issubset(string_literals),
        f"{path} exec target is lto check --ledger",
        errors,
    )


def main() -> int:
    errors: list[str] = []
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    doc_text = DOC.read_text(encoding="utf-8")

    top_entries = data.get("top_level_commands", [])
    hidden_entries = data.get("hidden_compatibility_commands", [])
    proxy_entries = data.get("python_compatibility_proxies", [])
    plugin_entries = data.get("plugin_subcommands", [])
    top_manifest = manifest_commands(top_entries)
    hidden_manifest = manifest_commands(hidden_entries)
    proxy_manifest = [str(entry.get("path", "")) for entry in proxy_entries]
    plugin_manifest = manifest_commands(plugin_entries)
    rust_plugin_manifest = [
        str(entry["command"]) for entry in plugin_entries if entry.get("owner") == "rust-core"
    ]

    check(data.get("schema_version") == 1, "ownership manifest schema_version is 1", errors)
    check(len(top_manifest) == len(set(top_manifest)), "top-level ownership commands are unique", errors)
    check(len(hidden_manifest) == len(set(hidden_manifest)), "hidden compatibility commands are unique", errors)
    check(len(proxy_manifest) == len(set(proxy_manifest)), "Python compatibility proxy paths are unique", errors)
    check(AUDIT_LEDGER_PROXY in proxy_manifest, "audit ledger compatibility proxy is registered", errors)
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

    for entry in proxy_entries:
        if entry.get("path") == AUDIT_LEDGER_PROXY:
            check_audit_ledger_proxy(entry, doc_text, errors)

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
