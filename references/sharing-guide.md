# 分享给朋友 — 前置安装清单 + 降级路径

> 主文件 §5 的展开。这个 skill 深度引用两个私有 skill（agent-delegate / memory-flow）和项目脚本（deploy.sh）。朋友没有这些也能用核心纪律——下面分清「0 安装能用什么」和「跑满整套要装什么」。

## 一、最小可用集（0 安装）

只用这些纯方法论，拷过去就能用，不依赖任何基建：

- §0 防归因三道闸口令
- §2 闸一（premature 挂 X）、闸三（用户拍板）、收缩不抽象
- §3 B4（核验而非信仰）、B5（机制真通电）
- §1 横切的 stale 免疫 + 后台不阻塞

**这是本 skill 真正可移植的硬核。** 朋友哪怕单 agent、无记忆库、无异构 runtime，照这几条做就能避开过度设计 + 长程翻车。

## 二、跑满整套主循环需要

| # | 依赖 | 是什么 | 没有则降级 |
|---|---|---|---|
| 1 | `agent-delegate` skill | 异构三方审计（triad.sh + codex/pi/agy runner），依赖 `tmux-autopilot` | 同模型多 subagent 自审；**对抗性大幅缩水，须显式声明「未做异构交叉」** |
| 2 | 3 个异构 runtime | codex(OpenAI) / pi(DeepSeek) / agy(Gemini) 本机装好各持 token | 同上降级 |
| 3 | `memory-flow` skill + 服务 | 经验库（6 库 + 溯源 + 衰减 + ranking），落盘与真数据闸门都用 | 落盘换 ADR/`MEMORY.md`；真数据闸门换朋友自己的生产库/日志源 |
| 4 | 生产数据访问 | 真数据探针要能跑聚合查询（mf 的库仅内网可达） | 换朋友自己的生产库/指标源；无则造样本（盖不住真实分布） |
| 5 | 可回滚部署脚本 | mf 的 deploy.sh：dry-run + health-check + 自动 .bak 回滚 + master guard | 自己复刻 **dry-run + auto-rollback** 两个安全网，否则别上生产 |
| — | AskUserQuestion / Task / Workflow | Claude Code 原生 | 无需安装 |

## 三、安装顺序建议

```
只装方法论(§0/§2 闸一闸三/§4 核验/stale 免疫)  → 0 安装，立刻可用
  ↓ 想要异构审计闭环
装 agent-delegate(+ tmux-autopilot + 3 runtime)  → 解锁异构收敛
  ↓ 想要落盘复利
装 memory-flow(+ 常驻服务)                       → 解锁决策落盘检索
  ↓ 想要完整主循环上生产
接生产库 + 复刻 deploy.sh 安全网                  → 完整闭环
```

## 四、在 codex / pi / gemini 当宿主时怎么用（cross-runtime）

本 skill 是 runtime-agnostic 的（主文件 §7）。朋友直接在 codex CLI / pi / gemini CLI 里用时：

- **skill 照常加载**：`allowed-tools` 里的 `Task`/`AskUserQuestion` 是 Claude 专属 hint，codex/gemini **静默忽略不报错**（skill-creator 实测 12/12 过）。它们读 body 的**能力描述**照样能跑。
- **能力映射**（body §7 表）：「问用户拍板」→ 各自交互提问；「起独立 agent」→ 子进程/tmux window；「后台并行」→ tmux 多 window。
- **异构审计的关键**：审计方必须跟**当前宿主**不同家族。朋友用 codex 当宿主 → 派 claude+pi+agy 审；用 pi 当宿主 → 派 claude+codex+agy。agent-delegate 的 runner 表本来就覆盖四家、互为委派方。
- **落盘**：无 memory-flow MCP 时走 REST（带 `X-Agent-ID` 标明是谁写的），或降级 ADR/MEMORY.md。

**一句话**：谁当宿主，就把「另外几家」当审计方——这正是异构对抗性的来源，跟宿主是不是 Claude 无关。

#### pi 当宿主 quickstart

