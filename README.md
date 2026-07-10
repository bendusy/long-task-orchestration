# LTO: 长任务 harness

LTO 是给主 agent 用的本地 CLI：它不替 agent 规划路线，不替 agent 写代码，而是把长任务的状态、证据、审计、runner 派工、沙箱、resume/recap 和 closeout 闸门落到 `.lto/`。适合几十轮会话、跨天交付、需要异构审计和可恢复证据的工作。

## 30 秒看懂

| 问题 | LTO 的回答 |
|---|---|
| LTO 是什么 | 外置记忆 + 质检 + 刹车。主 agent 仍是 planner，LTO 负责让过程可恢复、可审计、可验证。 |
| 真源在哪里 | `.lto/<run-id>/state.json`、artifacts、runner logs、audit ledger。am 只是可选投影，本地 `.lto/` 永远是项目真源。 |
| 怎么防跑偏 | start 写目标和交付契约，task 记录拆分，runner 写证据，next/resume/recap 拉回上下文，closeout 前检查红线。 |
| 怎么防自审 | `audit --auto-dispatch` 和 `audit --discover-risks` 派异构 runner 挑问题；runner 不健康时 fail-closed。 |
| 什么时候不用 | 小 bug、一行改动、普通 review、单次部署，不该套 LTO。 |

术语细节见 [references/onboarding.md](./references/onboarding.md)，完整命令面见 [COMMANDS.md](./COMMANDS.md)。

## 最小可跑路径

```bash
L="cargo run --quiet --"
# 安装 wrapper 后也可以用：
# L="lto"

# 1. 开工：目标、原因、完成标准
$L start --goal "重构登录模块" --why "线上空指针崩溃" --done-when "测试全绿+审计收敛"

# /goal 型长交付：交付契约进 Rust core state
$L start --goal "提升检索召回" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h; paid API <= $50" \
  --instrument "python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"

# 2. 建 task，再执行并落证据
$L task add --task-id T1 --title "给 login 加判空" --command "pytest tests/test_auth.py -x"
$L runner --task-id T1 --kind test --command "pytest tests/test_auth.py -x" --note "验证空指针修复"

# 3. 派外部 agent 跑长任务，别轮询——阻塞等完成事件（v0.6.1+）
$L dispatch-goal --runner codex --goal goal.md   # 默认在当前 tmux 会话开窗，不用传 --new-window
$L events --wait --event-type agent.dispatch.completed --timeout 1800   # goal/进程真完成后唤醒

# 4. 迷路时看事实简报
$L next
$L resume
$L recap

# 5. 高风险时异构审计（--prefer-runner 把慢 runner 挪出收口关键路径）
$L audit --auto-dispatch --prefer-runner codex --prefer-runner agy
$L audit --discover-risks

# 6. 收尾前硬检查，再 closeout
$L check --to closed --strict
$L closeout --summary "登录重构完成，测试和异构审计已收敛"
```

`audit` 当前命令面是 `--auto-dispatch`、`--discover-risks`、`--allow-same-family`、`--prefer-runner`。历史文档里出现过的 `audit --collect <dir>` 不是当前 Rust CLI 命令；已有回复应通过当前 `runner`/`collect-agent-run`/artifact 机制登记。

`dispatch-goal` 新建的窗口默认命名为 `lto:<runner>:<goal-slug>`，程序寻址与清理只使用 tmux 不可变 `@window_id`。成功完成后自动清理；非零 rc、超时、交互阻塞或 `--keep-window` 都保留现场。Codex 的 Stop hook 只是每轮结束：普通 Stop 只写 `agent.turn.completed`，只有 transcript 中存在真实 `update_goal complete` 证据才写 `agent.dispatch.completed`；pi/agy 则由 TUI 进程退出 wrapper 传真实 rc。不要再用 turn 事件判断整个 goal 完成。

## 架构全貌

```text
host agent = planner
  |
  v
lto-rs CLI primitives
  |-- state/task/phase/check/closeout
  |-- runner/scheduler/tmux/worktree sandbox
  |-- audit/judge/heterogeneous runner selection
  |-- plugin data packs and eval-run
  |-- events/telemetry/budget readouts
  |
  v
.lto/<run-id> = local source of truth
  |-- state.json / run-state.md / artifacts.json
  |-- live logs / replies / audit ledger / handoff
  v
optional sinks: am memory publish/resume, release docs, changelog
```

核心边界：

