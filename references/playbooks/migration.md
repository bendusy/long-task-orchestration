# migration playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


触发信号：

- 任务跨多个模块、schema、API contract、shared state 或持久化格式。
- 需要兼容期、rollback、分片合并或批量重命名。
- 单 context 很容易丢 dependency / ordering / blast radius。

可用 primitive：

- `lto task add` 拆 slice
- `lto runner` 逐 slice 落验证证据
- `lto run parallel` / `lto run pipeline` 跑批量 shell 验证
- `lto audit --auto-dispatch` 做 adversarial review
- `lto check --to closed --strict`
- 场景插件 `plugins/migration-refactor`：最小样例先行 + 批间回归闸门路径、diff 审计 / 语义等价 profile
  - 挂载示例：`lto plugin mount plugins/migration-refactor --run-id <run-id>`
  - 静态验证：`lto plugin validate plugins/migration-refactor --json`

期望 artifact：

- migration plan
- slice task list
- per-slice evidence
- compatibility / rollback notes
- audit ledger
- changelog / handoff

停止条件：

- 每个 slice done/skipped 且理由清楚。
- 兼容性、回滚、安全和测试证据都当前于 HEAD。
- adversarial review 收敛，human gate 批准不可逆步骤。

反模式：

- 先抽象再找问题。
- 没有 touched_files / evidence 就 closeout。
- 把 rollback 写成一句话，不验证可执行性。

