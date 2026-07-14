# review playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


触发信号：

- spec、代码路径、设计决策或任务涉及 auth、payment、migration、schema、security、concurrency、external API。
- 你对自己方案有明显路径依赖，或者同一模型已经连续多轮自审。
- closeout 前存在未审 high-risk task。

可用 primitive：

- `lto audit --auto-dispatch`
- `lto audit --discover-risks`
- `lto collect-agent-run --task-id <id> --runner <runner> --reply <reply.md>` for manually produced replies
- `lto check --to implementation|closed`
- `lto judge --phase <phase>`
- 场景插件 `plugins/adversarial-audit`：refute-first prompt、codex/pi/agy 跨族 profile、union 合并收敛路径
  - 挂载示例：`lto plugin mount plugins/adversarial-audit --run-id <run-id>`
  - 静态验证：`lto plugin validate plugins/adversarial-audit --json`

期望 artifact：

- audit brief
- heterogeneous replies
- structured findings JSON
- audit ledger
- judge verdict

停止条件：

- high/critical blocker 收敛到 0，或 human 明确 override 并记录理由。
- 每条采纳/否决都有源码、命令、截图或文档证据。

反模式：

- 让同一家 runtime 自审。
- 只看“三个 agent 都说没问题”，不核源码和证据。
- 把 `review` 做成一键通过闸门。

注：building 阶段的中途 verification 也适用本节——边建边验，不等到收尾才审。

