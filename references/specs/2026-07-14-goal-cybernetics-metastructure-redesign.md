# 设计：用工程控制论元结构改造 LTO（skill/docs 四层化 + 四个 core 机制 goal）

- 日期：2026-07-14
- 状态：design（已批准，待实施）
- LTO run：`20260714-043510-skill-lto-skill-readme-references-core-g-f3c7170b`
- 异构评审证据：run T1，`replies/reply-codex-cybernetics-discussion.md`（codex，gpt-5.6-sol ultra）
- 灵感源：`engineering-cybernetics-essence/SKILL.md`（钱学森《工程控制论》推理引擎 skill，四层元结构）

## 1. 目标与非目标

**目标**：借控制论 skill 的四层纪律（路由→策略→域图→溯源）重建 LTO 的信息组织：
SKILL.md 从 23.6K 线性长文变为四层引擎（预算 10KiB）；references 真实切分并标注状态/权威级别；
四个控制论机制写成可派工的 core goal 文档。

**非目标**：
- 不把 LTO 写成控制论 skill 的样子——软件项目的真源是 runtime/源码，不是文档"原文"。
- 不强制七段输出模板（公式分析专用，硬套污染工程输出）。
- 不新增 UI/daemon/自动语义路由；host 仍是 planner；judge/confidence/telemetry 不自动改 promote/route。
- 本轮不实现 Rust 机制（Phase C 只产出 goal 文档）。
- frontmatter（name/description/触发条件）不变，不借机扩大 skill 触发面。

## 2. 关键裁决（异构评审收敛结论）

1. **SOURCE 层必须倒置为权威层级**：binary `--help` → `src/cli.rs` → `COMMANDS.md` →
   active reference → 历史/设计材料。文档与 runtime 冲突判文档漂移，不做兼容解释。
2. **reference truth 是前置条件**：checker 假绿放行了成体系漂移（见 Phase 0），
   先压 SKILL/做路由会把错误升级成默认路径。迁移顺序不可倒置。
3. **10KiB 是预算不是完成证据**（Goodhart 风险）：完成证据 = 路由准确 + P1 不丢 + 零 stale flag。
4. **INDEX 只做路由/状态/迁移层**，不能替代切分超载文件。
5. **C4 可观性不单立机制**，并入 `autonomous_gate` 做子检查。

## 3. Phase 0：修 reference truth（前置硬条件）

已亲验坐实的漂移（checker 现全放行）：

| 漂移 | 位置 | 修法 |
|---|---|---|
| 教不存在的 `--request/--with-audit/--profile/--install-hooks` | `references/run-state-workflow.md:13-38` | 改为当前 `Start` 真实参数（`src/cli.rs:55-76`） |
| 教不存在的 `--auto-commit` | `references/execution-loop.md:77-86,140-157` | 对齐当前 `RunnerCommand`（`src/cli.rs:447-523`） |
| 手写"21 个可见业务命令"（现 26） | `SKILL.md:315`、`references/onboarding.md:100` | 删除手写总数，链接 `COMMANDS.md` |
| `VERSION`=0.9.2 vs `Cargo.toml`=0.9.3 | 仓库根 | **外部基线问题，单列处理**，不归本次重组，也不得在未修时声称全绿 |

checker（`scripts/check_docs_consistency.py`）补覆盖：
- active reference 显式清单（不由 INDEX 漏列决定 CI 覆盖）；
- Markdown 相对链接 + anchor 校验（含大小写）；
- 从 clap/`COMMANDS.md` 派生的 active 文档参数核对（或可执行 fenced-command smoke）；
- 禁止在 SKILL/onboarding 手写业务命令总数；
- old-path stub 与 leaf 都进检查。

## 4. Phase A：SKILL.md 四层化

