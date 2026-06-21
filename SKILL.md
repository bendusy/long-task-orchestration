---
name: long-task-orchestration
description: >-
  长任务 harness——帮 host agent 在 spec→审计→开发→部署→实测→记录 六个阶段保留状态、
  证据、审计、恢复点和人工刹车。
  Use when 一个任务要好多轮才能完成（设计+开发+上线），你担心做过头或者做着做着跑偏了。
  Do not trigger on 修个小 bug（走 diagnose）、让别人审一下代码（走 review）、
  只部署一下（走 ship）。普通「起个 spec / 做个功能」走对应专职 skill，
  只有当任务已经来回折腾好几轮、需要有人帮你踩刹车时才进 LTO。
metadata:
  tier: agent-driven
  domain: infra
  optional_integrations: [am, memory-flow]
  status: active
allowed-tools: [Bash, Read, Write, Edit, Task, AskUserQuestion]
---

# LTO：长任务 harness

做一个小功能只要 5 分钟。做一个大功能可能要 50 轮对话——中间你会迷路：不知道做到哪了、不知道是不是做过头了、不知道什么时候该停。

LTO 就是帮你解决这三个问题的。它**不替你写代码，也不替你选完整 workflow**。host agent 是 planner；LTO 是 harness：提供 state、artifact、audit、runner、sandbox、resume/recap 和 human gate，让你有证据、有刹车、有恢复点地推进。

## 架构哲学

- **主 agent 是 planner**：路径选择属于宿主模型。LTO 只提供状态、证据、工具、沙箱、审计和停止闸门。
- **交付契约进 core**：`/goal` 的价值是把交付目标变成可度量、可约束、可反过拟合的契约。Rust core 记录 `target` / `constraint` / `instrument` / `entropy-check`，phase gate 检查契约完整性；它不是后台 daemon，也不替 host agent 选路。
- **Preset 是 playbook，不是硬路由**：`review` / `debug` / `migration` / `claim-verify` / `research` 是调度先验，详见 `references/workflow-playbook.md`。不要先做替模型决策的固定执行入口。
- **Primitive 优先于产品命令**：先组合 `runner`、`audit`、`judge`、`next`、`autopilot`、`recap`；只有真实重复路径自然沉淀，才抽最薄 CLI。
- **外部观点默认先进插件**：有趣文章先做 `source_note → experimental path plugin → eval → promote/reject`，详见 `references/plugin-boundary.md`。只有像交付契约这种被拍板为通用 core primitive 的能力才进 core；插件仍只编译到现有 primitive，不替 host 规划、不自带升权。
- **覆盖四层 loop**：业界把 harness 看成可叠加四层 loop（loop engineering / loopcraft）。LTO 是覆盖 L1–L4 的长任务 harness——L1 agent loop（runner/scheduler）、**L2 verification（`audit` 跨族异构互审，比单模型 LLM-judge 抗盲区，是差异化）**、L3 event-driven（`events.jsonl` 事件总线就绪，tmux 派工+完成通知补触发层）、L4 hill-climbing（跨 run 挖掘喂 host，**只出 brief 不自动改写 harness**）。详见 README「放到业界 loop 工程坐标里看 LTO」。
- **自动化是梯度**：brief → supervised → sandboxed auto-exec → human gate。每一级都必须保留证据、可恢复状态和人工刹车。
- **派外部 agent 首选 tmux 真 TUI**：跨 runtime 派 codex/pi/agy 等外部 agent，**默认走 `lto dispatch-goal`（tmux 真 TUI 会话）或 `runner --runner tmux`**——agent 在 attached 会话里可见可监督，codex/agy 还能自动检测完成。headless delegate（`scripts/delegate/runners/*.sh`）只用于 shell 证据采集、只读审计派工，以及 tmux 不可用/headless CI 的兜底。不要默认退回 headless print（agy 的 `--print` 只出方案不执行，是假成功陷阱）。
- **派工完成自动唤醒，别轮询（v0.6.1+）**：派工出去后不要反复 `capture-pane`/手动盯。用 `lto events --wait --event-type agent.turn.completed --timeout <秒>` 阻塞等完成事件——runner 跑完时 `agent-turn-completed` 会通过本地 TCP 立刻唤醒你。还可选 `--bell`（响铃提示本地的人）和 `--notify-cmd '<命令>'`（host 自配远程通知，如发飞书；不可信文本走 `$LTO_SUMMARY` 环境变量，不硬编码任何通知工具）。
- **审计派工可控优先级（v0.6.1+）**：`lto audit --auto-dispatch --prefer-runner codex --prefer-runner agy` 限定并排序审计 runner 池，把慢的 pi 挪出收口关键路径，避免卡 timeout。host 可控旋钮，不按历史 telemetry 自动路由。
- **运行中可见、用量可查**：每个派工的输出边跑边写进 `.lto/<run-id>/live/<job-id>.log`，卡住时 `tail` 就能看；tmux 派工在 attached 会话里直接切窗观察。token 用量按 runner 计量（四家 runner 中 codex/pi/claude 有真实计量，agy 无 CLI 用量诚实标 unmetered），`recap`/`closeout` 汇总「这次 run 烧了多少 token」。
- **`.lto/` 是本地记忆，进项目先看**：装了 am（animem）时 run 成果 publish 到 am 做长期记忆；**没装 am 时 `.lto/` 就是全部记忆**（永远是本项目真源，am 只是下游投影）。接手项目第一件事跑 `lto runs`——列出本项目所有历史 run（目标/阶段/进度），别丢掉前人经验重复踩坑。

