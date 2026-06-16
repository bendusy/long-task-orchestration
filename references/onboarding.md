# LTO 装载手册 — 给 agent 读的一份文档

> 你是一个 agent（codex / pi / agy / claude / 任意能跑 Rust/Cargo + bash 的 runtime）。
> 读完这一份，你就知道 LTO 是什么、怎么装给自己用、长任务里怎么用它不迷路。

## 一句话

**LTO 是一个跨 runtime 的长任务 harness：它给你 state、artifact、runner、audit、sandbox、resume/recap 和 gate。你这个 host agent 仍然是 planner；LTO 让你的选择可恢复、可审计、可自动化推进。**

它不复制 Claude Code 原生任务 harness 的实现——它是给**任意 runtime**用的版本。当前接管线是 Rust v2；Python 入口只作为兼容 fallback 保留。

## 术语表（读正文前先扫一遍）

LTO 文档里反复出现这些词。第一次见不用全记，卡住了回这里查。

| 术语 | 一句话解释 |
|---|---|
| **harness** | 框架本身。把状态、证据、审计、沙箱、恢复、人工闸门串成一条「跑道」，让你（agent）在上面跑长任务不迷路、不丢进度。 |
| **host agent（宿主）** | **就是你**——正在用 LTO 的这个 LLM（claude / codex / pi / agy …）。LTO 不替你做决策，你才是 planner。 |
| **primitive** | LTO 提供的可组合基础动作：`runner` / `judge` / `audit` / `next` / `recap` 等。你把它们拼起来，而不是选一个预设流程。 |
| **run** | 一次长任务。`start` 创建一个 run，数据存在 `.lto/<run-id>/`。 |
| **task** | run 里一个可独立执行的单元（如「给 login 加判空」），有命令、有产出、有证据。`runner` 不会自动建 task，你得先 `task add`。 |
| **phase（阶段）** | 任务的逻辑阶段：spec → 审计 → 开发 → 部署 → 实测 → 收尾。`--phase` 参数按阶段批量操作 task。 |
| **artifact** | task 产出的证据：日志、代码 diff、审计报告。「说做完了」得拿得出 artifact。 |
| **runner** | LTO 派出去执行一个 task 的角色（可以是 codex/pi/agy/claude，或就跑一条 shell 命令）。 |
| **异构（heterogeneous）** | 审你的 agent 来自**不同厂商家族**（你是 claude，就派 codex/pi/agy）。目的：避免「自己审自己」的偏袒。 |
| **fan-out** | 一次派多个独立 agent 并行干（如三家同时审）。区别于 `runner` 串行跑一条命令。 |
| **audit / 对抗审计** | 派异构 agent 来挑你产出的毛病，输出结构化 findings（带 severity 字段），再判是否「收敛」。 |
| **收敛 / ledger** | 把每个发现（blocker）登记进 ledger，逐轮追踪 open→fixed。「收敛」= 高危发现单调下降且没新引入的同级问题。**计数由脚本算，你不手填。** |
| **closeout（收尾）** | 任务闭环：写 handoff、生成 CHANGELOG。有未审风险会被 closeout 闸门拦住。 |
| **闸门（gate）** | 关键路口的硬检查（commit/deploy/closeout 前）。没过就停，可能回吐给人。 |
| **worktree 沙箱** | 一份独立的 git worktree 副本。autopilot 自动执行的命令都在这里跑——`rm -rf` 再狠也只炸可弃副本，主工作树毫发无损。 |
| **autopilot 档** | 自动化的梯度：`--supervised`（只出建议）→ `--auto-exec`（沙箱跑 safe 子步骤）→ `--decide`（派三方收敛）→ `--autonomous`（攒够证据后机械推进）。越往后放权越多，但每升一级都要更多证据。 |
| **resume vs recap** | `resume` 给 **AI** 拉上下文（git head / task 状态）；`recap` 给 **人** 看的人话回顾（当初要做啥、做到哪、还剩啥）。 |
| **`.lto/` 目录** | 本项目所有 LTO run 的落盘地。**am（animem）没装时，它就是这个项目的本地记忆层**——每个 run 一个子目录，记着目标/阶段/任务/证据/handoff。`lto runs` 列出全部。 |

