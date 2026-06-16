# Goal: Python Fallback 退役 + 技术债清理(交 codex)

> **致 codex**:这是一份 goal 文档,不是逐行补丁。你是这个开源项目的协作维护者,
> 按维护者标准交付:每个改动可被陌生人从 repo + CI 复现,不依赖私有上下文。
>
> **任务全景**(执行顺序见文末图):
> 0. **dogfooding 铁律**(贯穿全程):lto 自己调不通 = lto 有 bug,优先修,不许绕过
> 1. 修 `plugin source-note` parity bug(P0)
> 2. 文档/命令面 drift 修复(P0)
> 3. Python fallback 整套退役(P1,breaking)
> 4. **CLI 命令面精简**(P1,25 命令太多 agent 记不住,设计先行)— 编号 Phase 2.5
> 5. 技术债清理(P2)
>
> **必须用 LTO skill 自管这个长任务**。这是 LTO 自己的仓库——吃自己的狗粮:
> - `lto start --goal ... --why ... --done-when ...` 起 run 锚定状态。
> - 每完成一个 Phase 跑 `lto check`,用 `lto next` 看事实简报决定下一步。
> - 阶段间用 `lto task-add` / `lto phase` 记录推进。
> - **每个 Phase 收口前 `lto audit --run-id <RID> --auto-dispatch --discover-risks`** 派跨模型族异构审计听别的 agent 意见(详见下一节,这是必做)。
> - 全部完成后 `lto closeout` 写 handoff,`lto check --to closed --strict` 必须 PASS。
> - 这不是形式:run-state 是你跨多轮不漂移的真源,审计方(host)会读你的 `.lto/` 验证。

---

## Phase 0(贯穿全程):dogfooding 铁律 —— lto 自己调不通 = lto 有 bug,优先修

你在 **LTO 自己的仓库**上用 **lto 自己**跑这个任务。这是 dogfooding。由此一条铁律:

> **任何 lto 命令(`lto audit --auto-dispatch` / `lto check` / `lto next` / `lto judge` / ...)调用失败、报错、行为不符预期 —— 那不是「环境问题,绕过去」,而是 LTO 本身的 bug,是 dogfooding 暴露出的真缺陷。**

处理方式:
- **不许 workaround**:不许「这个命令不行,我手动跑/换 Python 旧版/跳过这步」绕过去。绕过去 = 把产品 bug 藏起来,开源用户照样会撞。
- **当场记 + 优先修**:`lto audit` 派工失败(runner 派不出去 / healthcheck 全挂误判 / 退出码不对 / findings 解析炸)→ 记进 run-state 的 risk,**作为高优先级 bug 插进当前 Phase 修掉**,修完再继续原任务。
- **失败要追真因**:外层 rc≠runner 真 rc(管道会吞);某 runner 不健康 ≠ LTO 坏(可能那个 runner 本身的事)——区分「LTO 调度逻辑 bug」vs「单个 runner 不可用」。前者必修,后者收缩到存活异构族继续。判据:LTO 该派给存活的别族却整个哑火 = LTO bug;所有族真的都挂了 = 环境问题(记录,人工介入)。
- 这些「自己用自己用出来的 bug」往往是最真实的可用性缺陷,修它们的价值高于按部就班推 Phase。**它们就是技术债的一部分**,发现即收口。

> 为什么单列:host 这次审计就发现过 LTO 派工的真 bug(`audit.py` 单点派工无 failover,codex unhealthy 直接哑火)。如果当时「绕过去手动审」就永远不会暴露。dogfooding 的价值全在「调不通就是 bug,不许绕」。

---

## 必做:每个 Phase 收口前听异构 agent 意见(跨模型族审计)

你是 OpenAI 族,**自己审自己有共同盲区**(同族审计员漏同一类错)。LTO 自带跨族异构审计能力,**每个 Phase 改完、自验绿之后、进下一个 Phase 之前**,必须派一轮异构审计听别的 agent 意见:

```bash
lto audit --run-id <RID> --auto-dispatch --discover-risks
```

- `--auto-dispatch`:自动把审计任务派给**非宿主族**的 runner(你是 codex/OpenAI,它会派 claude/agy/pi 等别的族);默认**禁止同族**(`--allow-same-family` 不要加),这就是跨族保证。
- `--discover-risks`:让审计方对抗性生成风险点,不只是被动看你给的。
- 审计结果(findings)读进来,**HIGH/CRITICAL 必须在进下一 Phase 前消解或显式记录为已知风险**;审计方提的反驳如果成立,采纳并修,别因为是自己写的就维护。
- 派工要点:runner 健康度是(runner, 任务尺寸)的二元函数——审计任务别给太大,一个 Phase 的 diff 范围即可;某个 runner 不健康就收缩到存活的异构族,别塌缩回自己单线自评。

