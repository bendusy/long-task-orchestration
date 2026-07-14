# feature-dev playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> 新需求从零到定版的全链路。六阶段（specify → dispatch → impl-audit →
> converge → acceptance → release）是调度先验不是状态机，可在任何阶段
> 进入、跳过、退出。设计依据与验收闸门定义见 `dev-workflow-spec.md`。

触发信号：

- 新需求 / 新功能从零开始；改动会产生新模块或新的对外行为。
- 你发现自己想跳过 spec 直接写码。

可用 primitive：

- `lto start --goal/--why/--done-when` 记录目标与完成标准。
- `lto task add` 按阶段拆 task。
- 开发前补齐四证据：架构对齐、第一性原理、精简去重、价值测评。
- specify 阶段挂 `plugins/dev-workflow`（spec co-design 审可复用
  `plugins/adversarial-audit` 的 refuter profile）。
- `lto runner` 落实现证据；`lto audit --auto-dispatch` 做 impl-audit。
- 长目标要交给 codex/pi/agy 在 tmux 中自驱时，用 `lto dispatch-goal`
  派 goal；普通 Codex Stop 只写 per-turn 的 `agent.turn.completed`，
  transcript 中出现真实 `update_goal complete` 后才写
  `agent.dispatch.completed`。pi/agy 走真实 TUI，进程退出 wrapper 把真实
  rc 写入同一个 dispatch 完成事件。wait/cleanup 只认 dispatch 事件。
- 自动窗口名是 `lto:<runner>:<goal-slug>`；显示名不参与程序寻址。
  LTO 自建窗口的不可变 `@window_id` 记录在 run state，成功后清理，失败、
  timeout、交互阻塞或 `--keep-window` 时保留。显式用户 `--target` 不会被
  纳入清理，除非它本来就是该 run 记录的 retained LTO 窗口。
- worktree_exec 在 dispatch 阶段隔离写入（specify 全程 read-only，
  spec 收口后才开 worktree）。
- `lto judge` / `lto closeout`。
- 观测性查看触发条件：派工后看 `.lto/<run-id>/live/` 实时日志；收敛
  卡壳看 `events.jsonl` 与 telemetry；做完复盘看 interventions 记录与
  `lto recap --mine`。

期望 artifact：

- spec v1 与 v2（含异构审订正记录）
- architecture alignment / first-principles / simplification-dedupe / value-measurement note
- documentation alignment / historical cleanup / clean worktree / rebuild-package note
- worktree 分支与 per-task evidence
- findings union register
- test-pin 测试文件
- 验收闸门六条自查记录
- changelog entry

停止条件：

- 验收闸门六条同时满足：脚本全绿 / 实物读验 / 对抗审收敛 / 文档同步 /
  经验入库 / 可观测（新功能模块三件套：结构化日志 schema、doctor 入口、
  排障命令）。任何一条豁免须显式记录理由。
- 调优类改动必须给出 baseline 与复测结果；精简类改动必须说明删减/复用的代码路径。
- 收尾类改动必须证明文档口径一致、历史残留已处理、仓库 clean，并从最终状态重新打包/编译。

反模式：

- 跳过 spec 直接写码。
- 不对齐架构、不做第一性推导，直接堆实现。
- 把没有 baseline/复测的“调优”当优化成果。
- 自审代替异构审。
- 对抗审提到的不变量不落回归测试（test-pin 缺位）。
- 把「实现完」当「做完」。
- 先打包后继续改文件，或带着未解释的 dirty worktree 宣称完成。
- 观测性永远滞留 backlog。

