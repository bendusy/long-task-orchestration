#!/usr/bin/env python3
"""高价值命令端到端测试：task-add / judge / recap / memory。

B4（2026-06-04-spec-lto-gap-closure）：覆盖高价值命令的真实 CLI 边界，
复用 selftest 的临时 git 仓库 + lto() 子进程 helper 模式（不用 pytest，
standalone runner + FAIL 累加 + sys.exit）。runner 行为在 task-add 路径里覆盖。

parallel/pipeline 暂不做（YAGNI）。memory 覆盖 export/resume/publish
边界，防止 artifact-memory 第一片回归。

跑法：
  cd scripts && python3 test_orchestration_cmds.py
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

    # Old runs may contain stale blockers on a task later marked done. Judge is
    # read-only, but should classify them as superseded instead of forcing
    # humans to edit state.json by hand.
    state = json.loads(sp.read_text(encoding="utf-8"))
    state["tasks"][0]["blockers"] = [{"reason": "old failed attempt", "at": "earlier"}]
    state["tasks"][0]["evidence"] = [{"kind": "test", "rc": 0, "summary": "later pass"}]
    sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    r = lto("judge")
    ok(r.returncode == 0 and "verdict: pass" in r.stdout,
       f"judge: stale blocker on done+pass task is superseded (got {r.stdout[-220:]})")
    ok("Superseded Blockers" in r.stdout and "old failed attempt" in r.stdout,
       "judge: reports superseded blockers without failing verdict")
    interventions = (repo / ".lto" / run_id / "interventions.jsonl").read_text(encoding="utf-8")
    ok("avoided_intervention" in interventions and "superseded_blocker" in interventions,
       "judge: logs avoided intervention for superseded blocker")


def test_runner_blocker_supersede(repo: Path) -> None:
    """runner success archives old blockers instead of requiring human cleanup."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "runner supersede e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"runner_supersede: start rc=0 (got {r.returncode})")
    run_id = r.stdout.strip().split("/")[-1]
    r = lto("task-add", "--task-id", "T1", "--title", "flaky command", "--command", "false")
    ok(r.returncode == 0, f"runner_supersede: task-add rc=0 (got {r.returncode})")
    r = lto("runner", "--task-id", "T1", "--kind", "test", "--command", "false")
    ok(r.returncode != 0, "runner_supersede: failing command blocks task")
    r = lto("runner", "--task-id", "T1", "--kind", "test", "--command", "true")
    ok(r.returncode == 0, f"runner_supersede: passing rerun rc=0 (got {r.returncode})")
    state = _state_of(repo, run_id)
    t1 = next(t for t in state.get("tasks", []) if t.get("id") == "T1")
    ok(t1.get("status") == "done", f"runner_supersede: task done (got {t1.get('status')})")
    ok(t1.get("blockers") == [], f"runner_supersede: active blockers cleared (got {t1.get('blockers')})")
    ok(t1.get("resolved_blockers") and t1["resolved_blockers"][0].get("resolved_by") == "runner_success",
       "runner_supersede: old blocker archived with provenance")