对**关键判断**(尤其 Phase 1 的 parity 对等结论、Phase 2.5 的命令面设计稿、Phase 3 的删除清单完整性),额外用 `lto judge` 做 baseline-vs-candidate 异构判读,或多派一两个不同族审计方交叉验证——这三处错了代价最大,值得多花 token 换确定性。

> 为什么这条是必做不是建议:host 自己审这次交付时,就是靠跨族子代理(rust-reviewer + general-purpose)+ 主 agent 亲验三视角,才揪出 source-note parity bug 和你单测漏掉的副作用对照。单一视角(哪怕测试全绿)证明不了「对等」。

---

## 维护者验收哲学(贯穿所有 Phase)

> 问题不是「在我机器上能跑吗」,而是「陌生人能否从 repo / release / docs / CI 复现同样行为」。

四条硬标准,任何 Phase 不满足即未完成:

1. **Parity 证据 = 全部可观察副作用**,不只是主产出。比对两实现要覆盖:产出文件内容 + 被修改的其他文件 + 退出码 + **不传任何可选 flag 时的默认行为**。(这一条是血的教训,见 Phase 1。)
2. **删除前先证对等**:`classify → Rust parity → move owner → wrapper/docs/tests cleanup → rollback preserved → delete`。顺序不可乱。
3. **不留悬空引用**:删一个文件,所有 import/文档/CI 对它的引用同步清理,grep 验零残留。
4. **红线不弱化**:`cargo fmt --check` / `cargo clippy --locked --all-targets -- -D warnings` / `cargo test --locked --all-targets` 全程绿。禁止用 `|| true` / `#[allow]` / 跳过测试来「修绿」。

---

## Phase 1 — 修 `plugin source-note` parity bug(P0,先做,边界清楚)

### 缺陷(host 亲验坐实)
`--append-manifest` 的默认值与 Python 原实现**相反**,且缺一个 flag:

| | Python(`scripts/lto/commands/plugin.py:258-259`) | Rust(当前) |
|---|---|---|
| `--append-manifest` 默认 | `store_true, default=True` → **默认 true** | **默认 false** |
| `--no-append-manifest` | 有(`store_false`,用来关) | **缺失** |
| 不传 flag | append note 进 `plugin.json` 的 `source_notes` | 不 append |

后果:同一 plugin、都不传 flag,Python 改了 manifest、Rust 没改——行为分叉。这破坏「Rust 对等」前提,是 Phase 3 删 Python 的拦路石。

### 要求
1. `src/cli.rs`:`PluginCommand::SourceNote` 的 `append_manifest` 默认改 `true`,并新增 `--no-append-manifest`(关)。用 clap 的 `#[arg(long, default_value_t = true, action = ArgAction::Set)]` 或一对互斥布尔——以**对齐 Python 语义**为准(默认开,可显式关)。
2. 验证:同一 plugin 副本,**不传任何 append flag**,Python 与 Rust 都跑 `plugin source-note`,断言:
   - 产出 `sources/<id>.json` 逐字段一致(忽略 `captured_at`);
   - `plugin.json` 的 `source_notes` 数组**两边都被 append**(这是之前漏的副作用);
   - rc 一致。
3. 补一条 Rust 测试:断言**默认行为(不传 flag)会 append manifest**,且 `--no-append-manifest` 能关掉。现有 5 个 source_note 测试只测了显式 `--append-manifest`,漏了默认。

### 完成判据
`cargo test --locked` 全绿 + 上述 parity 三项一致 + `references/validation-log.md` 追加这次 parity 修正证据(覆盖 manifest 副作用对照)。

---

## Phase 2 — 文档与命令面 drift 修复(P0,与 Phase 1 可并行)

### 已知 drift(host 盘点)
1. **COMMANDS.md 列 24 命令,Rust 实际 25**(`lto-rs --help` 顶层命令数 = 25)。找出缺的那个补上;同时确认 `eval-run`/`source-note` 两个 plugin 子命令在 COMMANDS.md/README.md/SKILL.md **三处都已记录且一致**(README/COMMANDS 已有,核 SKILL.md)。
2. 交付契约 `references/open-source-delivery-requirements.md` 要求「Keep COMMANDS.md generated or checked against src/cli.rs; command count and flags must not drift」——若没有命令数一致性 gate,**加一个**(可在 `check_docs_consistency.py` 里加断言:COMMANDS.md 的命令数 == `cli.rs` 的 `Commands` enum 变体数)。

