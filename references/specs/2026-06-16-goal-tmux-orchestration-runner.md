# Goal: LTO 自建 tmux 编排 runner(repo 内派工底座,解开私有依赖)

> **致 codex**:沿用全部既有约束——LTO skill 自管、每 Phase 收口 `lto audit --auto-dispatch --discover-risks`
> 跨族异构审计、dogfooding 铁律(lto 自己调不通=lto bug 优先修)、维护者验收四标准、
> 红线不弱化(`clippy -D warnings` / `unsafe_code = "forbid"` / `cargo test --locked` 全绿)。
> commit 你写,release/tag 归 host。

---

## 为什么做这个(目标 + 第一性)

LTO 要支撑「host 合议 goal → 派 coding agent 长跑 → 回收 → 亲验」闭环(backlog ⑩)。当前两条派工路径都不够:
- **`scripts/delegate/*.sh`(现有 runner)**:`Command::new({runner}.sh)` 一发一收,跑不了长交互自驱。
- **tmux-autopilot 派工**:能长跑可观测,但是 **host 侧私有 skill,不在本 repo**,开源用户没有——写进 LTO = 违反交付契约「stranger 不依赖私有上下文」。

**目标:把 tmux 编排能力做进 LTO Rust(repo 内自建),成为 scheduler 的一种新 runner 形态。** 这解开私有依赖,让闭环能进开源。

---

## ⚠️ 必读:吸收业界长跑教训(否则会做出错误的设计)

host 拉子代理调研了 Anthropic 官方(`cwc-long-running-agents` + 两篇 harness 工程博客),硬结论:

1. **长跑 ≠ 长会话。长跑 = 短 headless 会话 × 外层 loop × workspace 外化状态续命。** 让单 agent 在膨胀 context 里啃整个大 goal,会撞两个失败模式:① one-shot 爆 context 留半成品 ② 后续会话「看到有进展就过早宣布完工」。**实证:host 在 tmux 看到 codex 啃整个 O2 goal 跑 20min 后 Goal blocked 卡住,正是这个。**
2. **完成检测用契约文件轮询**(feature 清单 `passes:false` 清零)+ exit code,比纯哨兵更语义化(哨兵只说「停了」,契约说「真做完了」)。
3. **host 亲验/独立 evaluator 是第一性共识**:Anthropic 点名 agent 第二大失败模式是「marks done without proper testing」。LTO 的 `audit --auto-dispatch` 异构审计已是「fresh-context evaluator」且比 cwc 同族更强,保留。

**因此本 goal 的设计立场**:
- tmux runner 是「**可观测的常驻外壳**」,不是「让一个 agent 啃整个 goal」。外壳里跑的应是**短会话 × 外层 loop**。tmux 的价值是 host 能实时观测 + agent 长跑续命,不是把 context 撑大。
- 不重造 evaluator——复用 LTO 已有 `audit --auto-dispatch` 异构审计当 fresh-context judge。
- **host 亲验是闭环的硬停止点,不可自动跳过**(hook 回来即当完成 = 自动放过 bug,实测每轮 codex 报全绿 host 都揪出真 bug)。

---

## Rust 化设计原则(别把 bash 直译成 Rust)

**源是 bash**:`dispatch_agent.sh` 是 122 行纯 shell。rust 化的价值**不是逐行翻译成 `Command::new`**,而是拿到 bash 拿不到的东西。每一条都要在实现里体现,否则这次 rust 化没意义:

| bash 版弱点 | Rust 必须发挥的长处 |
|---|---|
| send-keys 时序靠 `sleep` 硬等(慢机器错位) | tokio 结构化等待 + 真超时,**不靠盲 sleep**(用 `tmux wait-for` 事件 / 带 timeout 的轮询) |
| signal/sentinel 靠字符串拼 + 文件轮询 | **类型化 JobStatus / 完成信号**,编译期防错 |
| shell 引号/全角/`set -e` 吞错(host am 记过这些坑) | `Result` 传播 + 结构化错误不吞;`Command::new("tmux")` 不过 shell,**零注入面** |
| 派 n 个靠手动多窗口,无并发管理 | 复用 scheduler 的 tokio 并发 + 信号量 + healthcheck + failover |
| 无状态,派完不知发生啥 | 接 O2 事件流,每步 emit `runner.*` 结构化事件,可观测 |
| 完成检测两 mode 是分散 shell 逻辑 | 统一成一个 trait/enum,signal/sentinel/fire 是类型分支 |

