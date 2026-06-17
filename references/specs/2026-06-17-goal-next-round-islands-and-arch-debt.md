# Goal: 接线孤岛模块 + 补齐四层 loop 反馈边 + 抽象债清理（下一轮）

> 致 codex（本轮落地执行方 = codex；异构审计方 = pi / agy，不派 codex 自审）：沿用约束（LTO 自管 / 每 Phase 收口派 pi+agy 异构审计 / dogfooding / 红线不弱化 / commit 你写、release/tag/push 归 host）。
> 这份按 Phase 切，**每个 Phase 独立可收口可 commit。做完一个 Phase 停下 commit，再开下一个，别一口气啃完所有 Phase**（长 thread 精度掉）。
> **每 Phase 收口必跑红线**：`cargo fmt --all --check` + `cargo clippy --locked --all-targets -- -D warnings` + `cargo test --locked` + `python3 scripts/check_docs_consistency.py` + `python3 scripts/check_python_rust_ownership.py`；然后派 pi/agy 异构审本 Phase diff（你是落地方，审计要异构，别自审）。host 会在每个 Phase commit 后亲验，不信自述。

## 📋 进度看板（截至 v0.6.0，2026-06-17）

**✅ 已完成（上一轮，已随 v0.6.0 发布）**：
- BUG-7 state/events 双写不一致（commit `76b1022`，host 亲验 + pi/agy 异构审通过）
- BUG-2 events O(N)→O(1) counter 文件（`ec303a5` + 审计返工 `160c45a`，host 端到端亲验）
- L3 dispatch-goal 完成事件 + L4 recap --mine（v0.6.0 主体）

**🔴 待做 BUG（本轮优先级从上到下）**：
| # | 问题 | 严重度 | 落地前必做 |
|---|---|---|---|
| ✅ BUG-4 | 派工结果不进 `state.agent_runs` → autonomous_gate 失真（与 Phase D 同一问题两面）— 已落地 run-scoped scheduler 回填 | 高 | host 先 grep 亲验是否真没回填 |
| ✅ BUG-1 | scheduler 管道写端泄漏 → 超时任务挂死 — 已加 bounded drain / bounded kill wait | 中 | await 加 `tokio::time::timeout` |
| ✅ BUG-5 | `.events.lock` 孤立锁 → run 永久卡死 — 已加 pid/owner/time stale recovery + hard-link reclaim，保留 live-lock fail-closed | 低概率高后果 | 加 stale 锁检测（pid+时间戳），不回退 ⑫ fail-closed |
| ✅ BUG-3 | worktree 异常早退泄漏 persistent worktree — 已坐实并加 scheduler cleanup guard | 中 | host 先亲验真伪再改 |
| ✅ BUG-8 | 错误静默吞没（heartbeat 文件泄漏确定修；其余确认真才改）— 已修 heartbeat 泄漏、stderr/stdout drain error 记录、tmux sentinel read fail-closed；reply 半截归因驳回 | 中/低 | 逐条核 |
| BUG-2 残留 | 半写合法数字前缀 / count() 非纯读 | 非阻断 | 顺手硬化，不专门返工 |
| ✅ BUG-9 | `collect-agent-run --status` 枚举 UX 差 — 已列 possible values，非法值给相似值提示，`returned` 兼容归一为 `ok` | 中（UX，高频踩） | 三处一起修：① `--help` 用 clap `value_parser`/`possible_values` 列出枚举 ② 错误信息附上合法值列表 ③ 考虑给 "returned"→"ok" 之类近义词容错或在错误里建议。改前确认 clap 版本支持 enum value_parser |

**🟡 待做 Phase**：
| Phase | 内容 | 价值 |
|---|---|---|
| ✅ C | 补 ⑦ **model 维度** 到 cross_run_mining（slot key 3→4 元组加 model）— 已落地并通过 R2 异构审计收敛 | 高（四层 loop 真缺口） |
| ✅ A | 接线/删除 4 个函数孤岛（parse_agy_stdout / command_with_args / ledger_sequence / os_strs）— commit `45178a9` | 小（热身） |
| ✅ B | agy runner 事件解析未接进生产流 — commit `45178a9` 删除 `runner_events` 整体孤岛，见本节裁决 | 中 |
| ✅ F | 4 个未挂载插件去留裁决（含隐私扫描私域插件）— 已删除私域残留，保留三类场景插件并补 mount 路径 | 中 |
| ✅ D | autonomous_gate 升级证据驱动（与 BUG-4 联动，守 fail-closed 红线）— gate 现在保留计数门并追加 cross_run_mining 风险门 | 中 |
| ✅ E | runner_plan 硬编码抽象 — 裁决并入未来权限/runner profile 批，不做半截抽象 | 可选（并入权限批） |

