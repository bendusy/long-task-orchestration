# 长程状态纪律 — stale 免疫三层证据清单 + 后台编排

> SKILL.md 多轮任务和恢复纪律的执行细节。跨 /compact、多 MVP、后台并行下不迷失的具体动作。

## 一、stale 唤醒免疫：三层一手证据交叉确认

/compact 续会话、心跳唤醒、或任何二手 summary 给的「当前状态」，**先用一手证据确认再动手**。三层缺一不可全信：

| 层 | 查什么 | 命令示例 | 证明什么 |
|---|---|---|---|
| 代码层 | git HEAD + commit 内容 | `git log --oneline -8 && git rev-parse HEAD` | 代码改动真落了没 |
| 运行层 | 服务 pid + health + bin mtime | `ssh <prod-host> 'ss -tlnp \| grep <port>; stat -c %y <bin-path>'` | 生产真在跑哪个版本 |
| 落盘层 | 读 run 的 artifact 真源 | `lto resume`（打印 recent artifacts）/ 直接读 `.lto/<run-id>/artifacts.json` | 这事真做完没（落盘是完成标志） |

**冲突时信证据**：续会话指令描述的状态比一手证据旧 → 信证据，拒绝重做。

实证：/compact 后收到「W3 待部署 commit dcde565」→ 三层核验发现 HEAD=062a837（含修复）+ 生产 bin 编译于实测后 + 里程碑 slug 已存在 → 判定指令 stale，**没有重复部署、没有二次污染生产**。

### run-state 文件是恢复锚点

新开长任务时，跑 `scripts/lto_run.py start --goal <goal>`（默认 `--profile minimal` 只创建 `run-state.md`；`--profile audit` 加 `audit-ledger.md`；`--profile deploy` 在 audit 基础上再落一份 preflight 环境快照进 `state.json`）。每次进入新阶段、派后台审计、收到 reply、用户拍板、部署或观察窗结束，都更新 run-state。恢复时先跑 `scripts/lto_run.py check [--strict] [--json]`；要判断能否进写码/收尾，再跑 `scripts/lto_run.py check --to implementation|closed [--strict]` 读 phase-entry evidence——注意 `check --to` 出的报告带 `human_gate_required: true`，不自动放行；真正推进 phase 用 `scripts/lto_run.py phase --set <phase>`。之后按上面的三层证据核验；run-state 和证据冲突时，信证据并修正 run-state。

## 二、后台派工不阻塞 + 等待期挖地基

> 本节的 `Workflow` / `task-notification` / `ScheduleWakeup` 是**宿主 agent 平台**（如 Claude Code harness）的能力，不是 LTO 命令；`scripts/delegate/triad.sh` 才是 repo 自带的 tmux 派工脚本。下面描述的是「主 agent 在宿主环境里怎么用 LTO」的工作模式。

- **派**：审计/调研用宿主的 `Workflow`（后台）或 repo 自带 `scripts/delegate/triad.sh`（tmux window）跑，主对话立刻去做别的。
- **不空等**：完成会主动通知（宿主的 task-notification）。**别轮询**——harness 会叫醒你。
- **设兜底心跳**：用宿主的 `ScheduleWakeup` 设长心跳（1200s+）防后台任务挂死。注意：短心跳轮询已完成的后台任务是浪费——只设长兜底。
- **等待期做什么**：趁等待**挖下一步需要的事实地基**（真实代码 / 真实分布 / 真实配置），不靠记忆。等结果回来时，地基已备好可立刻校验子代理产出。
- **回收即记**：后台任务返回后，把 reply 路径、exit、字节数、采纳/否决状态写回 run-state；否则下一次 resume 只剩口头印象。

实证范式（transcript 高频）：「检查 X workflow (id) 是否完成，完成则…」——派后台 → 主对话推进别的 → 回来检查结果再处理。

## 三、错峰不减深度

多批并行任务**分批起**（避免几十上百并发把系统打挂，如「144 并发」），但**每批都完整深做**，不为省时间砍深度。「第二批要等第一批完成」是错峰，不是减量。

## 四、模型分工

| 模型 | 干什么 | 为什么 |
|---|---|---|
| Opus | 裁决 / 综合 / 起 spec / 三方分歧仲裁 | 高风险判断强模型亲做 |
| Sonnet | 重调研 / 批量代码扫 / 多路并行 | 额度足 + 隔离主 context 不被搜索结果污染 |

**依赖感知**：无依赖才 fan-out；有依赖链（核实真代码 → 改 → 测 → 部署）串行 + 任务清单跟踪。子代理产物回收后主 agent 二次核验再用。

## 五、阶段性推进 + 生产边界

- **阶段闸**：上一层稳了才进下一层。实证「框架稳定后才开始讨论数据治理」「W1 跑稳再推 W3」。
- **生产边界 = 审计边界**：服务端只在受限网络可达（如 PG 仅绑回环、服务仅绑内网地址）。开发机查被 refused **不是 bug**，是边界正确。不为「方便」打穿它，生产数据不离开生产环境。