## 三个核心原则

**1. 想做更多？先回答「缺了什么」**
你说「以后可能要支持 X」——LTO 会问：X 现在真的需要吗？你答不上来就说明你在过度设计。停。

**2. 你的假设对不对？拿真实数据验证**
你说「大部分用户都怎样怎样」——LTO 会问：你预设的及格线是什么？先定标准再查数据，避免「跑个数字然后找理由说够好了」。

**3. 该不该往下走？问人，别自己决定**
三个 AI 都说「没问题」≠ 真的没问题。最终拍板的是你，不是 AI。

> 记住这三个问题：(1) 缺的东西现在真的存在吗？(2) 数据按你说的标准过了吗？(3) 人说了算吗？

## 六个阶段（一张图说清楚）

```
你的想法
  ↓
[阶段1] 该不该做？
  ├─ 该做 → 往下
  └─ 太早 → 砍掉多余部分，只做现在能做完的最小版本
  ↓
[阶段2] 写方案
  └─ 把可能出问题的地方标出来，让 AI 重点审
  ↓
[阶段3] 让三个不同的 AI 审你的方案
  ├─ 它们说的问题 → 一个个核实，修完再审
  └─ 修到没有大问题为止
  ↓
  ⚠️ 问你：可以开始写代码了吗？
  ↓
[阶段4] 写代码
  ├─ 开发前对齐架构设计: 当前改动放在哪一层、遵守哪些边界、复用哪些既有模式
  ├─ 从第一性原理说明为什么要做: 真实约束/用户价值/故障根因是什么
  ├─ 先做精简去重检查: 能删旧分支/合并重复逻辑/复用现有抽象就不要新增一套
  ├─ 调优必须有价值和测评: 先写 baseline/指标/及格线, 改完复测并落证据
  ├─ 不同模块让不同 AI 同时写（不冲突）
  └─ 写完用同样方法审代码
  ↓
[阶段5] 部署到线上
  ├─ 先改数据库 → 试运行看有没有问题
  ├─ 先只读不写 → 确认安全
  └─ 正式上线 → 你真的测过新功能能用吗？（不是说「服务没挂」）
  ↓
[阶段6] 记下来
  ├─ 文档对齐: SKILL/README/INSTALL/AGENTS/CLAUDE/references 不能和代码口径漂移
  ├─ 历史清理: 旧入口、旧路径、过时 run/兼容说明要清理、归档或标历史
  ├─ 仓库干净: closeout/打包前 git status clean, 或明确列出人工接受的剩余脏文件
  ├─ 重新打包编译: 从最终状态重新 build/package, 记录命令和结果
  └─ 这次踩了什么坑、做了哪些决策、为什么这样选
  ↓
下一个想法 → 回到阶段1
```

