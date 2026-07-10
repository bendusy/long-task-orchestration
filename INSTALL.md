# INSTALL — long-task-orchestration

## 这个 skill 是什么

`long-task-orchestration`（LTO）是一套**长程任务从 spec 到生产的上层编排纪律**。它的核心是三道可证伪的闸门：**premature 闸**（用一个具体缺失信号 X 挂住"是不是太早"）、**真数据探针**（先承诺量化阈值再看数，不事后找理由继续）、**用户拍板**（判停权在人手里而非 AI 收敛）。在此之上，它通过**异构多方审计**（让跟宿主不同家族的模型来找真盲区）驱动推进、通过**亲核源码否决**而非信仰任何一方的报告，用**严格部署定序**（schema 先于代码 → dry-run → 只读探针 → 端到端实测）保证交付不翻车，并在每个拍板点**即时落盘反例与天花板**。纯方法论部分（三道闸、核验否决、stale 免疫、后台不阻塞）零依赖，拷过去就能用；三个可插拔插槽（异构审计 / 记忆落盘 / 派工）支持从最小降级到完整闭环按需装配。

---

## 安装

### 0. 依赖矩阵

| 层级 | 必需性 | 依赖 | 用途 | 没有时 |
|---|---|---|---|---|
| Rust v2 CLI | 必需 | Rust stable + Cargo | 运行/验证 `lto-rs` 当前接管线 | 不能验证当前 Rust 路线 |
| 核心 CLI | 必需 | bash | installer、wrapper、runner shell | 不能安装 wrapper |
| 核心 CLI | 必需 | git | HEAD 锚定、drift 检测、worktree 沙箱 | 多数长任务证据不完整 |
| 操作系统 | 必需 | macOS / Linux | 当前 CI、release binary、内置 shell runner 支持面 | Windows 原生支持暂缓；可用 WSL/类 Unix shell 自行验证 |
| 宿主 | 必需 | Codex / Claude Code / pi / agy / 其他能读 `SKILL.md` 并跑 shell 的 agent | 作为主 agent 推进任务 | LTO 只是文件和脚本，不能自己工作 |
| 异构审计 | 内置 | `scripts/delegate/` | 标准化派 codex/claude/pi/agy 审计 | runtime 不可用时用 `collect-agent-run` 或 manual evidence 登记已有报告 |
| 异构审计 | 可选 | `tmux` | 内置 delegate 的可观测并行窗口 | 无 tmux 时用 headless 子进程 |
| 异构审计 | 可选 | codex / claude / pi / agy CLI，至少 2 个非宿主家族可用 | 真异构交叉审计 | 同 runtime 自审，必须声明对抗性弱 |
| artifact memory | 可选 | ANIMEM 或 memory-flow compatible sink | 跨 runtime / 跨项目发现历史 run 和产物 | 本地 `.lto` + ADR 仍完整可用 |

LTO 已预装最小 delegate runtime：`scripts/delegate/`。如果你另有完整
agent-delegate 安装，可用环境变量覆盖内置脚本：

```bash
export AGENT_DELEGATE_HOME=/path/to/agent-delegate
export AGENT_DELEGATE_TRIAD=/path/to/agent-delegate/scripts/triad.sh
export AGENT_DELEGATE_RUNNERS=/path/to/agent-delegate/scripts/runners
```

`bash scripts/install.sh --check` 检查 Rust CLI 和全局 `lto` wrapper 状态；
可选 runtime 是否可派工，要在目标机器上跑 `lto preflight` 和
`scripts/delegate/runners/healthcheck.sh` 取得实测结果。

### 1. 放 skill 目录

把 `long-task-orchestration/` 整个目录放进你 agent 的 skills 加载路径。不同 agent 的惯例路径：

| Agent | Skills 路径（示例） |
|---|---|
| **Claude Code** | `~/.claude/skills/long-task-orchestration/` |
| **Codex CLI** | `~/.codex/skills/long-task-orchestration/`（或项目内 `.codex/skills/`） |
| **Cursor / Windsurf** | `.cursor/rules/`（把 SKILL.md 内容整合进 rule 文件） |
| **其他 agent** | agent 文档里的 "custom instructions / skill / prompt library" 目录 |

SKILL.md + `references/` 子目录要一起放进去（主文件通过 `[[wikilink]]` 引用 references）。

### 1b. 安装全局 `lto` wrapper（可选）

在仓库根目录运行：

```bash
bash scripts/install.sh          # 生成/刷新 ${LTO_BIN_DIR:-$HOME/.local/bin}/lto
bash scripts/install.sh --check  # 只检查，不写文件
lto self-test                    # wrapper 执行 Rust CLI
```

