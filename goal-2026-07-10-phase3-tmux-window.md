# Goal: Phase 3 — tmux 窗口可辨识 + ID 锚定 + 完成自动清理 + ready 超时

> 致 codex：沿用约束（LTO 自管、红线不弱化、commit 你写、release 归 host）。
> **这份只做 Phase 3，做完就停。** Phase 1（headless 写权限闸门，commit `4b7e415`）和 Phase 2（完成通知闭环，commit `92f7618`）**已由前一轮完成并经 host 亲验**，别重做、别回改。
> 当前 HEAD 应为 `92f7618`，工作树干净（`goal-*.md` 和 `.codestable/` 是预期 untracked，不当 blocker，也别提交它们）。

## 为什么做（host 真机实证，不是理论）

host 会话里一度同时挂着 6 个派工窗口，其中 **3 个都叫 `lto-goal-codex`**、2 个叫 `lto-goal-agy`，分别在 `animem-private` / `lto-release` / `yihub` 三个不同项目跑三个不同 goal。tmux 对重名窗口只会加一个 `-` 后缀（`lto-goal-codex-`），零信息量——**host 必须逐个 `capture-pane` 抓屏才能认出哪个是哪个**。

同一现场还暴露：窗口从不回收（跑完的、失败的、等确认的全堆着），窗口 index 因此漂移（我派的窗口从 `cc:9` 漂到 `cc:8`），首次派工还因 codex 的 hook trust 提示卡满 20s ready 超时而失败（trust 通过后重派即成功）。

## ⚠️ 必读：前提与坑

- **不信文档声称，以 grep 源码为准**。本 goal 的所有 file:line 基于 `d4c8e1a`，Phase 1/2 已改动 `cli.rs`/`dispatch_goal.rs`/`state.rs`/`agent_turn.rs`，行号会偏移——**以行为特征定位，别硬认行号**。
- **窗口名只是给人看的显示层**。tmux 会给重名窗口加后缀，人也可能手动改名。**一切程序寻址必须用 `window_id`（`@N`，不可变）**，绝不能把新窗口名当句柄。这是本 Phase 最容易做歪的地方。
- 进行中的 untracked 文件是预期状态，审计报 untracked 为 CRITICAL 时记录但不当 blocker。
- 异构审计快 runner 优先（codex/agy），pi 留补充不做关键路径阻塞。

## 核心架构裁决（host 已定，别猜）

1. **清理只清自己的**：`kill-window` 仅限本 run state 里记录的、由 dispatch-goal 亲手创建的 window_id。绝不遍历会话按名字匹配去杀——这是红线，误杀用户窗口不可接受。
2. **成功才清，失败留现场**：默认 on-success 清理；失败/timeout 保留窗口并在 stderr 提示"窗口保留供排障"。
3. **显示层与寻址层分离**：名字给人，`@id` 给程序。
4. **不弱化 controller-in-chief 不变量**（CLAUDE.md 第 1 条）：本 Phase 全是机械步骤自动化，不新增任何 LTO 自动决策。

## 要求

### R1：可辨识窗口名（优先做，痛点最直接）

现状：`format!("lto-goal-{}", options.runner)`（`dispatch_goal.rs` 里 `TmuxRunnerConfig.window_name` 的 `unwrap_or_else`，原 :172-175）。

- 新格式 `lto:<runner>:<goal-slug>`，例 `lto:codex:invocation-ux`。
- `goal-slug` 由 goal 文件 basename 推导：去扩展名 → 剥掉 `goal-` 前缀 → 剥掉紧随的 `YYYY-MM-DD-` 日期段 → 非 `[a-z0-9-]` 替换为 `-` → 折叠连续 `-` → 去首尾 `-` → 转小写 → **截断到 20 字符**（截断后仍去尾部 `-`）。
  - `goal-2026-07-10-invocation-ux.md` → `invocation-ux`
  - `goal-2026-07-10-phase3-tmux-window.md` → `phase3-tmux-window`
- slug 为空（如文件就叫 `goal.md`）→ 退化为 run-id 末 8 位。**绝不产生 `lto:codex:` 这种空尾巴。**
- `--window-name` 显式传入时完全尊重、不加工（现有行为不变）。
- 实现为纯函数 `fn goal_window_name(runner: &str, goal_path: &Path, run_id: &str) -> String`，同一 (runner, goal) 稳定可复现。

