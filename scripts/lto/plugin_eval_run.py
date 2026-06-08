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
from .agent_job import KNOWN_RUNNERS, AgentJob, AgentResult, Budget, PermissionPolicy

# v0 故意不实现的能力——写进证据，避免"看起来覆盖全了"
DEFERRED_V0 = [
    "llm_judge_blocker_quality",
    "llm_judge_false_positive_rate",
    "frozen_evidence_hash_redact",
    "automatic_promotion",
    # K: scheduler 当前不返回 token 计数（runner 无 token metadata），
    # 所以 token_delta 永远 None——明确声明，不让调用方误以为偶发缺失。
    "token_cost_metering",
]

# brief 来自插件数据，限大小防资源耗尽
_MAX_BRIEF_BYTES = 512 * 1024

# 私有路径泄露扫描：本机绝对路径前缀（candidate reply 不该把这些带进公开产物）。
# J: 补 /root、Linux /tmp、macOS /var/folders、Windows C:\Users。
_PRIVATE_PATH_RE = re.compile(
    r"(?:/Users/[^/\s]+|/home/[^/\s]+|/root/|"
    r"/private/(?:tmp|var)|/tmp/|/var/folders/|/Volumes/|"
    r"[A-Za-z]:\\Users\\[^\\\s]+)"
)

# pointer-only 检测：某些 runner（agy/gemini 已知）只回个文件指针/路径引用而非实质
# 内容，如 "见 /tmp/result.txt" / "done, see artifact" / "output written to ..."。
# agy-audit-contract 的 failure.pointer_only_reply_is_failure 把它定为失败。
_POINTER_SHORT_LEN = 200  # 超过此长度的回复大概率带实质内容，不判 pointer-only
_PATH_ONLY_MAX_LEN = 110  # 回复整体基本就是路径本身（含少量前后缀/换行）的上限
_FILE_URI_RE = re.compile(r"file://|\bsee\s+(?:the\s+)?(?:file|artifact|attachment|output|reply)\b", re.IGNORECASE)
# 指针引用短语：英文 + agy 已知的中文输出风格（见审计反馈，补 输出到/保存在/见/写到/保存至）
_POINTER_PHRASE_RE = re.compile(
    r"(?:written to|saved to|output to|results? (?:are )?(?:in|at)"
    r"|见附件|见文件|详见|结果在|已写入|输出到|保存在|保存至|写到|写入了?|见\s*/)",
    re.IGNORECASE,
)