**🔵 架构债（同族，未来一个"权限批"一起做，本轮不单独动）**：权限模型四家不通约 + 派工 sh/CLI 权限决策收进 Rust + Phase E runner_plan 抽象。另：`lto release` plan 不含 Cargo.toml（发版工具增强项，目前 `scripts/release_preflight.sh` 兜住）。

**▶ 本轮建议执行顺序**：
1. **✅ BUG-4 + Phase D**（同一问题两面，一起做）——run-scoped scheduler results 回填 state，gate 读 mining 风险信号
2. **Phase C**（model 维度）——L4 越用越聪明的核心
3. **BUG-1 + BUG-5 + BUG-8**（scheduler/events 健壮性，一批）
4. **Phase A / B / F**（孤岛清理）
5. 架构债批（权限模型，最后，可单独立 GOAL）

> 每完成一项，在本看板对应行标 ✅ + commit hash。发版走 `references/release-workflow.md` + `scripts/release_preflight.sh`（host 做，你不 release/tag）。

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
  - **✅ 已完成（2026-06-17）**：scheduler post-exit drain 改为 bounded join，超时后 abort drain task 并在 `AgentResult.cost` 记录 `stdout_drain_error` / `stderr_drain_error`；timeout/stall 后 `child.wait()` 也加同一短超时，避免 kill 失败或 non-Unix no-op 卡住。回归 `escaped_stdout_holder_does_not_hang_scheduler_drain` 构造 `setsid` 脱组子进程继承 stdout，证明 scheduler 在 drain deadline 内返回。
- **BUG-2 events emit O(N) I/O 放大（agy，host 亲验坐实，中危/性能）**：`events.rs:84` 每次 `emit` 调 `count_file_events`（`events.rs:159`）→ `fs::read_to_string` 全文件读 + 切行计数，只为拿顺序 event_id。emit 频繁 / 行数逼近 `HARD_STOP_AT`(50k) 时每次 O(N) 磁盘读，I/O 雪崩。**修**：event_id 不靠全文件计行——改增量计数（持锁时维护内存 counter / 或读文件尾部 seek / 或用 append 不依赖序号的 id）。注意：改动要守住 `safe_emit` 的 fail-closed 和 `.events.lock` 持锁语义（⑫ 已加固，别回退）。
  - **✅ 已完成（pi 落地 `ec303a5` + 审计返工 `160c45a`，host 端到端亲验 + codex/agy 两轮异构审）**：改用 `.events.count` 计数文件 O(1) 读写，HARD_STOP/WARN 读 counter，首访 fallback 迁移老 run，counter 写在 event append 前（crash 只产生无害 gap）。**异构审揪出 2 个引入型 bug 已返工修掉**：① counter 损坏静默归零→丢事件（`events.rs:194` 原 `unwrap_or(0)`）→ 改自愈（删损坏文件 + 回退 count_file_events）；② count() 无锁→改纯读。host 端到端亲验：造 `GARBAGE` 损坏 counter 实测自愈（counter 从 4 续到 5 非从 1，重复 event_id=0）+ test 200+1 全绿。
  - **⬜ 已知改进项（codex 第二轮揪，host 校准为非阻断，记录不返工）**：① **半写留合法数字前缀**：`fs::write` 半写若截成合法小数字（`1234`→`1`）parse 成功不触发自愈→理论仍可能重复 event_id。极罕见（小文件 write 多为单 syscall），作硬化项不阻断。② **count() 非严格纯读**：损坏路径 `fs::remove_file`（`events.rs:204`）是写副作用，与 docstring "Pure read" 不符——改 docstring 或把删文件挪出 count 路径，小事。两点边际收益递减，不搞第三轮返工（auditor 会越钻越深，区分阻断 bug vs 理论边界）。
