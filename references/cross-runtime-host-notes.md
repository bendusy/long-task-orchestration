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
3. **`codex exec` 子任务契约**：子 runner 必须禁止反问用户、一次性输出到 stdout/约定文件、不依赖中途审批、失败返回非零退出码（空输出和 timeout 分开记）、prompt 写明「默认只审计写 report，不改仓库除非明确授权」。LTO 的具体 CLI 控制面见 `codex-cli-control.md`：`codex exec -C "$PWD" -s read-only -o reply.md - < prompt.md`，长 prompt 走 stdin，最终答复写 `-o` 文件，默认 read-only。
4. **MCP 不从宿主会话继承**：codex child runner 用 MCP 前要在同启动方式下跑 `codex mcp list` 或等价 smoke——别因为宿主会话有 MCP tool 就假设 child `codex exec` 也有。实测旁证：codex TUI 启动时 memory-flow MCP 就 `failed`。
5. **resume/fork 恢复协议**：codex 长任务恢复不信 transcript 旧状态。建议每轮写 `run-state.md`，并让产物登记进 `.lto/<run-id>/artifacts.json`（run id / git SHA / sandbox profile / tmux id / 每 runner 的 reply/stdout 路径 / MCP smoke / 已采纳否决 blocker）。resume 后从 state + manifest + tmux + 进程重建状态，不信「1 background terminal running」之类旧叙述。
6. **ANIMEM/memory-flow 是可选索引，不是本地真源**：codex 当宿主时可以先跑
   `lto memory resume --project <key>` 找跨项目 artifact memory，但没装 ANIMEM
   或 child MCP 不可见时必须降级到本地 `lto resume`。不要因为 memory projection
   比本地 `.lto` 旧就覆盖本地 state；`.lto/current` 和 `state.json` 永远优先。

## 三、pi 当宿主专项（pi 自评，已核验，部分存疑标注）

### pi 调用速查 sample

> **标注约定**：每条 sample 标明来源——`[CLI]` pi 官方命令行参数（v0.78+），`[ad]` agent-delegate runner 实测，`[内部工具]` pi coding agent 运行时内部工具（非 CLI flag）。

**pi 当宿主跑 LTO 主循环 `[CLI]`**：
```bash
# 交互式启动，自动加载 skills（多来源：~/.pi/agent/skills/、.pi/skills/、packages 等）
pi
pi "帮我审核这份 spec"           # 带初始 prompt 启动

# 指定模型和 provider
pi --provider deepseek --model deepseek-v4-pro

# 会话管理
pi -c                            # 续最近会话
pi --name "lto-chengpi-audit"    # 命名会话
pi --fork <session-id>           # fork 会话到新文件
```

**pi 当审计方（被 agent-delegate 派工，headless）`[ad]`**：
```bash
# agent-delegate runners/pi.sh 实测命令：
pi -p --provider deepseek --model deepseek-v4-pro "$(cat prompt.txt)" > reply.txt

# -p                : 非交互式，stdout 输出
# --provider/model  : 本机配置（非 pi 官方默认，需自行配 DEEPSEEK_API_KEY）
# timeout≥240s      : deepseek-v4-pro thinking 审 16KB ~170-200s

# pi headless 特点：无 banner、无审批弹窗、无 MCP 加载噪声（本机实测，非官方契约）
```

**pi 内部 Agent 工具（异构审计/并行开发）`[内部工具]`**：

> ⚠️ **`isolation="worktree"` 非 pi CLI 官方保证**：下面 sample 里的 worktree 隔离参数不是 pi CLI 的官方契约，仅 **harness / 特定 extension 可能提供**（属各家运行时实现细节，主 agent 无法外部验证，据 pi 自述）。安装/依赖它前**自行验证**你这套 pi 运行时是否真支持 worktree 隔离——别假设给了 `isolation="worktree"` 就一定起独立 worktree，没起的话并行 agent 会在同一工作树互相踩文件。

