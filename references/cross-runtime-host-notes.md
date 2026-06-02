# 各家 CLI 当宿主的专项注意（三家自评提炼）

> 2026-05-31，让 codex/pi/agy 各自审视本 skill 中「对它这家 CLI 的描述」后提炼。每家最懂自己的坑——这些是单靠主 agent（只深测了 codex 一条路）发现不了的。主 agent 已逐条核验，标注采纳/存疑。

## 一、最该记的元发现：沙箱不是通用铁律（三家共识）

早期 sharing-guide 把「宿主派工必须放开沙箱」写成所有宿主的硬前提。**codex/pi/agy 三家自评一致反驳，且实测坐实这是错的**：

- **codex**：`exec`/TUI 默认沙箱挡子 runner 写文件 → 必须 `--dangerously-bypass-approvals-and-sandbox`。但 codex 自评指出更优解是给子 runner 专用可写 roots/HOME，最小放权，而非全盘 bypass。
- **pi**：TUI 派工无需放开沙箱（`Agent` 是内部机制不受外层沙箱限）。
- **agy**：无需放开沙箱（细粒度权限弹窗授权，强行套 codex 的 bypass 是安全降级）。

**教训**：只深测一条路（codex）就把它的坑写成通用铁律，是过度泛化。让每家评审自己，才暴露出来。已改 sharing-guide 坑1 为各家差异矩阵。

## 二、codex 当宿主专项（codex 自评，已核验采纳）

1. **权限边界是「启动时冻结」的，不是运行中能力**。子进程/tmux 在 codex 不是默认能力——要 sandbox+approval+network+MCP 启动时全允许才成立。派工命令能启动但子 runner 写 `~/.pi`/`~/.gemini`/锁/token cache/联网全被截断，且 `approval_policy=never`/headless `codex exec` 下基本不能中途补救。
2. **Codex Host Preflight**（采纳为建议，非强制闸门）：codex 当宿主前记录 宿主模式(interactive/exec) / sandbox / approval policy / child 写路径 / network / MCP env 可见性。任一失败 → 降级为「本机自审/活着的 runner 子集」，不硬宣称 triad 可用。
3. **`codex exec` 子任务契约**：子 runner 必须禁止反问用户、一次性输出到 stdout/约定文件、不依赖中途审批、失败返回非零退出码（空输出和 timeout 分开记）、prompt 写明「默认只审计写 report，不改仓库除非明确授权」。
4. **MCP 不从宿主会话继承**：codex child runner 用 MCP 前要在同启动方式下跑 `codex mcp list` 或等价 smoke——别因为宿主会话有 MCP tool 就假设 child `codex exec` 也有。实测旁证：codex TUI 启动时 memory-flow MCP 就 `failed`。
5. **resume/fork 恢复协议**：codex 长任务恢复不信 transcript 旧状态。建议每轮写 `run-state.md`（run id / git SHA / sandbox profile / tmux id / 每 runner 的 command/pid/timeout/exit/stdout 路径 / MCP smoke / 已采纳否决 blocker）。resume 后从文件+tmux+进程重建状态，不信「1 background terminal running」之类旧叙述。

## 三、pi 当宿主专项（pi 自评，已核验，部分存疑标注）

### pi 调用速查 sample（来自 pi-docs-playbook 源码 + agent-delegate 实测）

**pi 当宿主跑 LTO 主循环**：
```bash
# pi 交互式启动，自动加载 ~/.pi/agent/skills/ 下所有 skill
pi
pi "帮我审核这份 chengpi spec"     # 带初始 prompt 启动

# 指定模型（DeepSeek V4 Pro thinking，LTO 默认用这个）
pi --provider deepseek --model deepseek-v4-pro

# 只加 LTO 相关 skill（减少干扰）
pi --skill ~/.pi/agent/skills/long-task-orchestration

# 会话管理
pi -c                              # 续最近会话（/compact 后恢复）
pi --name "lto-chengpi-audit"      # 命名会话，便于回溯
pi --fork <session-id>             # fork 会话到新文件
```

**pi 当审计方（被 agent-delegate 派工，headless 模式）**：
```bash
# agent-delegate runners/pi.sh 实际命令（最简化）：
pi -p --provider deepseek --model deepseek-v4-pro "$(cat prompt.txt)" > reply.txt

# 关键参数：
#   -p          : 非交互式，一次性输出到 stdout
#   --provider  : deepseek（本机 DEEPSEEK_API_KEY 已配）
#   --model     : deepseek-v4-pro（thinking 模式，审 16KB ~170-200s）
# timeout≥240s : exit=124 是超时不是空返回

# pi 是最干净的 headless runner：无 banner、无审批弹窗、无 MCP 加载噪声
# 脚本走 PATH 命中 ~/.local/bin/pi（shell function 包裹真二进制）
```

