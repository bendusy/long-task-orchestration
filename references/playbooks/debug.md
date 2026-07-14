# debug playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


触发信号：

- task blocked，且有失败命令、stderr、日志、截图或用户可复现步骤。
- 同一失败指纹反复出现。
- 你有多个假设但没有证据排序。

可用 primitive：

- `lto runner --task-id <id> --kind test --command "..."`
- `lto next`
- `lto autopilot --supervised`
- 必要时手动或通过 repo 自带 `scripts/delegate/` fan-out 多个独立诊断假设。

期望 artifact：

- 最小复现命令
- stdout/stderr evidence
- 假设列表和排除理由
- 修复后通过的验证命令

停止条件：

- 一个根因被证据支持，并且修复后验证通过。
- 或所有合理假设都被排除，留下明确的 next diagnostic / human question。

反模式：

- 没有复现就改代码。
- 并发派多个 agent 改同一片文件。
- 把一次通过当成根因证明，不记录失败假设。

