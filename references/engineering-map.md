# LTO 工程化地图：循环 → primitive → 判断边界

> 用途：固定 LTO 的长任务循环每一步「能不能用 harness 兜底」。能脚本化的指向 primitive，不能的指向 host agent / human gate。
> 这是 SKILL.md 调度逻辑的依据表——宿主推进任务时按这张表决定「这一步该跑哪个 primitive / 该自己判断什么」。

## 一、完整循环（LTO 帮用户管理的长任务）

```
用户想法
  → [P1 intake]  该不该现在做？
  → [P2 spec]    写方案 + 标待审点
  → [P3 audit]   多 AI 异构审 → 逐轮收敛 → 修到零大问题
  → ⏸ 闸：可以写代码了吗？（人拍板）
  → [P4 implementation]  写代码 + runner 执行 + judge 审查
       └─ Micro Loop: runner → 测→ 修→ 复测 → evidence 记录
       └─ Meso Loop: judge phase → pass → 下一 phase
  → [P5 deploy]  pre-deploy hook → schema→试运行→只读→正式→真实测→清理
  → [P6 observe/closeout]  落盘决策 + pre-closeout hook + handoff
  → 下一个想法（回 P1）
  → 跨 session 恢复: resume → 上下文胶囊 → 暖启动
```

跨阶段贯穿三条刹车：(1) 喊不出缺什么→停 (2) 数据不达标→停 (3) 人说了算。
跨阶段 hook：pre-commit（commit 前）/ pre-deploy（部署前）/ pre-closeout（归档前）。

## 二、逐步拆解：输入 / 输出 / 能否脚本化

| 步 | 阶段 | 输入 | 输出 | 脚本化？ | 落点 |
|---|---|---|---|---|---|
| S0 | 开局 | `--goal` `--host` `--profile` | `.lto/<run-id>/` state.json + run-state.md + `current` | ✅ 全 | `lto_run.py start` |
| S1 | intake | 用户想法 / 「以后可能要 X」 | 该做 / 砍到最小版 | ❌ 判断 | SKILL §三原则·刹车1 |
| S2 | spec | 需求 | 方案 md + 待审点清单 | ❌ 创作 | SKILL §六阶段 P2；`references/decision-logging.md` |
| S3a | audit·派工 | 方案 md + auditors | 各 runner 的回复 md | ✅ 全 | `agent-delegate` runners |
| S3b | audit·健康 | runner 名单 | 每 runner exit/elapsed/bytes/verdict | ✅ 全 | `lto_run.py preflight` + `agent-delegate/.../healthcheck.sh` |
| S3c | audit·记账 | 每轮 blocker 计数 | `audit-ledger.md` 表格 | 🟡 半 | `templates/audit-ledger.md`（人填表） |
| S3d | audit·收敛判定 | `audit-ledger.md` | CONVERGED / CONVERGING / REBOUND / STALLED + rc | ✅ **新脚本** | `scripts/audit_ledger_check.py` |
| S3e | audit·逐条核验 | 每条 blocker claim | 采纳(怎么修)/否决(证伪依据) | ❌ 判断 | `references/audit-convergence.md` §二 |
| G1 | 写码闸 | 收敛状态 + entry evidence | 证据报告 + 人「可以写代码了」| 🟡 证据脚本化 / 人拍板 | `lto_run.py check --to implementation` + SKILL §刹车3 |
| S4 | implementation | 方案 | 代码 + 代码审计 | ❌ 创作（审同 S3） | SKILL P4 |
| S5 | deploy | 改动 | schema→试运行→只读→正式→真实测 | 🟡 半 | `references/deploy-sequencing.md` |
| S6 | observe | 真实用户流程 | 「新功能真通电」证据 | ❌ 判断 | SKILL §常见错觉「服务没挂≠上线成功」 |
| S7 | 落盘 | 本轮决策/天花板/反例 | memory-flow 条目 / `docs/decisions/` ADR + manifest | 🟡 半 | `write_decision.py` + `references/decision-logging.md` |
| S8 | 收尾 | summary + next_action | `current_phase=closed` + `handoff.md` | ✅ 全 | `lto_run.py closeout` |
| S9 | 恢复 | compact 后断点 | 上下文胶囊 + 真实进度（git+state.json+ledger 三层证据） | ✅ 校验 / ❌ 续推判断 | `lto_run.py resume` + `lto_run.py check` + `references/long-loop-state.md` |

