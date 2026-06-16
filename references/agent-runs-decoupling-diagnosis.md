# 诊断：delegate 派工与 agent_runs 脱钩（已采纳方案 A，2026-06-10 实现）

> **状态更新（2026-06-16）**：用户拍板方案 A。`lto collect-agent-run` 已由 Rust CLI
> 接管，把 delegate.sh 派工的 reply +
> `.meta.json` sidecar 收集成 AgentResult 追加进 `agent_runs[task_id]`，recap /
> token_rollup / cross-run-mining 都能看见。下文保留原始诊断与权衡为决策记录。
> 容错提示（next/recap 扫孤儿 sidecar 自动提示 collect）暂未做，留待按需补。


> 2026-06-10。来源：pi agent 实测 LTO 跑研究型任务的反馈（"用 delegate.sh
> 成功派了 codex 和 pi，但 LTO 完全不知道——agent_runs 为空、recap 不显示
> token、sidecar 落盘了但 LTO 不读"）。本文只诊断 + 给方案权衡，**不实现**——
> 这条触及 LTO 的核心边界（runner 接不接 agent），三方审反复辩过，需用户拍板。

## 1. 现象

agent 用 `scripts/delegate/delegate.sh -a codex -p prompt.md -o reply.md` 派工，
成功拿到 reply，但：

- `state.json` 的 `agent_runs` 是 `{}`
- `handoff.md` 显示 `token_usage: no agent runs`
- `recap` 不显示 token 用量
- delegate 产出的 token sidecar（`reply.md.meta.json`）落了盘，LTO 不读它

## 2. 根因（已核实代码）

LTO 有**两条平行的派工路径**，只有一条写 `agent_runs`：

| 路径 | 入口 | 写 agent_runs? | 证据 |
|---|---|---|---|
| Python 调度 | `agent_exec.spawn_agents(persist=True)` | ✅ 写 | `agent_exec.py:130-133`：`state["agent_runs"][job_id].append(result.to_dict())` |
| shell 派工 | `scripts/delegate/delegate.sh` → `runners/*.sh` | ❌ 不写 | delegate.sh 直接 exec runner 脚本，产出 reply + `.meta.json` sidecar，**完全不经过 agent_exec / state.py** |

`audit --auto-dispatch` 和 `--discover-risks` 走的是 Python 路径（agent_exec），
所以它们派的 agent **会**进 agent_runs。但 agent 手动调 `delegate.sh` 走的是
shell 路径，产物落在文件系统（reply + sidecar），LTO 的 state 层对此一无所知。

这不是 bug，是**两条路径从未打通**：delegate.sh 是给 host agent「手动 fan-out」
用的轻量工具，agent_exec 是 LTO 内部调度原语，两者各写各的，中间没有桥。

## 3. 为什么不能简单"让 runner 接 agent"

pi 的建议之一是"让 runner 支持 `--agent codex --prompt-file`"。但这撞 LTO 的
核心设计边界——`runner` 的职责是**执行一条 shell 命令 + 记录 exit code +
落 evidence**，它刻意**不** spawn agent。这条边界在 2026-06-03/04 的三方异构
审里反复辩过（见 CLAUDE.md changelog「autopilot 受约束自动推进」段、
`workflow-playbook.md` 的分层表）：

- runner 接 agent 会让「执行可验证命令」和「派不确定的 LLM」两种语义混进一个
  命令，破坏 evidence 合同（runner 的 evidence 是 rc + stdout，agent 的产出是
  reply + token，两者不可通约）。
- agent fan-out 已有专门入口（`audit --auto-dispatch`），它走 agent_exec、带
  scheduler 的并发/退避/healthcheck/权限快照——这些 delegate.sh 都没有。

所以方向不是「runner 接 agent」，而是「把 shell 派工的产物**收集**进 state」。

## 4. 两个候选方案（权衡）

### 方案 A：`lto collect-agent-run` 轻量桥（最小侵入）

加一个命令，把 delegate.sh 产出的 reply + `.meta.json` sidecar 合并进
`state["agent_runs"]`：

```
lto collect-agent-run --task-id T1 --runner codex \
    --reply reply.md --meta reply.md.meta.json
```

- **优点**：不碰 runner 语义、不碰 agent_exec、不让 runner 接 agent。纯粹补一个
  「事后登记」入口，和刚加的 `task-update` 同构（都是「把已发生的事实记进
  state」）。工作量小（~一个命令 + 复用 AgentResult schema）。
- **缺点**：要 agent 多走一步（派完工手动 collect）。容错靠人——忘了 collect
  就还是不进 state。sidecar 格式各 runner 不一（pi 有 totalTokens，agy 标
  unmetered），collect 要按 runner 解析。
- **契合度**：高。符合「CLI 是最后的 affordance」——delegate 已是稳定路径，
  补一个最薄的收集命令，不接管路径选择。

### 方案 B：delegate.sh 直接写 state（自动登记）

让 delegate.sh 在产出 reply 后，自己把 run 记进 `state.json` 的 agent_runs。

- **优点**：agent 无感，派完工自动进 state，不会忘。
- **缺点**：delegate.sh 是 shell，要它安全地读改 state.json（并发锁、schema
  校验、原子写）很别扭——这些 state.py 在 Python 侧已处理好，shell 侧重造一遍
  既危险又重复。而且 delegate.sh 设计上是**runtime-agnostic** 的轻量派工器，
  让它依赖 LTO 的 state 布局，把它和 LTO 绑死，违背它"可独立用"的初衷。
- **契合度**：低。把状态写入逻辑下沉到 shell，违背「state 由 Python 层统一管」。

## 5. 倾向（供参考，非决定）

倾向**方案 A**：它和今天刚加的 `task-update` / `phase` 是同一类东西——「把已
发生的事实补登进 state 的薄命令」，不碰核心边界，容错代价（要手动多走一步）
可接受，且可以在 `next` / `recap` 的简报里提示「检测到未登记的 sidecar，跑
collect-agent-run 收进来」来补容错。

但这需要你拍板，因为：

1. 是否值得为 delegate 路径专门补桥？还是引导 agent 改用 `audit
   --auto-dispatch`（已自动进 state）做 fan-out，delegate 只留给真正的临时手动
   派工？
2. 若做方案 A，collect 的 sidecar 解析要覆盖哪些 runner（codex/pi/agy/claude
   sidecar 格式不一）？
3. 容错提示要不要做（next/recap 扫到孤儿 sidecar 就提示 collect）？

## 6. 不做什么（已排除）

- ❌ 让 runner 接 agent（破坏 evidence 合同 + 撞三方审定的边界）
- ❌ delegate.sh 自己写 state（state 写入下沉到 shell，危险且违背分层）