**判据**:如果实现出来只是「Rust 包了一层 shell 调用」,那是失败的 rust 化。成功的标志是——时序不靠 sleep、完成检测是类型化的、错误不吞、接住了 scheduler 的并发与事件。

---

## runner session 复用 + 省 token 调度(host 调研落地,本机实测)

> 这节是 tmux runner「省 token」一等理由的具体兑现。子代理本机实测(pi 0.79.3/codex/claude/agy)+ 官方 docs 得出,直接指导实现。

**省 token 真机制 = warm prompt cache,不是「避免冷启重载」**:
- 同一 session 内重发的 context 走 provider **cacheRead(~10% 价)**;冷启 / 超 cache TTL(~5 分钟)才付 full price。实测 pi `--session-id` 续接:Turn1 `input:1502 cacheRead:0`(冷载)→ Turn2 `input:109 cacheRead:1408`(1408 走缓存,净增 109)。
- **杠杆排序:① 保持 session warm(同进程/5 分钟内复用,且不中途换 model/effort/system prompt——任一都使整段 cache 失效)> ② 裁剪 context 体积(`--no-skills --no-context-files --no-extensions`)。** 前者比后者直接。

**runner 复用能力排序(实测,决定每家怎么调)**:
| runner | 最优常驻方式 | 注意 |
|---|---|---|
| **pi** | `pi --mode rpc --session-id <run-id>`(私有 JSONL over stdio 常驻,**四家最干净**) | 优先用它做常驻 |
| claude | Agent SDK `ClaudeSDKClient` / `codex mcp-server` | 次之 |
| **codex** | **`codex mcp-server` + `codex-reply`**(不是 `exec resume`) | ⚠️ `exec resume` 保上下文但 system prompt 每轮 +18K,**token 不线性省** |
| agy | 仅 `--continue`/`--conversation` | 最弱无 RPC,退 tmux send-keys |

**pi `--mode rpc` 对接契约(实测,tmux runner / 常驻 runner 直接用)**:
- spawn `pi --mode rpc --session-id <run-id> --provider <p> --model <m> --no-skills --no-context-files --no-extensions`,**保持 stdin 打开**(EOF 即退)。
- stdin 每行一个 JSON command(**严格 `\n` 分行,禁 Node readline——`U+2028/U+2029` 会误切**):
  - `{"type":"prompt","message":"..."}` —— 发一轮(**参数键是 `message`,不是 `prompt`/`text`**)
  - `{"type":"get_session_stats"}` —— 读 `contextUsage.percent` 做预算
  - `{"type":"compact"}` —— context 超阈值时压缩
  - `{"type":"steer","message":...}` / `{"type":"abort"}` —— 纠偏 / 中断
- stdout 流式 event:`message_update.text_delta` 拼回复,`turn_end` 收尾(带 usage)。可带 `id` 字段做 request/response 关联(response 回带同 id,event 不带)。
- 源:`github.com/earendil-works/pi` 的 `packages/coding-agent/docs/rpc.md`。

**对本 goal 的影响**:
- tmux runner 不只「在 pane 里 send-keys」——对 pi 这种有干净 RPC 的,**优先走 `--mode rpc` 常驻进程多轮复用**(比 tmux 解屏更干净,省时序/截屏脏活)。tmux send-keys 是「CLI 只有 TUI、无 RPC」(如 agy)的兜底。
- 这其实给 T1 加了一条路径选择:**有 RPC 的 runner(pi)走 RPC 常驻,无 RPC 的(agy)走 tmux**。两者都解决「session 复用 warm cache」,只是机制不同。实现时按 runner 能力选。

---

## 核心架构裁决(先认同再实现,这是最容易做歪的地方)

**tmux runner = scheduler 的新 runner 形态,不是另起一套编排。**

现有 `Scheduler`(`src/scheduler.rs`)是 `Command::new(runners_dir/{runner}.sh)` spawn → 等退出 → 收 stdout 的一发一收模型。tmux 长跑 runner 应接入**同一个 AgentJob/AgentResult/healthcheck/failover/事件**抽象,只把「派工+完成检测」机制换掉:

| 维度 | 现有 headless runner | 新 tmux runner |
|---|---|---|
| 派工 | spawn `.sh`,阻塞等退出 | tmux 开窗/选 pane,send-keys 发 prompt |
| 完成检测 | 进程退出 + exit code | `tmux wait-for`(signal 模式)或 契约/哨兵文件轮询(sentinel 模式) |
| 可观测 | 收完才有 stdout | 全程 `capture-pane` 可读中间态 |
| 抽象归属 | `scheduler.rs` | **同一 scheduler,新增 runner kind 分支** |