## 什么时候停？三个刹车

| 刹车 | 怎么踩 |
|---|---|
| **刹车1：喊不出缺什么** | 你说「以后可能需要」→ LTO：现在不需要，停 |
| **刹车2：数据不达标** | 你先定的标准是 80%，数据只有 60% → 停 |
| **刹车3：人说了算** | AI 都说好 → LTO 还是要问你：你同意吗？ |

## 让多个 AI 帮你审（阶段3 怎么操作）

找个跟你不一样的 AI 来审——你用 DeepSeek 就让 GPT 和 Gemini 来审，反之亦然。同个模型审自己等于没审。

**一键编排（推荐）**：LTO 扫高风险 task、写审计简报、给派工指令、收口判收敛。下面命令默认从仓库根目录运行；装过 `scripts/install.sh` 且 `lto` 在 `PATH` 后，可把 `$LTO` 换成 `lto`。LTO 只编排不自审（harness 不是被审/写码方），派工走自带的 `scripts/delegate/`（codex/pi/claude/agy runner），强制「审者 ≠ 你这个 host」：

```bash
LTO="cargo run --quiet --"
# 装过 install.sh 且已 cargo build --release --bin lto-rs 后，可用：
# LTO="lto"

# 全自动（推荐）：扫高风险 task → 自动派异构三方 → 收口判收敛
$LTO audit --auto-dispatch

# 派 agent 主动找漏掉的风险点（对抗"自报完整性"，未审 risk 会被 closeout 闸门拦）
$LTO audit --discover-risks

# 让慢的重 thinking runner（pi）不阻塞收口：--prefer-runner 限定并排序审计池
# （host 可控旋钮，非按历史 telemetry 自动路由）
$LTO audit --auto-dispatch --prefer-runner codex --prefer-runner agy

# 半自动（想手动控制）：
$LTO audit                                           # 写简报 + 打印派工指令
# 已产出的外部 runner 回复用 collect-agent-run 登记到 state；
# 当前 Rust CLI 没有历史文档中的 audit --collect <dir>。
$LTO collect-agent-run --task-id T1 --runner codex --reply .lto/<run-id>/audit/replies/reply-codex.md
```

> 审者输出结构化 JSON findings（severity 是字段，不靠正文扫关键词）；
> auto-dispatch/risk-discovery 负责异构派工，已有回复用 `collect-agent-run` 登记证据。

**手动派工**（只读审计/一次性评审 → headless delegate 合适）：
```bash
AD="scripts/delegate/runners"  # 本 repo 自带，无需外部依赖
$AD/codex.sh  方案.md 回复-codex.md  300 &
$AD/agy.sh    方案.md 回复-agy.md    300 &
$AD/claude.sh 方案.md 回复-claude.md 300 &
wait  # 等它们都跑完
# 每份回复逐个登记，文件名/metadata 保留 runner 来源。
$LTO collect-agent-run --task-id T1 --runner agy --reply 回复-agy.md
```

> headless delegate 只适合**只读、一次性**的评审派工。**开发型派工**（让外部 agent 真改代码）
> 必须走 tmux 真 TUI，否则 agy `--print` 只出方案不执行（假成功），多轮交互也没有完成信号：
> ```bash
> # 不带 --target/--new-window 即默认：自动在你当前 attached 的会话（如 cc）开可见窗口
> # 派 codex/pi/agy，host 不用记得传参就不会退回游离/无头（agent 干完自动检测完成）
> $LTO dispatch-goal --runner codex --goal goal.md
> ```

**拿到结果后做什么**：
1. 不投票。三个都说「没问题」≠ 真的没问题
2. 亲自看源码验证它们说的每一条
3. 大问题数量必须越来越少，反弹了回头查（别硬修下一版）
4. 修到零大问题为止

每轮审完，把这一轮的大问题数量填进 ledger（`.lto/<run-id>/audit-ledger.md` 的 Round Summary 表，仅 `--with-audit` 或 `--profile audit|deploy` 时生成），然后让脚本替你判收敛：

