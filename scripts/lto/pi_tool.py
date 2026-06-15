"""LTO Pi Tool — 将 LTO 命令暴露为 Pi 工具，让模型直接调用。

借鉴 pi-dynamic-workflows 的 tool-level 集成模式。
"""

from __future__ import annotations

import os, subprocess, sys
from pathlib import Path
from typing import Any

LTO_RUN = Path(__file__).resolve().parent.parent / "lto_run.py"
LTO_RS = Path(__file__).resolve().parent.parent.parent / "target" / "release" / "lto-rs"

# Tool definitions in OpenAI/MCP-compatible format
LTO_TOOLS = [
    {
        "name": "lto_start",
        "description": "启动一个新的 LTO 长任务。自动创建 .lto/<run-id>/state.json 和 run-state.md，安装 git hook。",
        "parameters": {
            "type": "object",
            "properties": {
                "goal": {"type": "string", "description": "任务目标"},
                "host": {"type": "string", "description": "宿主 runtime 名称", "default": "pi"},
                "profile": {"type": "string", "enum": ["minimal", "audit", "deploy"], "default": "minimal"},
                "with_audit": {"type": "boolean", "description": "是否生成 audit-ledger.md"},
                "install_hooks": {"type": "boolean",
                                  "description": "安装 LTO pre-commit 闸门到 .git/hooks（opt-in，默认 false，检测到 husky/pre-commit 会跳过）"},
            },
            "required": ["goal"],
        },
    },
    {
        "name": "lto_resume",
        "description": "恢复上次的 LTO 任务，打印上下文胶囊（phase/tasks/last_failure/next_action）。",
        "parameters": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "lto_runner",
        "description": "执行单个 task 并自动记录证据。成功时 task.status=done，失败时 task.status=blocked。",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "description": "task ID"},
                "kind": {"type": "string", "enum": ["test", "lint", "build", "manual", "review", "deploy"], "default": "test"},
                "command": {"type": "string", "description": "shell 命令"},
                "note": {"type": "string", "description": "人类可读摘要"},
                "touch": {"type": "array", "items": {"type": "string"}, "description": "修改的文件列表"},
                "timeout": {"type": "integer", "default": 300},
                "auto_commit": {"type": "boolean", "description": "提交 .lto 状态改动（opt-in，默认 false）"},
            },
            "required": ["task_id", "command"],
        },
    },
    {
        "name": "lto_judge",
        "description": "只读审查 runner 产出，输出 YAML verdict。",
        "parameters": {
            "type": "object",
            "properties": {
                "phase": {"type": "string", "description": "审查整个 phase"},
                "task_id": {"type": "string", "description": "审查单个 task"},
                "rerun_tests": {"type": "boolean", "description": "重新运行 task 中记录的测试"},
                "auto_commit": {"type": "boolean", "description": "提交 .lto 状态改动（opt-in，默认 false）"},
            },
        },
    },
    {
        "name": "lto_parallel",
        "description": (
            "并发批量执行多个 task 的 shell 命令并落 evidence。"
            "注意：这是 shell 命令批处理，不是 pi-dynamic-workflows 的 agent fan-out（同名不同义）。"
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "task_ids": {"type": "array", "items": {"type": "string"}, "description": "指定 task IDs"},
                "phase": {"type": "string", "description": "执行某 phase 下所有 pending task"},
                "concurrency": {"type": "integer", "default": 4},
                "command": {"type": "string", "description": "默认命令"},
                "auto_commit": {"type": "boolean", "description": "提交 .lto 状态改动（opt-in，默认 false）"},
            },
        },
    },
    {
        "name": "lto_pipeline",
        "description": (
            "让每个 task 串行通过多个 shell stage（item 间并发），每个 stage 落 evidence。"
            "stages 里用 {task_id} 占位符。注意：shell 命令批处理，非 agent fan-out。"
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "stages": {"type": "array", "items": {"type": "string"},
                           "description": "各 stage 的 shell 命令，用 {task_id} 占位"},
                "task_ids": {"type": "array", "items": {"type": "string"}, "description": "指定 task IDs"},
                "phase": {"type": "string", "description": "执行某 phase 下所有 task"},
                "concurrency": {"type": "integer", "default": 4},
                "continue_on_error": {"type": "boolean", "description": "某 stage 失败仍继续后续 stage"},
                "auto_commit": {"type": "boolean", "description": "提交 .lto 状态改动（opt-in，默认 false）"},
            },
            "required": ["stages"],
        },
    },
    {
        "name": "lto_closeout",
        "description": "闭环任务：验证闸门→标记 closed→生成 handoff.md→自动生成 CHANGELOG.md。",
        "parameters": {
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "任务总结"},
                "next_action": {"type": "string", "description": "下一步", "default": "none"},
                "blocked_by": {"type": "string", "description": "阻塞因素", "default": "none"},
                "auto_commit": {"type": "boolean", "description": "提交 .lto + CHANGELOG.md（opt-in，默认 false）"},
                "force": {"type": "boolean", "description": "强制闭环（跳过闸门）"},
            },
            "required": ["summary"],
        },
    },
    {
        "name": "lto_check",
        "description": "校验 LTO 状态完整性。",
        "parameters": {
            "type": "object",
            "properties": {
                "strict": {"type": "boolean", "description": "严格模式"},
            },
        },
    },
    {
        "name": "lto_preflight",
        "description": "即时探活环境（sandbox/network/git/mcp/tmux），输出健康报告。",
        "parameters": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "lto_hook",
        "description": "运行边界闸门检查。",
        "parameters": {
            "type": "object",
            "properties": {
                "gate": {"type": "string", "enum": ["pre-commit", "pre-deploy", "pre-closeout"]},
                "force": {"type": "boolean", "description": "强制通过"},
                "reason": {"type": "string", "description": "通过原因"},
            },
            "required": ["gate"],
        },
    },
    {
        "name": "lto_add_task",
        "description": "向当前 LTO run 添加新 task。",
        "parameters": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "description": "task ID（如 T3）"},
                "title": {"type": "string", "description": "task 标题"},
                "phase": {"type": "string", "description": "所属阶段", "default": "implementation"},
            },
            "required": ["task_id", "title"],
        },
    },
]