- **BUG-3 worktree 异常早退泄漏持久 worktree（codex，待 host 深核）**：codex 报 scheduler worktree 路径异常早退会泄漏 persistent worktree。**codex 落地前 host 先亲验**：grep `worktree.rs` 的 `WorktreeHandle`/`Drop`/`prune`，确认早退路径真不清理才修。
  - **✅ 已完成（2026-06-17）**：host + read-only explorer 坐实为“部分属实”：`add_persistent_worktree` 成功后、进入 `finalize_write_task` 前的 live-log/spawn 早退路径会泄漏，因为 `WorktreeHandle` 不是 RAII guard。scheduler 已加 `WorktreeCleanupGuard`，默认 Drop prune，只有 merge-review handoff 显式 disarm；回归 `write_task_spawn_failure_prunes_persistent_worktree` 构造 runner 路径存在但 spawn 失败，断言 `.lto/worktrees/<run>/<job>` 不残留。保留有意 handoff 路径：成功写入或测试失败但有 diff 仍 `keep=true` 并产出 `merge_review`。
- **BUG-4 派工结果只进 events/telemetry 不进 state.agent_runs（codex，高价值，与 ③ 联动）**：codex 报 scheduler 派工结果只 emit 到 events/telemetry，**不写 `state.agent_runs`**。而 `autonomous_gate`（③）和预算闸门**正是数 `state.agent_runs`**——意味着跑了很多 agent 但 gate 看不到，gate 判断失真。**这条与 Phase D（③ autonomous_gate）是同一问题的两面**：gate 不准既因为它只计数不读 ⑥，也因为它数的 `agent_runs` 根本没被 scheduler 回填。**host 亲验**：grep scheduler 结果回收处有无 `state.agent_runs` 写入；若真没有，这比"gate 读 ⑥"更根本——先让派工结果如实落 state，gate 才有真数据可数。
  - **✅ 已完成（2026-06-17）**：不改 run-agnostic `Scheduler`，在 caller 层为 run-scoped scheduler dispatch 回填 `state.agent_runs`。覆盖 `runner --prompt/--job-file`、`run parallel/pipeline --job-file`、`audit --auto-dispatch`、`audit --discover-risks`、`judge --execute` 和 autopilot tmux worker；普通 `runner --command` 仍作为 task evidence，不污染 agent runs；`plugin eval-run` 仍作为 eval/report 域，不写业务 run agent_runs。runner result events 改为 checked emit，event 写失败时不保存 state，延续 BUG-7 的 fail-closed 方向。
- **BUG-5 .events.lock 孤立锁导致 run 永久卡死（agy，host 亲验部分坐实，低概率高后果）**：`.events.lock` 是磁盘实体文件锁（`events.rs:185` O_EXCL 创建 / `:173` Drop 删除），**无 stale 锁检测**。若 LTO 进程被 `kill -9`/OOM/掉电/panic，`EventsLockGuard::Drop` 不执行 → 锁残留 → resume 时所有 emit 卡自旋 + 5s 超时 fail-closed → **该 run 永久卡死在事件写入无法恢复**。这是 ⑫ fail-closed 加固的真实副作用（防了脏写但没处理孤立锁）。触发需进程异常死亡且恰好持锁，概率低但后果是 run 不可恢复。**修**：加 stale 锁检测——锁文件写入持有者 pid + 时间戳，acquire 时若持有者进程已死或锁超龄（如 > timeout 的若干倍）则强夺清理。**红线**：不准回退 ⑫ 的 fail-closed（`events.rs:211` 拒绝无锁脏写要保留），stale 检测是叠加不是替换。
  - **✅ 已完成（2026-06-17）**：新 `.events.lock` 写入 `{pid, created_at_unix_ms, owner_exe}`；dead-pid、owner_exe mismatch 或 legacy stale lock 走 `.events.lock.reclaiming` advisory lock 串行化 + hard-link reclaim，并在删除前复核 reclaim 文件与 `.events.lock` 仍是同一 inode，避免并发强夺误删后来者新锁；crash 后孤儿 guard 文件可残留但 OS advisory lock 自动释放，不再走按路径删除 guard 的 TOCTOU 路径；live pid / fresh lock 继续 fail-closed，unknown/non-Unix 才按时间兜底。回归覆盖 fresh lock timeout、dead-pid JSON recovery、legacy empty stale recovery、busy reclaimer guard、orphan guard file recovery、hard-link identity replacement、owner_exe mismatch 与并发 event id 唯一性。

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
  - **✅ 已完成（2026-06-17）**：heartbeat task 停止后删除本 job `.hb.jsonl`，保留 `.log` 作为 durable live artifact；stdout/stderr drain join 错误不再 `unwrap_or_default` 静默吞掉，而是进入 `AgentResult.cost`；tmux sentinel 文件存在但 UTF-8 读取短重试后仍失败时返回 `TmuxRunnerError::Io`，不再退成空成功；empty sentinel 依赖 pane capture 时不再吞掉 capture 错误。审计期间另修 runner 适配层：`pi` lean audit 不再把 raw JSON thinking stream 写爆 live log，`agy` auth timeout 不再 rc=0 假成功。**驳回/不扩项**：stdout 吞错不会影响 rate-limit（只看 stderr/reply）；有效 UTF-8 半截 reply 不会被 `read_to_string` 清空；tmux timeout/fire capture 的 `unwrap_or_default` 只影响错误上下文或 fire-and-forget 设计，本批不改。
