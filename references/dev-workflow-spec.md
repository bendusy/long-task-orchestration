# dev-workflow 插件 spec（草案 v1，待三方异构审）

> 2026-06-10。状态：**草案**，实现前须经 codex / pi / agy 三方异构对抗审收敛。
> 证据基础：对宿主用户 5 个真实项目、60+ 个开发 session 日志的并行挖掘
> （挖掘档为私有，本 spec 是脱敏后的公开版本——只保留模式，不含项目名与对话原文）。

## 0. 一句话

把「从一个想法到一次定版发布」的完整开发链路沉淀为 data-only 场景插件
`plugins/dev-workflow`，并同步补齐 workflow-playbook 的四个盲区章节、
修掉三个已有场景插件的已知缺口。插件仍遵守 plugin-boundary v0：
**只提供 path / profile / prompt / eval 素材，不替宿主选路。**

## 1. 动机：现有 playbook 都假设「任务已成形」

workflow-playbook 现有五节（review / debug / migration / claim-verify /
research）覆盖的都是**任务中段**的形态。但日志挖掘还原出的真实高频链路是
一条更长的链：

```
[A] 概念讨论 / 设计决策（动机先于需求）
[B] spec 先行（大改动前必经）
[C] spec 的异构 co-design 审（三方跨族复核）
[D] 综合订正出 spec v2（v1 + 订正 + 一次过审议）
[E] 派工实现（子代理写码，host 规划/审计；派工后不再问人）
[F] 实现层异构对抗审（findings union 合并，不投票，一条不漏）
[G] 逐 blocker 修复收敛（HIGH/CRITICAL 单调下降，2-4 轮）
[H] test-pin（对抗审提到的不变量必须落成回归测试）
[I] 验收闸门（五条同时满足，见 §3.3）
[J] 沉淀 / 同步（changelog + 文档对齐 + 经验入库 + handoff）
[K] 定版 / 发布（版本号 + changelog 定版 + 隐私自检 + push 人工确认）
```

这条链在日志中以高频出现，但 LTO 没有任何 playbook / 插件覆盖 A-D 与 I-K。
五条现有 playbook 只覆盖 E-G 的局部。这是最大的盲区。

### 1.1 隐性规则（从日志反复出现的纠偏中提炼，插件必须编码）

1. **模型不固化**：任何写死模型名/runner 名的设计都会被否。profile 声明
   能力与族别，不锁定具体型号。
2. **Verifier > Planner**：实物核验优先于计划讨论。exit code 0 不单独作数，
   产物必须实读。
3. **派工后不问人**：要问的在派工前问清，prompt 里给全停止条件。
4. **沉淀是交付动作**：经验入库与 push 同级，不是可选项。
5. **联网查证优先于模型记忆**：涉最新 API / 版本 / 价格的 claim 必真查。
6. **commit 当检查点**：每个收敛点 commit，防跑偏无法回档。

### 1.2 观测性：跨项目成立的横切规律

对 5 个项目的横向核验（含 LTO 自身）全部命中同一模式：

- **日志是事实源**（JSONL 结构化日志 / events 流）；
- **doctor / healthcheck 是入口**（排障第一命令）;
- **告警是最晚的一层**（已萌芽未普及，本期不做）。

且观测层总是**功能完成之后**刻意排期补齐（P1 任务），不是同步开发、
也不是遗忘。因此 dev-workflow 把观测性编码为 feature-dev path 里
**功能完成后的标准横切 step**，带三件套验收形态：

1. 结构化日志 schema（machine-parseable，append-only）；
2. doctor / healthcheck 入口（一条命令看健康）；
3. 排障命令（fails / recent / stats 类查询入口）。

## 2. 范围

| 块 | 内容 | 产物 |
|---|---|---|
| W1 | `plugins/dev-workflow` 插件 | plugin.json + paths + profiles + prompts + schemas + eval |
| W2 | workflow-playbook 四个新节 | feature-dev / docs-sync / release / direction-review |
| W3 | 三个已有插件缺口修复 | adversarial-audit / claim-verify-research / migration-refactor |

非目标（显式排除）：

- 告警系统（萌芽阶段，留 backlog）；
- 任何新 CLI 命令（不满足「何时可以抽 CLI」五条件）；
- 自动 promotion（eval-run 维持 deferred: automatic_promotion）；
- 把 A-K 链做成硬路由 / 状态机（违背 harness-first；它是调度先验，
  host agent 可在任何点进入、跳过、退出）。

## 3. W1：dev-workflow 插件设计

### 3.1 plugin.json

