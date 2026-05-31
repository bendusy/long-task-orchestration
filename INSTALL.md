# INSTALL — long-task-orchestration

## 这个 skill 是什么

`long-task-orchestration`（LTO）是一套**长程任务从 spec 到生产的上层编排纪律**。它的核心是三道可证伪的闸门：**premature 闸**（用一个具体缺失信号 X 挂住"是不是太早"）、**真数据探针**（先承诺量化阈值再看数，不事后找理由继续）、**用户拍板**（判停权在人手里而非 AI 收敛）。在此之上，它通过**异构多方审计**（让跟宿主不同家族的模型来找真盲区）驱动推进、通过**亲核源码否决**而非信仰任何一方的报告，用**严格部署定序**（schema 先于代码 → dry-run → 只读探针 → 端到端实测）保证交付不翻车，并在每个拍板点**即时落盘反例与天花板**。纯方法论部分（三道闸、核验否决、stale 免疫、后台不阻塞）零依赖，拷过去就能用；三个可插拔插槽（异构审计 / 记忆落盘 / 派工）支持从最小降级到完整闭环按需装配。

---

## 安装

### 1. 放 skill 目录

把 `long-task-orchestration/` 整个目录放进你 agent 的 skills 加载路径。不同 agent 的惯例路径：

| Agent | Skills 路径（示例） |
|---|---|
| **Claude Code** | `~/.claude/skills/long-task-orchestration/` |
| **Codex CLI** | `~/.codex/skills/long-task-orchestration/`（或项目内 `.codex/skills/`） |
| **Cursor / Windsurf** | `.cursor/rules/`（把 SKILL.md 内容整合进 rule 文件） |
| **其他 agent** | agent 文档里的 "custom instructions / skill / prompt library" 目录 |

SKILL.md + `references/` 子目录要一起放进去（主文件通过 `[[wikilink]]` 引用 references）。

### 2. 验证加载

agent 能读到 `SKILL.md` 并命中 description 里的触发场景（「长程任务」「开个 MVP」「起 spec」「是不是太早」「过度设计了吗」等）即生效。不需要重启或安装包。

> 注：触发词以 SKILL.md frontmatter `description` 字段的实际内容为准。`premature`、`三道闸` 等词在 body 里，不在 description 触发关键词里；agent 会按能力描述全文语义匹配，不需要逐词对齐。

### 3. 替换 references/ 里的项目特化示例

`references/deploy-sequencing.md` 包含基于作者项目的具体实例（占位符表名如 `<你的表>`、feature flag 等）。文件开头已声明"用户照着改成自己的拓扑"。**使用前把这些替换成你自己的数据库表和部署脚本**，否则示例会引起混淆。其他 references/ 文件（audit-convergence.md / decision-logging.md / long-loop-state.md / cross-runtime-host-notes.md / validation-log.md）是纯方法论，不需要替换。

---

## 三个插槽：零配置 vs 装配升级

LTO 依赖的是**接口**，不是具体实现。每个插槽都有两档：不装任何东西也能用降级实现；装上对应工具就解锁完整版。

---

### 插槽 1 — 异构审计

**用途**：让跟宿主不同家族的模型来审 spec/代码，暴露宿主自身盲区。

| 档位 | 配置 | 能力 | 声明要求 |
|---|---|---|---|
| **零配置** | 不装任何东西 | 用当前 agent 起同家族 subagent 多视角自审（2-3 个独立子 agent，不共享上下文） | **必须**在结论里声明「未做异构交叉，对抗性弱于真异构」 |
| **完整版** | 装 `agent-delegate`（含 `tmux-autopilot`）+ 本机装好 codex / pi(DeepSeek) / agy(Gemini) 各持 token | 真异构三方审计（宿主 Claude → 派 codex+pi+agy；宿主 codex → 派 claude+pi+agy，以此类推） | 记录实际可用了几家；**派工前对每个 runner 跑 smoke 巡检**（见下），不以"理论三家"当"实测三家" |

**关键规则**：审计方必须跟当前宿主**不同模型家族**——同家族多实例约等于自我重复，无交叉诊断价值。谁当宿主，就把另外几家当审计方。

