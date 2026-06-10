# Runner read-only contract — 跨 runner 的 sandbox 兑现统一抽象

> 状态：design spec v3 + **item 0 实测收口（2026-06-10，见 §7）**——三项真实 CLI 探针推翻了 agy 假设：`agy --sandbox`=workspace-write 非 read-only，read-only profile 下 agy 改判 deferred；pi `--tools` 替换语义确认可用；claude enforcer 实为 plan mode 非 allowlist。改码前据 §7 调 §2/§3.2/§3.3。
> （据 R3 复审三家命中的 3 个新 CRITICAL 收口；R2: 3C+8H → R3: 3C+5H，方向已稳）
> v2→v3 收口的三个 CRITICAL（R3 三家命中）：
> - **RC1（三家全员同向）**：`actual_tools` 来源未定义 + 空集绕过。判据 `actual_tools ⊆ allowlist` 缺可操作证据源；runner 没传 `--tools` 时解析成 `[]`，`[]⊆allowlist` 恒真 → 全权限被误判只读。见 §3/§3.1/§3.2。
> - **RC2（agy）**：H1 修复过头——`approved≠read-only 直接跳过判据` 让写型派工失去越权监管（workspace-write 越权成 danger-full-access 漏判）。改为统一偏序 `actual ⊆ approved`，任何等级都查。见 §3.2。
> - **RC3（codex+agy）**：sidecar 固定文件名并发竞态 + crash 残留逃逸。改原子写 + job-id 绑定 + 缺/旧证据 fail-closed。见 §3.1。
> 历史：v2 据 R2（3C+8H）重写；v1 被审出 3C+8H。
> 起因：`deep-agent-profiles` 四家 audit profile dogfooding 时，eval-run 把
> claude/pi/agy 三家全部标 `permission_violation: true`。根因不是误判——是
> 这三家 runner 实际跑在比 profile 声明的 `read-only` 更宽的权限下，fail-closed
> 检测如实暴露了它。

## 0. v2 相对 v1 的核心修正（三家审计逼出）

v1 把两件本质不同的事混成一件，导致判据写反、证据来源缺失。v2 拆开：

- **机制表达**（哪种 CLI 用哪种方式兑现 read-only）—— v1 已对，但判据写成 denylist（错）。
- **enforcement 证据链**（怎么*证明*这次真兑现了，而不是*请求*了）—— v1 完全漏掉，是最深的洞。

v2 的三条铁律（对应三家命中的 3 个 CRITICAL）：

1. **allowlist 不是 denylist**（C1）：read-only 判据是「实际工具/权限 ⊆ 已知只读安全集」，
   未知工具/别名/MCP/可写 Bash 一律 fail-closed。绝不写「含写工具才违规」。
2. **enforcement 必须可观测，不可推断**（C2）：`readonly_enforced` 只能来自 runner
   **实际执行的 argv/权限快照**，不能从 `runner 名 + 环境变量` 推断。runner 脚本
   负责回传它实际传了什么 flag；scheduler 看不到脚本内部条件逻辑，必须靠回传。
3. **env 不继承、显式传递**（C3）：read-only 意图不走会被子进程继承的普通环境变量，
   改为 job 级显式参数；runner 启动时主动隔离，杜绝父进程/前次 run 的污染。

## 1. 这次审出的真问题

`deep-agent-profiles` 声明了 4 家 read-only audit profile（codex/claude/pi/agy），
但只有 **codex** 一家真正兑现了 read-only：

| runner | runner 脚本现状 | 实际权限 | profile 声明 |
|---|---|---|---|
| codex | `codex exec -s read-only` | ✅ 真 read-only sandbox | read-only ✅ 一致 |
| claude | `claude -p --dangerously-skip-permissions` | ❌ 全权限（跳过所有检查） | read-only ❌ 一纸空文 |
| pi | `pi -p --mode json`（无工具限制） | ❌ 全工具（read+bash+edit+write） | read-only ❌ 一纸空文 |
| agy | `agy --dangerously-skip-permissions` | ❌ 全权限（跳过所有检查） | read-only ❌ 一纸空文 |