## 进项目第一件事：看 `.lto/` 了解 LTO 历史

你（agent）接手一个项目时，**先跑 `lto runs`**——它列出本项目所有 LTO run
（目标 / 阶段 / 任务进度 / 哪个是 current），让你一眼知道这里以前用 LTO 做过
什么、做到哪。

```bash
lto runs                    # 本项目所有 run 概览（newest first）
lto resume                  # 拉当前 run 的上下文（给 AI）
lto recap                   # 当前 run 的人话回顾（给人）
lto recap --mine            # 跨 run 模式：哪个 model/phase 在这项目里好用
# 想看某个具体 run：读 .lto/<run-id>/{state.json, handoff.md, run-state.md}
```

**为什么这是第一步**：LTO 的记忆有两层——装了 am（animem）时，run 成果会
publish 到 am 做跨项目长期记忆；**没装 am 时，`.lto/` 就是全部记忆**，且永远
是本项目的真源（am 只是它的投影下游）。不看 `.lto/` 就开工，等于丢掉了这个
项目所有历史 run 的经验，会重复踩前面踩过的坑。

## 它解决什么（你为什么要装它）

单 context window 跑长任务有三个通病，LTO 各有一道防线：

| 通病 | 现象 | LTO 的防线 |
|---|---|---|
| agentic laziness | 50 项做了 20 项就宣布完成 | risk coverage 闸门 + ledger 收敛：没审完不让 closeout |
| self-preferential bias | 偏爱自己的结论，自审等于没审 | `lto audit` 强制异构审计（审者 runtime ≠ 你这家） |
| goal drift | 多轮后丢了原始目标，compaction 后更甚 | state.json 持久化 + `lto resume` 跨 session 拉回目标 |

## 怎么装给自己用

### 前提
- Rust stable + Cargo、bash、git。
- Python fallback 已在 v0.5.0 退役；核心 CLI 不再依赖 Python。
- 异构派工 runner：repo 自带 `scripts/delegate/`（runners/codex.sh、pi.sh、agy.sh、claude.sh + healthcheck.sh），`lto audit --auto-dispatch` 和 spawn agent 直接用它，无需外部 skill。前提是本机至少装好其中两家 CLI。
- 可选：ANIMEM / memory-flow。没装也能完整使用 LTO 核心命令；它只增强跨项目 artifact memory。

### 找到入口
LTO 的当前接管入口是 Rust CLI：
```
cargo run --manifest-path <skill-root>/Cargo.toml -- <子命令>
```
`<skill-root>` 取决于你怎么装的 skill：
- 软链装载（推荐）：`~/.agents/skills/long-task-orchestration/`（codex/agy 标准路径）或 `~/.claude/skills/long-task-orchestration/`（claude）。
- 仓库内直接用：`cargo run -- <子命令>`。

装载方式（在 agent-skills 仓库根）：
```bash
bash scripts/install.sh          # 软链到 ~/.claude/skills + ~/.agents/skills
bash scripts/install.sh --check  # 只检查不装
```

### 在你这家 runtime 里怎么调
LTO 是个 CLI，任何能跑 bash 的 runtime 都能调，不需要你内置什么 Agent 工具：
```bash
cargo run --manifest-path <skill-root>/Cargo.toml -- --repo <目标仓库> <子命令> [参数]
# 或安装 wrapper 后默认走 Rust：
lto --repo <目标仓库> <子命令> [参数]
```
`--repo` 指向你要做长任务的那个仓库（默认当前目录）。

