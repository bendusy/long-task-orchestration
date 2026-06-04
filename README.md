# LTO：长任务的导航仪 + 自驱编排器

> 做一个大功能要几十轮对话。LTO 帮你**不迷路、不做过头、知道什么时候停**——关键步骤还能自动路由（pattern 运行时选）和在 worktree 沙箱里自动跑。

LTO 不替你写代码，它帮你管七件事：
1. **该不该做**——缺的东西现在真需要吗？答不上就停
2. **写方案**——把可能出问题的地方标出来
3. **对抗审计**——派跟你不一样的异构 AI 自动审（`audit --auto-dispatch`），不用手工跑脚本
4. **写代码**——runner 执行 + judge 审查 + 证据落盘
5. **动态决策**——`next`/`autopilot` 让 pattern 路由器运行时选下一步（不是人工编排）
6. **跨 session 回顾**——`recap` 用人话告诉你：做什么/为什么/跑多久/做到哪/还剩啥/现在轮到你
7. **部署 + 收尾**——pre-deploy hook 闸门 + `closeout` 自动生成 CHANGELOG.md

## 依赖

核心 LTO 只要求：

| 依赖 | 用途 |
|---|---|
| Python 3.10+ | 运行 `scripts/lto_run.py` 和自检 |
| bash | 运行安装脚本、wrapper 和 shell runner |
| git | 记录 HEAD、检测 drift、创建 worktree 沙箱 |
| 至少一个 host runtime | Codex / Claude Code / pi / agy / 其他能读 `SKILL.md` 并跑 shell 的 agent |

可选增强：

| 可选项 | 解锁能力 | 没装时 |
|---|---|---|
| `tmux` | 内置 delegate 的可观测并行窗口 | 无 tmux 时自动用 headless 子进程 |
| `codex` / `claude` / `pi` / `agy` CLI | 多模型家族交叉审计 | 单 runtime 自审，必须声明对抗性较弱 |
| ANIMEM 或 memory-flow compatible sink | 跨 runtime / 跨项目 artifact memory | 本地 `.lto` + ADR 仍完整可用 |

LTO 预装一份最小 delegate runtime：`scripts/delegate/triad.sh`、
`scripts/delegate/delegate.sh`、`scripts/delegate/runners/*`。自动派工默认用这份内置脚本；
需要替换成外部 agent-delegate 时，再设置 `AGENT_DELEGATE_HOME` /
`AGENT_DELEGATE_TRIAD` / `AGENT_DELEGATE_RUNNERS`。

`bash scripts/install.sh --check` 会检查核心 CLI；可选 runtime 是否可用以
`lto preflight` 和 `scripts/delegate/runners/healthcheck.sh` 的实测结果为准。

## 快速开始

```bash
L="python3 scripts/lto_run.py"

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
$L next                        # 出决策简报 + 路由建议（零 LLM）
$L autopilot --supervised      # 自驱：出 brief 回吐你判断
$L autopilot --supervised --auto-exec   # 安全子步骤在 worktree 沙箱自动跑

# 边界闸门 + 收尾
$L hook pre-commit
$L closeout --summary "做了什么，验证了什么"   # 自动生成 CHANGELOG.md
$L self-test                   # LTO 自检
```

## 对抗审计：让不同 AI 帮你审

```bash
# 推荐（自动）：派异构三方（≠你这家）审 + 自动收口判收敛
python3 scripts/lto_run.py audit --auto-dispatch

# 手工：自己派工，回复存一个目录再收口
AD=${AGENT_DELEGATE_RUNNERS:-scripts/delegate/runners}
$AD/codex.sh 方案.md replies/reply-codex.md 300 &
$AD/agy.sh   方案.md replies/reply-agy.md   300 &
wait
python3 scripts/lto_run.py audit --collect replies/
```

## 关键约束（不会变的底线）

- **决策权留给人**：`next` 只出决策简报，最终拍板是你/宿主 LLM，不是 LTO 替你决定。
- **git push 永不自动**：任何 autopilot 档位，push / 部署 / 删库等不可逆操作都停下来等人确认。
- **hook 默认不装**：`lto start --install-hooks` 才装（opt-in，撞 husky/pre-commit 会跳过）；auto-commit 也默认关。
- **autopilot 当前只到 supervised**：`--supervised`（出 brief）和 `--auto-exec`（worktree 沙箱跑 safe 子步骤）已实现；`--autonomous`（spawn 决策 agent 全自动）是下一期，未实现。
- **自动执行有沙箱**：`--auto-exec` 的命令全在独立 git worktree 副本里跑，`rm -rf` 再狠也只炸可弃的 worktree，主工作树/系统/凭据毫发无损。

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
- [references/sharing-guide.md](./references/sharing-guide.md) — 怎么装依赖、怎么给朋友用、项目级注入
- [references/cross-runtime-host-notes.md](./references/cross-runtime-host-notes.md) — 不同 AI 工具当宿主的具体用法