它们能跑出审计结果，纯粹因为审计任务恰好没去写文件——**沙箱契约一个都没真 enforce**。

根因两层：① runner 脚本没传只读约束；② `scheduler._permission_snapshot` 用
codex 专属的「sandbox 三级」模型（`read-only` / `workspace-write` /
`danger-full-access`），对非 codex runner 一律填 `None`，`_sandbox_exceeds("",
"read-only")` 全判违规（fail-closed）。这是正确的安全设计，但暴露了抽象没覆盖三家差异。

## 2. 三种不可通约的 read-only 兑现机制（实查 CLI 得出）

| runner | read-only 兑现命令 | 语义模型 | 实据 / 待验 |
|---|---|---|---|
| **codex** | `codex exec -s read-only` | **sandbox 三级**：read-only / workspace-write / danger-full-access | `codex.sh` `CODEX_SANDBOX=read-only` |
| **agy** | ~~`agy --sandbox`~~ | **sandbox 开关**：开 / 不开，无分级 | ❌ **实测推翻（见 §7）**：`agy --sandbox` 兑现的是 **workspace-write**，不是 read-only——工作区内 canary 改写/新建文件/shell 重定向全成功，只封工作区外（`/tmp` `operation not permitted`）。read-only profile 下 **agy 标 deferred** |
| **claude** | `claude -p --allowedTools <只读集> --permission-mode plan` | **工具白名单 + plan mode** | ⚠️ **实测部分修正（见 §7）**：`--allowedTools` **不硬裁工具集**（Write/Bash/Edit/Agent 仍在工具列表），真正拦住写的是 `--permission-mode plan` 的软约束 + 模型自觉。非交互不挂起（44s RC=0）、未偷开子代理/Web ✅，但 enforcer 是 plan mode 不是 allowlist |
| **pi** | `pi -p --tools read,grep,find,ls` | **工具白名单（替换语义）** | ✅ **实测确认（见 §7）**：`--tools` 是**完整替换**——工具列表只剩 read/grep/find/ls，write/edit/bash 全 NO-SUCH-TOOL，canary 未改。pi runner 落地清单 item 3 可用，**不必 deferred** |

关键结论：read-only 不是统一概念。把它表达成单个 `sandbox: <rank>` 字符串是
codex-centric 的错误抽象。permission snapshot 必须记录「用什么机制 + 实际传了什么 + 是否可观测地兑现」。

### 2.1 工具分类映射表（C1/H2 — v1 缺失，落地前置）

`_sandbox_exceeds` 判 tool-allowlist 必须有**显式只读安全工具 allowlist**，按 runner 分列。
不在 allowlist 的工具（含未知/别名/MCP/可写 Bash）一律视为越读：

| runner | 只读安全 allowlist（精确集，大小写敏感） | 说明 |
|---|---|---|
| claude | `Read`, `Grep`, `Glob`, `WebFetch` | PascalCase；`Bash`/`Write`/`Edit`/`NotebookEdit`/`Task` 不在集内 = 越读 |
| pi | `read`, `grep`, `find`, `ls` | 小写；`bash`/`edit`/`write` 不在集内 = 越读。**`bash` 视为可写工具**（可 `echo>file`），只读任务不得含 |

判据：`actual_tools ⊆ allowlist[runner]` → 满足 read-only；否则违规。**这是子集判定（allowlist），不是黑名单匹配。**

## 3. 统一抽象：enforcement evidence + 等价 read-only 判据

permission snapshot 扩成记录**可观测的兑现证据**：

