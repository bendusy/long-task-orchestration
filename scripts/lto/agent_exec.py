#!/usr/bin/env python3
"""agent_exec.py — spawn 原语：agent job 执行 + 结果落 state.json。

这是 exec.run_command（shell 世界）的 agent 世界对应物。
编排的是带独立 context 的 agent，不是 shell 命令。
对上层事实路由器（lto next）暴露干净接口。

设计原则：
- 只做"组装 job + 调 scheduler + 落 state"，不重复 scheduler 已有的并发/重试/限流。
- state 写操作用一次 load-modify-save，非逐条写。
- 不过滤 scheduler 返回的 FAILED/SKIPPED/TIMEOUT——原样穿透。
- runners_dir 透传给 Scheduler，方便测试注入假 runner。

纯标准库，Python 3.10+，类型注解齐全。
"""

from __future__ import annotations

import warnings
from pathlib import Path
from typing import Any

from lto.agent_job import AgentJob, AgentResult
from lto.scheduler import Scheduler
from lto import state as st


def spawn_agents(
    repo: Path,
    run_id: str,
    jobs: list[AgentJob],
    *,
    max_concurrency: int = 4,
    persist: bool = True,
    runners_dir: Path | None = None,
) -> list[AgentResult]:
    """执行一批 agent job，可选把结果落进 state.json 的 agent_runs 区。

    内部用 Scheduler 处理并发/重试/healthcheck/退出码判定。
    persist=True 时把每个 AgentResult 追加到 state['agent_runs'][job_id]
    （用一次 load-modify-save）。

    Parameters
    ----------
    repo:
        Repository root.
    run_id:
        LTO run identifier（对应 repo/.lto/<run_id>/state.json）。
    jobs:
        AgentJob 列表。空列表直接返回 []，不调 scheduler。
    max_concurrency:
        最大并发数，传给 Scheduler。
    persist:
        True 时把结果落进 state.json。state 文件不存在时打印警告，
        静默跳过 persist，仍返回 results。
    runners_dir:
        透传给 Scheduler，覆盖默认 runners 目录（测试用）。

    Returns
    -------
    list[AgentResult]
        顺序对应输入 jobs，包含 FAILED/SKIPPED/TIMEOUT 状态，不过滤。
    """
    if not jobs:
        return []

    sched = Scheduler(repo, max_concurrency=max_concurrency, runners_dir=runners_dir)
    results = sched.submit(jobs)

    if persist:
        state_path = repo / ".lto" / run_id / "state.json"
        state = st.load_state(state_path)
        if state is None:
            warnings.warn(
                f"state file not found: {state_path}, skipping persist "
                f"(results still returned)"
            )
        else:
            agent_runs = state.setdefault("agent_runs", {})
            for job, result in zip(jobs, results):
                # setdefault + append: fan-out 多轮同 job_id 累加不覆盖
                agent_runs.setdefault(job.job_id, []).append(result.to_dict())
            st.save_state(state_path, state)

    return results


def spawn_one(
    repo: Path,
    run_id: str,
    job: AgentJob,
    **kw: Any,
) -> AgentResult:
    """spawn_agents 的单 job 便捷封装。

    等价于 spawn_agents(repo, run_id, [job], **kw)[0]。
    """
    return spawn_agents(repo, run_id, [job], **kw)[0]


# ===========================================================================
# Self-test（注入假 runner，不依赖真 agent）
# ===========================================================================


