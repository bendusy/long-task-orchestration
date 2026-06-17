# Rust 继承审计、架构体检与 README 重写依据

日期：2026-06-17

范围：调查 + 文档任务。本文不提出已落地的代码重构；架构和代码改进只作为后续 goal 建议。

## 执行摘要

- Rust v2 已继承 LTO 的主运行时：21 个公开业务命令、5 个隐藏兼容命令、7 个 plugin 子命令均由 Rust CLI 接管。当前实证：`python3 scripts/check_python_rust_ownership.py` 通过；`cargo test --locked --test python_rust_compat` 通过；`cargo run --quiet -- self-test` 通过。
- Python 退役判断不能按 `scripts/*.py` 数量判断。`7070d77 feat: retire python fallback` 删除的是旧 `scripts/lto/` 包、`scripts/lto_run.py` 和 fallback 测试；当前保留的 Python 文件是构建/CI/ADR 辅助工具，不是运行时 fallback。
- 发现 3 个值得修正或另立 goal 的真实漂移：`autopilot --decide`、`audit --collect <reply-dir>`、`budget extend/start budget caps` 在部分活跃文档中被描述为当前命令，但当前 `lto-rs --help` 没有这些参数或子命令。
- 本任务 dogfooding 发现并修复 1 个 LTO 自身 bug：`audit --auto-dispatch --discover-risks` 在健康检查 probe 失败时曾 fail-open 选择首个 auditor，现改为 fail-closed 返回 “no healthy heterogeneous discoverer”。验证：`cargo test --locked audit_dispatch` 通过。
- 最终异构审计已在提升权限下跑通：`PROBE_TIMEOUT=30 scripts/delegate/runners/healthcheck.sh codex pi agy --json` 三家 OK；`cargo run --quiet -- audit --run-id 20260617-rust-inheritance-readme-investigation --auto-dispatch --discover-risks` 由 pi/agy 完成，HIGH/CRITICAL=0，留下 2 条 medium 文档/报告改进项并已处理。
- 插件系统已经能生效：`validate -> render-profile -> eval -> mount -> eval-run` 全链路可跑。当前 `plugins/` 目录实证有 6 个插件，不是 goal 文档里写的 7 个。
- 预设工作流是 host-agent playbook，不是 `lto workflow run X`。当前 playbook 覆盖 review、enterprise-audit、debug、migration、claim-verify、research、feature-dev、tmux-goal-loop、docs-sync、release、direction-review。

## A. Rust 功能继承审计

### A1. 基准与方法

退役基准来自 git 历史和 ownership manifest：

- `git log --oneline --all | rg -i "retire|python|port"` 定位到 `7070d77 feat: retire python fallback`、`d7971cf fix: close python retirement audit gaps`、`2bbd20c Port plugin legacy commands to Rust`。
- `git show --stat 7070d77` 显示删除 `scripts/lto/*.py`、`scripts/lto_run.py`、旧 Python fallback tests，共 23K+ 行删除。
- `references/python-rust-ownership.md:16-77` 声明 21 个公开业务命令和 7 个 plugin 子命令均为 Rust-owned。
- `src/cli.rs:49-269` 是当前 clap 命令面；`COMMANDS.md:18-40` 是当前公开命令摘要。

### A2. 继承矩阵

