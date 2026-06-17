# Goal（调查型）: Rust 是否完整继承历史功能 + 架构/代码提升点 + 插件系统/预设工作流澄清 + README 重写

> 致 codex:沿用既有约束(LTO skill 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写、release/tag 归 host)。
> **这份是调查 + 文档任务,不是大重构。产出一份调查报告 + 一次 README 重写。代码层面只许做「文档/注释澄清」级改动,任何实质代码重构只写进报告的「建议」不直接改**(架构重构要 host 看报告后另立 goal)。
> 做完停。

---

## 为什么做（第一性）

LTO 经历 Python→Rust 全量重写(v0.5.0)+ 多轮加功能(tmux runner / O2 events / lean+session 省 token / ⑫ 安全加固)。用户的真实疑问有五个,当前文档答不清:

1. **我们之前开发的功能,是否完全由 Rust 继承了?**（怕重写漏了能力——「重写绿测试≠语义对齐」是踩过的坑）
2. **架构设计 + 代码优化上还有什么值得提升?**（重写后该有一次冷静的架构体检）
3. **插件系统如何生效?**（plugin.rs/plugin_eval_run.rs 存在但「怎么用、边界在哪、数据怎么流」README 没讲）
4. **预设有哪些工作流?**（workflow-playbook.md 有 Playbooks 但 README 没索引,用户不知道有哪些可用）
5. **README 没讲清**（133 行,是版本变更堆叠,缺「插件怎么用 / 有哪些工作流 / 架构全貌」）

目标:产出**一份调查报告**回答 1-3,**列清** 4,**重写 README** 解决 5。

---

## ⚠️ 必读:前置事实（host 已盘清,别误判）

1. **Python 退役判断别误报**：`scripts/` 下仍有 4 个 `.py`（`check_docs_consistency.py` / `audit_ledger_check.py` / `write_decision.py` / `check_python_rust_ownership.py`）——这些是**构建/CI 检查工具,不是 LTO 运行时功能**,不在退役范围。运行时 Python（旧 `scripts/lto/` 包 + `lto_run.py`）已在 v0.5.0 删除。调查「功能继承」时区分:**运行时能力**（要 100% Rust 继承）vs **构建工具**（Python 写无妨）。
2. **功能继承的对照基准**：以 git 历史里 Python 退役前的能力为基准。查法:`git log --oneline --all | grep -iE "retire|python|port"` 找退役 commit,`git show <commit>` 看删了什么；`references/python-rust-ownership.md` 有命令 ownership 分类；旧能力散见各 `references/*.md`。**别只信 backlog/CHANGELOG 当现状,以 `grep src/*.rs` 实证 Rust 真有没有**（踩过 backlog 假阳性的坑）。
3. **30 个 Rust 模块**：`src/*.rs` + `src/commands/*.rs`。大头 cli.rs(67K)/ops.rs(156K)/scheduler.rs(70K)/tmux_runner.rs(53K)/decision.rs(47K)/plugin*.rs(74K)。
4. **插件系统落点**：`src/plugin.rs`(validate/render-profile/static eval/mount/source-note)+ `src/plugin_eval_run.rs`(真 A/B eval-run)；命令在 `cli.rs:264` 起的 `plugin` 子命令组；设计文档 `references/plugin-boundary.md` + `plugin-real-eval-runner.md`。
5. **预设工作流落点**：`references/workflow-playbook.md`（Playbooks 节:dev-workflow/review/... line 60 起）+ `dev-workflow-spec.md`。注意 LTO 哲学是「playbook 是调度先验,不是硬路由命令」——工作流是 host-agent 读的先验,不是 `lto workflow run X` 这种命令。报告要讲清这个定位。
7. **有现成插件可跑示例（别从零造 fixture）**：`plugins/` 下有 5 个真插件——`dev-workflow` / `deep-agent-profiles` / `migration-refactor` / `claim-verify-research` / `adversarial-audit`（含 paths/schemas/sources/eval 完整结构,是讲插件生命周期的最佳实例）。Phase 3 的插件示例直接用这些跑通（`lto plugin validate plugins/adversarial-audit` 等），dogfood 验证。
8. **预设工作流与插件的关系**：注意 `plugins/dev-workflow/` 既是插件又对应 workflow-playbook 的 dev-workflow——查清「插件如何为预设工作流提供 prompt/profile/eval」这层关系（Phase 3 和 4 的交汇点,讲清它俩怎么协作）。
6. **架构哲学基准**：`references/control-loop-harness.md` + `protocol-and-language-strategy.md` + `engineering-map.md`（模块表）。评架构提升点要对照这些既定哲学（state 真源 / runner-audit-worktree 是 affordance / 不替 host 决策），别提违背哲学的「改进」。

---

## Phase 1:功能继承审计（产出报告章节 A）

