#!/usr/bin/env python3
"""Static + fake-binary tests for scripts/delegate/runners/pi.sh.

No real pi call. A fake `pi` on PATH emits canned NDJSON so we can verify
pi.sh extracts the reply text + writes a token sidecar, and falls back to raw
output when the json schema doesn't match.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNNER = ROOT / "scripts" / "delegate" / "runners" / "pi.sh"


def ok(cond: bool, msg: str) -> int:
    if cond:
        print(f"OK   {msg}")
        return 0
    print(f"FAIL {msg}", file=sys.stderr)
    return 1


def _run_with_fake_pi(td: Path, ndjson_lines: list[str]) -> tuple[Path, Path]:
    """Put a fake `pi` on PATH that prints the given NDJSON, run pi.sh."""
    fake = td / "pi"
    payload = "\n".join(ndjson_lines)
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.stdout.write({payload!r})\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    prompt = td / "prompt.txt"
    prompt.write_text("count to five", encoding="utf-8")
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
    errors += ok(RUNNER.exists(), "pi runner exists")
    text = RUNNER.read_text(encoding="utf-8")
    for needle, label in [
        ("--mode json", "uses json mode for token usage"),
        ("message_end", "reads final assistant message_end"),
        (".meta.json", "writes token sidecar"),
        ("totalTokens", "rolls up pi totalTokens"),
        ("raw fallback", "documents raw fallback"),
    ]:
        errors += ok(needle in text, label)

    # 1) happy path: assistant message_end carries text + usage
    with tempfile.TemporaryDirectory(prefix="lto_pi_ok_") as td:
        tdp = Path(td)
        lines = [
            json.dumps({"type": "turn_start"}),
            json.dumps({"type": "message_end", "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "hmm"},
                    {"type": "text", "text": "1 2 3 4 5"},
                ],
                "usage": {"input": 100, "output": 20, "totalTokens": 5000},
            }}),
        ]
        reply, meta = _run_with_fake_pi(tdp, lines)
        errors += ok(reply.exists() and reply.read_text().strip() == "1 2 3 4 5",
                     "extracts reply text from message_end (not raw json)")
        if meta.exists():
            m = json.loads(meta.read_text())
            errors += ok(m.get("tokens_in") == 100 and m.get("tokens_out") == 20
                         and m.get("tokens") == 5000,
                         "sidecar carries tokens_in/out + totalTokens rollup")
        else:
            errors += ok(False, "sidecar written")

    # 2) fallback: non-json output → reply falls back to raw, no sidecar
    with tempfile.TemporaryDirectory(prefix="lto_pi_fb_") as td:
        tdp = Path(td)
        reply, meta = _run_with_fake_pi(tdp, ["this is not json at all"])
        errors += ok(reply.exists() and "not json" in reply.read_text(),
                     "falls back to raw output when json unparseable")
        errors += ok(not meta.exists(), "no sidecar when usage absent")

    # 3) reply present but usage missing → reply written, no sidecar
    with tempfile.TemporaryDirectory(prefix="lto_pi_nou_") as td:
        tdp = Path(td)
        lines = [json.dumps({"type": "message_end", "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "ok done"}],
        }})]
        reply, meta = _run_with_fake_pi(tdp, lines)
        errors += ok(reply.read_text().strip() == "ok done", "reply extracted without usage")
        errors += ok(not meta.exists(), "no sidecar when message has no usage")

    # 4) B1 回归：解析出空白 reply → 保留空，不 fallback 污染成 raw NDJSON
    with tempfile.TemporaryDirectory(prefix="lto_pi_empty_") as td:
        tdp = Path(td)
        lines = [json.dumps({"type": "message_end", "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "   "}],  # 纯空白
            "usage": {"input": 10, "output": 0, "totalTokens": 10},
        }})]
        reply, meta = _run_with_fake_pi(tdp, lines)
        # reply 文件存在且为空（不被 raw NDJSON 污染）
        content = reply.read_text() if reply.exists() else "MISSING"
        errors += ok("message_end" not in content and "usage" not in content,
                     "B1: empty reply not polluted by raw NDJSON")

    # 5) B2 回归：turn_end 跟在 message_end 后，不覆盖 message_end 的 usage
    with tempfile.TemporaryDirectory(prefix="lto_pi_te_") as td:
        tdp = Path(td)
        lines = [
            json.dumps({"type": "message_end", "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "real answer"}],
                "usage": {"input": 50, "output": 10, "totalTokens": 3000},
            }}),
            # turn_end usage 不完整（缺 input/output）——旧逻辑会覆盖成坏数据
            json.dumps({"type": "turn_end", "message": {
                "role": "assistant", "content": [], "usage": {"totalTokens": 0},
            }}),
        ]
        reply, meta = _run_with_fake_pi(tdp, lines)
        errors += ok(reply.read_text().strip() == "real answer", "B2: reply from message_end kept")
        if meta.exists():
            m = json.loads(meta.read_text())
            errors += ok(m.get("tokens_in") == 50 and m.get("tokens") == 3000,
                         "B2: message_end usage not clobbered by turn_end")
        else:
            errors += ok(False, "B2: sidecar written from message_end")

    # 6) 多轮：保留最后一个 assistant message_end 的 reply
    with tempfile.TemporaryDirectory(prefix="lto_pi_multi_") as td:
        tdp = Path(td)
        lines = [
            json.dumps({"type": "message_end", "message": {
                "role": "assistant", "content": [{"type": "text", "text": "first"}]}}),
            json.dumps({"type": "message_end", "message": {
                "role": "assistant", "content": [{"type": "text", "text": "second"}]}}),
        ]
        reply, _ = _run_with_fake_pi(tdp, lines)
        errors += ok(reply.read_text().strip() == "second", "keeps last assistant message of multi-turn")

    if errors:
        print(f"\n{errors} FAILURES", file=sys.stderr)
        return 1
    print("\nPI RUNNER TESTS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
