# Spec: 跨机 dispatch（remote dispatch + receipt 回传）

> 状态：**§3 的全自动协议 NO-GO，暂缓实施**。异构审计（codex/pi 独立并行，2026-07-24）双双判定当前设计不能进入实施；host 采纳。
> **当前生效的是 §0 的人工 handoff 薄路径**；§3 全自动协议降级为"待证据支持的未来设计"，其 BLOCKER 清单见 §7。
> 来源：UnDercontrol（oatnil.com）"agents on every machine you own" 的 hypothesis，经 codex/pi 异构合议 + host 定案（run `20260724-ud-primitives`）。
> 定位：这是 **principle 4（每个 actuator 有界）** 的一次边界外扩，不是新增编排层。host 仍是 controller-in-chief。

---

## 0. 当前采纳路径：人工 handoff（零新协议）

审计结论一致——全自动协议引入两份状态副本、SSH 不确定提交、分布式幂等、远端权限与恢复运维，成本远超"800–1200 行"的直觉估计；而**跨机派工的真实频率尚无基线数据**（principle 9：外部观点是 hypothesis，需 eval 证据才能进 core）。

因此先用**现有命令**跑通闭环，不写新代码：

```bash
# 1. 本机：确认远端仓库状态（人工，一次性）
ssh axis 'cd /path/repo && git rev-parse HEAD && git status --short'

# 2. 本机：goal 传过去
scp goal-x.md axis:/path/repo/

# 3. 远端：人工起 lto 派工（远端自己有完整 run 或直接跑 agent CLI）
ssh axis 'cd /path/repo && lto dispatch-goal --runner pi --goal goal-x.md ...'

# 4. 远端跑完，把 reply 取回本机
scp axis:/path/repo/reply-pi.md .

# 5. 本机：登记进本 run 的证据链（已有命令，无需 receipt 协议）
lto collect-agent-run --task-id <T> --runner pi --reply reply-pi.md --status ok
```

**收集什么证据再决定要不要自动化**：跨机派工频率、每次人工耗时、失败类型、证据丢失率。跑满 5–10 次真实派工后复盘；只有当"人工收集是主要瓶颈"被数据支持时，才重启 §3 的协议设计。

**唯一允许的增量**（可选，若人工步骤 1 反复出错才做）：一个**只读** `lto remote-preflight --host <alias> --cwd <path>`，输出远端 HEAD/dirty 与 content-addressed goal bundle 指令。不做 receipt、不做轮询、不做远端状态镜像、不做 push-back。

---

## 1. 为什么做

用户环境是多机（M4 本机 / axis / pve，SSH 别名已配）。当前 `dispatch-goal` 只能在本机 tmux 起窗口——想让 axis 上的 agent 干活，只能人工 SSH 过去手起，run-state 与证据完全脱离本机 `.lto/`。目标是让远端执行也纳入同一套 run/证据/完成信号闭环。

**非目标**（写死，防漂移）：
- 不做集群调度/负载均衡/自动选机；目标机由人显式指定。
- 不做代码同步（rsync/git push）。远端仓库状态由操作者负责，lto 只做**检查与拒绝**，不做修复。
- 不引入常驻 daemon / relay 服务（CLAUDE.md 实施姿态第 4 条：不加 UI/server/global daemon 除非显式批准）。

---

## 2. 现状事实（2026-07-24 核对，实施前需复核）

| 事实 | 位置 |
|---|---|
| `--target` 字符串直传 tmux `-t`，无任何 host 概念 | `src/dispatch_goal.rs` `validate_dispatch_target`；`src/tmux_runner.rs` `prepare_target` |
| tmux 调用是本机 `Command::new(tmux_bin)` | `src/tmux_runner.rs` |
| 完成信号靠 agent 在**执行机**跑 `lto agent-turn-completed`，写**该机** `.lto/<run>/events.jsonl` | `src/dispatch_goal.rs` 完成协议注入；`src/agent_turn.rs` |
| 完成协议命令里内联 `--repo <本机绝对路径>` | `materialize_goal_with_completion_protocol` |
| `events::emit` 有 `KNOWN_EVENT_TYPES` 白名单、本机单调 `event_id` 计数器、文件锁、ingress redaction | `src/events.rs:16,74` |
| 窗口清理直接调本机 tmux | `src/agent_turn.rs` |
| dispatch 窗口持久化在 `state.dispatch_windows`（window_id/target/runner/tmux_bin/status/…） | `src/state.rs:338` |

