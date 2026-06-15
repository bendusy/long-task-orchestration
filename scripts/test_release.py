#!/usr/bin/env python3
"""Tests for `lto release` — bump VERSION + CHANGELOG 归版 + tag。"""
from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

from lto.commands import release  # noqa: E402


class TestBump(unittest.TestCase):
    def test_minor(self):
        self.assertEqual(release._bump("0.3.0", "minor"), "0.4.0")

    def test_major(self):
        self.assertEqual(release._bump("0.3.0", "major"), "1.0.0")

    def test_patch(self):
        self.assertEqual(release._bump("0.3.0", "patch"), "0.3.1")

    def test_minor_resets_patch(self):
        self.assertEqual(release._bump("1.2.5", "minor"), "1.3.0")

    def test_non_semver_rejected(self):
        with self.assertRaises(SystemExit):
            release._bump("0.3", "minor")


class TestChangelog(unittest.TestCase):
    def test_unreleased_renamed_and_new_empty_added(self):
        text = "# Changelog\n\n## Unreleased\n\n### feat X\n\n## v0.3.0 — 2026-06-09\n"
        out = release._rewrite_changelog(text, "0.4.0", "2026-06-15")
        # 新空 Unreleased 在顶
        self.assertIn("## Unreleased\n\n## v0.4.0 — 2026-06-15", out)
        # 原内容归入 0.4.0（feat X 跟在 v0.4.0 之后）
        self.assertIn("## v0.4.0 — 2026-06-15\n\n### feat X", out)
        # 旧版本仍在
        self.assertIn("## v0.3.0 — 2026-06-09", out)

    def test_missing_unreleased_rejected(self):
        with self.assertRaises(SystemExit):
            release._rewrite_changelog("# Changelog\n\n## v0.3.0\n", "0.4.0", "2026-06-15")


def _args(repo, **kw):
    ns = argparse.Namespace(repo=repo, part="minor", date="2026-06-15",
                            dry_run=False, no_git=False)
    for k, v in kw.items():
        setattr(ns, k, v)
    return ns


class TestRunEndToEnd(unittest.TestCase):
    def _mk(self, tmp):
        repo = Path(tmp)
        (repo / "VERSION").write_text("0.3.0\n")
        (repo / "CHANGELOG.md").write_text(
            "# Changelog\n\n## Unreleased\n\n### feat X\n\n## v0.3.0 — 2026-06-09\n")
        subprocess.run(["git", "-C", str(repo), "init", "-q"], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.email", "t@t"], check=True)
        subprocess.run(["git", "-C", str(repo), "config", "user.name", "t"], check=True)
        subprocess.run(["git", "-C", str(repo), "add", "-A"], check=True)
        subprocess.run(["git", "-C", str(repo), "commit", "-qm", "init"], check=True)
        return repo

    def test_dry_run_writes_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._mk(tmp)
            rc = release.run(_args(repo, dry_run=True))
            self.assertEqual(rc, 0)
            self.assertEqual((repo / "VERSION").read_text().strip(), "0.3.0")  # 未变

    def test_no_git_writes_files_no_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._mk(tmp)
            rc = release.run(_args(repo, no_git=True))
            self.assertEqual(rc, 0)
            self.assertEqual((repo / "VERSION").read_text().strip(), "0.4.0")
            tags = subprocess.run(["git", "-C", str(repo), "tag"],
                                  capture_output=True, text=True).stdout
            self.assertNotIn("v0.4.0", tags)  # --no-git 不打 tag

    def test_full_release_commits_and_tags(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self._mk(tmp)
            rc = release.run(_args(repo))
            self.assertEqual(rc, 0)
            self.assertEqual((repo / "VERSION").read_text().strip(), "0.4.0")
            tags = subprocess.run(["git", "-C", str(repo), "tag"],
                                  capture_output=True, text=True).stdout
            self.assertIn("v0.4.0", tags)
            log = subprocess.run(["git", "-C", str(repo), "log", "--oneline"],
                                 capture_output=True, text=True).stdout
            self.assertIn("chore(release): v0.4.0", log)


if __name__ == "__main__":
    unittest.main()
