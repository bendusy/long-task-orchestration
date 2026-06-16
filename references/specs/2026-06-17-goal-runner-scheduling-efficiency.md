# Goal: runner 调度效率 —— 干掉 headless 冷启每次重载 4 万 token 的浪费

> 致 codex:沿用全部既有约束(LTO skill 自管 / 每 Phase 异构审计 / dogfooding 铁律:lto 自己调不通=lto bug 优先修 / 红线不弱化 `clippy -D warnings`+`unsafe_code=forbid`+`cargo test --locked` 全绿 / commit 你写、release/tag 归 host)。
> **这份只做 backlog ⑪(runner 调度效率)。做完停。别往下做 ⑥/⑧ 或别的 backlog 项。**

---

## 为什么做(第一性)

dogfood 实测:codex 跑 audit 收口卡 **1h33m+**,逐层挖到真根因——**不是 pi 模型慢,是调度方式蠢**:

| 层 | 实测 | 是不是根因 |
|---|---|---|
| 模型层 | `--thinking off` 只省 6s(44→38s) | ❌ 不是 |
| **context 层** | `pi -p "1+1"` 默认 = **44s / 40091 tokens**(每次冷载 AGENTS.md/CLAUDE.md/skills/extensions);加 `--no-skills --no-context-files --no-extensions` = **1.4s / 395 tokens(快 30 倍)** | ✅ 主因 |
| 调度层 | 每次 `pi -p` headless 一发一收冷启,从不复用 session/RPC 常驻 | ✅ 最深根因 |

审计是**一次性纯评审**——它不需要加载整个 skill 生态。每次审计冷载 4 万 token 是纯浪费。

**目标**:审计/纯评审类派工,去掉每次冷启重载的浪费。治标(加 context-control flag)立竿见影 30 倍;治本(audit 走常驻载体而非 headless 冷启)接上刚落地的 tmux runner。

---

## ⚠️ 必读:吸收的教训/前提

1. **异构性不可破**:跨族 failover 一字不动(codex/pi/agy/claude 仍异构,healthcheck gate 仍在)。这份只改「单次派工怎么调用」,不改「派给谁」。
2. **read-only 契约不可弱化**:pi.sh:29-32 的 `--tools read,grep,find,ls` 白名单(read-only 时)必须保留。加 context-control flag 与权限白名单**正交**,别误删权限逻辑(`references/runner-readonly-contract.md`)。
3. **`--no-skills` 等是 context 控制,不是权限控制**:它去掉的是 skill/extension/context-file 加载(省 token),不动工具权限。审计 read-only 时两者都要(实测 `--tools read,grep,find,ls` + 无 skills = 2.4s / 1388 tokens)。
4. **撤回的错误方向**:别给 audit 加「runner 优先级绕开 pi」——pi 慢是调度方式(冷启重载),不是 pi 本身;agy/claude headless 同样冷启。修调度效率,不是排序。
5. **治本别推倒治标**:治标(flag)和治本(常驻调度)是梯度,不是二选一。先治标拿到 30 倍,再治本接 tmux/RPC。

---

## 核心架构裁决(host 先定,别让 codex 猜)

**裁决 1:context-control flag 由 LTO 何时注入?**
- 审计/纯评审场景(`audit --auto-dispatch` / `llm_judge` / 任何 read-only 评审派工)**应注入** `--no-skills --no-context-files --no-extensions`——它们不需要 skill 生态。
- 开发/写码场景(`autopilot --worker-runner` / `runner --command` 跑实际任务)**不注入**——写码可能需要 skill 上下文。
- **机制**:沿用 pi.sh 已有的 env 注入模式(`LTO_PERM_SANDBOX` / `LTO_PERM_TOOLS`),新增一个 env 开关 `LTO_LEAN_CONTEXT=1`,由 LTO 派工时按场景设置。runner.sh 读到就加 context-control flag。**这样 LTO 侧(Rust)决定何时精简,runner.sh 只执行**——符合「runner.sh 是哑执行器」原则。

**裁决 2:各 runner 的 flag 名不通约,怎么抽象?**
- 实测各 CLI 的 context-control flag 不同名(pi 有 `--no-skills/--no-context-files/--no-extensions`;codex/agy/claude 待 codex 实测各自等价 flag 或确认无)。
- **机制**:每个 runner.sh 自己把 `LTO_LEAN_CONTEXT=1` 翻译成本 CLI 的等价 flag(类似现在每个 runner.sh 自己翻译 read-only)。**没有等价 flag 的 runner 就不加(优雅降级,不报错)**。LTO 侧只管设 env,不管各 CLI 差异。

**裁决 3:治本(audit 走常驻调度)做到哪一步?**
- tmux runner(v0.5.0 已落地)是常驻载体。但 audit_dispatch 当前走 scheduler→runner.sh headless。
- **这份 goal 的治本范围 = 让 audit 能选 tmux runner 调度**(复用已有 `runner: "tmux"` 路径),不新写 RPC 常驻协议(pi `--mode rpc` 留作独立后续 goal,避免这份摊太大)。
- 若评估后发现 audit 走 tmux 复杂度高(audit 需要结构化 findings 回收,tmux capture-pane 可能不如 headless json 干净),**治本可只到「记录如何接 + 留接口」,先交付治标**——治标已是 30 倍,治本不阻塞收口。

