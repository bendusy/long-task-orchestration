#!/usr/bin/env python3
"""Self-test for LTO data-only plugin validate/list/mount."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LTO = ROOT / "scripts" / "lto_run.py"
SAMPLE = ROOT / "plugins" / "deep-agent-profiles"


def ok(cond: bool, msg: str) -> int:
    if cond:
        print(f"OK   {msg}")
        return 0
    print(f"FAIL {msg}", file=sys.stderr)
    return 1


def run(*args: str, cwd: Path = ROOT) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(LTO), *args], cwd=str(cwd), text=True, capture_output=True, timeout=60)


def main() -> int:
    errors = 0
    errors += ok(SAMPLE.exists(), "sample plugin exists")

    r = run("plugin", "validate", str(SAMPLE), "--json")
    errors += ok(r.returncode == 0, f"sample validate rc=0 (got {r.returncode})")
    data = json.loads(r.stdout)
    errors += ok(data.get("ok") is True, "sample validation ok")
    errors += ok(data.get("plugin_id") == "deep-agent-profiles", "sample plugin id")
    errors += ok(str(data.get("manifest_hash", "")).startswith("sha256:"), "manifest hash present")

    r = run("plugin", "list", "--json")
    errors += ok(r.returncode == 0, "plugin list rc=0")
    rows = json.loads(r.stdout)
    errors += ok(any(row.get("id") == "deep-agent-profiles" and row.get("ok") for row in rows), "plugin list includes sample")

    with tempfile.TemporaryDirectory(prefix="lto_plugin_test_") as td:
        repo = Path(td) / "repo"
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / "README.md").write_text("test\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(repo), "add", "README.md"], check=True)
        subprocess.run(["git", "-C", str(repo), "commit", "-q", "-m", "init"], check=True)
        r = run("--repo", str(repo), "start", "--goal", "plugin test", "--host", "test", "--force")
        errors += ok(r.returncode == 0, f"start temp run rc=0 (got {r.returncode})")
        run_id = r.stdout.strip().split("/")[-1]
        r = run("--repo", str(repo), "plugin", "mount", str(SAMPLE), "--run-id", run_id, "--json")
        errors += ok(r.returncode == 0, f"mount rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
        entry = json.loads(r.stdout)
        errors += ok(entry.get("plugin_id") == "deep-agent-profiles", "mount entry plugin id")
        lock = repo / ".lto" / run_id / "plugin-mounts.json"
        errors += ok(lock.exists(), "mount lock written")
        lock_data = json.loads(lock.read_text(encoding="utf-8"))
        errors += ok(lock_data.get("mounts", [{}])[0].get("manifest_hash", "").startswith("sha256:"), "lock has manifest hash")
        artifacts = repo / ".lto" / run_id / "artifacts.json"
        errors += ok("plugin-mounts.json" in artifacts.read_text(encoding="utf-8"), "mount lock registered as artifact")

    with tempfile.TemporaryDirectory(prefix="lto_bad_plugin_") as td:
        bad = Path(td) / "bad"
        bad.mkdir()
        (bad / "plugin.json").write_text(json.dumps({
            "id": "bad-plugin",
            "version": "0.1.0",
            "stage": "experimental",
            "kind": "path-plugin",
            "source_notes": [],
            "provides": {},
            "security": {"executable_code": True, "max_sandbox": "danger-full-access"}
        }), encoding="utf-8")
        r = run("plugin", "validate", str(bad), "--json")
        errors += ok(r.returncode != 0, "bad plugin rejected")
        bad_data = json.loads(r.stdout)
        errors += ok(any("executable_code" in e for e in bad_data.get("errors", [])), "bad plugin executable_code error")

    if errors == 0:
        print("\nPLUGIN TESTS OK")
        return 0
    print(f"\n{errors} FAILURES", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
