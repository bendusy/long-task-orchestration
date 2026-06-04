"""AgentJob / AgentResult — agent 世界的数据合同（harness 地基）。

这是 spawn 原语(agent_exec)、调度器(scheduler)、事实简报器(lto next)
三方共享的单一真源。刻意区别于 exec.py 的 shell 世界合同
（command/cwd/rc/stdout/stderr）——agent harness 编排的是带独立 context 的
agent，不是 shell 命令。

设计原则（codex 异构审纠正）：
- 不用 run_command 的 shell 签名锁死 spawn，否则 host agent 的组合空间会被卡住。
- 每个字段都对应一个真实消费者：
  · scheduler 读 budget / retry_policy / runner（并发、退避、限流）
  · agent_exec 读 prompt_ref / runner / model / isolation / output_schema
  · lto next 读 parent_pattern / verifier_of / children（给 host agent 的编排事实）
"""

from __future__ import annotations

from dataclasses import dataclass, field, asdict
from enum import Enum
from typing import Any


# 编排 pattern（官方文章中 pattern 的实用子集，按真实需求增量铺开）
class Pattern(str, Enum):
    LINEAR = "linear"            # 单 agent 顺序
    FAN_OUT = "fan-out"          # 拆多步并发 + barrier 合成
    ADVERSARIAL = "adversarial"  # 每个 generator 配独立 verifier
    TOURNAMENT = "tournament"    # placeholder-only：枚举占位，无真实触发场景未实现（YAGNI）
    LOOP = "loop"                # placeholder-only：枚举占位，无真实触发场景未实现（YAGNI）


# 已知 runner 家族（与 audit.py 的家族映射保持一致，跨 runtime 异构判定用）
KNOWN_RUNNERS = ("codex", "pi", "agy", "gemini", "claude")
CODEX_SANDBOXES = ("read-only", "workspace-write", "danger-full-access")


class JobStatus(str, Enum):
    PENDING = "pending"
    RUNNING = "running"
    OK = "ok"
    FAILED = "failed"
    TIMEOUT = "timeout"
    RATE_LIMITED = "rate_limited"
    SKIPPED = "skipped"


@dataclass
class PermissionPolicy:
    """Per-job permission intent, used by scheduler/runner env guards."""
    sandbox: str = "read-only"
    reason: str = ""
    user_approved: bool = False

    def validate_for_runner(self, runner: str, env: dict[str, str]) -> None:
        if not all(isinstance(k, str) and isinstance(v, str) for k, v in env.items()):
            raise ValueError("env keys and values must be strings")

        if runner != "codex":
            return

        sandbox = env.get("CODEX_SANDBOX", self.sandbox)
        if sandbox != self.sandbox:
            raise ValueError(
                "CODEX_SANDBOX conflicts with permission_policy.sandbox "
                f"({sandbox!r} != {self.sandbox!r})"
            )
        if sandbox not in CODEX_SANDBOXES:
            raise ValueError(f"invalid codex sandbox: {sandbox!r}")
        if sandbox == "workspace-write" and not self.reason.strip():
            raise ValueError("workspace-write requires permission_policy.reason")
        if sandbox == "danger-full-access":
            if not self.reason.strip():
                raise ValueError("danger-full-access requires permission_policy.reason")
            if not self.user_approved:
                raise ValueError("danger-full-access requires user_approved=True")

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class Budget:
    """单 job 资源预算。scheduler 据此限流与计量。"""
    timeout_sec: int = 300
    max_tokens: int | None = None      # 可选 token 上限（成本计量）

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class RetryPolicy:
    """重试 + 退避。scheduler 在 rate_limited / timeout 时据此重试。"""
    max_retries: int = 1
    backoff_sec: float = 5.0           # 首次退避，指数增长
    retry_on: tuple[str, ...] = ("rate_limited", "timeout")

    def to_dict(self) -> dict[str, Any]:
        return {**asdict(self), "retry_on": list(self.retry_on)}


