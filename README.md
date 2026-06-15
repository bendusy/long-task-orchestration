# LTO：长任务 harness

> 做一个大功能要几十轮对话，做着做着就忘了目标、跳过验证、自己审自己还说没问题。
> **LTO 给主 agent（你这个 AI）一条带护栏的跑道**：状态存得下、进度恢复得了、产出有异构 AI 帮你审、危险操作进沙箱、关键路口能踩刹车。

## 30 秒看懂

**它是什么**：一个 CLI 工具，给跑长任务的 AI agent 当「外置记忆 + 质检 + 刹车」。

**它不是什么**：不替你写代码，也不替你选路径。**你仍然是 planner**，LTO 只让你的每一步可恢复、可审计、可安全自动推进。

**它解决的三个老毛病**（任何 agent 单 context 跑长任务都会犯）：

| 老毛病 | 现象 | LTO 的防线 |
|---|---|---|
| **偷懒** | 50 项做了 20 项就宣布完成 | 没审完不让收尾（closeout 闸门） |
| **自我偏袒** | 自己审自己，永远判「没问题」 | 强制派**不同厂商**的 AI 来审（异构审计） |
| **跑偏** | 几十轮后忘了最初要干嘛 | 状态落盘 + 一句话拉回目标（resume/recap） |

> 名词卡住了？所有术语（harness / 异构 / runner / phase / artifact / closeout / autopilot 档 …）在 **[references/onboarding.md](./references/onboarding.md)** 开头有一张速查表。

## 最小可跑路径

照着这 5 步就能跑通一个完整长任务。`L` 是 CLI 入口的简写。

> 先认两个词：**task** 是「要做什么」的定义，**runner** 是「执行它一次」的动作——一个 task 可以 run 多次（失败重试、换参数）。每次 run 的结果落进状态，后面 `next` / `audit` / `closeout` 都读这些结果。



```bash
L="cargo run --quiet --"         # Rust v2 当前接管线
# 装过 install.sh 且已 cargo build --release --bin lto-rs 后，也可用：
# L="lto --use-rust"

# ① 开工：记下目标、为什么做、做完的标准（recap 会用到）
$L start --goal "重构登录模块" --why "线上空指针崩溃" --done-when "测试全绿+review过"

# ② 加一个任务，然后执行它（task 要先建，runner 不会自动建）
$L task-add --task-id T1 --title "给 login 加判空" --command "pytest tests/ -x"
$L runner  --task-id T1 --kind test

# ③ 迷路了就问：现在该干什么？
$L next        # 它读状态，给你一份「下一步」事实简报（它只摆事实，你来决定）

# ④ 让不同厂商的 AI 帮你审（自动派 codex/pi/agy 三家）
$L audit --auto-dispatch

# ⑤ 收尾（有没审完的风险会被拦住）
$L closeout --summary "登录重构完成，空指针已修，异构审计已收敛"
```

**跨天回来接着干**：`$L resume`（给 AI 拉上下文）或 `$L recap`（给人看人话回顾）。

> 想看全部 24 个命令和参数摘要，先看 **[COMMANDS.md](./COMMANDS.md)**；想看先后关系、autopilot 自动化怎么用，去 **[onboarding.md](./references/onboarding.md)**，那是给 agent 读的完整手册。

## v0.3.0 新增（2026-06-09）

这一版的主题是让 LTO **越用越聪明**——但聪明的是你这个主 agent，不是 LTO。它只机械摆数据，判断永远归你：

- **跨 run 数据挖掘**（`recap --mine`）：扫历史所有 run，机械算出「哪个 AI 模型在哪类任务上真有效、哪个阶段总卡壳」，细到能区分同一个 pi 跑 deepseek 还是 glm。**只摆事实和假设，不替你拍板该用谁。**
- **全程留痕**（`events.jsonl`）：每个派工边跑边写黑匣子日志，派生出用量、耗时、人工干预记录。卡住了直接 `tail` 看，不用干等 timeout。这是「越用越聪明」的证据地基。
- **autonomous 档落地**：autopilot 多放一档权，但**不是全自动**——它先查跨 run 数据攒够没（没攒够就诚实退回半自动），攒够了才在沙箱里机械跑安全可逆的小步。要判断、要推代码、碰 `git push`，永远停下问你。