### 要求
- 跑 `python3 scripts/check_docs_consistency.py` + `python3 scripts/check_python_rust_ownership.py` 必须绿。
- 新增命令计数 gate(若无),并让它在命令面再 drift 时变红。
- 所有 active docs 提到 plugin 子命令的地方,把 `eval-run`/`source-note` 标对所有者:Phase 3 前是 `python-legacy`,Phase 3 后是 `rust-core`。

### 完成判据
COMMANDS.md 命令数 == Rust 25;三处文档对新命令一致;一致性 gate 全绿且能防回归。

---

## Phase 2.5 — CLI 命令面精简(P1,设计先行)

### 问题(host 实测)
顶层 25 个命令,`--help` 里**全无 short 描述**,agent(和人)记不住、选不对。命令太多本身就是认知负担,且无描述放大了它。这是真实的 UX/可用性债。

### 这是「设计先行」任务,先有设计再动手
合并命令是有架构风险的 breaking change——并错了比不并更糟。**先把命令面重构设计稿写进 run-state / ADR(新命令树 + 每个的 about + alias/deprecation 策略 + 影响的文档清单),再按设计实现。** 设计稿是给你自己定方案、也给 host 留审计痕迹,不是审批门。

### 设计方向(维护者建议,非硬性,你可提更好的)
按**心智模型聚类**,把同族操作收进子命令,减少顶层项。候选分组(25 → ~12-14 顶层):

| 现状(散) | 建议(聚) | 理由 |
|---|---|---|
| `task-add` / `task-update` / `phase` | `task add` / `task update` / `task phase`(或 `phase` 留顶层) | task 生命周期操作归一个 `task` 名词下,符合 `git remote add` 式心智 |
| `parallel` / `pipeline` | `run parallel` / `run pipeline`(或归入 `runner`) | 都是「批量跑 job」的编排原语,同族 |
| `recap` / `next` / `runs` | 评估:`recap`(给人)、`next`(给 agent 事实简报)、`runs`(列表)语义不同,**可能不该合**——保留但补 help 描述 | 别为减数量硬并语义不同的 |
| `collect-agent-run` | `runner collect`(归入 runner) | 它是 runner 结果回收,属 runner 族 |
| `self-test` | 保留顶层(诊断入口,陌生人第一个会跑的) | 高频诊断,别藏深 |

### 硬约束(防止精简变破坏)
1. **保留向后兼容**:被合并的旧命令名**不能直接删**。要么做隐藏 alias(`task-add` → `task add` 转发 + deprecation warning),要么走一个 deprecation release 周期。开源用户的脚本会调老命令名。
2. **不为减数量牺牲语义清晰**:语义确实不同的命令(如 `recap` 给人 / `next` 给 agent)宁可保留 + 补 help,也别强行合成一个带 mode flag 的大命令。
3. **每个命令必须有 short help**:无论合不合,`#[command(about = "...")]` 一句话描述全部补齐——这是最低成本、最高收益的「好记」改进,**即使不合并也要做**。
4. **同步**:COMMANDS.md / SKILL.md / README / `check_docs_consistency.py` 的命令计数 gate 全部跟着新命令面更新;ownership manifest 同步。
5. **不弱化红线**:fmt/clippy/test 全绿。

### 交付顺序
1. 先出**设计稿**(新命令树 + 每个的 about + alias/deprecation 策略 + 影响的文档清单)→ 写进 run-state。
2. 按设计实现 + 补全所有 about 描述 + alias + 文档同步。
3. 验证:`lto --help` 每个命令有描述;老命令名仍可用(带 deprecation 提示);命令计数 gate 绿。

### 完成判据
顶层命令数下降到设计稿目标且每个有 short help;旧命令名向后兼容可用;文档三处同步;gate 全绿。

> 注:此 Phase 与 Phase 3(Python 退役)有交叉——若先做 Python 退役,命令面以 Rust `cli.rs` 为唯一真源精简更干净(不用同步改 Python argparse)。**建议顺序:Phase 1+2 → Phase 3(退役)→ Phase 2.5(精简,此时只剩 Rust 一处命令面)→ Phase 4。** 即精简放在退役之后做,避免在两套命令面上同时改。

---