---

## Phase 1:治标 —— context-control flag 注入(立竿见影,先收口)

### 1.1 LTO 侧设 env(Rust)—— host 已亲验落点
- **机制(host 已盘清,别另找)**:`src/scheduler.rs:973 runner_env(job)` 从 `job.env.clone()` 起,再塞 `CODEX_SANDBOX`/`CODEX_MODEL`,然后 `scheduler.rs:413 .envs(runner_env(job))` 传给子进程。**所以只要在构造 audit/judge 的 `AgentJob` 时往 `job.env` 塞 `LTO_LEAN_CONTEXT=1`,scheduler 自动带下去——不用改 scheduler.rs**。
- **落点**:`src/audit_dispatch.rs`(审计派工构造 job 处)+ `src/llm_judge.rs`(judge 派工构造 job 处)——这两处构造 `AgentJob` 时,给 `job.env` 插 `LTO_LEAN_CONTEXT=1`。先 grep 这两文件里构造 AgentJob / 设 `.env` 的地方。
- **不改** `runner_env` 本身(它已 clone job.env);**不碰** autopilot worker 的 job 构造(开发派工不设 lean)。
- **判据**:`grep -rn "LTO_LEAN_CONTEXT" src/` 看到 audit/judge 构造 job 处设了;autopilot worker 路径没设。

### 1.2 各 runner.sh 翻译 flag
- **pi.sh**(落点 38-41 调用块):读 `LTO_LEAN_CONTEXT=1` → 给 `pi -p` 加 `--no-skills --no-context-files --no-extensions`。与现有 `PERM_ARGV`(read-only 白名单)并存。
- **codex.sh / agy.sh / claude.sh**:codex 实测各自 CLI 有无等价 context-control flag(`<cli> --help` 查),有就翻译,没有就不加(降级不报错)。把实测结果(谁有谁没有)写进各 runner.sh 注释 + `references/runner-readonly-contract.md` 旁记一笔。
- **判据**:`LTO_LEAN_CONTEXT=1 bash scripts/delegate/runners/pi.sh <prompt> <reply> 30` 实测,reply 正常 + token 数从 ~40000 降到 ~400(对比 dogfood 实测数)。

### 1.3 dogfood 端到端验证(治标真省 token)
- 跑一次真实 `lto audit --auto-dispatch --discover-risks`(小 diff),对比派工前后 token/耗时——pi 那一路应从 44s 级降到秒级。
- **判据**:`events.jsonl`(O2 已接线)里 audit 派工的 `runner.finished` 事件 elapsed_sec 显著下降;或直接看 audit 收口总耗时从十分钟级降到分钟级。**这是 ⑪ 的核心交付,必须有实测数对比。**

---

## Phase 2:治本 —— audit 可走 tmux 常驻调度(评估后决定深度)

### 2.1 先评估(架构裁决 3)
- 核 `audit_dispatch.rs` 怎么回收 findings(结构化 JSON?),tmux runner 的 capture-pane 回收能否产出同样结构化 findings。
- 若能:让 audit 派工可选 `runner: "tmux"`(env 或 flag 切),复用 v0.5.0 的 tmux runner 路径,context 只载一次。
- 若代价高(findings 回收不如 headless json 干净):**只写「如何接 + 留 TODO 接口」到 backlog ⑪,不强行接**。治标已收口,治本不阻塞。
- **判据**:run-state 里写一句评估结论(接 / 不接 + 理由);若接了,`lto audit` 能走 tmux 调度且 findings 正常回收;若不接,backlog ⑪ 更新「治本评估结论 + 留的接口」。

---

## 执行顺序 + 每 Phase 收口动作

1. **Phase 1 先做先收口**(治标,30 倍提速,独立可交付)。收口:`cargo fmt --check`+`clippy -D warnings`+`test --locked --all-targets` 全绿 → `lto audit --auto-dispatch --discover-risks` 跨族异构审计本批 diff(HIGH/CRITICAL 消解)→ `lto check --to closed --strict` PASS → commit。
2. **Phase 2 评估后做**(治本,可能只到「留接口」)。同样收口动作。
3. 两 Phase 可同一 thread 跑(都围绕 runner 调度,文件相关),但**各自独立 commit**。
4. backlog ⑪ 状态更新(✅ 治标已实现 / 治本到哪一步),CHANGELOG 记一笔。

---

## 提醒(复用什么别重写 / 安全阀)

- **复用** pi.sh 已有 env 注入模式(`LTO_PERM_SANDBOX`/`LTO_PERM_TOOLS` → `PERM_ARGV`),`LTO_LEAN_CONTEXT` 照同样套路,别新造机制。
- **复用** v0.5.0 已落地的 `runner: "tmux"` 路径(`src/tmux_runner.rs`),治本别重写 tmux 调用。
- **复用** O2 已接线的 `events.jsonl` 看耗时对比,别新写计时。
- **安全阀(写死,必须)**:① 异构 failover 不动 ② read-only 权限白名单不动 ③ 治标实测 token 对比是硬判据(不是「我加了 flag」就算完,要 grep 实测降了)④ host 亲验是硬停止点。
- **dogfooding**:这份 goal 本身用 `lto audit` 收口——若加了 `LTO_LEAN_CONTEXT` 后 audit 自己跑不动了,就是引入了 bug,优先修。
