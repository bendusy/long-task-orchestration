# dev-workflow 插件 spec（v2，三方异构审后订正）

> 2026-06-10。状态：**v2**——v1 经 codex / pi / agy 三方 refute-first 审
> （verdict: block / block / revise），findings union 合并逐条核验后订正。
> 审计记录见 §9。
> 证据基础：对宿主用户 5 个真实项目、60+ 个开发 session 日志的并行挖掘
> （挖掘档为私有，本 spec 是脱敏后的公开版本——只保留模式，不含项目名与对话原文）。

## 0. 一句话

把「从一个想法到一次定版发布」的完整开发链路沉淀为 data-only 场景插件
`plugins/dev-workflow`，并同步补齐 workflow-playbook 的四个盲区章节、
修掉三个已有场景插件的已知缺口、修一个审稿审出的 scheduler 真洞。
插件仍遵守 plugin-boundary v0：**只提供 path / profile / prompt / eval 素材，
不替宿主选路。**

## 1. 动机：现有 playbook 都假设「任务已成形」

workflow-playbook 现有五节（review / debug / migration / claim-verify /
research）覆盖的都是**任务中段**的形态。日志挖掘还原出的真实高频链路
是一条更长的链。v1 按 A-K 列了 11 个阶段，三方审一致判定粒度过细
（3:0，见 §9），v2 合并为 **6 个阶段**（括号内是 v1 字母，保留为
artifact 检查点名，不作独立 step）：

```
[specify]    概念→spec→异构 co-design 审→订正出 spec v2（A+B+C+D）
[dispatch]   派工实现：子代理写码，host 规划/审计；派工后不再问人（E）
[impl-audit] 实现层异构对抗审：findings union 合并，不投票，一条不漏（F）
[converge]   逐 blocker 修复收敛；test-pin 是本阶段的出口条件——
             对抗审提到的不变量没落成回归测试，不算收敛（G+H）
[acceptance] 验收闸门：六条同时满足，见 §3.3（I）
[release]    沉淀/同步 + 定版/发布：changelog + 文档对齐 + 经验入库 +
             隐私自检 + push 人工确认（J+K）
```

host agent 可在任何阶段进入、跳过、退出——这是调度先验，不是状态机。

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
- **doctor / healthcheck 是入口**（排障第一命令）；
- **告警是最晚的一层**（已萌芽未普及，本期不做）。

观测层在历史上总是功能完成后才补（P1 排期）——但「功能完成」不等于
「验收通过」。三方审 3:0 反对把观测性放在验收闸门之后（v1 把它放 I 与 J
之间，结构上允许不可观测的功能通过验收）。**v2 裁决：观测性是验收闸门
第 6 条**（见 §3.3），新功能模块默认必查，小修可豁免但须显式记录理由。
三件套验收形态：

1. 结构化日志 schema（machine-parseable，append-only）；
2. doctor / healthcheck 入口（一条命令看健康）；
3. 排障命令（fails / recent / stats 类查询入口）。

## 2. 范围

| 块 | 内容 | 产物 |
|---|---|---|
| W1 | `plugins/dev-workflow` 插件 | plugin.json + paths + profiles + prompts + schemas + eval |
| W2 | workflow-playbook 四个新节 | feature-dev / docs-sync / release / direction-review |
| W3 | 三个已有插件缺口修复 | adversarial-audit / claim-verify-research / migration-refactor |
| W4 | 审稿审出的小代码项 | gemini fail-closed + cross-family 机读字段（见 §6） |

非目标（显式排除）：

- 告警系统（萌芽阶段，留 backlog）；
- 任何新 CLI 命令（不满足「何时可以抽 CLI」五条件）；
- 自动 promotion（eval-run 维持 deferred: automatic_promotion）；
- 把开发链做成硬路由 / 状态机；
- profile 内任何运行时条件逻辑（data-only；族别选择是 host 的事）。

向后兼容：W1-W4 全部 additive。已有 `.lto/<run-id>/` run 继续按原 playbook
快照运行，新 run 才拾取新章节与插件；playbook 是 advisory 非状态机，无迁移。

## 3. W1：dev-workflow 插件设计