**结论**：远端 receipt **不能**直接写本机 events.jsonl（会绕过 emit 的白名单/计数器/redaction）。必须由本机进程读取 receipt → 校验 → 调 `events::emit` 导入。

---

## 3. 全自动协议设计（**NO-GO / 暂缓**，保留供未来重启）

> 以下 §3–§6 是首版设计，异构审计判定不可实施。保留原文以便未来带着 §7 的 BLOCKER 清单重做。**读到这里请先读 §7，不要照本实施。**

### 3.1 目标寻址：显式字段，不用 `ssh:` URI

**采纳 codex 方案，否决 `--target ssh:<host>`。** 理由：`--target` 现语义是 tmux target，塞进 host 会让一个字段承载两种寻址空间，解析歧义且污染既有校验路径。

新增互不重叠的字段：

```
--remote-host <ssh-alias>     # 必填，触发 remote 模式；只接受 ssh_config 别名或 user@host
--remote-cwd  <abs-path>      # 必填，远端仓库绝对路径；不接受相对路径与 ~ 展开
--remote-lto  <path>          # 可选，远端 lto 可执行路径，默认 "lto"
```

remote 模式下 `--target`/`--tmux-session`/`--new-window` 语义**不变**，但作用于**远端 tmux**（原样透传给远端 lto）。本机不再起窗口。

### 3.2 执行流程（最薄可行）

```
1. preflight    ssh <host> 检查：lto 可执行、--remote-cwd 存在且是 git repo、tmux 可用
                → 输出远端 HEAD/dirty 状态给人看
2. gate         首次使用该 (host, cwd) 组合 → 要求 --confirm-remote（人工闸，见 3.5）
                远端 dirty 或 HEAD 与本机不符 → 默认拒绝，需 --allow-remote-drift
3. ship goal    scp goal 文件到 <remote-cwd>/.lto/<run-id>/goal-<dispatch-id>.md
                本机记录 goal 的 sha256
4. dispatch     ssh <host> "cd <cwd> && <remote-lto> dispatch-goal --runner … --goal … \
                  --run-id <run-id> --remote-receipt <receipt-path>"
                → 远端起 tmux 窗口，本机 ssh 立即返回（非阻塞）
5. persist      本机 state.dispatch_windows 追加一条带 remote 字段的记录（见 3.4）
6. collect      本机 `lto dispatch-collect --run-id <id>`（或 dispatch-and-wait 内循环）
                ssh 拉 receipt → 校验 → events::emit 导入 agent.dispatch.completed
```

### 3.3 完成信号：远端原子 receipt + 本机拉取导入

**采纳 codex 主线，否决"反向 SSH 优先"。** 理由：家庭网络 NAT 下 axis→M4 的回连路由/密钥/权限均未证实；pi 也承认这是其方案的最大失败模式。反向 SSH 作为**可选加速通道**（`--remote-push-back <local-ssh-alias>`）保留，但**不是**正确性依赖——即使它成功，本机仍以 receipt 为准做幂等导入。

**Receipt 格式**（远端写，路径 `<remote-cwd>/.lto/<run-id>/receipts/<dispatch-id>.json`）：

```json
{
  "schema_version": 1,
  "dispatch_id": "d-20260724-abcd1234",
  "run_id": "20260724-ud-primitives",
  "runner": "pi",
  "goal_sha256": "…",
  "rc": 0,
  "source": "goal-self-report",
  "started_at": "…",
  "finished_at": "…",
  "summary": "≤200 chars, 远端已做 redaction"
}
```

**原子写**：远端先写 `<id>.json.tmp` 再 `mv`（同目录 rename 原子）。本机读到半截文件是明确禁止的失败模式。

**导入规则**：
- 幂等键 = `dispatch_id`。已导入过的 receipt 再次拉取 → no-op，不重复 emit。
- 校验：`run_id` 匹配、`goal_sha256` 与本机记录一致（防止远端跑的是另一份 goal）、schema_version 已知。任一不符 → 拒绝导入，记 warning，保留 receipt 供人工查。
- 导入成功后由**本机** `events::emit("agent.dispatch.completed")`，走既有白名单/计数器/redaction。远端文本进 `summary`/`fields` 前必须过本机 `redact` 一遍（不信任远端已 redact）。

### 3.4 状态模型扩展

`state.rs:338 DispatchWindowState` 增补可选字段（`#[serde(default, skip_serializing_if)]`，**保持既有 state.json 向后兼容**——principle 「保留 backwards compat」）：