```jsonc
{
  "runner": "agy",
  "job_id": "eval-agy-...-candidate",     // RC3：绑定具体 job，防并发串证/残留误读
  "readonly_mechanism": "sandbox-flag",   // sandbox-rank | sandbox-flag | tool-allowlist
  "enforced_argv": ["agy", "--sandbox"],  // scheduler 构造侧记录（见 §3.1，RH2）
  "actual_tools": null,                   // RC1：tool-allowlist runner 必填工具集；非 tool 机制为 null
  "readonly_requested": true,             // job 是否要求只读（来自 job 字段，非 env）
  "readonly_enforced": true,              // 由证据推导，缺证据=unknown→违规
  "sandbox": "read-only",                 // 归一化等级（跨 runner 比较用；非 codex 也填）
  "approved_sandbox": "read-only",        // 来自 mount-lock / profile
  "reason": ...,
  "user_approved": ...,
  "env_keys": [...]
}
```

### 3.1 enforcement 证据来源（C2 + RC1 + RC3 + RH2）

`readonly_enforced` **不得从 `runner 名 + 环境变量` 推断**——那会让审计从 fail-closed
退化成信任配置。但证据也不能纯靠 runner 自报（RH2：能客观记录的就别让被测方自证）。
v3 把证据分成两个来源，各记各的可信部分：

**(a) scheduler 构造侧（客观，首选）**：scheduler 是构造 runner 命令行的地方，它
**自己就知道**给这个 job 传了什么 sandbox/工具 flag。`enforced_argv` 和 `actual_tools`
由 scheduler 在 spawn 时直接记录它构造的命令行 + job 字段，**不依赖 runner 回报**。
这是最强证据——被测 runner 无法篡改 scheduler 自己构造的参数记录。

**(b) runner 侧 sidecar（补充，仅记 scheduler 看不到的）**：runner 脚本写
`<reply>.perm.json` 仅用于回传 scheduler 构造侧无法预知的运行时事实（如 CLI 实际
接受/拒绝了某 flag、CLI 版本）。它是补充，不是 `readonly_enforced` 的唯一依据。

**RC1 — `actual_tools` 来源 + 空集 fail-closed**：
- tool-allowlist runner（claude/pi）：`actual_tools` = scheduler 构造命令行时**它自己
  注入的工具集**（§4 job 字段 → 命令行），不是事后解析 argv 猜的。
- **空集即违规（fail-closed）**：若 job 要求 read-only（`readonly_requested=true`）但
  `actual_tools` 为空/null（= scheduler 没注入任何工具限制 = runner 跑全权限），
  **判违规**。绝不允许 `[] ⊆ allowlist` 恒真把「没限制」误判成「只读」。
- 未知工具（不在 §2.1 allowlist）→ 违规。

**RC3 — sidecar 原子写 + job 绑定 + 残留 fail-closed**：
- sidecar 含 `job_id`，scheduler 读时校验 `job_id` 匹配当前 job；不匹配 → 视为无证据。
- 写用 `tmp + 原子 rename`，绝不就地追加；并发各 job 各自唯一路径（含 job_id），不共享固定名。
- runner crash/超时未写 sidecar → 无证据 → `readonly_enforced=unknown` → 违规
  （绝不沿用上一次的残留文件——job_id 校验天然挡住）。

### 3.2 `_sandbox_exceeds` 按机制判（新签名）

v1 的 `_sandbox_exceeds(used, approved) -> bool` 改为
`_sandbox_exceeds(snapshot, approved_sandbox) -> bool`。

**RC2 — 统一偏序，任何等级都查越权，不对写任务免检**：
本函数回答的是「actual 权限是否超出 approved」（越权判定），对**所有** approved 等级生效。
v2 的「approved≠read-only 直接跳过」是错的——那让 workspace-write 越权成
danger-full-access 也漏判。正确判据：**`actual_sandbox` 的实际权限必须 ⊆ `approved_sandbox`**。

按机制求 `actual_sandbox`，再统一比偏序 `rank(actual) > rank(approved) → 违规`：

