"""llm_judge.py — eval-run 的 **主观判读层**（与确定性 metrics 严格隔离）。

设计不变量（见 references/plugin-real-eval-runner.md §7 Judged evidence + 派工 brief 三铁律）：

1. **judge 必须异构**：judge runner ≠ 产出 candidate reply 的 runner（自审无对抗价值）。
   复用 auditors._same_family。候选池逐个 healthcheck，选第一个**可运行的**异构 runner；
   都不可用 → judge 跳过（降级，不报错、不阻断确定性 eval）。
2. **必须可复现**：judge 看到的输入证据（redacted 副本）一起冻结成 evidence_hash；
   judge 的**裁决**另做 canonical hash（judgment_hash）连同 evidence_hash 一起冻进
   judge-verdict.json——相同证据下裁决被重跑/手改，judgment_hash 必变（输入与裁决各有
   可复现 hash）。judge 只看 frozen-evidence.json 里的 redacted 证据，绝不看原始 raw。
3. **judge 不夺权**：judge 分数单独成层，标 kind="subjective_judgment"，
   绝不混进确定性 metrics、绝不参与 promote。promote 仍只看确定性层。

纯标准库（json/hashlib/re/pathlib）。
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

from . import agent_exec
from .agent_job import AgentJob, Budget, PermissionPolicy, Pattern
from .auditors import _same_family
from .scheduler import Scheduler

# verdict 冻结 schema 版本——进 judgment_hash，schema 变 hash 变（防跨版本误比对）。
_VERDICT_SCHEMA_VERSION = 1

# judge 输入硬上限：坏 runner 的超长 reply 不应完整塞进另一家 LLM（成本/超时防护）。
# 量的是 redacted frozen prompt 的 utf-8 字节数；超限 → judge skipped，不派工。
_MAX_JUDGE_INPUT_BYTES = 256 * 1024

# secret redaction：与 events._SECRET_RE 同谱但更严。judge 上下文绝不能带 secret。
# - PEM 私钥：DOTALL 吃掉 BEGIN..END **整段**（含正文 base64），不再只匹配头一行。
# - key-value 型 secret：api_key=xxx / token: xxx / github_pat_xxx 等也打掉。
_SECRET_RE = re.compile(
    r"-----BEGIN[^-]*PRIVATE KEY-----.*?-----END[^-]*PRIVATE KEY-----"
    r"|sk-ant-[A-Za-z0-9_-]{12,}"
    r"|sk-[A-Za-z0-9_-]{12,}"
    r"|gh[pousr]_[A-Za-z0-9_]{12,}"
    r"|github_pat_[A-Za-z0-9_]{12,}"
    r"|AKIA[0-9A-Z]{16}"
    r"|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"
    # key-value 型：(api_?key|secret|token|password|passwd|pwd|access_token) = / : "值"
    r"|(?i:api[_-]?key|secret|token|password|passwd|pwd|access[_-]?token)"
    r"\s*[:=]\s*[\"']?[A-Za-z0-9_\-./+=]{6,}[\"']?",
    re.DOTALL,
)

# 整条绝对路径 redactor（freeze 层专用）：吃掉**整个**私有路径（含目录结构 + 文件名），
# 不再只替前缀漏尾部。覆盖：
# - POSIX `/Users/x/...` `/home/x/...` `/root/...`
# - JSON-escaped `\/Users\/x\/...`（reply 经 JSON 序列化后的形态）
# - Windows `C:\Users\x\...`
_FULL_PATH_RE = re.compile(
    r"(?:\\?/|/)(?:Users|home)(?:\\?/|/)[^\s\"'`,;:)\]}]+"
    r"|/root/[^\s\"'`,;:)\]}]+"
    r"|[A-Za-z]:\\Users\\[^\s\"'`,;:)\]}]+"
)

_REDACT_SECRET = "[REDACTED_SECRET]"
_REDACT_PATH = "[REDACTED_PATH]"

# judge 输出结构化合同（照 audit output_schema 模式：severity/分数是字段不靠正文扫词）
JUDGE_OUTPUT_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "blocker_quality": {
            "type": "string",
            "enum": ["strong", "adequate", "weak", "none"],
        },
        "false_positive_suspected": {"type": "boolean"},
        "rationale": {"type": "string"},
    },
    "required": ["blocker_quality", "false_positive_suspected"],
}

_VALID_QUALITY = {"strong", "adequate", "weak", "none"}

# 候选 judge 池（按优先序）。逐个 healthcheck 选第一个可运行的异构 runner。
_JUDGE_POOL = ["codex", "pi", "agy", "claude"]


def redact_text(text: str) -> str:
    """Redact secrets + **整条**私有绝对路径。**不截断**（judge 需要完整证据语义）。

    顺序：先 secret 再 path——secret 可能内嵌路径样式，先打掉 secret 整体。
    确定性：相同输入 → 相同输出（纯 regex 替换，无随机/时间）。
    """
    text = _SECRET_RE.sub(_REDACT_SECRET, text)
    text = _FULL_PATH_RE.sub(_REDACT_PATH, text)
    return text


def _canonical_inputs(brief: str, baseline_reply: str, candidate_reply: str) -> dict[str, str]:
    """规范化 + redact 三段证据，产出冻结输入字典。

    规范化：统一换行（\\r\\n / \\r → \\n），去尾随空白。保证跨平台 / 不同尾换行
    的等价输入产出相同 hash。
    """
    def _norm(s: str) -> str:
        s = s.replace("\r\n", "\n").replace("\r", "\n")
        return redact_text(s).strip()

    return {
        "brief": _norm(brief),
        "baseline_reply": _norm(baseline_reply),
        "candidate_reply": _norm(candidate_reply),
    }


def _sha256(canonical: str) -> str:
    return "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def freeze_evidence(
    case_dir: Path, brief: str, baseline_reply: str, candidate_reply: str
) -> dict[str, Any]:
    """冻结一个 case 的 judge 输入证据：redact → 规范化 → sha256 → 写 frozen-evidence.json。

    返回 {evidence_hash, frozen_inputs, redaction}。frozen_inputs 是 redacted 副本，
    judge 只看它，不看原始 raw。
    """
    frozen_inputs = _canonical_inputs(brief, baseline_reply, candidate_reply)
    # sort_keys 保证字段序无关；ensure_ascii=False 让中文 1:1 进 hash（确定性）
    canonical = json.dumps(frozen_inputs, ensure_ascii=False, sort_keys=True)
    evidence_hash = _sha256(canonical)

    bundle = {
        "evidence_hash": evidence_hash,
        "frozen_inputs": frozen_inputs,
        "redaction": "applied",
    }
    (case_dir / "frozen-evidence.json").write_text(
        json.dumps(bundle, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return bundle


def _freeze_verdict(
    case_dir: Path,
    *,
    evidence_hash: str,
    judge_runner: str | None,
    status: str,
    parsed_judgment: dict[str, Any] | None,
    error: str | None,
) -> str:
    """对 judge 裁决做 canonical hash 并写 judge-verdict.json，返回 judgment_hash。

    裁决一起冻结（BLOCKER1）：相同证据下裁决被重跑/手改 → judgment_hash 必变。
    canonical payload 含 schema_version + evidence_hash + judge_runner + 裁决主体
    （ok 时 parsed_judgment；非 ok 时 status/error）。
    """
    payload: dict[str, Any] = {
        "schema_version": _VERDICT_SCHEMA_VERSION,
        "evidence_hash": evidence_hash,
        "judge_runner": judge_runner,
        "status": status,
    }
    if parsed_judgment is not None:
        payload["parsed_judgment"] = parsed_judgment
    if error is not None:
        payload["error"] = error

    canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True)
    judgment_hash = _sha256(canonical)

    record = dict(payload)
    record["judgment_hash"] = judgment_hash
    (case_dir / "judge-verdict.json").write_text(
        json.dumps(record, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return judgment_hash


def _build_judge_prompt(frozen: dict[str, Any]) -> str:
    """从冻结的 redacted 证据构建 judge 简报。judge **只**看 frozen_inputs。"""
    fi = frozen["frozen_inputs"]
    return "\n".join(
        [
            "# LTO eval-run 主观判读简报（异构 judge）",
            "",
            f"- evidence_hash: {frozen['evidence_hash']}",
            "- 证据已 redact（私有路径/secret 已脱敏）。只依据下面内容判读，不要臆测原始值。",
            "",
            "你是异构质量裁判。任务：判 candidate（应用了 profile 的 reply）相对 baseline，",
            "其指出的 blocker / 问题质量如何，以及是否疑似假阳（无依据的告警）。",
            "你的判读**不影响**确定性 metrics，也**不参与** promote——只作主观参考层。",
            "",
            "## brief（任务简报）",
            "",
            fi["brief"],
            "",
            "## baseline reply（对照组，无 profile）",
            "",
            fi["baseline_reply"],
            "",
            "## candidate reply（应用 profile）",
            "",
            fi["candidate_reply"],
            "",
            "## 输出要求（结构化 JSON，字段必填）",
            "",
            "```json",
            '{"blocker_quality": "strong|adequate|weak|none", '
            '"false_positive_suspected": true, "rationale": "简短理由"}',
            "```",
            "",
            "blocker_quality 取值仅限 strong / adequate / weak / none。",
            "false_positive_suspected 为 bool。rationale 一句话。",
        ]
    )


def _parse_judge_reply(reply: str) -> dict[str, Any] | None:
    """解析 judge 结构化 JSON 输出。接受裸 JSON 或 ```json fence。

    严格校验（MEDIUM5）：false_positive_suspected 必须是真 bool，字符串 "false" /
    数字等非法值 → 整条裁决判非法（返回 None → judge failed），不做 bool() 强转。
    """
    def _try(obj: Any) -> dict[str, Any] | None:
        if not isinstance(obj, dict):
            return None
        q = str(obj.get("blocker_quality", "")).lower()
        if q not in _VALID_QUALITY:
            return None
        fp = obj.get("false_positive_suspected", None)
        if not isinstance(fp, bool):
            # 严格 bool：非法值不进主观层（防 "false" 被强转成 True 污染裁决）
            return None
        return {
            "blocker_quality": q,
            "false_positive_suspected": fp,
            "rationale": str(obj.get("rationale", ""))[:500],
        }

    text = (reply or "").strip()
    if not text:
        return None
    try:
        out = _try(json.loads(text))
        if out is not None:
            return out
    except (json.JSONDecodeError, ValueError):
        pass
    for block in re.findall(r"```json\s*\n(.*?)\n```", text, re.DOTALL):
        try:
            out = _try(json.loads(block))
            if out is not None:
                return out
        except (json.JSONDecodeError, ValueError):
            continue
    return None


def _skipped(
    case_dir: Path, reason: str, evidence_hash: str, *,
    judge_runner: str | None = None, extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    judgment_hash = _freeze_verdict(
        case_dir, evidence_hash=evidence_hash, judge_runner=judge_runner,
        status="skipped", parsed_judgment=None, error=reason,
    )
    layer = {
        "kind": "subjective_judgment",
        "status": "skipped",
        "reason": reason,
        "evidence_hash": evidence_hash,
        "judgment_hash": judgment_hash,
        "note": "judge does NOT affect promote; deterministic metrics own promotion",
    }
    if extra:
        layer.update(extra)
    return layer


def _failed(
    case_dir: Path, evidence_hash: str, judge_runner: str, error: str,
) -> dict[str, Any]:
    judgment_hash = _freeze_verdict(
        case_dir, evidence_hash=evidence_hash, judge_runner=judge_runner,
        status="failed", parsed_judgment=None, error=error,
    )
    return {
        "kind": "subjective_judgment",
        "status": "failed",
        "runner": judge_runner,
        "evidence_hash": evidence_hash,
        "judgment_hash": judgment_hash,
        "error": error,
        "note": "judge does NOT affect promote; deterministic metrics own promotion",
    }


def judge_case(
    repo: Path,
    run_id: str,
    case_dir: Path,
    *,
    candidate_runner: str,
    frozen: dict[str, Any],
    persist: bool = True,
    runners_dir: Path | None = None,
    judge_runner: str | None = None,
) -> dict[str, Any]:
    """对一个 case 跑异构 judge，返回 comparison.json 的 "judge" 层（主观层）。

    candidate_runner: 产出 candidate reply 的 runner——judge 必须与之异构。
    judge_runner: 显式指定 judge runner（测试注入）；None 时从候选池选**健康的**异构。
    选不到健康异构 → status="skipped"（降级，不报错）。
    每个终态都冻 judge-verdict.json 并回填 judgment_hash。
    """
    evidence_hash = frozen["evidence_hash"]

    # 输入大小闸门（MEDIUM6）：超限不派工，记 evidence_hash + size 后 skipped
    prompt = _build_judge_prompt(frozen)
    input_bytes = len(prompt.encode("utf-8"))
    if input_bytes > _MAX_JUDGE_INPUT_BYTES:
        return _skipped(
            case_dir,
            f"judge input {input_bytes} bytes exceeds limit {_MAX_JUDGE_INPUT_BYTES}",
            evidence_hash,
            extra={"input_bytes": input_bytes, "max_input_bytes": _MAX_JUDGE_INPUT_BYTES},
        )

    # 选健康的异构 judge runner（MEDIUM4：逐个 healthcheck，fallback 到下一个）
    if judge_runner is not None:
        if _same_family(judge_runner, candidate_runner):
            # 铁律1 兜底：显式指定了同族也拒（降级跳过，绝不自审）
            return _skipped(
                case_dir,
                f"judge runner {judge_runner!r} same family as candidate runner {candidate_runner!r}",
                evidence_hash,
            )
        chosen = judge_runner
    else:
        chosen = _pick_healthy_judge_runner(repo, candidate_runner, runners_dir=runners_dir)
        if chosen is None:
            return _skipped(case_dir, "no heterogeneous runner", evidence_hash)

    # judge 只看冻结的 redacted 证据
    job = AgentJob(
        job_id=f"judge-{case_dir.name}-{chosen}",
        prompt_ref=prompt,
        prompt_is_inline=True,
        runner=chosen,
        output_schema=JUDGE_OUTPUT_SCHEMA,
        permission_policy=PermissionPolicy(sandbox="read-only"),
        budget=Budget(timeout_sec=300),
        parent_pattern=Pattern.ADVERSARIAL.value,
        meta={"role": "judge", "evidence_hash": evidence_hash, "candidate_runner": candidate_runner},
    )

    try:
        results = agent_exec.spawn_agents(
            repo, run_id, [job], persist=persist, runners_dir=runners_dir
        )
    except Exception as exc:  # judge 派工失败不阻断确定性 eval
        return _failed(case_dir, evidence_hash, chosen, str(exc)[:200])

    res = results[0] if results else None
    if res is None or not res.ok:
        return _failed(case_dir, evidence_hash, chosen, (res.error[:200] if res else "no result"))

    parsed = _parse_judge_reply(res.reply_text or "")
    if parsed is None:
        return _failed(
            case_dir, evidence_hash, chosen,
            "judge reply not parseable as structured JSON (or false_positive_suspected not a bool)",
        )

    judgment_hash = _freeze_verdict(
        case_dir, evidence_hash=evidence_hash, judge_runner=chosen,
        status="ok", parsed_judgment=parsed, error=None,
    )
    return {
        "kind": "subjective_judgment",
        "status": "ok",
        "runner": chosen,
        "evidence_hash": evidence_hash,
        "judgment_hash": judgment_hash,
        "blocker_quality": parsed["blocker_quality"],
        "false_positive_suspected": parsed["false_positive_suspected"],
        "rationale": parsed["rationale"],
        "note": "judge does NOT affect promote; deterministic metrics own promotion",
    }


def _pick_healthy_judge_runner(
    repo: Path, candidate_runner: str, *, runners_dir: Path | None = None
) -> str | None:
    """从候选池逐个选**健康的**异构 judge runner。

    对每个异构候选跑 healthcheck，选第一个 healthy 的；都不健康/不可用 → None。
    healthcheck 不可用（脚本缺失/异常）时视该 runner 不健康，继续下一个。
    """
    heterogeneous = [r for r in _JUDGE_POOL if not _same_family(r, candidate_runner)]
    if not heterogeneous:
        return None
    try:
        sched = Scheduler(repo, runners_dir=runners_dir)
        health = sched.healthcheck(heterogeneous)
    except Exception:
        return None
    for r in heterogeneous:
        if health.get(r, False):
            return r
    return None
