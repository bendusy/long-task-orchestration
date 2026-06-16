# 分享给朋友 — 前置安装清单 + 降级路径

> LTO harness 分享说明。LTO 自带 `scripts/delegate/`（delegate.sh / triad.sh / runners/），可选集成 am（animem）CLI sink 和项目脚本（deploy.sh）。朋友没有这些也能用核心纪律——下面分清「0 安装能用什么」和「跑满整套要装什么」。

## 一、最小可用集（0 安装）

只用这些纯方法论，拷过去就能用，不依赖任何基建：

- SKILL.md 的三道刹车：缺口真实、数据达标、人来拍板
- `workflow-playbook.md` 的 playbook 思维：preset 是调度先验，不是硬路由
- `audit-convergence.md` 的不投票、亲核源码、blocker 单调收敛
- `long-loop-state.md` 的 stale 免疫、后台不阻塞、恢复锚点

**这是本 skill 真正可移植的硬核。** 朋友哪怕单 agent、无记忆库、无异构 runtime，照这几条做就能避开过度设计 + 长程翻车。

## 二、跑满整套主循环需要

| # | 依赖 | 是什么 | 没有则降级 |
|---|---|---|---|
| 1 | repo 自带 `scripts/delegate/` | 异构三方审计（triad.sh + codex/pi/agy runner），依赖 `tmux-autopilot` | 同模型多 subagent 自审；**对抗性大幅缩水，须显式声明「未做异构交叉」** |
| 2 | 3 个异构 runtime | codex(OpenAI) / pi(DeepSeek) / agy(Gemini) 本机装好各持 token | 同模型 subagent 自审；对抗性缩水 |
| 3 | am（animem）CLI sink（可选） | 经验落盘/检索/复利；`.lto/` 永远是真源，没装 am 也能跑（优雅降级） | 落盘换 ADR/`MEMORY.md`，纪律不变；memory-flow REST 是 legacy 兜底 |
| 4 | 生产数据访问 | 真数据探针要能跑聚合查询（mf 的库仅内网可达） | 换朋友自己的生产库/指标源；无则造样本（盖不住真实分布） |
| 5 | 可回滚部署脚本 | mf 的 deploy.sh：dry-run + health-check + 自动 .bak 回滚 + master guard | 自己复刻 **dry-run + auto-rollback** 两个安全网，否则别上生产 |
| — | AskUserQuestion / Task / Workflow | Claude Code 原生 | 无需安装 |

## 三、安装顺序建议

```
只装方法论(三道刹车/playbook/亲核/stale 免疫)  → 0 安装，立刻可用
  ↓ 想要异构审计闭环
装 tmux-autopilot + 3 runtime（repo 自带 scripts/delegate/）→ 解锁异构收敛
  ↓ 想要落盘复利
装 am（animem）CLI sink（可选，.lto/ 仍是真源）  → 解锁决策落盘检索
  ↓ 想要完整主循环上生产
接生产库 + 复刻 deploy.sh 安全网                  → 完整闭环
```

### 全局 `lto` wrapper

运行本仓库安装脚本会创建或刷新 `${LTO_BIN_DIR:-$HOME/.local/bin}/lto`：

```bash
cd <你的 long-task-orchestration checkout>
bash scripts/install.sh
bash scripts/install.sh --check
```

规则：

- wrapper 带 `# long-task-orchestration managed lto wrapper` sentinel；重装只覆盖托管文件。
- 如果同名 `lto` 已存在但没有 sentinel，安装按冲突退出 2，不覆盖用户文件。
- wrapper 内部记录当前 checkout 的 `target/release/lto-rs`，只执行 Rust CLI。Python fallback 已在 v0.5.0 退役；仓库移动后重跑安装。
- 如果 `$HOME/.local/bin` 不在 `PATH`，安装会 warning；可用 `LTO_BIN_DIR=/your/bin` 改落点。
- wrapper 透传参数，跨 repo 可用 `lto --repo /path/to/repo check` 或 `lto check --repo /path/to/repo`。

## 四、在 codex / pi / gemini 当宿主时怎么用（cross-runtime）

本 skill 是 runtime-agnostic 的。朋友直接在 codex CLI / pi / gemini CLI 里用时：

- **skill 照常加载**：`allowed-tools` 里的 `Task`/`AskUserQuestion` 是 Claude 专属 hint，codex/gemini **静默忽略不报错**（skill-creator 实测 12/12 过）。它们读 body 的**能力描述**照样能跑。
- **能力映射**（见 SKILL.md 的能力描述）：「问用户拍板」→ 各自交互提问；「起独立 agent」→ 子进程/tmux window；「后台并行」→ tmux 多 window。
- **异构审计的关键**：审计方必须跟**当前宿主**不同家族。朋友用 codex 当宿主 → 派 claude+pi+agy 审；用 pi 当宿主 → 派 claude+codex+agy。repo 自带 `scripts/delegate/` 的 runner 表本来就覆盖四家、互为委派方。
- **落盘**：先用 `write_decision.py` 写 repo-local ADR 并登记 artifact；am（animem）CLI sink 是当前默认，memory-flow REST 是 legacy 兜底，两者均不默认碰凭据。