这个脚本只安装 sentinel-managed `lto` 命令，不会自动把本仓软链到各
runtime 的 skills 目录。skill 装载路径由你按上表复制或软链。

Rust 二进制安装是 release-gated：先查 GitHub Releases 是否已有对应平台的
`.tar.gz` 和 `.sha256`，校验 checksum 并运行 `./lto-rs self-test` 后再使用。
二进制安装、checksum 校验和 release 打包流程见
[`references/rust-migration-release.md`](./references/rust-migration-release.md)。

### 2. 验证加载

agent 能读到 `SKILL.md` 并命中 description 里的触发场景（「长程任务」「开个 MVP」「起 spec」「是不是太早」「过度设计了吗」等）即生效。不需要重启或安装包。

> 注：触发词以 SKILL.md frontmatter `description` 字段的实际内容为准。`premature`、`三道闸` 等词在 body 里，不在 description 触发关键词里；agent 会按能力描述全文语义匹配，不需要逐词对齐。

### 3. 替换部署示例

`references/deploy-sequencing.md` 使用中性 `example_service` 示例说明顺序。
真实项目上线前，把其中的表名、服务名、构建命令和部署脚本替换成你自己的
拓扑；LTO 只规定顺序，不知道你的生产环境。

---

## 三个插槽：零配置 vs 装配升级

LTO 依赖的是**接口**，不是具体实现。每个插槽都有两档：不装任何东西也能用降级实现；装上对应工具就解锁完整版。

---

### 插槽 1 — 异构审计

**用途**：让跟宿主不同家族的模型来审 spec/代码，暴露宿主自身盲区。

| 档位 | 配置 | 能力 | 声明要求 |
|---|---|---|---|
| **零配置** | 不装任何东西 | 用当前 agent 起同家族 subagent 多视角自审（2-3 个独立子 agent，不共享上下文） | **必须**在结论里声明「未做异构交叉，对抗性弱于真异构」 |
| **完整版** | 使用内置 `scripts/delegate/` + `tmux` + 本机装好 codex / claude / pi(DeepSeek) / agy(Gemini) 中至少两家非宿主 runtime | 真异构三方审计（宿主 Claude → 派 codex+pi+agy；宿主 codex → 派 claude+pi+agy，以此类推） | 记录实际可用了几家；**派工前对每个 runner 跑 smoke 巡检**（见下），不以"理论三家"当"实测三家" |

**关键规则**：审计方必须跟当前宿主**不同模型家族**——同家族多实例约等于自我重复，无交叉诊断价值。谁当宿主，就把另外几家当审计方。

**派工前 smoke 巡检（完整版必做）**：三家 runner 健康度不一，不能假设都活。派工前对每个 runner 跑一次 smoke（`echo "1+1" | runner`），以退出码 + 耗时 + 字节数三元组判定，只派 verdict=OK 的家，并在结论里显式写「实际用了 N 家异构」。详细 preflight 步骤见 `references/cross-runtime-host-notes.md` §六。

**宿主差异注意**（cross-runtime 实测，见 `references/cross-runtime-host-notes.md`）：

- codex 当宿主：默认沙箱会挡子 runner 写文件，triad 派工全 FAIL；需 `--dangerously-bypass-approvals-and-sandbox` 才可用，仅受控本机场景适用。更优解是给子 runner 专用可写 roots/HOME，最小放权，而非全盘 bypass。
- pi / agy 当宿主：无需放开沙箱，默认可派工。
- 任何宿主：pi/DeepSeek 审 16KB 内容耗时可达 170-200s，timeout 要给足 240s+；agy 交互式启动用 `agy -i ''` 拉起真实 TUI，长 prompt 随后 paste，不能退回只给方案的 `--print`。pi/agy 的 dispatch 完成由 TUI 进程退出 wrapper 读取真实 rc；Codex Stop 只代表一轮结束，只有 `/goal` 的 `update_goal complete` 证据才算 dispatch 完成。

---

### 插槽 2 — 记忆落盘

**用途**：把每个拍板点的决策、反例、天花板即时写入可检索的持久存储，形成跨会话的决策可追溯链。

| 档位 | 配置 | 能力 | 丢的东西 |
|---|---|---|---|
| **零配置** | 不装任何东西 | 写 `docs/decisions/` ADR 文件（一决策一文件）+ 项目根 `MEMORY.md` 索引 | 没有衰减/reinforce/语义检索，靠人工找 |
| **完整版** | 接入 ANIMEM / `memory-flow` / compatible artifact-memory sink | 语义检索、衰减权重、取代式版本链、跨项目经验复用 | — |