完整技术条目见 [CHANGELOG.md](./CHANGELOG.md)。

## Rust v2 轨道（当前接管线）

`lto-rs` 是按 2026-06-15 v2 spec 落地的 Rust 核心轨道，也是接下来接管旧 Python CLI 的主线。当前状态是：Rust workspace、24/24 命令真实现、runner event parser、state/budget、scheduler typed core、worktree 沙箱、dispatch/merge-review/audit/decision/plugin 的核心类型、`plugin mount` data-only provenance、COMMANDS.md 和回归测试已建立。

当前 Rust v2 支持面聚焦 macOS 和 Linux。Windows 二进制与 runner 派工支持先暂停：内置 delegate runtime 仍是 `scripts/delegate/runners/*.sh` + `healthcheck.sh` 的 shell 协议，先把 Rust 接管旧 Python 和核心代码清理做稳，再重新评估 Windows 原生 runner。

```bash
cargo test
cargo run -- self-test
cargo run -- check --run-id <run-id> --json

# 全局 wrapper 在兼容期仍可回退 Python；显式开启 Rust 轨道
LTO_USE_RUST=1 lto recap --run-id <run-id>
lto --use-rust check --run-id <run-id> --json
```

Rust 侧的原则是“黑盒行为对齐，内部 Rust-native”：外部兼容 `.lto/` 历史 state 和现有插件 JSON，内部用 enum/typed struct/trait/Result/serde flatten 固化不变量，不机械翻译 Python 模块边界。Python 入口保留为兼容 fallback；后续重点是缩小 wrapper 回退面、清理重复实现、再切默认入口。

从 Python 切到 Rust、二进制下载状态和 release 打包流程见 [references/rust-migration-release.md](./references/rust-migration-release.md)。截至 2026-06-16，GitHub Releases 还没有可下载二进制；下一次 `v*` tag 成功后才会由 CI 上传 macOS/Linux 包。

## 什么时候**不**该用

- 修个小 bug、改一行 —— 直接改，别套 harness。
- 让人审一下代码 —— 走你自己的 review 流程。
- 只是部署一下 —— 走部署流程。

LTO 是给**要来回折腾好几轮、你担心做过头或做着做着跑偏**的长任务用的。短平快的活儿套上它只是负担。

## 安装

把整个 `long-task-orchestration/` 文件夹放进你的 agent skills 目录即可。详见 [INSTALL.md](./INSTALL.md)。

## 深入阅读

| 你想了解 | 读哪份 |
|---|---|
| **术语表 + 完整命令手册 + 怎么装给自己用** | **[references/onboarding.md](./references/onboarding.md)** ← agent 先读这份 |
| LTO 是什么、为什么这么设计 | [SKILL.md](./SKILL.md) |
| host agent 的调度先验（review/debug/migration…） | [references/workflow-playbook.md](./references/workflow-playbook.md) |
| 预设场景插件（对抗审 / 主张核验 / 迁移闸门 / 开发链路，data-only） | [plugins/](./plugins/) + [references/plugin-boundary.md](./references/plugin-boundary.md) |
| 开发链路插件的设计与三方审收敛记录 | [references/dev-workflow-spec.md](./references/dev-workflow-spec.md) |
| 控制论 harness：run logs / telemetry / 闭环 | [references/control-loop-harness.md](./references/control-loop-harness.md) |
| 在 codex/pi/agy 当宿主的专项坑 | [references/cross-runtime-host-notes.md](./references/cross-runtime-host-notes.md) |
| 插件真实世界 eval-run 设计 | [references/plugin-real-eval-runner.md](./references/plugin-real-eval-runner.md) |
| Rust 迁移、二进制下载和 release 打包 | [references/rust-migration-release.md](./references/rust-migration-release.md) |
| 本机 AI coding 隐私自检 | [references/privacy-self-check.md](./references/privacy-self-check.md) |
| 装依赖 / 给朋友用 / 项目级注入 | [references/sharing-guide.md](./references/sharing-guide.md) |
