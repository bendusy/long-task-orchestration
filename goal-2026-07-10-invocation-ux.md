# Goal: LTO 调用层优化——治无头惯性 + 完成通知闭环 + 窗口生命周期

> 致 codex：沿用约束——LTO 自管（你自己 `lto start` 起 run、task-add、每 Phase 异构审计、closeout），dogfooding，红线不弱化，commit 你写、release 归 host。
> **这份只做下面 3 个 Phase，做完就停。** 不做：weakness export、held-out eval gate、claude runner 支持、audit runner 优先级（backlog⑪ 已撤回该方向，别顺手实现）。

## 为什么做（目标 + 第一性）

host 观测到两类真实痛点（多轮实战 + am 记忆沉淀）：

1. **agent 调 LTO 习惯走 headless 而不是 tmux 真 TUI**。SKILL.md:144-145 已写"开发型派工必须 tmux"，但只是文案——工具层不拦，`lto runner` 默认就是 headless codex（cli.rs:474-479），delegate.sh runners 全是 `--print`/exec。agy print 空转假成功（rc=66 探测是补丁不是根治）、pi 管道毁 TTY，根因都是"开发型任务被 headless 接走"。文案挡不住惯性，要 fail-closed 的工具层闸门。
2. **主 agent 收不到完成通知**。完成 hook 三家（codex/agy/pi）都已实装且都调 `lto agent-turn-completed`，但：codex hook 漏 `--bell`（与 SKILL.md:34 "三者都带 --bell" 不符）；通知通道只有 TCP wake（notify.rs 只唤 `events --wait` 的 waiter），主 agent 不主动挂 wait 就永远不知道；`--notify-cmd` 机制在 agent_turn.rs 已存在但 dispatch-goal 没贯通。
3. **（host 新增）派工窗口用完不清理**。dispatch-goal 路径全程只有 `tmux new-window`（tmux_runner.rs:428），无任何 kill-window；窗口堆积还引发编号漂移（index 锚定的 target 在人工关窗后失效，实战漂过 2 次）。

## ⚠️ 必读：前提与坑（吸收的教训）

- **别信文档声称，以 grep 源码为准**。本 goal 的一个动机就是 SKILL.md:34 与 codex-stop-notify.sh 实际不符（无 --bell）。改完代码后同步改文档，别反过来。
- **进行中的 untracked 新文件是预期状态**，审计若报 untracked 为 CRITICAL，记录但不当 blocker，commit 时消解。
- **只读评审 headless 是特性不是 bug**（2026-06-25 实证：agy headless 评审正常产出方案专页）。Phase 1 的闸门只拦"写档权限 + 非 tmux"，绝不能误伤 audit/评审路径（audit_dispatch.rs:161-181 的 readonly 注入别动）。
- **agy.sh 的空转探测（agy.sh:47-51 rc=66）保留**，它是 headless 评审场景的最后防线。
- **ready_patterns 已存在且正确**（codex="gpt-" dispatch_goal.rs:229 / pi="deepseek","ctx" :265 / agy="? for shortcuts" :312），别重写探测逻辑，Phase 3 只改超时默认值和锚定方式。
- 慢 runner 审计：异构审计快 runner 优先（codex/agy），pi 留补充不做关键路径阻塞。

## 核心架构裁决（host 已定，别猜别改方向）

1. **闸门轴 = 权限档而非任务分类学**：判"开发型"不新造 enum，直接用已有 permission_policy——写档（workspace-write 及以上）+ 非 tmux runner ⇒ fail-closed 拒绝，错误信息给两条出路（`lto dispatch-goal --runner <r>` / 显式 `--allow-headless-write`）。readonly 权限 headless 一切照旧。
2. **通知不硬编码任何工具**（守 cli.rs:574-577 的设计）：dispatch-goal 贯通 `--notify-cmd`（存 state，hook 触发时经 run_notify_cmd 执行，summary 走 `$LTO_SUMMARY` env 防注入的现有机制），不默认注入任何命令。主 agent 感知的正道 = 派工后挂 `lto events --wait`，dispatch-goal 输出里给 ready-to-copy 的等待命令。
3. **窗口清理只清自己的**：kill-window 仅限 state 里记录的、dispatch-goal 自己创建的 window（用不可变 `#{window_id}` @N 锚定，不用会漂的 index）。成功默认清理，失败/timeout 默认保留现场（`--keep-window` / `--cleanup-window=never|always|on-success` 可配）。绝不碰非 LTO 窗口——这是红线。
4. **controller-in-chief 不变量不弱化**（CLAUDE.md:104）：本 goal 全部是"机械步骤自动化"，不新增任何 LTO 自动决策/route/promote。