**预留插槽说明**：ANIMEM / `memory-flow` 之外的记忆后端也可以接入。
LTO 的公开语义只是 artifact memory projection：导出 redacted run snapshot、
显式 publish、resume 时读取历史线索。没有 sink 时，本地 `.lto` 仍是真源。

**降级纪律不变**：无论用哪一档，都要做到：拍板后即时写（不事后补）、记录为什么判 premature（缺的那个 X）、记录被证伪/否决的 blocker、commit message 引 slug 或 ADR 文件名。丢的只是检索命中率和复利，不丢纪律本身。

详细模板 → `references/decision-logging.md`

---

### 插槽 3 — 派工

**用途**：把审计/调研任务丢到后台并行跑，主对话不阻塞。

| 档位 | 配置 | 落地方式 |
|---|---|---|
| **零配置** | 不装任何东西 | 用 agent 原生机制起 subagent（Claude Code 用 `Task`/`Workflow`；codex 用 `codex exec`/tmux；pi 用 `Agent` 工具；agy 用子进程/tmux）——各家机制不同，见 SKILL.md §7 能力映射表 |
| **完整版** | 使用内置 `scripts/delegate/`（`triad.sh` + runner 表） | 标准化多 runtime 派工接口，统一管 tmux window、wait-for 回收、反迎合 prompt 约束 |

**后台派工原则**（纯方法论，两档通用）：
- 派出去就不要轮询，设长兜底心跳，等通知。
- 等 `agent.dispatch.completed`，不要用 per-turn 的 `agent.turn.completed` 代替整个 goal 完成。
- LTO 新建窗口按 `lto:<runner>:<goal-slug>` 展示、按不可变 `@window_id` 寻址；成功自动清理，失败/超时/`--keep-window` 保留。
- 等待期挖下一步的事实地基（真实代码 / 真实分布 / 真实配置），不靠记忆。
- 多批并行分批起，每批都完整深做，不为省时间砍深度。

---

## 最小可用集（不装任何后端，纯方法论）

**0 安装，拷过去就能用。** 以下这些纯方法论零依赖，覆盖长程任务最高 ROI 的部分：

| 纯方法论条目 | 主文件位置 |
|---|---|
| 防归因三道闸口令 | §0 |
| 闸一（premature 挂具体 X） | §2 |
| 闸三（用户拍板，AI 不替代） | §2 |
| 收缩不抽象（切最小硬核子集） | §2 |
| 核验而非信仰（亲核源码否决，含否决自己） | §3 B4 |
| 机制真通电（手走真实路径，不止 health 200） | §3 B5 |
| stale 免疫（/compact 后三层一手证据交叉确认） | §1 横切 + references/long-loop-state.md |
| 后台不阻塞（派后等通知，等待期挖地基） | §1 横切 |
| 部署定序铁律（schema 先 → dry-run → 只读探针 → 端到端实测 → 观察窗） | §4 + references/deploy-sequencing.md |

**这是本 skill 真正可移植的硬核。** 哪怕单 agent、无记忆库、无异构 runtime，照这几条做就能避开过度设计和长程翻车。

---

## 不适用场景

LTO 不是万能的，以下场景走其他路径更高效：

| 场景 | 走哪里 |
|---|---|
| 单一 bugfix | diagnose/investigate skill |
| 纯一次性代码审查 | review skill |
| 只需要委派一轮给另一个 runtime | `scripts/delegate/delegate.sh` |
| 写 skill 本身 | `skill-creator` |
| 纯跑一条部署命令 | 走 ship/land-and-deploy，不需要整套编排 |

**触发 LTO 的典型场景**：「开个 MVP / 起 spec / 是不是过度设计了 / 从设计走到上线做多轮迭代 / 长任务编排 / 反复审计-修复-部署-实测-落盘的推进循环」。

---

## 安装后的三个常见坑

1. **别把仪式当因果**：跑了三方审计，不等于不会过度设计。三道闸（尤其闸一挂具体 X）才是防过度设计的核心；审计只是推进引擎。
2. **降级要声明**：用同模型 subagent 替代异构三方时，必须在结论里写明「未做异构交叉，对抗性弱」——否则会高估结论的可信度。
3. **真数据闸门不能省阈值**：换自己的数据源完全没问题，但「先承诺阈值再看数」这一步不能省，否则闸门退化成「跑个数字找继续做的理由」。
