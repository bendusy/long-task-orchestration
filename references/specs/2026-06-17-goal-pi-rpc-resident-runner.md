> # ⛔ 已证伪 / 勿执行（2026-06-17）
> 这份 goal 的核心前提是错的，**不要让任何 coding agent 执行它**。保留仅作调查记录。
>
> **为何证伪**：异构审 + 官方 docs(`pi/docs/rpc.md`+`sessions.md`)+ host 实测推翻它——
> 1. RPC 常驻进程「批内复用」的前提（同次 submit 有多个 pi job）在 LTO 异构 audit 主路径几乎不成立（一次 audit 派的是不同 runner，刻意异构）。
> 2. warm cache 的真实命中场景是「同 auditor 跨轮」，而跨轮是跨独立 `lto audit` 进程的。
> 3. **关键实测**：pi session 是持久 JSONL，`pi -p --session-id <id>` 让**新进程** resume 同 session 即命中 prompt cache（实测 cacheRead=1408，input 不膨胀）——**headless 就够，根本不需要 RPC 常驻进程那套工程**。
>
> **正解（已实现，取代本 goal）**：pi.sh 加 `--session-id`（`LTO_SESSION_ID` env，向后兼容），LTO 给 audit job 设稳定 per-(run,auditor) session-id。改动 ~3 行 bash + Rust env 注入，不碰 scheduler/并发模型。见 `backlog.md ⑪ 治本` 与 commit。
>
> （下方原始 goal 内容仅存档，协议探测细节仍有参考价值，但架构裁决全部作废。）

---

# Goal: pi RPC 常驻 runner —— 一批评审 job 复用一个 pi 进程，warm cache 省 token

> 致 codex:沿用全部既有约束(LTO skill 自管 / 每 Phase 异构审计 / dogfooding 铁律:lto 自己调不通=lto bug 优先修 / 红线不弱化 `clippy -D warnings`+`unsafe_code=forbid`+`cargo test --locked` 全绿 / commit 你写、release/tag 归 host)。
> **这份只做 pi RPC 常驻 runner(backlog ⑪ 治本)。做完停。别顺手改 backlog ⑫ 的存量 security 债,别动 tmux runner。**

---

## 为什么做(第一性)

backlog ⑪ 治标(`LTO_LEAN_CONTEXT` 砍冷启 context)已落地——pi 审计从 44s→3.2s。但那只是**杠杆②(砍冷启 context)**。更直接的**杠杆①是 warm prompt cache**:

- **省 token 真机制 = prompt cache 保持 warm**:同 session 重发 context 走 cacheRead(~10% 价),冷启/超 5min TTL 才付 full price。
- **实测(host 亲验,pi 0.79.3)**:`--session-id` 续接 Turn1 input1502 cacheRead0 → Turn2 input109 cacheRead1408(1408 走缓存,净增 109)。
- 当前 LTO 每个 audit job 走 `pi -p` headless **一发一收冷启**,session 不复用,每个 auditor job 都 full price 重载。一次 audit 派 2+ auditor + 多轮收敛 = 多次冷启浪费。

**目标**:让一批评审 job(audit/judge)复用一个常驻 pi RPC 进程——context 载一次,后续 job 走 warm cache。这是 ⑪ 治本的正路(治标 + 治本叠加:既砍 context 又 warm cache)。

---

## ⚠️ 必读:host 已亲验的协议真相(基于 pi 0.79.3,别照过时认知)

实测 `pi --mode rpc --session-id <id>`(私有 JSONL over stdio,**非标准 JSON-RPC**):

1. **发命令**:stdin 每行一个 JSON。prompt 命令 = `{"type":"prompt","message":"..."}` —— **键是 `message` 不是 `prompt`/`text`**。
2. **立即 ack**:`{"type":"response","command":"prompt","success":true}` —— 这只是"收到"回执,**不是 assistant 回复**。
3. **异步流式事件**(ack 之后):`agent_start`→`turn_start`→user `message_start`/`message_end`→**assistant `message_end`(reply 在这!)**→`turn_end`→`agent_end`。
4. **reply 提取**:assistant 角色的 `message_end` 的 `message.content[].text`(type==text 的拼接)。
5. **一轮完成信号** = `turn_end` 或 `agent_end`。RPC runner 按事件流判断 turn 完成,**不能靠 stdin EOF**。
6. **致命坑(host 实测踩到)**:**stdin 关闭会让 pi 提前退出**——发完 prompt 不能立刻关 stdin,要保持打开直到收完该 turn 的 `turn_end`,下一个 prompt 再写 stdin。
7. **严格 `\n` 分行**:每个命令一行 `\n` 结尾。**禁用 Node readline 式解析**(U+2028/U+2029 会被误当行分隔)。Rust 侧按字节 `\n` 切行,不要用会魔法处理 Unicode 行分隔的库。
8. **其他命令**:`{"type":"get_session_stats"}` 返 `data.contextUsage.percent` + `data.tokens.{input,output,cacheRead,cacheWrite}`(做 token 预算 / cache 命中验证);`{"type":"compact"}`(context 超阈值时压缩);`{"type":"abort"}` / `{"type":"steer"}` / `{"type":"set_model"}`。
9. **命令异步排队**:发 prompt 后立刻 `get_session_stats` 会返 0 tokens(turn 没跑完)。要等 `turn_end` 再查 stats。