def test_closeout_no_changelog(repo: Path) -> None:
    """--no-changelog supports post-commit/admin closeout without tracked dirt."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "closeout no changelog e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"closeout_no_changelog: start rc=0 (got {r.returncode})")
    r = lto("closeout", "--summary", "done", "--next-action", "none", "--no-changelog")
    ok(r.returncode == 0, f"closeout_no_changelog: closeout rc=0 (got {r.returncode}; {r.stderr[:120]})")
    ok(not (repo / "CHANGELOG.md").exists(), "closeout_no_changelog: does not create CHANGELOG.md")
    dirty = subprocess.check_output(["git", "status", "--porcelain", "--", "."], cwd=repo, text=True)
    tracked_dirty = [line for line in dirty.splitlines() if " .lto" not in line and " .lto/" not in line]
    ok(not tracked_dirty, f"closeout_no_changelog: no tracked working-tree dirt (got {tracked_dirty})")

    # If code is dirty, closeout should tell the operator the plain workflow:
    # commit/stash first; then --no-changelog for admin closeout.
    (repo / "README.md").write_text("dirty\n", encoding="utf-8")
    r = lto("closeout", "--summary", "admin", "--no-changelog")
    ok(r.returncode != 0, "closeout_no_changelog: dirty tree still blocked")
    ok("Commit or stash code changes first" in (r.stderr + r.stdout),
       "closeout_no_changelog: dirty error gives actionable workflow")
    run_id = (repo / ".lto" / "current").read_text(encoding="utf-8").strip()
    interventions = (repo / ".lto" / run_id / "interventions.jsonl").read_text(encoding="utf-8")
    ok("intervention_candidate" in interventions and "dirty_closeout_blocked" in interventions,
       "closeout_no_changelog: logs dirty closeout intervention candidate")
    subprocess.run(["git", "checkout", "--", "README.md"], cwd=repo, capture_output=True)


def test_closeout_force_intervention(repo: Path) -> None:
    """--force is logged as a meaningful human intervention and summarized."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "force intervention e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"closeout_force: start rc=0 (got {r.returncode})")
    run_id = r.stdout.strip().split("/")[-1]
    sp = repo / ".lto" / run_id / "state.json"
    state = json.loads(sp.read_text(encoding="utf-8"))
    state.setdefault("risk_points", []).append({
        "id": "RP1", "source": "test", "claim": "force required",
        "evidence_to_check": "none", "verified_by": "", "disposition": "open",
    })
    sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    r = lto("closeout", "--summary", "forced", "--no-changelog", "--force")
    ok(r.returncode == 0, f"closeout_force: forced closeout rc=0 (got {r.returncode}; {r.stderr[:120]})")
    ok("Interventions:" in r.stdout and "meaningful=1" in r.stdout,
       f"closeout_force: prints intervention summary (got {r.stdout[-220:]})")
    handoff = (repo / ".lto" / run_id / "handoff.md").read_text(encoding="utf-8")
    ok("intervention_summary: Interventions:" in handoff,
       "closeout_force: handoff includes intervention summary")
    events = (repo / ".lto" / run_id / "interventions.jsonl").read_text(encoding="utf-8")
    ok("human_intervention" in events and "force_closeout" in events,
       "closeout_force: logs force closeout intervention")
    ev = next(json.loads(l) for l in events.splitlines()
              if l.strip() and json.loads(l).get("category") == "force_closeout")
    ok(ev.get("actor") == "operator" and ev.get("gate") == "closeout",
       f"closeout_force: event carries actor/gate facts (got actor={ev.get('actor')}, gate={ev.get('gate')})")


