#!/usr/bin/env python3
"""test_heartbeat.py — P0-1 层 1 验收：结构化心跳 + `runs --watch` 汇总。

覆盖：
1. format_heartbeat 纯函数：字段齐全、JSON 可解析、elapsed/alive 正确。
2. heartbeat_path：从 live log 路径推导 .hb.jsonl 旁车路径。
3. read_last_heartbeat：多行 JSONL 取最后一条有效，坏行不崩。
4. scan_live_heartbeats：构造假 live 目录 → 每个在跑 job 一行汇总
   （runner / elapsed / 最后心跳距今 / reply 是否就绪）。
5. format_watch_table：汇总行渲染含关键字段。
6. runs --watch e2e：CLI 跑通，输出含构造的 job。
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from lto.heartbeat import (
    format_heartbeat,
    heartbeat_path,
    read_last_heartbeat,
    scan_live_heartbeats,
    format_watch_table,
)


class _Counter:
    def __init__(self) -> None:
        self.passed = 0
        self.total = 0

    def ok(self, label: str) -> None:
        self.passed += 1
        self.total += 1
        print(f"  OK  {label}")

    def fail(self, label: str, detail: str = "") -> None:
        self.total += 1
        print(f"  FAIL {label}" + (f": {detail}" if detail else ""))

    def expect(self, cond: bool, label: str, detail: str = "") -> None:
        self.ok(label) if cond else self.fail(label, detail)


def test_format_heartbeat(c: _Counter) -> None:
    line = format_heartbeat(
        ts=1000.0, job_id="job-a", runner="pi", elapsed_sec=42.5,
        phase="running", alive=True,
    )
    c.expect(isinstance(line, str), "format_heartbeat returns str")
    c.expect(not line.endswith("\n"), "format_heartbeat has no trailing newline (caller adds it)")
    try:
        obj = json.loads(line)
    except Exception as e:
        c.fail("format_heartbeat emits valid JSON", str(e))
        return
    for field in ("ts", "job_id", "runner", "elapsed_sec", "phase", "alive"):
        c.expect(field in obj, f"heartbeat has field '{field}'")
    c.expect(obj["job_id"] == "job-a", "job_id correct")
    c.expect(obj["runner"] == "pi", "runner correct")
    c.expect(obj["elapsed_sec"] == 42.5, "elapsed_sec correct", str(obj.get("elapsed_sec")))
    c.expect(obj["alive"] is True, "alive correct")
    c.expect(obj["phase"] == "running", "phase correct")

    # elapsed rounding stable
    line2 = format_heartbeat(ts=1.0, job_id="j", runner="codex",
                             elapsed_sec=3.14159, phase="running", alive=True)
    c.expect(json.loads(line2)["elapsed_sec"] == 3.142, "elapsed_sec rounded to 3 places")


def test_heartbeat_path(c: _Counter) -> None:
    p = heartbeat_path(Path("/tmp/x/.lto/run-1/live/job-a.log"))
    c.expect(p == Path("/tmp/x/.lto/run-1/live/job-a.hb.jsonl"),
             "heartbeat_path swaps .log -> .hb.jsonl", str(p))
    c.expect(heartbeat_path(None) is None, "heartbeat_path(None) is None")


def test_read_last_heartbeat(c: _Counter, tmp: Path) -> None:
    hb = tmp / "j.hb.jsonl"
    hb.write_text(
        format_heartbeat(ts=1.0, job_id="j", runner="pi", elapsed_sec=10.0,
                         phase="running", alive=True) + "\n"
        + "this is not json\n"
        + format_heartbeat(ts=2.0, job_id="j", runner="pi", elapsed_sec=40.0,
                           phase="running", alive=True) + "\n",
        encoding="utf-8",
    )
    last = read_last_heartbeat(hb)
    c.expect(last is not None, "read_last_heartbeat finds a record despite a bad line")
    c.expect(last and last["elapsed_sec"] == 40.0, "read_last_heartbeat returns the latest record")

    c.expect(read_last_heartbeat(tmp / "missing.hb.jsonl") is None,
             "read_last_heartbeat(missing) is None")

    empty = tmp / "empty.hb.jsonl"
    empty.write_text("", encoding="utf-8")
    c.expect(read_last_heartbeat(empty) is None, "read_last_heartbeat(empty) is None")


def _make_run(repo: Path, run_id: str) -> Path:
    live = repo / ".lto" / run_id / "live"
    live.mkdir(parents=True, exist_ok=True)
    return live


def test_scan_live_heartbeats(c: _Counter, tmp: Path) -> None:
    repo = tmp / "repo"
    run_id = "20260615-demo"
    live = _make_run(repo, run_id)
    now = 1000.0

    # job-a: fresh heartbeat, no reply yet
    (live / "job-a.hb.jsonl").write_text(
        format_heartbeat(ts=now - 5, job_id="job-a", runner="pi",
                         elapsed_sec=35.0, phase="running", alive=True) + "\n",
        encoding="utf-8",
    )
    # job-b: stale heartbeat (90s ago), reply file present (ready)
    (live / "job-b.hb.jsonl").write_text(
        format_heartbeat(ts=now - 90, job_id="job-b", runner="codex",
                         elapsed_sec=120.0, phase="running", alive=True) + "\n",
        encoding="utf-8",
    )
    (live / "job-b.reply.txt").write_text("done", encoding="utf-8")

    rows = scan_live_heartbeats(repo, run_id, now=now)
    c.expect(len(rows) == 2, "scan finds both jobs", f"got {len(rows)}")
    by_id = {r["job_id"]: r for r in rows}

    a = by_id.get("job-a")
    c.expect(a is not None, "job-a present")
    if a:
        c.expect(a["runner"] == "pi", "job-a runner=pi")
        c.expect(a["elapsed_sec"] == 35.0, "job-a elapsed=35")
        c.expect(abs(a["last_hb_age_sec"] - 5.0) < 0.01, "job-a last_hb_age≈5",
                 str(a["last_hb_age_sec"]))
        c.expect(a["reply_ready"] is False, "job-a reply not ready")

    b = by_id.get("job-b")
    c.expect(b is not None, "job-b present")
    if b:
        c.expect(abs(b["last_hb_age_sec"] - 90.0) < 0.01, "job-b last_hb_age≈90")
        c.expect(b["reply_ready"] is True, "job-b reply ready")

    # auto-resolve run_id from .lto/current when not passed
    (repo / ".lto" / "current").write_text(run_id, encoding="utf-8")
    rows2 = scan_live_heartbeats(repo, None, now=now)
    c.expect(len(rows2) == 2, "scan resolves run_id from .lto/current")

    # no live dir → empty, no crash
    rows3 = scan_live_heartbeats(repo, "nonexistent-run", now=now)
    c.expect(rows3 == [], "scan(nonexistent run) returns []")


def test_format_watch_table(c: _Counter) -> None:
    rows = [
        {"job_id": "job-a", "runner": "pi", "elapsed_sec": 35.0,
         "last_hb_age_sec": 5.0, "reply_ready": False, "alive": True},
        {"job_id": "job-b", "runner": "codex", "elapsed_sec": 120.0,
         "last_hb_age_sec": 90.0, "reply_ready": True, "alive": True},
    ]
    out = format_watch_table(rows)
    c.expect("job-a" in out and "job-b" in out, "table mentions both jobs")
    c.expect("pi" in out and "codex" in out, "table mentions runners")
    c.expect("35" in out, "table mentions elapsed")

    empty_out = format_watch_table([])
    c.expect(isinstance(empty_out, str) and len(empty_out) > 0,
             "empty table renders a non-empty hint")


def test_runs_watch_cli(c: _Counter, tmp: Path) -> None:
    repo = tmp / "cli_repo"
    run_id = "20260615-cli"
    live = _make_run(repo, run_id)
    # need state.json so the run is "real" for runs, plus current pointer
    (repo / ".lto" / run_id / "state.json").write_text(
        json.dumps({"goal": "demo", "current_phase": "develop", "tasks": []}),
        encoding="utf-8",
    )
    (repo / ".lto" / "current").write_text(run_id, encoding="utf-8")
    now = time.time()
    (live / "job-x.hb.jsonl").write_text(
        format_heartbeat(ts=now - 3, job_id="job-x", runner="agy",
                         elapsed_sec=12.0, phase="running", alive=True) + "\n",
        encoding="utf-8",
    )
    proc = subprocess.run(
        [sys.executable, str(SCRIPTS_DIR / "lto_run.py"),
         "--repo", str(repo), "runs", "--watch", "--once"],
        capture_output=True, text=True, timeout=20,
    )
    c.expect(proc.returncode == 0, "runs --watch --once rc=0",
             proc.stderr[-500:])
    c.expect("job-x" in proc.stdout, "runs --watch output includes the running job",
             proc.stdout[-500:])
    c.expect("agy" in proc.stdout, "runs --watch output includes the runner")


def main() -> int:
    import tempfile

    c = _Counter()
    test_format_heartbeat(c)
    test_heartbeat_path(c)
    test_format_watch_table(c)
    with tempfile.TemporaryDirectory() as d:
        tmp = Path(d)
        test_read_last_heartbeat(c, tmp)
        test_scan_live_heartbeats(c, tmp)
        test_runs_watch_cli(c, tmp)

    print(f"\n{c.passed}/{c.total} passed")
    return 0 if c.passed == c.total else 1


if __name__ == "__main__":
    sys.exit(main())