### 3.1 plugin.json（完整 v0 manifest，对齐 plugin-boundary §5）

```json
{
  "id": "dev-workflow",
  "version": "0.1.0",
  "stage": "experimental",
  "kind": "path-plugin",
  "description": "Feature development lifecycle playbook: specify -> dispatch -> impl-audit -> converge -> acceptance -> release, with docs-sync and direction-review side paths.",
  "source_notes": [
    "sources/internal-dev-workflow-sop.json",
    "sources/internal-dev-workflow-sop.md"
  ],
  "provides": {
    "paths": [
      "paths/feature-dev-main.json",
      "paths/docs-sync-loop.json",
      "paths/direction-review.json"
    ],
    "profiles": [
      "profiles/spec-refuter-heterogeneous-v1.json",
      "profiles/docs-drift-auditor-v1.json"
    ],
    "evals": [
      "eval/dev-workflow-cases.json"
    ]
  },
  "security": {
    "executable_code": false,
    "max_sandbox": "read-only",
    "env_allowlist": [],
    "requires_human_approval_for": [
      "workspace-write",
      "danger-full-access",
      "network",
      "git-push"
    ]
  },
  "default_enabled": false
}
```

### 3.2 paths/

**`feature-dev-main.json`**：§1 六阶段主路径。关键设计点：

- 每个阶段是「step + 期望 artifact + 停止条件 + 可选升级点」。
- specify 阶段的 co-design 子步骤与 impl-audit 阶段注明
  「可挂 `adversarial-audit` 插件复用 refuter profile」——**注明而非依赖**
  （plugin-boundary v0 无插件间依赖，宿主自行决定挂不挂）。
- **worktree 时序约束**（pi finding）：specify 阶段全程 read-only 不开
  worktree；dispatch 阶段在 specify 收口（spec v2 落盘）后才创建 worktree。
  禁止 spec 审与实现并行共用 worktree 范围。
- dispatch 注明 worktree_exec 沙箱与「派工后不问人」契约。
- converge 的出口条件含 test-pin（§1 所述）。
- acceptance 阶段以 prose 指引 host 读 `prompts/acceptance-gate.md`
  自查（host 自查 checklist 不派 runner，故不走 profile 机器引用——
  validator 只校验 profile 引用的 prompt，path 级 prompt 引用不在 v0
  合同内，见 §9 codex finding 5 的处理）。
- release 阶段以 prose 指引 host 执行仓库级隐私自检（不复制文件入插件，
  防 drift）：**strict 模式具体命令**
  `bash scripts/privacy_self_check.sh --repo . --strict`（gitleaks 不可用时
  加 `--no-gitleaks` 并在 run state 显式记录降级）。push 永远 NEEDS_CONFIRM。

**`docs-sync-loop.json`**：文档对齐独立路径（非 review 非 debug）：
fan-out 多路审计扫 doc-vs-code drift → union 合并 → 逐条实读修复 →
防 drift test-pin（改代码不同步文档即测试红）。
**有意分歧豁免**（pi finding）：文档可用行内注记声明有意偏离
（如 `<!-- drift-ok: 描述 2026Q4 目标架构 -->`），审计方必须尊重注记、
在 findings 中将命中注记的条目标为 `intentional` 而非 drift。

**`direction-review.json`**（v1 名 direction-review-vote，按 3:0 票决结果
重设计——票决降级为受限工具，不再是主轴）：

1. 分歧分类：先判定分歧是「证据可裁决」（有 path:line / 命令输出 /
   官方文档可核）还是「品味/方向」（无独立证据可裁决）。
2. 证据可裁决 → 派异构核验，按证据裁决（不投票）。
3. 品味/方向 → **默认升级人类**。异构意见仅作为 advisory 证据附上；
   2/3 票仅在人类显式授权「按多数走」时使用；任一审计方给出
   needs_human 即直接升级，不被多数票否决。
4. 产物是决策档（含各方立场、证据、最终裁决与理由），不是 findings JSON。

### 3.3 prompts/

**`acceptance-gate.md`**（六条验收闸门 checklist，host 在 acceptance 阶段自查）：