```
① ROUTER          入口顺序：1) 判有无 active run（有→runs/resume/check；无→才谈 start）
                  2) 判动作意图（接管/立项/执行/验证/交付/维护） 3) 按风险决定是否跨域
                  默认加载 1 个主 reference；跨域/安全边界最多 2 个（预算非禁令）
② OPERATING POLICY
   P1 必须：人说了算（phase transition/不可逆/语义争议/closeout）｜先观测后控制（先读
   .lto/git/runtime 再动作）｜审者≠host（限主观/对抗审计，确定性测试除外）｜信息不足
   先自助补证（state/source/runtime），权威证据仍缺且影响方案才问人｜证据先于断言
   （区分源码存在/二进制存在/runtime 可用）｜不可逆动作 human gate（不因 autonomous 取消）｜
   调优必须有 baseline/metric/pass line/post result｜已收敛即停（收敛须当前证据证明）
   P2 建议：最小版本优先（先删并复用再加）｜快 runner 优先收口（仅 host 明确选择，
   不按历史 telemetry 自动路由）
   P3 可选：回答格式建议。注意「不适用/失效条件」是每域必须的合同，不降为 P3。
③ DOMAIN MAP · 六域
   Ⅰ 接管与恢复  runs/resume/recap/check     → onboarding.md、long-loop-state.md
   Ⅱ 立项与契约  适用性/start/task/preflight/开发四证据 → run-state-workflow.md
   Ⅲ 执行与派工  runner/dispatch-goal/events/autopilot  → execution-loop.md、cross-runtime notes
   Ⅳ 验证与收敛  audit/judge/check/ledger    → audit-convergence.md、playbooks
   Ⅴ 交付与发布  部署实测（真实用户路径验收）/closeout/release → deploy-sequencing.md、release-workflow.md
   Ⅵ 学习与维护  decision/memory/telemetry/budget/prune/plugin → decision-logging.md、control-loop docs
   每域卡固定行：目标｜首个安全观测｜可用 primitive｜进入证据｜human/stop gate｜不适用/失效｜权威源
   state/evidence/source authority/budget/human gate 是六域共同覆盖层；decision 拍板即落盘，不只在收尾。
④ AUTHORITY & SOURCE：权威层级表（见 §2.1）
```

**必须留在根 skill**：frontmatter 触发/禁用条件；harness≠planner、host/human 最终决策；
`.lto` 与 runtime/source 真源层级；三刹车；human gate；证据先于断言；对抗审计 reviewer 隔离；
tmux 可见写任务 vs headless 只读/兜底边界；turn 完成≠goal 完成；最小端到端路径；停止规则；
「不该用 LTO」表；六域路由及每域不适用条件。

**下沉或删除**：audit/memory/release/plugin 完整命令块；ANIMEM/legacy REST 协议细节；
手写命令总数；完整 Resources 清单（改链 `references/INDEX.md`）；六阶段 ASCII 长图（压成域表）；
与 README/onboarding 重复的操作示例。

**验收**：
- `wc -c SKILL.md <= 10240` 作预算（超少许可接受，须说明）；
- ≥12 个路由用例（覆盖六域 + ≥4 个跨域场景）主 reference 命中率 100%，P1 边界召回 100%；
- 默认每例 1 个 reference，跨域/安全 ≤2；
- 全部示例命令对当前 clap surface 零 stale flag；
- `check_docs_consistency` + 链接/anchor 检查 + 命令 smoke + Cargo gates 全绿。

## 5. Phase B：README / references 真实切分

**切分**：
- `workflow-playbook.md` → 通用哲学/调度循环/闸门（`:1-69`）+「何时抽 CLI」（`:534-544`）+
  场景索引留原文件；11 个 playbook 各一文件进 `references/playbooks/`；旧文件保留同名
  heading + link（外链兼容，未做反链盘点前是兼容要求）。
- `control-loop-harness.md` → 留：目的/控制映射/五类 feedback loop/actuator limits/anti-patterns/
  non-goals（明确当前实现状态）；切出 `events-telemetry-contract.md`（现状合同）与
  `control-loop-roadmap.md`（typed workspace future 等，页首标 `future/design`）。
