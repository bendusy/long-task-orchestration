# long-task-orchestration 验证日志

## 2026-05-31：cross-runtime 真执行实测（codex 当宿主）

**目标**：坐实「谁当宿主都能跑这套编排 + 都能正确把另外家族当审计方」。不是问模型「如果当宿主会怎么做」（那只测阅读理解），而是让整条编排链在真机上**真的发生一遍**。

**方法纠正史**（诚实记录，因为走了弯路）：
1. 第一版用 `codex exec`（headless）当宿主 → **错**。headless 一发一收就结束，跑不了「派审计→等回收→读→收敛」多轮循环。宿主必须是交互式常驻会话。
2. 第二版用 triad.sh 让 codex 当审计方之一 → **测偏**。那测的是 codex 作审计方，不是 codex 当交互宿主。
3. 最终版：tmux 起 codex 交互 TUI 当宿主，send-keys 驱动多轮，宿主自己调 triad.sh 派 headless 审计方。**宿主交互、审计方 headless**——这才对。

### 已铁证坐实

| 测试 | 命令/动作 | 期望 | 结果 |
|---|---|---|---|
| C1 加载 | `tmux new-session "codex"` 进 TUI | codex 能看到 skill | ✓ TUI 画面 `Codex can still see every skill`（skill 描述被压进 2% 预算但全可见） |
| C2 交互宿主多轮 | send-keys 发指令 + 单独 Enter | TUI 真响应推理 | ✓ codex 答「宿主自己审自己共享同一套缺陷…无法提供独立失效面」(confidence: high) |
| C3 自主排除自己（核心考点） | 让 codex 当宿主启动异构审计 | 它该排除自己 | ✓ codex 原话「关键约束是宿主为 codex，所以委派列表只包含非宿主审计方」——**无人提示，自主推理** |
| C4 宿主真调 triad 派工 | codex 在 TUI 执行 triad.sh | 闭环真发生 | ✓ `Ran bash .../triad.sh ... -a "claude pi agy"` |
| C5 审计方真审（agy） | agy headless 审 16KB example spec | 真评审非空转 | ✓ 4954 字节，抓出「存量记录空转」「拆事务破坏一致性」「TOCTOU」，全带 (confidence: high)，守反迎合硬约束 |
| C6 审计方真审（pi） | pi/deepseek headless 审 16KB example spec（timeout 给足 600s） | 真评审非空转 | ✓ 3886 字节，最强反驳「新路径只修有伴生对象的行，存量无对象的行全死」，带 confidence，守反迎合 |

**单轮结论**：codex(OpenAI 宿主) + agy(Gemini) + pi(DeepSeek) 跨三个模型家族真审齐活——cross-runtime **单轮**异构编排坐实。

### C7：codex 当宿主跑**完整多轮长任务**（2026-05-31 追加，纠正「之前只测单轮」）

⚠ **诚实纠正**：C1-C6 全是**单轮**（派一份 spec → 回一份评审）。但本 skill 核心是**多轮长任务编排**。用户追问「各家长测真能跑吗」后，才真跑了完整多轮链——之前说「坐实」是过度宣称。

codex 交互 TUI 当宿主，send-keys 驱动，真跑完整长任务链：

| 环节 | 期望 | 真实结果 |
|---|---|---|
| 多轮上下文保持 | 第 N 轮记得第 1 轮 | ✓ 第 2 轮答出第 1 轮设定的「example spec / pi / agy」 |
| 中途审批可处理 | 不卡死 | ✓ codex 误把「记住」当写文件弹审批，esc 取消后继续 |
| 宿主真派 triad + 真等异步回收 | 派工成功，等几分钟 | ✓ `1 background terminal running`，等到 pi(5276字节)/agy(6000字节) 真审完 |
| 宿主读三方 + 收敛判断 | 不投票、亲核 | ✓✓ **超预期**：codex 发现两家对「存量对象」相反定性 → **不数票**，主动去读 spec/implementation/deploy plan 核验，发现代码与 spec 验收口径矛盾，并否决不符合当前部署拓扑的 rolling-deploy blocker。`Worked 6m 27s` |

**结论**：codex 当宿主**完整多轮长任务真跑通**，且自发做到 skill §3 的 B2(不投票)+B4(亲核否决)——我没教它，它读 skill 自己做到的。收敛质量**超过我之前几轮人工审计**（它抓到的代码↔spec 漂移我没抓到）。证据存档 `longtask-codex/codex-host-verdict.txt`。