```
# pi 作为 coding agent 运行时的内部工具，非 CLI flag。
# 可用参数：subagent_type, model, run_in_background, isolation, thinking, max_turns

# 异构审计：pi 宿主起 codex + agy 审计方
Agent(
  subagent_type="general-purpose",
  model="nciex/gpt-5.5",
  prompt="你是审计方。逐条审 spec 的 premature 假设/数据探针阈值/部署安全网。先给最强反驳，禁止迎合。输出 blocker register。",
  run_in_background=true,
  isolation="worktree"   # 非 pi CLI 官方保证，仅 harness/特定 extension 可能提供，用前自行验证
)

# worktree 并行开发：三 agent 不同模块
Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="实现前端 auth 模块。只改 src/components/。", run_in_background=true)
Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="实现后端 auth API。只改 src/api/。", run_in_background=true)
Agent(subagent_type="general-purpose", isolation="worktree",
  prompt="写 auth 模块测试。只改 tests/。", run_in_background=true)
```

**pi 配置速查 `[CLI]`**：
```bash
pi --list-models                  # 查看可用模型
pi --tools read,grep,find,ls -p "审计"  # 只读模式
pi --no-extensions --no-skills    # 禁用自动发现
pi --export session.jsonl report.html  # 导出会话
```

1. **pi 派工：CLI 子进程 vs 内部 Agent 工具**（核验后修正）：pi CLI 层面无内置 subagent——派工方式分两层：① CLI headless `pi -p` 子进程（agent-delegate runner 实测可用）；② pi 运行时内部 Agent 工具（含 `subagent_type`/`isolation:worktree`/`run_in_background`），仅在 pi 作为 coding agent 运行时可用，非 CLI flag。原先文档将二者混淆为 pi "原生能力"，现已分列标注。

**pi 当宿主通过 agent-delegate 派 codex/agy/claude（推荐方式）**：
```bash
# ad 的 runner 脚本是语言无关的——任何能跑 bash 的宿主都能调
# 统一接口：runner.sh <prompt_file> <reply_file> <timeout_sec>
AD="scripts/delegate/runners"  # standalone repo; or set to your agent-delegate install path

# pi 当宿主时：不派自己（同家族无交叉诊断）→ 派 codex + agy + claude
$AD/codex.sh  /tmp/audit-brief.md /tmp/reply-codex.md  300 &
$AD/agy.sh    /tmp/audit-brief.md /tmp/reply-agy.md    300 &
$AD/claude.sh /tmp/audit-brief.md /tmp/reply-claude.md 300 &
wait
# pi 读三方 reply → 按 audit-convergence 综合（不投票、亲核源码）

# 或用 triad.sh 一键派工（需 tmux）
bash scripts/delegate/triad.sh \
  -p /tmp/audit-brief.md -d /tmp/audit-replies -a "codex agy claude" -t 300
```

**pi 当被审方（被其他宿主派工时）**：
```bash
# agent-delegate runners/pi.sh 实测命令
pi -p --provider deepseek --model deepseek-v4-pro "$(cat prompt.txt)" > reply.txt
# timeout≥240s：deepseek-v4-pro thinking 审 16KB ~170-200s
```
2. **allowed-tools 对 pi 的行为**（修正）：不再声称 pi 会映射 `Task`→`Agent`。`allowed-tools` 是 Claude Code hint；pi 和其他 runtime 如何处理属于各自实现细节，LTO 不做断言。body 一律写能力描述不写工具名。
3. **thinking 模型耗时预算**（采纳，实测坐实）：pi/deepseek 审 16KB spec 170-200s。单轮审计 timeout ≥ 240s（留 20% 余量）。**exit=124 是 timeout 不是空返回**（validation-log 踩过 3 次的归因错）。pi 当宿主且自己也审时，spec 起草/亲核同样慢，整圈预算按非 thinking 模型 3-5× 估。
4. **`--continue` 的 stale 陷阱**（采纳，通用化）：pi `--continue`（及 codex `--resume`）拼接历史上下文，不刷新文件系统状态。每轮启动先 `git diff HEAD` 确认磁盘 vs 上下文记忆、重读上一轮 blocker 清单、不信上下文里「上一轮已修」——磁盘才是真源。
5. **DeepSeek 双模降级异构**（采纳进降级矩阵）：只有 pi 一家可用时，用 v4-pro(thinking) + v3(non-thinking) 双模自审——非同家族但 thinking vs non-thinking 出错模式有差异（thinking 过度推理 / v3 漏边界），残余交叉诊断价值，强于纯同模型多实例。仍须声明对抗性大幅缩水。
6. **memory projection 恢复顺序**：pi 当宿主接手时，先 `git status` / `lto resume`
   读本地，再可选 `lto memory resume --project <key>` 查跨项目索引。若两者不一致，
   本地 `.lto` 胜出；远端 projection 只当“可能有旧 handoff/旧 review 的线索”。

