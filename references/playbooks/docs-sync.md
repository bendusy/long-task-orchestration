# docs-sync playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> 文档与代码对齐是独立任务形态——既不是 review 也不是 debug。

触发信号：

- 代码大改后；周期性 drift 审计。
- 用户指出文档过时；changelog 与文档口径不一致。

可用 primitive：

- fan-out 多路审计扫 doc-vs-code drift（可挂 `plugins/dev-workflow` 的
  `docs-drift-auditor-v1`）。
- union 合并 findings；`lto runner --kind manual` 登记逐条修复证据。
- 防 drift test-pin：从源码动态抽阈值/命令名，断言文档同值——改了代码
  不同步文档即测试红。

期望 artifact：

- drift findings union 清单（命中 `drift-ok` 有意分歧注记的条目标
  `intentional`，不算 drift）
- 逐条修复 diff
- 防 drift 测试

停止条件：

- union 清单逐条处理完（修复或标 intentional）。
- 防 drift 测试落地并通过。

反模式：

- 只改 README 不查全部引用。
- 修文档不加防 drift test。
- 把有意分歧（ADR / 未来架构描述）当 drift 修掉。

