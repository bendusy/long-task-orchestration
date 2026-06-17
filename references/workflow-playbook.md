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

### review

触发信号：

- spec、代码路径、设计决策或任务涉及 auth、payment、migration、schema、security、concurrency、external API。
- 你对自己方案有明显路径依赖，或者同一模型已经连续多轮自审。
- closeout 前存在未审 high-risk task。

可用 primitive：

- `lto audit --auto-dispatch`
- `lto audit --discover-risks`
- `lto collect-agent-run --task-id <id> --runner <runner> --reply <reply.md>` for manually produced replies
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

注：building 阶段的中途 verification 也适用本节——边建边验，不等到收尾才审。

### enterprise-audit

> 高风险变更的分层审计门禁。十层（requirements → architecture →
> data-model → interface-contract → implementation → testing →
> operations-observability → security → migration-rollback → acceptance）
> 是覆盖模型，不是每个小改动都要跑的委员会。

触发信号：

- 变更涉及 schema、API contract、auth/security、migration/rollback、并发、
  新模块、对外发布或生产运维面。
- 用户要求“大厂标准”“全流程审计”“Bar Raiser / 架构评审委员会式”覆盖。
- 你发现普通 implementation review 无法覆盖需求、上线、回滚、验收等上下游风险。

可用 primitive：

- `lto plugin mount plugins/dev-workflow` 后读
  `paths/enterprise-audit-gate.json` 做 scope triage。
- 用 `profiles/enterprise-layer-auditor-v1.json` 给 codex/pi/agy/claude 等
  非同族 runner 派读-only layer audit；高风险默认至少 3 个 distinct families。
- `collect-agent-run` / artifact evidence / audit ledger 做 union 收口；host 逐条核
  path:line 或命令输出。
- 有方向争议时转 `direction-review`；有实现 blocker 时回到 `review` /
  `migration` / `feature-dev` 对应修复循环。

期望 artifact：

- layer scope matrix（十层 mandatory/exempt + 理由）。
- per-layer structured findings JSON（含 layer/redline/severity/evidence）。
- redline register 与 host triage record。
- test-pin / contract check / rollback evidence / acceptance record。

停止条件：

- 每个 mandatory layer 都有异构只读审计证据或记录了 dispatch failure。
- HIGH/CRITICAL redline 为 0，或人类显式 override 并记录 residual risk。
- 每条采纳/否决都有一手证据；每个豁免都有理由。

反模式：

- 小改动无脑跑十层，制造仪式成本。
- 单个模型审全部层后宣称“独立审计”。
- 用多数票丢掉某个 runner 提出的 redline。
- 把 exit code 0 当成 acceptance，而没有读产物、契约、回滚和人类 gate。

### debug

触发信号：

- task blocked，且有失败命令、stderr、日志、截图或用户可复现步骤。
- 同一失败指纹反复出现。
- 你有多个假设但没有证据排序。

可用 primitive：

- `lto runner --task-id <id> --kind test --command "..."`
- `lto next`
- `lto autopilot --supervised`
- 必要时手动或通过 repo 自带 `scripts/delegate/` fan-out 多个独立诊断假设。

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

- `lto task add` 拆 slice
- `lto runner` 逐 slice 落验证证据
- `lto run parallel` / `lto run pipeline` 跑批量 shell 验证
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

### feature-dev

> 新需求从零到定版的全链路。六阶段（specify → dispatch → impl-audit →
> converge → acceptance → release）是调度先验不是状态机，可在任何阶段
> 进入、跳过、退出。设计依据与验收闸门定义见 `dev-workflow-spec.md`。

触发信号：

- 新需求 / 新功能从零开始；改动会产生新模块或新的对外行为。
- 你发现自己想跳过 spec 直接写码。

可用 primitive：

- `lto start --goal/--why/--done-when` 记录目标与完成标准。
- `lto task add` 按阶段拆 task。
- 开发前补齐四证据：架构对齐、第一性原理、精简去重、价值测评。
- specify 阶段挂 `plugins/dev-workflow`（spec co-design 审可复用
  `plugins/adversarial-audit` 的 refuter profile）。
- `lto runner` 落实现证据；`lto audit --auto-dispatch` 做 impl-audit。
- worktree_exec 在 dispatch 阶段隔离写入（specify 全程 read-only，
  spec 收口后才开 worktree）。
