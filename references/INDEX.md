# references 索引（ROUTER 落地页）

> 六域详表唯一真源。SKILL.md / README 只保留压缩映射，域名集合与此处一致（checker 校验）。
> 加载纪律：默认读 1 个主 reference；跨域 / 安全边界最多 2 个（预算，不是绝对禁令）。

## 1. 六域 → 主 reference

| 域 | 核心动作 | 主 reference | 何时加载 | 不适用 |
|---|---|---|---|---|
| Ⅰ 接管与恢复 | `runs`/`resume`/`recap`/`check` | [onboarding.md](onboarding.md)、[long-loop-state.md](long-loop-state.md) | 进入陌生项目 / compact 后恢复 | 新 run 立项（→Ⅱ） |
| Ⅱ 立项与契约 | 适用性判断/`start`/`task`/`preflight`/开发四证据 | [run-state-workflow.md](run-state-workflow.md) | 决定开新 run、写 delivery contract | 已有 active run 的恢复（→Ⅰ） |
| Ⅲ 执行与派工 | `runner`/`dispatch-goal`/`events`/`autopilot` | [execution-loop.md](execution-loop.md)、[cross-runtime-host-notes.md](cross-runtime-host-notes.md) | 派外部 agent、等完成事件 | 确定性本地命令直接跑（runner 记证据即可） |
| Ⅳ 验证与收敛 | `audit`/`judge`/`check`/ledger | [audit-convergence.md](audit-convergence.md)、[workflow-playbook.md](workflow-playbook.md) | 多模型对抗审、判收敛 | 确定性测试（直接跑，不需异构审） |
| Ⅴ 交付与发布 | 部署实测（真实用户路径）/`closeout`/`release` | [deploy-sequencing.md](deploy-sequencing.md)、[release-workflow.md](release-workflow.md) | 上线、发版、收尾 | 未过Ⅳ收敛闸门时 |
| Ⅵ 学习与维护 | decision/memory/telemetry/budget/`prune`/plugin | [decision-logging.md](decision-logging.md)、[events-telemetry-contract.md](events-telemetry-contract.md) | 拍板落盘、跨 run 挖掘、清理 | 把历史 telemetry 当自动路由依据 |

`state / evidence / source authority / budget / human gate` 是六域共同覆盖层，不单属任何一域；
decision 拍板即落盘（见 decision-logging.md），不是只在收尾才记。

## 2. 跨域场景 → 加载顺序

| 场景 | 顺序 |
|---|---|
| 接手陌生项目并续跑 | Ⅰ onboarding → Ⅰ long-loop-state |
| 新长交付立项 | Ⅱ run-state-workflow（契约四件套） |
| 派 codex 改代码并等完成 | Ⅲ execution-loop →（跨 runtime 时）cross-runtime-host-notes |
| 方案多模型审到收敛 | Ⅳ audit-convergence → Ⅳ workflow-playbook（review 一节） |
| 上线并收尾 | Ⅴ deploy-sequencing → Ⅴ release-workflow |
| autopilot 升档评估 | Ⅲ execution-loop → Ⅵ events-telemetry-contract（跨 run 证据） |

## 3. 文档状态

| 状态 | 含义 | 文件 |
|---|---|---|
| active/current | 当前口径，ROUTER 可落地 | onboarding、run-state-workflow、execution-loop、workflow-playbook、control-loop-harness、events-telemetry-contract、audit-convergence、long-loop-state、decision-logging、release-workflow、deploy-sequencing、hooks、sharing-guide、cross-runtime-host-notes、hs-as-core-tool、plugin-boundary、rust-migration-release |
| design/future | 设计目标，未实现，不得当现状引用 | specs/*、backlog.md、control-loop-roadmap.md、plugin-real-eval-runner.md（含 future 段）、self-driving-wake-loop.md |
| historical/dated | 历史证据，只证明当时 | validation-log.md、python-rust-ownership.md、2026-06-17-rust-inheritance-and-architecture-review.md、agent-runs-decoupling-diagnosis.md、codex-cli-control.md、decision-logging 之外的 dated 材料 |

## 4. 权威级别（冲突时高层胜出；文档与 runtime 冲突判文档漂移，不做兼容解释）

| 级别 | 载体 |
|---|---|
| 1 runtime/source | 安装后二进制 `--help` / `src/cli.rs` / 实现与回归测试 / state·event 实物 |
| 2 command contract | `COMMANDS.md`（由 `src/cli.rs` COMMANDS 派生校验） |
| 3 operating policy | `SKILL.md`、`AGENTS.md`、已拍板 ADR（docs/decisions/） |
| 4 explanation | 上表 active/current references |
| 5 history/design | specs、backlog、dated docs——只作历史/候选证据，不证明现状 |

> 本表不复制命令参数、不手写命令总数（那是 COMMANDS.md 的职责，checker 强制）。
