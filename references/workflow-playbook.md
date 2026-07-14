# LTO workflow playbook

> 给宿主 agent 读的调度先验。这里的 `review` / `debug` / `migration` /
> `claim-verify` / `research` 不是 CLI preset，也不是 LTO 替你做决定的硬路由。
> 它们是你在 LTO harness 里选择 primitive 的思考框架。

## 架构哲学

LTO 是 **A harness for every task**：它给主 agent 一套任务操作系统，而不是菜单式
执行器。

分层如下：

| 层 | 责任 | 不做什么 |
|---|---|---|
| Host agent | 读目标、判断路径、拆 task、决定何时 fan-out / adversarial / linear / 停下来问人 | 不把判断外包给固定 preset |
| LTO harness | 保存 state、登记 artifacts、跑 runner/audit、隔离 worktree、恢复上下文、提供 gate | 不接管 planner 角色 |
| Primitive | `runner` / `audit` / `judge` / `next` / `autopilot` / `recap` 等可组合动作 | 不伪装成完整业务流程 |
| Human gate | irreversible action、phase transition、closeout、语义争议 | 不被自动化吞掉 |

优雅标准：新增能力必须扩大 host agent 的行动空间和证据质量。把模型本该判断的
路径提前固化成菜单、枚举或 schema，是倒退。

## 通用调度循环

每次进入长任务，host agent 先按这七步走：

1. **读状态**：`lto resume` / `lto check` / `lto recap`。
2. **识别任务形态**：是 review、debug、migration、claim-verify、research，还是普通 linear work。
3. **开发前四证据**：写码或调优前先落 architecture_alignment、first_principles、
   simplification_dedupe、value_measurement。调优必须有 baseline、指标、及格线和复测命令。
4. **选择 primitive**：只选下一段最小可验证动作，不一次性承诺整条 workflow。
5. **落证据**：每个动作必须回写 state / artifact / evidence / ledger。
6. **判断是否升级**：遇到高风险、歧义、不可逆动作、长时间停滞时，升级到 adversarial review 或 human gate。
7. **收尾四证据**：closeout/release/handoff 前补齐 documentation_alignment、
   historical_cleanup、clean_worktree、rebuild_package；先 clean，再从最终状态重新 build/package。

`lto next` 只提供事实简报和无歧义命令；最终 pattern 决策仍由 host agent 做。

### 开发/调优前置闸门

进入 implementation 或 optimization 前，host agent 需要把下面四项写进
run-state、task evidence 或等价 artifact：

- **Architecture alignment**：当前改动属于哪一层、遵守哪些模块边界、复用哪些已有模式；若偏离现有架构，写清理由。
- **First-principles reason**：从真实约束、用户价值、故障根因推导为什么要做，不用“以后可能需要”当理由。
- **Simplification / dedupe**：先检查能否删除旧逻辑、合并重复分支、复用现有 helper/API；新增抽象必须减少真实复杂度。
- **Value measurement**：调优必须先有 baseline、指标、及格线和复测命令；没有复测数据的调优只算假设，不能 closeout。

### 收尾/发布前置闸门

进入 closeout、release 或长期 handoff 前，host agent 需要把下面四项写进
run-state、task evidence 或等价 artifact：

- **Documentation alignment**：检查并同步 `SKILL.md`、`README.md`、`INSTALL.md`、`AGENTS.md`、`CLAUDE.md`、相关 `references/` 与 changelog；文档不能描述过时架构。
- **Historical cleanup**：清理、归档或显式标注旧入口、旧路径、旧 run、兼容期说明和过时 TODO；不能让历史材料冒充当前指引。
- **Clean worktree**：closeout/打包前 `git status --short` 为 clean；若故意保留 dirt，逐项命名、说明理由并取得 human gate。
- **Rebuild package**：仓库进入最终状态后重新编译/打包，并记录命令、版本、产物位置和结果。先 build 再改文档不算最终复测。

## Playbooks

> 其中多个场景已有配套的 data-only 场景插件（`plugins/` 下，合同见
> `plugin-boundary.md`）：`adversarial-audit`（review 的审计编队先验）、
> `claim-verify-research`（claim-verify / research 的核验先验）、
> `migration-refactor`（migration 的分批闸门先验）、
> `dev-workflow`（feature-dev / docs-sync / direction-review 的全链路先验，
> 设计依据见 `dev-workflow-spec.md`；含 `enterprise-audit` 的十层红线门禁先验）。插件提供 prompt /
> profile / path / eval 素材，**不替你选路**——读完本节再决定挂不挂。


| playbook |
|---|
| [review](playbooks/review.md) |
| [enterprise-audit](playbooks/enterprise-audit.md) |
| [debug](playbooks/debug.md) |
| [migration](playbooks/migration.md) |
| [claim-verify](playbooks/claim-verify.md) |
| [research](playbooks/research.md) |
| [feature-dev](playbooks/feature-dev.md) |
| [tmux-goal-loop](playbooks/tmux-goal-loop.md) |
| [docs-sync](playbooks/docs-sync.md) |
| [release](playbooks/release.md) |
| [direction-review](playbooks/direction-review.md) |

### review

已迁至 [playbooks/review.md](playbooks/review.md)。

### enterprise-audit

已迁至 [playbooks/enterprise-audit.md](playbooks/enterprise-audit.md)。

### debug

已迁至 [playbooks/debug.md](playbooks/debug.md)。

### migration

已迁至 [playbooks/migration.md](playbooks/migration.md)。

### claim-verify

已迁至 [playbooks/claim-verify.md](playbooks/claim-verify.md)。

### research

已迁至 [playbooks/research.md](playbooks/research.md)。

### feature-dev

已迁至 [playbooks/feature-dev.md](playbooks/feature-dev.md)。

### tmux-goal-loop

已迁至 [playbooks/tmux-goal-loop.md](playbooks/tmux-goal-loop.md)。

### docs-sync

已迁至 [playbooks/docs-sync.md](playbooks/docs-sync.md)。

### release

已迁至 [playbooks/release.md](playbooks/release.md)。

### direction-review

已迁至 [playbooks/direction-review.md](playbooks/direction-review.md)。

## 何时可以抽 CLI

只有同时满足这些条件，才考虑把某条 playbook 抽成最薄命令：

1. host agent 已经多次稳定选择同一路径；
2. 输入、输出、artifact 和停止条件自然沉淀；
3. 新命令只减少机械摩擦，不替 host agent 做语义判断；
4. human gate 和 evidence contract 不被削弱；
5. 失败时能清楚降级回人工/host-agent 判断。

不满足时，继续改 playbook、prompt contract 或 harness primitive。