def _is_pointer_only(reply: str, *, parsed_substantive: bool) -> bool:
    """确定性判断 reply 是否只是指针/引用而无实质内容（零 LLM）。

    parsed_substantive=True（reply 是合法 JSON）时一律不算 pointer-only：有结构化内容
    就不是裸指针。（注意：空 findings 的 JSON 不算 pointer-only，那是另一类"没干活"，
    属 DEFERRED_V0 的 quality 检测范畴。）否则：短回复 + 指针特征即判 pointer-only。
    """
    if parsed_substantive:
        return False
    stripped = reply.strip()
    if not stripped:
        return False  # 空回复是 empty failure，另算，不混进 pointer-only
    # 只在"短"回复上判——长回复即便含路径短语，也大概率带了实质内容
    if len(stripped) > _POINTER_SHORT_LEN:
        return False
    if _FILE_URI_RE.search(stripped):
        return True
    if _POINTER_PHRASE_RE.search(stripped) and _PRIVATE_PATH_RE.search(stripped):
        return True
    # 短回复且整体基本就是一个绝对路径（允许少量前后缀/换行，如 "Result saved.\nFile: /path"）
    if _PRIVATE_PATH_RE.search(stripped) and len(stripped) < _PATH_ONLY_MAX_LEN:
        return True
    return False


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

    plugin_id = str(validation.manifest.get("id", plugin_dir.name))
    approved_sandbox, mount_present = _mounted_sandbox(repo, run_id, plugin_id)

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

    warnings: list[str] = []
    if not mount_present:
        # 没 mount 就跑 = 绕过取证链；不阻断（v0），但必须显式声明，不静默
        warnings.append(
            "plugin not mounted for this run — ran at default read-only sandbox without a mount-lock "
            "provenance record; run `lto plugin mount` first for an auditable approval trail"
        )
    report = {
        "ok": all_ok,
        "run_id": run_id,
        "plugin": plugin_dir.name,
        "plugin_id": plugin_id,
        "eval_id": pack.get("id"),
        "mount_present": mount_present,
        "approved_sandbox": approved_sandbox,
        "metrics_declared": pack.get("metrics", []),
        "cases": case_reports,
        "warnings": warnings,
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

    # C: runner 来自插件数据，先校验在 KNOWN_RUNNERS 内——否则 AgentJob.validate()
    # 会抛 ValueError 崩掉整个 run（违背 case 级失败隔离），这里降级为 case 失败。
    if runner not in KNOWN_RUNNERS:
        return {"ok": False, "case_id": case_id, "error": f"unknown runner: {runner!r}"}

    # M: brief 来自插件数据，限大小防资源耗尽（写临时文件 + 喂 runner）
    if len(brief.encode("utf-8")) > _MAX_BRIEF_BYTES:
        return {"ok": False, "case_id": case_id, "error": f"brief exceeds {_MAX_BRIEF_BYTES} bytes"}

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

    # 编译两条腿为 AgentJob。D: 非 read-only sandbox 需要 reason，否则
    # PermissionPolicy.validate_for_runner 抛 ValueError 崩 run。
    def _policy() -> PermissionPolicy:
        reason = "" if approved_sandbox == "read-only" else f"eval-run mount-approved sandbox for case {case_id}"
        return PermissionPolicy(sandbox=approved_sandbox, reason=reason)

    baseline_job = AgentJob(
        job_id=f"eval-{case_id}-baseline",
        prompt_ref=str(baseline_brief),
        runner=runner,
        permission_policy=_policy(),
        budget=Budget(),
        meta={"eval_case": case_id, "leg": "baseline"},
    )
    candidate_job = AgentJob(
        job_id=f"eval-{case_id}-candidate",
        prompt_ref=str(candidate_brief),
        runner=runner,
        permission_policy=_policy(),
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

    # B: ok 反映两腿是否真跑成功，不硬编码——否则 runner 失败的 case 被算成成功，
    # 污染上层 report.ok（A/B 结果可信度的核心承诺）。
    case_ok = (
        base_res is not None and base_res.status == "ok"
        and cand_res is not None and cand_res.status == "ok"
    )

    comparison = {
        "ok": case_ok,
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
        # H: job 根本没返回结果（missing）≠ 越权。permission_violation 标 None
        # （不适用/未知），避免 _deltas 把"没跑"误判成 candidate 新增越权。
        return {
            "ran": False,
            "status": "missing",
            "parse_ok": None,
            "timeout": False,
            "permission_violation": None,
            "private_path_leak": False,
            "pointer_only": None,
            "elapsed_sec": None,
            "tokens": None,
        }
    reply = res.reply_text or ""
    parse_ok = True
    parsed_substantive = _json_parses(reply)
    if has_schema:
        # candidate 声明了 output_schema → reply 必须能 JSON parse 才算 parse_ok
        parse_ok = parsed_substantive
    leak = bool(_PRIVATE_PATH_RE.search(reply))
    pointer_only = _is_pointer_only(reply, parsed_substantive=parsed_substantive)
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
        "pointer_only": pointer_only,
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
        "candidate_new_pointer_only": bool(cand.get("pointer_only"))
        and not bool(base.get("pointer_only")),
    }


# ---- helpers ----

def _load_eval_pack(plugin_dir: Path, manifest: dict[str, Any], eval_id: str | None) -> dict[str, Any] | None:
    for rel in (manifest.get("provides", {}) or {}).get("evals", []) or []:
        if not isinstance(rel, str):
            continue
        # 防 TOCTOU 路径穿越：validate 与 eval-run 之间 manifest 可能被换，重新校验
        plugin_extra._ensure_rel_path(rel)
        path = (plugin_dir / rel).resolve()
        plugin_extra._ensure_inside(plugin_dir, path)
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, ValueError):
            continue
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


def _mounted_sandbox(repo: Path, run_id: str, plugin_id: str) -> tuple[str, bool]:
    """读 mount lock 拿本插件被批准的 max_sandbox。

    返回 (approved_sandbox, mount_present)。未 mount → ("read-only", False)，
    让调用者能区分"批准了 read-only"和"完全没 mount"（mount 是取证链，
    见 plugin-boundary.md §6）。lock 顶层 key 是 "mounts"，每条 entry 的
    sandbox 在 entry["approved_permissions"]["max_sandbox"]，用 plugin_id 精确匹配。
    """
    lock_path = core.mount_lock_path(repo, run_id)
    if not lock_path.exists():
        return "read-only", False
    try:
        lock = json.loads(lock_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, ValueError):
        return "read-only", False
    for entry in lock.get("mounts", []) or []:
        if entry.get("plugin_id") == plugin_id:
            approved = (entry.get("approved_permissions") or {}).get("max_sandbox", "read-only")
            return str(approved), True
    return "read-only", False


_SANDBOX_RANK = {"read-only": 0, "workspace-write": 1, "danger-full-access": 2}


def _sandbox_exceeds(used: str, approved: str) -> bool:
    """I: 未知 sandbox 值保守判违规——不认识的等级不能当成"没超"放过。
    used 为空（runner 没报）也保守视为违规（缺 permission snapshot）。"""
    if not used:
        return True
    if used not in _SANDBOX_RANK or approved not in _SANDBOX_RANK:
        return True
    return _SANDBOX_RANK[used] > _SANDBOX_RANK[approved]


_FENCE_RE = re.compile(r"^```(?:json)?\s*\n(.*?)\n```\s*$", re.DOTALL)


def _json_parses(text: str) -> bool:
    """G: 用正则精确提取 ```json fence 内容，而非 strip('`')（会从两端剥所有反引号）。"""
    text = text.strip()
    if not text:
        return False
    m = _FENCE_RE.match(text)
    if m:
        text = m.group(1).strip()
    try:
        json.loads(text)
        return True
    except (json.JSONDecodeError, ValueError):
        return False


def _dump_result(path: Path, res: AgentResult | None) -> None:
    if res is None:
        path.write_text("null\n", encoding="utf-8")
        return
    path.write_text(
        json.dumps(res.to_dict(), ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
