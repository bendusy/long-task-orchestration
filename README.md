# LTO：长任务 harness

> 做一个大功能要几十轮对话。LTO 给主 agent 一套可恢复、可审计、可自动化推进的任务操作系统：状态、证据、审计、沙箱、恢复、回顾和人工闸门。

## 最新更新（v0.3.0，2026-06-09）

这一版围绕一个核心：让 lto **越用越聪明**——但聪明的是你这个主 agent，不是 lto。lto 只机械地把真实数据摆全，反思和决策永远归你。

一是新增了跨 run 数据挖掘——lto 把历史所有 run 扫一遍，机械算出"哪个 agent 模型在哪类任务上真的有效、哪个阶段反复卡壳"，出一份给你看的事实简报（`recap --mine`）。它只摆事实和假设，绝不替你拍板"该用谁"。

二是这份挖掘现在能区分到具体模型——不只看 codex/pi 这种家族，还能分出同一个 pi 跑 deepseek 还是 glm，谁在你的活儿上更靠谱一目了然。

三是给插件评测加了质量判读——测一个工作流插件有没有效时，现在能派一个跟产出方不同家的 agent 来判它质量好不好、是不是误报，判读结果连同证据一起冻结存档（可复现），但只作参考，绝不影响"插件能不能晋升"的机械结论。

四是补全了 autopilot 的 autonomous 档——但它**不是**让 lto 自己做决定的全自动回路。它只做两件机械的事：先看跨 run 攒够真实数据没（没攒够就诚实退回半自动），攒够了就在沙箱里机械跑那些安全可逆的小步。要判断、要反思、要推代码，永远停下来回吐给你。

完整技术条目见 [CHANGELOG.md](CHANGELOG.md)。（v0.2.0：codex 假死修复 + token 计量 + 运行可观测 + events 留痕，详见 CHANGELOG。）

LTO 不替你写代码，也不替你选完整路径。host agent 仍然是 planner；LTO 提供可组合的 harness primitive：
1. **该不该做**——缺的东西现在真需要吗？答不上就停
2. **写方案**——把可能出问题的地方标出来
3. **对抗审计**——派跟你不一样的异构 AI 自动审（`audit --auto-dispatch`），不用手工跑脚本
4. **写代码**——runner 执行 + judge 审查 + 证据落盘
5. **运行中可见 + 用量可查**——每个派工边跑边写 `.lto/<run-id>/live/<job-id>.log`（卡住直接 `tail` 看，不用等 timeout）；token 按 runner 计量（codex/pi/claude 真实，agy 无 CLI 用量诚实标 unmetered）
6. **动态决策支持**——`next`/`autopilot` 整理事实、提供 safe action、必要时升级，最终路径由 host agent 判断
7. **跨 session 回顾**——`recap` 用人话告诉你：做什么/为什么/跑多久/做到哪/还剩啥/现在轮到你/花了多少 token
8. **留痕可审计**——append-only `events.jsonl`（run/phase/task/runner/artifact 起止）+ 派生 `telemetry.json` + `interventions.jsonl`（force、被拦 closeout、已避免的人手清理）；纯传感器、落盘前 redact，是调优证据地基
9. **越用越聪明**——先稳定 `.lto` 协议，再把真实 run 信号喂给 host agent 调优；Go shadow CLI 要等协议稳定
10. **部署 + 收尾**——pre-deploy hook 闸门 + `closeout` 自动生成 CHANGELOG.md

## 快速开始

```bash
L="python3 scripts/lto_run.py"
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
$L closeout --summary "做了什么，验证了什么"   # 默认生成 CHANGELOG.md
$L closeout --summary "行政收尾" --no-changelog  # 已提交后收尾，避免新 tracked dirt
$L self-test                   # LTO 自检
```

## 对抗审计：让不同 AI 帮你审

```bash
# 推荐（自动）：派异构三方（≠你这家）审 + 自动收口判收敛
$L audit --auto-dispatch

# 手工：直接用自带 runner 派工，回复存一个目录再收口
AD="scripts/delegate/runners"  # 本 repo 自带
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
- **autopilot 四档全实现**：`--supervised`（出 brief）、`--auto-exec`（worktree 沙箱跑 safe 子步骤）、`--decide`（escalate 时 opt-in 派三方收敛）、`--autonomous`（机械证据闸门 + 机械执行 safe 子步骤）。autonomous **不 spawn 决策 agent、不替你反思**——LTO 机械产出事实，反思归主 agent；证据不足时诚实退回 supervised，escalate/dangerous/push/网络一律停人类。
- **自动执行有沙箱**：`--auto-exec` 的命令在独立 git worktree 副本和隔离 HOME 中跑，可保护主工作树和常规凭据；这不是 OS chroot，仍靠危险命令拦截、路径校验和人工闸门兜底。
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
