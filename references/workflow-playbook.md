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

每次进入长任务，host agent 先按这五步走：

1. **读状态**：`lto resume` / `lto check` / `lto recap`。
2. **识别任务形态**：是 review、debug、migration、claim-verify、research，还是普通 linear work。
3. **选择 primitive**：只选下一段最小可验证动作，不一次性承诺整条 workflow。
4. **落证据**：每个动作必须回写 state / artifact / evidence / ledger。
5. **判断是否升级**：遇到高风险、歧义、不可逆动作、长时间停滞时，升级到 adversarial review 或 human gate。

`lto next` 只提供事实简报和无歧义命令；最终 pattern 决策仍由 host agent 做。

## Playbooks

> 其中三个场景已有配套的 data-only 场景插件（`plugins/` 下，合同见
> `plugin-boundary.md`）：`adversarial-audit`（review 的审计编队先验）、
> `claim-verify-research`（claim-verify / research 的核验先验）、
> `migration-refactor`（migration 的分批闸门先验）。插件提供 prompt /
> profile / path / eval 素材，**不替你选路**——读完本节再决定挂不挂。

### review

触发信号：

- spec、代码路径、设计决策或任务涉及 auth、payment、migration、schema、security、concurrency、external API。
- 你对自己方案有明显路径依赖，或者同一模型已经连续多轮自审。
- closeout 前存在未审 high-risk task。

可用 primitive：

- `lto audit --auto-dispatch`
- `lto audit --discover-risks`
- `lto audit --collect <reply-dir>`
- `lto check --to implementation|closed`
- `lto judge --phase <phase>`
- 场景插件 `plugins/adversarial-audit`：refute-first prompt、codex/pi/agy 跨族 profile、union 合并收敛路径

期望 artifact：

- audit brief
- heterogeneous replies
- structured findings JSON
- audit ledger
- judge verdict

停止条件：

- high/critical blocker 收敛到 0，或 human 明确 override 并记录理由。
- 每条采纳/否决都有源码、命令、截图或文档证据。

反模式：

- 让同一家 runtime 自审。
- 只看“三个 agent 都说没问题”，不核源码和证据。
- 把 `review` 做成一键通过闸门。

### debug

触发信号：

- task blocked，且有失败命令、stderr、日志、截图或用户可复现步骤。
- 同一失败指纹反复出现。
- 你有多个假设但没有证据排序。

可用 primitive：

- `lto runner --task-id <id> --kind test --command "..."`
- `lto next`
- `lto autopilot --supervised`
- 必要时手动或通过 agent-delegate fan-out 多个独立诊断假设。

期望 artifact：

- 最小复现命令
- stdout/stderr evidence
- 假设列表和排除理由
- 修复后通过的验证命令

停止条件：

- 一个根因被证据支持，并且修复后验证通过。
- 或所有合理假设都被排除，留下明确的 next diagnostic / human question。

反模式：

- 没有复现就改代码。
- 并发派多个 agent 改同一片文件。
- 把一次通过当成根因证明，不记录失败假设。

### migration

触发信号：

- 任务跨多个模块、schema、API contract、shared state 或持久化格式。
- 需要兼容期、rollback、分片合并或批量重命名。
- 单 context 很容易丢 dependency / ordering / blast radius。

可用 primitive：

- `lto task-add` 拆 slice
- `lto runner` 逐 slice 落验证证据
- `lto parallel` / `lto pipeline` 跑批量 shell 验证
- `lto audit --auto-dispatch` 做 adversarial review
- `lto check --to closed --strict`
- 场景插件 `plugins/migration-refactor`：最小样例先行 + 批间回归闸门路径、diff 审计 / 语义等价 profile

期望 artifact：

- migration plan
- slice task list
- per-slice evidence
- compatibility / rollback notes
- audit ledger
- changelog / handoff

停止条件：

- 每个 slice done/skipped 且理由清楚。
- 兼容性、回滚、安全和测试证据都当前于 HEAD。
- adversarial review 收敛，human gate 批准不可逆步骤。

反模式：

- 先抽象再找问题。
- 没有 touched_files / evidence 就 closeout。
- 把 rollback 写成一句话，不验证可执行性。

### claim-verify

触发信号：

- 文档、spec、研究输出、对外文章中有事实、版本、引用、价格、API 行为、法律/政策、技术能力声明。
- 错一个 claim 会影响决策或对外发布。

可用 primitive：

- host agent 自行抽 claim table。
- 对稳定事实用本地源码/文档验证；对时效性事实用 web/context7/官方文档验证。
- 可用 `lto runner --kind manual` 登记核验证据。
- 高风险时走 `audit --auto-dispatch` 做 source adversarial review。
- 场景插件 `plugins/claim-verify-research`：claim 拆解 / 证据反驳 / completeness critic 三类 profile 与主路径。

期望 artifact：

- claim ledger
- source/evidence map
- supported / refuted / unknown verdict
- unresolved claims list

停止条件：

- 每个 material claim 都有 supported/refuted/unknown。
- unknown 不被改写成确定口吻。
- 对外发布前 human gate 批准残余不确定性。

反模式：

- 用“看起来像”替代来源。
- 让 LTO 编造 source artifact。
- 把 research synthesis 当 verification。

### research

触发信号：

- 用户要多源研究、路线比较、技术选型、市场/生态判断，且答案不应只依赖单一来源。
- 需要记录 coverage、矛盾、置信度和待验证点。

可用 primitive：

- host agent 分源检索和摘录。
- 必要时 fan-out 不同角度研究，再 synthesis。
- `lto runner --kind manual` 登记关键证据。
- `audit --auto-dispatch` 用于 adversarial source critique。
- 场景插件 `plugins/claim-verify-research` 同样适用本场景（fan-out 检索 + completeness critic）。

期望 artifact：

- source notes
- contradiction ledger
- synthesis memo
- confidence labels
- open questions

停止条件：

- 关键来源覆盖达标。
- 重大矛盾被解决或显式披露。
- 结论区分 fact、inference、recommendation。

反模式：

- 把搜索结果堆叠成结论。
- 不标明推断。
- 为了“完整”继续无界检索，不回到任务目标。

## 何时可以抽 CLI

只有同时满足这些条件，才考虑把某条 playbook 抽成最薄命令：

1. host agent 已经多次稳定选择同一路径；
2. 输入、输出、artifact 和停止条件自然沉淀；
3. 新命令只减少机械摩擦，不替 host agent 做语义判断；
4. human gate 和 evidence contract 不被削弱；
5. 失败时能清楚降级回人工/host-agent 判断。

不满足时，继续改 playbook、prompt contract 或 harness primitive。
