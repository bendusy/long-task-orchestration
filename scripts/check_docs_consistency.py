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


# active 文档：ROUTER 允许落地的当前口径文档。specs/、backlog、validation-log、
# dated review 是历史/设计材料，不进 active 集合。
ACTIVE_DOCS = [
    "SKILL.md",
    "README.md",
    "COMMANDS.md",
    "INSTALL.md",
    "AGENTS.md",
    "CLAUDE.md",
    "references/INDEX.md",
    "references/onboarding.md",
    "references/run-state-workflow.md",
    "references/execution-loop.md",
    "references/workflow-playbook.md",
    "references/playbooks/review.md",
    "references/playbooks/enterprise-audit.md",
    "references/playbooks/debug.md",
    "references/playbooks/migration.md",
    "references/playbooks/claim-verify.md",
    "references/playbooks/research.md",
    "references/playbooks/feature-dev.md",
    "references/playbooks/tmux-goal-loop.md",
    "references/playbooks/docs-sync.md",
    "references/playbooks/release.md",
    "references/playbooks/direction-review.md",
    "references/control-loop-harness.md",
    "references/events-telemetry-contract.md",
    "references/audit-convergence.md",
    "references/long-loop-state.md",
    "references/decision-logging.md",
    "references/release-workflow.md",
    "references/deploy-sequencing.md",
    "references/hooks.md",
    "references/sharing-guide.md",
    "references/cross-runtime-host-notes.md",
    "references/hs-as-core-tool.md",
    "references/plugin-boundary.md",
    "references/rust-migration-release.md",
    "references/runner-readonly-contract.md",
    "references/protocol-and-language-strategy.md",
    "references/open-source-delivery-requirements.md",
    "references/engineering-map.md",
]

STALE_FLAG_DENYLIST = [
    "--request ",
    "--with-audit",
    "--profile audit",
    "--profile deploy",
    "--install-hooks",
    "--auto-commit",
]


def check_stale_flags(errors: list[str]) -> None:
    for rel in ACTIVE_DOCS:
        hits = contains_any(read(rel), STALE_FLAG_DENYLIST)
        check(not hits, f"{rel} has no stale CLI flags: {hits}", errors)


def check_no_handwritten_command_count(errors: list[str]) -> None:
    # 手写“N 个可见业务命令”必然漂移；总数只允许出现在 COMMANDS.md（由 cli.rs 派生校验）
    pattern = re.compile(r"\d+\s*个可见业务命令")
    for rel in ACTIVE_DOCS:
        if rel == "COMMANDS.md":
            continue
        check(
            not pattern.search(read(rel)),
            f"{rel} has no handwritten business-command count",
            errors,
        )


LINK_RE = re.compile(r"\[[^\]]*\]\(([^)#\s]+)(#[^)\s]*)?\)")


def _anchor_ok(target_text: str, anchor: str) -> bool:
    # GitHub 风格宽松校验：标题与 anchor 都去掉非字母数字汉字后归一比较
    def norm(value: str) -> str:
        return re.sub(r"[^0-9a-zA-Z一-鿿]+", "", value).lower()

    want = norm(anchor.lstrip("#"))
    return any(
        norm(line.lstrip("#")) == want
        for line in target_text.splitlines()
        if line.startswith("#")
    )


FENCE_RE = re.compile(r"```(?:bash|sh)\n(.*?)```", re.S)
_INVOKERS = ("lto", "lto-rs", "$LTO", "$L")


def _merged_lines(block: str) -> list[str]:
    # bash 续行（\ 结尾）合并成单行再分析
    merged: list[str] = []
    buffer = ""
    for raw in block.splitlines():
        line = raw.rstrip()
        if line.endswith("\\"):
            buffer += line[:-1] + " "
            continue
        merged.append(buffer + line)
        buffer = ""
    if buffer:
        merged.append(buffer)
    return merged


