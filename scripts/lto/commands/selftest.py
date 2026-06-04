"""lto self-test — 离线覆盖测试。"""

from __future__ import annotations

import argparse, json, subprocess, sys, tempfile, os
from pathlib import Path


def run(_args: argparse.Namespace) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        subprocess.run(["git", "init"], cwd=repo, capture_output=True)
        (repo / "README.md").write_text("test\n", encoding="utf-8")
        subprocess.run(["git", "add", "README.md"], cwd=repo, capture_output=True)
        # 测试 fixture：临时隔离仓库，用占位身份引导首个 commit（不污染真实 blame，
        # 不会 push）。生产代码已改为 opt-in + 真实身份，见 git_state.auto_commit_lto。
        subprocess.run(
            ["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid", "commit", "-m", "init"],
            cwd=repo, capture_output=True,
        )

        lto_path = Path(__file__).resolve().parent.parent.parent / "lto_run.py"

        def lto(*args: str) -> subprocess.CompletedProcess:
            return subprocess.run(
                [sys.executable, str(lto_path), "--repo", str(repo)] + list(args),
                cwd=repo, capture_output=True, text=True,
            )

        # Test start
        r = lto("start", "--goal", "self test", "--host", "codex", "--profile", "deploy", "--with-audit")
        if r.returncode != 0:
            print(f"FAIL start: {r.stderr}", file=sys.stderr)
            return 1

        # B5 regression: --profile deploy must capture a real preflight snapshot
        # (deploy ⊋ audit). Guards against deploy silently degrading back to a
        # dead alias of audit, or the snapshot dropping default_state keys.
        deploy_run = r.stdout.strip().split("/")[-1]
        snap = json.loads((repo / ".lto" / deploy_run / "state.json").read_text())["environment_snapshot"]
        if "verdict" not in snap:
            print(f"FAIL deploy snapshot: no verdict (deploy didn't run preflight): {snap}", file=sys.stderr)
            return 1
        if "write_roots" not in snap:
            print(f"FAIL deploy snapshot: missing write_roots (not a superset of default): {snap}", file=sys.stderr)
            return 1
        if snap.get("sandbox") not in ("ok", "fail"):
            print(f"FAIL deploy snapshot: sandbox not probed (still placeholder): {snap}", file=sys.stderr)
            return 1

        # audit profile must NOT carry a real snapshot (verdict absent) — proves the
        # two profiles diverge, so deploy is a strict superset not an alias.
        r2 = lto("start", "--goal", "audit cmp", "--host", "codex", "--profile", "audit", "--force")
        audit_run = r2.stdout.strip().split("/")[-1]
        asnap = json.loads((repo / ".lto" / audit_run / "state.json").read_text())["environment_snapshot"]
        if "verdict" in asnap:
            print(f"FAIL audit snapshot: unexpectedly probed (deploy/audit not distinct): {asnap}", file=sys.stderr)
            return 1

        # Test check
        r = lto("check")
        if r.returncode != 0:
            print(f"FAIL check: {r.stderr}", file=sys.stderr)
            return 1

        # Test resume
        r = lto("resume")
        if r.returncode != 0:
            print(f"FAIL resume: {r.stderr}", file=sys.stderr)
            return 1

        # Test preflight
        r = lto("preflight")
        if r.returncode != 0:
            print(f"FAIL preflight: {r.stderr}", file=sys.stderr)
            return 1

        # Test hook pre-commit
        os.environ["LTO_HOOK_MODE"] = "warn"
        r = lto("hook", "pre-commit")
        # warn mode always returns 0 for pre-commit
        if r.returncode != 0:
            print(f"FAIL hook pre-commit: {r.stderr}", file=sys.stderr)
            return 1

        # Test closeout
        r = lto("closeout", "--summary", "self test complete", "--next-action", "none")
        if r.returncode != 0:
            print(f"FAIL closeout: {r.stderr}", file=sys.stderr)
            return 1

        # Gate regression: non-converged ledger must block closeout
        r = lto("start", "--goal", "gate test", "--host", "codex", "--profile", "deploy", "--with-audit", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        ledger = repo / ".lto" / run_id / "audit-ledger.md"
        if ledger.exists():
            content = ledger.read_text(encoding="utf-8")
            content = content.replace(
                "| R1 |  |  |  |  |  | start | open |",
                "| R1 |  |  | 1 | 1 | 0 | start | open |\n| R2 |  |  | 2 | 1 | 0 | rebound | open |",
            )
            ledger.write_text(content, encoding="utf-8")
            subprocess.run(["git", "add", ".lto"], cwd=repo, capture_output=True)
            subprocess.run(
                ["git", "-c", "user.name=lto", "-c", "user.email=lto@example.invalid", "commit", "-m", "gate lto"],
                cwd=repo, capture_output=True,
            )
            r = lto("closeout", "--summary", "should be refused", "--next-action", "none")
            if r.returncode == 0:
                print("FAIL gate: closeout accepted non-converged ledger", file=sys.stderr)
                return 1

        # --- risk coverage gate (P1-a) ---

        # G1: risk_points unverified → closeout refused; --force bypasses
        r = lto("start", "--goal", "risk gate 1", "--host", "codex", "--profile", "deploy", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        sp = repo / ".lto" / run_id / "state.json"
        state = json.loads(sp.read_text())
        state["risk_points"] = [{
            "id": "RP1", "source": "diff",
            "claim": "test risk", "evidence_to_check": "test.py",
            "verified_by": "", "disposition": "open",
        }]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n")

        r = lto("closeout", "--summary", "should be refused", "--next-action", "none", "--allow-dirty")
        if r.returncode == 0:
            print("FAIL gate G1a: closeout accepted unverified risk points", file=sys.stderr)
            return 1

        r = lto("closeout", "--summary", "forced", "--next-action", "none", "--force", "--allow-dirty")
        if r.returncode != 0:
            print("FAIL gate G1b: --force did not bypass risk coverage", file=sys.stderr)
            return 1

        # G2: risk_points all verified → closeout passes
        r = lto("start", "--goal", "risk gate 2", "--host", "codex", "--profile", "deploy", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        sp = repo / ".lto" / run_id / "state.json"
        state = json.loads(sp.read_text())
        state["risk_points"] = [{
            "id": "RP1", "source": "diff",
            "claim": "test risk", "evidence_to_check": "test.py",
            "verified_by": "codex-R1", "disposition": "verified",
        }]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n")

        r = lto("closeout", "--summary", "all verified", "--next-action", "none", "--allow-dirty")
        if r.returncode != 0:
            print("FAIL gate G2: closeout refused all-verified risk points", file=sys.stderr)
            return 1

        # G3: risk_points empty → closeout not blocked (simple task)
        r = lto("start", "--goal", "risk gate 3", "--host", "codex", "--profile", "deploy", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        # default_state already has risk_points: [], no modifications
        r = lto("closeout", "--summary", "simple task", "--next-action", "none", "--allow-dirty")
        if r.returncode != 0:
            print("FAIL gate G3: closeout refused empty risk_points (simple task)", file=sys.stderr)
            return 1

        # --- empty ledger hole (P1-a) ---

        # G4: high-risk task without ledger → closeout refused; --force bypasses
        r = lto("start", "--goal", "ledger hole 1", "--host", "codex", "--profile", "deploy", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        sp = repo / ".lto" / run_id / "state.json"
        state = json.loads(sp.read_text())
        state["tasks"] = [{
            "id": "T1", "title": "重构认证 auth 模块", "status": "done",
            "phase": "implementation",
        }]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n")

        r = lto("closeout", "--summary", "high risk no ledger", "--next-action", "none", "--allow-dirty")
        if r.returncode == 0:
            print("FAIL gate G4a: closeout accepted high-risk run without ledger", file=sys.stderr)
            return 1

        r = lto("closeout", "--summary", "forced", "--next-action", "none", "--force", "--allow-dirty")
        if r.returncode != 0:
            print("FAIL gate G4b: --force did not bypass high-risk-no-ledger gate", file=sys.stderr)
            return 1

        # G5: high-risk task with empty ledger (placeholder only) → closeout refused
        r = lto("start", "--goal", "ledger hole 2", "--host", "codex", "--profile", "deploy", "--with-audit", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        sp = repo / ".lto" / run_id / "state.json"
        state = json.loads(sp.read_text())
        state["tasks"] = [{
            "id": "T1", "title": "数据库 schema 迁移", "status": "done",
            "phase": "implementation",
        }]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n")
        # ledger exists but placeholder R1 only (no real rounds)

        r = lto("closeout", "--summary", "high risk empty ledger", "--next-action", "none", "--allow-dirty")
        if r.returncode == 0:
            print("FAIL gate G5: closeout accepted high-risk run with empty ledger", file=sys.stderr)
            return 1

        # G6: no high-risk task, no ledger → closeout passes
        r = lto("start", "--goal", "ledger hole 3", "--host", "codex", "--profile", "deploy", "--force")
        run_id = r.stdout.strip().split("/")[-1]
        # no high-risk keywords, no ledger → should pass cleanly
        r = lto("closeout", "--summary", "simple no ledger", "--next-action", "none", "--allow-dirty")
        if r.returncode != 0:
            print("FAIL gate G6: closeout refused simple run without ledger", file=sys.stderr)
            return 1

        # ── decision.py 接线：autopilot --decide 派三方收敛 ──
        r = lto("start", "--goal", "decide wiring", "--host", "codex", "--force")
        if r.returncode != 0:
            print(f"FAIL decide start: {r.stderr}", file=sys.stderr)
            return 1
        run_id = r.stdout.strip().split("/")[-1]
        sp = repo / ".lto" / run_id / "state.json"

        def clear_autopilot_digest() -> None:
            state_inner = json.loads(sp.read_text())
            state_inner.setdefault("gates", {}).pop("autopilot_last_digest", None)
            sp.write_text(json.dumps(state_inner, indent=2, ensure_ascii=False) + "\n")

        # (1) 无 task（空 phase）+ --decide → 应拒绝派工（派三方在白纸上找问题无意义）
        r = lto("autopilot", "--supervised", "--decide", "--decide-budget", "1")
        if r.returncode != 10:
            print(f"FAIL decide empty-phase terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "无 task" not in r.stdout and "无可讨论" not in r.stdout:
            print(f"FAIL decide: empty phase should refuse spawn, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1
        if '"status": "needs_host"' not in r.stdout:
            print(f"FAIL decide empty-phase: terminal status should be needs_host, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # 注入一个 pending task → has_tasks=True，让 --decide 真正接到引擎
        state = json.loads(sp.read_text())
        state["tasks"] = [{"id": "T1", "title": "do work", "status": "pending", "command": "echo hi"}]
        sp.write_text(json.dumps(state, indent=2, ensure_ascii=False) + "\n")
        clear_autopilot_digest()

        # (2) --decide 默认关 → escalate 走 brief-only，不出 decide 块
        r = lto("autopilot", "--supervised")
        if r.returncode != 10:
            print(f"FAIL autopilot supervised terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "三方异构收敛" in r.stdout:
            print("FAIL decide: --decide off but tri-partite block appeared", file=sys.stderr)
            return 1
        if '"status": "needs_host"' not in r.stdout:
            print(f"FAIL autopilot supervised: terminal status should be needs_host, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # (3) --decide 开 + budget 不足 → 接到 run_decision，引擎优雅降级 budget_exhausted
        #     (不真 spawn：budget < est_cost 走 _budget_exhausted_result)
        r = lto("autopilot", "--supervised", "--decide", "--decide-budget", "1")
        if r.returncode != 21:
            print(f"FAIL autopilot --decide budget terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "三方异构收敛" not in r.stdout:
            print("FAIL decide: --decide on but no tri-partite block (not wired?)", file=sys.stderr)
            return 1
        if "Budget Exhausted" not in r.stdout:
            print(f"FAIL decide: budget=1 should hit budget_exhausted, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1
        if "needs_human" not in r.stdout:
            print("FAIL decide: budget_exhausted should yield needs_human status", file=sys.stderr)
            return 1
        if '"status": "budget_exhausted"' not in r.stdout:
            print(f"FAIL decide: terminal status should be budget_exhausted, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # (4) 显式 --decide-budget 0 必须被尊重为 0（不被 falsy 短路升级到默认 50000）
        r = lto("autopilot", "--supervised", "--decide", "--decide-budget", "0")
        if r.returncode != 21:
            print(f"FAIL decide budget0 terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "budget_remaining≈0 " not in r.stdout:
            print(f"FAIL decide: explicit --decide-budget 0 must stay 0, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1
        if '"status": "budget_exhausted"' not in r.stdout:
            print(f"FAIL decide budget0: terminal status should be budget_exhausted, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # (5) 负数 budget 无意义 → 回落默认（不显示 ≈-N）
        clear_autopilot_digest()
        r = lto("autopilot", "--supervised", "--decide", "--decide-budget", "-5")
        if r.returncode != 12:
            print(f"FAIL decide budget-neg terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "为负" not in r.stdout or "budget_remaining≈-" in r.stdout:
            print(f"FAIL decide: negative budget must fall back, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1
        if '"status": "needs_human"' not in r.stdout:
            print(f"FAIL decide budget-neg: terminal status should be needs_human, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # (6) supervised 只建议 judge/closeout，不自动执行 → NEEDS_CONFIRM=11
        r = lto("start", "--goal", "autopilot confirm", "--host", "codex", "--force")
        if r.returncode != 0:
            print(f"FAIL autopilot confirm start: {r.stderr}", file=sys.stderr)
            return 1
        run_id_confirm = r.stdout.strip().split("/")[-1]
        sp_confirm = repo / ".lto" / run_id_confirm / "state.json"
        state_confirm = json.loads(sp_confirm.read_text())
        state_confirm["tasks"] = [{"id": "T1", "title": "done work", "status": "done"}]
        state_confirm.setdefault("gates", {}).pop("autopilot_last_digest", None)
        sp_confirm.write_text(json.dumps(state_confirm, indent=2, ensure_ascii=False) + "\n")
        r = lto("autopilot", "--supervised")
        if r.returncode != 11:
            print(f"FAIL autopilot confirm terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if '"status": "needs_confirm"' not in r.stdout:
            print(f"FAIL autopilot confirm: terminal status should be needs_confirm, got: {r.stdout[-300:]}", file=sys.stderr)
            return 1

        # (7) auto-exec 遇 git push 必须 HELD 并升 NEEDS_CONFIRM=11
        r = lto("start", "--goal", "autopilot held", "--host", "codex", "--force")
        if r.returncode != 0:
            print(f"FAIL autopilot held start: {r.stderr}", file=sys.stderr)
            return 1
        run_id_held = r.stdout.strip().split("/")[-1]
        sp_held = repo / ".lto" / run_id_held / "state.json"
        state_held = json.loads(sp_held.read_text())
        state_held["tasks"] = [{
            "id": "T1", "title": "push", "status": "pending",
            "commands_run": ["git push origin main"],
        }]
        state_held.setdefault("gates", {}).pop("autopilot_last_digest", None)
        sp_held.write_text(json.dumps(state_held, indent=2, ensure_ascii=False) + "\n")
        r = lto("autopilot", "--supervised", "--auto-exec")
        if r.returncode != 11:
            print(f"FAIL autopilot held terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        if "HELD" not in r.stdout or '"status": "needs_confirm"' not in r.stdout:
            print(f"FAIL autopilot held: expected HELD + needs_confirm, got: {r.stdout[-500:]}", file=sys.stderr)
            return 1

        # (8) 混合 HELD / failed / retry-exhausted 时不能只报 confirmation，必须保留更强状态和全部原因
        r = lto("start", "--goal", "autopilot mixed terminal", "--host", "codex", "--force")
        if r.returncode != 0:
            print(f"FAIL autopilot mixed start: {r.stderr}", file=sys.stderr)
            return 1
        run_id_mixed = r.stdout.strip().split("/")[-1]
        sp_mixed = repo / ".lto" / run_id_mixed / "state.json"
        state_mixed = json.loads(sp_mixed.read_text())
        state_mixed["tasks"] = [
            {
                "id": "T1", "title": "push", "status": "pending",
                "commands_run": ["git push origin main"],
            },
            {
                "id": "T2", "title": "failing command", "status": "pending",
                "commands_run": ["false"],
            },
            {
                "id": "T3", "title": "retry cap", "status": "pending",
                "commands_run": ["echo retry"], "retry_count": 3,
            },
        ]
        state_mixed.setdefault("gates", {}).pop("autopilot_last_digest", None)
        sp_mixed.write_text(json.dumps(state_mixed, indent=2, ensure_ascii=False) + "\n")
        r = lto("autopilot", "--supervised", "--auto-exec")
        if r.returncode != 12:
            print(f"FAIL autopilot mixed terminal code: rc={r.returncode} stderr={r.stderr}", file=sys.stderr)
            return 1
        tail = r.stdout[-700:]
        for needle in ('"status": "needs_human"', "exceeded retry limit", "sandbox command(s) failed", "held for human confirmation"):
            if needle not in tail:
                print(f"FAIL autopilot mixed: missing {needle!r}, got: {tail}", file=sys.stderr)
                return 1

    print("SELFTEST OK")
    return 0


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("self-test", help="run offline smoke coverage")
    p.set_defaults(func=run)
