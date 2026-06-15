"""Run-level budget contract — pure measurement, no side effects.

budget.py 只算不刹车：autopilot 调它来硬刹车，next/recap 调它来软警告。
纯函数：不读文件、不取系统时间。token 总数与当前时间由调用方注入
（LTO 脚本环境里取系统时间归命令入口的薄边界，纯计量层一律外部传入）。

设计不变量：
- 全可选，limit=None → 该维度不参与判定，永远 ok。
- 单维度：ratio < warn_ratio → ok；warn_ratio <= ratio < 1.0 → warn；ratio >= 1.0 → exceeded。
- overall = 三维度里最严的（exceeded > warn > ok）。
"""
from __future__ import annotations

from datetime import datetime
from typing import Any

_SEVERITY = {"ok": 0, "warn": 1, "exceeded": 2}


def dimension_status(*, limit: Any, used: float, warn_ratio: float) -> dict[str, Any]:
    """单维度判定。limit=None → 不参与（永远 ok，ratio=None）。"""
    if limit is None:
        return {"limit": None, "used": used, "ratio": None, "status": "ok"}
    ratio = used / limit if limit else float("inf")
    if ratio >= 1.0:
        status = "exceeded"
    elif ratio >= warn_ratio:
        status = "warn"
    else:
        status = "ok"
    return {"limit": limit, "used": used, "ratio": ratio, "status": status}


def _parse_iso(s: str) -> datetime | None:
    """容忍带/不带 Z 的 ISO。LTO state 里 started_at 用 iso_now() 产出本地无 tz ISO。"""
    if not s:
        return None
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def deadline_status(*, deadline, started_at: str, now: str, warn_ratio: float) -> dict[str, Any]:
    """deadline 维度：用 (now-started)/(deadline-started) 算进度比。
    now/started_at 由调用方注入（纯函数不取系统时间）。deadline=None → 永远 ok。
    """
    if not deadline:
        return {"limit": None, "used": now, "ratio": None, "status": "ok"}
    dl = _parse_iso(deadline)
    st = _parse_iso(started_at) if started_at else None
    nw = _parse_iso(now)
    if nw >= dl:
        return {"limit": deadline, "used": now, "ratio": 1.0, "status": "exceeded"}
    if st is None or dl <= st:
        # 无法算进度比（缺 started_at 或区间非法），退化为「未到 deadline = ok」
        return {"limit": deadline, "used": now, "ratio": 0.0, "status": "ok"}
    ratio = (nw - st).total_seconds() / (dl - st).total_seconds()
    status = "warn" if ratio >= warn_ratio else "ok"
    return {"limit": deadline, "used": now, "ratio": ratio, "status": status}


def check_budget(state: dict, token_total: int, now_iso: str) -> dict[str, Any]:
    """聚合 budget 状态。纯函数：token_total 与 now_iso 由调用方注入。
    缺 budget 块 / 缺字段 → 该维度 None → ok。overall = 三维度最严。
    warnings 是人话软警告行，供 next/recap 直接打印。
    """
    b = state.get("budget") or {}
    warn_ratio = b.get("warn_ratio", 0.8)
    started_at = state.get("started_at", "")

    turns = dimension_status(
        limit=b.get("max_turns"), used=b.get("turns_used", 0), warn_ratio=warn_ratio
    )
    tokens = dimension_status(
        limit=b.get("max_tokens"), used=token_total, warn_ratio=warn_ratio
    )
    deadline = deadline_status(
        deadline=b.get("hard_deadline"), started_at=started_at, now=now_iso, warn_ratio=warn_ratio
    )

    dims = {"turns": turns, "tokens": tokens, "deadline": deadline}
    overall = max((d["status"] for d in dims.values()), key=lambda s: _SEVERITY[s])

    warnings = []
    for name, d in dims.items():
        if d["status"] in ("warn", "exceeded") and d["ratio"] is not None:
            pct = int(d["ratio"] * 100)
            warnings.append(f"⚠️ budget: {name} {pct}% ({d['used']}/{d['limit']})")
    return {"overall": overall, "dimensions": dims, "warnings": warnings}
