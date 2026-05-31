# long-task-orchestration (LTO)

一套**长程任务从 spec 到生产的上层编排纪律**，打包成可加载的 agent skill。

不是"更努力地干活"，而是一组**可证伪的判停闸门 + 异构交叉验证 + 即时落盘**——专治长程任务里最常翻车的两类问题：**过度设计**（太早抽象、为不存在的需求建框架）和**长程失稳**（多轮迭代后偏离真相、信仰自己的旧结论、部署翻车）。

## 核心：三道可证伪的闸

| 闸 | 做什么 | 防什么 |
|---|---|---|
| **premature 闸** | 用一个**具体缺失信号 X** 挂住"是不是太早" | 防"凭感觉觉得该往下走"——没有 X 就不许继续 |
| **真数据探针** | **先承诺量化阈值，再看数** | 防"跑个数字事后找理由继续做" |
| **用户拍板** | 判停权在人手里，**AI 不替代收敛** | 防 AI 自我说服"已经够好了" |

在此之上：**异构多方审计**（让跟宿主不同家族的模型来找真盲区）驱动推进、**亲核源码否决**而非信仰任何一方报告（含否决自己）、**严格部署定序**（schema 先于代码 → dry-run → 只读探针 → 端到端实测 → 观察窗）保证交付不翻车。

## 三个可插拔插槽（0 安装到完整闭环）

LTO 依赖的是**接口**不是具体实现。纯方法论零依赖，拷过去就能用；三个插槽按需装配升级：

| 插槽 | 零配置降级 | 完整版 |
|---|---|---|
| **异构审计** | 当前 agent 起同家族 subagent 多视角（须声明对抗性弱） | 装 `agent-delegate`，真异构三方（codex / pi / agy 等不同模型家族交叉诊断） |
| **记忆落盘** | `docs/decisions/` ADR + `MEMORY.md` 索引 | 接 `memory-flow` 或任何兼容 `experience_write` / `experience_search` 的后端，得语义检索 + 衰减 + 版本链 |
| **派工** | agent 原生 subagent 机制 | 装 `agent-delegate`，标准化多 runtime 派工 + 回收 |

**最小可用集（0 安装）就覆盖最高 ROI 的部分**：三道闸、核验否决、stale 免疫、部署定序铁律。哪怕单 agent、无记忆库、无异构 runtime，照这几条做就能避开过度设计和长程翻车。

## 安装

把 `long-task-orchestration/` 整个目录（含 `references/`）放进你 agent 的 skills 加载路径。各 agent 路径惯例、三档插槽装配、replace 项目特化示例的步骤，全部见 **[INSTALL.md](./INSTALL.md)**。

skill 主文件是 **[SKILL.md](./SKILL.md)**，references/ 是各章节的执行细节展开。

## 跨 runtime

LTO 是 runtime-agnostic 的——**谁当宿主都能跑**。skill 主文件只写"为什么委派、收敛怎么判停、何时停、停了干嘛"的上层纪律，派工实现一律走插槽降级路径，**不重述某一家 CLI 的 runner/tmux/quirk**。这意味着 agent 会按自己**实际运行的环境**微调，而不是套用某个固定本机配置。

不同家族 CLI 当宿主时的差异注意（沙箱、timeout、启动方式）见 [`references/cross-runtime-host-notes.md`](./references/cross-runtime-host-notes.md)。

## 不适用

| 场景 | 走哪里 |
|---|---|
| 单一 bugfix | debug skill |
| 纯一次性代码审查 | review skill |
| 只委派一轮给另一个 runtime | `agent-delegate`（LTO 是它的调用方） |
| 写 skill 本身 | `skill-creator` |

## License

[Apache-2.0](./LICENSE)