- **BUG-9 collect-agent-run 状态枚举 UX（host 实测坐实，中危/高频 UX）**：`collect-agent-run --status` 原 `--help` 不列合法值，非法直觉词 `returned` 报 `invalid job status` 且不提示合法值，导致 agent 容易 fallback 去掉 status。
  - **✅ 已完成（2026-06-17）**：`JobStatus` 暴露合法值，`collect-agent-run --status` 接入 clap possible values；非法值由 clap 给出 possible values 与相似值提示；`returned` 作为兼容 alias 接受但写入 state/events/stdout 时规范化为 `ok`，避免 telemetry slot 污染。回归覆盖 CLI parse/invalid-value 文案和 `collect_agent_run_accepts_returned_status_alias`。

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
| `parse_agy_stdout` | `src/runner_events.rs:64`（已随 commit `45178a9` 删除） | pi/codex/claude 三家 parser 有测试调用，**唯独 agy 零调用（连测试都没有）**；且整个 runner_events 模块无生产调用 | 已按 Phase B 删除分支收口：删除整个 `runner_events` 模块和 fixtures，不再补 parser 测试或生产接线 |
| `command_with_args` | `src/process.rs:83` | 泛型 helper 零外部调用 | 判断：有无未来调用方？无则删 |
| `ledger_sequence` | `src/commands/util.rs:693` | 格式化函数逻辑完整但零调用 | 判断：ledger 渲染是否该用它？该用则接线，否则删 |
| `os_strs` | `src/commands/util.rs:797` | &str→OsStr 预留转换器零接入 | YAGNI 倾向删 |

**完成判据**：每个孤岛要么有了生产调用路径（grep 使用数 > 0），要么从代码删除（grep 定义消失）；`cargo build` 无 unused warning；`cargo test --locked` 全绿。

## Phase B：agy runner 事件解析未接进生产流（中）

**当前裁决（2026-06-17，commit `45178a9`）**：Phase B 按下面的“冗余孤岛，整体删除”分支收口。Phase A 的异构审计先证实 `src/runner_events.rs`、四个 parser 和 `fixtures/runner/*` 全部只有自测调用、无生产 caller；随后 commit `45178a9` 删除整个模块、移除 `pub mod runner_events`，并删除 fixtures。当前生产路径不是 parser 模块，而是 `scheduler.rs` 调 runner 脚本、读取 `reply.txt`、合并 sidecar meta 到 `AgentResult.cost`，再由 `event_emit.rs` 投影 redacted runner 事件。仓库没有 `src/agent_exec.rs`。因此不再为 agy 单独补 parser 接线；若未来确有结构化 runner stdout 需求，应重新以 `AgentResult`/sidecar/event schema 为入口立新设计，不复活这份无 caller 模块。

**原缺陷（已关闭）**：`src/runner_events.rs` 的四个 parser（pi/codex/claude/agy）**整个模块无任何生产调用**（grep `runner_events::` 只有定义，零 caller）。这意味着 runner stdout 的结构化解析能力写了但没接进 scheduler/agent_exec 的事件 emit 流——runner 完成事件可能丢了 stdout 里的结构化信号（token 用量/turn 边界等）。当前裁决是删除冗余模块而不是接线。

**原落点（历史记录，勿作为待办执行）**：
- 盘清 scheduler.rs / agent_exec.rs 现在如何从 runner 回收结果（`grep -n "stdout\|reply\|AgentResult" src/scheduler.rs src/agent_exec.rs`）。
- 判断 runner_events 的 parser 该接到哪个回收点，让四家 runner 的 stdout 都过对应 parser → 结构化字段进 AgentResult/events。
- agy 补齐：让 `parse_agy_stdout` 与其他三家对等（有生产调用 + 测试覆盖）。

