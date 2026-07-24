# LTO（Long Task Orchestration）

让 AI agent 干几天的大活时：不迷路、不做过头、不糊弄你。

LTO 是一个本地命令行工具（Rust 单二进制 `lto-rs`），配合主 agent（比如 Claude Code）使用。它**不写代码、不做决定**——它管账：目标是什么、做到哪了、证据在哪、谁审过、什么时候该停。所有状态落在项目里的 `.lto/` 目录，断了随时接得上，换个 agent 也接得上。

## 它解决什么问题

一个大功能要几十轮对话、跨好几天、派好几个 AI 干活，常见的翻车方式和 LTO 的对策：

| 翻车方式 | LTO 的对策 |
|---|---|
| 对话被压缩，agent 失忆重开 | `lto resume` 把目标、任务、进度一口气喂回去 |
| AI 说"做完了"，是真的吗 | 每步留证据（跑过的命令、返回码、产物），`lto check` 不见证据不放行 |
| 写代码的 AI 审自己的代码 | `lto audit` 强制派**别家模型**挑刺（Claude 写的就派 codex/pi 审），审不动就报错，不装样子 |
| 说好修 bug，回头重构了半个项目 | 开工先写死目标和完成标准（`--goal` / `--done-when`），收尾对着检查 |
| 任务派给另一个 AI 后两眼一抹黑 | `dispatch-goal` 在 tmux 开窗派活、可视可查，干完自动回报，`events --wait` 等信号不轮询 |
| 三个 AI 都说没问题，就真没问题？ | 关键节点（换阶段、上线、收尾）必须人拍板，AI 不能替你点头 |

## 30 秒上手

```bash
L="cargo run --quiet --"
# 安装 wrapper 后直接用：L="lto"

# 1. 开工：说清楚要干什么、为什么、做到什么程度算完
$L start --goal "重构登录模块" --why "线上空指针崩溃" --done-when "测试全绿+审计收敛"

# 2. 拆任务，干活时把证据记下来
$L task add --task-id T1 --title "给 login 加判空" --command "pytest tests/test_auth.py -x"
$L runner --task-id T1 --kind test --command "pytest tests/test_auth.py -x" --note "验证空指针修复"

# 3. 迷路了？三条命令找回状态
$L next      # 下一步该干什么（事实简报）
$L resume    # 喂给 agent 的恢复上下文
$L recap     # 给人看的进度回顾

# 3b. 只想查某个对象，别读整份 state
$L get task --status blocked      # 列表+过滤（加 --json 给 agent 吃）
$L describe task T1               # 单个任务的全部上下文

# 4. 高风险的活，派别家模型来审
$L audit --auto-dispatch
$L audit --discover-risks

# 5. 收尾前硬检查，过了才关单
$L check --to closed --strict
$L closeout --summary "登录重构完成，测试和异构审计已收敛"
```

`start` 只硬要求 `--goal` 和 `--done-when` 两项。想把交付目标写成可测量的硬指标（比如"召回率 ≥95%"），加 `--target` 和 `--instrument`（测量命令），`lto check` 会拿它当闸门。细节见 [run-state workflow](./references/run-state-workflow.md)。

## 派活给别的 AI

```bash
# 把 goal 文件派给 codex（也支持 pi/agy），在当前 tmux 会话开一个可见窗口干活
$L dispatch-goal --runner codex --goal goal.md

# 阻塞等它干完（agent 完成后会自己执行回报命令，不靠轮询）
$L events --wait --event-type agent.dispatch.completed --timeout 1800

# 一步到位的版本
$L dispatch-and-wait --runner codex --goal goal.md
```

几个实用细节：

- 长指令写进 goal 文件，粘贴给 agent 的只有一行短 prompt（"读这个文件并执行"）。
- 如果 goal 文件里没写完成回报命令，dispatch 会在同目录生成 `<名字>.dispatch.md`（原文 + 完成协议附录）派给 agent，原文件不动。
- 窗口干完自动清理；出错、超时、卡在交互确认时**保留现场**让你看。
- agent 干完但你想把它的回复登记成证据：`lto collect-agent-run --task-id T1 --runner codex --reply reply.md`。

## 它不做什么

- **不替你规划**。主 agent（和你）决定路线，LTO 只提供事实、证据和刹车。
- **不自动路由**。"哪个 runner 快就多派谁"这种正反馈它不做，历史数据只作参考展示。
- **没有 UI、没有后台服务**。全部是 CLI + 文件（`.lto/<run-id>/` 下的 state.json、events.jsonl、artifacts.json），产品边界就是这套文件协议。
- **不把 AI 判断当真相**。audit/judge 的结论是证据不是裁决，大问题必须逐条人工核实。

## 什么时候别用它

修一行的小 bug、普通 code review、一次性脚本、单次部署——直接干，套 LTO 只会更麻烦。只有当任务要跨很多轮、需要异构审计、需要可恢复状态和收尾闸门时，才值得开一个 run。

## 安装与版本

安装见 [INSTALL.md](./INSTALL.md)。macOS/Linux 优先，Windows 原生支持暂停。

二进制从 [GitHub Releases](https://github.com/bendusy/long-task-orchestration/releases) 下载（每个 tag 由 CI 构建 3 平台产物 + sha256）。注意：二进制下载是 release-gated——下载前先确认对应版本的 release 里真的有 `.tar.gz` 和 `.sha256` 资产，校验 checksum 后跑 `./lto-rs self-test` 验证，别假设"有 tag 就有二进制"。从源码跑：`cargo run --quiet -- <command>`。

发版流程（维护者）：见 [release-workflow](./references/release-workflow.md)，发版前必跑 `bash scripts/release_preflight.sh --version X.Y.Z`。

## 深入阅读

| 想了解 | 读这里 |
|---|---|
| 完整命令面 | [COMMANDS.md](./COMMANDS.md)（`lto --help` 是最终权威） |
| agent 手册和术语 | [references/onboarding.md](./references/onboarding.md) |
| 设计原则（为什么这么设计） | [SKILL.md](./SKILL.md)、[references/control-loop-harness.md](./references/control-loop-harness.md) |
| 工作流 playbook（review/debug/migration/研究等 11 种） | [references/workflow-playbook.md](./references/workflow-playbook.md) |
| 插件系统（data-only，不执行代码） | [references/plugin-boundary.md](./references/plugin-boundary.md) |
| 审计收敛怎么判 | [references/audit-convergence.md](./references/audit-convergence.md) |
| 开源交付门槛 | [references/open-source-delivery-requirements.md](./references/open-source-delivery-requirements.md) |
