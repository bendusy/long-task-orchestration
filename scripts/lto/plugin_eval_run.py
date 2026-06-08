"""lto plugin eval-run — compile a data-only plugin eval pack into a real A/B LTO run.

设计不变量（见 references/plugin-real-eval-runner.md）：
eval-run 是 **compiler 不是新引擎**。它把 eval pack 的每个 case 编译成两个
AgentJob（baseline 裸 brief / candidate profile 注入），交给现成 scheduler 跑，
对比**确定性指标**，把证据落进 .lto/<run-id>/plugin-eval/<case-id>/。

v0 明确不做（诚实划界，不静默截断）：
- 不做 LLM judge 评 blocker_quality / false_positive（仅确定性指标）；
- 不做 frozen evidence hash/redact 的完整 critical-absorption pipeline；
- 不做自动 promotion（保持 human-gated）。
这些缺省都写进 comparison.json 的 "deferred" 字段。
"""

from __future__ import annotations

import json
import re
import time
from pathlib import Path
from typing import Any

from . import agent_exec
from . import plugin_extra
from . import plugins as core
from .agent_job import AgentJob, AgentResult, Budget, PermissionPolicy

# v0 故意不实现的能力——写进证据，避免"看起来覆盖全了"
DEFERRED_V0 = [
    "llm_judge_blocker_quality",
    "llm_judge_false_positive_rate",
    "frozen_evidence_hash_redact",
    "automatic_promotion",
]

# 私有路径泄露扫描：本机绝对路径前缀（candidate reply 不该把这些带进公开产物）
_PRIVATE_PATH_RE = re.compile(r"(?:/Users/[^/\s]+|/home/[^/\s]+|/private/(?:tmp|var)|/Volumes/)")