```rust
pub remote_host: Option<String>,
pub remote_cwd: Option<String>,
pub dispatch_id: Option<String>,
pub goal_sha256: Option<String>,
pub receipt_path: Option<String>,     // 远端路径
pub receipt_imported_at: Option<String>,
```

远端窗口的 `status` 增加取值 `unknown_remote`（见 3.6）。**本机 tmux 清理逻辑必须跳过 remote 记录**（`agent_turn.rs` 现在无条件调本机 tmux——这是实施时最容易踩的 bug）。

### 3.5 人工闸（不可自动化，写死）

1. 首次使用某 `(remote_host, remote_cwd)` 组合：必须 `--confirm-remote`；确认后写入 `.lto/remote-trust.json`（本机，记录 host/cwd/首次确认时间）。
2. 远端 worktree dirty，或远端 HEAD ≠ 本机 HEAD：默认**拒绝派工**，需显式 `--allow-remote-drift`。
3. 远端 sandbox/权限提升（等价于 codex `--sandbox workspace-write`）：需显式 flag，不继承本机默认。
4. 超时后的 retry / kill / 窗口清理：**永不自动**，只打印远端可执行的命令供人决定。

### 3.6 失败语义（枚举，禁止"猜"）

| 场景 | 状态 | 行为 |
|---|---|---|
| SSH 不可达 / 网络断 | `unknown_remote` | **不是 failed**。禁止自动重派（远端可能仍在跑，重派 = 重复执行）。打印恢复命令。 |
| receipt 缺失（远端仍在跑或挂起） | `unknown_remote` | 保留远端窗口，等人 inspect。collect 可重试。 |
| receipt 存在但 sha256/run_id 不符 | `rejected` | 拒绝导入，保留证据，人工介入。 |
| receipt rc≠0 | `failed` | 导入失败完成事件，**不清理**远端窗口。 |
| 本机重启 | — | 从 `state.dispatch_windows` 恢复，collect 是无状态命令，不依赖常驻进程。 |
| 远端机器彻底挂 | `unknown_remote` | 明确人工场景；文档写清恢复步骤。 |

---

## 4. 实施切分（后续 goal，文件尽量不相交）

| Goal | 范围 | 落点 |
|---|---|---|
| **B-P1** receipt 协议 + 导入内核 | receipt schema、原子写/读、幂等导入、校验 | 新建 `src/remote/receipt.rs`；`src/events.rs`（仅新增导入辅助，不改 emit 语义） |
| **B-P2** remote dispatch 执行面 | preflight/gate/scp/ssh 调用、`--remote-*` flags、state 扩展 | 新建 `src/remote/dispatch.rs`；改 `src/dispatch_goal.rs`、`src/state.rs`、`src/cli.rs` |
| **B-P3** collect 命令 + 清理隔离 | `lto dispatch-collect`、dispatch-and-wait 的 remote 分支、`agent_turn.rs` 跳过 remote 窗口 | 改 `src/agent_turn.rs`、`src/cli.rs`、`COMMANDS.md` |

**B-P1 必须先行**：它是纯数据+纯函数，可完全单测，且冻结了协议。B-P2/B-P3 有 `cli.rs` 共享热点，**串行执行**，不并行。

## 5. 测试策略

- B-P1 全部可单测（临时目录 + 构造 receipt，含半截文件、篡改 sha、重复导入）。
- B-P2/B-P3 的 SSH 用**注入式命令工厂**（复用 `src/process.rs` 的 Command factory 思路），单测里替换成假的 ssh/scp，断言 argv 而非真连网。
- 真机验证是**人工验收步骤**，不进 CI：M4 → axis 跑一次完整派工+collect，记录进 run 证据。

## 6. 已识别风险（合议时提出，实施时必须复查）

1. **"最薄"并不薄**：pi 估计 800–1200 行 + 多个新命令；这是本 spec 把它切成三份并要求 B-P1 先冻结协议的原因。
2. **receipt 轮询不是 push**：没有 collect 进程时完成事件不会自动到达本机。可接受（host 本来就要主动 collect），但 `dispatch-and-wait` 的 remote 分支需要明确轮询间隔与上限，并在文档里说清"不 collect 就不会有事件"。
3. **双 SSH 通道诱惑**：反向 push-back 一旦被当成主路径，就会出现两条正确性来源。spec 写死：**receipt 是唯一事实来源**。
4. **redaction 边界**：远端文本一律视为不可信输入，导入前过本机 redact。
5. **goal 里的绝对路径**：现完成协议内联本机 `--repo` 路径；remote 模式必须换成远端路径，且不得把本机私有路径泄露到远端文件（privacy self-check 覆盖）。