def execute_lto_tool(tool_name: str, args: dict[str, Any], repo: Path) -> str:
    """执行 LTO 工具调用，返回结果文本。"""
    try:
        cmd = _lto_cmd(repo)
    except Exception as e:
        return f"error: {e}"

    if tool_name == "lto_start":
        cmd += ["start", "--goal", args["goal"], "--host", args.get("host", "pi")]
        if args.get("with_audit"):
            cmd.append("--with-audit")
        if args.get("install_hooks"):
            cmd.append("--install-hooks")
        profile = args.get("profile", "minimal")
        if profile != "minimal":
            cmd += ["--profile", profile]
    elif tool_name == "lto_resume":
        cmd.append("resume")
    elif tool_name == "lto_runner":
        cmd += ["runner", "--task-id", args["task_id"], "--command", args["command"]]
        if args.get("kind"):
            cmd += ["--kind", args["kind"]]
        if args.get("note"):
            cmd += ["--note", args["note"]]
        if args.get("touch"):
            cmd += ["--touch"] + args["touch"]
        if args.get("timeout"):
            cmd += ["--timeout", str(args["timeout"])]
        if args.get("auto_commit"):
            cmd.append("--auto-commit")
    elif tool_name == "lto_judge":
        cmd.append("judge")
        if args.get("phase"):
            cmd += ["--phase", args["phase"]]
        if args.get("task_id"):
            cmd += ["--task-id", args["task_id"]]
        if args.get("rerun_tests"):
            cmd.append("--rerun-tests")
        if args.get("auto_commit"):
            cmd.append("--auto-commit")
    elif tool_name == "lto_parallel":
        cmd.append("parallel")
        if args.get("task_ids"):
            cmd += ["--task-ids"] + args["task_ids"]
        if args.get("phase"):
            cmd += ["--phase", args["phase"]]
        if args.get("concurrency"):
            cmd += ["--concurrency", str(args["concurrency"])]
        if args.get("command"):
            cmd += ["--command", args["command"]]
        if args.get("auto_commit"):
            cmd.append("--auto-commit")
    elif tool_name == "lto_pipeline":
        cmd += ["pipeline", "--stages"] + args["stages"]
        if args.get("task_ids"):
            cmd += ["--task-ids"] + args["task_ids"]
        if args.get("phase"):
            cmd += ["--phase", args["phase"]]
        if args.get("concurrency"):
            cmd += ["--concurrency", str(args["concurrency"])]
        if args.get("continue_on_error"):
            cmd.append("--continue-on-error")
        if args.get("auto_commit"):
            cmd.append("--auto-commit")
    elif tool_name == "lto_closeout":
        cmd += ["closeout", "--summary", args["summary"]]
        if args.get("next_action"):
            cmd += ["--next-action", args["next_action"]]
        if args.get("blocked_by"):
            cmd += ["--blocked-by", args["blocked_by"]]
        if args.get("auto_commit"):
            cmd.append("--auto-commit")
        if args.get("force"):
            cmd.append("--force")
    elif tool_name == "lto_check":
        cmd.append("check")
        if args.get("strict"):
            cmd.append("--strict")
    elif tool_name == "lto_preflight":
        cmd.append("preflight")
    elif tool_name == "lto_hook":
        cmd += ["hook", args["gate"]]
        if args.get("force"):
            cmd += ["--force", "--reason", args.get("reason", "")]
    elif tool_name == "lto_add_task":
        run_id = _current_run_id(repo)
        state_path = repo / ".lto" / run_id / "state.json"
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))
        from lto import state as st
        s = st.load_state(state_path)
        if s is None:
            return f"error: no state for {run_id}"
        phase = args.get("phase", s.get("current_phase", "implementation"))
        st.add_task(s, args["task_id"], args["title"], phase)
        st.save_state(state_path, s)
        return f"task {args['task_id']} added to phase {phase}"
    else:
        return f"unknown tool: {tool_name}"

    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        output = proc.stdout.strip() or proc.stderr.strip()
        if proc.returncode != 0:
            return f"[rc={proc.returncode}] {output}"
        return output
    except subprocess.TimeoutExpired:
        return "timeout (600s)"
    except Exception as e:
        return f"error: {e}"


def _current_run_id(repo: Path) -> str:
    current = repo / ".lto" / "current"
    if current.exists():
        return current.read_text(encoding="utf-8").strip()
    raise SystemExit("no active LTO run")


def _lto_cmd(repo: Path) -> list[str]:
    if os.environ.get("LTO_USE_PYTHON") == "1":
        return [sys.executable, str(LTO_RUN), "--repo", str(repo)]
    if not os.access(LTO_RS, os.X_OK):
        raise FileNotFoundError(
            f"Rust binary missing: {LTO_RS}; build with `cargo build --release --bin lto-rs` "
            "or set LTO_USE_PYTHON=1 for legacy fallback"
        )
    return [str(LTO_RS), "--repo", str(repo)]


# Pi extension entrypoint
def extension(pi: Any) -> None:
    """注册 LTO 工具到 Pi。"""
    for tool_def in LTO_TOOLS:
        tool_name = tool_def["name"]
        pi.registerTool({
            "name": tool_name,
            "description": tool_def["description"],
            "parameters": tool_def["parameters"],
            "execute": lambda args, tool_name=tool_name: execute_lto_tool(
                tool_name, args, Path.cwd()
            ),
        })
