# tmux-goal-loop playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> Host 合议 goal → tmux runner 短会话 loop 长跑 → 异构审计 → **host 亲验硬停止点**。
> 这是 repo 内 Rust tmux runner 落地后的闭环 playbook，不是新 CLI，不依赖私有
> `tmux-autopilot` skill。

触发信号：

- 用户给出一个足够大的 goal，需要 host 先合议目标，再派 coding worker 长跑若干轮。
- 单个 headless runner 一发一收无法承载交互式长跑，或单长会话容易 context 膨胀、过早自报完成。
- 已有 `state.tasks` 可拆成短 worker 任务，且每个 worker 的完成能用 evidence / contract 证伪。

可用 primitive：

- `lto start --goal ... --target ... --constraint ... --instrument ... --entropy-check ...`
  记录 goal 四件套。
- `lto task add` 写 feature/task 清单；host 保留拆分和优先级判断权。
- `lto runner --runner tmux --tmux-mode signal|sentinel|fire ...` 直接派可观测 worker，
  或 `lto autopilot --auto-exec --worker-runner tmux` 让现有 autopilot loop
  顺序派一个 bounded worker per pending task。
- `lto audit --auto-dispatch --discover-risks` 做 fresh-context 异构审计；
  runner 输出失败或跑偏时，host 读 live log / reply artifact 后逐条采纳或驳回。
- `lto check --to closed --strict` 和 `lto closeout` 做证据闸门和 handoff。

期望 artifact：

- goal 四件套和 task 清单。
- 每个 worker 的 live log、completion contract 或 runner evidence。
- host triage note：worker 自述了什么、host 一手验了什么、哪些自述被驳回。
- audit replies / audit ledger / redline register。
- host 亲验记录：测试命令、grep/文件读验、产物对比、残余风险。
- changelog / handoff。

Host 亲验硬停止点：

loop 跑完、blocked 或 worker 自报完成后，**不得**把 hook 返回、pane 停止、contract
存在或 agent 文字自述直接当完成。host 或独立 evaluator 必须先做一手核验，至少覆盖：

1. 跑项目自己的红线命令；失败输出必须登记为 evidence。
2. 对照 goal/task 清单逐条打开关键产物或源码，确认 worker 自述和实际 diff 一致。
3. 用 `rg` / 文件读取 / manifest 检查确认没有漏改、错 repo、私有依赖或历史入口冒充当前入口。
4. 对 worker 报告的“全绿”“已完成”“无风险”逐条找一手证据；找不到证据就按未完成处理。
5. 运行 `lto check --to closed --strict`；done task 没有 evidence 时默认 FAIL，不能 closeout。

停止条件：

- 所有 task 为 done/skipped，且 done task 都带 evidence。
- high/critical audit blocker 收敛到 0；采纳/驳回都有 path:line、命令输出或 artifact 证据。
- host 亲验清单完成并登记；如果有人类 override，残余风险写入 handoff。
- `git status --short` 干净；最终状态重新跑红线。

反模式：

- 让一个 worker 啃整个大 goal，并把它的自述当验收。
- loop 完成后直接 closeout，不读 diff、不跑测试、不看 artifact。
- 把 `tmux` pane 停止、sentinel 文件、contract 文件存在等同于语义完成。
- 把这个 playbook 抽成替 host 做判断的 `orchestrate` 命令。
- 依赖 host 侧私有 skill 或本机隐藏脚本，导致 stranger 无法复现。

