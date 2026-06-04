#!/usr/bin/env python3
"""高价值命令端到端测试：task-add / judge / recap / memory。

B4（2026-06-04-spec-lto-gap-closure）：覆盖高价值命令的真实 CLI 边界，
复用 selftest 的临时 git 仓库 + lto() 子进程 helper 模式（不用 pytest，
standalone runner + FAIL 累加 + sys.exit）。runner 行为在 task-add 路径里覆盖。

parallel/pipeline 暂不做（YAGNI）。memory 覆盖 export/resume/publish
边界，防止 artifact-memory 第一片回归。

跑法：
  cd skills/long-task-orchestration/scripts && python3 test_orchestration_cmds.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

_SCRIPT_DIR = Path(__file__).resolve().parent
LTO_PATH = _SCRIPT_DIR / "lto_run.py"

FAIL: list[str] = []


def ok(cond: bool, msg: str) -> None:
    print(("OK   " if cond else "FAIL ") + msg, file=sys.stdout if cond else sys.stderr)
    if not cond:
        FAIL.append(msg)


def _make_repo(tmp: Path) -> Path:
    """临时隔离 git 仓库（占位身份，不污染真实 blame，不会 push）。"""
    repo = tmp
    subprocess.run(["git", "init"], cwd=repo, capture_output=True)
    (repo / "README.md").write_text("test\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=repo, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid",
         "commit", "-m", "init"],
        cwd=repo, capture_output=True,
    )
    return repo


def _lto_factory(repo: Path):
    def lto(*args: str) -> subprocess.CompletedProcess:
        env = os.environ.copy()
        for key in ("MEMORY_FLOW_URL", "MEMORY_FLOW_TOKEN", "MEMORY_FLOW_AGENT_ID"):
            env.pop(key, None)
        return subprocess.run(
            [sys.executable, str(LTO_PATH), "--repo", str(repo)] + list(args),
            cwd=repo, capture_output=True, text=True, env=env,
        )
    return lto


def _state_of(repo: Path, run_id: str) -> dict:
    return json.loads((repo / ".lto" / run_id / "state.json").read_text(encoding="utf-8"))


def _manifest_of(repo: Path, run_id: str) -> dict:
    return json.loads((repo / ".lto" / run_id / "artifacts.json").read_text(encoding="utf-8"))


def test_task_add(repo: Path) -> None:
    """start 起 run → task-add → 断言 state.json tasks 含 T1 + commands_run 含命令；重复 id 拒绝。"""
    lto = _lto_factory(repo)

    r = lto("start", "--goal", "task-add e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"task_add: start rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    run_id = r.stdout.strip().split("/")[-1]
    manifest = _manifest_of(repo, run_id)
    kinds = {e.get("kind") for e in manifest.get("artifacts", [])}
    ok({"state_json", "run_state_md"}.issubset(kinds),
       f"task_add: start writes artifact manifest core entries (got {sorted(kinds)})")

    r = lto("task-add", "--task-id", "T1", "--title", "do work", "--command", "echo hi")
    ok(r.returncode == 0, f"task_add: task-add rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    ok("T1" in r.stdout and "added" in r.stdout,
       f"task_add: stdout confirms add (got: {r.stdout.strip()[:120]})")

    state = _state_of(repo, run_id)
    t1 = next((t for t in state.get("tasks", []) if t.get("id") == "T1"), None)
    ok(t1 is not None, "task_add: state.json tasks contains T1")
    if t1 is not None:
        ok(t1.get("title") == "do work", f"task_add: T1 title preserved (got {t1.get('title')!r})")
        ok("echo hi" in t1.get("commands_run", []),
           f"task_add: commands_run contains 'echo hi' (got {t1.get('commands_run')!r})")

    r = lto("runner", "--task-id", "T1", "--kind", "test",
            "--command", "printf stdout-text; printf stderr-text >&2",
            "--note", "artifact manifest e2e")
    ok(r.returncode == 0, f"task_add: runner rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    manifest = _manifest_of(repo, run_id)
    kinds = {e.get("kind") for e in manifest.get("artifacts", [])}
    ok("evidence_stdout" in kinds and "evidence_stderr" in kinds,
       f"task_add: runner stdout/stderr registered in manifest (got {sorted(kinds)})")

    # 重复同 id → 拒绝（非零退出，task 不重复追加，防 runner/next 选错对象）
    r = lto("task-add", "--task-id", "T1", "--title", "dup", "--command", "echo bye")
    ok(r.returncode != 0, f"task_add: duplicate id refused (rc={r.returncode}, expect nonzero)")
    ok("already exists" in (r.stderr + r.stdout),
       f"task_add: duplicate error message present (got: {(r.stderr or r.stdout).strip()[:120]})")
    state = _state_of(repo, run_id)
    n_t1 = sum(1 for t in state.get("tasks", []) if t.get("id") == "T1")
    ok(n_t1 == 1, f"task_add: T1 still single after dup attempt (got {n_t1})")
    # 重复尝试不得篡改原 task（title/command 不被 dup 的值覆盖）
    t1 = next((t for t in state.get("tasks", []) if t.get("id") == "T1"), {})
    ok(t1.get("title") == "do work" and "echo bye" not in t1.get("commands_run", []),
       "task_add: original T1 untouched by duplicate attempt")


def test_judge(repo: Path) -> None:
    """judge 基本路径：无可审 task 优雅返回；有 done task 出 verdict YAML。"""
    lto = _lto_factory(repo)

    r = lto("start", "--goal", "judge e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"judge: start rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    run_id = r.stdout.strip().split("/")[-1]

    # 无 task → 优雅返回 rc=0，不崩
    r = lto("judge")
    ok(r.returncode == 0, f"judge: empty rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    ok("no tasks to judge" in r.stdout,
       f"judge: empty path reports no tasks (got: {r.stdout.strip()[:120]})")

    # 加一个 task 并标 done → judge 出 verdict
    r = lto("task-add", "--task-id", "T1", "--title", "do work", "--command", "echo hi")
    ok(r.returncode == 0, f"judge: task-add rc=0 (got {r.returncode})")
    sp = repo / ".lto" / run_id / "state.json"
    state = json.loads(sp.read_text(encoding="utf-8"))
    state["tasks"][0]["status"] = "done"
    sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    r = lto("judge")
    ok(r.returncode == 0, f"judge: with done task rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    ok("# LTO Judge Verdict" in r.stdout, "judge: verdict header present")
    ok("verdict: pass" in r.stdout,
       f"judge: clean done task yields pass verdict (got: {r.stdout.strip()[-200:]})")
    ok("tasks_reviewed: 1" in r.stdout, "judge: tasks_reviewed count present")
    # verdict 文件落盘
    judge_dir = repo / ".lto" / run_id / "judge"
    ok(judge_dir.exists() and any(judge_dir.glob("judge-*.yaml")),
       "judge: verdict yaml written to judge/ dir")


def test_recap(repo: Path) -> None:
    """recap 基本路径：rc=0 + 六问输出齐全（给人看的回顾）。"""
    lto = _lto_factory(repo)

    r = lto("start", "--goal", "recap e2e", "--host", "codex",
            "--why", "因为要验证回顾", "--done-when", "六问都答上", "--force")
    ok(r.returncode == 0, f"recap: start rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    run_id = r.stdout.strip().split("/")[-1]

    r = lto("recap")
    ok(r.returncode == 0, f"recap: rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    ok("关键产物" not in r.stdout, "recap: default omits artifact paths")

    # 六问：每条都必须出现（recap 的契约就是用人话答这六问）
    six_questions = [
        "你当初要做什么",
        "为什么要做",
        "跑了多久",
        "已经做到哪",
        "还剩什么",
        "现在轮到你",
    ]
    for q in six_questions:
        ok(q in r.stdout, f"recap: six-question line present — {q}")

    # goal / why / done-when 真透传进 recap（不是空壳）
    ok("recap e2e" in r.stdout, "recap: goal echoed")
    ok("因为要验证回顾" in r.stdout, "recap: --why echoed in '为什么要做'")
    ok(run_id in r.stdout, "recap: run id in footer")

    r = lto("recap", "--artifacts")
    ok(r.returncode == 0, f"recap: --artifacts rc=0 (got {r.returncode})")
    ok("关键产物" in r.stdout and "state_json" in r.stdout,
       "recap: --artifacts includes manifest summary")


def test_memory(repo: Path) -> None:
    """memory export/resume/publish 基本边界：redaction + degraded local-first + token error。"""
    lto = _lto_factory(repo)

    r = lto("start", "--goal", "memory e2e token=SECRET", "--host", "codex",
            "--why", "path /Users/example/private/file and api_key=SECRET", "--force")
    ok(r.returncode == 0, f"memory: start rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    run_id = r.stdout.strip().split("/")[-1]

    r = lto("task-add", "--task-id", "T1", "--title", "publish projection",
            "--command", "echo token=SECRET")
    ok(r.returncode == 0, f"memory: task-add rc=0 (got {r.returncode})")

    r = lto("memory", "export", "--dry-run")
    ok(r.returncode == 0, f"memory: export rc=0 (got {r.returncode}; {r.stderr.strip()[:120]})")
    data = json.loads(r.stdout)
    text = json.dumps(data, ensure_ascii=False)
    ok(data.get("kind") == "lto_memory_projection", "memory: projection kind present")
    ok(str(repo) not in text, "memory: projection does not leak absolute repo path")
    ok(all(rec.get("project_key") == data.get("project_key") for rec in data.get("records", [])),
       "memory: all records use top-level project_key")
    ok("request_hash" in text and "original_user_request" not in text,
       "memory: original_user_request omitted, hash retained")
    ok("api_key=SECRET" not in text and "token=SECRET" not in text and "/Users/example/private" not in text,
       "memory: secrets and absolute private paths redacted")
    ok(any(rec.get("kind") == "workflow_routing_memory" and rec.get("schema_only")
           for rec in data.get("records", [])),
       "memory: workflow routing is schema-only placeholder")

    r = lto("memory", "resume", "--project", repo.name, "--timeout", "0.1")
    ok(r.returncode == 0, f"memory: resume degraded rc=0 (got {r.returncode})")
    ok("LTO MEMORY LOCAL CAPSULE" in r.stdout and "did not modify files" in r.stdout,
       "memory: resume prints local-first capsule")
    ok("unavailable" in r.stderr and "not configured" in r.stderr and "local .lto" in r.stderr,
       f"memory: resume prints degraded warning (stderr={r.stderr.strip()[:160]})")
    ok("Projection Drift: state_hash=" in r.stdout,
       "memory: resume capsule includes projection drift hashes")

    r = lto("memory", "publish")
    ok(r.returncode != 0, "memory: publish without config/token fails clearly")
    ok("optional memory sink" in (r.stderr + r.stdout) and "MEMORY_FLOW" in (r.stderr + r.stdout),
       f"memory: publish error mentions MEMORY_FLOW config (got {(r.stderr + r.stdout)[:160]})")


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        repo = _make_repo(Path(tmp))
        test_task_add(repo)
        test_judge(repo)
        test_recap(repo)
        test_memory(repo)

    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nORCHESTRATION CMDS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
