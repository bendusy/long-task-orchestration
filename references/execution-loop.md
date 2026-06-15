# LTO 执行循环器：harness primitives

> runner 执行 task + 自动记录证据，judge 只读审查 + YAML verdict，parallel/pipeline 批量执行多个 task 的校验命令。它们是 host agent 组合 workflow 的 primitive，不是完整业务流程执行层。

> **命名诚实说明**：LTO 的 `parallel`/`pipeline` 借用了 pi-dynamic-workflows 的**命令名**，但**语义不同**。
> pi-dynamic-workflows 编排的单元是 `agent(prompt)`——拉独立子 agent 跑 LLM 任务（fan-out + 隔离 + 沙箱确定性）。
> LTO 编排的单元是 **shell 命令**（`pytest`/`lint` 等批量校验），跑在同一个 repo cwd、共享 git HEAD。
> 这是**命令批处理**，不是 agent fan-out。需要真正的多 agent 分工走 `lto audit --auto-dispatch`（Rust CLI 路径，scheduler 派异构审计方）或 repo 自带 `scripts/delegate/`（codex/pi/agy 手动 fan-out）。

## parallel

并发批量执行多个 task 的 shell 校验命令，每个 task 落 evidence。

```bash
L="lto"  # or: L="cargo run --quiet --"

# 并发执行某 phase 下所有 pending task
$L parallel --phase implementation --concurrency 4

# 并发执行指定 task
$L parallel --task-ids T1 T2 T3 --concurrency 3

# 自定义命令
$L parallel --kind test --command "pytest tests/ -x" --timeout 600
```

输出实时进度：
```
◆ LTO Parallel: 3 tasks (3 concurrent)
  ✓ T1: PASS: task one
  ✓ T2: PASS: task two
  ✗ T3: FAIL: task three
◆ 2/3 passed (12.3s)
```

## pipeline

让每个 task 串行通过多个 shell stage（item 间并发），每个 stage 落 evidence。stage 命令里用 `{task_id}` 占位符。

```bash
# 每个 task 依次跑 lint → test 两个 stage
$L pipeline --phase implementation \
  --stages "ruff check {task_id}" "pytest -k {task_id}" --concurrency 4
```

行为：
1. 每个 stage 用 `{task_id}` 替换后执行（**只替换 task_id，不替换 title**——title 是用户自由文本，拼进 shell 是注入面）
2. 每个 stage 结果作为一条 evidence 追加到对应 task
3. 全部 stage rc=0 → task.status=done；任一失败 → blocked（除非 `--continue-on-error`）
4. stdout/stderr 落 artifact 到 `.lto/<run-id>/evidence/`

> runner / parallel / pipeline 三者共用 `lto/exec.py` 的 `run_command` 内核（**shell 层**），evidence 契约一致。
> LTO 有两层 harness：**shell 层**（runner/parallel/pipeline，跑 pytest/lint）+ **agent 层**（next/autopilot/audit，组织隔离 agent 和决策证据，见文末「Agent 执行层」）。host agent 负责选择何时组合这些 primitive。

## runner

单 task 执行 + 自动证据记录。

```bash
$L runner \
  --task-id T1 \
  --kind test \
  --command "pytest tests/test_auth.py -x" \
  --timeout 300 \
  --touch src/auth.py \
  --note "验证登录空指针修复"
```

行为：
1. 读 state.json 确认 task 存在且状态为 pending|in_progress
2. 执行 command，捕获 stdout/stderr/rc
3. rc=0 → task.status=done，追加 evidence
4. rc!=0 → task.status=blocked（或 in_progress，见 --status-on-fail），追加 blocker
5. 证据 artifact 保存到 `.lto/<run-id>/evidence/`
6. 写回 state.json

参数：
- `--task-id`：目标 task ID
- `--kind`：test|lint|build|manual|review|deploy
- `--command`：shell 命令
- `--cwd`：工作目录（默认 repo 根）
- `--timeout`：超时秒数（默认 300）
- `--touch`：本 task 修改的文件列表
- `--note`：人类可读摘要
- `--status-on-fail`：失败后 task 状态（blocked|in_progress）
- `--auto-commit`：提交 `.lto` 状态改动（**opt-in，默认关**，见下「git 提交策略」）

## judge

只读审查 + YAML verdict 输出。

```bash
# 审查整个 phase
$L judge --phase implementation --rerun-tests

# 审查单个高风险 task
$L judge --task-id T5
```

行为：
1. 读 state.json 获取 task 列表
2. 可选：重新运行 task 中记录的 test 命令（--rerun-tests）
3. 生成 YAML verdict
4. 保存到 `.lto/<run-id>/judge/judge-<phase>-<ts>.yaml`
5. 更新 state.json 的 `gates.last_reviewed_head`

