"""lto audit — 对抗审计提取：编排异构三方审计 + 收口进 ledger。

设计原则（与 LTO 架构一致）：
- LTO 只编排和收口，不自己派工（导航仪不是码农）。
- 内置 delegate：优先使用 scripts/delegate/triad.sh，也允许环境变量覆盖。
- 强制「审者 runtime ≠ 写者 host」——同家族自审无对抗价值。

两个动作：
  lto audit                  扫高风险 task → 写审计简报 → 打印派工指令 → 确保 ledger
  lto audit --collect <dir>  读三方 reply → 校验异构 → 抽 blocker 计数 → 追加 ledger 一行 → 判收敛
  lto audit --auto-dispatch  扫高风险 task → 写审计简报 → 通过 agent_exec 自动派工 → 收口

2026-06-03: 结构化 findings 优先（修 severity regex 老坑）+ --auto-dispatch 自动发起
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

from .. import state as st
from .. import artifacts as af
from ..auditors import (
    _parse_structured_reply,
    _pick_auditors,
    _runtime_from_filename,
    _same_family,
    parse_findings_text,
)

# 高风险关键词：碰这些领域的 task 默认进审计
HIGH_RISK_KEYWORDS = (
    "持久化", "迁移", "schema", "migration", "权限", "认证", "鉴权", "auth",
    "并发", "concurren", "锁", "lock", "外部接口", "api", "支付", "payment",
    "安全", "security", "加密", "crypt", "删除", "delete", "回滚", "rollback",
)

# reply 里标 blocker 严重度的启发式标记（regex fallback，结构化优先）
SEVERITY_PATTERNS = {
    "critical": re.compile(r"\b(critical|严重|致命|阻断)\b", re.IGNORECASE),
    "high": re.compile(r"\b(high|高危|高风险)\b", re.IGNORECASE),
}

def run(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    target_dir = repo / ".lto" / run_id
    state_path = target_dir / "state.json"
    state = st.load_state(state_path)
    if state is None:
        raise SystemExit(f"no state.json for {run_id}")

    if getattr(args, "discover_risks", False):
        return _discover_risks(repo, run_id, target_dir, state, args)
    if args.collect:
        return _collect(repo, run_id, target_dir, state, args)
    return _prepare(repo, run_id, target_dir, state, args)


def _prepare(repo: Path, run_id: str, target_dir: Path, state: dict, args: argparse.Namespace) -> int:
    host = state.get("host_runtime", "unknown")
    tasks = state.get("tasks", [])

    if args.task_id:
        targets = [t for t in tasks if t["id"] in args.task_id]
    else:
        targets = [t for t in tasks if _is_high_risk(t)]

    if not targets:
        print("no high-risk tasks found; pass --task-id T1 T2 to force, "
              "or this run may not need adversarial audit.")
        return 0

    # 写审计简报
    audit_dir = target_dir / "audit"
    audit_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    brief_path = audit_dir / f"audit-brief-{ts}.md"
    brief_path.write_text(_build_brief(state, targets), encoding="utf-8")
    af.register_path(
        repo, run_id, brief_path, kind="audit_brief",
        producer="lto.commands.audit.prepare", state=state,
        summary=f"audit brief for {len(targets)} task(s)", tags=["audit", "brief"],
    )

    # 确保 ledger 存在
    ledger_path = target_dir / "audit-ledger.md"
    _ensure_ledger(repo, ledger_path)
    af.register_path(
        repo, run_id, ledger_path, kind="audit_ledger",
        producer="lto.commands.audit.prepare", state=state,
        summary="audit convergence ledger", tags=["audit", "ledger"],
    )

    # 选异构审计方：排除与 host 同家族的
    auditors = _pick_auditors(host)

    # ---------- auto-dispatch 路径 ----------
    if getattr(args, "auto_dispatch", False):
        return _auto_dispatch(repo, run_id, target_dir, state, brief_path, audit_dir, auditors)

    # ---------- 现有 print 指令路径（不变） ----------
    print(f"◆ LTO Audit prepared: {len(targets)} high-risk task(s)")
    print(f"  brief:   {brief_path.relative_to(repo)}")
    print(f"  ledger:  {ledger_path.relative_to(repo)}")
    print(f"  host:    {host}  →  auditors: {' '.join(auditors)}  (审者必须 ≠ host)")
    print()
    print(_dispatch_hint(repo, brief_path, audit_dir, auditors))
    return 0


def _auto_dispatch(
    repo: Path, run_id: str, target_dir: Path, state: dict,
    brief_path: Path, audit_dir: Path, auditors: list[str],
) -> int:
    """--auto-dispatch: 通过 agent_exec 自动派工给异构审计方，然后收口。"""
    from lto import agent_exec
    from lto.agent_job import AgentJob, Budget, Pattern

    host = state.get("host_runtime", "unknown")

    output_schema = {
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "severity": {
                    "type": "string",
                    "enum": ["critical", "high", "medium", "low"],
                },
                "claim": {"type": "string"},
                "evidence_to_check": {"type": "string"},
                "file": {"type": "string"},
            },
            "required": ["severity", "claim"],
        },
    }

    jobs = []
    for auditor in auditors:
        job = AgentJob(
            job_id=f"audit-{auditor}",
            prompt_ref=str(brief_path),
            runner=auditor,
            output_schema=output_schema,
            budget=Budget(timeout_sec=300),
            parent_pattern=Pattern.ADVERSARIAL.value,
            meta={"host": host, "brief": str(brief_path)},
        )
        jobs.append(job)

    print(f"◆ LTO Audit auto-dispatching to {' '.join(auditors)} ...")
    results = agent_exec.spawn_agents(repo, run_id, jobs, persist=True)

    # 写 reply 文件到 audit/replies/
    replies_dir = audit_dir / "replies"
    replies_dir.mkdir(parents=True, exist_ok=True)
    for job, result in zip(jobs, results):
        reply_path = replies_dir / f"reply-{job.runner}.md"
        reply_path.write_text(result.reply_text, encoding="utf-8")
        af.register_path(
            repo, run_id, reply_path, kind="audit_reply",
            producer="lto.commands.audit.auto_dispatch", state=state,
            summary=f"{job.runner} audit reply", job_id=job.job_id,
            runner=job.runner, consumed_by=["audit.collect"], tags=["audit", "reply"],
        )
        if result.ok:
            print(f"  {job.runner}: OK")
            # 如果 result 有结构化 findings，也写一份 JSON 方便后续 collect
            if result.findings:
                json_path = replies_dir / f"reply-{job.runner}.json"
                json_path.write_text(
                    json.dumps(result.findings, indent=2, ensure_ascii=False),
                    encoding="utf-8",
                )
                af.register_path(
                    repo, run_id, json_path, kind="audit_findings_json",
                    producer="lto.commands.audit.auto_dispatch", state=state,
                    summary=f"{job.runner} structured audit findings",
                    job_id=job.job_id, runner=job.runner,
                    consumed_by=["audit.collect"], tags=["audit", "findings"],
                )
        else:
            print(f"  {job.runner}: {result.status} (exit={result.exit_code}) {result.error[:120]}")

    # 自动收口
    print()
    return _do_collect(
        repo, run_id, target_dir, state,
        reply_dir=replies_dir,
    )


def _collect(repo: Path, run_id: str, target_dir: Path, state: dict, args: argparse.Namespace) -> int:
    """CLI --collect 入口：解析 args 后委托 _do_collect。"""
    reply_dir = Path(args.collect)
    if not reply_dir.is_absolute():
        reply_dir = repo / reply_dir

    return _do_collect(
        repo, run_id, target_dir, state,
        reply_dir=reply_dir,
        high=args.high,
        critical=args.critical,
        minor=args.minor or 0,
        allow_same_family=args.allow_same_family,
    )


def _do_collect(
    repo: Path, run_id: str, target_dir: Path, state: dict,
    reply_dir: Path, high: int | None = None, critical: int | None = None,
    minor: int = 0, allow_same_family: bool = False,
) -> int:
    """Core collect logic：读 reply → 校验异构 → 抽 blocker 计数 → 追加 ledger → 判收敛。

    供 --collect CLI 路径和 --auto-dispatch 自动收口路径共用。
    """
    if not reply_dir.is_dir():
        raise SystemExit(f"reply dir not found: {reply_dir}")

    host = state.get("host_runtime", "unknown")
    replies = sorted(p for p in reply_dir.iterdir() if p.is_file() and p.suffix in (".md", ".txt"))
    if not replies:
        raise SystemExit(f"no reply files (.md/.txt) in {reply_dir}")

    # 校验异构：审者 runtime（从文件名推断）必须 ≠ host
    used_auditors = []
    same_family = []
    for r in replies:
        runtime = _runtime_from_filename(r.name)
        used_auditors.append(runtime or r.stem)
        if runtime and _same_family(runtime, host):
            same_family.append(runtime)
        af.register_path(
            repo, run_id, r, kind="audit_reply",
            producer="lto.commands.audit.collect", state=state,
            summary=f"{runtime or r.stem} audit reply",
            runner=runtime or r.stem, consumed_by=["audit.collect"],
            tags=["audit", "reply"],
        )
    if same_family and not allow_same_family:
        raise SystemExit(
            f"audit refused: reply from same-family runtime(s) {same_family} as host '{host}' "
            "(self-audit has no adversarial value; use --allow-same-family to override)"
        )

    # ---------- 结构化 findings 优先（P0-d）----------
    structured_findings: dict[str, list[dict]] = {}
    fallback_replies: list[Path] = []
    for r in replies:
        parsed = _parse_structured_reply(r)
        if parsed is not None:
            structured_findings[r.name] = parsed
        else:
            fallback_replies.append(r)

    if high is not None or critical is not None:
        # 手填优先（不变）
        high = high or 0
        critical = critical or 0
    elif fallback_replies:
        # 混合：结构化部分用 severity 字段数，其余退回 regex fallback
        high, critical = 0, 0
        for _name, findings in structured_findings.items():
            for f in findings:
                sev = str(f.get("severity", "")).lower()
                if sev == "critical":
                    critical += 1
                elif sev == "high":
                    high += 1
        fb_high, fb_critical = _scan_severity(fallback_replies)
        print(
            f"warning: {len(fallback_replies)} reply(s) lack structured JSON findings, "
            f"using regex fallback: {[r.name for r in fallback_replies]}"
        )
        high += fb_high
        critical += fb_critical
    else:
        # 全部结构化：从 severity 字段数（不用 regex，防"没有 critical"误数）
        high, critical = 0, 0
        for _name, findings in structured_findings.items():
            for f in findings:
                sev = str(f.get("severity", "")).lower()
                if sev == "critical":
                    critical += 1
                elif sev == "high":
                    high += 1

    # 追加 Round Summary 一行
    ledger_path = target_dir / "audit-ledger.md"
    _ensure_ledger(repo, ledger_path)
    round_label = _next_round_label(ledger_path)
    artifact = str(reply_dir.relative_to(repo)) if _is_relative_to(reply_dir, repo) else reply_dir.name
    _append_round(
        ledger_path, round_label, artifact, " ".join(used_auditors), high, critical, minor
    )
    af.register_path(
        repo, run_id, ledger_path, kind="audit_ledger",
        producer="lto.commands.audit.collect", state=state,
        summary=f"audit ledger updated {round_label}",
        consumed_by=["closeout"], tags=["audit", "ledger"],
    )

    print(f"◆ LTO Audit collected: {round_label}")
    print(f"  auditors: {' '.join(used_auditors)}  (host: {host})")
    print(f"  blockers: high={high} critical={critical} minor={minor}")
    if structured_findings:
        print(f"  structured: {len(structured_findings)} reply(s) parsed from JSON findings")
    if fallback_replies:
        print(f"  fallback:   {len(fallback_replies)} reply(s) used regex scan")
    print(f"  ledger:   {ledger_path.relative_to(repo)}")

    # 跑收敛判定
    verdict = _run_ledger_check(repo, ledger_path)
    if verdict:
        print(f"  verdict:  {verdict}")
    return 0


# ---------- 高风险判定 ----------

def _is_high_risk(task: dict) -> bool:
    hay = (task.get("title", "") + " " + " ".join(task.get("touched_files", []))).lower()
    return any(kw.lower() in hay for kw in HIGH_RISK_KEYWORDS)


# ---------- 简报 ----------

def _build_risk_brief(state: dict) -> str:
    """为 risk discoverer agent 构建对抗发现简报。

    侧重：找出可能出问题但还没人登记的风险点，对抗"自报完整"。
    """
    tasks = state.get("tasks", [])
    existing_rps = state.get("risk_points", [])

    lines = [
        "# Risk Discovery Brief（对抗生成）",
        "",
        f"- goal: {state.get('goal', '?')}",
        f"- host_runtime: {state.get('host_runtime', '?')}",
        f"- current_phase: {state.get('current_phase', '?')}",
        f"- existing risk_points: {len(existing_rps)}",
    ]
    if existing_rps:
        lines.append("")
        lines.append("## 已登记 risk point（不要重复）")
        lines.append("")
        for rp in existing_rps:
            lines.append(f"- {rp.get('id', '?')}: {rp.get('claim', '?')}")

    lines += [
        "",
        "## 任务清单",
        "",
    ]
    for t in tasks:
        lines.append(f"### {t.get('id', '?')}: {t.get('title', '?')}")
        if t.get("touched_files"):
            lines.append(f"- touched: {', '.join(t['touched_files'][:8])}")
        lines.append("")

    lines += [
        "## 指令",
        "",
        "你是对抗风险发现 agent。任务：",
        "",
        "1. 读上面的 goal/tasks/touched_files — 理解做了什么改动",
        "2. 找出**别人没提到**的潜在风险点（对抗「自报完整」）",
        "3. 不做代码审查，只找遗漏的 risk point",
        "",
        "重点检查维度：",
        "- 持久化/migration：schema 变更是否安全？回滚逻辑？",
        "- 权限/鉴权：新增入口是否挂载了鉴权？",
        "- 并发/竞态：多 agent/多进程路径有无锁或顺序假设？",
        "- 外部接口/API：是否对调用失败做了超时/重试/降级？",
        "- 状态一致性：中断恢复、部分失败时 state.json 是否一致？",
        "",
        "## 输出要求",
        "",
        "用 JSON 数组输出发现的风险点。无发现输出空数组 `[]`：",
        "",
        "```json",
        '[{"claim": "风险描述", "evidence_to_check": "应核查什么证据", "severity": "high|critical|medium"}]',
        "```",
        "",
        "severity 取 high / critical / medium（不设 low：低风险不值得登记）。",
    ]
    return "\n".join(lines)


def _discover_risks(repo: Path, run_id: str, target_dir: Path, state: dict, args: argparse.Namespace,
                    *, _runners_dir: Path | None = None) -> int:
    """派一个独立 agent 读 state + git diff，主动发现 risk point，
    写进 state.risk_points（source=risk-agent）。

    _runners_dir: 测试用，覆盖默认 runners 目录。"""
    from lto import agent_exec
    from lto.agent_job import AgentJob, Budget, Pattern

    host = state.get("host_runtime", "unknown")
    auditors = _pick_auditors(host)
    if not auditors:
        print("warning: no heterogeneous auditor available for host {host!r}; "
              "cannot run risk discovery (self-audit has no adversarial value)")
        return 1

    discoverer = auditors[0]
    if _same_family(discoverer, host):
        print(f"warning: discoverer {discoverer} same family as host {host}; "
              "risk discovery would have no adversarial value, skipping")
        return 1

    # 写 risk-discovery brief
    audit_dir = target_dir / "audit"
    audit_dir.mkdir(parents=True, exist_ok=True)
    ts = st.iso_now().replace(":", "-")[:19]
    brief_path = audit_dir / f"risk-brief-{ts}.md"
    brief_text = _build_risk_brief(state)
    brief_path.write_text(brief_text, encoding="utf-8")
    af.register_path(
        repo, run_id, brief_path, kind="audit_brief",
        producer="lto.commands.audit.discover_risks", state=state,
        summary="risk discovery brief", tags=["audit", "risk", "brief"],
    )

    output_schema = {
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "claim": {"type": "string"},
                "evidence_to_check": {"type": "string"},
                "severity": {
                    "type": "string",
                    "enum": ["high", "critical", "medium"],
                },
            },
            "required": ["claim", "severity"],
        },
    }

    job = AgentJob(
        job_id=f"risk-discover-{discoverer}",
        prompt_ref=str(brief_path),
        runner=discoverer,
        output_schema=output_schema,
        budget=Budget(timeout_sec=300),
        parent_pattern=Pattern.ADVERSARIAL.value,
        meta={"host": host, "brief": str(brief_path)},
    )

    print(f"◆ LTO Risk Discovery: spawning {discoverer} (host={host}) ...")
    state_path = target_dir / "state.json"

    try:
        results = agent_exec.spawn_agents(repo, run_id, [job], persist=False,
                                            runners_dir=_runners_dir)
    except Exception as exc:
        print(f"warning: spawn failed for {discoverer}: {exc}")
        return 2

    if not results:
        print(f"warning: spawn returned no results for {discoverer}")
        return 2

    result = results[0]
    if not result.ok:
        print(f"warning: {discoverer} returned {result.status} (exit={result.exit_code}); "
              f"fallback text: {result.error[:120]}")
        return 2

    # 解析 risk 列表
    reply_text = result.reply_text or ""
    risks: list[dict] = []

    reply_path = audit_dir / f"risk-reply-{discoverer}-{ts}.md"
    reply_path.write_text(reply_text, encoding="utf-8")
    af.register_path(
        repo, run_id, reply_path, kind="risk_discovery_reply",
        producer="lto.commands.audit.discover_risks", state=state,
        summary=f"{discoverer} risk discovery reply",
        job_id=job.job_id, runner=discoverer, tags=["audit", "risk", "reply"],
    )
    parsed = parse_findings_text(reply_text)
    if parsed is not None:
        risks = parsed
    else:
        data = _parse_json_list(reply_text)
        if data is not None:
            if len(data) == 0:
                print(f"  {discoverer}: no risks found (empty JSON list)")
                return 0
            risks = data

    if not risks:
        print(f"warning: {discoverer} reply contained no parseable risk JSON; "
              f"first 200 chars: {reply_text[:200]}")
        return 2

    # 写入 state.risk_points
    state = st.load_state(state_path)
    if state is None:
        print("warning: state reload failed; cannot write risk points")
        return 2

    existing_ids = {rp["id"] for rp in state.get("risk_points", [])}
    count = 0
    n = 1
    for r in risks:
        claim = r.get("claim", "")
        evidence = r.get("evidence_to_check", "")
        if not claim:
            continue
        while f"RP-auto-{n}" in existing_ids:
            n += 1
        rp_id = f"RP-auto-{n}"
        st.add_risk_point(state, rp_id=rp_id, source="risk-agent",
                          claim=claim, evidence_to_check=evidence)
        existing_ids.add(rp_id)
        count += 1
        n += 1

    st.save_state(state_path, state)
    print(f"  discoverer: {discoverer}")
    print(f"  risks found: {count}")
    print(f"  state:       {state_path.relative_to(repo)}")
    return 0


def _parse_json_list(text: str) -> list | None:
    """Parse a bare or fenced JSON list, accepting [] as a valid list."""
    try:
        data = json.loads(text)
        if isinstance(data, list):
            return data
    except (json.JSONDecodeError, ValueError):
        pass

    blocks = re.findall(r"```json\s*\n(.*?)\n```", text, re.DOTALL)
    for block in blocks:
        try:
            data = json.loads(block)
            if isinstance(data, list):
                return data
        except (json.JSONDecodeError, ValueError):
            continue
    return None


def _build_brief(state: dict, targets: list[dict]) -> str:
    lines = [
        "# LTO 异构审计简报",
        "",
        f"- goal: {state.get('goal', '?')}",
        f"- host_runtime: {state.get('host_runtime', '?')}（审者必须不同家族）",
        f"- phase: {state.get('current_phase', '?')}",
        "",
        "## 审计对象（高风险 task）",
        "",
    ]
    for t in targets:
        lines.append(f"### {t['id']}: {t.get('title', '')}")
        if t.get("touched_files"):
            lines.append(f"- touched: {', '.join(t['touched_files'][:8])}")
        if t.get("commands_run"):
            lines.append(f"- last command: {t['commands_run'][-1]}")
        lines.append("")
    lines += [
        "## 审计重点",
        "",
        "1. premature 假设是否存在？缺的具体信号 X 是什么？",
        "2. 持久化/迁移/权限/并发/外部接口的边界条件是否覆盖？",
        "3. 失败路径、回滚、并发竞态是否处理？",
        "",
        "## 输出要求",
        "",
        "逐 blocker 举证，标 severity（CRITICAL / HIGH / MEDIUM / LOW）+ 置信度。",
        "先给最强反驳，禁止迎合。没问题的维度也明确说「核查通过」。",
        "",
        "### 结构化输出（必选）",
        "",
        "请在回复末尾用 JSON 代码块输出 findings 列表：",
        "",
        "```json",
        '[{"severity": "critical|high|medium|low", "claim": "问题描述", "evidence_to_check": "应核查的证据", "file": "涉及文件路径"}]',
        "```",
        "",
        "severity 字段必填，取值仅限 critical / high / medium / low。",
        "无发现时输出空数组 `[]`。",
        "",
    ]
    return "\n".join(lines)


# ---------- 派工指令 ----------

def _dispatch_hint(repo: Path, brief_path: Path, audit_dir: Path, auditors: list[str]) -> str:
    triad = _find_triad()
    rel_brief = brief_path.relative_to(repo) if _is_relative_to(brief_path, repo) else brief_path
    rel_replies = (audit_dir / "replies")
    rel_replies_str = rel_replies.relative_to(repo) if _is_relative_to(rel_replies, repo) else rel_replies
    lines = ["下一步：派异构三方审计（LTO 用 bundled delegate 编排，不自审）", ""]
    if triad:
        lines += [
            "  # 用 bundled delegate triad.sh 一键派工（有 tmux 时可观测）：",
            f"  bash {triad} \\",
            f"    -p {rel_brief} \\",
            f"    -d {rel_replies_str} \\",
            f"    -a \"{' '.join(auditors)}\" -t 300",
            "",
            "  # 审完收口（自动校验异构 + 抽 blocker 计数 + 判收敛）：",
            f"  python3 scripts/lto_run.py audit --collect {rel_replies_str}",
        ]
    else:
        lines += [
            "  # 未检测到 triad.sh。降级方案：",
            "  # 手动让 3 个不同家族的 AI 各读简报独立审，回复存到一个目录，",
            f"  # 文件名带 runtime（如 reply-codex.md / reply-agy.md），然后：",
            f"  python3 scripts/lto_run.py audit --collect <reply-dir>",
            "",
            "  # 若只能同模型自审：对抗性大幅缩水，collect 时加 --allow-same-family，",
            "  # 并在结论里显式声明「未做异构交叉」。",
        ]
    return "\n".join(lines)


def _find_triad() -> Path | None:
    env_path = os.environ.get("AGENT_DELEGATE_TRIAD")
    env_home = os.environ.get("AGENT_DELEGATE_HOME")
    repo_root = Path(__file__).resolve().parents[3]
    candidates = []
    if env_path:
        candidates.append(Path(env_path).expanduser())
    if env_home:
        candidates.append(Path(env_home).expanduser() / "scripts" / "triad.sh")
    candidates += [
        repo_root / "scripts" / "delegate" / "triad.sh",
        Path.home() / ".agents" / "skills" / "agent-delegate" / "scripts" / "triad.sh",
        Path.home() / "Projects" / "agent-delegate" / "scripts" / "triad.sh",
    ]
    for c in candidates:
        if c.is_file():
            return c
    return None


# ---------- ledger 收口 ----------

def _ensure_ledger(repo: Path, ledger_path: Path) -> None:
    if ledger_path.exists():
        return
    template = Path(__file__).resolve().parent.parent.parent.parent / "templates" / "audit-ledger.md"
    if template.exists():
        ledger_path.parent.mkdir(parents=True, exist_ok=True)
        ledger_path.write_text(template.read_text(encoding="utf-8"), encoding="utf-8")


def _next_round_label(ledger_path: Path) -> str:
    content = ledger_path.read_text(encoding="utf-8")
    nums = [int(m) for m in re.findall(r"^\|\s*R(\d+)\s*\|", content, re.MULTILINE)]
    # 模板里 R1 是占位空行；真实数据从第一次 collect 算起
    return f"R{(max(nums) + 1) if nums else 1}"


def _append_round(
    ledger_path: Path, round_label: str, artifact: str, auditors: str,
    high: int, critical: int, minor: int,
) -> None:
    content = ledger_path.read_text(encoding="utf-8")
    trend = _trend(content, high + critical)
    row = f"| {round_label} | {artifact} | {auditors} | {high} | {critical} | {minor} | {trend} | open |"
    # 把占位的空 R1 行（全空字段）替换为首条真实数据；否则在 Round Summary 表末追加
    placeholder = re.compile(r"^\|\s*R1\s*\|\s*\|\s*\|\s*\|\s*\|\s*\|\s*start\s*\|\s*open\s*\|\s*$", re.MULTILINE)
    if placeholder.search(content) and round_label == "R1":
        content = placeholder.sub(row, content, count=1)
    else:
        # 在 Round Summary 区块的最后一行表格行后插入
        content = _insert_after_round_table(content, row)
    ledger_path.write_text(content, encoding="utf-8")


def _trend(content: str, current_hc: int) -> str:
    prev = [
        (int(h), int(c))
        for h, c in re.findall(
            r"^\|\s*R\d+\s*\|[^|]*\|[^|]*\|\s*(\d+)\s*\|\s*(\d+)\s*\|", content, re.MULTILINE
        )
    ]
    if not prev:
        return "start"
    last_hc = prev[-1][0] + prev[-1][1]
    if current_hc < last_hc:
        return "down"
    if current_hc > last_hc:
        return "rebound"
    return "flat"


def _insert_after_round_table(content: str, row: str) -> str:
    lines = content.splitlines()
    out = []
    in_round = False
    inserted = False
    last_table_idx = -1
    for i, ln in enumerate(lines):
        if ln.strip().startswith("## Round Summary"):
            in_round = True
        elif in_round and ln.strip().startswith("## "):
            in_round = False
        if in_round and re.match(r"^\|\s*R\d+\s*\|", ln):
            last_table_idx = i
    if last_table_idx >= 0:
        lines.insert(last_table_idx + 1, row)
        inserted = True
    if not inserted:
        lines.append(row)
    return "\n".join(lines) + ("\n" if content.endswith("\n") else "")


def _scan_severity(replies: list[Path]) -> tuple[int, int]:
    """Regex fallback: 扫 reply 文本数 critical/high 出现次数。

    注意：这是兜底方案，有"没有 critical 问题"被误数为 1 的已知缺陷。
    结构化 JSON findings 优先（_parse_structured_reply），本函数只在 reply
    不含结构化 findings 时作为 fallback 使用。
    """
    high = critical = 0
    for r in replies:
        text = r.read_text(encoding="utf-8", errors="replace")
        critical += len(SEVERITY_PATTERNS["critical"].findall(text))
        high += len(SEVERITY_PATTERNS["high"].findall(text))
    return high, critical


def _run_ledger_check(repo: Path, ledger_path: Path) -> str | None:
    checker = Path(__file__).resolve().parent.parent.parent / "audit_ledger_check.py"
    if not checker.exists():
        return None
    proc = subprocess.run(
        [sys.executable, str(checker), "check", str(ledger_path)],
        capture_output=True, text=True,
    )
    for line in proc.stdout.splitlines():
        if line.startswith("verdict:"):
            return line.split(":", 1)[1].strip()
    return None


# ---------- 工具 ----------

def _is_relative_to(path: Path, base: Path) -> bool:
    try:
        path.resolve().relative_to(base.resolve())
        return True
    except ValueError:
        return False


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("audit", help="orchestrate adversarial heterogeneous audit + collect into ledger")
    p.add_argument("--run-id")
    p.add_argument("--task-id", nargs="*", help="force-audit these task IDs (overrides keyword scan)")
    p.add_argument("--collect", metavar="REPLY_DIR",
                   help="collect mode: read auditor replies from dir, append a ledger round")
    p.add_argument("--high", type=int, help="collect: manual HIGH blocker count (overrides heuristic scan)")
    p.add_argument("--critical", type=int, help="collect: manual CRITICAL count (overrides scan)")
    p.add_argument("--minor", type=int, help="collect: minor count")
    p.add_argument("--allow-same-family", action="store_true",
                   help="collect: allow same-family auditor as host (self-audit, low adversarial value)")
    p.add_argument("--auto-dispatch", action="store_true",
                   help="prepare: auto-spawn auditors via agent_exec instead of printing dispatch hints")
    p.add_argument("--discover-risks", action="store_true",
                   help="spawn adversarial risk-discovery agent to find unregistered risk points")
    p.set_defaults(func=run)


# ===========================================================================
# Backward-compatible self-test entry
# ===========================================================================


def _run_selftest() -> int:
    """Delegate audit self-tests to lto.test_audit in a child process."""
    proc = subprocess.run([sys.executable, "-m", "lto.test_audit"])
    return proc.returncode


if __name__ == "__main__":
    sys.exit(_run_selftest())
