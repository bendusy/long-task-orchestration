# Goal(增量 O2): 事件覆盖面接线 —— 把哑的关键操作接上 events

> **致 codex**:这是 `2026-06-16-goal-observability-rust-implementation.md` 的 **O2 阶段**展开,
> 基于当前真实状态(O1 已落地:`src/events.rs` 地基 + 命令层 lifecycle 已 emit)。
> 沿用全部既有约束:LTO skill 自管、每 Phase 收口 `lto audit --auto-dispatch --discover-risks`
> 跨族异构审计、dogfooding 铁律(lto 自己 emit 出错=lto bug 优先修)、维护者验收四标准、
> 红线不弱化(`clippy -D warnings` / `unsafe_code = "forbid"` / `cargo test --locked` 全绿)。

---

## 当前状态(host 亲验,O2 的事实基础)

`src/events.rs`(289 行)已有 8 事件类型 + `safe_emit`/`emit` API + 「容忍未来类型」+ 体积闸门 + redact。已接线的是**命令层 lifecycle**:

| 已 emit(别重复接) | 落点 |
|---|---|
| `run.started` / `run.closed` | `cli.rs` / `closeout.rs` |
| `phase.changed` | `util.rs:543` |
| `task.created` / `task.status_changed` | `ops.rs`(1146/1220/1279/...) |
| `artifact.registered` | 各 save 点 |

**完全哑的子系统(O2 要接的,亲验 `grep -c safe_emit` 全 = 0)**:

| 子系统 | 文件 | 哑的关键时刻 |
|---|---|---|
| **runner 调度** | `scheduler.rs` | spawn / finished(rc+elapsed+timeout)/ 退出码分类 / 重试 / healthcheck 失败恢复 |
| **audit 派工** | `audit_dispatch.rs` / `audit.rs` | 审计员选择+同族过滤 / 健康发现者降级 / 批量提交 / findings 解析+合并 / 收敛轮次 |
| **gate/closeout** | `closeout.rs`(emit 了 run.closed,但 gate 判定哑) | ledger 拒绝 / unresolved blocks / risk 未验证 / 脏树 |
| **budget** | `budget.rs` | turns/tokens/deadline 超限 / warn 阈值触发 |
| **worktree 沙箱** | `worktree.rs` | semantic-judgement 拒绝 / network effect 拒绝 / push 阻断 / NEEDS_CONFIRM |
| **decision/judge** | `decision.rs` / `llm_judge.rs` | judge skip 原因 / 投票计票 / NeedsHuman escalate / findings 合并 |

---

## 照抄范式(O1 已确立的 emit 模式,O2 全部沿用)

```rust
crate::events::safe_emit(
    repo,
    &run_id,                       // ← 关键:run_id 怎么拿,见下面架构决策
    crate::events::EventRecord {
        event_type: "runner.finished".to_string(),
        actor_kind: "lto".to_string(),     // host / lto / runner
        summary: format!("{runner} rc={rc} {elapsed:.1}s"),
        object_id: Some(job_id.clone()),
        object_type: Some("runner_job".to_string()),
        fields: json!({"runner": runner, "rc": rc, "timeout": timed_out, "elapsed_sec": elapsed}),
        ..crate::events::EventRecord::default()
    },
);
```

- `safe_emit` 是 fail-safe(emit 出错不拖垮宿主)——所有 O2 接线**用 safe_emit 不用 emit**。
- `EventRecord::default()` 兜底,只填相关字段。
- raw 输出(stdout/stderr/reply)**禁止**进 `fields`/`summary`——`contains_raw_output=true` 会被拒写;要记就记摘要/rc/计数。
- redact 已在 events.rs 落盘前做,但你仍**不要**主动把路径/token 塞进 fields。

---

## ⚠️ 核心架构决策:scheduler 无 run_id(O2 最关键的设计点)

亲验:`Scheduler::submit(jobs)` **不接 run_id**——scheduler 是通用 job 执行器,被 `audit` / `parallel` / `pipeline` / `plugin eval-run` 多方复用,它本身不知道属于哪个 run。`Scheduler` 结构体有 `repo` 但无 run_id。

**这是真实的设计岔路,先决定再接,别乱接**。两个选项:

- **选项 A(推荐,host 已亲验可行):在调用方 emit,不在 scheduler 内部 emit。** 亲验 `AgentResult`(`agent_job.rs:394`)含 `job_id/runner/model/status/exit_code/findings` —— `runner.finished` 需要的字段调用方全拿得到。调用方(`audit_dispatch` / `ops.rs` 的 parallel/pipeline handler / `plugin eval-run`)拿到 results 后,在自己已有 run_id 上下文里 emit。
  - 优点:不改 scheduler 签名(它保持纯粹的通用执行器,不耦合 .lto 协议);run_id 在调用方天然可得;符合「scheduler 是 affordance 不是 owner」的架构原则。
  - 代价:`runner.started` 不好在调用方发(job 还没跑);可只发 `runner.finished`(有 result 就能发),`started` 若要发则需 scheduler 暴露一个轻量回调或事件 channel——**评估后若复杂度高,started 可延后,先把 finished + 分类 + 重试结果接上**(这些 result 里都有)。

- **选项 B:给 `submit` 加可选 `run_id: Option<&str>` 参数,scheduler 内部 emit。** 改签名,所有调用点传 run_id(eval-run 等内部用途传 None 不 emit)。
  - 优点:`runner.started`/healthcheck 失败恢复这种「过程中」事件能发(调用方拿不到中间态)。
  - 代价:污染通用执行器签名;所有调用点要改;违背 scheduler 不耦合协议的原则。