```json
{
  "id": "dev-workflow",
  "kind": "path-plugin",
  "stage": "experimental",
  "default_enabled": false,
  "executable_code": false,
  "permission": { "max_sandbox": "read-only", "env_allowlist": [] }
}
```

与其余三个插件同合同：data-only、默认不挂载、零环境变量。

### 3.2 paths/

**`feature-dev-main.json`**：A-K 全链路主路径。关键设计点：

- 每个阶段是「step + 期望 artifact + 停止条件 + 可选升级点」，
  不是必经状态机。host agent 声明从哪个阶段进入。
- C（spec co-design）与 F（实现对抗审）两个 step 注明
  「可挂 `adversarial-audit` 插件复用 refuter profile」——**注明而非依赖**
  （plugin-boundary v0 无插件间依赖，宿主自行决定挂不挂）。
- E（派工实现）注明 worktree_exec 沙箱与「派工后不问人」契约。
- H（test-pin）作为 F→G 收敛的出口条件：对抗审 findings 中的不变量
  没落成回归测试，不算收敛。
- I（验收闸门）引用 `prompts/acceptance-gate.md`。
- I 与 J 之间插入**观测性横切 step**（§1.2 三件套），标记为
  「功能完成后的 P1 任务，新功能模块默认排期，小修可跳过但须显式声明」。
- K（release）引用 `references/privacy-self-check.md`（该文档已存在但
  从未被任何路径接线——本插件是第一个消费者）；push 永远 NEEDS_CONFIRM。

**`docs-sync-loop.json`**：文档对齐独立路径（非 review 非 debug）：
fan-out 多路审计扫 doc-vs-code drift → union 合并 → 逐条实读修复 →
防 drift test-pin（改代码不同步文档即测试红）。

**`direction-review-vote.json`**：方向决策路径。与 findings 审计的
本质区别：**findings 用 union 合并（一条不漏），方向决策用 2/3 票 +
host 复核**（产物是决策档不是 findings JSON）。路径要编码这条分流规则，
和「分歧时升级人类」的出口。

### 3.3 prompts/

**`acceptance-gate.md`**（五条验收闸门 checklist，host agent 在 I 阶段自查）：

1. 脚本全绿：项目自有验证脚本（registry / lint / CI 类）全部通过；
2. 实物读验：关键产物实读核验，exit code 0 不单独作数；
3. 对抗审收敛：findings union 处理完，无遗留 blocker / high；
4. 文档同步：README / changelog / 接口文档与代码对齐；
5. 经验入库：本次踩坑与决策已沉淀（记忆系统 / 决策档 / handoff 任一）。

五条**同时满足**才算 done；任何一条豁免须显式记录理由。

**`observability-module.md`**：三件套 checklist（§1.2）+ 验收形态
（日志能被机器 parse、doctor 一条命令、排障查询能答「最近失败了什么」）。

**`spec-codesign.md`**：spec 审 prompt（给异构 runner）。要点：refute-first、
对照仓库现状实读验证（防脑补）、产出结构化 findings、区分
「spec 错误 / spec 缺失 / 方向异议」三类（前两类 union 处理，
第三类走 direction-review 票决）。

所有 prompt 末尾带 no-preamble JSON 硬约束（与现有 8 个 prompt 同款）。

### 3.4 profiles/

最小化：**只加 2 个**，其余复用场景（adversarial-audit 的 refuter）由
path 注明：

- `spec-refuter-heterogeneous-v1.json`：spec co-design 审计方，
  约束「与宿主跨族」（不锁 runner 名，由宿主挑选时满足跨族约束），
  read-only，挂 `spec-codesign.md`。
- `docs-drift-auditor-v1.json`：docs-sync 扫描方，read-only。

**沙箱注记（吸收 agy 实测教训）**：profile 若声明 read-only，必须附
`enforcement_note` 说明该声明在哪些 runner 上可真实兑现——scheduler 对
无法兑现 read-only 的 runner（如 agy 只有 workspace-write）会 fail-closed
拒绝派工，这是预期行为不是 bug。

### 3.5 eval/

eval pack 与现有三插件同合同：

- metrics 必含 `permission_violations` + `private_path_leaks`；
- `safety_regressions_allowed: 0`；`minimum_runs_before_promotion: 5`；
- cases（read-only，全部以本仓库公开文件为 fixture）：
  1. `feature-dev-path-playbook-consistency`：审 feature-dev path 与
     playbook 新节是否自洽（runner: codex）；
  2. `acceptance-gate-completeness`：用 acceptance-gate prompt 审一个
     刻意缺第 4/5 条的虚构收尾陈述，必须抓出缺口（runner: pi）；
  3. `observability-checklist-applicability`：用三件套 checklist 审
     LTO 自身（events.jsonl / telemetry / 排障命令），输出 gap
     （runner: claude，dogfooding）；
  4. `direction-vote-vs-union-discrimination`：给混合 findings + 方向异议
     的样本，审 prompt 能否正确分流（runner: codex）。

