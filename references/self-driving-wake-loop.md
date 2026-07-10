# 自驱动唤醒回路设计（dispatch → wake parent agent, no polling）

> 状态：设计 spec，待实现。2026-06-20 写。
> 来源：业界调研（hcom 343★ / Agent Room / Hook Shim Pattern）+ hcom 源码深调。
> 决策：host 已裁 ① 传输层 = TCP connect-drop（抄 hcom）② 范围 = 完整四件套 ③ 跨 runtime 通用。
>
> **2026-07-10 现状修正**：本文保留为历史设计记录。Codex Stop 是 per-turn，不能代表 goal 完成；当前 waiter 使用 `agent.dispatch.completed`。Codex 只有在 transcript 出现真实 `update_goal complete` 时发该事件，pi/agy 使用 TUI 进程退出的真实 rc。下文把 Stop/`agent.turn.completed` 写成“任务完成”的段落不再是当前运行指引。

## 为什么做（第一性）

LTO 派工后，完成信号断在最后一跳：

```
codex/agy Stop-hook → lto agent-turn-completed → events.jsonl(带 event_id)   ✅ 现状
                                                  └→ ❌ 无 wake：主 agent 不知道，要人提醒
```

代码实证：`agent_turn.rs:66` 写完 telemetry 就返回，无 notify；全 repo grep 无 `watch/inotify/wake/FIFO`。
对比 Claude Code Agent 工具子代理有 `task-notification` 自动唤醒——LTO 缺这个等价物，所以无法自驱动。

**机理（业界共识）**：Claude Code 模型看不到 MCP notification，除非用户敲字。所以解法**必须是 hook 回灌 / 输出尾随注入，不能是 MCP push**。这条否决了"用 MCP 通知主 agent"。

## ⚠️ 必读：吸收的教训

1. **唤醒不是单一机制，按 runtime 分两类**（hcom 源码实证，纠正常见误传）：
   - **hook 型**（claude/cursor/gemini…）：有生命周期 hook → Stop-hook 欲停时回灌 `decision:"block"`。
   - **hookless 型**（codex/adhoc）：无 hook → 消息**追加在 CLI 命令输出之后**，agent 下次跑任何 lto 命令时尾部带未读。
   - 跨 runtime 通用 = 必须 per-runtime adapter 分流，不能统一 Stop-hook。
2. **hcom 0 处用 `stop_hook_active`**：用显式状态机 + panic guard 防死循环，不依赖 Anthropic 的 flag。
3. **hook 永不崩 turn**：所有 hook 逻辑用 `catch_unwind` 包裹，任何失败 exit 0 / 返回 fallback。这是硬契约。
4. **events.jsonl 已有单调 event_id**（events.rs:93）——游标用 event_id，比 byte-offset 更稳，无需改 SQLite。

## 核心架构裁决

| 维度 | 裁决 | 理由 |
|---|---|---|
| 传输层 | **TCP connect-drop**（TcpListener 随机端口 + connect-and-close ping） | 抄 hcom，纯 std/nix，跨进程跨 runtime，近实时；`unsafe_code=forbid` 兼容（nix poll 不需 unsafe） |
| 游标 | **event_id**（存 `.lto/<run-id>/events.cursor`） | event_id 已单调；append-only seek 续读比 SQLite 简单 |
| 端口注册 | `.lto/<run-id>/notify-endpoints.json` | 无 DB，小文件存 (waiter_id, port) |
| 唤醒范围 | 跨 runtime，per-runtime adapter | host 裁决；codex hookless / cc hook 双协议 |
| 死循环防护 | 游标水位 + wake 预算上限 | 已消费事件不再唤醒；上限防空转挂死 |
| 失败模式 | hook/wake 失败一律 exit 0 | 抄 Shim 纪律，不 stall 主 agent |

## 四件套（分阶段，每阶段独立可收口）

### Phase 1：`lto events --wait` 阻塞原语【地基】

新增 `Commands::Events`（cli.rs:276 命令注册区），子命令 `--wait`：

```
lto events --wait --run-id <id> --event-type agent.turn.completed \
  [--after <event_id>] [--timeout <secs>] [--json]
```

实现（抄 hcom `events.rs:800-880` 三段式，落点 `src/events.rs` 旁新增 `wait_for`）：
1. **lookback 预检**：从 cursor（或 `--after`）到 EOF 有无匹配 → 有则立即返回（防"事件早于 wait 启动"竞态）。复用 `events::read()`（events.rs:135）+ 内存过滤 event_type。
2. **注册 endpoint**：`TcpListener::bind("127.0.0.1:0")` 拿随机端口 → 写 `.lto/<run-id>/notify-endpoints.json`。
3. **主循环**：`last_id` 始终推进；`SELECT id > last_id` 等价为 seek 续读；无新事件则 `poll(2)` 阻塞最多 30s 或被 connect 唤醒；超时返回。
4. 命中后推进 `events.cursor`，打印事件（`--json` 出结构化）。

完成判据：`cargo test events::wait_for` 覆盖 ①lookback 命中 ②阻塞被 wake 唤醒 ③timeout 返回 ④游标推进。

### Phase 2：TCP connect-drop 唤醒 + 人在环信号【传输层】— ✅ 已实现

