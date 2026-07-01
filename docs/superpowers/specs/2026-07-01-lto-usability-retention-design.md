# LTO 可用性/运维加固设计（日志 retention + 命令 UX）

> 状态：设计已对齐，待实现。2026-07-01。
> 范围：一份 spec 统一解决两块系统性欠债——A=日志 retention（.lto 已 3.1G/44 run 零清理），B=命令 UX（agent 调错/不愿调，子代理实证 15 痛点）。
> 决策已定：八项全做（A1-A3 + B1-B5）。

## 为什么做（第一性）

两个系统性缺陷，都不是单点 bug：

1. **日志零 retention**：`.lto/<run-id>/` 从设计起就没有任何清理机制（grep 全 repo 无 prune/gc/retention），已积累 3.1G / 44 run，只会无限增长。events.jsonl（最大 156K）+ live 日志 + audit 产物是主体。
2. **命令 UX 让 agent 调错/弃用**：23 个子命令，枚举值只在报错时暴露、默认值硬编码在 Rust 不进 --help、无 examples、多步工作流只在 SKILL.md 散文里。子代理实证 15 条痛点，Tier1 全集中在 `runner` 命令。

业界参照（hs 查证）：git gc（--auto 阈值提示 + 手动 prune）、docker builder prune（一次性 + keep-storage 上限）、git-lfs prune（age + keep-recent）、GitLab retention ADR。共识：**手动 prune 命令 + keep-N/age-TTL/size-cap 策略 + 永不删 active + dry-run 确认**。

## 红线（LTO CLAUDE.md 原则）

- **不静默删数据**（原则 1）：prune 是 host 显式调用 + dry-run 默认；自动化只到"提醒"为止，绝不自动删 run。
- **永不动 active/未完成 run**：只清 `phase=closed`。
- **保留轻量历史索引**：即使 prune，也留 state.json + run-state.md，让"发生过什么"永远可追溯。

---

## A. 日志 retention

### A1. `lto prune` 命令【核心】

新增 `Commands::Prune`（cli.rs 命令注册区）+ `src/commands/prune.rs`。

**删除条件**（三条同时满足才够格，默认保守）：
- `phase == "closed"`（读 state.json；active/未完成永不动）
- `age > 30d`（run 的 started_at 或 closed_at，`--older-than <days>` 覆盖，默认 30）
- 只删**大件**：`events.jsonl`、`live/`、`audit/`、`dispatch/` 产物；**保留** state.json、run-state.md、artifacts.json（轻量历史索引）

**flag**：
- `--dry-run`（**默认 true**）：只列出每个够格 run 要删的文件 + 各自大小 + 合计可回收空间，不真删
- `--yes`：真删（关闭 dry-run）
- `--older-than <days>`：覆盖 30d 默认
- `--keep-last <N>`：叠加保护——即使够格，最近 N 个 closed run 也不动（默认 0=不额外保）
- `--run-id <id>`：只 prune 指定 run（跳过条件，但仍不删非大件、仍拒 active）

**输出**：dry-run 列 `RUN_ID | closed | age | 可删大件 | 回收MB`，末尾合计。真删后打印实际回收。

**prune 后标记**：被 prune 的 run 在 run-state.md 追加一行 `> [pruned <date>] 大件已清理，保留状态索引`，让后来者知道日志被清过（不是数据丢失）。

**完成判据**：`cargo test prune::` 覆盖 ①closed+老+大件→删 ②active→拒 ③新 run(<30d)→跳过 ④dry-run 不真删 ⑤--keep-last 保护 ⑥保留 state.json/run-state.md ⑦回收空间统计正确。

### A2. closeout/preflight 超阈值提醒

closeout（commands/closeout.rs）和 preflight（commands/ops.rs cmd_preflight）末尾，计算 `.lto` 总大小 + closed run 数，若超阈值打印**提醒行**（不删）：
```
NOTE: .lto is 3.1 GB across 30 closed run(s). Consider `lto prune --dry-run` to reclaim space.
```
- 阈值写死合理默认：`>1GB` 或 `>30 closed run`（任一触发）。对齐 git gc --auto 提示模式。
- best-effort：算大小失败静默跳过，绝不阻塞 closeout/preflight。
- **完成判据**：超阈值出提醒、未超不出、算失败不崩。

### A3. `lto runs` 加体积 + 状态列

`lto runs`（commands 里 runs 列表）加两列：每个 run 的**磁盘占用**（du 该 run 目录）+ **phase 状态**（active/closed）。让 host prune 前看得见哪些占地方。
- best-effort du，失败显示 `?`。
- **完成判据**：列表含 size + phase 列；大小合理；不崩。

---

## B. 命令 UX

### B1. 枚举值 + 默认值全暴露进 --help【核心】

所有枚举 flag 在 `--help` 列出合法值（clap `PossibleValuesParser` 会自动在 help 显示 `[possible values: ...]`），所有默认值用 `default_value` + help 文本显示，校验失败的错误信息由 clap 自动提示合法值。逐个修（cli.rs）：