## Phase 1：headless 写权限闸门

**缺陷**：开发型（写档）派工可以静默走 headless，agy 假成功、可观测性为零。

**要求**：
- 落点 `src/agent_job.rs:177-255`（权限 fail-close 区）：permission_policy 为写档且 runner ∉ {tmux} 时拒绝，错误信息含两条出路（见裁决 1）。新 flag `--allow-headless-write`（cli.rs RunnerCommand 附近，参考现有 danger 权限 `user_approved` 的实现形状）。
- `scripts/delegate/delegate.sh`：默认按只读评审用途运行；新增显式 `--write` flag 才放开写档派工，放开时 stderr 打印一行推荐 dispatch-goal 的警告。
- SKILL.md 派工章节同步：把"开发型必须 tmux"从建议改为"工具层已强制，逃生口是 --allow-headless-write"。

**测试**：单测 ①headless+写档默认拒绝且错误信息含 dispatch-goal 指引 ②带 --allow-headless-write 放行 ③readonly headless 行为与现状完全一致（audit 路径回归）。

**完成判据**：`cargo test` 新增用例全绿；`lto runner --runner codex`（默认写档？以现状为准，若默认就是写档则该命令裸跑应被拒）+ `lto audit` 冒烟不受影响。独立 commit。

## Phase 2：完成通知闭环

**缺陷**：主 agent 派完工就"失聪"；codex hook 与文档不符。

**要求**：
- `scripts/hooks/codex-stop-notify.sh` 补 `--bell`（对齐 agy/pi 两个 hook）。
- dispatch-goal 增加 `--notify-cmd <tmpl>`：存入 run state；`cmd_agent_turn_completed`（agent_turn.rs:82-96 收尾三跳）读取该 run 的 notify_cmd 并经现有 `run_notify_cmd`（agent_turn.rs:103-118）执行。先核实 notify_cmd 现在从哪来（CLI flag / config / state），复用现有管道别开新路。
- dispatch-goal 成功输出末尾打印 ready-to-copy 的等待命令，含真实 run-id：`lto events --wait --event-type agent.turn.completed --run-id <rid> --timeout <n>`（`dispatch-and-wait` 已存在 cli.rs:549-550，输出里一并提及）。
- SKILL.md：派工节写死"派完即在后台挂 events --wait（或直接用 dispatch-and-wait）"，附主 agent（Claude Code）用法示例。

**测试**：端到端——派一个最小 goal，完成后断言 ①bell 字节输出（或 hook 调用参数含 --bell）②notify-cmd 被执行（用写哨兵文件的 notify-cmd 验证）③events --wait 在完成事件后即刻返回 exit 0。

**完成判据**：端到端脚本可重复跑通；SKILL.md 声称与代码一致（grep 复核 --bell 三家齐）。独立 commit。

## Phase 3：窗口 ID 锚定 + 可辨识窗口名 + 完成自动清理 + ready 超时

**缺陷**：index 锚定漂移；**窗口名无辨识度**；窗口不清理；ready 默认 20s 对 pi/glm 冷启太短。

**要求**：
- **可辨识窗口名（host 真机实证的痛点，优先做）**：现名 `format!("lto-goal-{}", options.runner)`（dispatch_goal.rs:172-175）只带 runner。实测 host 会话里同时挂 3 个 `lto-goal-codex` + 2 个 `lto-goal-agy`，分别在 animem-private / lto-release / yihub 三个不同项目跑不同 goal，**必须 capture-pane 逐个抓屏才能分辨谁是谁**，tmux 自动加的 `-` 后缀（`lto-goal-codex-`）毫无信息量。
  - 新命名格式：`lto:<runner>:<goal-slug>`，例：`lto:codex:invocation-ux`。
  - `goal-slug` 取自 goal 文件名 basename 去扩展名，剥掉常见前缀（`goal-` 及紧随的 `YYYY-MM-DD-` 日期段），非 `[a-z0-9-]` 字符替换为 `-`，转小写，**截断到 20 字符**（tmux status bar 宽度有限；总长控制在 ~32 字符内）。`goal-2026-07-10-invocation-ux.md` → `invocation-ux`。
  - slug 为空（如 goal 文件名就叫 `goal.md`）时退化为 run-id 末 8 位，绝不产生 `lto:codex:` 这种空尾巴。
  - `--window-name` 显式传入时完全尊重，不做任何加工（现有行为不变）。
  - 用纯函数实现（如 `fn goal_window_name(runner: &str, goal_path: &Path, run_id: &str) -> String`）便于单测；同一 (runner, goal) 稳定可复现。