> 真身:npm `@earendil-works/pi-coding-agent`,repo `github.com/earendil-works/pi`,协议文档 `docs/rpc.md`。**codex 实现前先复跑 host 的探测确认协议没变**(pi 版本可能更新):
> `(printf '{"type":"prompt","message":"reply PONG"}\n'; sleep 25) | pi --mode rpc --session-id probe-$$ --no-skills --no-context-files --no-extensions 2>/dev/null` —— 看 assistant message_end 出 PONG + turn_end。

---

## 核心架构裁决(host 已盘清，别另择)

**裁决 1:常驻进程生命周期 = 单次 `Scheduler::submit` 调用内。**
- host 已亲验:`Scheduler` 结构体**无状态**(只有 repo/runners_dir/config,无进程池字段),`submit(jobs)` 接一批 job 跑完即返。
- **所以**:一批 audit/judge job(同 run、同 submit)复用**一个** pi RPC 进程——进程在这批 job 开始时 spawn(载 context 一次),这批 job 依次走 RPC(warm cache),submit 返回前关闭进程。
- **不要**让 Scheduler 持久持有进程池跨 submit——那污染无状态设计 + 跨调用生命周期复杂。常驻只在一次 submit 的批内。

**裁决 2:接法 = 新 `src/pi_rpc_runner.rs` + scheduler 分支,照搬 tmux runner 同构模式。**
- host 已亲验:`scheduler.rs:324 if job.runner == "tmux"` 短路到 `tmux_runner::run_job`,其他走 `runners/{runner}.sh`。
- pi RPC 同构:`runner == "pi-rpc"`(或复用 `"pi"` + 一个 `LTO_PI_RPC=1` 模式标，**裁决 2a 见下**)分支到 `pi_rpc_runner`。
- **不要**让 bash pi.sh 管常驻 JSONL 连接(bash 管异步双向流笨重易错)——Rust 管常驻进程/tokio 异步流是强项,照 tmux_runner.rs 的 `Command` + 异步读写模式。

**裁决 2a:新 runner 名 `pi-rpc` vs 复用 `pi` + 模式标 —— host 倾向新名 `pi-rpc`。**
- 理由:① healthcheck/审计员选择/同族过滤都按 runner 名,`pi-rpc` 显式独立更清晰可观测;② `pi`(headless)保留作 fallback(RPC 起不来时降级 headless pi.sh);③ 异构族归类:`pi-rpc` 仍属 pi 族(family() 里映射 `pi-rpc`→同 pi 族,**别让它被当成新异构族破坏同族过滤**)。
- codex 评估后若发现复用 `pi` 名 + 内部切 RPC/headless 更省事且不破坏可观测性,可改——但 family 归类必须保证 `pi-rpc` 与 `pi` 同族(裁决依据:它就是 pi,只是调度方式不同)。

**裁决 3:批内 job 间复用，但失败要 fail-safe 降级。**
- RPC 进程起不来 / 协议异常 / 某 job turn 超时 → 该 job 降级到 headless pi.sh(或标失败让 scheduler 既有 failover 接管),**不拖垮整批**。
- 一个 job 的 turn 卡死(无 turn_end)→ 用 job 的 `budget.timeout_sec` 兜底,abort 该 turn,进程可继续下个 job 或重启。

---

## Phase 1:pi_rpc_runner 核心(常驻进程 + 协议 + 单 job 跑通)

### 1.1 新 `src/pi_rpc_runner.rs`
- `spawn` 一个 `pi --mode rpc --session-id <run-id-or-batch-id> --provider deepseek --model deepseek-v4-pro --no-skills --no-context-files --no-extensions`(read-only 时加 `--tools read,grep,find,ls`,沿用 pi.sh 的权限白名单逻辑)。
  - **注意**:`--no-skills` 等(治标)与 RPC(治本)叠加——RPC 进程也加 lean flag,起始 context 最小,后续 warm cache。
- 持有进程的 stdin(写命令)+ stdout(读 JSONL 事件流),tokio 异步。
- **保持 stdin 打开**直到这批 job 全跑完(协议坑 6)。
- 按 `\n` 字节切行解析事件(协议坑 7),不用 Node readline 式库。