- `lto judge` / `lto closeout`。
- 观测性查看触发条件：派工后看 `.lto/<run-id>/live/` 实时日志；收敛
  卡壳看 `events.jsonl` 与 telemetry；做完复盘看 interventions 记录与
  `lto recap --mine`。

期望 artifact：

- spec v1 与 v2（含异构审订正记录）
- architecture alignment / first-principles / simplification-dedupe / value-measurement note
- documentation alignment / historical cleanup / clean worktree / rebuild-package note
- worktree 分支与 per-task evidence
- findings union register
- test-pin 测试文件
- 验收闸门六条自查记录
- changelog entry

停止条件：

- 验收闸门六条同时满足：脚本全绿 / 实物读验 / 对抗审收敛 / 文档同步 /
  经验入库 / 可观测（新功能模块三件套：结构化日志 schema、doctor 入口、
  排障命令）。任何一条豁免须显式记录理由。
- 调优类改动必须给出 baseline 与复测结果；精简类改动必须说明删减/复用的代码路径。
- 收尾类改动必须证明文档口径一致、历史残留已处理、仓库 clean，并从最终状态重新打包/编译。

反模式：

- 跳过 spec 直接写码。
- 不对齐架构、不做第一性推导，直接堆实现。
- 把没有 baseline/复测的“调优”当优化成果。
- 自审代替异构审。
- 对抗审提到的不变量不落回归测试（test-pin 缺位）。
- 把「实现完」当「做完」。
- 先打包后继续改文件，或带着未解释的 dirty worktree 宣称完成。
- 观测性永远滞留 backlog。

### tmux-goal-loop

> Host 合议 goal → tmux runner 短会话 loop 长跑 → 异构审计 → **host 亲验硬停止点**。
> 这是 repo 内 Rust tmux runner 落地后的闭环 playbook，不是新 CLI，不依赖私有
> `tmux-autopilot` skill。

触发信号：

- 用户给出一个足够大的 goal，需要 host 先合议目标，再派 coding worker 长跑若干轮。
- 单个 headless runner 一发一收无法承载交互式长跑，或单长会话容易 context 膨胀、过早自报完成。
- 已有 `state.tasks` 可拆成短 worker 任务，且每个 worker 的完成能用 evidence / contract 证伪。

可用 primitive：

- `lto start --goal ... --target ... --constraint ... --instrument ... --entropy-check ...`
  记录 goal 四件套。
- `lto task add` 写 feature/task 清单；host 保留拆分和优先级判断权。
- `lto runner --runner tmux --tmux-mode signal|sentinel|fire ...` 直接派可观测 worker，
  或 `lto autopilot --auto-exec --worker-runner tmux` 让现有 autopilot loop
  顺序派一个 bounded worker per pending task。
- `lto audit --auto-dispatch --discover-risks` 做 fresh-context 异构审计；
  runner 输出失败或跑偏时，host 读 live log / reply artifact 后逐条采纳或驳回。
- `lto check --to closed --strict` 和 `lto closeout` 做证据闸门和 handoff。

期望 artifact：

- goal 四件套和 task 清单。
- 每个 worker 的 live log、completion contract 或 runner evidence。
- host triage note：worker 自述了什么、host 一手验了什么、哪些自述被驳回。
- audit replies / audit ledger / redline register。
- host 亲验记录：测试命令、grep/文件读验、产物对比、残余风险。
- changelog / handoff。

Host 亲验硬停止点：

loop 跑完、blocked 或 worker 自报完成后，**不得**把 hook 返回、pane 停止、contract
存在或 agent 文字自述直接当完成。host 或独立 evaluator 必须先做一手核验，至少覆盖：

1. 跑项目自己的红线命令；失败输出必须登记为 evidence。
2. 对照 goal/task 清单逐条打开关键产物或源码，确认 worker 自述和实际 diff 一致。
3. 用 `rg` / 文件读取 / manifest 检查确认没有漏改、错 repo、私有依赖或历史入口冒充当前入口。
4. 对 worker 报告的“全绿”“已完成”“无风险”逐条找一手证据；找不到证据就按未完成处理。
5. 运行 `lto check --to closed --strict`；done task 没有 evidence 时默认 FAIL，不能 closeout。

停止条件：

- 所有 task 为 done/skipped，且 done task 都带 evidence。
- high/critical audit blocker 收敛到 0；采纳/驳回都有 path:line、命令输出或 artifact 证据。
- host 亲验清单完成并登记；如果有人类 override，残余风险写入 handoff。
- `git status --short` 干净；最终状态重新跑红线。