- **codex**（sandbox-rank）：`actual = enforced_argv 里的 -s 值`。
- **agy**（sandbox-flag）：**实测修正（§7）**——`--sandbox` 在场 → actual=**workspace-write**（非 read-only！工作区内可写，仅封工作区外）；缺席 → actual=danger-full-access。
  于是 approved=read-only 时 agy **无任何档位可兑现**（`--sandbox` 仍允许工作区写）→ read-only profile 下 agy 一律越权 → **标 deferred，validate 阶段拒绝 agy 承接 read-only job**；
  approved=workspace-write 且 runner=agy → `--sandbox` 恰好兑现（§3.3 改判）；approved=danger-full-access 而不开 sandbox → 合规。
- **claude/pi**（tool-allowlist）：`actual_tools ⊆ allowlist[runner]`（§2.1）→ actual=read-only；
  含 allowlist 外工具 → actual 按最宽工具定级（含写工具→danger-full-access）。再比偏序。
- **空集/无证据**：`readonly_requested=true` 但 `actual_tools` 空 → actual=danger-full-access
  （RC1，没限制=最宽）→ 必然越权。`readonly_enforced=unknown`（缺 sidecar 校验）→ 违规。

### 3.3 agy 中间级 + 写边界（H4 / MEDIUM — §7 实测后全面改判）

实测（§7）证明 v3 之前对 agy 的映射写反了：`--sandbox` 不是 read-only 而是 **workspace-write**。
agy 的真实二档是「`--sandbox`=workspace-write」与「不设=danger-full-access」，**没有 read-only 档**。

- agy + approved=`read-only`：**无可兑现档位**——`--sandbox` 仍允许工作区写，达不到 read-only。
  规则：**read-only profile 下 agy runner 标 deferred**，validate 阶段拒绝 agy 承接 read-only job
  （这与 §5 item 0「实测不过即 deferred」一致）。
- agy + approved=`workspace-write`：**恰好可兑现**——开 `--sandbox`，actual=workspace-write ⊆ approved → 合规。
  （这反转了 v3 之前「agy 不支持 workspace-write」的结论。）
- agy + approved=`danger-full-access`：不设 `--sandbox`，actual=danger-full-access ⊆ approved → 合规。
- 写边界实据（§7）：`--sandbox` 下工作区内 canary 改写 / 新建文件 / shell `>` 重定向**全部成功**；
  工作区**外**（`/tmp`、`~`）写被内核级拦截（`operation not permitted`）。即「工作区可写、工作区外只读」。

## 4. 约束传递：job 级显式参数，不走继承环境变量（C3）

v1 用普通环境变量 `LTO_RUNNER_SANDBOX` 是错的——会被子进程/前次 run/用户 shell 继承污染。v2：

- read-only 意图作为 **job spec 的显式字段**（如 `AgentJob.permission_policy.sandbox`），
  由 scheduler 在 spawn runner 时**显式构造命令行/隔离环境**传入，不依赖进程环境继承。
- runner 启动时**主动 `unset` 任何同名残留变量**再按 job 参数重设，杜绝父环境污染。
- 写型派工（approved 非 read-only）：job 字段就是写权限，runner 照常全权限，不受影响——
  零回归靠的是「显式字段隔离」，不是「碰巧没设变量」。
- 空值/未设语义统一在一处（scheduler 构造层）定义，不落到三个 runner 脚本各自实现（避免 pi 指出的不一致）。
- **RH1 — codex 的 `CODEX_SANDBOX` 必须来自 scheduler 构造的隔离 env**：codex.sh 读
  `CODEX_SANDBOX` 不与「不走继承 env」冲突的前提是——该变量由 scheduler 在 spawn 时
  以隔离 env（`env={...}` 显式构造，不继承父进程）注入，而非从用户 shell/父进程读。
  codex 用 env 通道、其他家用命令行 flag，都统一为「scheduler 构造侧显式注入」，无双路径冲突。