- **Rust v2 是唯一支持的 CLI runtime**。Python fallback 已在 v0.5.0 删除；保留的 `scripts/*.py` 是文档检查、ledger 检查、ADR 写入等维护工具，不是运行时 fallback。
- **LTO 是 harness，不是 planner**。`next`、`recap`、`autopilot --supervised` 只整理事实；路线判断仍由 host agent 和人决定。
- **runner/audit/worktree 是 affordance**。它们提供派工、异构检查和可弃沙箱，不改变最终责任归属。
- **macOS/Linux 优先**。Windows native support 暂停；当前内置 runner 协议依赖 `scripts/delegate/runners/*.sh` 和 `healthcheck.sh`，WSL/Unix-like shell 属于用户侧环境验证。

### 放到业界 loop 工程坐标里看 LTO

业界把 agent harness 看成可叠加的四层 loop（LangChain "loop engineering" / swyx "loopcraft"）。LTO 是一个**覆盖 L1–L4 的长任务 harness**，各层状态如下：

| Loop 层 | 是什么 | LTO 落点 | 状态 |
|---|---|---|---|
| **L1 Agent** | model 调 tool 循环到完成 | `runner` / `scheduler` / `agent_job`（dumb loop，智能在 model/host） | ✅ |
| **L2 Verification** | grader 检查输出、不达标反馈 | `audit --auto-dispatch`（**跨族异构** runner 互审）+ `judge` + `check` gate | ✅ **差异化**：业界多用单模型 LLM-as-judge，LTO 用异构 runner 跨族互审，抗同族盲区 |
| **L3 Event-driven** | 事件触发 agent 后台跑，非手动调 | `events.jsonl` + `dispatch-goal`；turn 与 dispatch 完成分事件，codex 用 goal-state proof，pi/agy 用真实进程 rc | ✅ 已实现 |
| **L4 Hill-climbing** | 扫历史 trace、改进 harness 自身 | 跨 run 数据挖掘（按 runner 模型 × 任务 × 时间聚合，只读喂回 host 出 tuning brief） | ✅ 已实现（`recap --mine`） |

**贯穿原则——薄 harness + 人在环**：LTO 赌「模型变强、harness 变薄」（primitive 不硬路由、preset 是 host playbook 不是固定菜单）。L4 与业界关键分歧是：LTO 挖掘出的是**证据和 brief**，喂 host 决策，**绝不自动改写 harness / 自动 promote**——所有路线判断和敏感操作（`git push`、closeout）都回到 host 和人。这与 Anthropic「薄 harness」、各家「human oversight at every level」一致，LTO 把人在环守得更严。

## 插件系统怎么用

插件是 data-only path plugin：它可以提供 source note、path/playbook JSON、runtime profile、prompt suffix、output schema 和 eval pack；不能执行任意代码、自动提升权限、替 host 选 workflow 或自动 promotion。边界见 [references/plugin-boundary.md](./references/plugin-boundary.md)，真实 A/B eval 设计见 [references/plugin-real-eval-runner.md](./references/plugin-real-eval-runner.md)。

当前仓库有 5 个插件：

- `plugins/adversarial-audit`
- `plugins/claim-verify-research`
- `plugins/deep-agent-profiles`
- `plugins/dev-workflow`
- `plugins/migration-refactor`

最小生命周期示例，以下命令已用 `plugins/adversarial-audit` 实跑验证：

```bash
$L plugin list
$L plugin validate plugins/adversarial-audit --json
$L plugin render-profile plugins/adversarial-audit codex-refuter-v1 \
  --input .lto/<run-id>/plugin-profile-input.md \
  --output .lto/<run-id>/plugin-profile-rendered.md \
  --meta-output .lto/<run-id>/plugin-profile-rendered.meta.json \
  --json
$L plugin eval plugins/adversarial-audit --json \
  --output .lto/<run-id>/plugin-static-eval-adversarial-audit.json
$L plugin mount plugins/adversarial-audit --run-id <run-id>
$L plugin eval-run plugins/adversarial-audit \
  --run-id <run-id> \
  --case agy-refute-adversarial-path \
  --no-persist --json \
  --output .lto/<run-id>/plugin-eval-run-adversarial-negative.json
```

数据流是：validate 检查 manifest 和引用文件，render-profile 编译 profile prompt，eval 静态校验 eval pack，mount 只在 run 里写 provenance lock，eval-run 通过 scheduler 跑 baseline vs candidate 并输出 report。

场景插件只在 host 明确选择后挂载，不会自动改 runner 权重或启动 workflow：

```bash
$L plugin mount plugins/adversarial-audit --run-id <run-id>       # review / feature-dev refute-first 审计
$L plugin mount plugins/claim-verify-research --run-id <run-id>  # claim-verify / research 证据完整性核验
$L plugin mount plugins/migration-refactor --run-id <run-id>     # migration 分批回归与语义等价审计
```

## 预设工作流