1. 脚本全绿：项目自有验证脚本（registry / lint / CI 类）全部通过；
2. 实物读验：关键产物实读核验，exit code 0 不单独作数；
3. 对抗审收敛：findings union 处理完，无遗留 blocker / high；
4. 文档同步：README / changelog / 接口文档与代码对齐；
5. 经验入库：本次踩坑与决策已沉淀（记忆系统 / 决策档 / handoff 任一）；
6. 可观测（三方审新增，3:0）：新功能模块带 §1.2 三件套；小修可豁免，
   豁免须显式记录理由。

六条**同时满足**才算 done；任何一条豁免须显式记录理由。

**`observability-module.md`**：三件套 checklist（§1.2）+ 验收形态
（日志能被机器 parse、doctor 一条命令、排障查询能答「最近失败了什么」）。
供 acceptance 第 6 条与 playbook feature-dev 节引用。

**`spec-codesign.md`**：spec 审 prompt（给异构 runner，挂
`spec-refuter-heterogeneous-v1` profile——机器引用，validator 可校验）。
要点：refute-first、对照仓库现状实读验证（防脑补）、产出结构化 findings、
区分「spec 错误 / spec 缺失 / 方向异议」三类（前两类 union 处理，
第三类走 direction-review）。

所有 runner prompt 末尾带 no-preamble JSON 硬约束（与现有 8 个 prompt 同款）。

### 3.4 profiles/

最小化：**只加 2 个**，其余复用场景（adversarial-audit 的 refuter）由
path 注明：

- `spec-refuter-heterogeneous-v1.json`：spec co-design 审计方，read-only，
  挂 `spec-codesign.md`。**跨族约束用机读字段表达**（三方审 q4 共识）：
  profile 带 `family` 枚举值；新增 `runner_constraints` 数据字段
  `{"exclude_host_family": true, "min_distinct_families": 3}`——
  字段本期落进 profile/path 数据与 schema 校验（W4），scheduler 派工时
  最小校验（同族即拒）。host 派工时自报族别。
- `docs-drift-auditor-v1.json`：docs-sync 扫描方，read-only。

**read-only 兑现注记**：v1 发明的 `enforcement_note` 字段不存在于现行
schema / scheduler（pi 审出）。v2 改为：兑现差异写进 profile 的
`description` 自由文本 + `sources/` 文档，**不声称 scheduler 读它**——
scheduler 已有的 fail-closed 行为（agy read-only 即拒）不依赖注记。
把注记升级为机读字段列入 backlog，不在本期。

### 3.5 eval/

eval pack 与现有三插件同合同：

- metrics 必含 `permission_violations` + `private_path_leaks`；
- `safety_regressions_allowed: 0`；`minimum_runs_before_promotion: 5`；
- **gate 与 judge 分层**（codex finding：eval-run 的 case_ok 只由
  baseline/candidate runner status 决定，judge 不参与 promotion）：
  确定性指标（parse_ok / timeout / 安全指标）是 gate；语义质量期望
  （「必须抓出缺口」类）写进 case 的 `expected_findings` 字段，
  **仅供 judge 层对照，不进 case_ok**。维持 deferred: automatic_promotion。
- cases（read-only）：
  1. `feature-dev-path-playbook-consistency`：审 feature-dev path 与
     playbook 新节是否自洽（runner: codex；fixture: 本仓库公开文件）；
  2. `acceptance-gate-completeness`：用 acceptance-gate prompt 审一个
     **冻结的合成收尾陈述 fixture**（刻意缺第 4/5/6 条），expected_findings
     供 judge 对照（runner: pi）；
  3. `observability-checklist-frozen-fixture`（按 3:0 票决重设计）：
     用三件套 checklist 审**冻结的合成项目快照 fixture**（预埋已知观测性
     缺口），不审活的 LTO 仓库——避免 eval 自身写 `.lto/` 产生的测量干扰
     与自评循环（runner: claude）。LTO 自审 dogfood 可另行手动跑，
     不进 eval pack 不作 gate；
  4. `direction-vote-vs-union-discrimination`：给混合 findings + 方向异议
     的冻结样本，审 prompt 能否正确分流（runner: codex）。

## 4. W2：workflow-playbook 四个新节（全五字段矩阵）

