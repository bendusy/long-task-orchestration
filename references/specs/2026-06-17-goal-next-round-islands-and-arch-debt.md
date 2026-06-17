# Goal: 接线孤岛模块 + 补齐四层 loop 反馈边 + 抽象债清理（下一轮）

> 致 pi（本轮落地执行方 = pi；异构审计方 = codex / agy，不派 pi 自审）：沿用约束（LTO 自管 / 每 Phase 收口派 codex+agy 异构审计 / dogfooding / 红线不弱化 / commit 你写、release/tag/push 归 host）。
> 这份按 Phase 切，**每个 Phase 独立可收口可 commit。做完一个 Phase 停下 commit，再开下一个，别一口气啃完所有 Phase**（长 thread 精度掉，pi deepseek thinking 尤其吃 context）。
> **执行顺序建议**：先做高价值三家共识项 → BUG-7（state/events 双写不一致）→ BUG-2（events O(N) I/O）→ Phase C（② model 维度）；再做孤岛/其余 bug；抽象债（Phase E runner_plan）可选最后。
> **每 Phase 收口必跑红线**：`cargo fmt --all --check` + `cargo clippy --locked --all-targets -- -D warnings` + `cargo test --locked` + `python3 scripts/check_docs_consistency.py` + `python3 scripts/check_python_rust_ownership.py`；然后派 codex/agy 异构审本 Phase diff（你是落地方，审计要异构，别自审）。host 会在每个 Phase commit 后亲验，不信自述。

## 为什么做（第一性）

backlog ①-⑫ 几乎全标 ✅，但用 LTO 综合 3 个异构子代理（孤岛猎手 / 架构审 / 路线考古）+ host 逐条亲验后，发现两类真问题：
1. **已开发但未接线的孤岛**——能力写好了但没接到任何流程（agy 事件解析、4 个未挂载插件、4 个零调用函数）。
2. **声明已实现实则降级/失真**——backlog 承诺的能力代码没完整兑现（⑦ model 维度、③ autonomous gate 数据驱动）。

> ⚠️ **亲验纠错记录**（防 codex 重蹈）：路线考古子代理报了 5 个"失真"，host 亲验后 **2 个是误判**：
> - ⑪ session id **不是**硬编码 "1"——`audit_dispatch.rs:88-89` 真注入 `audit_session_id(run_id, runner)`，考古把 lean context 的 `LTO_LEAN_CONTEXT=1` 错当 session id。**⑪ 属实，不要动。**
> - ⑥ recap 渲染**不是**未实现——`recap.rs:376-377` 真调 `cross_run_mining`+`render_mining_brief`，host 实跑出过完整 brief。**⑥ 属实，不要动。**
> 下面列的都是 host 亲验**坐实为真**的，每条带 file:line。别信任何二手"失真"声明，自己 grep 验。

## ⚠️ 异构审（codex + agy 跨族）新揪出的真 bug —— 优先级最高

Claude 子代理（同族）漏掉这些，跨族 codex/agy 独立审 + host 亲验坐实。**这些是本轮第一优先**（孤岛/抽象债靠后）：