def _extract_sub_and_flags(cmdline: str) -> tuple[list[str], list[str]]:
    tokens = cmdline.split()
    idx = None
    for i, token in enumerate(tokens):
        if token in _INVOKERS:
            idx = i + 1
            break
        if token == "cargo" and "--" in tokens[i:]:
            idx = tokens.index("--", i) + 1
            break
    if idx is None:
        return [], []
    # 跳过子命令前的全局旗标（如 --repo <path>）
    j = idx
    while j < len(tokens) and tokens[j].startswith("--"):
        j += 2
    subpath: list[str] = []
    while j < len(tokens) and len(subpath) < 2:
        token = tokens[j]
        if not re.fullmatch(r"[a-z][a-z0-9-]*", token):
            break
        subpath.append(token)
        j += 1
    if not subpath:
        return [], []
    rest = " ".join(tokens[j:])
    # 引号字符串里的 --xxx 是命令参数值（如 --instrument "eval.py --hidden"），不是本命令旗标
    rest = re.sub(r'"[^"]*"', "", rest)
    rest = re.sub(r"'[^']*'", "", rest)
    flags = re.findall(r"(--[a-z][a-z0-9-]*)", rest)
    return subpath, flags


def _lto_bin() -> list[str] | None:
    import shutil

    release = ROOT / "target/release/lto-rs"
    if release.exists():
        return [str(release)]
    if shutil.which("cargo"):
        return ["cargo", "run", "--quiet", "--"]
    return None


def check_fenced_lto_flags(errors: list[str]) -> None:
    import subprocess

    bin_cmd = _lto_bin()
    if bin_cmd is None:
        print("SKIP fenced-command flag check (no lto binary/cargo)")
        return
    help_cache: dict[tuple[str, ...], str] = {}
    for rel in ACTIVE_DOCS:
        for block in FENCE_RE.findall(read(rel)):
            for line in _merged_lines(block):
                if line.lstrip().startswith("#"):
                    continue
                subpath, flags = _extract_sub_and_flags(line)
                if not subpath or not flags:
                    continue
                key = tuple(subpath)
                if key not in help_cache:
                    proc = subprocess.run(
                        [*bin_cmd, *subpath, "--help"],
                        capture_output=True,
                        text=True,
                        cwd=ROOT,
                    )
                    help_cache[key] = proc.stdout + proc.stderr
                    # 两级子命令拿不到 help 时回退一级（如 `task add` → `task`）
                    if "Usage:" not in help_cache[key] and len(subpath) == 2:
                        proc = subprocess.run(
                            [*bin_cmd, subpath[0], "--help"],
                            capture_output=True,
                            text=True,
                            cwd=ROOT,
                        )
                        help_cache[key] = proc.stdout + proc.stderr
                for flag in flags:
                    check(
                        flag in help_cache[key],
                        f"{rel}: `lto {' '.join(subpath)}` supports {flag}",
                        errors,
                    )


def check_relative_links(errors: list[str]) -> None:
    for rel in ACTIVE_DOCS:
        base = (ROOT / rel).parent
        for match in LINK_RE.finditer(read(rel)):
            target, anchor = match.group(1), match.group(2)
            if target.startswith(("http://", "https://", "mailto:")):
                continue
            path = (base / target).resolve()
            check(path.exists(), f"{rel} link target exists: {target}", errors)
            if anchor and path.suffix == ".md" and path.exists():
                check(
                    _anchor_ok(path.read_text(encoding="utf-8"), anchor),
                    f"{rel} anchor resolves: {target}{anchor}",
                    errors,
                )


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

    check_stale_flags(errors)
    check_no_handwritten_command_count(errors)
    check_relative_links(errors)
    check_fenced_lto_flags(errors)

    if errors:
        print(f"\n{len(errors)} documentation consistency failure(s)", file=sys.stderr)
        return 1
    print("\nDOCS CONSISTENCY OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