def test_closeout_untracked_cache_not_blocked(repo: Path) -> None:
    """Untracked files (e.g. runtime caches) warn but don't block closeout;
    tracked changes still block. Regression for pi's .fastembed_cache/ report."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "untracked cache closeout", "--host", "codex", "--force")
    ok(r.returncode == 0, f"untracked_cache: start rc=0 (got {r.returncode})")

    # Pure untracked dropping (simulates .fastembed_cache/ that isn't gitignored
    # in the user's project) → closeout must NOT block, only warn.
    cache_dir = repo / ".fancy_runtime_cache"
    cache_dir.mkdir(exist_ok=True)
    (cache_dir / "blob.bin").write_text("cache\n", encoding="utf-8")
    r = lto("closeout", "--summary", "done", "--next-action", "none", "--no-changelog")
    ok(r.returncode == 0,
       f"untracked_cache: untracked-only closeout rc=0 (got {r.returncode}; {r.stderr[:160]})")
    ok("not blocking" in (r.stdout + r.stderr),
       "untracked_cache: warns about untracked files without blocking")

    # Now a tracked modification → must still block.
    r = lto("start", "--goal", "tracked still blocks", "--host", "codex", "--force")
    ok(r.returncode == 0, f"untracked_cache: 2nd start rc=0 (got {r.returncode})")
    (repo / "README.md").write_text("tracked dirty\n", encoding="utf-8")
    r = lto("closeout", "--summary", "x", "--no-changelog")
    ok(r.returncode != 0, "untracked_cache: tracked change still blocks closeout")
    ok("tracked uncommitted change" in (r.stderr + r.stdout),
       "untracked_cache: tracked-block message is specific")
    subprocess.run(["git", "checkout", "--", "README.md"], cwd=repo, capture_output=True)
    import shutil
    shutil.rmtree(cache_dir, ignore_errors=True)


def test_task_update_and_phase(repo: Path) -> None:
    """task-update records facts without spawning; phase advances current_phase.
    Regression for pi's report: no way to mark done without `runner --command
    true`, and run stuck in intake with no phase-set command."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "task-update + phase e2e", "--host", "codex", "--force")
    ok(r.returncode == 0, f"task_update: start rc=0 (got {r.returncode})")
    run_id = r.stdout.strip().split("/")[-1]
    r = lto("task-add", "--task-id", "T1", "--title", "research step", "--phase", "intake")
    ok(r.returncode == 0, f"task_update: task-add rc=0 (got {r.returncode})")

    # task-update: status + manual note + touched files, no subprocess
    r = lto("task-update", "--task-id", "T1", "--status", "done",
            "--note", "manually verified", "--touch", "docs/a.md", "--touch", "docs/b.md")
    ok(r.returncode == 0, f"task_update: update rc=0 (got {r.returncode}; {r.stderr[:120]})")
    state = json.loads((repo / ".lto" / run_id / "state.json").read_text(encoding="utf-8"))
    t = next(t for t in state["tasks"] if t["id"] == "T1")
    ok(t["status"] == "done", f"task_update: status set to done (got {t['status']})")
    ok(any(e.get("kind") == "manual" and "manually verified" in e.get("summary", "")
           for e in t["evidence"]), "task_update: manual evidence note recorded")
    ok("docs/a.md" in t["touched_files"] and "docs/b.md" in t["touched_files"],
       "task_update: touched files recorded")

    # no-op guard
    r = lto("task-update", "--task-id", "T1")
    ok(r.returncode != 0 and "no-op" in (r.stderr + r.stdout),
       "task_update: empty update is a guarded no-op")
    # invalid status rejected
    r = lto("task-update", "--task-id", "T1", "--status", "finished")
    ok(r.returncode != 0 and "invalid status" in (r.stderr + r.stdout),
       "task_update: invalid status rejected")

    # phase: report then advance
    r = lto("phase")
    ok(r.returncode == 0 and "current phase: intake" in r.stdout,
       f"phase: reports current phase (got {r.stdout[:60]})")
    r = lto("phase", "--set", "audit")
    ok(r.returncode == 0 and "intake → audit" in r.stdout,
       f"phase: advances current_phase (got {r.stdout[:60]}; {r.stderr[:120]})")
    state = json.loads((repo / ".lto" / run_id / "state.json").read_text(encoding="utf-8"))
    ok(state["current_phase"] == "audit", f"phase: state shows audit (got {state['current_phase']})")
    ok(any(tr["to"] == "audit" for tr in state.get("phase_transitions", [])),
       "phase: transition recorded in history")
    r = lto("phase", "--set", "nonsense")
    ok(r.returncode != 0 and "invalid phase" in (r.stderr + r.stdout),
       "phase: invalid phase rejected")
    # events emitted with the correct taxonomy (no UserWarning leaking through)
    events = (repo / ".lto" / run_id / "events.jsonl").read_text(encoding="utf-8")
    ok("task.status_changed" in events and "phase.changed" in events,
       "phase/task_update: emit valid event types")


def test_collect_agent_run(repo: Path) -> None:
    """collect-agent-run bridges delegate.sh products into state.agent_runs so
    recap/rollup see them. Regression for pi's 'agent_runs empty / recap no
    token' report (delegate path decoupled from agent_exec)."""
    lto = _lto_factory(repo)
    r = lto("start", "--goal", "collect agent run e2e", "--host", "claude", "--force")
    ok(r.returncode == 0, f"collect: start rc=0 (got {r.returncode})")
    run_id = r.stdout.strip().split("/")[-1]
    r = lto("task-add", "--task-id", "T1", "--title", "dispatch codex+pi")
    ok(r.returncode == 0, f"collect: task-add rc=0 (got {r.returncode})")

    # simulate delegate.sh products
    (repo / "reply-codex.md").write_text("codex findings...\n", encoding="utf-8")
    (repo / "reply-pi.md").write_text("pi findings...\n", encoding="utf-8")
    (repo / "reply-pi.md.meta.json").write_text(
        '{"tokens_in": 514, "tokens_out": 3683, "tokens": 87525}\n', encoding="utf-8")

    # codex without sidecar → unmetered (cost empty, still recorded)
    r = lto("collect-agent-run", "--task-id", "T1", "--runner", "codex", "--reply", "reply-codex.md")
    ok(r.returncode == 0 and "unmetered" in r.stdout,
       f"collect: codex unmetered run recorded (got {r.stdout.strip()[:80]})")
    # pi with sidecar → tokens captured
    r = lto("collect-agent-run", "--task-id", "T1", "--runner", "pi",
            "--reply", "reply-pi.md", "--elapsed-sec", "53")
    ok(r.returncode == 0 and "87525 tokens" in r.stdout,
       f"collect: pi tokens captured (got {r.stdout.strip()[:80]})")

    state = json.loads((repo / ".lto" / run_id / "state.json").read_text(encoding="utf-8"))
    runs = state["agent_runs"]["T1"]
    ok(len(runs) == 2, f"collect: agent_runs has both dispatches (got {len(runs)})")
    pi_run = next(r for r in runs if r["runner"] == "pi")
    ok(pi_run["cost"].get("tokens") == 87525 and pi_run["cost"].get("elapsed_sec") == 53.0,
       f"collect: pi cost has tokens+elapsed (got {pi_run['cost']})")
    codex_run = next(r for r in runs if r["runner"] == "codex")
    ok(codex_run["cost"] == {}, f"collect: codex unmetered cost is empty (got {codex_run['cost']})")

    # rollup honestly reports 1 of 2 metered
    from lto import state as st_mod
    roll = st_mod.token_rollup(state)
    ok(roll["total_tokens"] == 87525 and roll["runs_with_tokens"] == 1 and roll["runs_total"] == 2,
       f"collect: rollup is 87525 total, 1/2 metered (got {roll['total_tokens']}, "
       f"{roll['runs_with_tokens']}/{roll['runs_total']})")

    # unknown runner / missing reply / missing task are rejected
    r = lto("collect-agent-run", "--task-id", "T1", "--runner", "bogus", "--reply", "reply-pi.md")
    ok(r.returncode != 0 and "unknown runner" in (r.stderr + r.stdout), "collect: unknown runner rejected")
    r = lto("collect-agent-run", "--task-id", "T1", "--runner", "pi", "--reply", "nope.md")
    ok(r.returncode != 0 and "not found" in (r.stderr + r.stdout), "collect: missing reply rejected")
    r = lto("collect-agent-run", "--task-id", "TX", "--runner", "pi", "--reply", "reply-pi.md")
    ok(r.returncode != 0 and "no such task" in (r.stderr + r.stdout), "collect: unknown task rejected")

    # cleanup the simulated products so later tests see a clean tree
    for f in ("reply-codex.md", "reply-pi.md", "reply-pi.md.meta.json"):
        (repo / f).unlink(missing_ok=True)


def test_runs_overview(repo: Path) -> None:
    """`lto runs` lists real runs (with state.json) so an agent entering a
    project sees its LTO history — the local memory when am isn't installed.
    Ad-hoc dirs under .lto/ without state.json are filtered out."""
    lto = _lto_factory(repo)
    # no runs yet
    r = lto("runs")
    ok(r.returncode == 0 and "hasn't run LTO" in r.stdout or "no runs yet" in r.stdout
       or "0 total" in r.stdout, f"runs: empty project handled (got {r.stdout[:60]})")

    r = lto("start", "--goal", "first run for runs overview", "--host", "codex", "--force")
    ok(r.returncode == 0, f"runs: start rc=0 (got {r.returncode})")
    run_id = r.stdout.strip().split("/")[-1]
    lto("task-add", "--task-id", "T1", "--title", "step one")
    lto("task-update", "--task-id", "T1", "--status", "done")

    # an ad-hoc scratch dir under .lto/ without state.json must NOT appear
    scratch = repo / ".lto" / "scratch-replies"
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "note.md").write_text("not a run\n", encoding="utf-8")

    r = lto("runs")
    ok(r.returncode == 0, f"runs: list rc=0 (got {r.returncode})")
    ok("first run for runs overview" in r.stdout, "runs: shows the run's goal")
    ok(run_id in r.stdout, "runs: shows the run id")
    ok("←current" in r.stdout, "runs: marks the current run")
    ok("local memory" in r.stdout, "runs: explains .lto is local memory")
    ok("scratch-replies" not in r.stdout, "runs: filters out non-run dirs (no state.json)")
    ok("unreadable" not in r.stdout, "runs: no unreadable noise from scratch dirs")

    r = lto("runs", "--json")
    ok(r.returncode == 0, "runs: --json rc=0")
    data = json.loads(r.stdout)
    ok(data["count"] == 1 and data["current"] == run_id,
       f"runs: json count=1 current matches (got count={data['count']})")
    import shutil
    shutil.rmtree(scratch, ignore_errors=True)


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


def test_next_cross_run_friction(repo: Path) -> None:
    """`lto next` surfaces recurring friction aggregated across runs.

    Slice: next reads interventions.jsonl across all runs and prints a
    cross-run advisory once a friction category recurs in >= 2 runs.
    Threshold is by distinct run, so one run alone must NOT trigger it.
    """
    lto = _lto_factory(repo)

    def _trigger_dirty_closeout(tag: str) -> str:
        """Start a run, dirty the tree, attempt closeout (blocked → logs a
        dirty_closeout_blocked candidate), then clean up. Returns run_id.

        ``tag`` keeps run-ids distinct: the timestamp resolution is one second,
        so two same-goal starts in the same second collide on run-id.
        """
        r = lto("start", "--goal", f"friction run {tag}", "--host", "codex", "--force")
        ok(r.returncode == 0, f"friction: start rc=0 (got {r.returncode}; {r.stderr[:120]})")
        run_id = r.stdout.strip().split("/")[-1]
        (repo / "README.md").write_text("dirty-for-friction\n", encoding="utf-8")
        r = lto("closeout", "--summary", "x", "--no-changelog")
        ok(r.returncode != 0, "friction: dirty closeout blocked as expected")
        subprocess.run(["git", "checkout", "--", "README.md"], cwd=repo, capture_output=True)
        return run_id

    # ── Run 1: one friction event. next must NOT yet show recurring advisory. ──
    run1 = _trigger_dirty_closeout("alpha")
    events = (repo / ".lto" / run1 / "interventions.jsonl").read_text(encoding="utf-8")
    ok("dirty_closeout_blocked" in events, "friction: run1 logged candidate")

    r = lto("next", "--run-id", run1)
    ok(r.returncode == 0, f"friction: next rc=0 (got {r.returncode}; {r.stderr[:120]})")
    ok("Recurring Friction" not in r.stdout,
       "friction: single run does not trigger cross-run advisory (threshold>=2 runs)")

    # ── Run 2: second distinct run with same friction → now it recurs. ──
    run2 = _trigger_dirty_closeout("beta")
    ok(run2 != run1, "friction: run2 is a distinct run")

    r = lto("next", "--run-id", run2)
    ok(r.returncode == 0, f"friction: next rc=0 after run2 (got {r.returncode})")
    ok("Recurring Friction (cross-run)" in r.stdout,
       f"friction: advisory appears after 2 runs (got tail: {r.stdout[-400:]})")
    ok("dirty_closeout_blocked" in r.stdout and "seen in 2 runs" in r.stdout,
       f"friction: advisory names category + run count (got tail: {r.stdout[-400:]})")
    ok("Commit or stash code before closeout" in r.stdout,
       "friction: advisory includes actionable hint")
    ok("Advisory only" in r.stdout,
       "friction: advisory explicitly marked non-authoritative")

    # ── JSON surface carries the same structured signal. ──
    r = lto("next", "--run-id", run2, "--json")
    ok(r.returncode == 0, f"friction: next --json rc=0 (got {r.returncode})")
    data = json.loads(r.stdout)
    rf = data.get("recurring_friction", [])
    ok(any(f.get("category") == "dirty_closeout_blocked" and f.get("runs") == 2 for f in rf),
       f"friction: JSON recurring_friction carries category + runs (got {rf})")


def test_next_cross_run_avoided_not_friction(repo: Path) -> None:
    """avoided_intervention events (harness helping) must NOT trigger friction.

    The harness silently cleaning stale blockers is help, not friction the
    human should be nagged about. Even across many runs it stays out of the
    Recurring Friction advisory.
    """
    lto = _lto_factory(repo)

    def _trigger_superseded(tag: str) -> str:
        r = lto("start", "--goal", f"avoided run {tag}", "--host", "codex", "--force")
        ok(r.returncode == 0, f"avoided: start rc=0 (got {r.returncode})")
        run_id = r.stdout.strip().split("/")[-1]
        r = lto("task-add", "--task-id", "T1", "--title", "w", "--command", "echo hi")
        ok(r.returncode == 0, f"avoided: task-add rc=0 (got {r.returncode})")
        sp = repo / ".lto" / run_id / "state.json"
        state = json.loads(sp.read_text(encoding="utf-8"))
        state["tasks"][0]["status"] = "done"
        state["tasks"][0]["blockers"] = [{"reason": "old fail", "at": "earlier"}]
        state["tasks"][0]["evidence"] = [{"kind": "test", "rc": 0, "summary": "pass"}]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        r = lto("judge")  # logs avoided_intervention / superseded_blocker
        ok(r.returncode == 0, f"avoided: judge rc=0 (got {r.returncode})")
        return run_id

    run1 = _trigger_superseded("alpha")
    run2 = _trigger_superseded("beta")
    events = (repo / ".lto" / run2 / "interventions.jsonl").read_text(encoding="utf-8")
    ok("avoided_intervention" in events, "avoided: judge logged avoided_intervention")

    r = lto("next", "--run-id", run2)
    ok(r.returncode == 0, f"avoided: next rc=0 (got {r.returncode})")
    ok("superseded_blocker" not in r.stdout,
       "avoided: pure avoided_intervention does not surface as friction")


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

    # Default sink (am-cli) with am absent must degrade to the local-first
    # capsule, never error. Use a missing binary to force the degraded path
    # deterministically (CI may or may not have am installed).
    r = lto("memory", "resume", "--project", repo.name, "--am-bin", "am-nonexistent-ci")
    ok(r.returncode == 0, f"memory: resume degraded rc=0 (got {r.returncode})")
    ok("LTO MEMORY LOCAL CAPSULE" in r.stdout and "did not modify files" in r.stdout,
       "memory: resume prints local-first capsule")
    ok("unavailable" in r.stderr and "am CLI not found" in r.stderr and "local .lto" in r.stderr,
       f"memory: resume prints degraded warning (stderr={r.stderr.strip()[:160]})")
    ok("Projection Drift: state_hash=" in r.stdout,
       "memory: resume capsule includes projection drift hashes")

    # legacy-rest resume still degrades via the old not-configured path.
    r = lto("memory", "resume", "--project", repo.name, "--sink", "legacy-rest", "--timeout", "0.1")
    ok(r.returncode == 0, f"memory: legacy-rest resume degraded rc=0 (got {r.returncode})")
    ok("not configured" in r.stderr,
       f"memory: legacy-rest resume degraded warning (stderr={r.stderr.strip()[:120]})")

    # Default sink is am-cli since am 0.7.0. With am absent (CI), it must fail
    # clearly pointing at the am binary — and explicitly reassure that local
    # .lto/ is still the source of truth (publish is optional).
    r = lto("memory", "publish", "--am-bin", "am-nonexistent-ci")
    ok(r.returncode != 0, "memory: am-cli publish without am binary fails clearly")
    out = r.stderr + r.stdout
    ok("am CLI not found" in out and "am-nonexistent-ci" in out,
       f"memory: am-cli publish error names missing am binary (got {out[:160]})")
    ok("local .lto" in out and "source of truth" in out,
       "memory: am-cli publish error reassures local .lto is source of truth")

    # legacy-rest sink remains a fallback and still requires MEMORY_FLOW config.
    r = lto("memory", "publish", "--sink", "legacy-rest")
    ok(r.returncode != 0, "memory: legacy-rest publish without config fails clearly")
    out = r.stderr + r.stdout
    ok("optional memory sink" in out and "MEMORY_FLOW" in out,
       f"memory: legacy-rest publish error mentions MEMORY_FLOW config (got {out[:160]})")


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        repo = _make_repo(Path(tmp))
        test_task_add(repo)
        test_judge(repo)
        test_runner_blocker_supersede(repo)
        test_closeout_no_changelog(repo)
        test_closeout_force_intervention(repo)
        test_recap(repo)
        test_memory(repo)

    # Cross-run friction aggregation reads ALL runs in a repo's .lto/, so each
    # of these gets a fresh isolated repo to avoid contamination from the
    # interventions written by the tests above.
    with tempfile.TemporaryDirectory() as tmp:
        test_next_cross_run_friction(_make_repo(Path(tmp)))
    with tempfile.TemporaryDirectory() as tmp:
        test_next_cross_run_avoided_not_friction(_make_repo(Path(tmp)))
    with tempfile.TemporaryDirectory() as tmp:
        test_closeout_untracked_cache_not_blocked(_make_repo(Path(tmp)))
    with tempfile.TemporaryDirectory() as tmp:
        test_task_update_and_phase(_make_repo(Path(tmp)))
    with tempfile.TemporaryDirectory() as tmp:
        test_collect_agent_run(_make_repo(Path(tmp)))
    with tempfile.TemporaryDirectory() as tmp:
        test_runs_overview(_make_repo(Path(tmp)))

    if FAIL:
        print(f"\n{len(FAIL)} FAILURES", file=sys.stderr)
        return 1
    print("\nORCHESTRATION CMDS OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