复用(勿重写):`AgentJob`/`AgentResult`、healthcheck、failover、O2 的事件 emit(tmux runner 的 spawn/finished 也应 emit `runner.*` 事件)。

**port 源**:`~/.claude/skills/tmux-autopilot/scripts/dispatch_agent.sh` 的三 mode 是参考实现(host 会把它读给你,或你按本 spec 的契约实现):
- `signal`:prompt 当 shell 命令交付,尾拼 `tmux wait-for -S done-<id>`,orchestrator `tmux wait-for done-<id>` 阻塞等待——零轮询事件驱动(headless `codex exec`/`claude -p` 命令适用)。
- `sentinel`:交互 TUI 聊天交付,prompt 末尾追加「完成后写哨兵文件」,orchestrator 轮询哨兵存在即完成(长交互 TUI 适用)。
- `fire`:只发不等。
- 三条社区最佳实践必须 port:① 两步发 Enter(先 `-l` 发文本,sleep 后单独 Enter,避免 Enter 被吞/错位)② 发送前安全前置(退出 copy-mode、清当前行)③ 完成检测两机制。

---

## 默认载体策略(host 裁定,贯穿 T1/T2)

> host 裁定:**主 agent 所在的 tmux session 是第一载体;tmux 派工是长任务的默认首选派工方式。**

含义:
1. **同 session 优先**:tmux runner 默认在**主 agent 自己所在的 session** 新开 window 派 worker,而不是另起陌生 session。这样 worker 和主 agent 同一工作台,host 天然能 attach 观测、复用已有 n-agent 布局。
2. **detect 机制(host 已亲验可行)**:主 agent 进程能读 `$TMUX_PANE`(如 `%12615`)→ `tmux display-message -p -t "$TMUX_PANE" "#{session_name}:#{window_index}.#{pane_index}"` 反查到自己位置(实测得 `cc:2.1`)。tmux runner 据此确定「我在哪个 session」,默认就在这个 session 开 window。
3. **长任务首选 tmux 派工**:`lto autopilot` / 长任务编排首选 tmux runner——不只为可观测+长跑续命,更因为**对第三方 CLI(codex/pi/agy)tmux 交互模式比 headless 更稳、输出更清晰、功能全开**(host 实测,见 T1.1)。headless `.sh` runner 降为「无 tmux 环境/CI」的 fallback。
   - **降级链**:`$TMUX` 存在 → tmux runner(默认);`$TMUX` 不存在(headless/CI)→ 回退 headless `.sh` runner;两者都不行 → 清晰报错。

**设计警示(别做歪)**:
- **不要在主 agent 自己的 pane 派工**——会和主 agent 的交互打架。要在同 session **新开 window/pane**。
- `$TMUX_PANE` 是主 agent 进程的环境;Rust runner 要拿到它,可能需主 agent 把它传进 LTO(env 透传)或 LTO 自己读 `$TMUX`/`$TMUX_PANE`。确认 LTO 进程能见到这俩 env(它是主 agent 的子进程,通常能继承——但 `$TMUX_PANE` 有时不透传子进程,T1 要实测)。
- 无 tmux 时不能硬依赖——降级链是硬要求,否则 CI/headless 环境直接挂。

---

## Phase T1 — tmux runner adapter(Rust,核心)

> **为什么 tmux 交互 > headless(host 一线实测,优先级高于业界泛论)**:对 codex/pi/agy 这类**第三方 CLI**,headless `exec` 模式是弱项(一发一收、输出被 SPA/壳吞、功能受限);tmux 交互模式下它们是**完整 TUI,功能全开、输出清晰、能做的工作更多、运行更稳**。Anthropic「headless 更好」的结论隐含前提是 `claude -p`(自家高度优化);对第三方 CLI 不成立。**所以 tmux 是首选载体,不只是「可观测外壳」——它本身让 agent 跑得更稳更全。** headless `.sh` runner 仅作无 tmux(CI)的 fallback。
>
> **⚠️ tmux 的一等理由:session 复用省 token(host 实测,别漏)**。headless `pi -p` **每次冷启重载约 4 万 token context**(AGENTS.md/CLAUDE.md/skills/extensions),实测算个 `1+1` 都要 44s/40091 input tokens——是 token 黑洞。tmux 常驻 session 里,**context 载一次,后续多轮 send-keys 复用同一 session**,省大量 token。pi 调度最自由(`--continue`/`--session-id`/`--mode rpc` 支持 session 复用)。**因此 tmux runner 必须支持复用已有 pane 的常驻 session(而非每次新开冷启),这是省 token 的关键设计点,不是可选优化。** 具体 session 复用调用法见即将补充的最佳实践(host 正调研)。这条和「稳/功能全/可观测」并列,是 tmux 派工的核心理由之一。

