# LTO ↔ yh（yihub）对接需求

> 2026-06-10。给 yihub 项目方：在 LTO harness 里跑办文工作流时，yh 怎么配合。
> 本文只描述对接契约，不含任一侧私有数据。

## 0. 定位：LTO 是无状态 harness，不是 agent

**先把主语摆正**（这决定整篇文档的框法）：

- LTO **不"调"yh**。LTO 是个无状态 harness——它只提供 state / runner / audit /
  evidence 的轨道，自己不主动做任何事、不决策。
- 真正的主语是 **host agent（planner）在 LTO harness 里跑的「办文工作流」**。
  yh cap 是这个工作流里的**执行单元**（一条命令），LTO 是**承载工作流的轨道**。
- 类比今晚的 am 对接：LTO 不"调"am，是工作流在 closeout 时**经过** LTO 把产物
  投影给 am。yh 同理——yh cap 由 `runner` 执行、产物经 LTO 的 state/evidence
  落盘、质量门结果进 audit。LTO 全程无状态、不决策。

所以本文要的不是"LTO 发现并调用 yh 的 29 个 cap"，而是：**办文工作流怎么在
LTO 轨道上编排 yh cap，让长任务可恢复、可审计、产物可追溯。**

## 1. yh 现状（已核实，0.0.0-dev / 2026-06-10）

- **调用入口**：`yh cap <name> [--key value | --key=value] ...`，120s timeout，
  失败 `exit 1 + slog 错误`，`metrics.Record` 记耗时。两个编排快捷入口：
  `yh chengpi`（= `yh cap chengpi-orchestrate`）、`yh official`（= official-orchestrate）。
- **29 个已注册能力**（源码 `RegisteredNames()`）：转换（convert-to-md / doc-format
  / doc-render / doc-template / doc-ingest-vision）、公文生成（make-official-doc /
  make-approval-doc / make-opinion-letter / opinion-redhead/plenary/whitehead /
  gov-template-fill）、质检（gov-doc-verify / govdocx-qc / chengpi-gate /
  chengpi-fact-check / chengpi-final-check / chengpi-render-verify / chengpi-proofread）、
  呈批（chengpi-clone / chengpi-variant / chengpi-gold-select / chengpi-orchestrate）、
  编排（official-orchestrate）、桥接（animem-bridge / mtg-bridge）、其他
  （docx-split-footer / doc-format）。
- yh 给 agent 的现有文档：`docs/agent-quickref.md` / `agent-guide.md` /
  `yh-skill-roadmap.md`。

## 2. 对接缺口（agent 实测 + 本次核实）

### 2.1 无能力发现入口（最关键，P0）

- **现象**：`yh cap list` / `yh cap --help` 都不工作（把 `list`/`--help` 当成
  能力名去 Lookup，报 `capability not found`）。`RegisteredNames()` 存在但只用在
  启动日志。**agent 不知道 yh 有哪些 cap、每个 cap 吃什么参数**——今天 codex/pi
  都反馈"yh 无可发现性"。
- **为什么 LTO 工作流需要它**：host agent 在 LTO 里规划办文工作流时，要能
  机器可读地查"yh 提供哪些能力、各自的 input/output/参数契约"，才能把它们编排
  进 runner 步骤。靠人读 quickref 不可发现、不可机读。
- **对 yh 的要求**：
  - 加 `yh cap list [--json]`：列所有 cap 名 + 一行描述 + 参数 schema
    （input/output/必填项/默认值）。机器可读优先（`--json`）。
  - 每个 cap 支持 `yh cap <name> --help`（或 `--describe`）：单个 cap 的 I/O 契约。

### 2.2 quickref 文档与实际 drift（P1）

- **现象**：`docs/agent-quickref.md` 标题写"18 能力"，源码实际 **29 个**——11 个
  没进 quickref（含 chengpi-orchestrate / official-orchestrate 两个编排入口、
  chengpi-final-check / chengpi-render-verify 等质检）。
- **对 yh 的要求**：cap 清单从 `RegisteredNames()` 单一真源生成（防 drift），
  或加测试断言 quickref 命令数 == 注册数。这同时是 2.1 `yh cap list` 的副产物
  ——有了机读 list，文档可自动生成。

### 2.3 质量门输出格式要可进 LTO audit（P1）

- **背景**：办文工作流的质检 cap（gov-doc-verify / chengpi-gate / govdocx-qc）
  是工作流的**闸门**——它们的结果该进 LTO 的 audit ledger 做收敛判定，而不是只
  打印给人看。LTO audit `--collect` 读结构化 findings（带 `severity` 字段：
  critical/high/medium/low）。