**pi Agent 工具派工（LTO 内异构审计）**：
```
# pi 宿主起异构审计方（不派自己，派 codex + agy）：

# 审计方 1：codex (OpenAI)
Agent(
  subagent_type="general-purpose",
  model="gpt-5.5",
  prompt="你是审计方。逐条审 LTO spec 的 premature 假设/数据探针阈值/部署安全网。先给最强反驳，禁止迎合。输出 blocker register（逐条附置信度 HIGH/MODERATE/LOW）。",
  run_in_background=true,
  isolation="worktree"
)

# 审计方 2：agy (Gemini) 同理
Agent(
  subagent_type="general-purpose",
  model="gemini-2.5-pro",
  prompt="同上审计任务。独立审，不参考其他审计方结论。",
  run_in_background=true,
  isolation="worktree"
)

# pi 的三个内置 agent type：
#   general-purpose : 全工具（含 WebSearch/WebFetch）
#   Explore         : 只读（read/grep/find/ls，无 Edit/Write）
#   Plan            : 只读，架构设计

# Agent 工具关键参数：
#   subagent_type    : "general-purpose" | "Explore" | "Plan" | 自定义 agent 名
#   model            : 指定模型家族（异构审计核心）
#   run_in_background: true → 后台并行不阻塞，完成主动通知
#   isolation        : "worktree" → git worktree 隔离，不改宿主工作树
#   thinking         : off/minimal/low/medium/high/xhigh
```

**pi worktree 并行开发（借鉴 harness orchestrator 模式）**：
```
# 三 agent 并行开发不同模块（类似 harness 的 fan-out 模式）：
Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="实现前端 auth 模块。只改 src/components/。完成后 commit。",
  run_in_background=true)

Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="实现后端 auth API。只改 src/api/。完成后 commit。",
  run_in_background=true)

Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="写 auth 模块测试。只改 tests/。完成后 commit。",
  run_in_background=true)

# 合并：主 agent 读各 worktree 产物 → git merge → 跑测试闸门
```

**pi 链式委派（scout → planner → worker）**：
```
# pi 的 subagent 扩展支持链式模式，用 {previous} 传上下文：
# 1. scout: 找到所有相关代码
# 2. planner: 基于 {previous} 创建实现计划
# 3. worker: 基于 {previous} 实现
# 每步输出自动传给下一步，中间失败即停
```

**pi 长任务恢复（stale 免疫）**：
```bash
pi -c                    # 续最近会话（上下文拼接，不刷新磁盘）
# 进会话第一句必须：「读 .lto/<run-id>/run-state.md 确认当前状态」
# 然后跑 git diff HEAD 交叉确认磁盘 vs 上下文记忆
# 不信任上下文里「上一轮已修」

pi --fork <session-id>   # fork 会话到新文件（保留原会话，新开分支）
```

**pi 配置速查**：
```bash
# 查看已装 models
pi --list-models

# 只读模式（安全审计）
pi --tools read,grep,find,ls -p "审计这个 spec"

# 禁用所有扩展（干净环境）
pi --no-extensions --no-skills -e ./my-ext.ts

# 导出会话为 HTML
pi --export session.jsonl audit-report.html
```

1. **pi 有 `Agent` 工具，不是「子进程/tmux」**（采纳）：pi 自述 `Agent`（subagent_type / run_in_background / isolation:worktree / steer_subagent）是第一公民抽象，与 tmux window 语义不同。把 Agent 当子进程用会丢弃 worktree 隔离/模型选择/后台回收。实测旁证：pi 当宿主直接派工成功，没用 tmux。
2. **allowed-tools 对 pi 不静默忽略**（存疑，据 pi 自述）：pi 称读到 `Task` 会找不到对应工具→能力缺口，需映射 `Task`→`Agent`。主 agent 未独立验证 pi 的解析行为，但 pi 是当事方，可信度较高。**结论**：body 一律写能力描述不写工具名（这条无论 pi 自述是否精确都对）。
3. **thinking 模型耗时预算**（采纳，实测坐实）：pi/deepseek 审 16KB spec 170-200s。单轮审计 timeout ≥ 240s（留 20% 余量）。**exit=124 是 timeout 不是空返回**（validation-log 踩过 3 次的归因错）。pi 当宿主且自己也审时，spec 起草/亲核同样慢，整圈预算按非 thinking 模型 3-5× 估。
4. **`--continue` 的 stale 陷阱**（采纳，通用化）：pi `--continue`（及 codex `--resume`）拼接历史上下文，不刷新文件系统状态。每轮启动先 `git diff HEAD` 确认磁盘 vs 上下文记忆、重读上一轮 blocker 清单、不信上下文里「上一轮已修」——磁盘才是真源。
5. **DeepSeek 双模降级异构**（采纳进降级矩阵）：只有 pi 一家可用时，用 v4-pro(thinking) + v3(non-thinking) 双模自审——非同家族但 thinking vs non-thinking 出错模式有差异（thinking 过度推理 / v3 漏边界），残余交叉诊断价值，强于纯同模型多实例。仍须声明对抗性大幅缩水。