反模式：

- 让一个 worker 啃整个大 goal，并把它的自述当验收。
- loop 完成后直接 closeout，不读 diff、不跑测试、不看 artifact。
- 把 `tmux` pane 停止、sentinel 文件、contract 文件存在等同于语义完成。
- 把这个 playbook 抽成替 host 做判断的 `orchestrate` 命令。
- 依赖 host 侧私有 skill 或本机隐藏脚本，导致 stranger 无法复现。

### docs-sync

> 文档与代码对齐是独立任务形态——既不是 review 也不是 debug。

触发信号：

- 代码大改后；周期性 drift 审计。
- 用户指出文档过时；changelog 与文档口径不一致。

可用 primitive：

- fan-out 多路审计扫 doc-vs-code drift（可挂 `plugins/dev-workflow` 的
  `docs-drift-auditor-v1`）。
- union 合并 findings；`lto runner --kind manual` 登记逐条修复证据。
- 防 drift test-pin：从源码动态抽阈值/命令名，断言文档同值——改了代码
  不同步文档即测试红。

期望 artifact：

- drift findings union 清单（命中 `drift-ok` 有意分歧注记的条目标
  `intentional`，不算 drift）
- 逐条修复 diff
- 防 drift 测试

停止条件：

- union 清单逐条处理完（修复或标 intentional）。
- 防 drift 测试落地并通过。

反模式：

- 只改 README 不查全部引用。
- 修文档不加防 drift test。
- 把有意分歧（ADR / 未来架构描述）当 drift 修掉。

### release

> 定版与对外发布。push 永远是人类闸门。

触发信号：

- 版本定版；对外 push；公开仓库同步；向他人交付。

可用 primitive：

- changelog 定版（版本号与条目对应）。
- `bash scripts/privacy_self_check.sh --repo . --strict`（gitleaks 不可用
  时加 `--no-gitleaks` 并在 run state 显式记录降级——dry-run 默认 exit 0，
  不能冒充 strict 通过）。
- 敏感扫描（私有项目名 / 内部路径 / 对话原文）。
- `lto closeout --summary`；push 前 human gate。

期望 artifact：

- 版本号对应的 changelog 段
- 隐私自检输出
- closeout handoff
- push 确认记录

停止条件：

- 隐私自检 strict 通过（或降级被显式记录且人类接受）。
- 人工确认 push。
- 沉淀完成（验收闸门第 5 条在 release 复查）。

反模式：

- push 与沉淀脱节。
- 版本号无 changelog 对应。
- 私有内容混入公开仓（gitignore + 敏感扫描双防线）。
- 用 dry-run 的 exit 0 冒充 strict 通过。

### direction-review

> 方向 / 品味分歧与 findings 审计本质不同：findings 用 union 合并
> （一条不漏），方向分歧默认升级人类——票决只是受限工具。

触发信号：

- 架构边界判断；两个都对但只能选一的方案分歧。
- 审计方之间出现非事实性矛盾。

可用 primitive：

- 分歧分类：先判定是「证据可裁决」（有 path:line / 命令输出 / 官方文档
  可核）还是「品味/方向」（无独立证据可裁决）。
- 证据可裁决 → 派异构核验，按证据裁决（不投票）。
- 品味/方向 → 升级人类；异构意见仅作为 advisory 证据附上。
- 决策档落 decision log 类位置（见 `decision-logging.md`）。

期望 artifact：

- 分歧分类记录
- 各方立场与证据
- 决策档（含最终裁决与理由）

停止条件：

- 证据分歧被证据裁决；品味分歧由人类拍板并落档。
- 任一审计方给出 needs_human 即直接升级，不被多数票否决；
  2/3 票仅在人类显式授权「按多数走」时使用。

反模式：

- 用 findings union 流程处理方向分歧（永不收敛）。
- 让同族模型投三票。
- 用 2/3 票否决 needs_human。
- 票决品味问题。

## 何时可以抽 CLI

只有同时满足这些条件，才考虑把某条 playbook 抽成最薄命令：

1. host agent 已经多次稳定选择同一路径；
2. 输入、输出、artifact 和停止条件自然沉淀；
3. 新命令只减少机械摩擦，不替 host agent 做语义判断；
4. human gate 和 evidence contract 不被削弱；
5. 失败时能清楚降级回人工/host-agent 判断。

不满足时，继续改 playbook、prompt contract 或 harness primitive。