跨 runtime 当宿主的专项坑（沙箱、派工、preflight）见 `cross-runtime-host-notes.md`——不同家差异大，派工前先读。

## 21 个可见业务命令速查

> 参数真源摘要见仓库根目录 [COMMANDS.md](../COMMANDS.md)。下表按常用工作流排序；`audit --discover-risks` 是 audit 的重要变体，与主命令同计。

| 命令 | 干什么 | 阶段 |
|---|---|---|
| `start --goal "..."` | 创建 `.lto/<run-id>/`，记下目标、宿主、HEAD；可附 `--target/--constraint/--instrument/--entropy-check` 形成 core delivery contract | 开工 |
| `runs` | **列本项目所有 LTO run**（目标/阶段/进度/current）。am 没装时 `.lto/` 就是本地记忆，进项目先跑这个 | 接续 |
| `resume` | 跨 session 拉回上下文胶囊（目标/进度/上次失败/下一步） | 接续 |
| `memory export/resume/publish` | 可选 artifact memory：导出 redacted projection / 发现历史 run / 显式发布到 sink | 接续 |
| `next` | **事实简报**：分析状态 → 给下一步 primitive 建议或决策简报 | 导航 |
| `check [--strict] [--to implementation\|closed] [--json]` | 校验状态完整性；只读输出 phase-entry 证据 | 自检 |
| `preflight` | 探活环境（sandbox/network/git/mcp/tmux） | 自检 |
| `task add --task-id T1 --title "..."` | 给当前 run 加一个 task（runner/next 的操作对象） | 开工 |
| `task update --task-id T1 --status done` | 改 task 状态/证据/touched_files，**不跑 subprocess**（标记完成别滥用 runner --command true） | 执行 |
| `task phase [--set audit]` | 看 / 推进 run 的 current_phase（轻量，无 evidence 闸门；正式收尾走 check --to/closeout） | 导航 |
| `runner --task-id T1 --command "..."` | 执行单 task（跑命令）+ 落证据 | 执行 |
| `collect-agent-run --task-id T1 --runner codex --reply r.md` | 把 delegate.sh 手动派工的产物（reply+token sidecar）登记进 agent_runs，让 recap 看见 | 执行 |
| `run parallel --phase X` | 并发批量跑多 task 的 shell 校验命令 | 执行 |
| `run pipeline --stages "..." "..."` | 每 task 串行过多 stage（item 间并发） | 执行 |
| `judge --phase X [--rerun-tests]` | 只读审查 + YAML verdict | 审查 |
| `audit [--auto-dispatch]` | **对抗审计**：派异构审计方 + 收口判收敛 | 审计 |
| `audit --discover-risks` | 派独立 agent 主动发现漏掉的风险点 | 审计 |
| `plugin list/validate/mount` | data-only 插件发现、校验和挂载 provenance；mount 不升权、不路由、不自动 promote | 插件 |
| `autopilot --supervised [--auto-exec]` | **自驱动**：读状态出决策简报，可在 worktree 沙箱自动跑 safe 子步骤 | 编排 |
| `recap` | **给人看的回顾**：你当初要做啥/为什么/跑了多久/做到哪/还剩啥/现在轮到你 | 回顾 |
| `budget check --run-id ...` | 查 run 预算用量/状态（tokens/turn/deadline） | 闸门 |
| `hook <pre-commit\|pre-deploy\|pre-closeout>` | 边界闸门检查 | 闸门 |
| `closeout --summary "..."` | 闭环 + 写 handoff + CHANGELOG；已提交后可加 `--no-changelog` 避免新 tracked dirt | 收尾 |
| `release --date ... [--dry-run]` | 打印 host-owned VERSION/CHANGELOG/git tag 发布计划；不替 host 写 `.git` | 发布 |
| `self-test` | 离线自检（验证 LTO 自己没坏） | — |

