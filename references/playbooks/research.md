# research playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


触发信号：

- 用户要多源研究、路线比较、技术选型、市场/生态判断，且答案不应只依赖单一来源。
- 需要记录 coverage、矛盾、置信度和待验证点。

可用 primitive：

- host agent 分源检索和摘录。
- 必要时 fan-out 不同角度研究，再 synthesis。
- `lto runner --kind manual` 登记关键证据。
- `audit --auto-dispatch` 用于 adversarial source critique。
- 场景插件 `plugins/claim-verify-research` 同样适用本场景（fan-out 检索 + completeness critic）。
  - 挂载示例：`lto plugin mount plugins/claim-verify-research --run-id <run-id>`
  - 静态验证：`lto plugin validate plugins/claim-verify-research --json`

期望 artifact：

- source notes
- contradiction ledger
- synthesis memo
- confidence labels
- open questions

停止条件：

- 关键来源覆盖达标。
- 重大矛盾被解决或显式披露。
- 结论区分 fact、inference、recommendation。

反模式：

- 把搜索结果堆叠成结论。
- 不标明推断。
- 为了“完整”继续无界检索，不回到任务目标。