---

## 7. 审计裁定与 BLOCKER 清单（2026-07-24，codex + pi 独立并行审计）

两家独立得出同一判断：**当前设计不能进入实施**。以下 BLOCKER 均已由审计方给出源码证据，host 已逐条复核属实。

### 7.1 致命事实错误（首版 spec 写错的）

| # | 事实 | 证据 |
|---|---|---|
| F1 | 远端 `dispatch-goal --run-id` 会走 `load_run`，而它要求远端已存在 `.lto/<run>/state.json`。§3.2 只 scp goal 就调用它——**流程根本跑不起来** | `src/commands/util.rs:126-131`（无 state.json 直接 bail）；`src/dispatch_goal.rs:76` |
| F2 | §5 声称可复用 `src/process.rs` 的"注入式命令工厂"——**该设施不存在**，process.rs 只有 `shell_command` 与具体 git 辅助。测试策略据此失效 | `src/process.rs:16` |
| F3 | `dispatch-and-wait` 只按 event_type 等待，无法按 dispatch_id 关联，**可能消费另一次派工的完成事件** | `src/cli.rs:1450` |
| F4 | 超时路径无条件把"最新 active 窗口"标 retained，未绑定本次派工 | `src/dispatch_goal.rs:326` |

### 7.2 BLOCKER（重启设计时必须先解决）

1. **write-ahead intent**：必须先持久化 `prepared` 意图再执行 scp/ssh。首版顺序（远端启动后才记本机）会在两步间崩溃时留下无法关联的远端任务。
2. **`dispatch_id` 冻结**：首版只给了示例值没给算法。需规定字符集、≥128-bit 唯一性、生成方、作用域、重试复用规则与 `supersedes` 语义。
3. **events lock 内按 dispatch_id 幂等**：`events::emit` 只串行化 `event_id`，`events::read` 也只按 `event_id` 去重（`src/events.rs:74,153`）。并发/重试 collect 会产生重复 `agent.dispatch.completed`。幂等必须在 events lock 内完成，state 只存可重建的 `receipt_event_id/imported_at`。
4. **远端 bootstrap 协议**：需要一个不依赖远端完整 state.json 的 worker/accept-once 路径（见 F1）。
5. **关联等待与非终态 gate**：补 `dispatch-inspect` / `resolve --outcome` / `retry --supersedes`；`check`/`closeout` 目前**完全不消费 `dispatch_windows`**，会在远端状态未知时关闭 run（比"永远悬着"更糟）。
6. **actuator 有界（principle 4 明确不合规）**：远端执行缺硬 timeout、max concurrency、permission snapshot、预算上限；`ssh cat` 无超时；shell 参数需安全构造。
7. **隐私（principle 11 部分不合规）**：整份 goal 传到远端可能携带本机私有路径/秘密，首次 host/cwd trust **不是逐 payload 的披露确认**；trust 应绑定 SSH host fingerprint 而非 alias；receipt 需 `KNOWN_RECEIPT_KEYS` 白名单防远端塞不可信字段绕过 redact。
8. **崩溃持久性**：rename 原子 ≠ 崩溃持久（缺 fsync file + fsync dir）；固定 `.tmp` 名允许同 ID 两写者互相覆盖；需唯一临时名 + write-once + "同内容 no-op / 不同内容拒绝"。跨文件系统（NFS/bind-mount）时 `mv` 退化为 cp+rm，preflight 需检查同 device。

### 7.3 已采纳的其他修正

- **删除 §3.3 的可选 push-back 通道**：无核心价值，只扩大协议与安全面（两家均建议）。
- **状态枚举需扩充**：至少补 `prepared` / `submitted` / `preflight_rejected` / `ship_failed` / `receipt_invalid` / `corrupt` / `import_pending` / `collect_timeout` / `abandoned` / `superseded`；`DispatchWindowState.status` 现为无约束 `String`。
- **切分作废**：首版 B-P1「纯数据+纯函数可完全单测」判断错误——真正的幂等要动 events lock 内逻辑与 state reconciliation。重启时按 P0 状态机与协议 fixture → P1 远端 worker/accept-once → P2 host transport + write-ahead → P3 幂等 collect/inspect/resolve/closeout gate。