- `onboarding.md` 瘦身为纯 onboarding：术语/为什么/进项目先看 .lto/resume-recap/最小流程；
  安装链 INSTALL.md、命令链 COMMANDS.md、深入阅读链 INDEX.md。
- 不动：`COMMANDS.md` 结构；`audit-convergence/long-loop-state/decision-logging/release-workflow/
  hooks` 等小而单责文件；dated review/backlog 保持历史身份。

**`references/INDEX.md` 四块**：①六域→主 reference→何时加载→不适用；②跨域场景→允许的两段
加载顺序；③每文档状态标注（active/current、design/future、historical/dated）；④权威级别
（runtime/source、COMMANDS contract、operating policy、explanation、history）。
INDEX 不复制命令参数、不手写命令总数。六域详表以 INDEX 为唯一真源，SKILL/README 只放压缩映射
（checker 校验域名集合一致）。

**README**：保留 `:60-79` runtime 拓扑（六域是操作坐标不是模块分层）；新增六域闭环表；
`:88-99` L1-L4 叙述压一句链接 control-loop overview；release-gated/Windows paused/Rust-only
等 checker 锚点集中在稳定段落。

**迁移顺序（不可倒置）**：修漂移 → INDEX+状态标注+链接检查 → 切 control-loop-harness →
切 workflow-playbook → 瘦 onboarding → 压根 SKILL → 改 README 六域坐标 → 全套 gate。

## 6. Phase C：四份 core goal 文档（本轮交付文档；实现后续派工）

共同红线：本轮只产出 goal 文档；judge/reported confidence/历史 telemetry 不自动改 promote/route；
不可逆动作/phase transition/语义争议走 human gate；每 goal 必写 baseline/metric/pass line/
验证命令/post result；schema 演进 optional-load + new-write + 旧 fixture 回归。

### C1 稳定性信号升级（`src/commands/util.rs` 三件套 → 建议下沉 `src/ledger.rs`）
- 保留硬门禁 verdict（Converged/Converging/Rebound/Stalled，兼容后仍是 closeout 唯一依赖）；
  另加只读 diagnostics（正交维度，不塞一个 enum）：
  `sample_sufficiency / terminal / direction / oscillation / envelope`。
- 前置修三个真问题：①Rust/Python 判定不等价（Rust `util.rs:681-700` 末轮 0 短路 vs Python
  `audit_ledger_check.py:101-122` 先扫 rebound）→ **砍 Python 重复 evaluator**（Rust-only 方向；
  兼容期只允许调 Rust 或共享 golden fixtures）；②空 ledger ≠ 零 blocker（evaluator 判 Converged、
  `ops.rs:563-570` 报 no filled rounds、`closeout.rs:157-175` 可放行——先裁决语义）；
  ③`audit.converged` 事件名撒谎（每次 append round 都发，`cli.rs:1995-2034`、
  `event_emit.rs:305-330`）→ 新增 `audit.round.recorded` + `audit.ledger.evaluated`，旧事件按历史 schema 解释。
- 观测可比性：记录 round id/coverage/auditor set/finding hash lineage，否则结论标 low-confidence advisory。
- 振荡→「换假设」仅 advisory：达最小可比样本 + 持续交替/包络不缩时展示 `forced_entropy`，host/human 决定。

### C2 信息不足禁猜闸门（run readiness + contract completeness 两层）
- base readiness：新 run 必须非空 `--goal`/`--done-when`，缺则**写盘前** fail 并输出「需补充: …」
  （现状 `cli.rs:1093-1118` 空串静默写盘）；`--why` 保持 advisory；legacy state 可读。
- extended contract：四项全空继续允许（兼容）；任一项出现则其余缺项写盘前 fail，
  输出真实 CLI flags（`--target/--constraint/--instrument/--entropy-check`）。