- **BUG-1 scheduler 管道写端泄漏挂死（agy，host 亲验坐实，中危）**：`scheduler.rs:457` `kill_child_group` 杀进程后，`stdout_task.await`（`scheduler.rs:462`）/ `stderr_task.await`（`:467`）**无超时保护**。若被杀进程的孙进程脱离进程组且仍持 stdout 写端，`drain_pipe` 的 `read().await` 收不到 EOF → 这两个 `.await` 永久阻塞 → 整个 job 收口挂死。触发条件较窄（需孙进程脱组持写端），但一旦命中 LTO 调度进程整体卡死。**修**：给 `stdout_task.await`/`stderr_task.await` 加 `tokio::time::timeout`（如 5s），超时则放弃读残余输出而非无限等。
- **BUG-2 events emit O(N) I/O 放大（agy，host 亲验坐实，中危/性能）**：`events.rs:84` 每次 `emit` 调 `count_file_events`（`events.rs:159`）→ `fs::read_to_string` 全文件读 + 切行计数，只为拿顺序 event_id。emit 频繁 / 行数逼近 `HARD_STOP_AT`(50k) 时每次 O(N) 磁盘读，I/O 雪崩。**修**：event_id 不靠全文件计行——改增量计数（持锁时维护内存 counter / 或读文件尾部 seek / 或用 append 不依赖序号的 id）。注意：改动要守住 `safe_emit` 的 fail-closed 和 `.events.lock` 持锁语义（⑫ 已加固，别回退）。
  - **✅ 已完成（pi 落地 `ec303a5` + 审计返工 `160c45a`，host 端到端亲验 + codex/agy 两轮异构审）**：改用 `.events.count` 计数文件 O(1) 读写，HARD_STOP/WARN 读 counter，首访 fallback 迁移老 run，counter 写在 event append 前（crash 只产生无害 gap）。**异构审揪出 2 个引入型 bug 已返工修掉**：① counter 损坏静默归零→丢事件（`events.rs:194` 原 `unwrap_or(0)`）→ 改自愈（删损坏文件 + 回退 count_file_events）；② count() 无锁→改纯读。host 端到端亲验：造 `GARBAGE` 损坏 counter 实测自愈（counter 从 4 续到 5 非从 1，重复 event_id=0）+ test 200+1 全绿。
  - **⬜ 已知改进项（codex 第二轮揪，host 校准为非阻断，记录不返工）**：① **半写留合法数字前缀**：`fs::write` 半写若截成合法小数字（`1234`→`1`）parse 成功不触发自愈→理论仍可能重复 event_id。极罕见（小文件 write 多为单 syscall），作硬化项不阻断。② **count() 非严格纯读**：损坏路径 `fs::remove_file`（`events.rs:204`）是写副作用，与 docstring "Pure read" 不符——改 docstring 或把删文件挪出 count 路径，小事。两点边际收益递减，不搞第三轮返工（auditor 会越钻越深，区分阻断 bug vs 理论边界）。
- **BUG-3 worktree 异常早退泄漏持久 worktree（codex，待 host 深核）**：codex 报 scheduler worktree 路径异常早退会泄漏 persistent worktree。**codex 落地前 host 先亲验**：grep `worktree.rs` 的 `WorktreeHandle`/`Drop`/`prune`，确认早退路径真不清理才修。
- **BUG-4 派工结果只进 events/telemetry 不进 state.agent_runs（codex，高价值，与 ③ 联动）**：codex 报 scheduler 派工结果只 emit 到 events/telemetry，**不写 `state.agent_runs`**。而 `autonomous_gate`（③）和预算闸门**正是数 `state.agent_runs`**——意味着跑了很多 agent 但 gate 看不到，gate 判断失真。**这条与 Phase D（③ autonomous_gate）是同一问题的两面**：gate 不准既因为它只计数不读 ⑥，也因为它数的 `agent_runs` 根本没被 scheduler 回填。**host 亲验**：grep scheduler 结果回收处有无 `state.agent_runs` 写入；若真没有，这比"gate 读 ⑥"更根本——先让派工结果如实落 state，gate 才有真数据可数。
- **BUG-5 .events.lock 孤立锁导致 run 永久卡死（agy，host 亲验部分坐实，低概率高后果）**：`.events.lock` 是磁盘实体文件锁（`events.rs:185` O_EXCL 创建 / `:173` Drop 删除），**无 stale 锁检测**。若 LTO 进程被 `kill -9`/OOM/掉电/panic，`EventsLockGuard::Drop` 不执行 → 锁残留 → resume 时所有 emit 卡自旋 + 5s 超时 fail-closed → **该 run 永久卡死在事件写入无法恢复**。这是 ⑫ fail-closed 加固的真实副作用（防了脏写但没处理孤立锁）。触发需进程异常死亡且恰好持锁，概率低但后果是 run 不可恢复。**修**：加 stale 锁检测——锁文件写入持有者 pid + 时间戳，acquire 时若持有者进程已死或锁超龄（如 > timeout 的若干倍）则强夺清理。**红线**：不准回退 ⑫ 的 fail-closed（`events.rs:211` 拒绝无锁脏写要保留），stale 检测是叠加不是替换。

