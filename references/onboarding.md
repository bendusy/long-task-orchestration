# LTO 装载手册 — 给 agent 读的一份文档

> 你是一个 agent（codex / pi / agy / claude / 任意能跑 Rust/Cargo + bash 的 runtime）。
> 读完这一份，你就知道 LTO 是什么、怎么装给自己用、长任务里怎么用它不迷路。
> 细节按需再读：路由见 [INDEX.md](INDEX.md)，命令面见 [COMMANDS.md](../COMMANDS.md)。

## 一句话

**LTO 是一个跨 runtime 的长任务 harness：它给你 state、artifact、runner、audit、sandbox、resume/recap 和 gate。你这个 host agent 仍然是 planner；LTO 让你的选择可恢复、可审计、可自动化推进。**

当前接管线是 Rust v2；Python fallback 已在 v0.5.0 退役。

## 术语表（读正文前先扫一遍）

| 术语 | 一句话解释 |
|---|---|
| **harness** | 框架本身。把状态、证据、审计、沙箱、恢复、人工闸门串成一条「跑道」。 |
| **host agent（宿主）** | **就是你**——正在用 LTO 的这个 LLM。LTO 不替你做决策，你才是 planner。 |
| **primitive** | 可组合基础动作：`runner`/`judge`/`audit`/`next`/`recap` 等。拼装，而非选预设流程。 |
| **run / task / phase** | 一次长任务（`.lto/<run-id>/`）/ run 里可独立执行的单元 / 任务的逻辑阶段。 |
| **artifact** | task 产出的证据：日志、diff、审计报告。「说做完了」得拿得出 artifact。 |
| **runner** | 派出去执行一个 task 的角色（codex/pi/agy/claude，或一条 shell 命令）。 |
| **异构 / fan-out** | 审你的 agent 来自不同厂商家族（避免自审）/ 一次派多个独立 agent 并行。 |
| **audit / 收敛 / ledger** | 派异构 agent 挑毛病出结构化 findings；blocker 逐轮登记进 ledger，计数由脚本算收敛。 |
| **closeout / 闸门** | 任务闭环（handoff+CHANGELOG）；关键路口硬检查，未审风险会被拦。 |
| **worktree 沙箱** | 独立 git worktree 副本，autopilot 自动执行只在这里跑，主工作树无恙。 |
| **autopilot 档** | `--supervised`（只出建议）→ `--auto-exec`（沙箱跑 safe 子步骤）→ `--autonomous`（攒够证据后机械推进）。 |
| **resume vs recap** | `resume` 给 AI 拉上下文；`recap` 给人看的人话回顾。 |
| **`.lto/` 目录** | 本项目所有 run 的落盘地。**没装 am 时它就是本地记忆**，永远是本项目真源。 |

## 进项目第一件事：看 `.lto/` 了解 LTO 历史

接手项目先跑 `lto runs`——列出本项目所有 run（目标/阶段/进度/current）。

```bash
lto runs                    # 本项目所有 run 概览（newest first）
lto resume                  # 拉当前 run 的上下文（给 AI）
lto recap                   # 当前 run 的人话回顾（给人）；跨 run 挖掘用 recap --mine
# 想看某个具体 run：读 .lto/<run-id>/{state.json, handoff.md, run-state.md}
```

不看 `.lto/` 就开工 = 丢掉本项目全部历史经验、重复踩坑。装了 am（animem）时 run
成果可 publish 到 am 做跨项目记忆；am 只是 `.lto/` 的下游投影。

## 它解决什么（你为什么要装它）

| 通病 | 现象 | LTO 的防线 |
|---|---|---|
| agentic laziness | 50 项做了 20 项就宣布完成 | risk coverage 闸门 + ledger 收敛：没审完不让 closeout |
| self-preferential bias | 自审等于没审 | `lto audit` 强制异构审计（审者 runtime ≠ 你这家） |
| goal drift | 多轮/compaction 后丢目标 | state.json 持久化 + `lto resume` 跨 session 拉回 |

## 怎么装、怎么调

前提：Rust stable + Cargo、bash、git；异构派工需本机至少装两家 agent CLI
（repo 自带 `scripts/delegate/`）。安装细节（软链路径、wrapper 生成、校验）见
[INSTALL.md](../INSTALL.md)：