> `runner/run parallel/run pipeline` 编排的是 **shell 命令**（pytest/lint 批处理）。
> 真正的 **agent fan-out**（spawn 隔离 agent 做对抗审计/找风险）走 `audit --auto-dispatch` 和 `audit --discover-risks`，底层是 `agent_exec` spawn 原语 + scheduler（带并发/退避/限流/healthcheck）。

## Harness-first 新能力（区别于旧提示清单）

LTO 现在不只是"告诉你做什么"，它把长任务拆成可组合 primitive。你先读状态和 playbook，再决定下一段组合：

- **agent fan-out**：`audit --auto-dispatch` 自动派 codex/pi/agy 三家异构审计方，并发跑、自动收口。底层 scheduler 处理并发上限、429 退避、healthcheck 剔除挂的 runner。
  - **它审哪些 task（触发条件，2026-06-10 补：实测有人撞"no high-risk tasks found"）**：`--auto-dispatch` 只挑被判定为**高风险**的 task。判定是**关键词匹配**——task 的 `title` 或 `touched_files` 命中以下任一关键词即算高风险：持久化/迁移/schema/migration、权限/认证/鉴权/auth、并发/concurren/锁/lock、外部接口/api、支付/payment、安全/security/加密/crypt、删除/delete、回滚/rollback。
  - **没有高风险 task 时怎么办**：① 想审某个不含关键词的 task，用 `audit --auto-dispatch --task-id T1 T2` 强制指定；② 想让系统主动找你漏登记的风险，用 `audit --discover-risks` 派 agent 生成 `risk_points`（这些会被 closeout 闸门拦）；③ 研究/探索型 task 通常**不需要** auto-dispatch——它是给改 auth/payment/schema 这类高风险代码用的，纯研究跳过它很正常，不是 bug。
- **对抗审计闭环**：审者输出结构化 JSON findings（severity 是字段，不靠正文扫关键词），`--collect` 校验异构（审者 ≠ 宿主家族）+ 抽 blocker 计数 + 判 ledger 收敛趋势。
- **手动派工的登记桥（`collect-agent-run`）**：如果你不走 `audit --auto-dispatch` 而是手动用 `delegate.sh -a codex -p prompt.md -o reply.md` 派工，产物默认**不进** state（两条平行路径，见 `agent-runs-decoupling-diagnosis.md`）。派完工跑 `lto collect-agent-run --task-id T1 --runner codex --reply reply.md` 把它登记进 `agent_runs`——它会自动读同名 `reply.md.meta.json` token sidecar，于是 recap / closeout / cross-run-mining 都能看见这次派工的 token 与产物。agy 无 sidecar 会诚实标 unmetered。它不 spawn 进程，只登记已发生的事实。
- **risk 对抗生成**：`--discover-risks` 派独立 agent 主动找你漏登记的风险点（source=risk-agent），对抗"自报完整性"。未审风险会被 closeout 闸门拦。
- **事实简报（`lto next`）**：读状态 → 给宿主 LLM（就是你）一份富决策简报（目标 + task + 真实失败信息），让你推理下一段该 fan-out / adversarial / linear / 停下来问人。它自己零 LLM、零 key，只整理事实——**判断由你做**。无歧义时（如全 done 该 closeout）直接给可执行命令，`lto next --exec` 可跑；模糊时只出简报不替你猜。
- **playbook 调度先验**：`workflow-playbook.md` 把 `review/debug/migration/claim-verify/research` 写成 host agent 的调度先验：触发信号、可用 primitive、artifact、停止条件和反模式。它不是 CLI preset。

这是带护栏的 harness：路径你运行时选，但跑在可恢复可审计的 6 阶段 + state.json + git 边界轨道上。

## 自驱动：`lto autopilot`（受约束的自动推进）

`lto autopilot` 让 LTO 读状态 → 给 brief / 执行安全子步骤 / 必要时组织异构讨论。它是受约束 harness，不是替 host agent 接管 planner。三档：

