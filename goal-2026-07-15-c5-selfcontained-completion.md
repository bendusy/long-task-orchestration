# Goal: dispatch-goal 完成信号自包含化——runner 端零 skill 依赖 + 修 pi/agy REPL 完成检测

> 致 codex：沿用约束（LTO 自管 dogfooding / 每 Phase 异构审计 / 红线不弱化 / commit 你写 release 归 host）。
> **这份只做 C5（完成信号协议），做完就停**——不做 audit brief 上下文注入、不做 pipe drain、不动 C1-C4 已交付逻辑。

## 为什么做（目标 + 第一性）

当前 dispatch-goal 的完成检测依赖链有两个硬伤（host 已亲验）：

1. **codex 路径依赖 runner 端第三方 skill**：`dispatch_goal.rs:491` 对 codex 发 `/goal <path>`，
   完成证明靠 codex 端装了 roostery goal-runtime skill（`update_goal` 工具调用出现在 Stop
   payload，`agent_turn.rs:326-380` 解析为 `codex-update-goal-complete`）。codex 没装该 skill
   时 `/goal` 不存在，任务根本起不来。**用户裁决：LTO 不再依赖 superpowers/goal-runtime 等
   任何 runner 端 skill 生态**——派工与完成检测必须是 LTO 自包含机制。
2. **pi/agy 的 process-exit 对 REPL 形同虚设**：`dispatch_goal.rs:500-562` 把 pi/agy 以交互
   REPL 方式 launch（`pi` REPL、`agy -i ''`），`process_exit_wrapper`（:567）要等**整个 REPL
   进程退出**才 fire 完成事件。REPL 完成 goal 后停在输入框**不会自己退出**→ 完成事件永远不
   fire → host 退化为 capture-pane 盯梢（2026-07-15 在 cc:1 窗口实证：host 30 秒一抓近 2 小时）。

第一性：完成**信号**（何时看）≠ 完成**判定**（是否真做完）。信号可以来自 agent 自报——因为判定
永远走 LTO 自己的闸门（done_when instrument、audit、closeout gate）。现在的 `update_goal` 证明
本质也是 agent 自报，只是借了 goal-runtime 的工具调用形态。所以自报信号不降低安全性，反而砍掉
了 skill 依赖。

## ⚠️ 必读：吸收的教训 / 前提

- **别信文档，以 `grep src/*.rs` 与二进制 `--help` 实证为准**；进行中的 untracked 新文件是预期
  状态，审计报 untracked 不当 blocker。
- agy `--print` 只出方案不执行（假成功陷阱），`-i` 是 value flag——`dispatch_goal.rs:521-538`
  的注释是血泪史，改动 launch 时逐条保留这些语义。
- `agent.turn.completed` 是 turn 级不是 goal 完成；C2-C4 实测 codex Stop hook 每轮 turn 都 fire。
- tmux prompt 粘贴有截断上限（agy 1000+ 字符被截过）：**给 REPL 的 prompt 保持短**，长内容留在
  goal 文件里让 agent 自己读。

## 核心架构裁决（host 已定，勿另起炉灶）

1. **新完成信号 = goal 文件尾部注入的自报命令**：`goal_prompt()`（dispatch_goal.rs:577）改为
   在 prompt 中明确要求——「goal 全部完成判据满足后，执行
   `lto agent-turn-completed --run-id <id> --runner <r> --source goal-self-report --rc 0 --window-id $LTO_WINDOW_ID --bell`；
   若被阻塞无法完成，用 `--rc 1` 并在 goal 文件同目录写 blocked 说明」。三家 runner 统一走这
   一条路径（codex 不再发 `/goal`，改用与 pi/agy 相同的 `goal_prompt()` 文本模式）。
2. **`agent_turn.rs` 增加 `goal-self-report` source 的路由**：视同 dispatch 完成（
   `dispatch_completed=true`），事件字段记 `completion_proof: "goal-self-report"`。现有
   `codex-stop-hook` / `*-process-exit` source **保留不删**——它们降级为旁路增强信号（装了
   goal-runtime 的 codex 依旧多一路证明；REPL 真退出时 process-exit 依旧 fire），但
   dispatch-goal 的 `completion_mode` 主路径统一为 `goal-self-report`。
3. **自报≠判定的防线显式化**：`agent.dispatch.completed` 事件带 `completion_proof` 字段后，
   closeout/check 的语义**不变**（它们本来就不看完成事件、只看 evidence/ledger）。禁止任何
   「self-report 直接放行闸门」的捷径。