def _run_selftest() -> int:
    import json
    import os
    import shutil
    import sys
    import tempfile

    tests_passed = 0
    tests_total = 0

    def ok(label: str) -> None:
        nonlocal tests_passed, tests_total
        tests_passed += 1
        tests_total += 1
        print(f"  ✅ {label}")

    def fail(label: str, detail: str = "") -> int:
        nonlocal tests_total
        tests_total += 1
        print(f"  ❌ {label}")
        if detail:
            print(f"     {detail}")
        return 1

    # ---- scaffold ----
    tmpdir = Path(tempfile.mkdtemp(prefix="lto_agent_exec_test_"))

    repo = tmpdir / "repo"
    repo.mkdir()

    runners_dir = tmpdir / "runners"
    runners_dir.mkdir()

    # Fake codex runner — writes a controlled reply to the reply file ($2)
    fake_runner_py = tmpdir / "fake_runner.py"
    fake_runner_py.write_text(
        "#!/usr/bin/env python3\n"
        "import json, os, sys\n"
        "prompt_file, reply_file, timeout_sec = sys.argv[1:4]\n"
        "\n"
        "with open(prompt_file) as f:\n"
        "    first_line = f.readline().strip()\n"
        'job_id = first_line.replace("# JOB_ID:", "").strip()\n'
        "\n"
        'ctrl_path = os.environ.get("AGENT_EXEC_TEST_CONTROL", "")\n'
        "behaviour = {}\n"
        "if ctrl_path and os.path.exists(ctrl_path):\n"
        "    with open(ctrl_path) as f:\n"
        "        behaviour = json.load(f).get(job_id, {})\n"
        "\n"
        'sleep_sec = float(behaviour.get("sleep", 0))\n'
        'exit_code = int(behaviour.get("exit_code", 0))\n'
        'output = str(behaviour.get("output", "fake reply"))\n'
        "\n"
        "if sleep_sec > 0:\n"
        "    import time\n"
        "    time.sleep(sleep_sec)\n"
        "\n"
        'with open(reply_file, "w") as f:\n'
        "    f.write(output)\n"
        "\n"
        "sys.exit(exit_code)\n"
    )
    fake_runner_py.chmod(0o755)

    codex_sh = runners_dir / "codex.sh"
    codex_sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake_runner_py}" "$@"\n')
    codex_sh.chmod(0o755)

    # Fake healthcheck
    hc_sh = runners_dir / "healthcheck.sh"
    hc_sh.write_text('#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"}]\'\nexit 0\n')
    hc_sh.chmod(0o755)

    # Control file helpers
    def set_control(behaviours: dict[str, dict]) -> Path:
        p = tmpdir / "control.json"
        p.write_text(json.dumps(behaviours))
        os.environ["AGENT_EXEC_TEST_CONTROL"] = str(p)
        return p

    def make_job(job_id: str, runner: str = "codex", **kw: Any) -> AgentJob:
        from lto.agent_job import AgentJob, Budget, RetryPolicy
        prompt = f"# JOB_ID:{job_id}\nTest prompt for {job_id}"
        defaults: dict[str, Any] = {
            "job_id": job_id,
            "runner": runner,
            "prompt_ref": prompt,
            "prompt_is_inline": True,
            "budget": Budget(timeout_sec=30),
            "retry_policy": RetryPolicy(max_retries=0),
        }
        defaults.update(kw)
        return AgentJob(**defaults)

    run_id = "test-run-001"
    state_dir = repo / ".lto" / run_id
    state_dir.mkdir(parents=True)

    # ===================================================================
    # Test 1: 3 jobs spawn → 3 results, 顺序对应
    # ===================================================================
    print("\n[1] 3 jobs spawn → 3 results in order")
    set_control({
        "t1_a": {"exit_code": 0, "output": "reply A"},
        "t1_b": {"exit_code": 0, "output": "reply B"},
        "t1_c": {"exit_code": 0, "output": "reply C"},
    })
    # seed an empty state
    state_path = state_dir / "state.json"
    st.save_state(state_path, {"run_id": run_id, "phase": "test"})

    jobs = [make_job("t1_a"), make_job("t1_b"), make_job("t1_c")]
    results = spawn_agents(repo, run_id, jobs, persist=False, runners_dir=runners_dir)

    if len(results) != 3:
        fail("count", f"expected 3, got {len(results)}")
    elif [r.job_id for r in results] == ["t1_a", "t1_b", "t1_c"]:
        ok("3 results, correct order")
    else:
        fail("order", f"got {[r.job_id for r in results]}")

    all_ok_flag = all(r.ok for r in results)
    reply_match = (results[0].reply_text == "reply A"
                   and results[1].reply_text == "reply B"
                   and results[2].reply_text == "reply C")
    if all_ok_flag and reply_match:
        ok("all OK, replies match")
    else:
        details = [(r.job_id, r.status, r.exit_code, r.error[:500], r.reply_text[:80]) for r in results]
        fail("reply content", f"ok={all_ok_flag}, details={details}")

    # ===================================================================
    # Test 2: persist=True → state.json agent_runs 有 3 个 job_id
    # ===================================================================
    print("\n[2] persist=True → agent_runs populated")
    st.save_state(state_path, {"run_id": run_id, "phase": "test"})  # fresh state
    set_control({
        "t2_a": {"exit_code": 0, "output": "pA"},
        "t2_b": {"exit_code": 0, "output": "pB"},
        "t2_c": {"exit_code": 0, "output": "pC"},
    })
    jobs2 = [make_job("t2_a"), make_job("t2_b"), make_job("t2_c")]
    results2 = spawn_agents(repo, run_id, jobs2, persist=True, runners_dir=runners_dir)

    loaded = st.load_state(state_path)
    agent_runs = loaded.get("agent_runs", {}) if loaded else {}
    if (set(agent_runs.keys()) == {"t2_a", "t2_b", "t2_c"}
            and all(len(v) == 1 for v in agent_runs.values())):
        ok("agent_runs has 3 job_ids, each with 1 entry")
    else:
        fail("agent_runs keys", f"got keys={list(agent_runs.keys())}, lens={[len(v) for v in agent_runs.values()]}")

    # Verify saved result content
    saved = agent_runs.get("t2_a", [])[0] if "t2_a" in agent_runs else {}
    if saved.get("reply_text") == "pA" and saved.get("status") == "ok":
        ok("saved result content correct (reply_text + status)")
    else:
        fail("saved content", f"got reply_text={saved.get('reply_text')}, status={saved.get('status')}")

    # ===================================================================
    # Test 3: persist=False → state.json 不变
    # ===================================================================
    print("\n[3] persist=False → state.json unchanged")
    st.save_state(state_path, {"run_id": run_id, "phase": "test", "marker": "before"})
    set_control({
        "t3_a": {"exit_code": 0, "output": "no_persist"},
    })
    _ = spawn_agents(repo, run_id, [make_job("t3_a")], persist=False, runners_dir=runners_dir)

    loaded3 = st.load_state(state_path)
    if loaded3.get("marker") == "before" and "agent_runs" not in loaded3:
        ok("state unchanged (marker intact, no agent_runs key)")
    else:
        fail("state mutation", f"marker={loaded3.get('marker')}, keys={list(loaded3.keys())}")

    # ===================================================================
    # Test 4: run_id 无对应 state 文件 → persist 跳过不崩
    # ===================================================================
    print("\n[4] Missing state file → persist skip, warn, still return results")
    missing_run_id = "no-such-run"
    missing_state_path = repo / ".lto" / missing_run_id / "state.json"
    # ensure dir exists but no file
    missing_state_path.parent.mkdir(parents=True, exist_ok=True)
    if missing_state_path.exists():
        missing_state_path.unlink()

    set_control({
        "t4_a": {"exit_code": 0, "output": "missing state"},
    })

    import warnings as _warnings
    with _warnings.catch_warnings(record=True) as caught:
        _warnings.simplefilter("always")
        results4 = spawn_agents(repo, missing_run_id, [make_job("t4_a")],
                                persist=True, runners_dir=runners_dir)

    if len(results4) == 1 and results4[0].ok:
        ok("results returned despite missing state")
    else:
        fail("results", f"len={len(results4)}, ok={results4[0].ok if results4 else 'N/A'}")

    warn_msgs = [str(w.message) for w in caught if "state file not found" in str(w.message).lower()]
    if warn_msgs:
        ok(f"warning issued: {warn_msgs[0][:80]}...")
    else:
        fail("warning", "no warning for missing state")

    # ===================================================================
    # Test 5: spawn_one → 返回单个 result
    # ===================================================================
    print("\n[5] spawn_one → single result")
    set_control({
        "t5_one": {"exit_code": 0, "output": "solo"},
    })
    r5 = spawn_one(repo, run_id, make_job("t5_one"), persist=False, runners_dir=runners_dir)
    if isinstance(r5, AgentResult) and r5.job_id == "t5_one" and r5.reply_text == "solo":
        ok("spawn_one returns single AgentResult")
    else:
        fail("spawn_one", f"type={type(r5).__name__}, job_id={r5.job_id if hasattr(r5,'job_id') else 'N/A'}")

    # ===================================================================
    # Adversarial tests
    # ===================================================================
    print("\n[ADV] Adversarial edge cases")

    # A1: agent_runs 累加（同 job_id 两批 → append，不是覆盖）
    print("  A1: agent_runs accumulation (append not overwrite)")
    st.save_state(state_path, {"run_id": run_id, "phase": "test", "agent_runs": {}})
    set_control({
        "ta1_dup": {"exit_code": 0, "output": "batch1"},
    })
    spawn_agents(repo, run_id, [make_job("ta1_dup")], persist=True, runners_dir=runners_dir)
    set_control({
        "ta1_dup": {"exit_code": 0, "output": "batch2"},
    })
    spawn_agents(repo, run_id, [make_job("ta1_dup")], persist=True, runners_dir=runners_dir)

    loaded_a1 = st.load_state(state_path)
    entries = loaded_a1.get("agent_runs", {}).get("ta1_dup", [])
    if len(entries) == 2 and entries[0]["reply_text"] == "batch1" and entries[1]["reply_text"] == "batch2":
        ok("A1: 2 batches for same job_id → appended, not overwritten")
    else:
        fail("A1", f"len={len(entries)}, replies={[e.get('reply_text') for e in entries]}")

    # A1b: agent_runs 累加时不丢其他 state 字段
    st.save_state(state_path, {"run_id": run_id, "phase": "test", "other_field": "keep_me"})
    set_control({
        "ta1b_extra": {"exit_code": 0, "output": "extra"},
    })
    spawn_agents(repo, run_id, [make_job("ta1b_extra")], persist=True, runners_dir=runners_dir)
    loaded_a1b = st.load_state(state_path)
    if loaded_a1b.get("other_field") == "keep_me" and "agent_runs" in loaded_a1b:
        ok("A1b: other state fields preserved during persist")
    else:
        fail("A1b", f"other_field={loaded_a1b.get('other_field')}, has agent_runs={'agent_runs' in loaded_a1b}")

    # A2: 空 jobs 列表 → 返回 []，不崩
    print("  A2: empty jobs → []")
    results_a2 = spawn_agents(repo, run_id, [], persist=True, runners_dir=runners_dir)
    if results_a2 == []:
        ok("A2: empty jobs → []")
    else:
        fail("A2", f"got {results_a2}")

    # A3: 非 ASCII（中文 reply）→ json 序列化 ensure_ascii=False 不乱码
    print("  A3: non-ASCII reply → state.json preserves Unicode")
    st.save_state(state_path, {"run_id": run_id, "phase": "test"})
    set_control({
        "ta3_cn": {"exit_code": 0, "output": "这是中文回复内容 —— 呈批件已审核通过 ✓"},
    })
    spawn_agents(repo, run_id, [make_job("ta3_cn")], persist=True, runners_dir=runners_dir)

    raw = state_path.read_text(encoding="utf-8")
    if "这是中文回复内容" in raw and "✓" in raw and "\\u" not in raw[raw.find("agent_runs"):]:
        ok("A3: non-ASCII preserved in state.json (no escaped unicode)")
    else:
        has_escaped = "\\u" in raw[raw.find("agent_runs"):] if "agent_runs" in raw else False
        fail("A3", f"unicode escaped={has_escaped}")

    # A4: FAILED result 原样穿透（不过滤）
    print("  A4: FAILED result passes through unfiltered")
    st.save_state(state_path, {"run_id": run_id, "phase": "test"})
    set_control({
        "ta4_fail": {"exit_code": 1, "output": ""},
        "ta4_ok": {"exit_code": 0, "output": "ok"},
    })
    results_a4 = spawn_agents(
        repo, run_id,
        [make_job("ta4_fail"), make_job("ta4_ok")],
        persist=True, runners_dir=runners_dir,
    )
    if results_a4[0].status == "failed" and results_a4[1].status == "ok":
        ok("A4: FAILED + OK both returned in order")
    else:
        fail("A4", f"statuses={[r.status for r in results_a4]}")

    # A5: error field preserved in persisted state
    loaded_a4 = st.load_state(state_path)
    saved_fail = loaded_a4.get("agent_runs", {}).get("ta4_fail", [])[0] if loaded_a4 else {}
    if saved_fail.get("status") == "failed" and saved_fail.get("error"):
        ok("A5: FAILED result error field preserved in state")
    else:
        fail("A5", f"status={saved_fail.get('status')}, error={saved_fail.get('error')}")

    # A6: spawn_agents with runners_dir=None uses default (integration)
    print("  A6: runners_dir=None does not crash (uses default path)")
    # This just tests the parameter is accepted; actual runner resolution
    # would need a real repo structure.  We verify no TypeError.
    try:
        # Use a separate call that won't actually execute (invalid runner)
        # but validates the parameter flows through.
        spawn_agents(repo, run_id, [make_job("ta6_default")],
                     persist=False)  # no runners_dir → uses default
        ok("A6: runners_dir=None accepted")
    except TypeError as e:
        fail("A6", f"TypeError: {e}")
    except Exception:
        # Expected: default runners dir won't have our fake runner,
        # so healthcheck or execution may fail. That's OK — we just
        # want to verify no TypeError.
        ok("A6: runners_dir=None accepted (expected exec failure on default dir)")

    # ---- cleanup ----
    shutil.rmtree(tmpdir, ignore_errors=True)

    print(f"\n{'='*50}")
    print(f"Results: {tests_passed}/{tests_total} passed")
    if tests_passed == tests_total:
        print("AGENT_EXEC SELFTEST OK")
        return 0
    else:
        print(f"AGENT_EXEC SELFTEST FAILED ({tests_total - tests_passed} failures)")
        return 1


if __name__ == "__main__":
    import sys
    sys.exit(_run_selftest())