**架构岔路 host 裁决（已采用）**：若发现 runner_events 是「设计了但当时没接、现在 AgentResult 已用别的方式拿到等价信息」→ 那是冗余孤岛，**删模块**别硬接；若 AgentResult 确实缺这些结构化字段 → 接线。codex 已按前者处理：生产路径使用 `scheduler.rs` 的 `reply.txt` + sidecar meta → `AgentResult` → `event_emit.rs`，仓库没有 `src/agent_exec.rs`。

**完成判据（已满足）**：runner_events 模块要么有生产调用（grep caller > 0，四家对等），要么整体删除；`cargo test` 全绿。当前状态是整体删除，`cargo test --locked --all-targets` 通过。

## Phase C：补齐 ⑦ model 维度到 cross_run_mining（中，四层 loop 真缺口）

**缺陷**（host 亲验坐实）：`AgentResult.model` 字段加了（`agent_job.rs`），但 `cross_run_mining` 的 slot key 是 `(String, String, String)` = `(runner, task_type, time_window)`（`telemetry.rs:246`），**无 model 维度**。backlog 第79行承诺"按 runner 模型分组（哪个模型在哪类任务有效），不只按 category"**未兑现**。

**为什么重要**：这是 L4「越用越聪明」的核心——同一 runner 换 model（如 codex 背后 gpt-5.5 vs 其他）效果不同，不分 model 就挖不出真实有效性。⑦ 字段是为 ⑥ 服务的，现在字段存了但挖掘不用，半截工程。

**落点**：
- slot key 从 3 元组扩到 4 元组 `(runner, model, task_type, time_window)`（`telemetry.rs:246` + `record_runner_finished`/`record_agent_turn_completed` 的 key 构造 + entry 输出 `telemetry.rs:285`）。
- model 从 events.jsonl 的 runner.finished 事件里读（确认 `event_emit.rs` 真写了 model 字段——亲验过写了）。
- `render_mining_brief`（`recap.rs:383+`）表格加 model 列。
- model 缺失时优雅降级（老 run 无 model 字段 → 标 "unknown"，不 panic）。

**完成判据**：`recap --mine` 输出表格有 model 列；构造一个带 model 的 events.jsonl fixture 测试断言按 model 分组；老 run（无 model）不崩、标 unknown；`cargo test` 全绿。

**当前裁决（2026-06-17）**：已按最小接线分支落地。`cross_run_mining` 现在按
`runner × model × task_type × time_window` 分组；老事件缺失/空 model 时降级为
`unknown`。`agent.turn.completed` 不扩 CLI，但会在同一 run、同一 runner/task/time
槽位只有一个明确 `runner.finished` model 时继承该 model；多模型歧义保持
`unknown`，避免错误归因。`recap --mine` 表格和派生 WARN 均显示 model，
`collect-agent-run` 的 manual `runner.finished` 事件也补齐 `fields.runner/model`。
R1 pi/agy 审计指出 split-slot 和 WARN 缺 model；修复后 R2 pi/agy 返回 `[]`，
`audit_ledger_check --strict` 判定 `CONVERGED`。

## Phase D：③ autonomous_gate 升级为证据驱动（中，但守红线）

**缺陷**（host 亲验坐实）：`autonomous_gate`（`src/commands/ops.rs`）只数 `agent_runs` 计数（`runs>=5 && results>=10`），**完全没读 cross_run_mining**。backlog 第15行"证据闸门读⑥"失真——它是个低端计数门。

**落点**：让 gate 除了计数，还读 `cross_run_mining` 的派生信号——若挖掘出高失败率/未收敛的 runner 信号，gate 应更保守（提示 host 而非放行）。

**⚠️ 红线（本 Phase 最易做歪）**：autonomous gate 升级**不等于**让 LTO 自动决策。gate 只做「是否允许进入 autonomous 模式」的**事实闸门**——读 ⑥ 信号当作更严的准入证据，**仍然 fail-closed**（信号不足/有风险 → 拒绝放行，交回 host）。**绝不**让 gate 读了 ⑥ 就自动调 runner 权重/自动 promote。autonomous 模式本身的「机械执行」边界不变（不 spawn 决策 agent）。

**架构岔路 host 裁决**：若深核发现 autonomous 模式当前真实使用率为零、且升级 gate 收益不明 → 记录为「保持计数版，文档说清 gate 不读 ⑥ 是有意为之」也是合法结论。codex 先判这个 Phase 值不值做（autonomous 有没有真实使用场景），不值就降级为「文档澄清 + backlog 第15行改为属实描述」，别硬塞数据驱动。

