# LTO：长任务 harness

> 做一个大功能要几十轮对话。LTO 给主 agent 一套可恢复、可审计、可自动化推进的任务操作系统：状态、证据、审计、沙箱、恢复、回顾和人工闸门。

LTO 不替你写代码，也不替你选完整路径。host agent 仍然是 planner；LTO 提供可组合的 harness primitive：
1. **该不该做**——缺的东西现在真需要吗？答不上就停
2. **写方案**——把可能出问题的地方标出来
3. **对抗审计**——派跟你不一样的异构 AI 自动审（`audit --auto-dispatch`），不用手工跑脚本
4. **写代码**——runner 执行 + judge 审查 + 证据落盘
5. **动态决策支持**——`next`/`autopilot` 整理事实、提供 safe action、必要时升级，最终路径由 host agent 判断
6. **跨 session 回顾**——`recap` 用人话告诉你：做什么/为什么/跑多久/做到哪/还剩啥/现在轮到你
7. **部署 + 收尾**——pre-deploy hook 闸门 + `closeout` 自动生成 CHANGELOG.md

## 快速开始

```bash
L="python3 skills/long-task-orchestration/scripts/lto_run.py"
# 装过 scripts/install.sh 且 lto 在 PATH 后可用：L="lto"

# 开一个长任务（记下为什么做、做完的标准——recap 会用到）
$L start --goal "做用户登录" --why "降低登录失败率" --done-when "失败率<5%，三端覆盖"

# 加任务（task 是 runner/next/audit 的操作对象，runner 不会自动建）
$L task-add --task-id T1 --title "给 login 加判空" --command "pytest tests/ -x"

# 断点恢复：resume 给 AI 拉上下文 / recap 给人看回顾
$L resume                      # 喂接手的 AI（git head / task 状态）
$L recap                       # 给人看（人话回顾，长 gap 后尤其有用）

# 环境探活 + 状态校验
$L preflight
$L check --strict

# 执行 task + 落证据
$L runner --task-id T1 --kind test --command "pytest tests/ -x" --touch src/auth.py

# 批量跑 shell 校验命令（不是 agent fan-out）
$L parallel --phase implementation --concurrency 4
$L pipeline --stages "ruff check {task_id}" "pytest -k {task_id}"

# 审查 + 对抗审计
$L judge --phase implementation --rerun-tests
$L audit --auto-dispatch       # 自动派异构三方审 + 收口判收敛
$L audit --discover-risks      # 派 agent 主动找漏掉的风险点

# 动态决策 / 自驱推进
$L next                        # 出事实简报 + 无歧义命令建议（零 LLM）
$L autopilot --supervised      # 读状态出 brief，回吐 host agent 判断
$L autopilot --supervised --auto-exec   # 安全子步骤在 worktree 沙箱自动跑

# 实验路径插件（data-only；挂载只记录 provenance，不自动改 core）
$L plugin list
$L plugin validate plugins/deep-agent-profiles
$L plugin render-profile plugins/deep-agent-profiles codex-audit-readonly-v1 \
  --input brief.md --output rendered-brief.md
$L plugin eval plugins/deep-agent-profiles --json  # static eval-pack validation
$L plugin mount plugins/deep-agent-profiles
# Real runtime eval design: references/plugin-real-eval-runner.md

# 隐私自检（默认只报告；--delete 逐项输入 delete 才删）
scripts/privacy_self_check.sh --repo .

# 边界闸门 + 收尾
$L hook pre-commit
$L closeout --summary "做了什么，验证了什么"   # 自动生成 CHANGELOG.md
$L self-test                   # LTO 自检
```

## 对抗审计：让不同 AI 帮你审

```bash
# 推荐（自动）：派异构三方（≠你这家）审 + 自动收口判收敛
$L audit --auto-dispatch

# 手工（没装 agent-delegate 时）：自己派工，回复存一个目录再收口
AD=~/Projects/agent-skills/skills/agent-delegate/scripts/runners
$AD/codex.sh 方案.md replies/reply-codex.md 300 &
$AD/agy.sh   方案.md replies/reply-agy.md   300 &
wait
$L audit --collect replies/
```

## 关键约束（不会变的底线）

- **决策权留给人**：`next` 只出决策简报，最终拍板是你/宿主 LLM，不是 LTO 替你决定。
- **Preset 是 playbook，不是菜单**：`review/debug/migration/claim-verify/research` 是 host agent 的调度先验，先读 playbook 再组合 primitive，不先做硬路由。
- **git push 永不自动**：任何 autopilot 档位，push / 部署 / 删库等不可逆操作都停下来等人确认。
- **hook 默认不装**：`lto start --install-hooks` 才装（opt-in，撞 husky/pre-commit 会跳过）；auto-commit 也默认关。
- **autopilot 当前只到 supervised**：`--supervised`（出 brief）和 `--auto-exec`（worktree 沙箱跑 safe 子步骤）已实现；`--autonomous`（spawn 决策 agent 全自动）是下一期，未实现。
- **自动执行有沙箱**：`--auto-exec` 的命令全在独立 git worktree 副本里跑，`rm -rf` 再狠也只炸可弃的 worktree，主工作树/系统/凭据毫发无损。
- **外部观点先进插件**：文章/方法论先收录为 source note，再做 experimental path plugin + eval；验证前不进 core。

## 什么情况不要用

| 你要做 | 用这个 |
|---|---|
| 修个 bug | diagnose |
| 让人审代码 | review |
| 部署上线 | ship |

## 安装

把整个 `long-task-orchestration/` 文件夹放到你的 agent skills 目录里就行。详见 [INSTALL.md](./INSTALL.md)。

## 更多

- [SKILL.md](./SKILL.md) — 完整导航手册
- [references/onboarding.md](./references/onboarding.md) — **给 agent 读一份就懂怎么装载 LTO**
- [references/workflow-playbook.md](./references/workflow-playbook.md) — host agent 的 playbook 调度先验
- [references/control-loop-harness.md](./references/control-loop-harness.md) — 控制论 harness：run logs / telemetry / 性能成本质量闭环
- [references/plugin-real-eval-runner.md](./references/plugin-real-eval-runner.md) — plugin 真实世界 eval-run 设计边界（sub-LTO-run compiler）
- [references/privacy-self-check.md](./references/privacy-self-check.md) — 本机 AI coding 隐私自检与逐项确认清理
- [references/sharing-guide.md](./references/sharing-guide.md) — 怎么装依赖、怎么给朋友用、项目级注入
- [references/cross-runtime-host-notes.md](./references/cross-runtime-host-notes.md) — 不同 AI 工具当宿主的具体用法
