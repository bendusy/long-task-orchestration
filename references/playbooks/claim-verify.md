# claim-verify playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


触发信号：

- 文档、spec、研究输出、对外文章中有事实、版本、引用、价格、API 行为、法律/政策、技术能力声明。
- 错一个 claim 会影响决策或对外发布。

可用 primitive：

- host agent 自行抽 claim table。
- 对稳定事实用本地源码/文档验证；对时效性事实用 web/context7/官方文档验证。
- 可用 `lto runner --kind manual` 登记核验证据。
- 高风险时走 `audit --auto-dispatch` 做 source adversarial review。
- 场景插件 `plugins/claim-verify-research`：claim 拆解 / 证据反驳 / completeness critic 三类 profile 与主路径。
  - 挂载示例：`lto plugin mount plugins/claim-verify-research --run-id <run-id>`
  - 静态验证：`lto plugin validate plugins/claim-verify-research --json`

期望 artifact：

- claim ledger
- source/evidence map
- supported / refuted / unknown verdict
- unresolved claims list

停止条件：

- 每个 material claim 都有 supported/refuted/unknown。
- unknown 不被改写成确定口吻。
- 对外发布前 human gate 批准残余不确定性。

反模式：

- 用“看起来像”替代来源。
- 让 LTO 编造 source artifact。
- 把 research synthesis 当 verification。

