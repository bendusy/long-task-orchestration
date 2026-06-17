# Goal: 标准化 tmux 派 goal 原语 —— LTO 把「给 codex/pi/agy 派 goal 文档」封装成一条命令

> 致 codex:沿用既有约束(LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写、release/tag 归 host)。
> **这份做 tmux 派 goal 原语(把主 agent 手搓 send-keys 的派工标准化）。做完停,别顺手改别的 backlog 项。**
> 这是 `src/tmux_runner.rs` 的自然扩展（已有 signal/sentinel/fire 模式 + send-keys preflight），不是从零造。

---

## 为什么做（第一性）

现在主 agent 给外部 coding agent 派 goal 文档,是**手搓 tmux send-keys**:new-window → 起 CLI → 探针确认 TUI 接管 → 清 → 发 prompt → 确认进框 → Enter → capture 看开跑。每次都踩同样的坑(探针漏了把中文打进 shell、带 `/` 的路径被 TUI 吃掉、prompt 以 `/` 开头被当斜杠命令)。这套该被 LTO 封装成一条命令,主 agent 只说「派这个 goal 给 codex」,不再手搓终端操作。

**目标**:`lto dispatch-goal --runner <codex|pi|agy> --goal <file> [--target <pane>]` 一条命令完成派发,内部处理三家入口差异 + TUI 派工坑。

---

## ⚠️ 必读:host 已亲验的三家入口事实（pi 0.79.3 / codex 0.140.0 / agy 1.0.8）

全部 host 实测跑通（三家读同一 goal 文件、答对 Phase 数=5）。**三家 goal 入口不通约**:

| runner | 启动 | goal 入口 | 实测 | 坑 |
|---|---|---|---|---|
| **codex** | `codex`（TUI） | **`/goal <文件路径>`** → 进 goal-runtime（底部 `Pursuing goal` 阶段追踪，最正规） | ✅ | 路径 `/` 被 TUI 吃 → 必 `send-keys -l` literal |
| **pi** | `pi`（TUI） | **直发 prompt**「Read 文件 + 执行」（无 `/goal`，它的 22 个 `/` 命令里没有） | ✅ | prompt 别以 `/` 开头（当斜杠命令）；路径 `/` → literal |
| **agy** | — | **`agy -i '<prompt>'`** goal prompt 作命令行参数（无 `/goal`） | ✅ | 无 TUI 输入坑（prompt 是 shell 参数，不进 TUI 框） |

**通用 TUI 派工坑（codex/pi 适用，agy 因走 shell 参数免疫）**:
1. **发 prompt 前必探针确认 TUI 接管**：发短串（如 `LTO_PROBE`，不带 Enter），capture 确认它进了 TUI 输入框（`›` 开头 + 底部状态行），不是落在 shell 提示符（`➜ dir git:(branch)`）。确认后 `Ctrl-U` 清掉再发真 prompt。**漏这步 = 整段 prompt 打进 shell**。
2. **带 `/` 的路径/内容必 `tmux send-keys -l`（literal 模式）**：否则 `/` 被 TUI 当命令字符，`references/specs/x.md` → `referencesspecsx.md`。
3. **pi 的 prompt 别以 `/` 开头**：会被当斜杠命令。goal prompt 用「Read the file <path> and ...」开头。
4. **启动后等加载**：tmux 新窗起 CLI 有升级提示/组件加载延时，等足够久（codex ~10s）再探针；遇升级/加载提示选跳过求稳。
5. **启动失败 fallback**：CLI 没起来 send-keys 会全打进 shell 且 cwd 可能漂回 `~`。探针发现没接管 → `Ctrl-C` 清 → `tmux display-message -p '#{pane_current_path}'` 核 cwd → 必要时 cd 回 repo 重启。

---

## 核心架构裁决（host 先定）

**裁决 1:派发载体生命周期 = 派出即返回（fire-and-forget），不阻塞等完成。**
- goal 是长任务（codex 跑几十分钟到几小时）。`dispatch-goal` 的职责是**把 goal 正确送进目标 agent 并确认它开跑**，不是等它跑完。
- 返回:派发成功（agent 已 `Pursuing goal`/`Working`）+ 目标 pane id + 起的 session/window 信息，写进 `.lto/<run>/` 便于后续观测/回收。
- 「完成通知」是**另一件事**（见下方「与完成通知的关系」），不在这份 goal 的核心——这份只解决「正确派出去」。

**裁决 2:复用 `tmux_runner.rs`，加一个 dispatch-goal 路径，不新写 tmux 操作层。**
- host 已亲验 `tmux_runner.rs` 有 send-keys preflight + capture-pane + target 解析 + ready-pattern 匹配。dispatch-goal 复用这些。
- 新增的是「按 runner 的 goal 入口编排」:codex 发 `/goal <path>`（literal）、pi 发 lean prompt、agy 用 `-i`。

**裁决 3:命令面 = `lto runner` 的子模式 还是 新 `lto dispatch-goal`？host 倾向新顶层命令但你评估。**
- 倾向 `lto dispatch-goal`（语义清晰，主 agent 一眼懂）。
- 但若发现它和现有 `lto runner --tmux-mode` 高度重叠（runner 已经能 send prompt 到 tmux），可作 `runner` 的一个模式（如 `--tmux-mode goal`）。codex 评估:dispatch-goal 与 runner tmux 模式的真实差异是「goal 入口编排 + 探针确认 + 不阻塞等完成」——若这些值得独立命令就独立，否则并进 runner。**给出评估结论 + 理由**，别两可。

---

## Phase 1:dispatch-goal 核心（三家入口编排 + TUI 坑封装）

### 1.1 命令 + 参数
- `lto dispatch-goal --runner <codex|pi|agy> --goal <file> [--target <pane>] [--new-window] [--window-name <n>] [--cwd <repo>]`。
- `--target` 给现有 pane；不给则 `--new-window` 在指定 cwd 新开窗（默认 cwd = goal 文件所在 repo）。
- 校验:goal 文件存在、runner 是三家之一、target/new-window 二选一。

### 1.2 按 runner 编排入口（核心）
- **codex**：启动 `codex` → 等 ready（ready-pattern 匹配 codex TUI 标志，如 `gpt-` 状态行）→ 探针确认 → `send-keys -l "/goal <goal_path>"` → Enter → 确认 `Pursuing goal`/`Working`。
- **pi**：启动 `pi`（带 lean flags `--no-skills --no-context-files --no-extensions` 省 context）→ 等 ready → 探针 → `send-keys -l "Read the file <goal_path> and execute it. <约束摘要>"`（不以 `/` 开头）→ Enter → 确认开跑。
- **agy**：`send-keys -l "agy -i 'Read the file <goal_path> and execute it. <约束摘要>'"` → Enter（goal prompt 作 shell 参数，无 TUI 探针需求）→ 确认 agy 启动 + 开跑。
- **约束摘要**：派 goal 时附带的通用约束（LTO 自管/每 Phase 异构审/dogfooding/红线/commit 你写 release 归 host）——抽成一个常量，三家共用，goal 文件路径 + 这段约束一起送。

### 1.3 探针 + literal + ready 封装（复用 tmux_runner）
- 探针:send `LTO_PROBE_<rand>` 不 Enter → capture 确认它出现在 TUI 输入框 → Ctrl-U 清 → 才发真 prompt。封装成一个 `confirm_tui_ready` 步骤。
- literal:所有含 `/` 的内容用 `send-keys -l`。
- ready 等待:沿用 tmux_runner 的 ready-pattern + timeout，等 CLI TUI 起来再操作。
- 升级/加载提示:ready-pattern 匹配到 TUI 就绪即可（升级提示不阻塞 TUI，但探针会确认真接管）。

### 1.4 产出 + 返回
- 派发成功写 `.lto/<run>/dispatch/<runner>-<ts>.json`:runner、goal 文件、target pane、起的 window/session、派发时间、确认开跑的证据（capture 片段）。
- 返回 AgentResult 风格:status=dispatched、target pane、不含 reply（因为不等完成）。
- **判据**:`lto dispatch-goal --runner codex --goal <某 goal 文件> --new-window` 真在新 tmux 窗起 codex + 发 `/goal` + 确认 `Pursuing goal`。三家各跑一次 dogfood（codex/pi/agy 都把一个测试 goal 文件派出去并确认开跑）。**这是核心交付,必须三家实跑通。**

---

## Phase 2:完成通知子系统（host 已逐层实测 + 21 条边界一次设计全，不挤牙膏）

dispatch-goal 派出去后「agent 跑完了没」由这个子系统答。**host 已实测每条机制 + codex 真实 payload，把边界一次设计全**，codex 一次实现完整，别想到什么补什么。

### 机制选择（host 实测，按结构化程度）

**关键修正（host 在 config 里实测发现）**：codex 不止有简单 `notify`，更有**完整的 Claude-Code 风格 `~/.codex/hooks.json`**（`SessionEnd`/`Stop`/`PostToolUse`/`UserPromptSubmit`，带 matcher/timeout/trusted_hash）。**feishu_hub、roostery 已在用 codex `Stop` hook 做完成通知**（`~/.feishu_hub/bin/agent-stop-notify.sh` 是成熟范例）。**用 codex `Stop` hook，不用 `notify`**（更标准、有生态、payload 结构化）。

| 场景 | 机制 | host 实测 |
|---|---|---|
| 进程退出型（`codex exec`/`pi -p`/runner.sh） | `new-window 'agent命令'`（agent 作 pane 主进程）+ `remain-on-exit on` + 全局 `pane-died` hook | ✅ 捕获 `pane_dead_status`（带 rc）。**取代脆弱的命令尾拼 `printf '\a'`/`wait-for`** |
| 交互 TUI 跑完 turn（codex `/goal`，进程不退） | **codex `Stop` hook**（payload stdin JSON 有 `cwd`/`session_id`/`transcript_path`/`prompt_response`） | ✅ feishu_hub/roostery 已用；payload 字段实测确认 |
| pi 交互 | pi SDK stop hook（issue #1884 / 社区 pi-notify-agent） | 待 LTO 侧实测 |
| agy / 无 hook | sentinel 文件 + monitor-silence 兜底 | LTO 已有 Sentinel |
| 人听觉提醒 | terminal bell（仅提醒，三家不主动发、hook 进程无 tty 发不回） | ✅ 不作完成判定 |

### 三个 host 拍板的设计裁决

**裁决 A（run 路由 + 完成语义，host：调查实际场景定）**：实测 codex `Stop` hook payload 有 **`cwd` + `session_id`**。
- **多会话路由靠 `cwd`**（payload 的 cwd = agent 工作目录 → 映射到对应 run 的 `.lto/<run>/`）；同 cwd 多 run 罕见，再用 `session_id` 二级区分。
- **完成语义：LTO 只如实报「turn 完成」，不替 host 判「整个 goal 完成」**。codex `Stop` hook 每个 turn 停都触发——LTO 把每次 turn 完成写进 `.lto/<run>/dispatch/<runner>-<session>.turns.jsonl`（追加，含 ts/session/summary）。**「整个 goal 完成没」由 host 判**（看 codex 是否退出 `Pursuing goal` / state.tasks 全 done / 人亲验），LTO 不猜。符合「LTO 不替 host 决策」铁律。

**裁决 B（A3 notify 冲突，host：LTO 内部仲裁）**：用户 `hooks.json` 已有自己的 `Stop` hook（如 feishu_hub/roostery，实测真有）→ **LTO 不覆盖、不简单 append，做内部仲裁**：
- LTO 把自己的 stop hook **追加进 `Stop` 数组**（codex hooks.json 的 Stop 是数组，多个并存都会被调），用注释/独立文件标记 LTO 段便于卸载只删自己的。
- 这样用户原 hook（feishu 通知等）+ LTO hook（写 turn 文件 + tmux 信号）**都生效**，互不干扰。这就是「LTO 内部仲裁」：接管自己那一份，不动别人的。

**裁决 C（A7/B2 解析健壮性，host：纯 bash + schema 容错）**：LTO 的 stop hook 脚本：
- **纯 bash 解析**（不依赖 jq；jq 在则可选加速）——`cwd`/`session_id` 用 bash 参数/grep 提取，jq 缺失也能跑。
- **schema 容错**：codex 升级改 payload schema → 认不出已知字段就**降级写 raw payload + 发通用完成信号**，不假设固定 schema、不崩。
- **fail-safe**：脚本任何错都 `exit 0`，绝不拖垮 codex（B7）。

### 21 条边界（codex 实现时全处理，分组）

**A 安装**：A1 无 `~/.codex/`（codex 没装）→ 跳过 hook 装，直接降级层兜底，不创建。A2 `config.toml`/`hooks.json` 不存在 → 创建最小合法文件。A3 已有用户 Stop hook → 仲裁追加（裁决 B）。A4 hooks.json 非法 JSON → 不写坏，报警 + 降级。A5 已装 LTO hook（重复 install）→ 幂等跳过（按标记识别）。A6 `~/.codex/hooks/` 无写权限 → 降级。A7 无 jq → 纯 bash（裁决 C）。
**B 运行时**：B1 payload 非 turn 完成事件 → 忽略。B2 schema 变 → 容错写 raw（裁决 C）。B3 LTO env（run 路由用）没传（用户手动跑非 LTO 派工）→ 静默 exit 0。B4 tmux wait-for channel 不存在/tmux 没跑 → 不报错。B5 done 目录不存在（run 已 closeout）→ 不报错。B6 多会话 → cwd/session_id 路由（裁决 A）。B7 脚本出错 → always exit 0。
**C 检测/降级**：C1 hook 装了但会话是装之前起的 → 降级（检测不到 turn 文件更新就退轮询）。C2 没装 hook → 降级层兜底。C3 交互 TUI 不退出 → 靠 hook/sentinel 非 pane-died。C4 agent 崩溃（非正常完成）→ pane-died rc≠0 / 超时无 turn 文件 = 区分「死了」vs「完成」。C5 多 turn → 每 turn 写 turns.jsonl，goal 完成归 host 判（裁决 A）。
**D 卸载**：D1 卸载只删 LTO 标记段，不动用户 hook。D2 备份丢了 → 靠 LTO 标记注释定位删除。

### 安装机制（裁决：LTO 自动装 + 幂等 + 可回滚）

- repo 内版本化模板 `scripts/hooks/codex-stop-notify.sh`（纯 bash，裁决 C）。
- `install.sh`（或 dispatch-goal 首次派 codex）自动装到 `~/.codex/hooks/` + 仲裁追加 `Stop` hook 到 `hooks.json`（裁决 B）+ 备份原文件。
- `lto dispatch-goal --uninstall-hooks` 还原（删 LTO 标记段 + 删脚本）。
- **新用户开箱即用**：没装 hook → 自动降级层兜底（pane-died/sentinel），不阻塞不报错；想要精确再装。

### 完成判据
- ① fresh 环境 install → stop hook 在位 + hooks.json 仲裁追加（用户原 hook 仍在）+ 备份在。
- ② 派 codex 测试 goal，turn 完成 → `.lto/<run>/dispatch/*.turns.jsonl` 真追加（host 已验 codex Stop hook 真触发 + payload 有 cwd/session_id）。
- ③ 卸载真还原（用户原 hook 不动）。
- ④ 模拟无 jq / payload schema 变 / env 没传 / 用户已有 hook → 各边界不崩不误伤（裁决 C + 21 条）。
- ⑤ dispatch json 含完成信号方式字段；多会话按 cwd 路由不串。


---

## 执行顺序 + 收口

1. Phase 1 实现 + 三家 dogfood 实跑通（核心派发）。**先收口 commit**——可独立交付。
2. Phase 2 完成通知子系统（codex Stop hook + pane-died + 21 边界 + 自动装 + 仲裁 + 降级）。**独立 commit，比 Phase 1 重，建议拆子批**（hook 模板 / 安装+仲裁 / 降级各自小批，防长 thread）。
3. 每批收口:`cargo fmt/clippy -D warnings/test --locked` 全绿 → `lto audit --auto-dispatch --discover-risks` 异构审本批 diff → `lto check` → commit。
4. 文档:`workflow-playbook.md` 加「dispatch-goal 派 goal + 完成通知」一节;README/INSTALL 写清 hook 自动装做了什么/如何回滚/不装也能用;CHANGELOG 一笔。
5. backlog ⑩/⑪ 关联更新（dispatch-goal 是 tmux-goal-loop 的命令化落地）。

## 提醒（安全阀）
- **复用 tmux_runner.rs**,别重写 tmux 操作层（裁决 2）。
- **三家入口按实测表编排**（必读),codex 用 `/goal`、pi 直发、agy 用 `-i`,别统一成一种（会错）。
- **探针确认 + literal 路径是硬要求**（坑 1/2,漏了派工会失败/打进 shell）。
- **派发不阻塞等 goal 完成**（裁决 1）;但完成通知信号在派发时挂好（Phase 2），不是留空接口。
- **完成通知用 codex Stop hook 不用 notify**（host 实测 codex 有完整 hooks.json，feishu_hub/roostery 已用）;改用户全局文件必须仲裁追加+备份+可回滚（裁决 B）。
- **LTO 只报 turn 完成，不替 host 判 goal 完成**（裁决 A）。
- dogfood:dispatch-goal 自己派测试 goal 给三家实跑通 + 完成信号真触发才算完。
- host 亲验是硬停止点;commit 你写,release/tag 归 host。