**沙箱真坑（C7 暴露，比单轮更深）**：codex `exec`/交互默认沙箱挡子 runner 写文件 → 宿主派 triad 必 `pi/agy FAIL`。**codex 当宿主派工的硬前提：`--dangerously-bypass-approvals-and-sandbox`（或 `-s danger-full-access`）放开沙箱**，否则派工必失败。单轮没暴露这个（单轮没真派工到子进程写文件那步）。

### C8：pi 当宿主跑完整多轮长任务（2026-05-31 追加）

pi/deepseek 交互 TUI 当宿主，send-keys 驱动，真跑完整长任务链：

| 环节 | 真实结果 |
|---|---|
| pi TUI 起 + 接 send-keys | ✓ 进交互界面，单轮答出「自审丧失独立性——审计者与被审计者必须异体」 |
| pi 真派 triad（排除自己，派 codex+agy） | ✓ codex(2964字节)/agy(6871字节) 真审落盘；**pi TUI 无沙箱问题**，直接派工成功（与 codex 当宿主需放开沙箱不同——pi 默认就能跑子进程） |
| pi 读两份 + 收敛 | ✓✓ 做了 codex/agy 反驳对比表，**不投票**精准识别 3 个共识 blocker 并交叉引用（存量空转、TOCTOU、计数字段漏更新），给出「不通过，三 blocker 修完可进开发」+ **分层裁决**（区分运维面 vs spec 逻辑面）。token 真实计费 `$0.033` |

**结论**：pi 当宿主完整长任务真跑通，收敛质量与 codex 当宿主相当甚至更结构化（blocker 交叉引用表）。证据存档 `longtask-pi/pi-host-verdict.txt`。**关键差异**：pi TUI 派工**不需要**放开沙箱（codex 需要），各家宿主的沙箱行为不同。

### C9：agy 当宿主跑完整多轮长任务（2026-05-31 追加，三家齐）

agy/Gemini 交互 TUI（`agy -i "初始prompt"`，**不带初始 prompt 会立即退出**）当宿主：
- ✓ 真派 triad（排除自己，派 codex+pi）→ codex(2763)/pi(6137) 真审落盘，**agy TUI 无沙箱问题**直接派工
- ✓✓ 收敛：支持 codex 的「新旧版本结果倒挂」Blocker、深挖「无伴生对象空转」Major、给出 v3 对齐建议（剔除死代码/对齐 WHERE/补全 lifecycle 字段）
- 证据存档 `longtask-agy/agy-host-verdict.txt`

### 「三家各当宿主」最终结论：✅ 3/3 全坐实

| 宿主 | 派的审计方 | 收敛亮点 | 沙箱 | TUI 启动 |
|---|---|---|---|---|
| codex(OpenAI) | pi+agy | 发现代码实现 ↔ spec 验收矛盾，否决不适用的 rolling blocker | **需放开** `--dangerously-bypass-approvals-and-sandbox` | 直接 `codex` |
| pi(DeepSeek) | codex+agy | 3 共识 blocker 交叉引用（`codex#3/agy#1`）+ 运维/逻辑分层 | 默认可派工 | 默认交互 |
| agy(Gemini) | codex+pi | 支持倒挂 blocker + v3 对齐建议 | 默认可派工 | `agy -i "prompt"`（须带初始 prompt） |

**三家当宿主都自发做到 skill §3 的 B2(不投票)+B4(亲核否决)——读 skill 自己做到的，没人教。** 多轮长任务编排在三个模型家族当宿主下全部真跑通。各家 CLI 的沙箱/启动差异已列表，是真实 cross-runtime 工程约束。

### 真实失败（不粉饰）

| 项 | 现象 | 真因（核验后） | 是否阻塞结论 |
|---|---|---|---|
| claude 审计方 | reply 35 字节 = `Not logged in · Please run /login` | claude headless 未登录 | 否（待 `/login`） |
| pi 审计方 | 大 prompt（16KB）下 timeout（exit=124），小 prompt 正常 | **真因：pi/deepseek 审 16KB spec 耗时 ~170-200s，卡在 timeout 边界横跳。** 一手时间戳证据：一次成功跑 `[10:20:36]→[10:23:29] = 173s`（给了 200s 刚够，返回 5914 字节真评审：最强反驳「demotion 是 label flipper 不 gate 任何事」+ BLOCKER，守反迎合带 confidence）；190s/200s 给得不够的几次就 timeout。**不是 model 参数问题、不是非 TTY、不是上游不稳**——纯粹是慢 + timeout 给太短。**正确修法**：triad/delegate 派 pi 审大 spec 时 timeout 给足（≥300s），不是改 model | ✅ 慢但可用，给足 timeout 即出真评审 |
| codex 沙箱挡子 runner | codex exec 内派 pi/agy 报 `EPERM mkdir ~/.pi/agent/...lock` | codex exec 沙箱不许子进程写锁文件；普通 shell 直跑正常 | 否（宿主派工改真 shell 即可，已记入 sharing-guide 坑1） |

