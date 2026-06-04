#!/usr/bin/env python3
"""Static+fake-binary tests for scripts/delegate/runners/codex.sh.

No real Codex call. The fake binary records argv/stdin and honors -o.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts" / "delegate" / "runners" / "codex.sh"


def ok(cond: bool, msg: str) -> int:
    if cond:
        print(f"OK   {msg}")
        return 0
    print(f"FAIL {msg}", file=sys.stderr)
    return 1


def main() -> int:
    errors = 0
    errors += ok(RUNNER.exists(), "codex runner exists")
    text = RUNNER.read_text(encoding="utf-8")
    for needle, label in [
        ("codex exec --help", "probes exec help"),
        ("CODEX_SANDBOX:-read-only", "defaults to read-only"),
        ("-C", "passes workdir"),
        ("-s", "passes sandbox"),
        ("-o", "writes final message file"),
        ("< \"$PROMPT_FILE\"", "uses stdin prompt"),
        ("CODEX_IMAGES", "supports image attachment env"),
    ]:
        errors += ok(needle in text, label)

    with tempfile.TemporaryDirectory(prefix="lto_codex_runner_") as td:
        tmp = Path(td)
        fake = tmp / "fake_codex.py"
        log = tmp / "log.json"
        fake.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "args = sys.argv[1:]\n"
            "stdin = sys.stdin.read()\n"
            "if args[:2] == ['exec', '--help']:\n"
            "    print('fake codex exec help')\n"
            "    raise SystemExit(0)\n"
            "out = None\n"
            "for i,a in enumerate(args):\n"
            "    if a in ('-o','--output-last-message') and i+1 < len(args):\n"
            "        out = args[i+1]\n"
            "if out:\n"
            "    open(out, 'w').write('fake final reply')\n"
            "open(os.environ['FAKE_CODEX_LOG'], 'w').write(json.dumps({'args': args, 'stdin': stdin}))\n"
            "raise SystemExit(0)\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        prompt = tmp / "prompt.md"
        reply = tmp / "reply.md"
        prompt.write_text("Goal:\nReview only.\n", encoding="utf-8")
        env = {
            **os.environ,
            "CODEX_BIN": str(fake),
            "CODEX_WORKDIR": "/tmp/lto-workdir",
            "CODEX_SANDBOX": "workspace-write",
            "CODEX_MODEL": "gpt-test",
            "CODEX_PROFILE": "lto-test",
            "CODEX_IMAGES": "before.png,after.png",
            "FAKE_CODEX_LOG": str(log),
        }
        proc = subprocess.run(
            ["bash", str(RUNNER), str(prompt), str(reply), "30"],
            capture_output=True,
            text=True,
            env=env,
            timeout=40,
        )
        errors += ok(proc.returncode == 0, f"fake codex run rc=0 (got {proc.returncode})")
        errors += ok(reply.read_text(encoding="utf-8") == "fake final reply", "reply written via -o")
        data = json.loads(log.read_text(encoding="utf-8"))
        args = data["args"]
        errors += ok(args[0] == "exec", "uses codex exec")
        errors += ok("--skip-git-repo-check" in args, "allows external cwd")
        errors += ok("-C" in args and args[args.index("-C") + 1] == "/tmp/lto-workdir", "passes CODEX_WORKDIR")
        errors += ok("-s" in args and args[args.index("-s") + 1] == "workspace-write", "passes CODEX_SANDBOX")
        errors += ok("-m" in args and args[args.index("-m") + 1] == "gpt-test", "passes CODEX_MODEL")
        errors += ok("-p" in args and args[args.index("-p") + 1] == "lto-test", "passes CODEX_PROFILE")
        errors += ok(args.count("-i") == 2, "passes repeated image attachments")
        errors += ok(args[-1] == "-", "uses '-' stdin prompt")
        errors += ok(data["stdin"] == "Goal:\nReview only.\n", "prompt piped on stdin")

    if errors == 0:
        print("\nCODEX RUNNER TESTS OK")
        return 0
    print(f"\n{errors} FAILURES", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