| 旧运行时能力 | Rust 落点 | 对等性 | 证据 |
|---|---|---:|---|
| 公开 CLI 命令面 | `src/cli.rs:49-269` | 完整 | `cargo run --quiet -- --help` 列出 21 个业务命令 + clap `help`；ownership gate 全 OK。 |
| 隐藏兼容入口 | `src/cli.rs:201-235` | 完整 | `task-add --help`、`phase --help`、`parallel --help`、`pipeline --help` 均由 Rust 解析；`references/python-rust-ownership.md:45-58` 列出替代命令。 |
| run state / delivery contract | `src/cli.rs:50-72`, `src/cli.rs:837-854`, `src/state.rs:40-77`, `src/state.rs:127-177` | 完整 | 本 run 用 `start --target --constraint --instrument --entropy-check` 成功创建 `.lto/20260617-rust-inheritance-readme-investigation`。 |
| `check` phase gates | `src/commands/ops.rs:290-325`, `src/commands/ops.rs:405-720` | 完整 | `cargo run --quiet -- check --run-id ... --to implementation --json` 返回 delivery contract OK、tasks present。 |
| `closeout` gate / handoff | `src/commands/closeout.rs:17-267` | 完整 | closeout gate 代码覆盖 dirty worktree、ledger convergence、handoff/changelog；本任务尚未 closeout。 |
| `resume` / `recap` | `src/commands/resume.rs:11-88`, `src/commands/recap.rs:14-34` | 完整 | `tests/python_rust_compat.rs:7-56` 固定 legacy run fixture 能被 Rust recap/resume/check 读取；测试通过。 |
| task 管理 | `src/cli.rs:280-315`, `src/commands/ops.rs:1257-1439` | 完整 | 本 run 成功登记 P1-P5，invalid phase 被 Rust fail-fast 拒绝。 |
| runner / parallel / pipeline | `src/commands/ops.rs:913-1043`, `src/commands/ops.rs:1571-1665`, `src/scheduler.rs:708-859` | 完整 | scheduler 区分 OK/FAILED/TIMEOUT/RATE_LIMITED，并实现 retry/backoff。 |
| audit auto-dispatch / risk discovery | `src/cli.rs:1216-1291`, `src/audit_dispatch.rs:10-30`, `src/audit_dispatch.rs:60-160` | 部分 | `audit --auto-dispatch --discover-risks` 当前存在；但活跃文档中的 `audit --collect <reply-dir>` 当前 CLI 不存在。 |
| audit failover / healthcheck | `src/audit_dispatch.rs:32-48`, `src/cli.rs:1357-1367`, `src/scheduler.rs:186-207` | 完整（本任务修复后） | risk discoverer 从健康 auditor 中选；健康检查 probe 失败或全员不健康时 fail-closed，不再 fallback 到首个 auditor。 |
| autopilot 三档 | `src/cli.rs:154-174`, `src/commands/ops.rs:1095-1172` | 完整（按三档口径） | help 显示 `--supervised`、`--auto-exec`、`--autonomous`；auto-exec/autonomous 会进入 `cmd_autopilot`。 |
| `autopilot --decide` | `src/decision.rs:145-248`, `src/decision.rs:571-599` | 部分/未暴露 | decision engine 存在；但当前 `autopilot --help` 无 `--decide`。`SKILL.md:264`、`references/onboarding.md:156`、`references/engineering-map.md:69` 的“已实现”描述与 CLI 不符。 |
| worktree 沙箱与危险命令拦截 | `src/effect.rs:19-49`, `src/effect.rs:51-105`, `src/worktree.rs:124-190` | 完整 | `rm -rf`、`git push`、`git reset --hard`、`DROP TABLE`、`curl|sh`、绝对路径逃逸等会被分类为 human judgment 或禁网。 |
| memory export/publish/resume | `src/cli.rs:1169-1195`, `src/commands/ops.rs:1203-1251`, `src/commands/ops.rs:3009-3173` | 完整 | 本地 `.lto` 是真源；publish/resume 走 am CLI，可用性依赖本机 am。 |
| budget gate | `src/budget.rs:210-291`, `src/commands/ops.rs:1097-1112`, `src/cli.rs:583-592` | 部分 | budget 计量和 autopilot hard brake 存在；但当前 CLI 只有 `budget check`，没有文档所说 `budget extend`，`start --help` 也没有 `--max-turns/--max-tokens/--deadline`。 |
| events / telemetry | `src/events.rs:68-155`, `src/event_emit.rs:20-455`, `src/telemetry.rs:13-49` | 完整 | 事件已覆盖 runner、audit、gate、budget、sandbox、judge/decision；redaction enforced。 |
| plugin commands | `src/cli.rs:481-553`, `src/cli.rs:595-788`, `src/plugin.rs:153-324`, `src/plugin_eval_run.rs:61-148` | 完整 | `plugin validate/render-profile/eval/mount/eval-run` 均在本 run 实跑通过。 |
| release planning | `src/cli.rs:187-195`, `src/commands/ops.rs:1175-1195` | 完整（host-owned） | CLI 只 plan，不负责本任务 release/tag；符合约束。 |