图例：✅ 已/可脚本化　🟡 半脚本化（脚本兜一部分，剩下靠人）　❌ 本质靠人判断/创作

## 三、脚本职责清单（固定调用顺序）

| 脚本 | 职责 | 输入 | 输出 | 运行 |
|---|---|---|---|---|
| `lto_run.py start` | 建状态文件（state.json + run-state.md；audit/deploy 加 audit-ledger；deploy 再落 preflight 快照） | `--goal --host --profile{minimal\|audit\|deploy}` [--with-audit] | `.lto/<run-id>/` | `python3 scripts/lto_run.py start --goal X` |
| `lto_run.py resume` | 跨 session 断点恢复 | `[--run-id]` | 上下文胶囊 + state.json 更新 | `python3 scripts/lto_run.py resume` |
| `lto_run.py check` | 校验状态完整+git 锚定+收敛趋势；可附 phase-entry 证据报告 | `[--run-id] [--strict] [--to implementation\|closed] [--json]` | WARN/ERROR + `OK <dir>`；phase evidence；rc 0/1 | `python3 scripts/lto_run.py check --to implementation --strict` |
| `write_decision.py` | 生成 ADR 决策记录，更新 state.user_decisions，并登记 `decision_record` artifact | `--repo --run-id --title --context --decision --consequences [--slug]` | `docs/decisions/YYYY-MM-DD-<slug>.md` + manifest entry | `python3 scripts/write_decision.py --run-id <id> --title "..." ...` |
| `lto_run.py preflight` | 即时探活 stdout | `[--record]` | 环境健康报告 | `python3 scripts/lto_run.py preflight` |
| `lto_run.py task-add` | 给当前 run 加一个 task（runner/next/audit 的操作对象） | `--task-id --title [--phase] [--command]` | state.json tasks 追加 + commands_run 记录 | `python3 scripts/lto_run.py task-add --task-id T1 --title "..."` |
| `lto_run.py runner` | 单 task 执行+证据记录 | `--task-id --kind --command [--cwd] [--timeout]` | evidence + state.json 更新 | `python3 scripts/lto_run.py runner --task-id T1 --kind test --command "..."` |
| `lto_run.py judge` | 只读审查+YAML verdict | `[--phase] [--task-id] [--rerun-tests]` | `.lto/<id>/judge/*.yaml` | `python3 scripts/lto_run.py judge --phase implementation` |
| `lto_run.py hook` | 外部边界闸门 | `pre-commit\|pre-deploy\|pre-closeout [--force --reason]` | rc 0/1 | `python3 scripts/lto_run.py hook pre-commit` |
| `lto_run.py closeout` | 标 closed + 出 handoff，默认写 CHANGELOG；`--no-changelog` 用于已提交后的行政收尾 | `--summary [--next-action] [--force] [--no-changelog]` | `handoff.md`；rc 0/非0 | `python3 scripts/lto_run.py closeout --summary "…"` |
| `audit_ledger_check.py` | 判 blocker 单调收敛 | `<ledger.md>` 或 `--run-id` `[--strict]` | verdict + rc 0/1/2 | `python3 scripts/audit_ledger_check.py .lto/<id>/audit-ledger.md` |
| `lto_run.py self-test` | 离线自检 start→resume→check→closeout→hook | — | `SELFTEST OK`；rc 0/1 | `python3 scripts/lto_run.py self-test` |
| `audit_ledger_check.py self-test` | 离线自检四档收敛 | — | `LEDGERCHECK SELFTEST OK`；rc 0/1 | `python3 scripts/audit_ledger_check.py self-test` |
| `lto_run.py parallel` | 并发批量跑多 task 的 shell 校验命令 | `--phase\|--task-ids [--concurrency] [--command]` | evidence + state.json | `python3 scripts/lto_run.py parallel --phase impl --concurrency 4` |
| `lto_run.py pipeline` | 每 task 串行过多 stage（item 并发） | `--stages "..." [--phase] [--concurrency]` | evidence + state.json | `python3 scripts/lto_run.py pipeline --stages "lint {task_id}" "test {task_id}"` |
| `lto_run.py audit` | 对抗审计编排+收口判收敛 | `[--auto-dispatch\|--discover-risks\|--collect <dir>]` | 审计简报 + audit-ledger.md | `python3 scripts/lto_run.py audit --auto-dispatch` |
| `lto_run.py next` | 事实简报器（零 LLM，不接管路径选择） | `[--exec] [--json]` | 决策简报 / argv 命令 | `python3 scripts/lto_run.py next` |
| `lto_run.py autopilot` | 受约束推进 harness | `--supervised [--auto-exec] [--decide [--decide-kind] [--decide-budget]]`；`--autonomous` 未实现 | 决策简报 / 沙箱执行 + evidence / 三方收敛 brief | `python3 scripts/lto_run.py autopilot --supervised --decide` |
| `lto_run.py recap` | 面向人类的回顾视图 | `[--run-id]` | 人话回顾（六问） | `python3 scripts/lto_run.py recap` |
| `smoke_test.py` | skill 自检（结构+脚本+模板+eval+ref） | — | `SMOKE OK`；rc 0/1 | `python3 scripts/smoke_test.py` |
| `scripts/install.sh` | 安装 skill 软链，并生成/检查全局 `lto` wrapper | `[--check] [target]` + `LTO_BIN_DIR` | skill links + sentinel-managed wrapper；冲突 rc 2 | `bash scripts/install.sh --check` |