verdict 格式：
```yaml
verdict: pass|fail
reviewed_head: abc123
runner: codex
phase: implementation
must_fix:
  - task: T2
    reason: "null check missing at src/auth.py:156"
    files: [src/auth.py]
should_fix: []
scope_drift: []
residual_risks: []
next_action: fix_and_rerun|commit_allowed
```

## 微循环流程

```
runner T1 → rc=0 → done → evidence 记录
runner T2 → rc=1 → blocked → blocker 记录
  → 人工/Codex 修复 → runner T2 → rc=0 → done
phase 完成 → judge --phase implementation → pass → 下一 phase
```

## judge 时机

- 每 phase 完成后：默认审查
- 高风险 task 单独审查：涉及持久化格式、迁移、权限、安全、并发、外部接口
- commit 前：至少有一次 judge 或显式 `--force --reason`

judge 不修改工作区文件（只读），只提 findings。

## git 提交策略（opt-in，2026-06-03 修正）

runner / judge / parallel / pipeline / closeout **默认不自动 git commit**——只写文件，把提交权还给你。

```bash
# 默认：只更新 .lto，打印提示，不 commit
$L runner --task-id T1 --command "pytest"

# 显式 opt-in：用仓库真实 git identity 提交 .lto 改动
$L runner --task-id T1 --command "pytest" --auto-commit
```

规则：
- 默认 `--auto-commit` 关。关时打印 `git add .lto && git commit` 提示，不替你提交。
- 开时用**仓库真实 `git config user.name/email`**，**不伪造 `lto@example.invalid` 身份**（避免污染 blame + 违反「禁止自动元数据」）。
- 仓库未配置 git identity 时**拒绝 commit 并提示**，绝不静默成功。
- commit 失败（如被 pre-commit hook 拒）会**打印 rc 和错误**，不再静默吞错。
- closeout 的 `--auto-commit` 还会提交 `CHANGELOG.md`（用户真实产物，更要显式授权）。

## Agent 执行层（harness primitive）

shell 层（上面）编排 shell 命令；agent 层编排带独立 context 的隔离 agent。

- **`agent_job.py`** — `AgentJob`/`AgentResult` 数据合同（agent 世界，区别于 shell 的 command/rc）。字段含 runner/model/isolation/output_schema/parent_pattern/budget/retry_policy/verifier_of。
- **`scheduler.py`** — 并发调度 + 退出码三元判定（OK/FAILED/TIMEOUT/RATE_LIMITED，429 不当成功也不当 timeout）+ 指数退避重试（带总上限）+ healthcheck gate（派工前剔除挂的 runner）。
- **`agent_exec.py`** — `spawn_agents()` spawn 原语：组装 AgentJob → 调 scheduler → 落 `state.agent_runs`。`audit --auto-dispatch`/`--discover-risks` 的底层。
- **`next.py`** — 事实简报器（零 LLM）：`analyze` 状态 → 无歧义给 argv 命令 / escalate 给宿主 LLM 富决策简报。它不选完整路径；host agent 读 brief 后决定下一段 pattern。`--exec` 走 shell=False（无注入面）。
- **`progress.py`** — 推进检测 + stall 闸门：`progress_digest`/`has_progressed`。推进 = done↑/ledger↓/risk verified/blocked↓(带新成功证据)；同失败指纹 = 停滞。单向棘轮防伪推进博弈。
- **`worktree_exec.py`** — autopilot 自动执行沙箱：`run_in_sandbox` 在独立 worktree 跑命令 + env 隔离 HOME/凭据 + 17 类危险操作（rm -rf/git push/curl|sh/绝对路径逃逸…）HELD 不执行。
- **`autopilot.py`** — 受约束推进 harness，三档：`--supervised`（出 brief 回吐宿主）/ `--auto-exec`（沙箱跑 safe 子步骤，retry/stall 刹车）/ `--decide`（escalate 时 opt-in spawn 三方异构 agent 跑双轨收敛，出 brief 给宿主读，决策权仍归宿主；配 `--decide-kind direction|review|both` + `--decide-budget`，已实现）/ `--autonomous`（机械证据闸门 + 机械执行，已实现）。**autonomous 不 spawn 决策 agent、不替宿主反思**——读跨 run 挖掘事实判证据闸门（攒够真实派工才解锁，不够退回 supervised），过闸后机械推进 safe 子步骤；escalate/dangerous/push（含 `git -C . push` 变体）/网络停人类，与 `--decide` 互斥。
- **`recap.py`** — 面向人类的回顾（与 resume 正交：resume 喂 AI，recap 给人）。