`src/notify.rs`（对标 hcom `notify/server.rs` + `wake.rs`）：
- `NotifyServer::register(repo, run_id, waiter_id) -> Server`：非阻塞 `TcpListener` 绑随机端口，写进 `.lto/<run-id>/notify-endpoints.json`；`Drop` 自动注销。
- `drain()`：非阻塞 `accept()` 排空，返回是否被唤醒。
- `wake_run(repo, run_id)`：读 `notify-endpoints.json` → 对每个 waiter port `TcpStream::connect_timeout` connect-and-close（100ms）。

**实现裁决（偏离 spec 草案）**：hcom 的 `wait()` 用 `nix::poll` + `unsafe { BorrowedFd::borrow_raw }`（源码实证 server.rs:47）——这违反 LTO `unsafe_code = "forbid"`。改用**纯 std**：非阻塞 listener + `accept()` 轮询（hcom 自己的测试 helper 就这么做）。零新依赖、零 unsafe，唤醒延迟降到 poll-tick 级（仍远优于 Phase 1 的 500ms）。

**人在环三路（吸收用户建议：iaf + tmux bell）**——`agent-turn-completed` 写完事件后统一发，全部可选、best-effort、绝不 fail turn：
1. `notify::wake_run` — 机器唤醒主 agent（machine→machine）。
2. `--bell` — 终端/tmux BEL，本地人注意（machine→local human）。
3. `--notify-cmd "<模板>"` — host 自配通知器。可信内部字段用 `{run_id}`/`{runner}`/`{rc}` 占位符；**不可信的 summary（runner 输出）经 `$LTO_SUMMARY` 环境变量传入**，不内联进 shell 字符串，杜绝命令注入（审计 #3）。LTO **不硬编码** iaf 等私有工具，保持可移植；host 在派工时传 iaf 命令即可（machine→remote human）。

接入点：`agent_turn.rs`（写完 agent.turn.completed 事件、telemetry::save 后）依次触发上述三路。

完成判据（已验）：`notify::wake_unblocks_a_registered_server`、endpoint 注册/Drop 注销、`wake_run` 无 endpoint 安全、`run_notify_cmd` 占位符替换 + 失败吞掉。242 测试全绿。

### Phase 3：per-runtime adapter 双协议【跨 runtime】

`src/hooks/`（对标 hcom `hooks/`，每家一文件，共享 `common.rs`）：
- **共享层** `common.rs`：`dispatch_with_panic_guard`（`catch_unwind` 包裹，panic 返回 fallback）+ pre-gate（无活跃 wait 直接 skip exit 0）。
- **hook 型** `claude.rs`：Stop-hook 入口，查 cursor→EOF 有无未消费 agent.turn.completed → 有则输出 `{"decision":"block","reason":<事件摘要>}`，无则 `{}`。状态机 ACTIVE/BLOCKED/LISTENING（不用 stop_hook_active）。
- **hookless 型** `codex.rs`：无 block，未读事件**尾随注入**到下次 lto 命令输出（对标 hcom `cli_context.rs:172`）。

dispatch_goal.rs:218（codex completion_event）/ :258（agy）已分家，天然契合双协议分流。

完成判据：cc 路径产出 decision:block JSON；codex 路径产出尾随注入；hook panic 时 exit 0 不崩。

### Phase 4：游标 + 死循环防护 + 预算【收口】

- `.lto/<run-id>/events.cursor`：JSON `{waiter_id, last_consumed_event_id}`，消费后推进。已消费事件不再触发 block（防同一事件无限唤醒）。
- wake 预算上限：`--max-wakes`（默认参考 hcom 60×30s≈30min）封顶空转。
- 失败模式全 exit 0；endpoint 文件 stale（进程死）时 connect 失败静默跳过。

完成判据：同一完成事件只回灌一次；超预算优雅退出；stale endpoint 不报错。

## 执行顺序 + 每 Phase 收口

```
Phase 1 (events --wait, 纯读, 零风险) → Phase 2 (notify TCP, 单测可验)
  → Phase 3 (per-runtime hooks, 双协议) → Phase 4 (游标+防护+预算)
```

每 Phase 收口：`cargo fmt --all --check` + `cargo clippy --locked --all-targets -- -D warnings` + `cargo test --locked --all-targets` + `python3 scripts/check_docs_consistency.py`。

## 复用什么（勿重写）

- `events::read()`（events.rs:135）、`events_path()`（:163）、event_id 游标（:93）——wait 直接复用，不重造读取。
- `agent_turn.rs:66`——wake 注入点，事件已写好，只追加一行。
- dispatch_goal.rs 的 codex/agy completion_event 分家（:218/:258）——双协议分流天然落点。
- TmuxRunner 的 Signal/Sentinel/Fire 完成检测——已有，不动。

## 不可越的红线（CLAUDE.md 原则1/3）

- wake 只是**通知主 agent 有事件可看**，不替它决策路线。decision:block 回灌的是事件事实，不是指令。
- 不按历史 telemetry 自动路由；不自动 promote/deploy/push。
- 自动化仍是梯度：wake 让主 agent 不需人提醒就 react，但**做什么仍由主 agent + 人决定**。
- 失败 fail-safe：宁可漏唤醒（退回人提醒）也不崩主 agent turn。
