# direction-review playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> 方向 / 品味分歧与 findings 审计本质不同：findings 用 union 合并
> （一条不漏），方向分歧默认升级人类——票决只是受限工具。

触发信号：

- 架构边界判断；两个都对但只能选一的方案分歧。
- 审计方之间出现非事实性矛盾。

可用 primitive：

- 分歧分类：先判定是「证据可裁决」（有 path:line / 命令输出 / 官方文档
  可核）还是「品味/方向」（无独立证据可裁决）。
- 证据可裁决 → 派异构核验，按证据裁决（不投票）。
- 品味/方向 → 升级人类；异构意见仅作为 advisory 证据附上。
- 决策档落 decision log 类位置（见 `decision-logging.md`）。

期望 artifact：

- 分歧分类记录
- 各方立场与证据
- 决策档（含最终裁决与理由）

停止条件：

- 证据分歧被证据裁决；品味分歧由人类拍板并落档。
- 任一审计方给出 needs_human 即直接升级，不被多数票否决；
  2/3 票仅在人类显式授权「按多数走」时使用。

反模式：

- 用 findings union 流程处理方向分歧（永不收敛）。
- 让同族模型投三票。
- 用 2/3 票否决 needs_human。
- 票决品味问题。