**harness primitive 底层模块**（不直接走 CLI，是 host agent 可组合的能力）：
`agent_job.py`（AgentJob/AgentResult 数据合同）/ `scheduler.py`（并发+退出码三元判定+退避+healthcheck）/ `agent_exec.py`（spawn 原语）/ `worktree_exec.py`（autopilot 沙箱，17 攻击向量拦截）/ `progress.py`（推进检测+stall 闸门，防伪推进博弈）/ `decision.py`+`decision_brief.py`（双轨收敛引擎：direction 投票 / review union 合并，被 `autopilot --decide` 调用 spawn 三方）。

## 四、为什么这些步骤不脚本化（边界声明）

- **S1/G1 刹车**：「现在真需要 X 吗」「可以写代码了吗」是价值判断，没有确定性输入→输出函数。G1 的证据采集可脚本化，但最终批准仍由人拍板；脚本不能输出“ready/approved”。
- **S3e 逐条核验**：blocker 是真是假要亲核源码:行号 / 实测数据，是语义工作。脚本只能数计数（S3d），数不了「这条 claim 对不对」。**S3d 与 S3e 互补不重叠**——脚本管「计数趋势对不对」（确定性），人管「每条 blocker 真不真」（语义）。
- **S6 真实测**：「走一遍用户流程」无法用 `ping` 替代。脚本测不了「功能真通电」，只能由宿主驱动真实操作。
- **S2/S4 创作**：写方案、写代码本身是 host agent 主体工作，不是 LTO 的职责（LTO 是 harness，不是 planner）。

## 五、剩余缺口（按优先级，未来增量脚本化）

已脚本化/半脚本化（本轮同步后）：S3d 收敛判定 + G1/S8 phase-entry evidence 报告 + S0 state.json + S9 resume/check 文件级漂移（限 task `touched_files`）+ runner + judge + hook + artifact manifest + ADR 决策 helper + install-time `lto` wrapper。其余排队：

| 缺口 | 优先级 | 方案 | 现状落点 |
|---|---|---|---|
| 直接 memory-flow 写入桥 | 低 | 在 ADR helper 之后另加显式 `--memory-flow` opt-in | 当前 `write_decision.py` 只写 ADR/state/manifest，不碰凭据 |
| 工作区未提交 task 文件漂移 | 低 | 将 dirty diff 与 task `touched_files` 相交 | 当前仅 dirty warning；S9 文件漂移是 commit-to-commit |
| task `touched_files` 覆盖率 | 中 | runner/task 生产者继续补全 touched_files | 当前无 touched_files 时给精度 warning，不全仓兜底 |

> 增量原则：每个缺口脚本化前过准入闸门（命中频率/误报比/能否复用现有/外部依赖/具体翻车复盘），别为「补全」而造装饰脚本。

已收口：跨 host artifact 索引统一为 `.lto/<run-id>/artifacts.json`。写入点同步登记 replies/briefs/evidence/judge/handoff/decision records；`resume` 默认列 Recent Artifacts，`closeout` handoff 从 manifest 渲染；旧 run 缺 manifest 时只读合成，closed run 不写回。跨 repo 入口由 `scripts/install.sh` 生成 sentinel-managed `lto` wrapper，非托管同名文件按安装冲突退出 2。