## 四、agy 当宿主专项（agy 自评，已核验采纳）

1. **`agy -i "初始prompt"` 启动**：交互式必须带初始 prompt，不带会立即退出 → 通用派工脚本拉起 agy 时因管道适配崩溃。派工端把指令重组为 `agy -i "Prompt"`。
2. **无需放开沙箱**：agy 当宿主用细粒度权限弹窗授权，不要套 codex 的 bypass（安全降级）。
3. **`--print-timeout` 防上游 API 超时**：审大文件（16KB+）给 agy 传 `--print-timeout <秒>` 防上游连接过早断开。
4. **`--continue` 断点恢复**：横切纪律里 agy 的 stale 恢复用 `agy --continue` 结合 `git status`（同 pi 的 stale 陷阱条目）。
5. **未装 ANIMEM 不降级为失败**：agy 当宿主也按本地 `.lto` 优先。`lto memory resume`
   无 sink 配置时出现 warning 是正常 degraded，不是任务失败；继续用 `lto resume`
   和 artifacts manifest 接手。

## 五、主 agent 的核验态度（不盲从）

三家自评质量都高，但仍逐条核验：实测能坐实的（沙箱差异、thinking 耗时、agy 启动）直接采纳；属各家自述其内部机制、主 agent 无法外部验证的（pi 的 Agent 工具细节、allowed-tools 解析行为），标注「据自述」不绝对化。这本身是 SKILL.md 的核验证据原则——连被审对象给的改进意见也要核，不因为「它最懂自己」就全盘照收。

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

## 七、token 计量：各家 runner 的 sidecar 支持（2026-06-08 实测）

token sidecar 协议：runner 可选写 `<reply>.meta.json`（`{tokens_in, tokens_out, tokens}`），scheduler 跑完读它 merge 进 `AgentResult.cost`。不写就退化（向后兼容）。各家 CLI 暴露 token 的能力不同，**宣称范围 = 实测范围**：

| runner | token 可用 | 来源（实测） |
|--------|-----------|-------------|
| **codex** | ✅ 真实 | `codex exec --json` 的 `turn.completed.usage.input_tokens/output_tokens`（`CODEX_JSON=1` 触发）。实测单 turn，usage 即完整值 |
| **pi** | ✅ 真实 | `pi -p --mode json` 末个 assistant `message_end` 的 `usage.input/output/totalTokens`（实测 tokens_in=38695/out=36/total=40651）。reply 同时从该事件 content 的 text 块抽，json 解析失败回退 raw |
| **claude** | ✅ 真实 | `claude -p --output-format json` 输出单 JSON 对象：`result` 是 reply，`usage.input_tokens/output_tokens` + cache 字段。tokens rollup 含 cache（实测 in=16916/out=5/total=44112，total 远大于 in+out 因含 cache_creation）。reply 从 result 抽，解析失败回退 raw |
| **agy** | ❌ 不可用 | agy CLI `--print` 只出纯文本，无 `--json`/usage flag，`--log-file` 只有 OAuth 报错。**且本机实测 agy 未登录**（`not logged into Antigravity`）。等未来版本暴露 usage 再补 |
| **gemini** | ⏳ 未实现 | sidecar 协议对其开放，runner 写即生效；gemini-cli 已停服（继任 agy），未实测 |

`eval-run` 的 `comparison.token_metering_available` 按两腿是否都拿到 token 标注（与 `token_delta` 的 and 条件对齐），不可用时为 False，不静默假装有。
