# Goal(增量): Rust 可观测层从零实现(日志 + 事件流 + 遥测)

> **致 codex**:这是对 `2026-06-16-goal-python-retirement-and-debt-cleanup.md` 的**增量追加**,
> 不是替换。同样用 LTO skill 自管、每 Phase 收口跑 `lto audit --auto-dispatch --discover-risks`
> 跨族异构审计、dogfooding 铁律(lto 调不通=lto bug 优先修)、维护者验收四标准全部适用。

---

## ⚠️ 先读:这条改变了主 goal 的 Phase 3 退役清单(撞车依赖)

host 拉子代理研究 + 亲验坐实一个**被 backlog 假阳性掩盖的失真**:

> `backlog.md` 标 `events.jsonl` / `telemetry.json`「✅ 已实现」——**但那是 Python 的**(`scripts/lto/events.py` / `telemetry.py`)。**Rust 重写时这一层根本没移植**。亲验:`grep events.jsonl|telemetry.json|safe_emit src/*.rs` = **0 命中**;Cargo.toml **无 tracing/log 依赖**;Rust 二进制跑完整 run(start→runner→closeout)产出**零事件流、零派生遥测**,运行时日志全是裸 `println!`。

**后果(和 source-note/eval-run 同一个坑,但更隐蔽)**:主 goal 的 Phase 3 要整套退役 Python,清单里就有 `events.py`/`telemetry.py`。**如果照原计划裸删,这个可观测能力会彻底消失**——因为 Rust 侧从来没接管过。这违反「port then delete」。

**因此对主 goal Phase 3 的硬性修正**:
- `scripts/lto/events.py` 和 `scripts/lto/telemetry.py` **不能跟着 Phase 3 裸删**。
- 要么:**本增量 goal 的 P0-2/P2-1 先在 Rust 实现 events/telemetry,再让 Phase 3 删 Python**(推荐,零功能损失);
- 要么:若 Phase 3 已先于本增量执行,必须把这两个文件**显式从退役清单移出、标 `removal-candidate(blocked-on: rust-observability)`**,留到 Rust 接管后再删。
- 不管哪条,`events.py`/`telemetry.py` 是 Rust 可观测层的**参考实现**(schema/8 事件类型/redact/体积闸门),删之前 Rust 必须镜像其行为契约。

---

## 现状(子代理研究 + host 亲验,带证据)

| 能力 | 协议定义 | Rust 现状 | Python 现状(退役中) |
|---|---|---|---|
| 运行时日志分级 | — | ❌ 裸 `println!`×144/`eprintln!`×8,无 tracing/RUST_LOG/-v | — |
| `events.jsonl` 事件流 | `delivery:111` append-only/redacted/容忍未来类型 | ❌ 零实现(`runner_events.rs` 是**读** runner NDJSON,不是**写**事件) | ✅ `events.py`(8 类型+文件锁+event_id 单调+10K/50K 体积闸门+safe_emit fail-safe) |
| `telemetry.json` 派生遥测 | `delivery:112` derived-only/不可成 command source | ❌ 无统一派生层(token/budget 各命令 inline 各算各) | ✅ `telemetry.py`(run/task metrics+redaction_summary,红线:不持久化 recommendations) |
| redact | — | 🟡 有(`llm_judge.rs:81`)但只服务 freeze_evidence,未覆盖 events/telemetry | events/telemetry 复用同一套正则(对称) |
| 体积闸门 | — | ❌ 无 | ✅ WARN 10K / HARD_STOP 50K |
| failure-query 命令 | 项目自带验收门(observability-module.md 三件套) | ❌ 无 `logs/fails/recent/stats` | — |
| live job log | — | ✅ `scheduler.rs:281`(唯一移植的) | — |

> 项目对自己每个模块的验收门(`plugins/dev-workflow/prompts/observability-module.md`)是可观测三件套:①结构化日志 schema ②doctor/healthcheck ③failure-query。**Rust core 自己只满足半条**(preflight 算半个 doctor)。吃自己狗粮就该达标。

---

## 贯穿全程的三条协议红线(原样守住)

a. **telemetry 不能成为 command source**(`telemetry.py:1-9` 红线)——纯派生、可重建、零 route/promote/recommendation 建议。测试钉死「telemetry.json 无 control_recommendations 字段」。
b. **events 必须 redacted + 容忍未来事件类型**(`delivery:111`)——落盘前脱敏;反序列化遇未知 type 不炸。
c. **落 Rust 不落 Python**(退役中);不弱化 `clippy -D warnings` / `unsafe_code = "forbid"`。

---

## Phase O1 — 可观测性基础设施(P0,地基,其余全依赖它)

| # | 项 | 落点 | 动协议? |
|---|---|---|---|
| O1-1 | 引入 `tracing` + `tracing-subscriber`,裸 `println!/eprintln!` 的**诊断输出**迁分级 span/event,加 `RUST_LOG` + `-v/--verbose`。**用户面正常输出(recap 表格等)保留 stdout**,只迁诊断/调试 | 新增 `src/observability.rs`;Cargo.toml 加依赖;`main.rs` init subscriber;命令层逐步替换 | 否(纯运行时日志,不碰 `.lto/`) |
| O1-2 | 把 redact 从 `llm_judge.rs:81` 提到共享模块(`redact_text`/`SECRET_RE`/`FULL_PATH_RE`),events/telemetry/judge 三方共用(保持 Python 对称性) | 移到 `src/redact.rs`,llm_judge 改 import | 否 |
| O1-3 | 实现 `events.jsonl` writer,**镜像 Python `events.py` 行为契约**:8 类型(`run.started/closed`、`phase.changed`、`task.created/status_changed`、`runner.started/finished`、`artifact.registered`)+ append-only + 文件锁原子写 + `event_id`=行数+1 单调 + `safe_emit` fail-safe(事件子系统坏不能拖垮宿主命令)+ 体积闸门(WARN 10K/HARD_STOP 50K)+ 落盘前用 O1-2 redact + `contains_raw_output=true` 拒写 | 新增 `src/events.rs` | **是**(surface 协议已定义,属「补实现」;需对齐 Python schema + fixture 证明旧 Python run 的 events.jsonl 仍可读) |