### A3. 结论

Rust 对主运行时的继承总体成立，但不是“无缺口”。当前应把缺口区分为三类：

1. **已完整继承**：命令面、state/check/runner/audit auto-dispatch、sandbox、memory、plugin、events/telemetry。
2. **代码存在但完全未接入当前产品面**：decision convergence engine 与 `autopilot --decide`。当前 `src/decision.rs` 没有外部 `crate::decision` consumer，只有内部单元测试覆盖。
3. **活跃文档描述了不存在的命令面**：`audit --collect`、`budget extend`、start budget caps。
4. **狗食修复已落地**：risk discovery 的 runner 健康检查现在 fail-closed；沙箱内 runner 不健康会明确失败而不是伪造审计通过。提升权限后 codex/pi/agy healthcheck 全绿，最终 pi/agy 异构审计已完成。

## B. 架构与代码提升点

### B1. High：活跃文档与 CLI 命令面漂移

证据：

- `autopilot --help` 只有 `--supervised`、`--auto-exec`、`--autonomous`、`--worker-runner` 等；没有 `--decide`。
- `audit --help` 只有 `--auto-dispatch`、`--discover-risks`、`--allow-same-family`；没有 `--collect`。
- `budget --help` 只有 `check`；没有 `extend`。
- 漂移文本：`SKILL.md:264`、`references/onboarding.md:156`、`references/engineering-map.md:69`、`references/run-state-workflow.md:310-314`。

影响：用户和 future agent 会按不存在的命令推进，dogfooding 时会误判为环境问题。

改进方向：本任务内修正文档；若 host 确认需要这些能力，另立实现 goal。尤其 `--decide` 要么重新接线到 `src/decision.rs`，要么明确为 deferred。

收益/风险：收益高、风险低。文本澄清不会改变运行时。

### B1.5. High：risk discovery 曾在 probe failure 时 fail-open

证据：

- 复现命令：`cargo run --quiet -- audit --run-id 20260617-rust-inheritance-readme-investigation --auto-dispatch --discover-risks` 曾返回 `risk discovery runner pi returned skipped exit=None: runner unhealthy: pi`。
- `scripts/delegate/runners/healthcheck.sh codex pi agy claude` 当前返回 codex rc=2、pi rc=1、agy rc=1、claude timeout。
- 旧 `src/audit_dispatch.rs` 在 `healthcheck_blocking(...)` 返回 `Err(_)` 时 fallback 到 `auditors.first()`，导致 probe 失败被当成可派工。

改进方向：本任务已做最小 bug fix：selector 对 probe failure 返回 `None`，复用 `src/cli.rs` 既有 `risk discovery has no healthy heterogeneous discoverer` 错误分支。验证：`cargo test --locked audit_dispatch` 通过；重跑 audit 得到 fail-closed 错误。

收益/风险：收益高、风险低。它不改变审计策略，只修正无健康 runner 时的错误派工。

### B2. High：`ops.rs` / `cli.rs` 过载，阻碍命令面审计

证据：当前原始行数 `src/commands/ops.rs` 4751、`src/cli.rs` 2291、`src/scheduler.rs` 2094、`src/tmux_runner.rs` 1609、`src/decision.rs` 1421、`src/plugin.rs` 1086、`src/plugin_eval_run.rs` 1159。

为什么是问题：命令解析、状态写入、audit dispatch、runner、memory、hook、release、autopilot、测试 fixture 混在同一文件时，文档漂移更难被定位；新增选项容易只改文档或只改代码。

改进方向：另立重构 goal，把 `ops.rs` 拆成 command modules：`check.rs`、`task.rs`、`runner.rs`、`audit.rs`、`memory.rs`、`autopilot.rs`、`release.rs`。`cli.rs` 只保留 clap schema 和 dispatch glue。

收益/风险：收益高；风险中高。属于实质重构，不在本任务直接做。

### B3. Medium：插件实现职责偏大，但边界哲学正确