- `new_window_in_session`（tmux_runner.rs:428-432）：`-F` 增加 `#{window_id}`（@N 不可变），canonical 清理句柄用 window_id 存入 run state；send-keys/capture 现有 target 格式若依赖 index，评估切到 window_id 形式（tmux target 支持 `@N.%pane`），至少清理路径必须用 @id。
  - 注意：窗口名只是给人看的显示层，**一切程序寻址仍用 window_id**（名字可能被 tmux 去重加后缀，也可能被人手动改）。别把新窗口名当句柄。
- 完成清理：`cmd_agent_turn_completed` 收尾加第四跳——该 run state 有 dispatch window_id 且状态成功 ⇒ `tmux kill-window -t @N`（默认 on-success；`--keep-window` 或 cleanup 配置可关；失败/timeout 保留并 stderr 提示"窗口保留供排障"）。清理动作 emit 事件（复用 events 体系，类型如 `runner.window.cleaned`，加进 KNOWN_EVENT_TYPES）。
- ready 超时默认：`dispatch_goal.rs:165` `unwrap_or(20)` → `unwrap_or(60)`（flag `--ready-timeout` 已存在，只动默认值；tmux_runner.rs:14 的 30 常量保持）。
- `--target` 显式传入时（tmux_runner.rs:410-412 直接返回处）：校验目标 pane 前台命令是 shell（bash/zsh/fish），否则 bail "target pane busy"，杜绝注入到被占用窗口。

**测试**：①`goal_window_name` 纯函数单测：正常 goal 文件名出 `lto:codex:invocation-ux`、超长名截断到 20 字符、`goal.md` 退化到 run-id 末 8 位、特殊字符被规范化、`--window-name` 显式传入原样透传 ②window_id 锚定在 index 漂移场景下清理仍准（测试里先开两个窗口、kill 前一个制造漂移）③on-success 清理 / 失败保留 / --keep-window 三态 ④--target 指向忙 pane 被拒。tmux 相关测试注意环境自适应（tmux 内验 attached 分支、`env -u TMUX` 验 detached fallback——此坑 2026-06-20 踩过）。

**完成判据**：新测试全绿 + 真机冒烟一次（派最小 goal 到 tmux，`tmux list-windows` 里能一眼认出是哪个 goal，完成后窗口自动消失、events.jsonl 有 window.cleaned）。独立 commit。

## 执行顺序 + 每 Phase 收口

顺序 1→2→3（互不依赖但 2 的端到端测试受益于 1 的环境干净）。每 Phase 收口动作：
1. `cargo fmt --check && cargo check && cargo clippy -- -D warnings && cargo test`
2. `lto audit --auto-dispatch`（快 runner 优先；untracked 中间态不当 blocker）
3. `lto check` 干净后独立 commit（commit message 你写，不加 AI 署名/Co-Authored-By）

全部完成后 `lto closeout`，把 run handoff 留好，**停**。release/tag 归 host。

## 提醒

- 复用勿重写：ready_patterns（dispatch_goal.rs:229/265/312）、run_notify_cmd（agent_turn.rs:103-118）、notify::wake_run（notify.rs:145-155）、agy 空转探测（agy.sh:47-51）、audit readonly 注入（audit_dispatch.rs:161-181）。
- 不可自动化的安全阀：host 亲验是硬停止点；本 goal 完成 ≠ 上线，host 会真机复跑端到端再定 release。
- 所有 file:line 基于 HEAD d4c8e1a，若你落地时代码有偏移，以行为特征定位为准。