## 四、agy 当宿主专项（agy 自评，已核验采纳）

1. **`agy -i "初始prompt"` 启动**：交互式必须带初始 prompt，不带会立即退出 → 通用派工脚本拉起 agy 时因管道适配崩溃。派工端把指令重组为 `agy -i "Prompt"`。
2. **无需放开沙箱**：agy 当宿主用细粒度权限弹窗授权，不要套 codex 的 bypass（安全降级）。
3. **`--print-timeout` 防上游 API 超时**：审大文件（16KB+）给 agy 传 `--print-timeout <秒>` 防上游连接过早断开。
4. **`--continue` 断点恢复**：横切纪律里 agy 的 stale 恢复用 `agy --continue` 结合 `git status`（同 pi 的 §三.4）。

## 五、主 agent 的核验态度（不盲从）

三家自评质量都高，但仍逐条核验：实测能坐实的（沙箱差异、thinking 耗时、agy 启动）直接采纳；属各家自述其内部机制、主 agent 无法外部验证的（pi 的 Agent 工具细节、allowed-tools 解析行为），标注「据自述」不绝对化。这本身是 skill §4「核验而非信仰」——连被审对象给的改进意见也要核，不因为「它最懂自己」就全盘照收。

## 六、派工前 preflight（以详细指标为依据，不靠"我觉得三家都行"）

异构审计派工前**必须先 preflight**——这次实测靠人工一个个试各 runner 健康度，浪费大量时间且误判（claude 的 35 字节在 codex 沙箱里被误读过）。preflight 是清单，不是框架：

落地产物：用 `../templates/preflight.md` 写 `.lto/<run-id>/preflight.md`。preflight 不是额外文档工作，它是「实际用了几家、为什么降级、timeout 怎么给」的证据。

**第 1 步：runner 健康巡检（实物工具）**
跑 `agent-delegate/scripts/runners/healthcheck.sh`，得一张**详细指标表**，按 verdict 挑能用的家：

```
RUNNER   EXIT   ELAPSED  BYTES    VERDICT
codex    0      9s       2        OK        ← 派
pi       0      29s      2        OK        ← 派（但慢，timeout 给足≥240s）
agy      0      11s      2        OK        ← 派
claude   1      1s       35       ERROR     ← 35字节1秒=未登录，别派，先 /login
```

**关键：以「退出码 + 耗时 + 字节数」三元组判定，不是 ok/fail 二元**——
- `exit=0 + 字节>0` = OK
- `exit=0 + 字节=0` = EMPTY（输出/参数问题，查 `--model`/`--mode`/重定向，**不是慢**）
- `exit=124` = TIMEOUT（给短了，加大 timeout 重试，**不是坏**）
- 非零 = ERROR（看 stderr：未登录/缺 token/CLI 缺失）

混淆这三者是上次连续归因错 3 次的根因（见 validation-log）。`healthcheck.sh --json` 供脚本消费。

**第 2 步：宿主权限画像（codex 当宿主时，源自 codex 自评建议，不实现成框架——手动核一遍即可）**
- 宿主模式 interactive / exec？sandbox（read-only / workspace-write / danger-full-access）？approval policy？
- 子 runner 写路径（`~/.pi`/`~/.gemini`/`~/.codex`/XDG cache）放行了吗？network roundtrip 允许吗？MCP 在 child context 可见吗（`codex mcp list` smoke）？
- 任一不放行 → **降级**为「本机自审 / 仅用 healthcheck verdict=OK 的子集」，**不硬宣称 triad 可用**。

**第 3 步：宣称范围 = 实测范围（诚实护栏）**
异构审计结论里显式写「实际用了 N 家异构」——healthcheck 挑出几家 OK 就是几家，不把"理论三家"当"实测三家"。这条直接治这次「单轮当多轮」「沙箱一刀切」式的过度宣称。