证据：

- `src/plugin.rs:153-324` 同时负责 validate、render-profile、static eval、source-note。
- `src/plugin_eval_run.rs:61-148` 负责 eval-run 总控，后续还包含 scheduler job、metrics、judge、env filtering、mount lookup。
- 插件边界文档明确 data-only：`references/plugin-boundary.md:58-64`、`references/plugin-boundary.md:117-157`、`references/plugin-boundary.md:236-272`。

为什么是问题：`plugin_eval_run.rs` 已经同时处理 A/B prompt 编译、调度、metrics、permission 检查和 report 持久化。继续加 promotion 或更多 metrics 会进一步膨胀。

改进方向：另立 goal 拆分 `plugin_eval_run/compile.rs`、`plugin_eval_run/metrics.rs`、`plugin_eval_run/report.rs`，保持 public command 不变。

收益/风险：收益中高；风险中。可在有新增 plugin 功能时顺手拆。

### B4. Medium：docs consistency gate 只覆盖命令存在，不覆盖选项语义

证据：`python3 scripts/check_python_rust_ownership.py` 能发现 top-level/plugin command ownership；但没有捕获文档中的 `--decide`、`--collect`、`budget extend` 与 CLI 不一致。

改进方向：另立小 goal，为 `COMMANDS.md` 或 `scripts/check_docs_consistency.py` 加 option-level smoke：从 `cargo run --quiet -- <cmd> --help` 提取参数，验证关键 docs 不含未知 option。

收益/风险：收益中；风险低。

### B5. Medium：decision engine 存在但当前没有外部消费者

证据：`src/decision.rs:145-248` 实现 direction/review/both convergence；`src/event_emit.rs:428-455` 有 decision event；但 `src/cli.rs:154-174` 的 autopilot 参数没有 `--decide`，`src/commands/ops.rs:1095-1172` 的 autopilot path 也没有调用 decision engine。`rg -n "decision::|use crate::decision|crate::decision" src` 当前无命中，说明该 engine 没有外部 consumer；现有覆盖主要是 `src/decision.rs` 内部单元测试，缺少产品路径集成测试。

改进方向：host 需要先裁决：保留 decision engine 作为内部库、删除/归档未接线代码，还是恢复 `autopilot --decide`。若恢复，必须加 help、CLI dispatch、integration tests、docs、budget cap、heterogeneous runner gate，避免只靠内部单测证明产品可用性。

收益/风险：收益取决于真实使用频率；风险中高，容易把 LTO 从 harness 推向 planner。

## C. 插件系统如何生效

### C1. 边界

插件是 data-only path plugin。它能提供：

- source notes；
- path/playbook JSON；
- runtime profiles；
- prompt suffix；
- output schema；
- eval packs。

它不能：

- 执行任意代码；
- 自己选择工作流；
- 自动提升权限；
- 自动 promotion；
- 代替 host 做最终判断。

代码证据：

- manifest 安全字段：`src/plugin.rs:42-65`。
- `security.executable_code` 必须 false：`src/plugin.rs:178-185`。
- profile sandbox 不能超过 plugin `max_sandbox`：`src/plugin.rs:447-485`。
- mount 只写 provenance lock：`src/plugin.rs:105-147`。
- eval-run 读取 mount sandbox，未 mount 时降级 read-only 并 warning：`src/plugin_eval_run.rs:86-148`。

### C2. 命令面与数据流

| 命令 | 作用 | 数据流 |
|---|---|---|
| `plugin list` | 发现 repo 内插件 | 扫 `plugins/*/plugin.json`。当前实证 6 个插件。 |
| `plugin validate <dir>` | 校验 manifest、引用文件、profile、eval pack | 读 plugin tree，不写。 |
| `plugin render-profile <dir> <profile>` | 将 base prompt + profile suffix 渲染成普通 prompt | 读 profile/prompt/schema，写 output 和可选 meta。 |
| `plugin eval <dir>` | 静态检查 eval pack | 读 eval pack，验证 case/profile/metrics 引用。 |
| `plugin mount <dir>` | 把插件启用 provenance 写入 run | 写 `.lto/<run-id>/plugin-mounts.json`，不改插件本身。 |
| `plugin source-note <dir>` | 创建 source note，并可 append manifest | 写 `sources/*.note.json`，可更新 `plugin.json` 的 `source_notes`。 |
| `plugin eval-run <dir>` | 真实 baseline vs candidate A/B | 编译 baseline/candidate AgentJob，经 scheduler 跑 case，输出 deterministic metrics 和 report。 |