## Phase 3 — Python Fallback 整套退役(P1)

> 这是 breaking change(`LTO_USE_PYTHON=1` fallback 消失)。前提是 Phase 1+2 的 Rust 对等证据已落盘(C.0 gate),对等没证明前不要删——这是技术前提,不是审批。

退役方案见同目录 `2026-06-16-python-removal-via-rust-port.md` 的 C.0/C.1/C.2/C.3。要点摘录:

### 删除清单(约 90 个 .py)
- 整个 `scripts/lto/`(36 + commands 25)+ `scripts/lto_run.py` + 测 fallback 的 `scripts/test_*.py`。

> ⚠️ **例外(撞车依赖,必读)**:`scripts/lto/events.py` 和 `scripts/lto/telemetry.py` **不能跟着裸删**。
> 亲验坐实:Rust 侧**从未实现** events.jsonl/telemetry.json(`grep events.jsonl|telemetry.json|safe_emit src/*.rs` = 0;无 tracing 依赖),backlog 标的「✅ 已实现」是 Python 假阳性。裸删 = 丢可观测能力(和 source-note/eval-run 同坑)。
> 处理:这两个文件先由 `2026-06-16-goal-observability-rust-implementation.md` 的 O1-3/O3-1 在 Rust 接管,**接管前从本删除清单移出**,标 `removal-candidate(blocked-on: rust-observability)`。它们是 Rust 实现的参考 spec,删前必须镜像其行为契约。

### 必须保留 / Rust 化(红线)
- `scripts/delegate/runners/*.sh` + `healthcheck.sh`:**保留**(Rust scheduler 现役 spawn,不是 Python)。删了 Rust 运行时直接断。
- 测 runner.sh 的逻辑(`test_codex_runner.py` 等):若仍需,**用 Rust 重写**——它们测的是现役共享基础设施。
- 纯文本 gate(`check_docs_consistency.py`、`audit_ledger_check.py` 中不 import `lto` 的部分):评估能否独立存活,否则 Rust 化。**不能让 docs/ownership/隐私 gate 随 Python 一起裸消失留下验证真空。**

### 同步改动(删后零悬空)
1. `scripts/install.sh`:删 `LTO_USE_PYTHON` 分支 + `LTO_RUN_TARGET`。`lto` 只指 `lto-rs`,缺失清晰报错。
2. `references/python-rust-ownership.md` + `.json`:改为「Python removed at <commit>; Rust owns all commands」或退役该 gate(若 gate 本身依赖跑 Python)。
3. `references/open-source-delivery-requirements.md`:把所有「Python fallback must remain tested」改为「Python removed at <commit>, migration note」。`Hard Non-Goals` 的「No hidden Python default」作为历史保留。
4. `tests/python_rust_compat.rs`:**保留 old-run 兼容那半**(legacy fixture `tests/fixtures/legacy-run/state.json` 可被 Rust 读——这是 `.lto` 协议兼容,与 Python 死活无关);删掉「Python 写的 run 可被 Rust 读」那半。
5. README/INSTALL/AGENTS/CLAUDE/COMMANDS/SKILL/onboarding:删所有 Python fallback 安装/使用段。
6. CHANGELOG:记录退役 + 失去了什么本地 gate(若有)+ 迁移说明。
7. 版本 bump **v0.5.0**(MINOR,fallback 本是 documented 能力),同步 `Cargo.toml`/`VERSION`/`CHANGELOG`,version drift gate 会校验。

### C.0 安全删除 gate(进 Phase 3 前逐项记进 closeout)
1. 每个 Python surface 已分类(rust-core/fallback/legacy/removal-candidate)。
2. Rust 已实现对应 CLI/JSON/文件契约,有成功+失败+路径安全+兼容测试。
3. parity 证据落盘;不保留的行为有 explicit retirement decision。
4. wrapper 不再路由 Python;docs 不再教 fallback;CI/tests/gates 不再 import `scripts/lto/`。
5. rollback 已保留(old-run fixture / release note / 前一版本 tag 至少一项)。
6. 删除清单不含 `scripts/delegate/runners/*.sh` 和 `healthcheck.sh`。
7. `LTO_USE_PYTHON=1 lto self-test` 退役后**清晰报错**,不静默不半路 ImportError。