## 5. 落地清单（据本 spec v3 改）

| # | 文件 | 改动 |
|---|---|---|
| 0 | ✅ **已实测（2026-06-10，见 §7）** | ① pi `--tools`=**替换**（可用，不 deferred）② agy `--sandbox`=**workspace-write 非 read-only**（read-only profile 下 **agy deferred**）③ claude 非交互不挂起/不偷开，但 enforcer 是 plan mode 非 allowlist。结论已写回 §2/§3.2/§3.3 |
| 1 | `scripts/delegate/runners/agy.sh` | **read-only 时拒绝承接（§7：`--sandbox`=workspace-write 兑现不了 read-only）**；workspace-write 时用 `--sandbox`；full-access 时维持 `--dangerously-skip-permissions`。写 `<reply>.perm.json`（job_id 绑定，原子 rename） |
| 2 | `scripts/delegate/runners/claude.sh` | read-only 时 → `--allowedTools Read,Grep,Glob,WebFetch --permission-mode plan`；写 perm sidecar（同上） |
| 3 | `scripts/delegate/runners/pi.sh` | read-only 时追加 `--tools read,grep,find,ls`（实测为替换语义才用；否则 deferred）；写 perm sidecar |
| 4 | `scripts/delegate/runners/codex.sh` | `CODEX_SANDBOX` 仅从 scheduler 隔离 env 读（RH1）；写 perm sidecar |
| 5 | `scripts/lto/scheduler.py` `_permission_snapshot` + spawn | **construct 侧记录 `enforced_argv`/`actual_tools`**（scheduler 自己构造的，不靠 runner 自报，RH2）；spawn 用隔离 env（不继承父进程）；读 `<reply>.perm.json` 仅作补充 + 校验 `job_id`；缺/不匹配→`unknown` |
| 6 | `scripts/lto/plugin_eval_run.py` `_sandbox_exceeds` | 按 §3.2 **统一偏序 `actual ⊆ approved`**（不对写任务免检，RC2）；tool-allowlist 子集判 + **空集→danger-full-access→违规**（RC1）；job 级显式传 sandbox/tools 字段 |
| 7 | `scripts/lto/plugins.py` validate | **§7 改判**：agy + approved=**read-only** 组合拒绝（agy 无 read-only 档）；agy + workspace-write 现为合法（`--sandbox` 兑现）。validate 阶段拒不可兑现组合 |
| 8 | `AgentJob` 数据模型 | 新增 `permission_policy.sandbox` + 工具集字段（pi M-v2：防 ad-hoc 字典绕类型） |
| 9 | 测试 | 更新 `_permission_snapshot`/`_sandbox_exceeds` 单测 + REG6；新增：空集→违规(RC1)、未知工具→违规、缺/旧 sidecar→违规(RC3)、approved=workspace-write 越权成 full-access→违规(RC2)、approved=write 合法不误判、含 Write 工具集→违规(经 job 字段) |
| 10 | `references/plugin-boundary.md` | 第 8 章每家补兑现机制 + 证据链；L176 `permission_violation` 从「sandbox-rank based」改「mechanism-aware, evidence-backed」 |
| 11 | 调用侧统一收口：`auditors.readonly_intent_policy(runner)` | 审计/评审/judge 等只读意图派工的唯一权限入口：agy → workspace-write + reason（保留异构视角，不 deferred 弃用；越权靠 `--sandbox` 工作区外封锁 + perm sidecar 兜底），其余 runner → read-only。替换 5 个派工点：`decision.py` direction/review 双轨、`commands/audit.py` auto-dispatch/risk-discover、`llm_judge.py` judge |

状态行已与清单对齐：v3 涉及 **9 个代码文件 + 1 个文档**；item 11 为调用侧收口（2026-06-10）。

## 6. 验证（改完后）