三方审指出 v1 只给了要点不成矩阵（codex/agy），v2 补全。实现时按现有
五节的格式逐字段落地：

### feature-dev

- 触发信号：新需求 / 新功能从零开始；改动会产生新模块或新对外行为；
  你发现自己想跳过 spec 直接写码。
- 可用 primitive：`lto start --goal/--why/--done-when`；`task add` 拆阶段；
  specify 阶段挂 `dev-workflow` 插件 + 可选 `adversarial-audit`；
  `runner` 落实现证据；`audit --auto-dispatch`（impl-audit）；
  `worktree_exec`（dispatch 阶段隔离写入）；`judge`；`closeout`。
  观测性查看触发条件：派工后看 `.lto/<run-id>/live/` 实时日志；
  收敛卡壳看 `events.jsonl` 与 telemetry；复盘看 interventions 记录
  与 `recap --mine`。
- 期望 artifact：spec v1 与 v2（含审稿订正记录）、worktree 分支、
  per-task evidence、findings union register、test-pin 测试文件、
  六条闸门自查记录、changelog entry。
- 停止条件：六条验收闸门同时满足（§3.3）；任何豁免有显式理由。
- 反模式：跳过 spec 直接写码；自审代替异构审；test-pin 缺位；
  把「实现完」当「做完」；观测性永远滞留 backlog。

### docs-sync

- 触发信号：代码大改后；周期性 drift 审计；用户指出文档过时；
  changelog 与文档口径不一致。
- 可用 primitive：fan-out 多路审计（可挂 `docs-drift-auditor-v1`）；
  union 合并；`runner --kind manual` 登记逐条修复证据；
  防 drift test-pin（如从源码动态抽阈值断言文档同值）。
- 期望 artifact：drift findings union 清单（含 intentional 标注）、
  逐条修复 diff、防 drift 测试。
- 停止条件：union 清单逐条处理完（修复或标 intentional）；
  防 drift 测试落地并通过。
- 反模式：只改 README 不查全部引用；修文档不加防 drift test；
  把有意分歧（ADR / 未来架构描述）当 drift 修掉。

### release

- 触发信号：版本定版；对外 push；公开仓库同步；向他人交付。
- 可用 primitive：changelog 定版；
  `bash scripts/privacy_self_check.sh --repo . --strict`（gitleaks 缺席时
  `--no-gitleaks` + 记录降级）；敏感扫描；`closeout --summary`；
  push 前 human gate。
- 期望 artifact：版本号对应的 changelog 段、隐私自检输出、
  closeout handoff、push 确认记录。
- 停止条件：隐私自检 strict 通过（或降级被显式记录且人类接受）；
  人工确认 push；沉淀完成（验收闸门第 5 条在 release 复查）。
- 反模式：push 与沉淀脱节；版本号无 changelog 对应；私有内容混入公开仓
  （gitignore + 敏感扫描双防线）；用 dry-run 的 exit 0 冒充 strict 通过。

### direction-review

- 触发信号：架构边界判断；两个都对但只能选一的方案分歧；
  审计方之间出现非事实性矛盾。
- 可用 primitive：分歧分类（证据可裁决 vs 品味/方向）；
  证据可裁决→异构核验裁决；品味/方向→升级人类（异构意见仅 advisory）；
  决策档落 decision log 类位置（见 `decision-logging.md`）。
- 期望 artifact：分歧分类记录、各方立场与证据、决策档（含理由）。
- 停止条件：证据分歧被证据裁决；品味分歧由人类拍板并落档；
  任一审计方 needs_human 即升级，不被多数票否决。
- 反模式：用 findings union 流程处理方向分歧（永不收敛）；
  让同族模型投三票；用 2/3 票否决 needs_human；票决品味问题。

另：现有 review 节补一句「building 阶段的中途 verification 也适用本节」
（边建边验，不等到收尾才审）。

## 5. W3：三个已有插件缺口修复

**adversarial-audit**：
- 新增 `claude-refuter-v1.json`（第四审计方）；**同步更新**
  `paths/adversarial-fanout-convergence.json` step 1 的 profiles 数组加入
  claude-refuter-v1（pi/agy 审出：只加 profile 不接 path = 死数据），
  并在 path 的 anti_patterns 注明：宿主为 claude 时禁用 claude-refuter
  （同族自审）；