- **`--supervised`（默认）**：出富决策简报 + 路由建议，escalate（多 blocked / 方案分歧 / 高风险）回吐你这个宿主 LLM 推理。集成 stall 检测（同失败指纹反复 = 停滞，提示别空转）。
- **`--supervised --auto-exec`（opt-in）**：对 pending task 的 **safe/reversible** 命令，在 **worktree 沙箱**里自动跑 + 落证据。
- **`--decide`（opt-in，已实现）**：escalate 时 **opt-in** spawn 三方异构 agent 跑双轨收敛（direction 投票 / review union 合并），出一份收敛 brief 给你读——**决策权仍归你这个宿主**，工具只整理三方结论不替你拍板。配 `--decide-kind`（direction|review|both，默认从状态推断）选收敛轨、`--decide-budget` 给 token 预算上限（默认 50000；传 0 强制 needs_human 不 spawn）。`--autonomous`（机械证据闸门 + 机械执行，已实现）**不 spawn 决策 agent、不替你反思**——读跨 run 挖掘事实判证据闸门（攒够真实派工才解锁，不够诚实退回 supervised），过闸后机械推进 safe 子步骤；与 `--decide` 互斥（autonomous 绝不派决策 agent）。反思永远归你这个宿主。

安全是硬底线（autopilot 自动执行的命令全经 `worktree_exec` 沙箱）：
- 每条自动执行的命令在**独立 git worktree 副本**里跑——`rm -rf` 再狠也只炸可弃的 worktree，主工作树/系统/家目录毫发无损。
- 执行环境隔离 HOME + 剥离 git 凭据——读不到 `~/.ssh`，`git push` 无凭据推不动。
- 危险命令（`rm -rf` / `git push` / `DROP` / `sudo` / `curl|sh` / 绝对路径逃逸 / 解释器 `-c -e` / 执行脚本文件）一律 **HELD = 不执行，回吐人确认**。
- 同命令失败 ≥3 次自动跳过（不空转烧额度）；无推进则退回只出简报。
- `--autonomous`（机械证据闸门 + 机械执行 safe 子步骤，已实现）：autonomous 默认禁网（`curl/wget/nc/ssh/scp` 一律 HELD），git push 含 `git -C . push`/`git -c k=v push` 变体全拦；不 spawn 决策 agent。

```bash
$L autopilot --supervised               # 只出决策简报，回吐你判断（最安全）
$L autopilot --supervised --auto-exec    # 安全子步骤自动在沙箱跑，危险的停下问你
```

## 给人看的回顾：`lto recap`（对抗"人忘了在做什么"）

长任务跑几天后，**人**会忘了当初要做什么、为什么、跑这么久在干嘛——这是人类侧的 goal drift。`resume` 是给 **AI** 拉上下文的（git head / task id）；`recap` 是给 **人** 看的回顾，用人话答六个问题：

```
$L recap
```
```
╭─ LTO Recap ─ 给人看的回顾（不是给 AI 看的状态）
│ 你当初要做什么 ── 重构登录模块，消除空指针
│ 为什么要做 ────── 线上空指针崩溃，影响登录（lto start --why 记录的）
│ 跑了多久 ──────── 11 天，中间最长停了 168 小时（约 7 天）
│ 已经做到哪 ────── 已完成 3 项：定位空指针、加判空、补测试
│ 还剩什么 ──────── 1 项卡住（集成测试偶发失败）。算做完的标准：全测试绿 + code review 过
│ 现在轮到你 ────── 决定怎么处理那 1 个卡住的任务
╰─ run: ...
```

数据来自 `state.json`。开 run 时用 `lto start --why "..." --done-when "..."` 记录"为什么"和"做完的标准"，recap 才答得全；不记也能用（缺的字段温和提示补）。`resume` 检测到距上次活动 >24 小时时，会主动提示你跑 `recap`。

## 最小跑通流程（照着做）