| flag | 现状 | 改法 |
|---|---|---|
| `runner --kind` (:340) | 自由字符串无校验 | 加 `PossibleValuesParser`（列合法 kind）或至少 help 列出 + `ensure_valid_kind` 前移到 parse |
| `runner --runner` (:359) | 默认 codex 不显示 | help 显示默认 + 列 KNOWN_RUNNERS(agent_job.rs:9) |
| `runner --status-on-fail` (:352) | 默认 blocked 隐藏（最坑：agent 以为 failed 去重跑） | help 显示默认 + 说明语义 |
| `collect-agent-run --status` (:262) | PossibleValuesParser 但 help 不显示值 | 确认 help 显示 possible values（可能是 help 模板问题） |
| `task update --status` (:315) | 无 value_parser | 加 PossibleValuesParser + 列值 |
| `runner --tmux-mode` (:371) | 枚举无语义说明 | help 补 signal/sentinel/fire 各自何时用 |

- **纯 help/校验层改动，不动执行逻辑，最低风险。**
- **完成判据**：每个 flag `--help` 显示合法值/默认；`runner --kind badvalue` 报错含合法值列表；现有测试不回归。

### B2. runner 三模式 help 说清

`runner` 有 `--command`/`--prompt`/`--job-file` 三互斥模式，`--command` 隐式必填 `--task-id`（现在只在 ops.rs:1918 报错时才知道）。在 runner 子命令的 `long_about`/help 里讲清三模式 + 各自必填 flag。
- 纯 help 文字，零逻辑风险。
- **完成判据**：`runner --help` 明列三模式；`--command` 段注明需 `--task-id`。

### B3. 每个复杂命令加 examples

dispatch-goal/runner/audit/judge/collect-agent-run 的子命令用 clap `after_help` 加真实可照抄的 example + "See also" 交叉引用。
- 例：dispatch-goal after_help 给 `lto dispatch-goal --runner codex --goal goal.md`（无参默认当前 tmux 会话）+ "配 `lto events --wait` 收完成"。
- **完成判据**：上述命令 --help 末尾有 example 段；example 命令真实可跑（自测一遍）。

### B4. `lto dispatch-and-wait` 组合命令

新增 `Commands::DispatchAndWait`——把"派工 → `events --wait` 等 agent.turn.completed → 打印结果摘要"封成一步。内部复用 dispatch_goal + events::wait_for，不重造。
- flag 继承 dispatch-goal（--runner/--goal/--run-id/tmux 相关）+ `--timeout`（等待上限，传给 wait_for）。
- 语义：派工 → 阻塞等完成事件（v0.8.0 的机制级 hook 会触发）→ 返回时打印 completion 事件摘要 + 提示"用 collect-agent-run 登记产物"。
- **不吞掉** dispatch-goal / events --wait 单独命令（它们仍在，组合命令是便利层）。
- **完成判据**：`dispatch-and-wait` 派 trivial goal 能一步走到完成返回；超时优雅报告；单元测试覆盖 plan 组装（真机派工验证同 v0.8.0 手法）。

### B5. 验证一致性（dispatch-goal 严 vs runner 松）

dispatch-goal 的 `--runner`(:397) 用 value_parser 严格校验 `[codex,pi,agy]`，runner 的 `--runner`(:359) 无校验默认 codex。统一——runner 的 --runner 也加相同 value_parser（或共享 KNOWN_RUNNERS 校验）。
- 搭 B1 一起做顺手（都是 cli.rs flag 层）。
- **完成判据**：runner/dispatch-goal 的 --runner 校验行为一致；非法 runner 都报同样风格错误 + 合法值。

---

## 执行顺序（每组独立可收口）

```
A1(prune 命令) → A2(阈值提醒) → A3(runs 体积列)        # 日志组
B1+B5(flag 层一起) → B2(runner help) → B3(examples) → B4(组合命令)  # UX 组，B1/B5 同层先做
```

B1/B5 同属 cli.rs flag 层，一起改。B4 改动最大放最后。两组可并行（文件不太重叠：A 主要新增 prune.rs + 碰 closeout/ops/runs；B 主要碰 cli.rs + 各命令 help）。

## 每 Phase 收口

`cargo fmt --all --check` + `cargo check` + `cargo clippy -- -D warnings` + `cargo test --locked --all-targets` + `python3 scripts/check_docs_consistency.py`。A 组碰 delegation/artifacts 要跑 `scripts/privacy_self_check.sh`。文档同步：SKILL.md（agent 真读）+ COMMANDS.md（prune/dispatch-and-wait 新命令、flag 枚举值）。

## 复用什么（勿重写）

- `events::wait_for`（events.rs:167）——B4 组合命令复用，不重造等待。
- `dispatch_goal` 的 runner_plan + run_dispatch——B4 复用派工。
- state.rs load_state（判 phase=closed/age）、KNOWN_RUNNERS（agent_job.rs:9）——A1/B1/B5 复用。
- clap PossibleValuesParser（collect-agent-run 已用）——B1 照搬到其他枚举 flag。

## 红线复述（必须）

- prune 默认 dry-run；只清 closed；永不删 active；保留 state.json/run-state.md。
- 阈值提醒只提醒不删。
- 组合命令不吞单命令。
- 文档同步 SKILL.md（不只 COMMANDS.md——agent 真读 SKILL）。