```bash
python3 scripts/audit_ledger_check.py .lto/<run-id>/audit-ledger.md
```

它会打出一行 `verdict:`——数量降到 0 是 CONVERGED（收敛了，可以收尾），还在降但没到 0 是 CONVERGING（继续修），数量反弹是 REBOUND、卡在原地不动（`--strict`）是 STALLED，这俩它会喊停，让你别自我感觉良好地往下冲。

`lto check` 会顺手帮你跑这个：反弹/停滞默认只**提醒**（WARN），`--strict` 下才**拦住**（ERROR）——提醒不等于强制停，最终还是你拍板。但 `closeout` 不一样：只要 ledger 填了轮次且没降到 0（CONVERGING/REBOUND/STALLED），它会**直接拒绝收尾**（除非你 `--force` 明确越过）。也就是说——脚本算的 Round Summary 收敛才是收尾的硬条件，你手填的 Closure Gate 字段只是辅助记录，骗不过收敛闸门。

## 部署上线（阶段5 怎么操作）

**必须按顺序**：
1. 数据库先改（要能改回去）
2. 试运行看会改什么
3. 先只查不写，确认安全
4. 正式上线
5. **真的测过新功能吗？**——不是 ping 一下看服务活着，是走一遍用户操作流程
6. 测完删掉测试数据，观察一段时间

## 怎么记（阶段6）

每个决定**当时就记**，别等做完再补。重点记：
- 为什么觉得太早（缺的到底是什么）
- 改了几轮、问题怎么变少的
- 查数据时定的标准是什么、实际是什么
- 别人做到什么程度了（天花板在哪）

用什么记：先用 `write_decision.py` 写 `docs/decisions/` ADR 并登记 artifact；有 memory-flow 时再把值得复利的经验写入库。

每次 closeout 会自动生成人类友好的 CHANGELOG.md，从 state.json 的 task/evidence 中提取。

### ANIMEM (am) artifact memory（可选）

LTO 的真源仍是本地 `.lto/<run-id>/state.json` + `artifacts.json`。
am（ANIMEM）只是可选的 artifact-memory sink，用来让不同 runtime
跨项目发现“哪个 run 活着、产物在哪里、谁审过、下一步是什么”。

`memory publish` 默认走 **am 原生 CLI**（`--sink am-cli`，am 0.7.0+）：把
redacted 投影信封管道喂给 `am ingest -f - --json`，am 负责 slug 生成、
written/updated/skipped 三态去重和 supersede 版本链；LTO 不构造 slug、
不碰 PG、不持有任何连接串。旧的 memory-flow REST 仍在（`--sink legacy-rest`）
作兜底，但已不推荐。

没装 am 也能完整使用 LTO（publish/resume 优雅降级，本地 `.lto/` 仍是真源）：

```bash
$LTO memory export --run-id <run-id> --dry-run  # 纯本地 redacted JSON
$LTO memory resume --project <key>              # 无 sink 时降级到本地 .lto
$LTO memory publish --run-id <run-id>           # 默认 am-cli；am 缺席则报错并提示本地真源
$LTO memory publish --run-id <run-id> --sink legacy-rest  # 兜底：memory-flow REST
```

`memory export/publish` 不投影 `original_user_request` 原文、raw transcript、
`agent_runs`、secrets、完整源码或私有文档正文；长文本只写 redacted summary/hash。
`memory resume` 只读，不覆盖 `.lto/current` 或 `state.json`。

## 多轮任务怎么不迷路

