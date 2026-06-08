#!/usr/bin/env python3
"""Static + fake-binary tests for scripts/delegate/runners/claude.sh.

No real claude call. A fake `claude` on PATH emits a canned --output-format
json envelope so we can verify claude.sh extracts the reply (result field) +
writes a token sidecar, and falls back to raw output when the schema mismatches.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts" / "delegate" / "runners" / "claude.sh"


def ok(cond: bool, msg: str) -> int:
    if cond:
        print(f"OK   {msg}")
        return 0
    print(f"FAIL {msg}", file=sys.stderr)
    return 1


def _run_with_fake_claude(td: Path, stdout: str) -> tuple[Path, Path]:
    fake = td / "claude"
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.stdout.write({stdout!r})\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    prompt = td / "prompt.txt"
    prompt.write_text("2+2?", encoding="utf-8")
    reply = td / "reply.txt"
    env = dict(os.environ)
    env["PATH"] = f"{td}:{env['PATH']}"
    subprocess.run(
        ["bash", str(RUNNER), str(prompt), str(reply), "30"],
        env=env, capture_output=True, cwd=td,
    )
    return reply, reply.with_name(reply.name + ".meta.json")


def main() -> int:
    errors = 0
    errors += ok(RUNNER.exists(), "claude runner exists")
    text = RUNNER.read_text(encoding="utf-8")
    for needle, label in [
        ("--output-format json", "uses json output format"),
        ('"result"', "reads result field as reply"),
        (".meta.json", "writes token sidecar"),
        ("cache_creation_input_tokens", "rolls cache tokens into total"),
        ("PARSED_FLAG", "sentinel guards raw fallback"),
    ]:
        errors += ok(needle in text, label)

    # 1) happy path: result + usage
    with tempfile.TemporaryDirectory(prefix="lto_cl_ok_") as td:
        tdp = Path(td)
        env = {
            "type": "result", "subtype": "success", "result": "Four.",
            "usage": {
                "input_tokens": 100, "output_tokens": 5,
                "cache_creation_input_tokens": 200, "cache_read_input_tokens": 50,
            },
        }
        reply, meta = _run_with_fake_claude(tdp, json.dumps(env))
        errors += ok(reply.exists() and reply.read_text().strip() == "Four.",
                     "extracts result text (not raw json)")
        if meta.exists():
            m = json.loads(meta.read_text())
            errors += ok(m.get("tokens_in") == 100 and m.get("tokens_out") == 5
                         and m.get("tokens") == 355,  # 100+5+200+50
                         "sidecar rolls up in/out + cache into tokens")
        else:
            errors += ok(False, "sidecar written")

    # 2) fallback: non-json / no result envelope → raw, no sidecar
    with tempfile.TemporaryDirectory(prefix="lto_cl_fb_") as td:
        tdp = Path(td)
        reply, meta = _run_with_fake_claude(tdp, "plain non-json output")
        errors += ok(reply.exists() and "non-json" in reply.read_text(),
                     "falls back to raw when not the json envelope")
        errors += ok(not meta.exists(), "no sidecar on fallback")

    # 3) result present, usage absent → reply written, no sidecar
    with tempfile.TemporaryDirectory(prefix="lto_cl_nou_") as td:
        tdp = Path(td)
        reply, meta = _run_with_fake_claude(tdp, json.dumps({"type": "result", "result": "ok"}))
        errors += ok(reply.read_text().strip() == "ok", "reply extracted without usage")
        errors += ok(not meta.exists(), "no sidecar when usage absent")

    # 4) empty result → kept empty, not polluted by raw json (sentinel)
    with tempfile.TemporaryDirectory(prefix="lto_cl_empty_") as td:
        tdp = Path(td)
        reply, _ = _run_with_fake_claude(tdp, json.dumps({"type": "result", "result": ""}))
        content = reply.read_text() if reply.exists() else "MISSING"
        errors += ok("result" not in content and "usage" not in content,
                     "empty result not polluted by raw json")

    if errors:
        print(f"\n{errors} FAILURES", file=sys.stderr)
        return 1
    print("\nCLAUDE RUNNER TESTS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