## 4. W2：workflow-playbook 四个新节

按现有五节同构（触发信号 / 可用 primitive / 期望 artifact / 停止条件 /
反模式），各节要点：

**feature-dev**：触发=新需求/新功能从零开始；primitive 全链（task-add /
runner / audit / worktree_exec / judge / closeout + dev-workflow 插件）；
停止条件=五条验收闸门；反模式=跳过 spec 直接写码、自审代替异构审、
test-pin 缺位、把「实现完」当「做完」。
观测性接线：本节写明 telemetry / live log / interventions 的**查看触发条件**
（何时看什么——修掉「有工具无路径」盲区）。

**docs-sync**：触发=代码大改后 / 周期性 drift 审计 / 用户指出文档过时；
反模式=只改 README 不查全部引用、修文档不加防 drift test。

**release**：触发=版本定版 / 对外 push / 公开仓库同步；primitive 含
privacy-self-check + 敏感扫描 + changelog 定版；停止条件=人工确认 push；
反模式=push 与沉淀脱节、版本号无 changelog 对应、私有内容混入公开仓
（gitignore + 敏感扫描双防线）。

**direction-review**：触发=架构边界判断 / 两个都对但只能选一的方案分歧；
与 review 的区别=票决而非 union；停止条件=2/3 共识 + host 复核落决策档，
或升级人类；反模式=用 findings 流程处理方向分歧（永不收敛）、
让同族模型投三票。

另：现有 review 节补一句「building 阶段的中途 verification 也适用本节」
（边建边验，不等到收尾才审）。

## 5. W3：三个已有插件缺口修复

**adversarial-audit**：
- 新增 `claude-refuter-v1.json`（第四审计方；注明宿主为 claude 时禁用——
  同族自审）；
- agy profile 加 `enforcement_note`：agy 无法兑现 read-only，scheduler
  会 fail-closed 拒绝（2026-06-10 实测）；read-only 审计任务勿派 agy；
- agy prompt 加幻觉警告：禁止假设「用户已批准」类对话状态；
- path 加方向分歧分流出口：findings 矛盾→host 实读裁决；方向分歧→
  转 direction-review 票决路径。

**claim-verify-research**：
- path 加显式 step：**本地代码 claim 必须实读源码验证**（LLM 断言
  不是证据，evidence 必须是 path:line / 命令输出）；
- sources 注明与 docs-sync 的边界：核验对象是「对外 claim」，
  文档与代码的 drift 修复走 docs-sync。

**migration-refactor**：
- prompt/path 加 minimal exemplar 选点指引（选最小但覆盖全部变换模式的
  样例，列出判断维度）；
- `claude-semantic-equivalence-v1` 加同族冲突注记：宿主为 claude 时
  换异构 runner 执行该 profile；
- path 的 merge step 加 rollback primitive 序列（冲突→停止→保留现场→
  worktree 不删→host 决定 rebase / 弃批重跑，给出具体 git 命令序列）。

## 6. 验收标准（本 spec 自身的 done 定义）

1. `lto plugin validate` 全绿（4 插件）+ 静态 eval 全绿；
2. eval-run 实测：dev-workflow 4 case 全 `ok=true`、candidate
   `parse_ok=true`、零新增 `private_path_leak` / `permission_violation`；
3. 三方异构 spec 审收敛（findings union 处理完，无遗留 blocker/high）；
4. workflow-playbook / README / CHANGELOG 同步；
5. 公开内容敏感扫描通过（无私有项目名 / 对话原文 / 私有路径）。

——即用五条验收闸门验收「验收闸门」自身（dogfooding）。

## 7. 开放问题（请三方审重点对抗）

1. feature-dev path 的 A-K 粒度是否过细？哪些阶段该合并成一个 step？
2. 观测性 step 放 I 与 J 之间是否正确？还是该作为 J（沉淀）的子项？
3. direction-review 的 2/3 票制：三方都是 LLM 时，票决对「判断品味」类
   分歧是否真有效？是否应该默认直接升级人类？
4. `spec-refuter-heterogeneous-v1` 的「跨族约束不锁 runner」在 profile
   schema 里如何表达才可被 scheduler 机读检查？
5. eval case 3（用 checklist 审 LTO 自身）会不会把 eval 变成
   「自己给自己打分」？需不需要换 fixture？
