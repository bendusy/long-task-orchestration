#!/usr/bin/env python3
"""lto release — 发布纪律自动化：bump VERSION + CHANGELOG 归版 + git tag。

补今天发现的漏洞：budget 契约合 main 但 VERSION 没 bump、还躺 Unreleased、没 tag。
全是 .git 写操作 → 由 host 跑（runner sandbox 写不了 .git，今天实证）。

流程（--dry-run 看计划，不写）：
  1. 读 VERSION → 按 --part(major/minor/patch) 算新版本
  2. CHANGELOG 的 `## Unreleased` 段重命名为 `## vX.Y.Z — <date>`，顶部新建空 Unreleased
  3. 写回 VERSION
  4. （非 --dry-run）git commit + git tag vX.Y.Z
date 由 --date 注入（脚本环境取系统时间受限，与 LTO 既有模式一致）。
smoke 门由调用方在 release 前自行跑；release 只管版本机械操作（单一职责）。
"""
from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


def _bump(version: str, part: str) -> str:
    nums = version.strip().split(".")
    if len(nums) != 3 or not all(n.isdigit() for n in nums):
        raise SystemExit(f"VERSION not semver x.y.z: {version!r}")
    major, minor, patch = (int(n) for n in nums)
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise SystemExit(f"invalid --part: {part!r}")


def _rewrite_changelog(text: str, new_version: str, date: str) -> str:
    """`## Unreleased` → `## vX.Y.Z — date`，并在顶部插回空 Unreleased。
    缺 Unreleased 段则报错（防止误发空版本）。"""
    marker = "## Unreleased"
    if marker not in text:
        raise SystemExit("CHANGELOG.md has no '## Unreleased' section")
    versioned = f"## v{new_version} — {date}"
    # 只替换第一个出现的 Unreleased 标题行，正文跟随归入该版本
    new_unreleased = f"## Unreleased\n\n{versioned}"
    return text.replace(marker, new_unreleased, 1)


def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    version_path = repo / "VERSION"
    changelog_path = repo / "CHANGELOG.md"
    if not version_path.exists():
        raise SystemExit(f"no VERSION file at {version_path}")
    if not changelog_path.exists():
        raise SystemExit(f"no CHANGELOG.md at {changelog_path}")

    old_version = version_path.read_text(encoding="utf-8").strip()
    new_version = _bump(old_version, args.part)
    tag = f"v{new_version}"

    changelog = changelog_path.read_text(encoding="utf-8")
    new_changelog = _rewrite_changelog(changelog, new_version, args.date)

    print(f"# lto release: {old_version} → {new_version} (tag {tag})")
    print(f"  VERSION: {old_version} → {new_version}")
    print(f"  CHANGELOG: Unreleased → v{new_version} — {args.date}")

    if args.dry_run:
        print("  (dry-run — nothing written)")
        return 0

    version_path.write_text(new_version + "\n", encoding="utf-8")
    changelog_path.write_text(new_changelog, encoding="utf-8")

    if args.no_git:
        print("  VERSION + CHANGELOG written (--no-git: skipped commit/tag)")
        return 0

    # git commit + tag（host 做，runner sandbox 写不了 .git）
    subprocess.run(["git", "-C", str(repo), "add", "VERSION", "CHANGELOG.md"], check=True)
    subprocess.run(
        ["git", "-C", str(repo), "commit", "-m", f"chore(release): {tag}"], check=True
    )
    subprocess.run(["git", "-C", str(repo), "tag", tag], check=True)
    print(f"  committed + tagged {tag}")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("release", help="bump VERSION + CHANGELOG 归版 + git tag")
    p.add_argument("--part", choices=["major", "minor", "patch"], default="minor",
                   help="semver part to bump (default: minor)")
    p.add_argument("--date", required=True, help="release date (ISO, injected by caller)")
    p.add_argument("--dry-run", action="store_true", help="show plan, write nothing")
    p.add_argument("--no-git", action="store_true",
                   help="write VERSION+CHANGELOG but skip git commit/tag")
    p.set_defaults(func=run)