用一个文件记住当前状态：
```bash
# 开始（--why/--done-when 记下为什么做、做完的标准，recap 会用到）
LTO="cargo run --quiet --"

$LTO start --goal "做用户登录" \
  --why "降低登录失败率" --done-when "失败率<5%，三端覆盖"
# /goal 型长交付：目标函数四件套进 core delivery_contract
$LTO start --goal "提升检索召回" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h; paid API <= $50" \
  --instrument "python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"
# 当前 start 只接收目标/约束/测量/熵检查；max-turns/max-tokens/deadline
# 预算 cap 和 budget extend 不是当前 Rust CLI 命令面。

# 续接（上次 compact 之后，或新 session 恢复）
$LTO resume        # 给接手的 AI 拉上下文
$LTO memory resume # 可选：先查 ANIMEM/memory-flow，失败则降级本地
$LTO recap         # 给人看的回顾（含「花了多少 token」「当前在跑哪些 job」）
$LTO recap --mine  # 跨 run 聚合挖掘（按 runner 模型 × 任务 × 时间统计派工、失败率、耗时与收敛轮次，出只读 tuning brief）

# 看 job 实时进度（每个派工的输出边跑边写，卡住时主 agent 能直接看）
tail -f .lto/<run-id>/live/<job-id>.log

# 检查状态 / 问下一步
$LTO check
$LTO check --to implementation   # 只读检查进入写码前的证据
$LTO check --to closed --strict  # 预查收尾前硬证据
$LTO next                        # 出事实简报+无歧义命令建议（零 LLM，判断由你做）

# 自驱推进（受约束 harness，不接管 planner）
$LTO autopilot --supervised   # 出 brief 回吐你判断
#   --auto-exec：safe/reversible 子步骤在 worktree 沙箱自动跑（dangerous 停下等确认）
#   --autonomous：机械证据闸门 + 机械执行；不 spawn 决策 agent

# 预算（当前只读查询）
$LTO budget check                       # 各维度 used/limit/status

# 完成
$LTO closeout --summary "做了什么，验证了什么"      # 默认写 CHANGELOG.md
$LTO closeout --summary "行政收尾" --no-changelog  # 已提交后避免新 tracked dirt

# 发布（bump VERSION + CHANGELOG 归版 + git tag；全是 .git 写 → host 跑，runner sandbox 写不了）
$LTO release --part minor --date 2026-06-15 --dry-run  # 看计划不写
$LTO release --part minor --date 2026-06-15            # 真发：写 VERSION/CHANGELOG + commit + tag
```

> **resume vs recap**：resume 喂 AI（git head / task 状态，防 compact 后丢上下文）；
> recap 给人（人话回顾：当初要做啥/为什么/跑多久/做到哪/还剩啥/现在轮到你）。两者正交。
>
> **check --to**：`check --to implementation|closed` 只报告 phase-entry evidence，
> 不自动切阶段、不替人批准。`--strict` 才把缺失的 required evidence 变成非零退出；
> `--json` 输出单个 JSON 对象给其他 host 接手读取。
>
> **进入开发/调优前的四证据**：host 应在 run-state / task evidence 里写清
> architecture_alignment（架构设计与边界）、first_principles（约束/价值/根因）、
> simplification_dedupe（精简去重或复用判断）、value_measurement（baseline、指标、
> 及格线、复测命令）。没有测评的“调优”只算假设，不能当完成证据。
>
> **收尾前的四证据**：host 应在 closeout / release / handoff 前写清
> documentation_alignment（文档已与代码和架构对齐）、historical_cleanup（旧入口、
> 旧说明、旧 run/兼容残留如何处理）、clean_worktree（打包前仓库 clean 或剩余 dirt
> 已命名并获准）、rebuild_package（最终状态重新编译/打包的命令和结果）。
>
> **autopilot 档位**：当前 Rust CLI 暴露 `--supervised`（出 brief，默认）、`--auto-exec`（worktree 沙箱跑 safe 子步骤）和 `--autonomous`（机械证据闸门 + 机械执行）。`src/decision.rs` 保留 decision engine，但历史文档里的 `autopilot --decide` / `--decide-kind` / `--decide-budget` 未接到当前 CLI；需要另立 goal 恢复或正式移除。**autonomous 不 spawn 决策 agent、不替你反思**——它只做两件机械的事：读跨 run 挖掘事实判证据闸门（攒够真实派工才解锁，不够诚实退回 supervised），过闸后在 worktree 沙箱机械推进 safe 子步骤。escalate / dangerous / git push（含 `git -C . push` 等变体）/ 网络副作用一律停人类，反思永远归你。
>
> **budget 刹车**：当前 CLI 暴露 `lto budget check`，用于读取 token/预算事实；`start --max-turns/--max-tokens/--deadline` 和 `budget extend` 不在当前 Rust CLI 命令面。若要恢复 cap/extend，需要另立实现 goal 并同步 CLI/tests/docs。