- **架构观察 派工 sh 脚本 vs CLI 内置（研究 run `20260617-075932-sh-lto-cli`，非 bug，供未来权限批参考）**：派工走 `scripts/delegate/runners/*.sh` 而非 Rust CLI 内置，是**有意分层**：CLI（`scheduler.rs`）管通用调度（找脚本/注入 env/spawn/捕获 reply/并发/重试/healthcheck 汇总），sh 管每家 runner 的方言适配（codex `exec -s` / pi `-p --provider --tools` / claude `--allowedTools --permission-mode` / agy `--sandbox`，四家 flag+权限机制+token schema 全不同）。符合"薄 harness + runner 是 affordance"哲学，且 runner CLI 演进快（codex.sh 注释 "flags change over time" + 内置 flag 探针），sh 改一行不用重编。**真代价**：① 权限逻辑两边重复（`agent_job.rs::readonly_intent_to_policy` + 各 sh 脚本各一份"agy 无 read-only/pi 工具列表"，改一处要同步两处）② shell 坑（fail-silent Python token 解析、bash 数组 tricky 扩展、`<<<` 非 POSIX）③ 跨平台锁死 Unix。**判断（不单独重构，ROI 低）**：权限**决策**该收进 Rust（四家可穷举，单一来源消除两边不同步），token schema 该收进 Rust（类型安全可测）；但 flag **翻译**留 sh（演进快）。这与下方"架构债：权限模型四家不通约"是同族——**合并进未来权限批一起做**（那时本就要动 `agent_job.rs` 权限抽象），现在不为它单独动刀。

- **BUG-6 pi runner 派工姿势问题（host 实测深究，真因是非 TTY 调用，非模型）**：pi 长期"调不动"，**host 逐层实测推翻多个错误归因**：
  - ❌ "pi CLI 不能用" → 证伪：`pi -p "1+1"` / `pi "1+1"` 都秒回 `2` rc=0。
  - ❌ "deepseek-v4-pro 模型卡死" → 证伪：单轮秒回。
  - ❌ "thinking level 爆炸" → 是表象，非真因。
  - ✅ **真根因（host 实测坐实）**：派 pi 时用了 `pi "$(cat ...)" | tee` —— **管道 `| tee` 把 pi TUI 的 stdout 变成非 TTY**，pi 检测到非终端就把交互 TUI 降级成单发/静默，无回显、reply 抓空。codex/agy 正常正因为它们在 pane 里直接跑（TTY）。**正确姿势**：tmux pane 直接起裸 `pi` 进真 TUI（不带 `-p`、不带管道，纯 TTY，状态栏显示 `(deepseek) deepseek-v4-pro • high` + `ctx ~29k/1.0M`），再用 `dispatch_agent.sh -m sentinel` 交付 prompt + 哨兵——和 codex 完全一样。实测这样派 pi 正常 `Working`。
  - **次要优化（非阻塞）**：pi.sh（`:61` deepseek-v4-pro，默认 thinking high）若用于 headless 批量审计，可按 job kind 设 `--thinking low/medium` 提速；但这是优化不是 bug 根因。
  - **修方向**：① 修 `pi.sh` / dispatch 路径——确保 pi 永远在 TTY 下跑，绝不用管道捕获 TUI 输出（reply 走 dispatch sentinel + 从 TUI/log 捞，不靠 `| tee`）。② 文档化 pi 正确派工姿势（真 TUI + sentinel）。
  - **价值**：pi 长期"不可用"实为派工姿势错，修好后异构 runner 池恢复四家，audit/judge 跨族能力完整。

- **BUG-7 state/events 双写不一致（三家全命中，host 亲验坐实，高危——本轮最该修）**：`ops.rs:1533` `save_run` 后 `:1535` `safe_emit` 两步无事务，`safe_emit` 失败静默吞错（`events.rs` eprintln + 返回 None）→ `state.agent_runs` 写了但 `events.jsonl` 没写 → `autonomous_gate`（读 state）与 `cross_run_mining`（读 events，`telemetry.rs:242`）**永久分歧**，两个 reader 报不同 {runs, results}。这是 BUG-4 的精确化：不只"不写 state"，而是两个真源会漂。**codex+agy+pi 三家独立全部命中**，最高信任。**修方向**：让 autonomous_gate / cross_run_mining 读同一数据源（events 为投影则两者都从 state 或都从 events 派生），或两处读取加一致性校验告警；保留 ⑫ 的 safe_emit fail-closed，但 emit 失败要让 state 写入也可感知（不能一方静默成功一方静默失败）。
  - **✅ 已完成（pi 落地 commit `76b1022`，host 亲验 + codex/agy 异构审通过）**：修法=safe_emit 移到 save_run 前 + 检查 `emitted.is_none()` 则 `anyhow::bail!`（state 不写）+ 保留 ⑫ fail-closed（向调用端传播而非静默）+ 测试 `collect_agent_run_bails_when_events_emit_fails...`。host 亲验：fmt/clippy/test(194+1) 全绿。codex+agy 独立审一致结论：正向单向分歧（state 多 event 少 = 审计盲区）已消除；fail-closed 未破坏反而补全。
  - **⚠️ 已知可接受残留（异构审一致裁定，不返工）**：反向不一致（emit 成功后 `save_state` 失败 → event 多 state 少）仍在，因 `state.rs::save_state` 是直接 `fs::write` 无事务、events.jsonl append-only 无跨文件 ACID。agy 论证：state 是真源 events 是投影，投影多一条悬挂事件**偏安全**（resume/重试可幂等去重），要在 append-only 上回滚是引入海量复杂度，当前折中合理。**未来若上 ACID/WAL 再处理**。可选小增强：补反向测试用例（emit 成功 save 失败）。