- 配套 typed contract update primitive：partial 已在写盘前被拒，update 入口服务于旧 run / 后补契约场景（否则用户只能手改 JSON）。
- preflight 主职责仍是环境健康（`ops.rs:234-350`）；显式 run 时 readiness 作为独立子结果，与 `--record` 解耦。
- 回归矩阵：空 contract 兼容/partial 拒绝/complete 成功/legacy 可读/explicit preflight missing run
  不静默/与 `check --to implementation --strict` 一致。

### C3 finding 元数据（`reported_confidence` + `invalidated_when`）
- `reported_confidence{level: high|medium|low, rationale}`：审计方自报元数据，非校准非概率，
  **永不进** promote/gate/排序/severity 映射；`invalidated_when`：何种证据出现时 claim 失效（核心价值）。
- **贯通全消费链**（只改 struct 会静默丢字段）：`audit.rs:89-121` typed parser 白名单、
  `cli.rs:1903-1925` risk discovery 复制、`cli.rs:2182-2189` audit prompt 示例、
  `decision.rs:900-917` brief 渲染、`event_emit.rs:270-299` 事件字段、`decision.rs:559-565` fallback
  initializer、`audit_dispatch.rs:194-202` JSON schema。
- 隐私：event 只记 level/presence/hash，不写原始 rationale 文本。
- 回归：改变 confidence/invalidated_when 只影响 review payload/host brief，不改变
  direction/status/pick/gate verdict（现有 `judge_verdict_has_no_numeric_score_and_is_isolated`
  只查 note 文案，不够）。

### C4 可观性子检查（并入 `autonomous_gate`，不单立 gate）
- 现状：`cmd_autopilot`（`ops.rs:1215-1241`）调 `autonomous_gate(repo)` 不传当前 state；
  gate（`ops.rs:3192-3291`）只验历史 operational reliability，不看当前 goal/done_when/instruments。
- 改法：同一 gate 返回两个命名子结果 `operational_reliability` + `current_run_observability`；
  后者检查当前 run 的 goal/done_when/contract/instrument——instrument 与最新 evidence 有结构化
  关联且结果可解析才是 `observable_verified`；只有非空字符串仅 `signal_declared`。
- 顺手修 reliability gate：`any` 历史 timeout 永久污染 autonomous（`ops.rs:3256-3267`）→ 按
  runner/model/task type 匹配 + 有界近期样本 + 比例/连续失败规则；`mining_dispatches` 实为
  distinct_runs 求和（`ops.rs:3230-3237`），阈值调优前改名或改指标。
- 失败降级：observability 未证实回 supervised/NEEDS_CONFIRM；不自动补 instrument、不替 host 选目标。

## 7. 风险与对策

1. 六域三处复制成三真源 → INDEX 唯一详表，SKILL/README 压缩映射 + checker 校验域名集合。
2. 外部反链断裂 → 旧 path/heading 保 stub；stub 与 leaf 都进 checker。
3. Goodhart（为 10K 删边界）→ 字节数只作预算；路由用例 + P1 召回是 pass line。
4. current/future 混载冒充现状 → INDEX 强制状态标注；roadmap 单独成文标 future。
5. checker 改动本身出错 → checker 变更配 fixture 自测；基线红项（VERSION）单列不混入。
6. finding lineage 缺失导致稳定性误判 → C1 记 coverage/auditor set/hash lineage，不足则 advisory。

## 8. 执行分工与顺序

- Phase 0 → A → B：host 直做（文档/skill 工作），每 phase 收口跑
  `check_docs_consistency` + Cargo gates + `lto check`，B/A 完成后走 `lto audit --auto-dispatch` 异构审计。
- Phase C：host 按 goal-doc-for-codex 规范写 4 份 goal 文档（落点 file:line 已盘），
  文件不相交可并行派工（C1 动 ledger/events，C2 动 start/preflight，C3 动 audit schema 链，
  C4 动 autonomous_gate；C1 与 C4 都碰 telemetry——派工时序注意或抽共享步骤 host 收口）。
- 全程在 run `20260714-043510-…` 内推进；closeout 前按六证据收口。