### 1.2 单 job 跑通(发 prompt → 捞 reply）
- 发 `{"type":"prompt","message":<prompt>}`,读事件流到 assistant `message_end`,拼 `content[].text` 作 reply(协议坑 4),`turn_end` 作完成信号(坑 5)。
- 产出 `AgentResult`(照 tmux_runner 的 result 构造):reply_text、exit_code、elapsed、findings(若 output_schema 要求 JSON,reply 就是干净 JSON——这正是 RPC 优于 tmux capture 的点)。
- timeout 兜底:turn 超 `budget.timeout_sec` 无 turn_end → abort + 失败 result。
- **判据**:`pi_rpc_runner::run_job` 单 job 实测发 prompt 收到正确 reply;turn_end 后 result 正常。单元/集成测试覆盖:正常 reply、turn 超时 abort、stdin 不提前关。

### 1.3 接进 scheduler
- `scheduler.rs` 加 `runner == "pi-rpc"` 分支(照 324 行 tmux 模式),短路到 pi_rpc_runner。
- `family()`(audit.rs)映射 `pi-rpc`→pi 族(裁决 2a)。
- healthcheck:`pi-rpc` 健康探测(可复用 pi 的 healthcheck 或加 RPC ack 探测)。
- **判据**:`grep "pi-rpc" src/scheduler.rs src/audit.rs` 看到分支 + 族映射;同族过滤测试:pi 当 host 时 pi-rpc 被同族 skip(不破坏异构)。

---

## Phase 2:批内复用 + warm cache 实证(⑪ 治本的核心交付)

### 2.1 一批 job 复用一个进程
- 一次 `submit` 里多个 pi-rpc job → spawn 一次进程,依次发 prompt(每个 job 一个 prompt),收各自 reply,批末关进程。
- **裁决 1 落地**:进程生命周期 = 批内。注意 LTO 的 audit 一次 submit 可能是「不同 auditor 各一 job」——若都是 pi-rpc 才复用;混合异构(pi-rpc + agy)时 pi-rpc 那些复用同进程,agy 走自己的。**评估 scheduler 当前并发模型(concurrency)能否让同 runner 的 job 串到一个进程**——若并发架构使同 runner job 并行跑(各自要进程),批内复用要么串行化 pi-rpc job,要么起进程池。codex 评估后定:**优先简单(pi-rpc job 串行复用一进程),并发收益让位于 warm cache 省 token**(LTO 不图快,图省+确定性)。

### 2.2 warm cache 实证(硬判据，不是「我接了 RPC」就算完)
- 跑一个真实 audit(2+ 轮或 2+ auditor 用 pi-rpc),抓 `get_session_stats` 的 `tokens.cacheRead`:
  - 第一个 job:cacheRead≈0(冷)。
  - 后续 job:cacheRead>0 且显著(warm,context 走缓存)。
- **判据(必须有实测数对比)**:`events.jsonl` 或日志里记每个 pi-rpc job 的 cacheRead/input tokens,后续 job 的 cacheRead 显著>0。对比 headless 模式每 job input 都 full。**这是 ⑪ 治本的证明,没有这个实测对比 = 没收口。**

### 2.3 dogfood
- 这份 goal 收口本身用 `lto audit --auto-dispatch`(可指定 pi-rpc 当 auditor),亲验 RPC 路径真跑 + findings 干净回收(RPC 的 JSONL 比 tmux capture 干净——印证 backlog ⑪ 治本选 RPC 不选 tmux 的判断)。
- **dogfooding 铁律**:若 pi-rpc 接进后 audit 自己跑不动 = 引入 bug,优先修。

---

## 执行顺序 + 每 Phase 收口

1. Phase 1(单 job 跑通 + 接 scheduler)先收口:`cargo fmt --check`+`clippy -D warnings`+`test --locked --all-targets` 全绿 → `lto audit --auto-dispatch --discover-risks` 跨族异构审本批 diff(HIGH/CRITICAL 消解)→ `lto check --to closed --strict` PASS → commit。
2. Phase 2(批内复用 + warm cache 实证)收口同上 + **warm cache 实测数对比是硬判据**。
3. 两 Phase 各自独立 commit。
4. backlog ⑪ 更新「治本 ✅ 已实现(pi-rpc)」+ CHANGELOG 记一笔。

---

## 提醒(复用什么别重写 / 安全阀)

- **复用** `src/tmux_runner.rs` 的常驻进程 + tokio 异步读写 + AgentResult 构造模式(pi_rpc_runner 与它同构,别从零设计)。
- **复用** pi.sh 的权限白名单逻辑(read-only→`--tools read,grep,find,ls`)+ 治标的 lean flag(RPC 进程也加)。
- **复用** scheduler 既有 failover(RPC 失败降级走它)。
- **安全阀(写死,必须)**:① 异构同族过滤不破(pi-rpc 必须归 pi 族)② read-only 权限白名单不动 ③ warm cache 实测对比是硬判据 ④ stdin 不提前关 / `\n` 严格分行 / turn_end 判完成(协议坑,错了 reply 收不全)⑤ host 亲验是硬停止点 ⑥ RPC 起不来要 fail-safe 降级 headless,不拖垮整批。
- **不做**:不跨 submit 持久进程池(裁决 1);不让 bash 管 RPC 连接(裁决 2);不顺手改 backlog ⑫ 存量债。