### R2：window_id 锚定

- `new_window_in_session`（`tmux_runner.rs` 原 :428-432，`tmux new-window -P -F '#{session_name}:#{window_index}.#{pane_index}'`）：`-F` 增加 `#{window_id}`，把 `@N` 作为 canonical 清理句柄存入 run state。
- send-keys/capture 现有 target 若依赖 index，评估切到 `@N.%pane` 形式（tmux target 支持）。**至少清理路径必须用 `@id`**。

### R3：完成后自动清理

> **⚠️ host 亲验推翻了本条的原始前提，先读完再动手。**
>
> `agent.turn.completed` 绑在 codex 的 **`Stop` hook** 上（见 `~/.codex/hooks.json`），而 codex 的 `Stop` = **每个 turn 结束**（agent 说完一轮话、把控制权交回），**不是 goal 完成**。
>
> 本 run 的 `events.jsonl` 时间线是铁证：
> ```
> 09:55:11  runner.started        ← goal 注入
> 09:55:36  agent.turn.completed  ← 25 秒后！agent 刚说完开场白
> 09:58:49  phase.changed         ← 此时才真正开始干活
> 10:01:00  phase.changed         ← 还在跑
> ```
> **完成事件在任务真正开始之前就 fire 了。** 因此：
> - 任何 `dispatch-and-wait` / `events --wait` 的使用者都会在几十秒后收到**假完成**；
> - R3 若直接挂在这个事件上 kill-window，**窗口会在 agent 刚开口时被杀掉**。
>
> 这不是 Phase 3 引入的问题，是 Phase 2（`92f7618`）与更早设计遗留的语义错配。SKILL.md 里"派完必须挂 waiter"的说法同样受影响。

**R3 因此先做正名，再做清理**：

1. **先确认事实**：读 `.lto/20260710-095237-lto-phase3-tmux-id-ready-f8378d12/events.jsonl` 的时间线与 `~/.codex/hooks.json` 的 `Stop` 注册，自己复核上述结论。若你的证据与 host 不一致，**停下报告**，不要按错误前提写代码。
2. **区分 turn 与 goal**：三选一，你评估后选并说明理由——
   - (a) 找到 codex 真正的 session-end 触发点（若存在），把 goal 完成挂上去；
   - (b) 保留 `agent.turn.completed` 的真实语义（它就是 turn 级），**另立** goal 完成信号（如 sentinel 文件 / 显式 `lto collect-agent-run` / dispatch-and-wait 自己判定），事件里带可区分字段；
   - (c) 若 (a)(b) 都不可靠，则 R3 的清理**只在显式 `dispatch-and-wait` 且拿到真实 rc 时**执行，普通 `dispatch-goal` 不自动清理（宁可不清，不可错杀）。
3. **rc 必须真实**：你已把 `scripts/hooks/codex-stop-notify.sh` 改成 `--rc 0`（硬编码成功）。Stop hook 拿不到 agent 真实退出码，硬写 0 = **失败也报成功**，会让"成功才清理 / 失败保留现场"三态永远只走成功分支。要么不传 `--rc`，要么从 payload 里取真实状态。
4. **清理实现**（前提 2/3 落定后）：run state 有 dispatch window_id 且**确认成功** ⇒ `tmux kill-window -t @N`。三态可控：默认 on-success；`--keep-window` 关闭；失败/timeout 保留 + stderr 提示"窗口保留供排障"。
5. 清理动作 emit 事件（复用 events 体系，类型如 `runner.window.cleaned`，加进 `KNOWN_EVENT_TYPES`）。

### R3b：全局 hook 不得固化具体 repo 路径

`~/.codex/hooks.json` 的 Stop hook 命令里写死了 `LTO_REPO_FALLBACK='<另一项目的绝对路径>'`——上一轮 yihub 派工留下的。它是**全局** hook，跨项目派工会串台（本次只是恰好被 `LTO_RUN_ID` env 覆盖才没出错）。