预设工作流是 host-agent 调度先验，不是 `lto workflow run X` 这种硬命令。host 读 playbook 后组合 `start`、`task`、`runner`、`audit`、`judge`、`next`、`recap`、`closeout` 等 primitive。完整说明见 [references/workflow-playbook.md](./references/workflow-playbook.md) 和 [references/dev-workflow-spec.md](./references/dev-workflow-spec.md)。

| 工作流 | 何时用 | 关键 primitive | 期望产物 |
|---|---|---|---|
| `review` | 高风险 spec/code/设计需要异构审计 | `audit`, `judge`, `check` | audit brief、findings、ledger、verdict |
| `enterprise-audit` | 多层级/大厂标准审计 | `plugin mount plugins/dev-workflow`, audit ledger | layer findings、redline register、验收证据 |
| `debug` | 同一失败反复出现或 task blocked | `runner`, `next`, `autopilot --supervised` | 最小复现、日志、根因、修复验证 |
| `migration` | 跨模块/schema/API/持久化格式迁移 | `task`, `runner`, `run parallel`, `run pipeline`, `audit` | 分片计划、兼容/回滚证据、per-slice evidence |
| `claim-verify` | 文档、研究、版本/API/事实主张要核验 | source notes, `runner --kind manual`, `audit` | claim ledger、supported/refuted/unknown verdict |
| `research` | 多源研究、选型或生态判断 | 分源检索、manual evidence、source critique | source map、矛盾表、confidence labels |
| `feature-dev` | 新功能从 spec 到实现和验收 | `start`, `task`, `runner`, `audit`, `judge`, `closeout` | 开发四证据、实现证据、测试、changelog |
| `tmux-goal-loop` | 长跑 worker + host 亲验 | `runner --runner tmux`, `autopilot --worker-runner tmux` | live log、worker contract、host verification |
| `docs-sync` | 代码大改后文档漂移 | docs drift fan-out, manual evidence | drift union、修复 diff、防漂移测试 |
| `release` | 版本定版和公开交付 | `release --dry-run`, `check`, `closeout` | release plan、privacy scan、handoff |
| `direction-review` | 架构方向或品味分歧 | evidence adjudication, decision log | 分歧分类、证据裁决、人类拍板点 |

## Rust 迁移与 release 口径

`lto-rs` 当前版本走 Rust-only path：source build 用 `cargo`，安装后的 `lto` wrapper 执行 `lto-rs`，Python fallback 不再存在。二进制下载是 release-gated：下载前必须实查 GitHub Releases 是否有对应 `.tar.gz` 和 `.sha256`，校验 checksum 后再运行 `./lto-rs self-test`。不要在未查 live release assets 前声称 GitHub 已有可下载 Rust 二进制。

Release 流程归 host，按确定性 SOP 走：完整步骤见 [references/release-workflow.md](./references/release-workflow.md)，发版前必跑 `bash scripts/release_preflight.sh --version X.Y.Z`（检查版本三处一致 / 隐私扫描 / CI 全部红线 / self-test，全绿才发）。概要：写人话 CHANGELOG → 同步三处版本（Cargo.toml/VERSION/Cargo.lock）→ preflight → commit/push → tag push 触发 CI `release-binaries` 构建 3 平台二进制并上传。更完整的公开交付门槛见 [references/open-source-delivery-requirements.md](./references/open-source-delivery-requirements.md) 和 [references/rust-migration-release.md](./references/rust-migration-release.md)。

## 什么时候不该用 LTO

- 修一个小 bug、改一行、一次性脚本：直接做。
- 只想让别人 review 一下：走普通 review。
- 只是部署一下：走部署流程，必要时再用 LTO 登记验证证据。
- 没有跨轮状态、异构审计、沙箱、resume/recap、closeout gate 的需求：LTO 只会增加负担。

## 安装与深入阅读

安装见 [INSTALL.md](./INSTALL.md)。常用入口：

| 想了解 | 读这里 |
|---|---|
| 完整命令面 | [COMMANDS.md](./COMMANDS.md) |
| agent 手册和术语 | [references/onboarding.md](./references/onboarding.md) |
| 设计哲学 | [SKILL.md](./SKILL.md) |
| 工作流 playbook | [references/workflow-playbook.md](./references/workflow-playbook.md) |
| 插件边界 | [references/plugin-boundary.md](./references/plugin-boundary.md) |
| 真实 eval-run | [references/plugin-real-eval-runner.md](./references/plugin-real-eval-runner.md) |
| Rust 迁移/release | [references/rust-migration-release.md](./references/rust-migration-release.md) |
| Rust/Python ownership | [references/python-rust-ownership.md](./references/python-rust-ownership.md) |
| 本次继承和架构调查 | [references/2026-06-17-rust-inheritance-and-architecture-review.md](./references/2026-06-17-rust-inheritance-and-architecture-review.md) |