**派工前 smoke 巡检（完整版必做）**：三家 runner 健康度不一，不能假设都活。派工前对每个 runner 跑一次 smoke（`echo "1+1" | runner`），以退出码 + 耗时 + 字节数三元组判定，只派 verdict=OK 的家，并在结论里显式写「实际用了 N 家异构」。详细 preflight 步骤见 `references/cross-runtime-host-notes.md` §六。

**宿主差异注意**（cross-runtime 实测，见 `references/cross-runtime-host-notes.md`）：

- codex 当宿主：默认沙箱会挡子 runner 写文件，triad 派工全 FAIL；需 `--dangerously-bypass-approvals-and-sandbox` 才可用，仅受控本机场景适用。更优解是给子 runner 专用可写 roots/HOME，最小放权，而非全盘 bypass。
- pi / agy 当宿主：无需放开沙箱，默认可派工。
- 任何宿主：pi/DeepSeek 审 16KB 内容耗时可达 170-200s，timeout 要给足 240s+；agy 交互式启动需带初始 prompt（`agy -i "..."`，不带会立即退出）。

---

### 插槽 2 — 记忆落盘

**用途**：把每个拍板点的决策、反例、天花板即时写入可检索的持久存储，形成跨会话的决策可追溯链。

| 档位 | 配置 | 能力 | 丢的东西 |
|---|---|---|---|
| **零配置** | 不装任何东西 | 写 `docs/decisions/` ADR 文件（一决策一文件）+ 项目根 `MEMORY.md` 索引 | 没有衰减/reinforce/语义检索，靠人工找 |
| **完整版** | 接入 `memory-flow` MCP 后端（`experience_write` / `experience_search`）或兼容接口 | 语义检索、衰减权重、取代式版本链、跨项目经验复用 | — |

**预留插槽说明**：`memory-flow` 之外的记忆后端（任何其他实现了 `experience_write` / `experience_search` 语义的工具）可以直接插进来替换——LTO 只调用「写一条有 slug 的决策条目」和「按 slug 检索」这两个最小接口。未来任何符合此接口的后端（本地文件、向量库、第三方记忆服务）都可插入，不需要改 skill 主文件。

**降级纪律不变**：无论用哪一档，都要做到：拍板后即时写（不事后补）、记录为什么判 premature（缺的那个 X）、记录被证伪/否决的 blocker、commit message 引 slug 或 ADR 文件名。丢的只是检索命中率和复利，不丢纪律本身。

详细模板 → `references/decision-logging.md`

---

### 插槽 3 — 派工

**用途**：把审计/调研任务丢到后台并行跑，主对话不阻塞。

| 档位 | 配置 | 落地方式 |
|---|---|---|
| **零配置** | 不装任何东西 | 用 agent 原生机制起 subagent（Claude Code 用 `Task`/`Workflow`；codex 用 `codex exec`/tmux；pi 用 `Agent` 工具；agy 用子进程/tmux）——各家机制不同，见 SKILL.md §7 能力映射表 |
| **完整版** | 装 `agent-delegate`（`triad.sh` + runner 表） | 标准化多 runtime 派工接口，统一管 tmux window、wait-for 回收、反迎合 prompt 约束 |

**后台派工原则**（纯方法论，两档通用）：
- 派出去就不要轮询，设长兜底心跳，等通知。
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
| 单一 bugfix | debug skill |
| 纯一次性代码审查 | review skill |
| 只需要委派一轮给另一个 runtime | `agent-delegate`（LTO 是它的调用方，不是替代） |
| 写 skill 本身 | `skill-creator` |
| 纯跑一条部署命令 | 直接走各自部署脚本，不需要整套编排 |

**触发 LTO 的典型场景**：「开个 MVP / 起 spec / 是不是过度设计了 / 从设计走到上线做多轮迭代 / 长任务编排 / 反复审计-修复-部署-实测-落盘的推进循环」。

---

## 安装后的三个常见坑

1. **别把仪式当因果**：装上 agent-delegate 跑了三方审计，不等于不会过度设计。三道闸（尤其闸一挂具体 X）才是防过度设计的核心；审计只是推进引擎。
2. **降级要声明**：用同模型 subagent 替代异构三方时，必须在结论里写明「未做异构交叉，对抗性弱」——否则会高估结论的可信度。
3. **真数据闸门不能省阈值**：换自己的数据源完全没问题，但「先承诺阈值再看数」这一步不能省，否则闸门退化成「跑个数字找继续做的理由」。