4. **窗口回收语义**：`cleanup_on_success` 现在由 self-report rc=0 触发；rc≠0 或超时保留现场
   （与现状一致）。`agent_turn.rs:59-92` 的 effective_rc 逻辑对新 source 对齐。
5. **仓库内 superpowers 残留清理**：`references/specs/2026-07-14-plan-cybernetics-metastructure-redesign.md:3`
   的 "REQUIRED SUB-SKILL: superpowers:*" 行删除（specs 是 design 材料，删行不改判定语义）。
   全仓 `grep -ri superpower` 清零（.lto/ 除外）。

## Phase 1：goal-self-report 信号协议

- `goal_prompt()` 注入完成信号指令（含 rc=0/rc=1 两分支）；prompt 保持 ≤500 字符，长说明进
  goal 文件模板不进 prompt。
- codex 分支改走 `goal_prompt()`：`launch` 仍为 `LTO_RUN_ID=... codex`，prompt 不再是
  `/goal {goal}`。ready/confirm patterns 按实测调整。
- `agent_turn.rs` 路由 `goal-self-report`；单测覆盖：self-report rc=0 → dispatch_completed、
  rc=1 → 完成但 failed、无 run-id → 拒绝。
- 完成判据：`cargo test --locked agent_turn` 全绿；`grep -n "goal-self-report" src/` 覆盖
  dispatch_goal.rs + agent_turn.rs 两处以上。

## Phase 2：REPL 完成语义 + 三 runner 真机 smoke

- pi/agy 保留 REPL launch 与 process_exit_wrapper（旁路），主完成事件改 self-report。
- 真机验证（三家各一次，goal 用一个 trivial 测试 goal 文件）：
  `lto dispatch-goal --runner codex|pi|agy --goal <trivial-goal>` →
  `lto events --wait --event-type agent.dispatch.completed --timeout 900` 在 REPL **不退出**
  的情况下等到事件；`completion_proof=goal-self-report`；rc=0 时窗口被回收。
- 完成判据：三家 events.jsonl 各有一条 `agent.dispatch.completed` 且 `source=goal-self-report`；
  窗口回收行为与 rc 对应。

## Phase 3：文档 + 收口

- COMMANDS.md / references/playbooks/tmux-goal-loop.md / execution-loop.md 中「codex 用
  /goal、goal-state hook 才算完成」的表述改为 self-report 协议；hook 表述降级为「可选旁路」。
- specs 文件删 superpowers 行（裁决 5）。
- 全套 gate：`cargo fmt --all --check && cargo clippy --locked --all-targets -- -D warnings
  && cargo test --locked --all-targets && python3 scripts/check_docs_consistency.py
  && python3 scripts/check_python_rust_ownership.py && bash scripts/privacy_self_check.sh`。
- 每 Phase 收口跑 `lto audit --auto-dispatch`（自开 LTO run 跟踪，run-id 建议
  `20260715-c5-selfcontained-completion`）。

## 执行顺序

Phase 1 → 2 → 3，每 Phase 独立 commit 点。全部完成判据满足后 commit（message 你写），
**不 release / 不 tag / 不 push**（归 host）。

## 提醒（复用什么别重写）

- `agent_turn.rs` 的 payload 解析、window cleanup、事件 emit 全部复用，只加 source 分支。
- `process_exit_wrapper`、agy 空占位 launch、ready_patterns 的既有注释语义原样保留。
- 事件 schema 走 `event_emit.rs` 既有通道，隐私红线：prompt/goal 路径过 redact，不 inline
  goal 原文进事件。
- C2 的 readiness、C4 的 observability 闸门**不动**；本 goal 只碰信号层。
- **环境变量传递陷阱（host 盘出，先实证再定 prompt 形态）**：launch 命令现在只注入
  `LTO_RUN_ID`（dispatch_goal.rs:490,503,542）；`$LTO_WINDOW_ID` 是 process_exit_wrapper
  字符串里的 shell 展开，REPL 内 agent 的 bash 子进程**不一定**继承它。self-report 命令的
  window-id 来源三选一（实测后定）：① launch 时一并 export LTO_WINDOW_ID；② prompt 里直接
  内联字面 window-id（dispatch 时已知）；③ agent-turn-completed 允许缺 window-id 时按
  run-id 反查 dispatch 记录。倾向 ②（最简、不依赖环境继承）。