完成判据:`RUST_LOG=debug lto-rs ...` 能按级别过滤;跑一个 run 产出 `events.jsonl` 且 schema 与 Python 样例逐字段对齐;redact 测试(塞 `/Users/x/secret`+假 token → 落盘被脱敏);体积闸门测试;`cargo test/clippy/fmt` 全绿。

## Phase O2 — 事件覆盖面接线(P1,地基好后把哑的关键操作接 safe_emit)

靠 O1-3 的「容忍未来类型」吸收新子类型,不改协议。逐子系统接 `safe_emit`:
- **runner**(`scheduler.rs`):spawn / finished(rc+elapsed+timeout)/ 退出码分类 / 重试 / healthcheck 失败恢复。
- **audit/judge/decision**:审计员选择+同族过滤 / judge skip 原因 / 投票计票 / NeedsHuman escalate / findings 合并 / 收敛轮次。
- **gate/closeout/budget/worktree**:check pass/fail / ledger 拒绝 / 脏树 / budget 超限+warn / 沙箱 effect 拒绝 / `NEEDS_CONFIRM`。
- **phase 转移**:`util.rs:379 append_phase_transition` 现在只写 state,补一条 `phase.changed` 事件(让 phase 有时间线不只快照)。

完成判据:每个子系统关键失败/决策时刻在 events.jsonl 有对应事件;反序列化遇未知 type 不炸的测试;全绿。

## Phase O3 — 遥测派生 + failure-query(P2,有 events 才有数据)

| # | 项 | 落点 | 动协议? |
|---|---|---|---|
| O3-1 | `telemetry.json` build/save,镜像 Python:run metrics(wall/tasks_done/blocked/runner_calls/timeout_count)+ task metrics(retry/status_transition/evidence/touched_files)+ budget + redaction_summary + event_log。**守红线 a:无 recommendations** | 新增 `src/telemetry.rs`,从 state+events derive | **是**(补已定义 surface;测试钉死无 control_recommendations) |
| O3-2 | `logs`/`fails`/`recent` failure-query 命令:over events.jsonl 答「最近啥失败、哪步、何时、什么错」,补齐验收三件套第③条 | 新增 `commands/logs.rs` + `cli.rs` 注册 | 否(新 CLI 读 events) |
| O3-3 | `doctor` 命令(或扩 `preflight`):依赖在场+配置有效+state 可读,非零退出指明哪条 fail,补齐三件套第②条 | 扩 `ops.rs:191` 或新增 | 否 |
| O3-4(可选) | 跨 run 聚合(runner×model×status 有效性 + phase friction)喂 host brief,对等 Python `interventions.py aggregate_across_runs` | 新增跨 run scanner | 否(读多 run,不改单 run 协议) |

完成判据:`telemetry.json` 能答「这个 run 花多少 token / 哪个 runner 失败率高 / 哪个 phase 卡最久 / audit 收敛几轮」;`lto logs --fails` 能列最近失败;`lto doctor` 单命令体检;全绿。

---

## 执行顺序(与主 goal 协调)

```
主 goal: Phase 1+2 → Phase 3(Python 退役)
                         ↑
                  ⚠️ 删 events.py/telemetry.py 前,本增量 O1-3/O3-1 必须先 Rust 接管
                         │
本增量:  O1(基础设施) → O2(事件接线) → O3(遥测+query) ──┘ 与主 goal Phase 4 一并收口
```

- **O1 是地基,最先做**(tracing + events writer + 共享 redact)。
- **O1-3(events)和 O3-1(telemetry)是 Phase 3 删 Python 的前置**——这两个不先 Rust 化,`events.py`/`telemetry.py` 就不能删。把它们排在主 goal Phase 3 之前,或把那两个 Python 文件从 Phase 3 清单移出留到这里收。
- O2/O3 其余可在 Python 退役后做(那时只剩 Rust 一处实现,更干净)。
- 每个 Phase 收口照例 `lto audit --auto-dispatch --discover-risks` 跨族异构审计;dogfooding 铁律全程生效(实现 events 时若 `lto` 自己 emit 出错,那就是要修的 bug)。

## 给 codex 的提醒
- backlog 的「✅ 已实现」是 Python 假阳性,**别信 backlog 当 Rust 已有**——以 `grep src/*.rs` 实证为准。这本身是 Phase 4 技术债:修正 backlog.md 标注,区分 Python-done vs Rust-done。
- events/telemetry 的 Python 实现是**最好的 spec**——逐字段镜像它的 schema/类型/redact/闸门,别自己另设一套(否则旧 run 读不了)。
- 这是从零实现一整层,体量不小,用 LTO 分 Phase 推、每步 audit、commit 归你、release 归 host。