## 什么情况下不要用 LTO

| 你要做什么 | 走哪里 |
|---|---|
| 修个报错 | diagnose |
| 让人审代码 | review |
| 写新 skill | skill-creator |
| 部署上线 | ship |

## 常见错觉（看起来对其实错）

| 你觉得 | 实际上 |
|---|---|
| 「先让 AI 审一下再说」 | 没想清楚缺什么就审 = 浪费。先过刹车1 |
| 「服务没挂，上线成功」 | 新功能可能根本没通电。走一遍用户操作才算 |
| 「三个 AI 都说好，可以合了」 | 还得问你。AI 不替你决定 |
| 「跑个数看看」 | 先定标准再看数，不然看到任何数字都觉得「还行」 |
| 「写完了，结束」 | 不记下来下次从头踩坑 |

## Resources

**入口与文档**
- `src/main.rs` / `src/cli.rs` — Rust v2 当前接管入口（21 个可见业务命令；旧 task/run 入口隐藏兼容；`--help` 另显示 clap 内置 `help` 行；plugin 含 `source-note` / `eval-run`）
- `scripts/write_decision.py` — ADR-first 决策落盘 helper（写 `docs/decisions/` + state + artifact manifest）
- `scripts/install.sh` — 安装 skills，并生成 sentinel-managed 全局 `lto` wrapper
- `references/onboarding.md` — **给 agent 读一份就懂怎么装载 LTO**（跨 runtime）
- `references/workflow-playbook.md` — `review/debug/migration/claim-verify/research` 调度先验
- `references/run-state-workflow.md` — 完整命令详细用法手册
- `references/execution-loop.md` — runner/judge/parallel/pipeline + agent 执行层
- `references/rust-migration-release.md` — Rust-only CLI、二进制下载状态、release 打包流程
- `references/python-rust-ownership.md` — Rust 命令 ownership 与 Python retirement 记录
- `references/hooks.md` — pre-commit/pre-deploy/pre-closeout 边界 hook（opt-in）
- `references/sharing-guide.md` — 怎么装、怎么给朋友用、项目级注入
- `references/cross-runtime-host-notes.md` — 不同 AI 工具当宿主的具体用法

**状态产物**
- `.lto/<run-id>/state.json` — 机器真源（含 tasks/risk_points/why/done_when/agent_runs）
- `.lto/<run-id>/artifacts.json` — 跨 host 产物索引（replies/briefs/evidence/judge/decision records/handoff，repo-relative 路径）
- `templates/run-state.md` — 人类可读状态模板
- `templates/audit-ledger.md` — 审计台账（仅 `--with-audit` 时生成）

**harness primitive 底层模块**（不直接走 CLI，是 next/autopilot/audit 的地基）
- `src/agent_job.rs` — AgentJob/AgentResult 数据合同（agent 世界，非 shell）
- `src/scheduler.rs` — 并发调度 + 退出码三元判定(OK/FAILED/TIMEOUT/RATE_LIMITED) + 指数退避
- `src/audit_dispatch.rs` / `src/audit.rs` — 异构审计派工、结构化 finding 解析与 ledger 收口
- `src/decision.rs` / `src/dispatch.rs` — 决策收敛、任务描述和派工 affordance
- `src/worktree.rs` / `src/effect.rs` — autopilot 自动执行的 worktree 沙箱与 effect 分类
- `src/events.rs` / `src/event_emit.rs` / `src/telemetry.rs` — 事件流、O2 调用方接线与派生遥测
- `scripts/delegate/runners/*.sh` — Rust scheduler 现役 runner adapter（保留，不属于 Python fallback）

## Workload Profile

**Tier: heavy** — LTO 不替你写代码，但需要你（宿主 AI）在关键节点做判断：要不要继续、要不要修、要不要部署。