- **BUG-8 多处错误静默吞没（pi 挖，host 部分亲验，中/低危）**：`scheduler.rs:455` `stdout_task.await...unwrap_or_default()` 吞 drain_pipe panic/IO 错误；heartbeat 文件 `{job_id}.hb.jsonl`（`scheduler.rs:307-312`）创建后无清理逻辑、`.lto/<run>/live/` 缓慢积累（坐实，低危）；timeout 后 `read_to_string(reply_path)...unwrap_or_default()` 把半截 reply 当空（低概率）。**host 亲验校准**：pi 称 stdout_text 错误吞没会致"rate-limit 误判"——**证伪**：`scheduler.rs:724` rate-limit 检测用的是 `stderr`/`reply_text` 不是 stdout_text，归因有误，严重度降级。pi 报的 `tmux_runner.rs:509` sentinel 读吞错行号漂移（实际是 ready_patterns 检测），**待重新定位验证再决定修不修**。**修方向**：heartbeat 文件在 job 收口时清理（确定要修）；其余错误吞没改为至少 log，不静默（确认真才改）。

> **三方共识（codex+agy+pi 独立都确认 + host 亲验）**：① runner_events 全模块死代码、② cross_run_mining 无 model 维度、③ autonomous_gate 只计数不读挖掘、**state/events 双写不一致（BUG-7，三家都报）**。高共识强信任，下面 Phase 照做。
> **本轮异构审实操教训（写给 codex/host）**：跨 runtime 派 agent 走 tmux 真实 session（headless 闷死看不见卡点）；任务太大是超时真因（缩粒度 > 加 timeout）；pi 复杂任务慢见 BUG-6（短期审计优先 codex+agy 快 runner，pi 修好 thinking 前不做关键路径阻塞）。
> **处置铁律**：上述 BUG-1~4 codex 落地前，host 必逐条亲验真伪 + 校准严重度（异构 auditor 会夸大/误报，已知偏差）；误报记录不改，真 bug 才修。

## 核心架构裁决（host 先定，别让 codex 猜）

- **薄 harness 红线不动**：recap --mine 已确认只读（`recap.rs:384` 印"不写配置/不改 runner 优先级/不自动 promote"）。本轮**所有**新增能力同样守只读+人在环，L4 信号只产出 brief 给 host，**绝不自动改 config/promote/降权 runner**。
- **孤岛处置二分法**：每个孤岛先判「接线」还是「删除」——有真实消费场景的接线，YAGNI 的直接删（别留装饰性死代码）。判断依据写在每条里。
- **抽象债本轮只做最痛的一处**（runner_plan 硬编码），其余记录不做——避免洁癖式重构摊大。

---

## Phase A：接线/删除 4 个函数孤岛（小，先做，热身）

host 亲验坐实（grep 全仓定义数 > 使用数，扣除测试）：

| 孤岛 | file:line | 亲验结论 | 处置 |
|---|---|---|---|
| `parse_agy_stdout` | `src/runner_events.rs:64` | pi/codex/claude 三家 parser 有测试调用，**唯独 agy 零调用（连测试都没有）**；且整个 runner_events 模块无生产调用 | 见下 Phase B（agy 解析孤岛是更大问题的一角，本 Phase 先补 agy 的测试对齐其他三家，或随 B 一起接线） |
| `command_with_args` | `src/process.rs:83` | 泛型 helper 零外部调用 | 判断：有无未来调用方？无则删 |
| `ledger_sequence` | `src/commands/util.rs:693` | 格式化函数逻辑完整但零调用 | 判断：ledger 渲染是否该用它？该用则接线，否则删 |
| `os_strs` | `src/commands/util.rs:797` | &str→OsStr 预留转换器零接入 | YAGNI 倾向删 |