```bash
L="lto --repo ."

# 1. 开工，记下目标
$L start --goal "重构登录模块，消除空指针" --host <你这家:codex/pi/agy/claude>

# /goal 型长交付，直接把交付契约落进 Rust core state
$L start --goal "提升检索召回" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h; paid API <= $50" \
  --instrument "python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"

# 2. 加任务（task 是 runner/next/audit 的操作对象，先建出来）
$L task add --task-id T1 --title "给 login 加判空" --command "pytest tests/test_auth.py -x"

# 3. 干活：执行 task + 落证据
$L runner --task-id T1 --kind test --command "pytest tests/test_auth.py -x" --note "验证空指针修复"

# 4. 高风险？派异构对抗审计（需 agent-delegate）
$L audit --auto-dispatch        # 自动派 ≠ 你这家的审计方
#   没装 agent-delegate：$L audit  然后按提示手动派，再 $L audit --collect <reply-dir>

# 5. 开始写代码前，先看 entry evidence（不自动批准，仍要人拍板）
$L check --to implementation

# 6. 迷路了？问 LTO 下一步
$L next                          # 给你决策简报或下一步命令建议

# 7. 收尾前可先预查 required evidence
$L check --to closed --strict

# 8. 收尾
$L closeout --summary "登录模块重构完成，空指针已修，异构审计收敛"
```

跨 session 回来：直接 `$L resume`，它把目标和进度念给你听。

如果你在多 runtime / 多项目之间接手，先试：

```bash
$L memory resume --project <repo-key>
```

它会尝试查 ANIMEM/memory-flow 的 artifact memory；没装或没配置时不会失败成硬阻塞，
而是明确 warning 后降级读取本地 `.lto`。注意：`memory resume` 只读，不会覆盖
本地 `.lto/current` 或 `state.json`。本地 `.lto` 永远是真源。

想看会写入记忆层的内容，先 dry-run：

```bash
$L memory export --run-id <run-id> --dry-run
```

它只输出 redacted JSON，不联网。只有显式 `$L memory publish` 才需要 sink。
`publish` 默认走 am 原生 CLI（`--sink am-cli`，am 0.7.0+）：信封管道喂
`am ingest`，am 负责三态去重（written/updated/skipped），LTO 不碰 PG、
不持有连接串。am 缺席时报错并提示本地 `.lto` 仍是真源（publish 非硬依赖）。
旧 memory-flow REST 用 `--sink legacy-rest` + `MEMORY_FLOW_URL/TOKEN` 兜底。

## hook：让你别忘了用 LTO

hook 是 commit/deploy/closeout 前的边界闸门，提醒你"测过了吗 / 审过了吗 / 有没有没解决的 block"。详见 `hooks.md`。

**hook 是 opt-in**（2026-06-03 改）：`lto start --install-hooks` 才装进 `.git/hooks`，且检测到 husky / pre-commit framework / 已有自定义 hook 会跳过不覆盖。默认不装——LTO 不擅自动你的 git。

## 还想深入

| 想了解 | 读 |
|---|---|
| 完整命令手册 + add-task | `run-state-workflow.md` |
| workflow 调度先验 | `workflow-playbook.md` |
| 外部观点 / 路径插件边界 | `plugin-boundary.md` |
| 执行循环器（runner/judge/parallel/pipeline）细节 | `execution-loop.md` |
| Rust 迁移 / 二进制 / release 打包 | `rust-migration-release.md` |
| Codex CLI runner 控制面（`exec -C/-s/-o/stdin`） | `codex-cli-control.md` |
| 在 codex/pi/agy 当宿主的专项坑 | `cross-runtime-host-notes.md` |
| 审计收敛逻辑 | `audit-convergence.md` |
| 边界 hook 配置 | `hooks.md` |
| 分享给朋友 / 项目级注入 | `sharing-guide.md` |
| LTO 是什么、为什么这么设计 | `../SKILL.md` |