### C3. 本次实跑示例

使用现成插件 `plugins/adversarial-audit`：

```bash
cargo run --quiet -- plugin validate plugins/adversarial-audit --json
cargo run --quiet -- plugin render-profile plugins/adversarial-audit codex-refuter-v1 \
  --input .lto/20260617-rust-inheritance-readme-investigation/plugin-profile-input.md \
  --output .lto/20260617-rust-inheritance-readme-investigation/plugin-profile-rendered.md \
  --meta-output .lto/20260617-rust-inheritance-readme-investigation/plugin-profile-rendered.meta.json \
  --json
cargo run --quiet -- plugin eval plugins/adversarial-audit --json \
  --output .lto/20260617-rust-inheritance-readme-investigation/plugin-static-eval-adversarial-audit.json
cargo run --quiet -- plugin mount plugins/adversarial-audit \
  --run-id 20260617-rust-inheritance-readme-investigation
cargo run --quiet -- plugin eval-run plugins/adversarial-audit \
  --run-id 20260617-rust-inheritance-readme-investigation \
  --case agy-refute-adversarial-path --no-persist --json \
  --output .lto/20260617-rust-inheritance-readme-investigation/plugin-eval-run-adversarial-negative.json
```

结果：

- validate：`ok=true`，manifest hash `sha256:cfae8b12d3b85897a79e88f37ddbe46ebcf1eb924614fe04d822619de20e8124`。
- render-profile：生成 1400 bytes prompt；meta 显示 permission `read-only` 和 output schema `schemas/findings.json`。
- static eval：`ok=true`，eval pack `adversarial-audit-cases-v1` 有 4 个 cases。
- mount：写入 `.lto/20260617-rust-inheritance-readme-investigation/plugin-mounts.json`。
- eval-run：negative case `agy-refute-adversarial-path` `ok=true`，scheduler validate 阶段拒绝 `agy cannot enforce read-only; defer it for read-only jobs`，没有启动 agy，证明 fail-closed 路径生效。

### C4. 插件与预设工作流的关系

`workflow-playbook.md` 是 host-agent 调度先验；插件提供素材。二者不是“命令 vs 插件”的替代关系。

例子：

- `plugins/dev-workflow` 提供 `feature-dev-main.json`、`docs-sync-loop.json`、`direction-review.json`、`enterprise-audit-gate.json`，对应 workflow-playbook 中的 feature-dev、docs-sync、direction-review、enterprise-audit。
- host 可以 `plugin mount plugins/dev-workflow` 后读取 path/profile/eval 素材；LTO 不会因为 mount 自动启动工作流。
- `plugins/adversarial-audit` 可辅助 review / feature-dev 的 co-design 或 impl-audit，但是否挂载由 host 决定。

## D. 预设工作流清单

定位：这些是 host-agent 调度先验，不是硬路由命令。使用方式是：host 读 playbook，选择 `runner` / `audit` / `judge` / `next` / `autopilot` / `recap` 等 primitive，并把产物登记到 `.lto/`。