```bash
bash scripts/install.sh          # 软链到 ~/.claude/skills + ~/.agents/skills，并生成 lto wrapper
bash scripts/install.sh --check  # 只检查不装
```

LTO 是纯 CLI，任何能跑 bash 的 runtime 都能调：

```bash
lto --repo <目标仓库> <子命令> [参数]
# 未装 wrapper 时：cargo run --manifest-path <skill-root>/Cargo.toml -- --repo <目标仓库> <子命令>
```

跨 runtime 当宿主的专项坑（沙箱、派工、preflight）见
[cross-runtime-host-notes.md](cross-runtime-host-notes.md)。

## 命令速查

命令面以 [COMMANDS.md](../COMMANDS.md) 为准（真源 `src/cli.rs`，checker 强制同步）。
注意两条易混：`runner`/`run parallel`/`run pipeline` 编排 **shell 命令**；真正的
**agent fan-out** 走 `audit --auto-dispatch`（派异构审计）和 `audit --discover-risks`
（对抗找漏登记的风险）。手动派工的 reply 用 `collect-agent-run` 登记进 state，
recap/closeout 才看得见。细节见 [execution-loop.md](execution-loop.md) 与
[audit-convergence.md](audit-convergence.md)。

## 自驱动与回顾（要点）

- `autopilot --supervised`（默认出简报回吐你判断）→ `--auto-exec`（safe/reversible
  子步骤在 worktree 沙箱自动跑，危险命令 HELD 回吐人）→ `--autonomous`（机械证据
  闸门：攒够真实派工才解锁，不 spawn 决策 agent，反思永远归你）。详见
  [run-state-workflow.md](run-state-workflow.md)。
- `recap` 对抗**人**的 goal drift：当初要做啥/为什么/跑多久/做到哪/还剩啥/轮到你。
  开 run 时记 `--why/--done-when`，recap 才答得全。`resume` 距上次活动 >24h 会提示跑 recap。

## 最小跑通流程（照着做）

```bash
L="lto --repo ."

# 1. 开工，记下目标
$L start --goal "重构登录模块，消除空指针" --host <你这家:codex/pi/agy/claude>

# /goal 型长交付，把交付契约落进 Rust core state
$L start --goal "提升检索召回" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h" \
  --instrument "python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"

# 2. 加任务（task 是 runner/next/audit 的操作对象，先建出来）
$L task add --task-id T1 --title "给 login 加判空" --command "pytest tests/test_auth.py -x"

# 3. 干活：执行 task + 落证据
$L runner --task-id T1 --kind test --command "pytest tests/test_auth.py -x" --note "验证空指针修复"

# 4. 高风险？派异构对抗审计；手动派工的 reply 用 collect-agent-run 登记
$L audit --auto-dispatch
$L collect-agent-run --task-id T1 --runner codex --reply reply-codex.md

# 5. 开始写代码前，先看 entry evidence（不自动批准，仍要人拍板）
$L check --to implementation

# 6. 迷路了？问 LTO 下一步（零 LLM 事实简报）
$L next

# 7. 收尾前预查 required evidence，然后收尾
$L check --to closed --strict
$L closeout --summary "登录模块重构完成，空指针已修，异构审计收敛"
```

跨 session 回来：`$L resume`。多 runtime / 多项目接手：`$L memory resume --project
<repo-key>`（只读，am 缺席时降级本地 `.lto`，不覆盖 current/state）。想看会写入
记忆层的内容先 `$L memory export --run-id <id> --dry-run`（纯本地 redacted JSON）；
`memory publish` 走 am 原生 CLI（唯一 sink，早期 memory-flow REST 已移除），am
缺席时报错并提示本地 `.lto` 仍是真源。

## hook：让你别忘了用 LTO

hook 是 commit/deploy/closeout 前的边界闸门（测过了吗/审过了吗/有没有未解决的
block）。**opt-in、按需手跑**：`lto hook <gate> [--force] [--reason]`，CLI 不写入
`.git/hooks`——LTO 不擅自动你的 git。详见 [hooks.md](hooks.md)。

## 还想深入

按域路由与全部文档的状态/权威标注见 [INDEX.md](INDEX.md)；LTO 是什么、为什么这么
设计见 [../SKILL.md](../SKILL.md)。