### 真执行 vs 纸面测（为什么坚持真执行）

并行跑过一个纸面测（`/tmp/codex-xruntime-test/`：把 skill 全文喂 codex exec，问它三道探针）。对比：
- 纸面测只能证明 codex **会读 §7 表格复述规则**（阅读理解）。
- 真执行证明整条链**真的发生**：codex 真进交互会话、真自主排除自己、真调 triad、agy 真审出真 bug。
- **关键**：纸面测永远测不出「codex 沙箱挡子 runner 写锁」「pi/deepseek 大 spec 审计 timeout 边界」这两个真坑——它们只在真跑时暴露。

### 我自己的教训（照 skill 的镜子）—— 一次糟糕的诊断纪律实录

诊断 pi 这一个问题，我**实打实归因错了三次**，每次都过早下了定论，且其中两次还把错误结论写进了文档（后回退改正）：

| 第几次 | 我的归因 | 我做了什么 | 为什么错 |
|---|---|---|---|
| 1-4 | 沙箱/模型坏/捕获bug/容量 | 逐个排查 | 都是合理排除，这阶段 OK |
| 5 | **上游 deepseek 间歇不稳** | 差点写进定论 | 「外部不稳」是不用查自己的方便结论 |
| 6 | **`--model deepseek-v4-pro` thinking模型空返回** | **改了 pi.sh + 写进 validation-log/sharing-guide「已修复」** | 裸命令通就宣布修复，**没验证 runner 闭环** |
| 7（真因） | **纯慢：审16KB耗时~170-200s，timeout边界横跳** | 一手时间戳 `10:20:36→10:23:29=173s` 锁定 | exit=124 是 timeout 不是空返回，我把两者混了 |

**三个具体失败模式（比结论更值钱）**：
1. **把 exit=124(timeout) 和 exit=0(空返回) 混为一谈**，导致归因方向全偏。退出码是最硬的一手信号，我却没第一时间分清。
2. **「裸命令通」≠「runner 通」就宣布修复**——跳过修复闭环验证，正是 skill §4「核验而非信仰」的现行犯。我自己在同一份 log 上一行写着这个教训，下一步就犯了。
3. **过早写文档**：把未验证的"真因+已修复"写进 validation-log/sharing-guide/pi.sh 注释，让文档撒谎。发现错后全部回退改正。

**最该记的一条**：「间歇性失败 + 退出码不一致」时，**先把每次失败的退出码/耗时列成表**再归因，不要凭单次观察跳结论。真因（纯慢）其实最朴素，我却绕了 6 个弯——因为我没在第一时间把 124 和 0 分开看。

**最讽刺的一条**：`triad.sh` 默认 timeout **就是 900s**，足够 pi 审 16KB（只要 ~200s）。**整场 pi「失败」是我实测时手动传 `-t 190/200/600` 才制造出来的边界问题——用 triad 默认 900s，pi 从头到尾就不会失败。** 我花 7 轮归因去查一个**自己制造的**问题。教训：复现问题前，先确认自己的测试参数没有偏离 runner 的默认调用方式；别用非默认参数测出一个"故障"然后当真。

**最终结论**：pi 不用任何修复（pi.sh 已回退原样），agy 不用修，两家审计方真审大 spec 都出真评审（pi 3886 字节 / agy 4954 字节，皆带最强反驳 + confidence + 守反迎合）。唯一真实的环境缺口是 claude headless 未登录（待 `/login`）。

### 待补验证（下一轮，不阻塞当前结论）

- [ ] claude headless `/login` 后补一轮 → claude 作为审计方也可用
- [ ] 后续大 spec 审计继续使用 `triad.sh` 默认 900s timeout，避免把 pi/deepseek 慢审误判为失败

### 诚实的范围声明

本轮已真跑 codex / pi / agy 三家分别当宿主的完整多轮链路；三家宿主能力均为实测坐实。未坐实的是 claude headless 作为审计方，因为当时返回 `Not logged in · Please run /login`。validation 等级：codex/pi/agy 当宿主 = 实测坐实；claude headless 审计方 = 待登录后补测。