### 完成判据
- `cargo test --locked --all-targets` 全绿(含改造后 python_rust_compat 只测 legacy fixture)。
- `git ls-files '*.py' | wc -l` 降到预期残量,且每个残留文件有保留理由。
- `bash scripts/install.sh && lto self-test && lto plugin source-note ... && lto plugin eval-run ...` 全跑通(证 port 的 legacy 真可用)。
- 隐私扫描 clean;`grep -rn "use-python\|LTO_USE_PYTHON\|lto_run.py" --include=*.md` 无悬空引用。

---

## Phase 4 — 技术债清理(P2,退役后做,维护者卫生)

> backlog.md 已是 deferred **功能项**真源(非 bug)。本 Phase 只清「退役/重写带出的卫生问题」,不碰 backlog 里有意延后的功能。

逐项核查并清理:
1. **死引用扫描**:退役 Python 后,grep 全 repo(docs + Rust 注释 + CI)对 `scripts/lto`/`lto_run`/`--use-python`/已删 test 的引用,零残留。
2. **references/ 体检**:34 个 reference 文档里,哪些只描述 Python 实现/已退役机制?标 historical 或重写或删。重点嫌疑:`plugin-real-eval-runner.md`(若 eval-run 已 Rust 化)、`protocol-and-language-strategy.md`(若仍说 Python primary)、`rust-migration-release.md`。
3. **ownership/gate 收口**:Python 退役后 `check_python_rust_ownership.py` 的存在意义?要么退役要么改成纯 Rust 命令面自检。
4. **CI 补强**:`rust-v2.yml` 目前不跑任何本地 gate(check_docs/ownership/smoke 都是本地手跑)。退役后把仍有效的 Rust 化 gate **接进 CI**,让一致性/命令数检查变成 CI 强制而非全靠人记得手跑。
5. **`DEFERRED_V0` 对齐**:确认 Rust eval-run 的 deferred 概念(`['automatic_promotion']`)与 docs/backlog 一致,别留 Python-only 的 deferred 描述。

### 完成判据
死引用零残留;references/ 每篇要么 active-accurate 要么明确 historical;CI 跑上 Rust 化的一致性 gate;`lto check --to closed --strict` PASS。

---

## 交付顺序

> Phase 编号是逻辑分组,**执行顺序按下图**(精简 CLI 放在 Python 退役之后,避免在 Rust+Python 两套命令面上同时改):

```
Phase 1 (parity bug)  ─┐
                       ├─→ Phase 3 (Python 退役) ─→ Phase 2.5 (CLI 精简) ─→ Phase 4 (技术债) ─→ closeout
Phase 2 (docs drift)  ─┘
```

一路做到底,全部 Phase 已获授权。技术前提(不是审批,是顺序约束):
- Phase 3 删 Python 前,Phase 1+2 的 Rust 对等证据必须先落盘(C.0 gate)——对等没证明就删 = 丢功能。
- Phase 2.5 实现前先出设计稿写进 run-state(留审计痕迹 + 自己定方案),然后照设计实现。

通则:
- Phase 1+2 可并行,做完跑全验证矩阵,产出 `validation-log.md` 证据。
- **每个 Phase 收口前派一轮 `lto audit --auto-dispatch --discover-risks` 跨族异构审计**,HIGH/CRITICAL 消解后才进下一 Phase。
- 每个 Phase 用 `lto task-add`/`lto phase`/`lto check` 记录;全程 `.lto/` run-state 是你的真源。
- 任何一步 parity 对不齐 / 红线变红 / 悬空引用 → 停,记进 run-state 的 risk 并修复,不要绕过或猜。
- **lto 命令调用失败 = LTO bug(dogfooding),当场记 + 优先修,不许 workaround 绕过**(见 Phase 0)。
- release/tag 归 host;commit 你写,做完一路推进到 closeout。

## 给 codex 的最后提醒
- **用 LTO skill 跑这个任务**(它就是干这个的,且这是它自己的仓库)。
- **每个 Phase 收口听异构 agent 意见**:`lto audit --auto-dispatch --discover-risks` 派跨模型族审计,别自己审自己(同族盲区)。审计方反驳成立就采纳修,别护着自己写的。
- **lto 自己调不通就是 lto 的 bug**——dogfooding 暴露的真缺陷,优先修,不许手动绕过(见 Phase 0)。
- 不信「测试绿就对等」——parity 要比全部可观察副作用,尤其不传 flag 的默认。
- commit 你写,但 **release/tag 归 host**——你不要自己打 v0.5.0 tag。
- 边界判断(哪些 reference 该删、某行为保不保留)→ 用维护者标准自己定,把决策依据记进 run-state(留审计痕迹),别凭感觉乱删。