**一句话**：谁当宿主，就把「另外几家」当审计方——这正是异构对抗性的来源，跟宿主是不是 Claude 无关。

#### pi 当宿主 quickstart：host 判断 → LTO 派 codex/agy/claude 异构审计

**完整流程**：pi (DeepSeek) 加载 LTO → pi 按 playbook 判断需要异构审计 → 调用 repo 自带 `scripts/delegate/` 派 codex (OpenAI) + agy (Gemini) + claude (Anthropic) → pi 综合三方结论。

```bash
# ===== 第 1 步：pi 启动，加载 LTO skill =====
pi
# 会话内：「审一下这个 spec，用异构三方审计」
# host agent 判断 review playbook 触发 → 进入审计阶段

# ===== 第 2 步：pi 写审计简报（review playbook 触发） =====
cat > /tmp/lto-audit-brief.md << 'EOF'
## 审计对象
[spec 内容]

## 审计重点
1. premature 假设是否存在？缺的具体信号 X 是什么？
2. 数据探针阈值是否预设？是否可证伪？
3. 部署安全网是否完整（schema 先于代码 / dry-run / 回滚）？

## 输出要求
逐 blocker 举证，附置信度 HIGH/MODERATE/LOW。先给最强反驳，禁止迎合。
EOF

# ===== 第 3 步：pi 通过 repo 自带 scripts/delegate/ 派工 =====
# runner 是独立 shell 脚本，任何宿主可调
AD="scripts/delegate/runners"  # path within this repo's checkout

# 派 codex (OpenAI)，后台跑
$AD/codex.sh /tmp/lto-audit-brief.md /tmp/lto-reply-codex.md 300 &
CODEX_PID=$!

# 派 agy (Gemini)，后台跑
$AD/agy.sh /tmp/lto-audit-brief.md /tmp/lto-reply-agy.md 300 &
AGY_PID=$!

# 派 claude (Anthropic)，后台跑
$AD/claude.sh /tmp/lto-audit-brief.md /tmp/lto-reply-claude.md 300 &
CLAUDE_PID=$!

# ===== 第 4 步：pi 不阻塞，做别的事 =====
# 等待期挖下一步地基：读真代码、真配置、真分布
# 三方跑完会各自退出，pi 检查 exit code + reply 文件

wait $CODEX_PID $AGY_PID $CLAUDE_PID

# ===== 第 5 步：pi 按 audit-convergence 综合 =====
# 亲核每份 reply（不投票、核源码）
# 分档 blocker → 单调递减判停 → 修 → 再审
# 用户拍板后进实现或部署
```

**如果用 triad.sh 一键派工**（需 tmux 环境）：
```bash
# repo 自带 triad.sh 自动开 tmux window 并行跑三家
cd <你的 long-task-orchestration checkout>
bash scripts/delegate/triad.sh \
  -p /tmp/lto-audit-brief.md \
  -r codex pi agy claude \
  -t 300
# 回收：读 replies/ 下各 runner 输出
```

**pi 派工的关键点**：
- pi **不派自己**（同家族无交叉诊断价值）→ 派 codex + agy + claude
- `scripts/delegate/` 的 runner 脚本是**语言无关**的——任何能跑 bash 的宿主都能调
- runner 统一接口：`runner.sh <prompt_file> <reply_file> <timeout_sec>`
- 不需要 pi 有内置 Agent 工具——bash + 子进程就够了
- pi/deepseek 自己作为综合裁决者（不投票、亲核源码），不做被审方

### 真机实测暴露的两个坑（2026-05-31 codex 当宿主实测，见 validation-log.md）

1. **宿主派工能不能成，取决于宿主 CLI 的沙箱模型——这是各家差异，不是通用铁律**。
   ⚠ 早期版本把「必须放开沙箱」写成所有宿主的硬前提——**这是错的**（codex/pi/agy 三家自评一致反驳，且实测坐实）。真相是各家不同：

   | 宿主 CLI | 沙箱对派工的影响 | 派工前提 |
   |---|---|---|
   | **codex** | `exec`/TUI 默认沙箱挡子 runner 写文件（`~/.pi` 锁、`~/.gemini` 日志）→ triad 派的全 `FAIL` | **必须** `--dangerously-bypass-approvals-and-sandbox`（或 `-s danger-full-access`）。代价：全盘放权有安全风险，仅受控本机用 |
   | **pi** | TUI 派工**无需放开沙箱**（pi 自述其 `Agent` 是内部机制不受外层沙箱限） | 默认即可派工 |
   | **agy** | TUI 派工**无需放开沙箱**（agy 自述用细粒度权限弹窗授权，强行套 codex 的 bypass 反而是安全降级） | 默认即可派工 |

   **codex 的更优解（codex 自评建议，未实测）**：不一定要全盘 bypass，可给子 runner 专用可写 roots / 专用 HOME / XDG 目录，最小放权。本 skill 不鼓励「长任务编排 = 全盘放权」的绑定。
   **codex host preflight（codex 自评建议）**：codex 当宿主前先记录 sandbox/approval/network/MCP 画像，任一不放行就降级为「本机自审/活着的 runner 子集」，不要硬宣称 triad 可用。Codex runner 具体控制面见 `references/codex-cli-control.md`：默认 `read-only`，用 `-C` 固定工作区、`-o` 固定回复文件、stdin 承载长 prompt。