**完成判据**：每个孤岛要么有了生产调用路径（grep 使用数 > 0），要么从代码删除（grep 定义消失）；`cargo build` 无 unused warning；`cargo test --locked` 全绿。

## Phase B：agy runner 事件解析未接进生产流（中）

**缺陷**：`src/runner_events.rs` 的四个 parser（pi/codex/claude/agy）**整个模块无任何生产调用**（grep `runner_events::` 只有定义，零 caller）。这意味着 runner stdout 的结构化解析能力写了但没接进 scheduler/agent_exec 的事件 emit 流——runner 完成事件可能丢了 stdout 里的结构化信号（token 用量/turn 边界等）。

**落点**：
- 盘清 scheduler.rs / agent_exec.rs 现在如何从 runner 回收结果（`grep -n "stdout\|reply\|AgentResult" src/scheduler.rs src/agent_exec.rs`）。
- 判断 runner_events 的 parser 该接到哪个回收点，让四家 runner 的 stdout 都过对应 parser → 结构化字段进 AgentResult/events。
- agy 补齐：让 `parse_agy_stdout` 与其他三家对等（有生产调用 + 测试覆盖）。

**架构岔路 host 裁决**：若发现 runner_events 是「设计了但当时没接、现在 AgentResult 已用别的方式拿到等价信息」→ 那是冗余孤岛，**删模块**别硬接；若 AgentResult 确实缺这些结构化字段 → 接线。codex 先 grep 实证哪种情况，在 commit message 说明判断依据。

**完成判据**：runner_events 模块要么有生产调用（grep caller > 0，四家对等），要么整体删除；`cargo test` 全绿。

## Phase C：补齐 ⑦ model 维度到 cross_run_mining（中，四层 loop 真缺口）

**缺陷**（host 亲验坐实）：`AgentResult.model` 字段加了（`agent_job.rs`），但 `cross_run_mining` 的 slot key 是 `(String, String, String)` = `(runner, task_type, time_window)`（`telemetry.rs:246`），**无 model 维度**。backlog 第79行承诺"按 runner 模型分组（哪个模型在哪类任务有效），不只按 category"**未兑现**。

**为什么重要**：这是 L4「越用越聪明」的核心——同一 runner 换 model（如 codex 背后 gpt-5.5 vs 其他）效果不同，不分 model 就挖不出真实有效性。⑦ 字段是为 ⑥ 服务的，现在字段存了但挖掘不用，半截工程。

**落点**：
- slot key 从 3 元组扩到 4 元组 `(runner, model, task_type, time_window)`（`telemetry.rs:246` + `record_runner_finished`/`record_agent_turn_completed` 的 key 构造 + entry 输出 `telemetry.rs:285`）。
- model 从 events.jsonl 的 runner.finished 事件里读（确认 `event_emit.rs` 真写了 model 字段——亲验过写了）。
- `render_mining_brief`（`recap.rs:383+`）表格加 model 列。
- model 缺失时优雅降级（老 run 无 model 字段 → 标 "unknown"，不 panic）。

**完成判据**：`recap --mine` 输出表格有 model 列；构造一个带 model 的 events.jsonl fixture 测试断言按 model 分组；老 run（无 model）不崩、标 unknown；`cargo test` 全绿。

## Phase D：③ autonomous_gate 升级为证据驱动（中，但守红线）

**缺陷**（host 亲验坐实）：`autonomous_gate`（`src/commands/ops.rs`）只数 `agent_runs` 计数（`runs>=5 && results>=10`），**完全没读 cross_run_mining**。backlog 第15行"证据闸门读⑥"失真——它是个低端计数门。

**落点**：让 gate 除了计数，还读 `cross_run_mining` 的派生信号——若挖掘出高失败率/未收敛的 runner 信号，gate 应更保守（提示 host 而非放行）。

**⚠️ 红线（本 Phase 最易做歪）**：autonomous gate 升级**不等于**让 LTO 自动决策。gate 只做「是否允许进入 autonomous 模式」的**事实闸门**——读 ⑥ 信号当作更严的准入证据，**仍然 fail-closed**（信号不足/有风险 → 拒绝放行，交回 host）。**绝不**让 gate 读了 ⑥ 就自动调 runner 权重/自动 promote。autonomous 模式本身的「机械执行」边界不变（不 spawn 决策 agent）。