### T1.1 能力契约
新增一个 tmux runner,能:
1. **派工**:在指定 tmux target(session:window.pane,或新开 window)send-keys 发 prompt。支持三 mode(signal/sentinel/fire)。
   - **⚠️ 启动 ready 检测 + 升级提示处理(host 实测的真坑)**:CLI 刚在 tmux 打开时有延时——升级提示 / 组件加载 / API key 检测 / TUI 渲染。**不能 send-keys 完立刻发 prompt,会被吞或打断启动**。处理:
     - **碰到升级/更新提示一律选「跳过」,不升级——求稳**。长任务进行中升级 = 引入新版本不稳定/交互变化的风险,绝不在派工时升级。ready 检测要能识别常见升级提示(如 "update available" / "upgrade?" / "new version")并发对应的「跳过/否/稍后」按键(各 CLI 不同:可能是 `n` / `Esc` / 回车选默认 No),做成可配置的 skip-pattern → skip-key 映射。
     - 跳过升级 + 等组件加载完后,`capture-pane` 轮询到「CLI ready 可接收输入」的特征(如 codex prompt 行 `›` + 状态栏 `gpt-5.x · <path>`;各 CLI 不同,做成可配置 ready-pattern),ready 才发 prompt。
     - 带超时(启动慢/卡住时不无限等);超时则报错回收,不静默挂起。
2. **完成检测**:signal 用 `tmux wait-for`;sentinel 轮询哨兵/契约文件(带超时)。
3. **可观测**:暴露 `capture-pane` 抓中间态(供 host/事件用)。
4. **安全前置**:两步发 Enter、退 copy-mode、清行(port 自 dispatch_agent.sh)。
5. **回收**:产出 `AgentResult`(status/exit_code/reply 摘要),接入 scheduler 事件(emit `runner.started/finished`)。

### T1.2 落点
- 新增 `src/tmux_runner.rs`(或 scheduler 内新 runner kind 分支)。
- `AgentJob` 接入:已有 `runner: String` 字段——`runner: "tmux"` 走新分支。target/mode/sentinel 路径用**新增可选字段**(`#[serde(default)]`,向后兼容,现有 headless job 反序列化不受影响)或复用 `env`。别改 AgentJob 现有字段语义。
- 用 Rust `Command::new("tmux")` 调 tmux 子命令(send-keys / wait-for / capture-pane / new-window / display-message),**不 shell out 到 dispatch_agent.sh**(那是私有的,要的是 repo 内自带实现)。
- tmux 不可用时优雅降级(`which tmux` 探测,缺失清晰报错或回退 headless runner)。

### T1.3 测试
- tmux 可用时:派一个 `echo + wait-for` job,断言完成检测真触发、AgentResult 正确。
- tmux 不可用时:优雅降级/清晰报错(CI 无 tmux,测试要能 skip 或 mock)。
- 安全前置:copy-mode 下派工不被吞(可 mock tmux 命令序列断言)。
- 事件:tmux job 跑完 emit `runner.finished`。

### T1.4 完成判据
`lto runner --runner tmux --target <sess:win> --command ...`(或等价)能派工、检测完成、回收 AgentResult;tmux 缺失优雅处理;事件 emit;全绿。

---

## Phase T2 — 短会话×外层 loop 长跑编排(吸收业界教训的关键)

> 这是「让长跑正确」的 Phase。不是让 tmux runner 啃整个 goal,而是实现「Initializer/Worker/Judge + 外层 loop」。

### T2.1 能力
- **Initializer**:把一个 goal 展开成 feature/task 清单(LTO 已有 `task-add` + state.tasks,复用),每条初始未完成。
- **外层 loop**:LTO 驱动「取下一个未完成 task → 用 tmux runner 派一个**短会话** worker 只做这一个 → 完成检测 → 更新 state → 下一个」,直到清单清零或人工停。
- **完成检测 = 契约轮询**:loop 终止条件是 state 里 task 全 done(对等业界 `passes:false` 清零),不是「agent 说完了」。
- **Judge**:每个 task 完成后可选 `audit --auto-dispatch` 异构审计当 fresh-context evaluator(复用,不重造)。