| 工作流 | 何时用 | 关键 primitive | 期望产物 | 停止条件 |
|---|---|---|---|---|
| review | spec/code/设计涉及 auth、payment、migration、schema、security、concurrency、external API，或 closeout 前有高风险未审 | `audit --auto-dispatch`, `audit --discover-risks`, `judge`, `check` | audit brief、heterogeneous replies、findings JSON、audit ledger、judge verdict | high/critical blocker 到 0，或 human override 并记录理由 |
| enterprise-audit | 高风险变更、大厂标准/全流程审计、普通 review 覆盖不了上下游风险 | `plugin mount plugins/dev-workflow`, layer auditor profiles, audit ledger, direction-review | layer scope matrix、per-layer findings、redline register、rollback/acceptance evidence | mandatory layers 有证据，HIGH/CRITICAL redline 到 0 或 human override |
| debug | task blocked，已有失败命令/log/stderr/复现步骤，同失败指纹反复出现 | `runner`, `next`, `autopilot --supervised`, 手动 fan-out 诊断 | 最小复现、stdout/stderr、假设排除记录、修复后验证命令 | 根因有证据且修复验证通过，或留下明确 next diagnostic/human question |
| migration | 跨模块/schema/API/shared state/持久化格式，需要兼容期/rollback/批量分片 | `task add`, `runner`, `run parallel`, `run pipeline`, `audit`, `check --to closed --strict` | migration plan、slice task list、per-slice evidence、compat/rollback notes、audit ledger | slice 均 done/skipped 且理由清楚；兼容/回滚/测试证据当前于 HEAD |
| claim-verify | 文档/spec/研究输出含事实、版本、引用、API 行为、价格、政策声明 | claim table, local/web/context7 verification, `runner --kind manual`, `audit` | claim ledger、source map、supported/refuted/unknown verdict | 每个 material claim 有 verdict；unknown 不改写成确定口吻 |
| research | 多源研究、路线比较、选型、市场/生态判断 | 分源检索、fan-out research、manual evidence、source critique | source notes、contradiction ledger、synthesis memo、confidence labels | 关键来源覆盖达标，重大矛盾解决或披露，区分 fact/inference/recommendation |
| feature-dev | 新需求/新功能从零开始，可能产生新模块或对外行为 | `start`, `task add`, dev gate evidence, `plugin mount plugins/dev-workflow`, `runner`, `audit`, `judge`, `closeout` | spec v2、四项开发证据、实现证据、findings union、test-pin、验收六门、changelog | 六条验收门同时满足，豁免有理由，审计收敛，文档/观测/沉淀完成 |
| tmux-goal-loop | 大 goal 需要 host 合议目标、短会话 worker 长跑、host 亲验 | `start` goal 四件套, `task add`, `runner --runner tmux`, `autopilot --worker-runner tmux`, `audit`, `check`, `closeout` | worker live log、completion contract、host triage note、audit ledger、host 亲验记录 | worker 自述不能当完成；host 一手核验 goal/task/源码/产物/红线 |
| docs-sync | 代码大改后、用户指出文档过时、changelog 与文档口径不一致 | docs drift fan-out, `runner --kind manual`, anti-drift test-pin | drift findings union、逐条修复 diff、防 drift 测试 | union 逐条处理完，防 drift 测试落地并通过 |
| release | 版本定版、公开仓库同步、向他人交付 | changelog 定版、privacy self-check strict、`closeout`, human push gate | changelog 段、隐私自检输出、handoff、push 确认记录 | strict privacy 通过或降级获接受；push 人工确认；沉淀完成 |
| direction-review | 架构边界/方向/品味分歧，或审计方出现非事实性矛盾 | disagreement classification, evidence adjudication, human escalation, decision log | 分歧分类、各方立场与证据、决策档 | 证据分歧由证据裁决；品味分歧由人类拍板；needs_human 不被多数票否决 |

## Backlog / 后续建议

| 优先级 | 建议 | 类型 | 本任务是否处理 |
|---|---|---|---|
| High | 修正文档中 `--decide`、`--collect`、budget caps/extend 的当前可用性口径 | 文档澄清 | 是，限文档 |
| High | 若需要 `autopilot --decide`，另立 goal 接回 `src/decision.rs` 并补 CLI/tests/docs | 功能实现 | 否 |
| High | 拆分 `src/commands/ops.rs` 与 `src/cli.rs` | 架构重构 | 否 |
| Medium | 为 docs consistency 增加 option-level smoke | 测试/工具 | 否 |
| Medium | 拆分 `plugin_eval_run.rs` 的 compile/metrics/report | 架构重构 | 否 |
| Medium | 裁决 `audit --collect` 是恢复、替换为 `collect-agent-run`，还是正式移除文档 | 产品决策 | 文档中先按当前 CLI 澄清 |