**架构岔路 host 裁决**：若深核发现 autonomous 模式当前真实使用率为零、且升级 gate 收益不明 → 记录为「保持计数版，文档说清 gate 不读 ⑥ 是有意为之」也是合法结论。codex 先判这个 Phase 值不值做（autonomous 有没有真实使用场景），不值就降级为「文档澄清 + backlog 第15行改为属实描述」，别硬塞数据驱动。

**完成判据**：要么 gate 真读 ⑥ 信号且 fail-closed（测试断言高风险信号下 gate 拒绝），要么 backlog 第15行改成与代码一致的诚实描述 + 文档说明为何不读 ⑥；`cargo test` 全绿。

## Phase E：runner_plan 硬编码抽象（中，抽象债最痛的一处）

**缺陷**（host 亲验坐实）：`dispatch_goal.rs:201` + `:524` 两处 `match runner` 硬编码 codex/pi/agy 的 launch/prompt/ready/confirm/hook 逻辑。加第四家（如 claude）到 dispatch-goal 必须改 dispatch_goal 代码多处。

**落点**：抽出 `GoalRunnerProfile { launch, prompt_template, ready_patterns, confirm_patterns, needs_probe, hook_provider, completion_event }`，dispatch_goal 变通用驱动，新 runner 通过 profile 注册而非散弹改代码。

**架构岔路 host 裁决**：本 Phase **可选**——若 codex 评估「短期不会加第四家 dispatch runner，抽象收益 < 重构风险」→ 记录为「已识别，暂不做，加 runner 时再抽」也合法（YAGNI）。但**插件孤岛（4 个未挂载插件）必须本轮处置**，见 Phase F。

## Phase F：4 个未挂载插件的去留裁决（中）

**缺陷**（host 亲验坐实）：`adversarial-audit` / `claim-verify-research` / `meeting-transcript` / `migration-refactor` 四个插件 grep 全仓**零代码引用**（只有 README 提及 + 完整 manifest/profile/eval），从无 mount/eval-run 调用路径。对比 `deep-agent-profiles`/`dev-workflow` 有代码引用（不是孤岛）。

**落点**：每个插件二选一——
- **有真实工作流场景** → 在 workflow-playbook.md 写清触发信号 + 给一个 `plugin mount` 实跑示例（让它至少有一条文档化的使用路径），并在某个测试/smoke 里 validate。
- **无场景/重复** → 从仓库删除（plugins/ 目录 + README 引用），别留装饰性插件冒充能力。

**架构岔路 host 裁决**：`adversarial-audit` 大概率该留（对抗审计是 LTO 核心卖点），但要给它真实接线示例；`meeting-transcript` 在开源 LTO 里大概率是私有领域时代残留（**注意隐私边界——若插件内容含私有业务痕迹必须删，不只是不挂载**）。逐个判，commit message 说明每个的去留依据。

> ⚠️ **隐私红线**：处置 meeting-transcript 等插件时，扫描插件内容确保无私有业务领域痕迹，有则连内容一起删干净（去敏感内容的 diff 应全是删除行）。

---

## 执行顺序 + 每 Phase 收口动作

A（热身小孤岛）→ B（agy 事件接线）→ C（model 维度，四层 loop 真缺口，价值最高）→ F（插件去留，含 yh 红线）→ D（autonomous gate，可降级）→ E（runner_plan 抽象，可选）。

每个 Phase 收口必跑：
```
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 scripts/check_docs_consistency.py
python3 scripts/check_python_rust_ownership.py
lto audit --auto-dispatch   # 异构审本 Phase diff（快 runner 优先：codex/agy，pi 留补充不阻塞）
```
收口后 commit（你写），停下，再开下一个 Phase。release/tag/push 归 host。

## 提醒（复用什么别重写 / 安全阀）

- **复用**：cross_run_mining 的 slot 机制（`telemetry.rs:242+`）扩维度别重写；render_mining_brief 表格（`recap.rs:383+`）加列别另起渲染。
- **不可自动化的安全阀**：① host 亲验是硬停止点（codex 报全绿，host 必自跑红线 + grep 实证 + 实跑 recap --mine）；② 薄 harness 红线（L4 只读、人在环、fail-closed）写死不弱化；③ yh 隐私扫描（Phase F）。
- **孤岛删除前先确认**：删任何函数/模块/插件前，grep 全仓 + 看 git log 确认不是「正在被另一未完成功能引用」，删的 diff 应是纯删除行。
