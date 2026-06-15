"""heartbeat.py — P0-1 层 1：结构化心跳（事实源）+ runs --watch 汇总。

为什么需要它（用户 2026-06-15 实证）：派 pi/agy 这类 headless runner 后台跑，
若 runner 无流式输出，scheduler 的 live log（<job_id>.log）就一直空——host 只能
反复手动 poll，用户反复问「做好了吗」。

心跳补这个洞：即便 runner 一行流式输出都没有，scheduler 也每 ~30s 向旁车文件
<job_id>.hb.jsonl 写一行结构化心跳 {ts, job_id, runner, elapsed_sec, phase, alive}，
证明「还活着、跑了多久」。`runs --watch` 一行汇总所有在跑 job，host 一条命令看全。

设计取舍：
- 心跳写**独立旁车** .hb.jsonl，不写进二进制 stdout tee 的 .log——避免结构化
  JSON 与原始字节流交织互相污染（.log 是 runner stdout 原样，.hb.jsonl 是机械事实）。
- 纯机械事实：不判断、不推荐，对齐 LTO「机械事实 + host 消费」分层。
- progress.py 是 autopilot stall 闸门（防伪推进），不是这里的进度汇报，互不相干。
"""

from __future__ import annotations

import json
import time
from pathlib import Path

# 心跳写入间隔（秒）。scheduler 的心跳线程按此节奏写一行。
HEARTBEAT_INTERVAL_SEC = 30.0


def format_heartbeat(
    ts: float,
    job_id: str,
    runner: str,
    elapsed_sec: float,
    phase: str,
    alive: bool,
) -> str:
    """格式化一行结构化心跳为 JSON 字符串（不含尾随换行，调用方负责加）。

    纯函数，无副作用——便于测试。字段固定六个：
      ts          心跳写入时刻（time.time() 墙钟，供 runs --watch 算距今多久）
      job_id      job 标识
      runner      runner 名（codex/pi/claude/agy）
      elapsed_sec job 已跑多久（从 exec 开始算，圆整到 3 位）
      phase       阶段标签（当前固定 "running"，留扩展位）
      alive       是否还活着（心跳本身即活着的证明 → True；收尾可补一条 alive=False）
    """
    return json.dumps(
        {
            "ts": round(float(ts), 3),
            "job_id": str(job_id),
            "runner": str(runner),
            "elapsed_sec": round(float(elapsed_sec), 3),
            "phase": str(phase),
            "alive": bool(alive),
        },
        ensure_ascii=False,
        sort_keys=True,
    )


def heartbeat_path(live_log_path: Path | None) -> Path | None:
    """从 live log 路径（…/<job_id>.log）推导心跳旁车路径（…/<job_id>.hb.jsonl）。

    live_log_path 为 None（未解析出 run-id，优雅降级）时返回 None。
    """
    if live_log_path is None:
        return None
    return live_log_path.with_suffix(".hb.jsonl")


def read_last_heartbeat(hb_path: Path) -> dict | None:
    """读旁车里最后一条**有效** JSON 心跳。坏行/缺文件/空文件 → None，绝不抛。"""
    try:
        text = hb_path.read_text(encoding="utf-8")
    except OSError:
        return None
    last: dict | None = None
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        if isinstance(obj, dict):
            last = obj
    return last


def scan_live_heartbeats(
    repo: Path,
    run_id: str | None,
    now: float | None = None,
) -> list[dict]:
    """汇总某个 run 下所有 job 的最新心跳，供 runs --watch 一行展示。

    每行：{job_id, runner, elapsed_sec, last_hb_age_sec, reply_ready, alive}。
    - last_hb_age_sec：距最后一次心跳多久（now - ts），越大越可疑（可能闷死）。
    - reply_ready：同目录 <job_id>.reply.txt 是否存在（host 判「能收口了吗」）。

    run_id 为 None 时从 .lto/current 解析。无 live 目录 → []（优雅降级，不抛）。
    """
    if now is None:
        now = time.time()
    repo = Path(repo)

    if not run_id:
        try:
            run_id = (repo / ".lto" / "current").read_text(encoding="utf-8").strip()
        except OSError:
            return []
    if not run_id:
        return []

    live_dir = repo / ".lto" / run_id / "live"
    if not live_dir.is_dir():
        return []

    rows: list[dict] = []
    for hb_path in sorted(live_dir.glob("*.hb.jsonl")):
        hb = read_last_heartbeat(hb_path)
        if hb is None:
            continue
        job_id = str(hb.get("job_id") or hb_path.name[: -len(".hb.jsonl")])
        ts = hb.get("ts")
        try:
            age = round(now - float(ts), 3) if ts is not None else None
        except (TypeError, ValueError):
            age = None
        reply_ready = (live_dir / f"{job_id}.reply.txt").is_file()
        rows.append(
            {
                "job_id": job_id,
                "runner": str(hb.get("runner") or "?"),
                "elapsed_sec": hb.get("elapsed_sec"),
                "last_hb_age_sec": age,
                "reply_ready": reply_ready,
                "alive": bool(hb.get("alive", True)),
            }
        )
    return rows


def format_watch_table(rows: list[dict]) -> str:
    """渲染心跳汇总为人读的多行文本。纯函数。"""
    if not rows:
        return "  (no live jobs — no .lto/<run>/live/*.hb.jsonl heartbeats found)"

    lines = [
        "  RUNNER    ELAPSED   LAST-HB   REPLY  JOB",
    ]
    for r in rows:
        runner = str(r.get("runner") or "?")
        elapsed = r.get("elapsed_sec")
        elapsed_s = f"{elapsed:.0f}s" if isinstance(elapsed, (int, float)) else "?"
        age = r.get("last_hb_age_sec")
        age_s = f"{age:.0f}s ago" if isinstance(age, (int, float)) else "?"
        reply = "ready" if r.get("reply_ready") else "-"
        alive = "" if r.get("alive", True) else " (DONE)"
        job = str(r.get("job_id") or "?")
        lines.append(
            f"  {runner:<8}  {elapsed_s:>7}   {age_s:>9}   {reply:<5}  {job}{alive}"
        )
    return "\n".join(lines)
