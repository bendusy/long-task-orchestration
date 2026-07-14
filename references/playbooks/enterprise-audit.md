# enterprise-audit playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> 高风险变更的分层审计门禁。十层（requirements → architecture →
> data-model → interface-contract → implementation → testing →
> operations-observability → security → migration-rollback → acceptance）
> 是覆盖模型，不是每个小改动都要跑的委员会。

触发信号：

- 变更涉及 schema、API contract、auth/security、migration/rollback、并发、
  新模块、对外发布或生产运维面。
- 用户要求“大厂标准”“全流程审计”“Bar Raiser / 架构评审委员会式”覆盖。
- 你发现普通 implementation review 无法覆盖需求、上线、回滚、验收等上下游风险。

可用 primitive：

- `lto plugin mount plugins/dev-workflow` 后读
  `paths/enterprise-audit-gate.json` 做 scope triage。
- 用 `profiles/enterprise-layer-auditor-v1.json` 给 codex/pi/agy/claude 等
  非同族 runner 派读-only layer audit；高风险默认至少 3 个 distinct families。
- `collect-agent-run` / artifact evidence / audit ledger 做 union 收口；host 逐条核
  path:line 或命令输出。
- 有方向争议时转 `direction-review`；有实现 blocker 时回到 `review` /
  `migration` / `feature-dev` 对应修复循环。

期望 artifact：

- layer scope matrix（十层 mandatory/exempt + 理由）。
- per-layer structured findings JSON（含 layer/redline/severity/evidence）。
- redline register 与 host triage record。
- test-pin / contract check / rollback evidence / acceptance record。

停止条件：

- 每个 mandatory layer 都有异构只读审计证据或记录了 dispatch failure。
- HIGH/CRITICAL redline 为 0，或人类显式 override 并记录 residual risk。
- 每条采纳/否决都有一手证据；每个豁免都有理由。

反模式：

- 小改动无脑跑十层，制造仪式成本。
- 单个模型审全部层后宣称“独立审计”。
- 用多数票丢掉某个 runner 提出的 redline。
- 把 exit code 0 当成 acceptance，而没有读产物、契约、回滚和人类 gate。

