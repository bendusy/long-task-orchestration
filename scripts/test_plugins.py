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

    with tempfile.TemporaryDirectory(prefix="lto_plugin_render_") as td:
        tmp = Path(td)
        brief = tmp / "brief.md"
        out = tmp / "rendered.md"
        meta = tmp / "rendered.meta.json"
        brief.write_text("Goal:\nAudit this design.\n", encoding="utf-8")
        r = run("plugin", "render-profile", str(SAMPLE), "codex-audit-readonly-v1",
                "--input", str(brief), "--output", str(out), "--meta-output", str(meta), "--json")
        errors += ok(r.returncode == 0, f"render-profile rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
        rendered = out.read_text(encoding="utf-8")
        errors += ok("Goal:" in rendered and "batch" in rendered.lower(), "rendered prompt includes base + profile instructions")
        meta_data = json.loads(meta.read_text(encoding="utf-8"))
        errors += ok(meta_data.get("profile_id") == "codex-audit-readonly-v1", "render meta profile id")

    r = run("plugin", "eval", str(SAMPLE), "--json")
    errors += ok(r.returncode == 0, f"plugin static eval rc=0 (got {r.returncode})")
    eval_data = json.loads(r.stdout)
    errors += ok(eval_data.get("ok") is True and eval_data.get("evals"), "plugin static eval ok")

    with tempfile.TemporaryDirectory(prefix="lto_source_note_") as td:
        copy = Path(td) / "plugin"
        subprocess.run(["cp", "-R", str(SAMPLE), str(copy)], check=True)
        r = run("plugin", "source-note", str(copy), "--id", "note.test.article", "--title", "Test Article",
                "--url", "https://example.com/test", "--claim", "claim one", "--hypothesis", "hypothesis one",
                "--append-manifest", "--json")
        errors += ok(r.returncode == 0, f"source-note rc=0 (got {r.returncode})")
        note = copy / "sources" / "note.test.article.json"
        errors += ok(note.exists(), "source-note file written")
        manifest = json.loads((copy / "plugin.json").read_text(encoding="utf-8"))
        errors += ok("sources/note.test.article.json" in manifest.get("source_notes", []), "source-note appended to manifest")
        r = run("plugin", "validate", str(copy), "--json")
        errors += ok(r.returncode == 0, "plugin validates after source-note append")

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


    with tempfile.TemporaryDirectory(prefix="lto_bad_edges_") as td:
        base = Path(td) / "edge"
        subprocess.run(["cp", "-R", str(SAMPLE), str(base)], check=True)

        (base / "evil.sh").write_text("echo bad\n", encoding="utf-8")
        r = run("plugin", "validate", str(base), "--json")
        errors += ok(r.returncode != 0, "undeclared executable file rejected")
        (base / "evil.sh").unlink()

        (base / "prompts" / "bad.md").symlink_to("/etc/passwd")
        prof = json.loads((base / "profiles" / "codex-audit-readonly.json").read_text(encoding="utf-8"))
        prof["prompt_suffix_ref"] = "prompts/bad.md"
        (base / "profiles" / "codex-audit-readonly.json").write_text(json.dumps(prof), encoding="utf-8")
        r = run("plugin", "validate", str(base), "--json")
        errors += ok(r.returncode != 0 and "symlink" in r.stdout.lower(), "symlink escape rejected")
        (base / "prompts" / "bad.md").unlink()
        prof["prompt_suffix_ref"] = "prompts/codex-audit.md"
        (base / "profiles" / "codex-audit-readonly.json").write_text(json.dumps(prof), encoding="utf-8")

        manifest = json.loads((base / "plugin.json").read_text(encoding="utf-8"))
        manifest["security"]["env_allowlist"].append("PATH")
        (base / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
        r = run("plugin", "validate", str(base), "--json")
        errors += ok(r.returncode != 0 and "host-approved" in r.stdout, "host-owned env allowlist enforced")

    with tempfile.TemporaryDirectory(prefix="lto_bad_eval_") as td:
        base = Path(td) / "evalbad"
        subprocess.run(["cp", "-R", str(SAMPLE), str(base)], check=True)
        (base / "eval" / "profile-ab-cases.json").write_text("[]", encoding="utf-8")
        r = run("plugin", "eval", str(base), "--json")
        errors += ok(r.returncode != 0, "non-object eval rejected cleanly")
        eval_report = json.loads(r.stdout)
        errors += ok(any("JSON must be an object" in e or "eval must be an object" in e for e in eval_report.get("errors", [])), "non-object eval error reported")

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