- 四家各跑 `lto plugin eval-run`：codex/claude/pi/agy（实测通过的）candidate 都 `permission_violation: false`；**实测不过标 deferred 的 runner 不计入此条**（解 codex+pi M-v2 验收/deferred 冲突）。
- **空集 fail-closed（RC1）**：read-only job 但 scheduler 没注入工具限制 → `actual_tools` 空 → 判违规。
- **allowlist fail-closed**：claude job 注入含 `Write` 的工具集 → 违规；未知工具 `Foo` → 违规。
- **越权偏序（RC2）**：approved=workspace-write 的 job 实际跑成 danger-full-access → 判违规（不再免检）。
- **缺/旧证据 fail-closed（RC3）**：删 perm sidecar / 喂上一个 job 的残留 sidecar（job_id 不匹配）→ `unknown` → 违规。
- **写型派工零回归**：approved=workspace-write 且父 env 预设残留只读变量 → scheduler 隔离 env spawn 后按写权限跑，不被误判。
- **agy+workspace-write**：validate 阶段拒绝（§3.3）。
- **四家 perm.json 字段/类型一致性**（codex M-v2）：schema 校验四家 sidecar 同构。
- `python3 scripts/lto_run.py self-test` + 现有 pytest（含更新后断言）全绿。

## 7. item 0 实测结论（2026-06-10，真实 CLI 黑盒探针）

方法：三家各开独立工作目录 + canary.txt，让 runner 在声明的 read-only flag 下尝试
4 类写通道（工具自报 / 工作区文件改写 / 工作区外 `/tmp` 写 / shell `>` 重定向），
事后查文件系统验证「自报 ≠ 实兑现」。CLI 版本：pi 0.78.1 / agy 1.0.7 / claude 2.1.170。

| # | runner | flag | 工具自报 | 工作区写 | 工作区外写 | 实兑现等级 | 落地裁决 |
|---|---|---|---|---|---|---|---|
| ① | pi | `--tools read,grep,find,ls` | 只剩 read/grep/find/ls，write/edit/bash = NO-SUCH-TOOL | ❌ 无工具可写 | ❌ | **read-only**（替换语义证实） | ✅ 可用，item 3 不 deferred |
| ② | agy | `--sandbox` | 全工具在（write/run_command/subagent…） | ✅ **成功**（canary 改写 + 新建 + `>` 重定向） | ❌ `operation not permitted` | **workspace-write** | ❌ read-only profile 下 **deferred** |
| ③ | claude | `--allowedTools Read,Grep,Glob,WebFetch --permission-mode plan` | **工具未裁**（Write/Bash/Edit/Agent 仍在列） | ❌ plan mode 拒绝 | ❌ | read-only **靠 plan mode 软约束** | ⚠️ 可用但 enforcer≠allowlist，需 plan mode 同开 |

三条关键结论（直接改写设计假设）：

1. **pi `--tools` 是完整替换**，不是追加——这是落地清单 item 3 的放行前提，确认成立。
2. **agy `--sandbox` ≠ read-only**。它兑现 workspace-write（工作区内全可写，仅封工作区外）。
   v3 之前把 `--sandbox` 映射成 read-only 是错的；read-only profile 下 agy **无档可兑现，必须 deferred**。
   反向收益：agy 恰好是 **workspace-write 的天然兑现者**（§3.3 已改判）。
3. **claude `--allowedTools` 不硬裁工具集**。Write/Bash/Edit/Agent/Workflow 在 plan mode 下仍出现在工具列表，
   真正阻止写的是 `--permission-mode plan` + 模型自觉。含义：claude 的 read-only enforcement
   **依赖 plan mode 软约束**，§3.1 的 `actual_tools ⊆ allowlist` 证据链对 claude **不足以单独成立**——
   必须额外记录 `permission-mode=plan` 在 `enforced_argv` 里，缺则 fail-closed。非交互 44s 正常退出、
   未偷开子代理/Web（plan mode 下模型主动拒绝），挂起风险排除。