- **对 yh 的要求**：质检类 cap 的 `--json` 输出应包含**结构化 findings 数组**，
  每条带 `severity` + `claim` + `location`（哪个段落/字段）。这样 LTO 工作流
  可以 `yh cap gov-doc-verify --input x --json > findings.json` 然后喂给 audit
  收敛，不达标不让 closeout。（注：chengpi-gate 今天反馈"22 violations 仍 pass"
  ——阈值偏宽是 yh 内部的事，但 severity 字段化能让 LTO 侧自己定收敛标准。）

### 2.4 animem-bridge 与 LTO↔am 桥重叠（需协调，P2）

- **现象**：yh 自带 `animem-bridge`（`--action search/write`）直接读写 am。而
  LTO 也在对接 am（见 `am-integration-requirements.md`，走 `lto memory publish`）。
  **三方各搞一套 am 桥**会导致：同一条经验从 yh 和从 LTO 各写一次、去重键不一、
  library/tag 归属打架。
- **协调建议**（不是谁吃掉谁）：
  - yh 的 `animem-bridge` 管"办文领域经验"（人名热词 / 模板 / 公文规范）的读写
    ——这是 yh 的领域知识，该 yh 管。
  - LTO 的 `memory publish` 管"LTO run 的运行记录投影"（哪个 run 干了什么）。
  - 两者写 am 时**用不同的 tag/kind 区分**（yh 用 `yihub-domain` 类、LTO 用 `lto`
    tag，见 am 文档约定），别互相覆盖。am 侧的去重键能容两条线共存即可。

## 3. LTO 侧承诺（对接契约的 LTO 半边）

- **yh cap 直接当 runner 命令跑**：`lto runner --task-id T1 --command "yh cap
  convert-to-md --input a.docx --output a.md"`——这是条 shell 命令，runner 执行、
  落 exit code + evidence，无需为 yh 造任何新机制。yh 只要保持 `yh cap` 的稳定
  CLI 契约（input/output/exit code/--json），LTO 侧零特殊适配。
- **办文工作流 playbook**（LTO 侧要补的，见 §4）：把"办文长任务怎么在 LTO 轨道
  上编排 yh cap"写成 host agent 的调度先验——不是硬路由，是思考框架。
- **LTO 不持有办文领域知识**：人名 / 模板 / 公文规范都在 yh（和 am 的办文库）。
  LTO 只负责让办文工作流可恢复、可审计、产物可追溯。

## 4. LTO 侧要补的（不依赖 yh，LTO 自己做）

新增 workflow-playbook 的 **`doc-workflow`（办文）节**，写清：
- 触发信号：任务是"把素材办成公文 / 呈批 / 意见函"，要经多步转换 + 质检 + 呈批。
- 可用 primitive：`runner` 跑 yh cap 链（convert-to-md → make-official-doc →
  gov-doc-verify）、质检结果进 `audit --collect`、产物 sha256 进 evidence、
  跨 session 用 `runs`/`resume` 恢复。
- 期望 artifact：每步 yh cap 的输入输出 + exit code、质量门 findings、最终公文
  产物指纹。
- 停止条件：质量门收敛（无 high/critical violation）+ 人工确认终稿（办文是
  对外产出，human gate）。
- 反模式：跳质检直接出稿、把 yh cap 失败的 exit 1 当成功、办文产物不留指纹无法
  追溯改了哪版。

这一节等本文对齐后，我在 LTO 侧实现（纯文档，不依赖 yh 改任何东西）。

## 5. 优先级

1. **P0**：2.1 `yh cap list [--json]` + 单 cap `--describe`——没有能力发现，
   host agent 没法把 yh 编排进 LTO 工作流（只能靠人读 quickref，不可机读）。
2. **P1**：2.2 quickref drift（list 落地后自动解决）、2.3 质检 cap 的结构化
   findings 输出（让质量门进 LTO audit 收敛）。
3. **P2**：2.4 animem-bridge 与 LTO↔am 桥的 tag/kind 协调（三方对齐，避免重复写）。

## 6. 分工

- **yh 侧**：P0 加 `yh cap list --json` + 单 cap describe；P1 质检 cap 结构化
  findings；P2 与 LTO/am 协调 am 写入的 tag/kind。
- **LTO 侧**：补 `doc-workflow` playbook 节（不依赖 yh，等本文对齐后做）；yh cap
  当 runner 命令跑零适配。
- **对齐点**：yh 的 `yh cap list --json` schema（cap 名 + 参数契约）一旦定了，
  LTO 的 doc-workflow playbook 就能引用确切的 cap 清单写编排步骤。