2. **审计方 runner 各家健康度不一，派工前先 smoke**。实测三家最终结果：agy 正常（审 16KB spec 真评审）；pi **慢但可用**（审 16KB 耗时 ~170-200s，给足 timeout 即出 5914 字节真评审）；claude headless 未登录需先 `/login`。**结论**：异构审计别假设三家都活，派工前对每个 runner 跑一次 `echo "1+1" | runner` smoke；某家挂了就用活着的、并在结论里声明「实际用了 N 家异构」。
3. **审计方 timeout 要按模型速度给足，并分清 exit=124(timeout) 和 exit=0(空返回)**。pi/deepseek 审 16KB spec 耗时近 200s，timeout 给 190s 就 exit=124 失败、给 200s+ 就出真评审——纯粹是慢，不是坏。**诊断教训**（见 validation-log）：我把 timeout(124) 和空返回(0) 混为一谈，连续归因错 3 次（沙箱→model→非TTY）才发现真因最朴素就是慢。**退出码是最硬的一手信号，归因前先把每次失败的退出码列清。**

## 五、提醒朋友的三个坑

1. **别把仪式当因果**：跑了三方审计，不等于不会过度设计。三道闸（尤其闸一挂 X）才是防过度设计的核心，审计只是推进引擎。
2. **降级要声明**：用同模型 subagent 替代异构三方时，必须在结论里写明「未做异构交叉，对抗性弱」——否则会高估结论可信度。
3. **真数据闸门不能省阈值**：换自己的数据源没关系，但「先承诺阈值再看数」这一步不能省，否则闸门退化成「跑个数据找继续做的理由」。

## 六、让 agent 自动想起用 LTO（项目级注入）

光装 skill 不够——长任务跑一半模型经常忘了用 LTO。在**你自己项目**的 `CLAUDE.md` / `AGENTS.md` 加一段轻量触发指引（只放触发条件 + 入口命令，**不要**复制整套手册，手册在 SKILL.md，按需加载即可）：

```markdown
## 长任务（>3 文件 / 跨 session / 需审计 / 要上线）

开工前 `lto start --goal "…"`，跨 session 接续用 `lto resume` 拉回上下文胶囊，
迷路时先 `lto check`。详见 long-task-orchestration skill（不必整篇读，按阶段加载）。
```

若未安装 wrapper，用绝对路径替代：

```bash
cargo run --manifest-path <repo>/Cargo.toml -- \
  --repo . check
```

**不要做的**：
- ❌ 不要把 `lto start --install-hooks` 写进项目初始化脚本默认执行——hook 是 opt-in，自动装会撞 husky / pre-commit framework（见 `start.py` 的冲突检测）。
- ❌ 不要把 `--auto-commit` 设成默认——LTO 默认不替你 commit，提交权在你手里。
- ❌ 不要把整份 SKILL.md 贴进 CLAUDE.md——那是反渐进式披露，每次对话白烧 context。

### 和 pi-dynamic-workflows 同装会冲突吗？

**不会，且互补。** 两者编排的东西不同：
- `pi-dynamic-workflows`（Pi extension / Node VM）管 **agent fan-out**——拉独立子 agent 跑 LLM 任务，沙箱保证可复现。
- LTO（Py CLI）管 **长任务状态导航 + 批量跑 shell 校验**——跨 session 持久化、evidence 落盘、审计收敛。

Pi tool 命名上 LTO 用了 `lto_` 前缀（`lto_pipeline` / `lto_parallel`），与 pi-dynamic-workflows 注册的 `workflow` 工具不撞名。同装时模型两种能力都有：要派 agent 做 fan-out 审查/重构走 pi-dynamic-workflows，要长任务导航走 LTO。

> 注意：LTO 的 `parallel` / `pipeline` 子命令**借了 pi-dynamic-workflows 的命令名但语义不同**——LTO 编排的是 shell 命令（`pytest`/`lint`）不是 agent。这是命令批处理，不是 agent fan-out。别因为同名就以为它们能互相替代。