@dataclass
class AgentJob:
    """一个待调度的 agent 任务（host-agent-facing 合同）。"""
    job_id: str
    prompt_ref: str                                # 简报文件路径或内联文本
    runner: str = "codex"                          # KNOWN_RUNNERS 之一
    prompt_is_inline: bool = False                 # True=prompt_ref 是文本不是路径
    model: str | None = None                       # 可选，model routing 用
    env: dict[str, str] = field(default_factory=dict)  # per-job runner env (CODEX_SANDBOX, etc.)
    permission_policy: PermissionPolicy = field(default_factory=PermissionPolicy)
    isolation: str = "none"                        # none | worktree
    output_schema: dict[str, Any] | None = None    # 强制审者结构化输出（findings/severity）
    parent_pattern: str = Pattern.LINEAR.value     # 本 job 属于哪个编排 pattern
    budget: Budget = field(default_factory=Budget)
    retry_policy: RetryPolicy = field(default_factory=RetryPolicy)
    verifier_of: str | None = None                 # 若本 job 对抗验证某 job，指向其 job_id
    children: list[str] = field(default_factory=list)  # fan-out 子 job_id
    meta: dict[str, Any] = field(default_factory=dict)  # 自由扩展（task_id 等）

    def validate(self) -> None:
        if self.runner not in KNOWN_RUNNERS:
            raise ValueError(f"unknown runner: {self.runner!r} (known: {KNOWN_RUNNERS})")
        if self.isolation not in ("none", "worktree"):
            raise ValueError(f"invalid isolation: {self.isolation!r}")
        valid_patterns = {p.value for p in Pattern}
        if self.parent_pattern not in valid_patterns:
            raise ValueError(f"invalid parent_pattern: {self.parent_pattern!r}")
        if not self.prompt_ref:
            raise ValueError("prompt_ref is required")
        self.permission_policy.validate_for_runner(self.runner, self.env)

    def to_dict(self) -> dict[str, Any]:
        d = asdict(self)
        d["budget"] = self.budget.to_dict()
        d["retry_policy"] = self.retry_policy.to_dict()
        d["permission_policy"] = self.permission_policy.to_dict()
        return d

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "AgentJob":
        d = dict(d)
        if "budget" in d and isinstance(d["budget"], dict):
            d["budget"] = Budget(**d["budget"])
        if "retry_policy" in d and isinstance(d["retry_policy"], dict):
            rp = dict(d["retry_policy"])
            if "retry_on" in rp and isinstance(rp["retry_on"], list):
                rp["retry_on"] = tuple(rp["retry_on"])
            d["retry_policy"] = RetryPolicy(**rp)
        if "permission_policy" in d and isinstance(d["permission_policy"], dict):
            d["permission_policy"] = PermissionPolicy(**d["permission_policy"])
        known = {f for f in cls.__dataclass_fields__}
        return cls(**{k: v for k, v in d.items() if k in known})


@dataclass
class AgentResult:
    """一个 agent 任务的执行结果。"""
    job_id: str
    runner: str
    status: str = JobStatus.PENDING.value
    exit_code: int | None = None                   # 区分 124 timeout / 0 空返回 / 非零失败
    findings: list[dict[str, Any]] = field(default_factory=list)  # 结构化（非 regex 扫文本）
    reply_text: str = ""                           # 原始回复（findings 解析失败时兜底）
    cost: dict[str, Any] = field(default_factory=dict)  # tokens / elapsed_sec
    permissions: dict[str, Any] = field(default_factory=dict)  # runner sandbox/env decision snapshot
    artifacts: list[str] = field(default_factory=list)  # reply 文件路径等
    attempts: int = 1                              # 实际尝试次数（含重试）
    error: str = ""                                # 失败时的诊断信息

    @property
    def ok(self) -> bool:
        return self.status == JobStatus.OK.value

    def severity_counts(self) -> dict[str, int]:
        """从结构化 findings 数 severity（P1 去 regex 的基础）。"""
        counts = {"critical": 0, "high": 0, "medium": 0, "low": 0}
        for f in self.findings:
            sev = str(f.get("severity", "")).lower()
            if sev in counts:
                counts[sev] += 1
        return counts

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "AgentResult":
        known = {f for f in cls.__dataclass_fields__}
        return cls(**{k: v for k, v in d.items() if k in known})