- agy profile 的 `description` 加 read-only 兑现注记：agy 无法兑现
  read-only，scheduler 会 fail-closed 拒绝（2026-06-10 实测）；read-only
  审计任务勿派 agy/gemini。path 的 dispatch 说明同步注明；
- **eval 的 agy case 改为负向 case**：预期 scheduler 以
  「agy cannot enforce read-only」拒绝派工，把 fail-closed 行为本身固化为
  回归断言（比删 case 或绕沙箱都更连贯——codex 审出 v1 的修法不完整，
  此为选定的连贯路线）；
- agy prompt 加幻觉警告：禁止假设「用户已批准」类对话状态；
- path 加方向分歧分流出口：findings 矛盾→host 实读裁决；方向分歧→
  转 dev-workflow 的 direction-review 路径（prose 注明，无插件间硬依赖）。

**claim-verify-research**：
- path 加显式 step：**本地代码 claim 必须实读源码验证**（LLM 断言
  不是证据，evidence 必须是 path:line / 命令输出）；
- sources 注明与 docs-sync 的边界：核验对象是「对外 claim」，
  文档与代码的 drift 修复走 docs-sync。

**migration-refactor**：
- prompt/path 加 minimal exemplar 选点指引（选最小但覆盖全部变换模式的
  样例，列出判断维度）；
- 同族冲突处理改为**静态方案**（agy 审出 v1 的「profile 动态换 runner」
  违反 data-only）：新增 `codex-semantic-equivalence-v1.json` 静态 profile
  （同 prompt 不同 runner/family），path 注明宿主按族选 profile，
  profile 本身零运行时逻辑；
- path 的 merge step 加 **host 执行的 rollback 命令序列**（明确：scheduler
  遇冲突只停不自动回滚，回滚由 host 按序执行——agy 审出非交互执行歧义）：
  1. 保留现场：不删 worktree，登记 `rollback-artifact.json`
     （worktree 路径 + 冲突 diff）；
  2. 若 merge 中途：`git merge --abort`（或 rebase 中途 `git rebase --abort`）；
  3. host 决策：rebase 后重试，或弃批重跑
     （`git worktree remove --force <path>` + `git branch -D <batch-branch>`，
     仅在弃批决策落档后执行）；
  4. 不级联：冲突批之后的批次不自动继续。

## 6. W4：审稿审出的代码小项

1. **gemini fail-closed 洞**（agy 审出，已实读 `agent_job.py` 确认）：
   `validate_for_runner` 只拦 agy 的 read-only，gemini 同样无 read-only
   兑现机制却直接放行。修法：gemini 与 agy 同等拒绝（一行级），
   加回归测试。gemini 停服在即更要 fail-closed 不能裸奔。
2. **cross-family 机读字段**（三方 q4 共识）：profile schema 加 `family`
   枚举（openai/anthropic/google/deepseek/meta）+ `runner_constraints`
   （`exclude_host_family` / `min_distinct_families`）；
   plugin validate 校验枚举合法性；scheduler 派工侧做最小同族拒绝
   （host 自报族别）。完整的族别自动推断不做（host 声明制）。

## 7. 验收标准（本 spec 自身的 done 定义）

1. `lto plugin validate` 全绿（4 插件）+ 静态 eval 全绿；
2. eval-run 实测：dev-workflow 4 case 全 `ok=true`、candidate
   `parse_ok=true`、零新增 `private_path_leak` / `permission_violation`；
   adversarial-audit 的 agy 负向 case 断言 scheduler 拒绝信息命中；
3. 三方异构 spec 审收敛（本档 §9：v1 findings 已 union 处理完，
   无遗留 blocker/high；实现后对最终产物再跑一轮对抗审）；
4. workflow-playbook / README / CHANGELOG 同步；
5. 公开内容敏感扫描通过（无私有项目名 / 对话原文 / 私有路径）；
6. W4-1 gemini 拒绝有回归测试；W4-2 字段有 validate 覆盖。

——即用验收闸门验收「验收闸门」自身（dogfooding）。

## 8. 开放问题处理记录（v1 §7 五问，三方裁决）