```bash
# 1. 进 pi 交互式会话（自动加载 skills 目录下的 LTO）
pi

# 2. 会话内触发 LTO——说「开个 MVP」或「起 spec」即命中
# pi 的 Agent 工具映射 LTO §7 的「起独立 agent」：
#   Agent(subagent_type, model, run_in_background, isolation="worktree")

# 3. 异构审计：pi 当宿主 → 派 codex + agy 审（不派自己）
# pi 用 Agent 工具起后台审计方：
#   Agent(subagent_type="general-purpose", model="codex", ...)
#   Agent(subagent_type="general-purpose", model="gemini", ...)

# 4. 落盘：pi 有 memory-flow MCP → experience_write 落盘
# 无 memory-flow → 降级 docs/decisions/ ADR
```

### 真机实测暴露的两个坑（2026-05-31 codex 当宿主实测，见 validation-log.md）

1. **宿主派工能不能成，取决于宿主 CLI 的沙箱模型——这是各家差异，不是通用铁律**。
   ⚠ 早期版本把「必须放开沙箱」写成所有宿主的硬前提——**这是错的**（codex/pi/agy 三家自评一致反驳，且实测坐实）。真相是各家不同：

   | 宿主 CLI | 沙箱对派工的影响 | 派工前提 |
   |---|---|---|
   | **codex** | `exec`/TUI 默认沙箱挡子 runner 写文件（`~/.pi` 锁、`~/.gemini` 日志）→ triad 派的全 `FAIL` | **必须** `--dangerously-bypass-approvals-and-sandbox`（或 `-s danger-full-access`）。代价：全盘放权有安全风险，仅受控本机用 |
   | **pi** | TUI 派工**无需放开沙箱**（pi 自述其 `Agent` 是内部机制不受外层沙箱限） | 默认即可派工 |
   | **agy** | TUI 派工**无需放开沙箱**（agy 自述用细粒度权限弹窗授权，强行套 codex 的 bypass 反而是安全降级） | 默认即可派工 |

   **codex 的更优解（codex 自评建议，未实测）**：不一定要全盘 bypass，可给子 runner 专用可写 roots / 专用 HOME / XDG 目录，最小放权。本 skill 不鼓励「长任务编排 = 全盘放权」的绑定。
   **codex host preflight（codex 自评建议）**：codex 当宿主前先记录 sandbox/approval/network/MCP 画像，任一不放行就降级为「本机自审/活着的 runner 子集」，不要硬宣称 triad 可用。
2. **审计方 runner 各家健康度不一，派工前先 smoke**。实测三家最终结果：agy 正常（审 16KB spec 真评审）；pi **慢但可用**（审 16KB 耗时 ~170-200s，给足 timeout 即出 5914 字节真评审）；claude headless 未登录需先 `/login`。**结论**：异构审计别假设三家都活，派工前对每个 runner 跑一次 `echo "1+1" | runner` smoke；某家挂了就用活着的、并在结论里声明「实际用了 N 家异构」。
3. **审计方 timeout 要按模型速度给足，并分清 exit=124(timeout) 和 exit=0(空返回)**。pi/deepseek 审 16KB spec 耗时近 200s，timeout 给 190s 就 exit=124 失败、给 200s+ 就出真评审——纯粹是慢，不是坏。**诊断教训**（见 validation-log）：我把 timeout(124) 和空返回(0) 混为一谈，连续归因错 3 次（沙箱→model→非TTY）才发现真因最朴素就是慢。**退出码是最硬的一手信号，归因前先把每次失败的退出码列清。**

## 五、提醒朋友的三个坑

1. **别把仪式当因果**：装了 agent-delegate 跑三方审计，不等于不会过度设计。三道闸（尤其闸一挂 X）才是防过度设计的核心，审计只是推进引擎。
2. **降级要声明**：用同模型 subagent 替代异构三方时，必须在结论里写明「未做异构交叉，对抗性弱」——否则会高估结论可信度。
3. **真数据闸门不能省阈值**：换自己的数据源没关系，但「先承诺阈值再看数」这一步不能省，否则闸门退化成「跑个数据找继续做的理由」。