**你的第一步:在 run-state 里写一句决策(选 A 还是 B + 理由),再接线。** 我倾向 A(scheduler 保持纯粹),但若 healthcheck 失败恢复 / runner.started 这类中间态事件被判定为高价值且 A 发不出,B 可接受——以「scheduler 该不该知道 run_id」这个架构问题的答案为准,不是以省事为准。

---

## O2 接线清单(决策定了之后逐子系统接)

按价值排序(高→低),每个子系统接完单独 `lto check` + 可跑 `lto audit` 异构审计本批 diff:

### O2-1 runner 调度(最高价值,audit/parallel/pipeline/eval-run 全继承)
- `runner.finished`:rc + elapsed + timed_out + 退出码分类(用 `classify_exit` 已有的 timeout/rate-limit/signal 分类)。
- 重试:每次重试结果(retry_count 递增、退避)。
- healthcheck 失败/恢复(若选 B 或调用方能拿到 health probe)。
- 落点:按架构决策,选 A 在 `audit_dispatch.rs` / `ops.rs` parallel/pipeline handler / `plugin.rs` eval-run handler;选 B 在 `scheduler.rs:134 submit` 内。

### O2-2 audit 派工 + 收敛
- 审计员选择 + 同族过滤(对应 `audit_dispatch.rs` 的选择逻辑)。
- 健康发现者降级(codex unhealthy → 收缩到存活异构,这正是历史 failover bug 修过的路径,值得有事件)。
- findings 解析 + 合并(`decision.rs::merge_findings`)+ 收敛轮次(blocker 单调下降)。
- 事件类型:新增 `audit.dispatched` / `audit.finding` / `audit.converged`(靠「容忍未来类型」吸收,不改 events.rs 的固定校验——确认 events.rs 不是白名单硬拒未知类型;若是,放宽成「未知类型也收」)。

### O2-3 gate / closeout 判定
- check gate pass/fail(每个 gate 项的 OK/MISSING/WARN)。
- ledger 未 CONVERGED 拒绝 / unresolved blocks / risk 未验证 / 脏树检测。
- 事件类型:`gate.evaluated` / `gate.blocked`。

### O2-4 budget / worktree 沙箱
- budget:turns/tokens/deadline 超限、warn 阈值触发(`budget.rs`)。
- worktree:semantic-judgement 拒绝 / network effect 拒绝 / push 阻断 / `NEEDS_CONFIRM`(现在只 `println!`,补结构化事件)。
- 事件类型:`budget.exceeded` / `budget.warned` / `sandbox.rejected`。

### O2-5 decision / judge
- judge skip 原因(input 超限 / 同族 / 无异构)。
- 投票计票 / NeedsHuman escalate。
- 事件类型:`judge.skipped` / `decision.voted` / `decision.escalated`。

---

## ⚠️ 先决步骤:扩 `PHASE1_EVENT_TYPES`(host 已亲验,这是硬阻塞)

亲验 `src/events.rs:55-57`:`emit` 对 event_type 做**落盘硬校验**——

```rust
if !PHASE1_EVENT_TYPES.contains(&record.event_type.as_str()) {
    anyhow::bail!("invalid or deferred event type: {}", record.event_type);
}
```

**这意味着 O2 的新类型(`audit.*`/`gate.*`/`budget.*`/`sandbox.*`/`judge.*`/`decision.*`)会被 emit 直接拒写**。「容忍未来类型」只在**消费侧**(读),生产侧是白名单 enforce。所以:

1. **第一步必须扩白名单**:把 `PHASE1_EVENT_TYPES` 改名 `KNOWN_EVENT_TYPES`(或加 PHASE2 组)并补全 O2 要发的新类型。**不接线先扩这个,否则新事件一个都发不出。**
2. **消费侧保持容错**:读 events 的 telemetry/logs 对未知 type 不炸(`future.event` 测试已覆盖,确认没回归)。生产侧白名单 enforce 是对的(防 typo),但要随 O2 同步加新类型。
3. 新类型在 `references/` 的 events schema 文档登记,文档与 `KNOWN_EVENT_TYPES` 同步(可加个 gate 断言两者一致,防漂移)。

---

## 完成判据

- 上述 6 子系统的关键失败/决策时刻在 `events.jsonl` 有对应事件(跑一个真实 run + 一次 audit dispatch,grep events.jsonl 看到 runner.*/audit.*/gate.* 等)。
- 未知事件类型消费不炸(回归测试)。
- raw 输出不泄漏进 events(测试:制造含 secret 的 job 输出,断言 events.jsonl 里被 redact/不含 raw)。
- `telemetry.json` 现在能多答几个问题(有了 runner/audit 事件):哪个 runner 失败率高 / audit 收敛几轮——验证 O3-1 的 telemetry 派生能用上这些新事件。
- `cargo fmt --check` / `clippy -D warnings` / `test --locked --all-targets` 全绿。
- 每个 O2-x 子批收口跑 `lto audit --auto-dispatch --discover-risks` 跨族异构审计,HIGH/CRITICAL 消解。
- `lto check --to closed --strict` PASS;closeout 记 O2 证据。

## 提醒
- **先定 scheduler run_id 架构决策再动手**——这是 O2 唯一需要想清楚的设计点,接错了要返工。
- 接线纯增量,不改已 emit 的命令层 lifecycle。
- commit 你写,release/tag 归 host。
- backlog.md 的 ⑨「Scheduler runner lifecycle events / O1-1 tracing」就是这件事,做完更新它状态(✅ 并注明 Rust 落地,别再留 Python 假阳性)。