**完成判据**：要么 gate 真读 ⑥ 信号且 fail-closed（测试断言高风险信号下 gate 拒绝），要么 backlog 第15行改成与代码一致的诚实描述 + 文档说明为何不读 ⑥；`cargo test` 全绿。

**当前裁决（2026-06-17）**：已选择“真读 ⑥ 信号”分支。`autonomous_gate`
保留原 `state.agent_runs` 计数门（>=5 run / >=10 result），并在通过后读取
`cross_run_mining`。mining 不可用、无 entries、dispatch 证据不足、仅主观样本、
出现 timeout/rate_limited 或高失败率（>=50%，且样本 >=3）时全部 fail-closed，
只返回 host-facing reason，不改 runner 权重、不写配置、不自动 promote。为支持
限流风险解释，`CrossRunMiningEntry` 增加 `rate_limited` 计数。

## Phase E：runner_plan 硬编码抽象（中，抽象债最痛的一处）

**缺陷**（host 亲验坐实）：`dispatch_goal.rs:201` + `:524` 两处 `match runner` 硬编码 codex/pi/agy 的 launch/prompt/ready/confirm/hook 逻辑。加第四家（如 claude）到 dispatch-goal 必须改 dispatch_goal 代码多处。

**落点**：抽出 `GoalRunnerProfile { launch, prompt_template, ready_patterns, confirm_patterns, needs_probe, hook_provider, completion_event }`，dispatch_goal 变通用驱动，新 runner 通过 profile 注册而非散弹改代码。

**架构岔路 host 裁决**：本 Phase **可选**——若 codex 评估「短期不会加第四家 dispatch runner，抽象收益 < 重构风险」→ 记录为「已识别，暂不做，加 runner 时再抽」也合法（YAGNI）。但**插件孤岛（4 个未挂载插件）必须本轮处置**，见 Phase F。

**当前裁决（2026-06-17）**：Phase E 不单独实现，正式并入未来“权限/runner profile 批”。理由：`dispatch-goal` 当前只有 codex/pi/agy 三家，runner launch/prompt/ready/hook 差异与“权限模型四家不通约、sh/CLI 权限决策收进 Rust”是同族问题；单独抽 `GoalRunnerProfile` 会先固化不完整权限语义，收益小于重构风险。下一次新增第四家 dispatch runner 或收敛权限模型时，再一起抽 profile，避免半截抽象。

## Phase F：4 个未挂载插件的去留裁决（中）

**缺陷**（host 亲验坐实）：`adversarial-audit` / `claim-verify-research` / 一个私域 transcript 插件 / `migration-refactor` 四个插件 grep 全仓**零代码引用**（只有 README 提及 + 完整 manifest/profile/eval），从无 mount/eval-run 调用路径。对比 `deep-agent-profiles`/`dev-workflow` 有代码引用（不是孤岛）。

**落点**：每个插件二选一——
- **有真实工作流场景** → 在 workflow-playbook.md 写清触发信号 + 给一个 `plugin mount` 实跑示例（让它至少有一条文档化的使用路径），并在某个测试/smoke 里 validate。
- **无场景/重复** → 从仓库删除（plugins/ 目录 + README 引用），别留装饰性插件冒充能力。

**架构岔路 host 裁决**：`adversarial-audit` 大概率该留（对抗审计是 LTO 核心卖点），但要给它真实接线示例；私域 transcript 插件在开源 LTO 里大概率是私有领域时代残留（**注意隐私边界——若插件内容含私有业务痕迹必须删，不只是不挂载**）。逐个判，commit message 说明每个的去留依据。

> ⚠️ **隐私红线**：处置私域插件时，扫描插件内容确保无私有业务领域痕迹，有则连内容一起删干净（去敏感内容的 diff 应全是删除行）。

**当前裁决（2026-06-17）**：`adversarial-audit`、`claim-verify-research`、
`migration-refactor` 保留为 host 主动选择的 data-only 场景插件；README 和
`workflow-playbook.md` 已补对应触发场景、`plugin mount` 示例和 `plugin validate`
静态验证入口，并用测试固定三者 validate / static eval / mount provenance。
私域 transcript 插件判定为私有领域残留，已删除插件文件和 `.gitignore`
忽略规则，避免未来私域材料绕过 `git status` / privacy scan 静默回流。

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