def eval_run(
    repo: Path,
    run_id: str,
    plugin_dir: Path,
    *,
    eval_id: str | None = None,
    only_case: str | None = None,
    max_concurrency: int = 4,
    persist: bool = True,
    runners_dir: Path | None = None,
) -> dict[str, Any]:
    """Compile + run an eval pack as baseline-vs-candidate A/B, return report dict.

    返回 {ok, run_id, plugin, eval_id, cases:[...], deferred}。
    每个 case 含 baseline/candidate 指标与 comparison，并已落盘到
    .lto/<run-id>/plugin-eval/<case-id>/。
    """
    plugin_dir = plugin_dir.resolve()
    validation = core.validate_plugin(plugin_dir)
    if not validation.ok or validation.manifest is None:
        return {
            "ok": False,
            "error": "plugin validation failed: " + "; ".join(validation.errors),
            "plugin": str(plugin_dir),
        }

    pack = _load_eval_pack(plugin_dir, validation.manifest, eval_id)
    if pack is None:
        return {"ok": False, "error": f"eval pack not found (eval_id={eval_id})", "plugin": str(plugin_dir)}

    approved_sandbox = _mounted_sandbox(repo, run_id, plugin_dir)

    cases = pack.get("cases", []) or []
    if only_case:
        cases = [c for c in cases if c.get("id") == only_case]
        if not cases:
            return {"ok": False, "error": f"case not found: {only_case}", "plugin": str(plugin_dir)}

    out_root = repo / ".lto" / run_id / "plugin-eval"
    case_reports: list[dict[str, Any]] = []
    all_ok = True
    for case in cases:
        rep = _run_case(
            repo,
            run_id,
            plugin_dir,
            case,
            out_root=out_root,
            approved_sandbox=approved_sandbox,
            metrics=pack.get("metrics", []) or [],
            max_concurrency=max_concurrency,
            persist=persist,
            runners_dir=runners_dir,
        )
        case_reports.append(rep)
        all_ok = all_ok and rep.get("ok", False)

    report = {
        "ok": all_ok,
        "run_id": run_id,
        "plugin": plugin_dir.name,
        "eval_id": pack.get("id"),
        "metrics_declared": pack.get("metrics", []),
        "cases": case_reports,
        "deferred": DEFERRED_V0,
    }
    out_root.mkdir(parents=True, exist_ok=True)
    (out_root / "eval-run-report.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return report


def _run_case(
    repo: Path,
    run_id: str,
    plugin_dir: Path,
    case: dict[str, Any],
    *,
    out_root: Path,
    approved_sandbox: str,
    metrics: list[str],
    max_concurrency: int,
    persist: bool,
    runners_dir: Path | None,
) -> dict[str, Any]:
    case_id = str(case.get("id", "case"))
    runner = str(case.get("runner", "codex"))
    profile_id = case.get("profile")
    brief = str(case.get("brief", ""))

    case_dir = out_root / case_id
    case_dir.mkdir(parents=True, exist_ok=True)

    # 冻结 baseline brief（candidate 在其上注入 profile，便于人核对差异来源）
    baseline_brief = case_dir / "baseline-brief.md"
    baseline_brief.write_text(brief.rstrip() + "\n", encoding="utf-8")

    # candidate：render_profile 把 profile 指令追加到 brief
    candidate_brief = case_dir / "candidate-brief.md"
    render_meta: dict[str, Any] = {}
    output_schema = None
    try:
        render_meta = plugin_extra.render_profile(plugin_dir, profile_id, baseline_brief, candidate_brief)
        output_schema = _load_output_schema(plugin_dir, render_meta.get("output_schema_ref"))
    except Exception as exc:  # profile 缺失/坏 → case 失败但不崩整个 run
        return {
            "ok": False,
            "case_id": case_id,
            "error": f"render_profile failed: {exc}",
        }

    # 编译两条腿为 AgentJob
    baseline_job = AgentJob(
        job_id=f"eval-{case_id}-baseline",
        prompt_ref=str(baseline_brief),
        runner=runner,
        permission_policy=PermissionPolicy(sandbox=approved_sandbox),
        budget=Budget(),
        meta={"eval_case": case_id, "leg": "baseline"},
    )
    candidate_job = AgentJob(
        job_id=f"eval-{case_id}-candidate",
        prompt_ref=str(candidate_brief),
        runner=runner,
        permission_policy=PermissionPolicy(sandbox=approved_sandbox),
        output_schema=output_schema,
        budget=Budget(),
        meta={"eval_case": case_id, "leg": "candidate", "profile": profile_id},
    )

    t0 = time.monotonic()
    results = agent_exec.spawn_agents(
        repo,
        run_id,
        [baseline_job, candidate_job],
        max_concurrency=max_concurrency,
        persist=persist,
        runners_dir=runners_dir,
    )
    wall = time.monotonic() - t0
    by_id = {r.job_id: r for r in results}
    base_res = by_id.get(baseline_job.job_id)
    cand_res = by_id.get(candidate_job.job_id)

    base_m = _deterministic_metrics(base_res, approved_sandbox, has_schema=False)
    cand_m = _deterministic_metrics(cand_res, approved_sandbox, has_schema=output_schema is not None)

    comparison = {
        "ok": True,
        "case_id": case_id,
        "runner": runner,
        "profile": profile_id,
        "wall_clock_sec": round(wall, 3),
        "baseline": base_m,
        "candidate": cand_m,
        "deltas": _deltas(base_m, cand_m),
        "metrics_declared": metrics,
        "deferred": DEFERRED_V0,
    }
    (case_dir / "comparison.json").write_text(
        json.dumps(comparison, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    _dump_result(case_dir / "baseline-result.json", base_res)
    _dump_result(case_dir / "candidate-result.json", cand_res)
    return comparison


def _deterministic_metrics(res: AgentResult | None, approved_sandbox: str, *, has_schema: bool) -> dict[str, Any]:
    """从 AgentResult 算确定性指标（零 LLM）。res 为 None 视为彻底失败。"""
    if res is None:
        return {
            "ran": False,
            "status": "missing",
            "parse_ok": False,
            "timeout": False,
            "permission_violation": True,
            "private_path_leak": False,
            "elapsed_sec": None,
            "tokens": None,
        }
    reply = res.reply_text or ""
    parse_ok = True
    if has_schema:
        # candidate 声明了 output_schema → reply 必须能 JSON parse 才算 parse_ok
        parse_ok = _json_parses(reply)
    leak = bool(_PRIVATE_PATH_RE.search(reply))
    sandbox_used = str(res.permissions.get("sandbox", "")) if res.permissions else ""
    violation = _sandbox_exceeds(sandbox_used, approved_sandbox)
    return {
        "ran": res.status != "missing",
        "status": res.status,
        "exit_code": res.exit_code,
        "parse_ok": parse_ok if has_schema else None,
        "timeout": res.exit_code == 124 or res.status == "timeout",
        "permission_violation": violation,
        "private_path_leak": leak,
        "elapsed_sec": res.cost.get("elapsed_sec") if res.cost else None,
        "tokens": res.cost.get("tokens") if res.cost else None,
    }


def _deltas(base: dict[str, Any], cand: dict[str, Any]) -> dict[str, Any]:
    """candidate 相对 baseline 的变化。正向=candidate 更好/更差由调用者解读。"""
    def _num(x: Any) -> float | None:
        return float(x) if isinstance(x, (int, float)) else None

    bt, ct = _num(base.get("elapsed_sec")), _num(cand.get("elapsed_sec"))
    btok, ctok = _num(base.get("tokens")), _num(cand.get("tokens"))
    return {
        "elapsed_delta_sec": (ct - bt) if (bt is not None and ct is not None) else None,
        "token_delta": (ctok - btok) if (btok is not None and ctok is not None) else None,
        "candidate_new_timeout": bool(cand.get("timeout")) and not bool(base.get("timeout")),
        "candidate_new_permission_violation": bool(cand.get("permission_violation"))
        and not bool(base.get("permission_violation")),
        "candidate_new_private_path_leak": bool(cand.get("private_path_leak"))
        and not bool(base.get("private_path_leak")),
    }


# ---- helpers ----

def _load_eval_pack(plugin_dir: Path, manifest: dict[str, Any], eval_id: str | None) -> dict[str, Any] | None:
    for rel in (manifest.get("provides", {}) or {}).get("evals", []) or []:
        data = json.loads((plugin_dir / rel).read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            continue
        if eval_id is None or data.get("id") == eval_id:
            return data
    return None


def _load_output_schema(plugin_dir: Path, schema_ref: Any) -> dict[str, Any] | None:
    if not schema_ref or not isinstance(schema_ref, str):
        return None
    plugin_extra._ensure_rel_path(schema_ref)
    path = plugin_dir / schema_ref
    if not path.exists():
        return None
    data = json.loads(path.read_text(encoding="utf-8"))
    return data if isinstance(data, dict) else None


def _mounted_sandbox(repo: Path, run_id: str, plugin_dir: Path) -> str:
    """读 mount lock 拿本插件被批准的 max_sandbox；未 mount 默认 read-only。"""
    lock_path = core.mount_lock_path(repo, run_id)
    if not lock_path.exists():
        return "read-only"
    try:
        lock = json.loads(lock_path.read_text(encoding="utf-8"))
    except Exception:
        return "read-only"
    for entry in lock.get("plugins", []) or []:
        if entry.get("plugin_path", "").endswith(plugin_dir.name):
            return str(entry.get("approved_max_sandbox", "read-only"))
    return "read-only"


_SANDBOX_RANK = {"read-only": 0, "workspace-write": 1, "danger-full-access": 2}


def _sandbox_exceeds(used: str, approved: str) -> bool:
    if not used:
        return False
    return _SANDBOX_RANK.get(used, 99) > _SANDBOX_RANK.get(approved, 0)


def _json_parses(text: str) -> bool:
    text = text.strip()
    if not text:
        return False
    # 容忍 ```json fenced block
    if text.startswith("```"):
        text = text.strip("`")
        if text.startswith("json"):
            text = text[4:]
    try:
        json.loads(text.strip())
        return True
    except Exception:
        return False


def _dump_result(path: Path, res: AgentResult | None) -> None:
    if res is None:
        path.write_text("null\n", encoding="utf-8")
        return
    path.write_text(
        json.dumps(res.to_dict(), ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