### T2.2 落点
- **优先扩 `lto autopilot`,不新增命令**(host 已盘:autopilot 已有 `--supervised`/`--auto-exec`/`--autonomous` 三档 + stall 闸门 + retry 限制 + `AUTOPILOT_STATUS`,它**已经是个 loop 编排器**)。T2 本质是给 autopilot 的「执行下一步」换成「用 tmux runner 派短会话 worker」,而不是从头写 loop。除非有硬理由(写进 run-state),否则别新增 `lto orchestrate`。
- 复用 autopilot 已有的 stall digest(防伪推进)、retry 限制、progress 棘轮——这些正是防「过早宣布完工」的现成闸门。
- worker 短会话:每轮 fresh context(不累积),读 state/progress 自己定位,只做一个 task。

### T2.3 完成判据
能把一个多 task 的 goal 用「短会话×loop」跑完,每个 task 独立派工+检测+(可选)审计;loop 靠契约(state task 状态)终止不靠 agent 自报;host 能 `capture-pane` 全程观测;全绿。

---

## Phase T3 — host 亲验硬停止点 + 闭环 playbook(进开源)

T1+T2 让 LTO 有了 repo 内自建的可观测长跑派工底座后,backlog ⑩ 的触发条件满足,把闭环写成 playbook 进开源:

### T3.1
- 在 `references/workflow-playbook.md` 加「host 合议 goal → tmux runner 短会话 loop 长跑 → 异构审计 → **host 亲验硬停止点**」playbook。
- **host 亲验硬停止点**写死:loop 跑完/blocked 后,host(或独立 evaluator)必须亲验,不能 hook 回来即当完成。给出亲验清单(跑测试/grep 产物/对比自述)。
- Default-FAIL 证据门(借鉴 cwc):评估能否加 PreToolUse 式机制——task 标 done 前必须有证据 artifact,否则 check 拒。
- backlog ⑩ 状态更新为 ✅(注明 Rust 落地)。

### T3.2 完成判据
playbook 不依赖任何私有工具(纯 `lto` + repo 内 tmux runner);stranger 能照着复现;host 亲验是文档里的硬步骤。

---

## 实战痛点对策(从 O2 派工提炼,本 goal 必须规避)

host 观察 codex 跑 O2 时暴露的痛点,本 goal 写进对策,别重蹈:

1. **长 thread 精度下降**:codex 啃整个大 goal 跑 32min 触发 Context compacted + 官方精度警告。**对策:T1/T2/T3 各自独立收口 + 独立 commit,codex 跑完一个 Phase 就停、commit、可新开 thread 跑下一个。别一口气啃完三个 Phase。**
2. **慢 runner 拖死收口**:O2 收口的 `lto audit` 派给 pi(重 thinking 模型)卡满 300s。**对策:本 goal 每 Phase 收口的异构审计优先派 codex/agy(快),pi 留补充;审计范围限本 Phase diff,别全仓。**
3. **进行中 untracked 文件触发 CRITICAL 误报**:O2 的异构审计把新建的 `event_emit.rs`(未 commit)报成 CRITICAL。**对策:新建的 `tmux_runner.rs` 等文件 untracked 是预期,审计若报 untracked 风险,记录但不当 blocker——commit 时消解。**

---

## 执行顺序与停止点

```
T1 (tmux runner adapter) → T2 (短会话×loop 长跑) → T3 (亲验硬停止点 + playbook 进开源)
每 Phase 收口:cargo 全绿 + lto audit 跨族异构审计 + lto check
```

- **T1 先做**(地基:repo 内 tmux 派工能力)。
- **T2 是让长跑正确的关键**——别跳过直接让 tmux runner 啃整个 goal(那就重蹈 codex 卡 20min 的覆辙)。
- T2 动手前先在 run-state 写「复用 autopilot 还是新命令」的架构决策。
- dogfooding 全程:实现 tmux runner 时若 lto 自己派工出错,那就是要修的 bug。

## 提醒
- tmux 子命令用 Rust `Command::new("tmux")` 直调,**不依赖 dispatch_agent.sh**(要 repo 内自带)。
- 不重造 evaluator/loop 闸门——复用 `audit --auto-dispatch`(异构 judge)+ autopilot 的 stall/progress(若适用)。
- host 亲验不可自动化——这是闭环的安全阀,不是可选优化。