逐项核 Python 退役前的运行时能力是否在 Rust 有对等实现。**方法**：
- `git log` 找退役 commit + `references/python-rust-ownership.md` 列出旧运行时命令/能力清单。
- 对每个能力 `grep src/*.rs` 实证 Rust 有无 + 跑 `lto-rs --help` / 子命令 `--help` 看命令面。
- 重点查易漏的:autopilot 三档(supervised/auto-exec/autonomous)、audit failover、worktree 沙箱攻击向量拦截、recap/resume、memory sink、decision 投票、budget 闸门、events/telemetry。
- **判据**：报告 A 章给一张表「旧运行时能力 | Rust 落点 file:line | 对等？(完整/部分/缺失) | 证据」。每行有 grep/help 实证,不是「应该有」。缺失/部分的明确标出（这是这份 goal 最高价值产出）。

## Phase 2:架构 + 代码提升点（产出报告章节 B）

冷静体检,对照既定哲学（必读 6）给**有依据**的提升点,不为提而提：
- **架构层**：模块职责是否清晰（ops.rs 156K / cli.rs 67K 是否过载该拆？scheduler/decision 边界？）；有没有重复机制（如本轮刚合一的 redact 双正则那种）；affordance 是否正交。
- **代码层**：大文件可读性、重复代码、错误处理一致性、测试覆盖盲区（哪些关键路径无测试）。
- **每条提升点**：给 file:line + 为什么是问题 + 改进方向 + **预估收益/风险**。区分「现在就该修的小债」vs「要另立 goal 的大重构」。
- **判据**：报告 B 章按 severity 排序,每条可定位（file:line）、有理有据。**明确标注哪些 host 该另立 goal**（不在这份直接动）。

## Phase 3:插件系统如何生效（产出报告章节 C + README 章节）

讲清插件系统的**实际数据流和边界**（读 plugin.rs / plugin_eval_run.rs / plugin-boundary.md 实证,不照文档复述）：
- 插件是什么（数据-only？能注入什么:prompt/profile/eval pack？不能做什么:不能跑任意代码?边界在哪）。
- 命令面:`plugin validate/render-profile/eval/eval-run/mount/source-note` 各做什么,输入输出。
- 一个插件从 mount 到 eval-run 的完整生命周期（数据怎么流、谁消费、产物落哪）。
- `plugin eval-run` 怎么真跑 A/B（baseline vs candidate,经 scheduler,judge 评分）。
- **判据**：报告 C 章 + 能直接放进 README 的「插件系统」小节（含一个最小可跑示例命令序列）。示例命令 codex 要**实际跑通**验证（dogfooding:`lto plugin validate <某个 fixture>` 等），不是编的。

## Phase 4:预设工作流清单（产出报告章节 D + README 章节）

从 `workflow-playbook.md` + `dev-workflow-spec.md` 提清**有哪些预设工作流 + 各自触发信号/动作/产物/停止条件**：
- 列全:dev-workflow(feature-dev/docs-sync/direction-review)、review、debug、migration、claim-verify、research... 实际有哪些。
- 讲清定位:这些是 **host-agent 调度先验（playbook）**,不是 `lto workflow run` 硬命令——用户怎么「用」它们（host 读了按先验调度 primitive）。
- **判据**：报告 D 章 + README「预设工作流」小节(一张表:工作流 | 何时用 | 关键 primitive | 期望产物)。

## Phase 5:README 重写（交付物）

基于 Phase 3/4 的产出,重写 README 让它答清五问。**保留**现有「30 秒看懂 / 最小可跑路径 / 何时不该用」的好部分,**补上**:
- 架构全貌一图/一段（state 真源 + runner/audit/worktree affordance + recap/resume + human gate）。
- 插件系统怎么用（Phase 3 小节）。
- 有哪些预设工作流（Phase 4 表）。
- **砍掉**版本变更堆叠（v0.5/v0.3「新增」段移到 CHANGELOG,README 不该是 changelog）。
- **判据**：新 README 一个新读者能答出「LTO 是什么 / 怎么最小跑 / 插件怎么用 / 有哪些工作流 / 何时别用」；`check_docs_consistency.py` 仍绿（命令数等不漂）。

---

## 执行顺序 + 收口

1. Phase 1-4 先产出**调查报告**（建议落 `references/2026-06-17-rust-inheritance-and-architecture-review.md` 或 `.lto/<run>/` 报告产物），各章有 file:line 实证。
2. Phase 5 基于报告重写 README。
3. 收口:`cargo fmt/clippy/test` 全绿（即使只改文档,也确认没碰坏）→ `check_docs_consistency.py` 绿 → `lto audit --auto-dispatch --discover-risks` 异构审**报告的结论**（让异构 auditor 挑战「继承审计有没有漏、架构建议站不站得住」）→ `lto check` → commit（报告 + README 分开 commit）。
4. backlog 若调查出新的真缺口,入 backlog 表（别散落）。

## 提醒（安全阀）

- **这份不做实质代码重构**——架构提升点只进报告「建议」,host 看完另立 goal。顺手能做的只有文档/注释级澄清。
- **实证优先**:功能继承用 grep + help 实证,不信文档自述；插件示例 codex 实际跑通,不编。
- **区分运行时能力 vs 构建工具**（必读 1）,别把 4 个构建 .py 误报成「Python 没退役干净」。
- **README 不是 changelog**:版本堆叠移走。
- host 亲验是硬停止点:报告的「功能缺失」结论 host 会复核（重写漏功能是高代价错误,值得异构审 + host 双验）。
- commit 你写,release/tag 归 host。
