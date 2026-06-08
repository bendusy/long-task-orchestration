#!/usr/bin/env python3
"""test_live_log.py — live log 可观测改造验收测试。

覆盖：
1. happy path：fake runner 写 stdout → live log 存在且非空，reply 正常，status OK
2. stall 检测：fake runner sleep 且不输出 → stall_timeout 小值 → exit_code=124，提前杀
3. 空 stdout（只写 reply）→ 不误报 stall，正常完成
4. recap 显示当前在跑；无 live/ 不显示
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# 让 scripts/ 目录进 sys.path
SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from lto.agent_job import AgentJob, Budget, RetryPolicy
from lto.scheduler import Scheduler
from lto import state as st
from lto.agent_job import JobStatus


# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------

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


def _make_repo(root: Path) -> Path:
    """创建带 git 初始化的仓库目录（供 lto/current 路径解析用）。"""
    repo = root / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.name=T", "-c", "user.email=t@x.com",
         "commit", "-q", "--allow-empty", "-m", "init"],
        cwd=repo, capture_output=True,
    )
    return repo


def _make_runners(root: Path, runner_script: str) -> Path:
    """创建 runners/ 目录，fake runner 用 runner_script 内容（Python 脚本）。"""
    runners = root / "runners"
    runners.mkdir(exist_ok=True)

    fake_py = root / "fake_runner.py"
    fake_py.write_text(runner_script, encoding="utf-8")
    fake_py.chmod(0o755)

    for name in ("codex", "pi", "agy"):
        sh = runners / f"{name}.sh"
        sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake_py}" "$@"\n', encoding="utf-8")
        sh.chmod(0o755)

    hc = runners / "healthcheck.sh"
    verdicts = ','.join('{"agent":"' + n + '","verdict":"OK"}' for n in ("codex", "pi", "agy"))
    hc.write_text(f'#!/usr/bin/env bash\necho \'[{verdicts}]\'\nexit 0\n', encoding="utf-8")
    hc.chmod(0o755)

    return runners


def _make_job(job_id: str, runner: str = "codex", timeout: int = 30, retries: int = 0) -> AgentJob:
    return AgentJob(
        job_id=job_id,
        runner=runner,
        prompt_ref=f"# JOB_ID:{job_id}\ntest",
        prompt_is_inline=True,
        budget=Budget(timeout_sec=timeout),
        retry_policy=RetryPolicy(max_retries=retries),
    )


def _write_lto_current(repo: Path, run_id: str) -> Path:
    """写 .lto/current，让 Scheduler 能自动推导 live log 路径。"""
    lto_dir = repo / ".lto" / run_id
    lto_dir.mkdir(parents=True, exist_ok=True)
    (repo / ".lto" / "current").write_text(run_id + "\n", encoding="utf-8")
    return lto_dir


# ---------------------------------------------------------------------------
# Test 1：happy path — stdout 写到 live log
# ---------------------------------------------------------------------------

def test_happy_path(c: _Counter) -> None:
    print("\n[1] happy path：fake runner 写 stdout → live log 存在且非空")
    runner_script = '''\
#!/usr/bin/env python3
import sys, time
# stdout 写可见内容（这会进 live log）
print("hello from fake runner", flush=True)
print("second line", flush=True)
# reply 文件写回复
reply_file = sys.argv[2]
with open(reply_file, "w") as f:
    f.write("job reply ok")
sys.exit(0)
'''
    with tempfile.TemporaryDirectory(prefix="lto_live_t1_") as tmp:
        root = Path(tmp)
        repo = _make_repo(root)
        runners = _make_runners(root, runner_script)
        run_id = "test-run-t1"
        _write_lto_current(repo, run_id)

        sched = Scheduler(repo=repo, runners_dir=runners, run_id=run_id, stall_timeout=0)
        results = sched.submit([_make_job("t1_job1")])
        r = results[0]

        # 验证 reply 正常
        c.ok("status OK") if r.status == JobStatus.OK.value else c.fail("status OK", r.status)

        # 验证 live log 存在且非空
        live_log = repo / ".lto" / run_id / "live" / "t1_job1.log"
        if live_log.exists() and live_log.stat().st_size > 0:
            content = live_log.read_bytes()
            c.ok(f"live log 非空 ({live_log.stat().st_size} bytes)")
            if b"hello from fake runner" in content:
                c.ok("live log 内容正确（含 stdout）")
            else:
                c.fail("live log 内容正确", f"got: {content[:100]!r}")
        else:
            c.fail("live log 存在且非空", f"exists={live_log.exists()}")

        # reply 正确
        c.ok("reply 非空") if r.reply_text else c.fail("reply 非空", repr(r.reply_text))


# ---------------------------------------------------------------------------
# Test 2：stall 检测 — fake runner sleep 不输出，stall_timeout 小 → 提前杀
# ---------------------------------------------------------------------------

def test_stall_detection(c: _Counter) -> None:
    print("\n[2] stall 检测：fake runner sleep 不输出 → 提前杀，exit_code=124")
    runner_script = '''\
#!/usr/bin/env python3
import sys, time
# 写一点 stdout 然后停止输出，触发 stall
print("starting...", flush=True)
# sleep 很久，模拟卡死
time.sleep(60)
reply_file = sys.argv[2]
with open(reply_file, "w") as f:
    f.write("never reached")
sys.exit(0)
'''
    with tempfile.TemporaryDirectory(prefix="lto_live_t2_") as tmp:
        root = Path(tmp)
        repo = _make_repo(root)
        runners = _make_runners(root, runner_script)
        run_id = "test-run-t2"
        _write_lto_current(repo, run_id)

        # stall_timeout=3s，timeout_total 远大于此
        sched = Scheduler(repo=repo, runners_dir=runners, run_id=run_id, stall_timeout=3)
        t0 = time.monotonic()
        results = sched.submit([_make_job("t2_stall", timeout=120)])
        elapsed = time.monotonic() - t0
        r = results[0]

        # exit_code 应为 124（stall 杀）
        if r.exit_code == 124:
            c.ok(f"exit_code=124（stall 杀）")
        else:
            c.fail("exit_code=124", f"got exit_code={r.exit_code} status={r.status}")

        # elapsed 应远小于 timeout_total（120s）
        if elapsed < 30:
            c.ok(f"提前终止（elapsed={elapsed:.1f}s < 30s）")
        else:
            c.fail("提前终止", f"elapsed={elapsed:.1f}s，未提前杀")


# ---------------------------------------------------------------------------
# Test 3：空 stdout（只写 reply）→ 不误报 stall，正常完成
# ---------------------------------------------------------------------------

def test_no_stdout_no_stall(c: _Counter) -> None:
    print("\n[3] 空 stdout（只写 reply）→ 不误报 stall，正常完成")
    runner_script = '''\
#!/usr/bin/env python3
import sys
# 不写任何 stdout（live log 为空），但 reply 写好
reply_file = sys.argv[2]
with open(reply_file, "w") as f:
    f.write("silent job done")
sys.exit(0)
'''
    with tempfile.TemporaryDirectory(prefix="lto_live_t3_") as tmp:
        root = Path(tmp)
        repo = _make_repo(root)
        runners = _make_runners(root, runner_script)
        run_id = "test-run-t3"
        _write_lto_current(repo, run_id)

        # stall_timeout=3s，但 job 很快完成（不应触发 stall）
        sched = Scheduler(repo=repo, runners_dir=runners, run_id=run_id, stall_timeout=3)
        t0 = time.monotonic()
        results = sched.submit([_make_job("t3_silent", timeout=30)])
        elapsed = time.monotonic() - t0
        r = results[0]

        if r.status == JobStatus.OK.value:
            c.ok(f"status OK（无误报 stall，elapsed={elapsed:.2f}s）")
        else:
            c.fail("status OK（无误报 stall）", f"status={r.status} error={r.error} elapsed={elapsed:.2f}s")


# ---------------------------------------------------------------------------
# Test 4a：recap 显示当前在跑（有 live log 且 mtime 新鲜）
# ---------------------------------------------------------------------------

def test_recap_shows_running(c: _Counter) -> None:
    print("\n[4a] recap 显示在跑：有 live log mtime 新鲜")
    from lto.commands.recap import _render_recap, _running_jobs

    with tempfile.TemporaryDirectory(prefix="lto_live_t4a_") as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        run_id = "recap-run-1"
        live_dir = repo / ".lto" / run_id / "live"
        live_dir.mkdir(parents=True)

        # 写一个 mtime=now 的 live log
        log_file = live_dir / "job_abc.log"
        log_file.write_bytes(b"some output\n")

        running = _running_jobs(repo, run_id, window_sec=120)
        if running and "job_abc" in running[0]:
            c.ok(f"_running_jobs 返回正确：{running}")
        else:
            c.fail("_running_jobs 返回正确", f"got: {running}")

        # 验证 _render_recap 包含"当前在跑"行
        state = {
            "schema_version": 1,
            "run_id": run_id,
            "goal": "test recap running",
            "started_at": "",
            "current_phase": "execute",
            "tasks": [],
        }
        out = _render_recap(state, run_id, repo=repo)
        if "当前在跑" in out and "job_abc" in out:
            c.ok("recap 输出含'当前在跑'和 job_id")
        else:
            c.fail("recap 含当前在跑", f"output:\n{out}")


# ---------------------------------------------------------------------------
# Test 4b：recap 无 live/ 目录 → 不显示"当前在跑"
# ---------------------------------------------------------------------------

def test_recap_no_live_dir(c: _Counter) -> None:
    print("\n[4b] recap 无 live/ 目录 → 不显示当前在跑")
    from lto.commands.recap import _render_recap, _running_jobs

    with tempfile.TemporaryDirectory(prefix="lto_live_t4b_") as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        run_id = "recap-run-nodir"
        # 故意不建 live/ 目录

        running = _running_jobs(repo, run_id, window_sec=120)
        if running == []:
            c.ok("_running_jobs 返回空列表（无 live/ 目录）")
        else:
            c.fail("_running_jobs 返回空", f"got: {running}")

        state = {
            "schema_version": 1,
            "run_id": run_id,
            "goal": "test no live dir",
            "started_at": "",
            "current_phase": "execute",
            "tasks": [],
        }
        out = _render_recap(state, run_id, repo=repo)
        if "当前在跑" not in out:
            c.ok("recap 不含'当前在跑'（正确降级）")
        else:
            c.fail("recap 不含当前在跑", f"output:\n{out}")


# ---------------------------------------------------------------------------
# Test 4c：recap 有 live/ 但 mtime 过期 → 不显示
# ---------------------------------------------------------------------------

def test_recap_stale_log(c: _Counter) -> None:
    print("\n[4c] recap live log mtime 过期 → 不显示")
    from lto.commands.recap import _running_jobs

    with tempfile.TemporaryDirectory(prefix="lto_live_t4c_") as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        run_id = "recap-run-stale"
        live_dir = repo / ".lto" / run_id / "live"
        live_dir.mkdir(parents=True)

        log_file = live_dir / "old_job.log"
        log_file.write_bytes(b"old output\n")
        # 设置 mtime 为 300 秒前
        old_mtime = time.time() - 300
        os.utime(log_file, (old_mtime, old_mtime))

        running = _running_jobs(repo, run_id, window_sec=120)
        if running == []:
            c.ok("过期 log 不进'在跑'列表")
        else:
            c.fail("过期 log 不进在跑", f"got: {running}")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> int:
    c = _Counter()

    test_happy_path(c)
    test_stall_detection(c)
    test_no_stdout_no_stall(c)
    test_recap_shows_running(c)
    test_recap_no_live_dir(c)
    test_recap_stale_log(c)

    print(f"\n{'='*50}")
    print(f"test_live_log: {c.passed}/{c.total} passed")
    return 0 if c.passed == c.total else 1


if __name__ == "__main__":
    sys.exit(main())