检查 `install_codex_hook`（`dispatch_goal.rs`）：fallback 不该把某个具体 repo 的绝对路径固化进全局 hooks.json。改为运行时从 env / cwd 推导，或每次 dispatch 时覆写为当前 repo。加测试覆盖"连续给两个不同 repo 派工，hook 不残留前一个 repo 路径"。

### R4：ready 超时默认 20s → 60s，且识别"交互阻塞"并立刻失败

- `dispatch_goal.rs` 里 `ready_timeout_sec.unwrap_or(20)` → `unwrap_or(60)`。`--ready-timeout` flag 已存在，只动默认值；`tmux_runner.rs` 的 `DEFAULT_READY_TIMEOUT_SEC = 30` 常量保持不动。
- 理由：pi/glm 冷启常超 20s。

**⚠️ host 现场修正（派本 goal 时实测，别照抄旧假设）**：codex 首次在某目录跑、或 LTO 的 hook 脚本内容变更后，会弹 **"Hooks need review / Trust all and continue"** 交互提示。这**不是慢，是在等人按键**——`--ready-timeout 60` 同样跑满超时后失败。加长超时永远治不了它。

因此 R4 还要求：`wait_until_ready` 的抓屏轮询里增加一组 **blocked_patterns**（与现有 ready_patterns 并列），命中即**立刻 bail** 并给出可操作的错误信息，不要空等到超时。至少覆盖：

- codex：`Hooks need review`、`Trust all and continue`
- 通用：`Press enter to confirm`

错误信息要告诉 host 怎么办，例如：`runner codex is blocked on an interactive trust prompt in <target>; resolve it in tmux (select "Trust all and continue"), then re-dispatch with --target <target>`。

这条把"卡满超时才失败"变成"数秒内可感知失败 + 明确指引"，与 Phase 1 把 agy 空转从"假成功"变"可感知失败"是同一形状。

### R5：`--target` 忙 pane 拒绝

- `prepare_target` 里显式 `config.target` 直接返回的分支（`tmux_runner.rs` 原 :410-412）：校验目标 pane 的 `#{pane_current_command}` 是 shell（bash/zsh/fish/sh），否则 bail `"target pane busy"`。杜绝把 prompt 注入到已被别的 agent 占用的窗口。

## 测试

1. `goal_window_name` 纯函数单测：正常文件名 → `lto:codex:invocation-ux`；超长名截断到 20 字符且不留尾部 `-`；`goal.md` → run-id 末 8 位；特殊字符规范化；`--window-name` 显式传入原样透传。
2. window_id 锚定在 index 漂移下仍准：测试里先开两个窗口、kill 掉前一个制造 index 漂移，验证清理仍打中正确窗口。
3. 清理三态：on-success 清理 / 失败保留 / `--keep-window` 保留。
4. `--target` 指向忙 pane 被拒。

tmux 测试注意**环境自适应**：测试读全局 `$TMUX_PANE`，`set_var` 会污染并行测试。在 tmux 内验 attached 分支，`env -u TMUX` 验 detached fallback，双路径都要覆盖（此坑 2026-06-20 踩过）。

## 完成判据

- `cargo fmt --all --check && cargo check --locked --all-targets && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets` 全绿（基线 268 测试，只增不减）。
- `python3 scripts/check_docs_consistency.py` 和 `python3 scripts/check_python_rust_ownership.py` 通过。
- 真机冒烟一次：派一个最小 goal 到 tmux，`tmux list-windows` 里**能一眼认出是哪个 goal**；完成后窗口自动消失；`events.jsonl` 里有 `runner.window.cleaned`。
- `lto audit --auto-dispatch` 收敛、`lto check` 干净后**独立 commit**（commit message 你写，不加 AI 署名 / Co-Authored-By / Generated 标记）。

做完 `lto closeout` 留好 handoff，**停**。release/tag 归 host。

## 提醒

- 复用勿重写：ready_patterns（codex=`"gpt-"` / pi=`"deepseek"`,`"ctx"` / agy=`"? for shortcuts"`）、`notify::wake_run`、events emit API、Phase 2 刚加的 `persist_notify_cmd` 形状（存 run state 的模式可照抄给 window_id）。
- 不可自动化的安全阀：host 亲验是硬停止点。本 goal 完成 ≠ 上线。