| 问 | 三方共识 | v2 裁决 |
|---|---|---|
| q1 粒度 | 3:0 太细 | 11 阶段并为 6（§1），字母保留为 artifact 检查点 |
| q2 观测性位置 | 3:0 反对 I/J 之间 | 验收闸门第 6 条，可豁免须记录（§3.3） |
| q3 票决有效性 | 3:0 票决品味无效 | 品味/方向默认升人类；票决仅限证据可裁决 + needs_human 一票即升级（§3.2） |
| q4 跨族机读 | 3:0 要机读字段 | family 枚举 + runner_constraints，W4-2（§6） |
| q5 eval 自评 | 3:0 换冻结 fixture | case 3 改冻结合成 fixture；LTO 自审降为非 gate（§3.5） |

## 9. v1→v2 审计记录（findings union 处理台账）

三方 verdict：codex=block，pi=block，agy=revise。union 共 21 条，全部处理：

| # | 来源 | 类型/级别 | 处理 |
|---|---|---|---|
| 1 | 三家 | spec-error/critical：plugin.json 违 schema | 采纳，§3.1 重写为完整 manifest |
| 2 | pi | spec-error/high：enforcement_note 字段不存在 | 采纳，改 description+sources 注记，不声称 scheduler 读（§3.4） |
| 3 | pi+agy | spec-error/high：claude-refuter 不接 path = 死数据 | 采纳，§5 同步改 path profiles 数组 |
| 4 | agy | spec-error/high：gemini 同漏 read-only 拦截 | **实读 agent_job.py 确认真洞**，立项 W4-1 |
| 5 | agy | spec-error/high：profile 动态换 runner 违 data-only | 采纳，改静态双 profile（§5） |
| 6 | codex | spec-missing/critical：agy 修复路线不连贯 | 采纳，eval agy case 改负向断言（§5） |
| 7 | codex | spec-missing/high：eval 语义期望 vs case_ok 机制不匹配 | 采纳，gate/judge 分层 + expected_findings 仅供 judge（§3.5） |
| 8 | codex+agy | spec-missing/high：playbook 四节字段不全 | 采纳，§4 给全五字段矩阵 |
| 9 | codex | spec-missing/high：path 级 prompt 引用不在 validator 合同内 | 采纳，host 自查 prompt 走 prose 指引；runner prompt 走 profile 机器引用（§3.2/3.3） |
| 10 | codex | spec-missing/high：privacy gate 默认 exit 0 | 采纳，写明 --strict 具体命令与降级记录（§3.2/§4 release） |
| 11 | codex+pi | spec-missing/high：跨族约束无机读机制 | 采纳，W4-2（§6） |
| 12 | pi | spec-missing/medium：docs-sync 缺有意分歧豁免 | 采纳，drift-ok 注记机制（§3.2） |
| 13 | pi | spec-missing/medium：C/E 并发 worktree 冲突 | 采纳，specify read-only、dispatch 后开 worktree（§3.2） |
| 14 | pi | spec-missing/low：rollback 无具体命令 | 采纳，§5 给 git 命令序列 |
| 15 | pi | spec-missing/low：无向后兼容声明 | 采纳，§2 additive 声明 |
| 16 | agy | spec-missing/medium：rollback 非交互执行歧义 | 采纳，明确 scheduler 只停、host 执行（§5） |
| 17 | codex+pi+agy | direction：观测性位置 | 3:0 采纳，闸门第 6 条（§3.3） |
| 18 | codex+pi+agy | direction：eval case 3 自评 | 3:0 采纳，冻结 fixture（§3.5） |
| 19 | codex+pi+agy | direction：A-K 粒度 | 3:0 采纳，并为 6 阶段（§1） |
| 20 | agy | direction：privacy-self-check 引用违反插件目录隔离 | **否决**——实读 plugin-boundary.md 无自包含合同条款，依据不成立；但采纳其精神：用 prose 指引不作机器路径引用，不复制文件防 drift（§3.2） |
| 21 | pi | direction/low：A-D 合并为 specify | 已并入 #19 处理 |

审稿原件：`.lto/spec-review-dev-workflow/replies/`（gitignored 运行时产物，
不入仓；本表为权威摘要）。
