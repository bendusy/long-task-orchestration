# LTO ↔ am（animem）对接需求

> 2026-06-10。给 am 项目方：LTO 这侧的记忆对接现状、配对思路，以及对 am 的
> 对接要求。背景是 agent 实测同时跑 LTO + am + hs 时暴露的几处接口摩擦。
> 本文只描述对接契约，不含任一侧的私有数据。

## 0. 两个系统的定位

- **LTO**：长任务 harness，真源是 `.lto/<run-id>/state.json`（本地、git 边界内）。
  它在 run 收尾时可以把一份**脱敏投影**（artifact-memory projection）推给 am，
  作为"这个 run 干了什么"的长期经验。
- **am（animem）**：经验记忆库，真源是 md 文件（`~/animem-md/`）+ PG 元数据。
  CLI `am write/search/get`，原生推荐"CLI 直连 PG，无需常驻服务"。

配对思路：**LTO 产经验，am 存经验**。LTO 不持有长期记忆，只在 closeout 时把
run 的成果投影出去；am 负责索引、召回、跨 run 模式挖掘。两者通过一个明确的
写入契约解耦——LTO 不该知道 am 的 PG schema，am 不该知道 LTO 的 state 布局。

## 1. 当前对接现状（LTO 侧）

- `lto memory export`：打印脱敏投影 JSON（records 数组，每条带 kind/project_key/
  run_id/sha256/summary 等）。纯本地，不依赖 am。
- `lto memory publish`：把投影 POST 到 `${MEMORY_FLOW_URL}/v1/write`
  （`memory_sink.py`），需要一个常驻的 am-server（Axum REST）。

## 2. 暴露的对接摩擦（agent 实测，2026-06-10）

### 2.1 架构断层：legacy REST 已过时，am 原生 CLI 是正解（最关键）

- **定调（用户 2026-06-10 确认）**：memory-flow 那套 REST 服务化（`am-server`
  Axum + `MEMORY_FLOW_URL/v1/write`）**已过时**。am 原生推荐的 **CLI 直连 PG、
  无需常驻服务**才是正确调用方式。问题在于：包括 LTO 在内的现有 agent **还都在
  用旧的 REST 方案**——LTO 的 `memory publish` 仍硬编码走 `MEMORY_FLOW_URL`
  （代码里已标注 `legacy` / `Temporary private REST adapter`，但缺替代路径）。
- **方向（不再是二选一）**：LTO 应新增一条走 **am 原生 CLI** 的 sink，把 legacy
  REST 标为 deprecated（保留一段过渡期，不立即删）。这条路落地依赖 am 提供
  **稳定的写入 CLI 契约**——这是对 am 的核心要求。
- **对 am 的对接要求**：
  - 提供一个**稳定、文档化的写入子命令**，签名建议
    `am write --json < projection.json`（逐条）或 `am ingest --file projection.json`
    （批量 records 数组）。明确：入参 JSON schema、退出码语义、stdout 是否回写
    入库结果（slug/id）。LTO 会用 subprocess 调它，不碰 PG、不依赖常驻服务。
  - **幂等保证（去重键见 §6，不是单一 sha256）**：LTO 可能对同一 run 多次
    publish（重跑 closeout）。am 需按**复合键**去重——不同 record kind 的稳定
    标识不一样（详见 §6 表），统一去重键是
    `(project_key, run_id, kind, task_id?)`，部分 kind 另带 `sha256`/`*_hash` 可
    作内容指纹判"变没变"。重复写入同一键不产生重复条目（返回"已存在/已更新"
    而非报错或叠加）。**注意**：不要假设每条 record 都有 `sha256` 字段——只有
    `lto_artifact_memory` 有，其余 kind 没有。
  - **退出码契约**：写入失败（PG 不可达、schema 不符）要用非零退出码 + stderr
    诊断。**更正（2026-06-10，am 0.6.2 亲验）**：本文初稿说 am「部分错误返回 0」
    是**误报**——实测 am 已是 `exit=1 + stderr 结构化 JSON`，错误信息齐全。
    那条 `exit 0` 反馈出自另一个外部工具，被错安到 am 头上。LTO 侧 sink 直接按
    am 现有的退出码 + stderr 判成败即可，此项 am 零工作。

### 2.2 写入后搜不到（FTS/向量不同步）

- **现象**：`am write` 写了 md + PG 元数据，但 **FTS 索引和向量嵌入不同步**——
  写完 `am search` 搜不到（实测向量队列积压数千条；手动写入的条目尤其只能
  `am get` by-slug，不进 FTS）。对 agent 是"写了等于没写"。
- **对接要求**：`am write` 返回前（或返回里）应能让调用方知道**条目何时可被
  search 召回**。要么写入即同步建 FTS（向量可异步），要么 `am write` 返回一个
  `searchable: bool / pending_index: bool` 字段，让 LTO/agent 知道还没入索引、
  别急着 search。最低限度：文档说明「写入后多久可召回」。

### 2.3 body 不返回

- **现象**：`am get <slug> --json` 只返回元数据，`body` 字段为空（body 在 md
  真源文件里，PG 不存正文）。调用方拿不到经验正文，只能自己去
  `~/animem-md/<file_path>` 读。
- **对接要求**：`am get` 加一个 `--with-body` 选项（或默认带 body），从 md 真源
  读出正文一起返回。否则每个消费方都要自己拼 md 路径、自己读文件，重复且脆弱。

### 2.4 library 与 slug 不一致（已澄清：非"大量"，是有意库迁移）

- **更正（2026-06-10，am 0.6.2 亲验）**：本文初稿说"大量条目 slug 库名 ≠
  library 字段"**不实**——LTO 侧只看到工作库 2144 条就推断"写错库"，是错误
  归因。am 实测仅 **1 条**真不一致；那批 slug=`...-技术-...` 而 library=`工作`
  的条目是**有意的库迁移**（某领域库→工作库），slug 设计上不可变、但 library 迁了，
  属预期行为不是 bug。
- **对接要求（降级）**：`am write/ingest` 在 library≠slug前缀时**加 warn 不加
  reject**（硬校验会和库迁移现实冲突——am 的判断，LTO 接受）。LTO publish 的
  条目 library 由 LTO 投影显式指定（见拍板项），不依赖 slug 反推，所以这条对
  LTO↔am 对接实际无阻碍。

## 3. LTO 侧承诺（对接契约的 LTO 半边）

- 投影**已脱敏**：`memory export/publish` 出的 records 是 redacted projection，
  本地 `.lto/` 仍是真源（投影里只有 sha256 + summary + 机器字段，不带原始产物
  正文）。am 收到的不含敏感内容。
- 每条 record 带稳定标识：复合键 `project_key` + `run_id` + `kind`
  （+ `task_id`，task/artifact 级 record 有），供 am 去重 / 关联。内容指纹
  （`state_hash` / `request_hash` / `artifact_hash` / `sha256`）按 kind 不同而
  不同——见 §6 表，**不是每条都有 sha256**。
- LTO **不依赖 am 才能跑**：core 命令（start/runner/audit/closeout）零 am 依赖，
  publish 是可选的收尾动作。am 不可用时 LTO 照常工作。
- **am 缺席时 `.lto/` 是回退记忆层**：没装 am，项目的 `.lto/` 目录就是全部记忆
  ——每个 run 一个子目录（state/handoff/run-state/证据）。`lto runs` 列出本项目
  所有历史 run，agent 进项目先看它。装了 am 后，`.lto/` 仍是真源，am 是它的
  跨项目投影下游。换句话说：**LTO 的本地记忆永远在，am 是可选的长期/跨项目增强**。

## 4. 建议的对接验收（双方各跑一遍）

1. LTO `memory export` 出投影 → am 用对接 CLI/REST 写入 → `am search` 能召回
   （证明 2.1 + 2.2 通）。
2. 同一 run 重复 publish → am 去重不产生重复条目（证明幂等）。
3. `am get` 拿回的条目带 body（证明 2.3 通）。
4. library 与 slug 一致性校验生效（证明 2.4 通）。

## 5. 优先级（LTO 侧视角）

1. **P0**：2.1——am 给出稳定的写入 CLI 契约，LTO 据此加 am-CLI sink 取代 legacy
   REST。方向已定（CLI 是正解、REST 过时），只等 am 的 CLI 签名/幂等/退出码契约
   落地。这条不定，LTO 就只能继续挂在过时的 REST 上。
2. **P1**：2.2 写入后可召回——否则 publish 了也搜不到，等于白做。
3. **P2**：2.3 body 返回、2.4 library 校验——影响消费体验和数据质量，但不阻断
   主链路。

## 6. LTO 投影 schema（am 实现 write CLI 照这个做）

`lto memory export --run-id <id>` 的真实输出结构（脱敏后）。am 的 `am write
--json` / `am ingest` 要能吃这个 JSON。

**顶层信封**：

```json
{
  "kind": "lto_memory_projection",
  "schema_version": 1,
  "project_key": "long-task-orchestration",
  "run_id": "<active run id>",
  "generated_at": "2026-06-10T21:49:48+08:00",
  "repo_path": "[redacted-path]",
  "records": [ ... ]
}
```

**records[] 的 5 种 kind 及其去重键**（这是 §2.1 幂等去重的依据）：

| record kind | 去重键（除 project_key 外） | 内容指纹字段 | 说明 |
|---|---|---|---|
| `project_snapshot` | （仅 project_key，全局唯一一条） | 无 | git head/branch/dirty、active/latest-closed run |
| `lto_run_snapshot` | `run_id` | `state_hash` / `request_hash` / `artifact_hash` | 一个 run 的目标（已脱敏 `goal_redacted`/`why_redacted`/`done_when_redacted`）、phase、task 计数 |
| `lto_task_memory` | `run_id` + `task_id` | 无（按 task 字段比对） | 单个 task：commands_run、blockers、assumptions、depends_on |
| `lto_artifact_memory` | `run_id` + `task_id` + `sha256` | `sha256` | 产物指纹（state.json/handoff 等的 sha256），唯一带 sha256 的 kind |
| `workflow_routing_memory` | `run_id` | 无 | 这个 run 走了哪条 workflow pattern |

**给 am 的去重实现建议**：用 `(project_key, run_id, kind, task_id)` 做唯一约束
（task_id 对 run/project 级 record 为 NULL）。同键再写时，若带内容指纹
（hash/sha256）则比指纹决定 update vs skip，不带指纹的（task/routing）直接 upsert。

**⚠️ slug 反解析歧义（回应 am 方案的 slug 编码）**：am 拟用
`lto-<project_key>-<run_id>-<kind>[-<task_id>]` 把复合键编码进 slug。问题：
**这些字段自身都含 `-`**——`run_id` 形如 `20260604-224630-plugin-boundary-v0-...-9ee3507c`
（10 个 `-`），`project_key`=`long-task-orchestration`（2 个 `-`），`kind`=`lto_run_snapshot`
（含 `_`）。用 `-` 拼接后**无法可逆反解析**回 4 个字段。两条出路（am 选其一）：
- **(A 推荐) slug 当不透明唯一键，不反解析**：去重的复合键从 record JSON 的
  显式字段读（`record["project_key"]`/`["run_id"]`/`["kind"]`/`["task_id"]`），
  slug 只是个稳定唯一字符串。LTO 投影里这 4 个字段都是独立显式字段（见 §6
  样例），不需要从 slug 抠。
- **(B) 要可逆就换不冲突的分隔符**：如 slug 用 `lto/<pk>/<run_id>/<kind>/<task_id>`
  或对各段 base32，但更重也没必要——A 已够。

**脱敏已做（am 不用再脱）**：`repo_path` → `[redacted-path]`，goal/why/done-when
都是 `*_redacted` 字段，原始产物正文不进投影（只留 hash + summary）。

**library / tag 归属（回应 am 拍板项 1）**：LTO 投影**当前不带 `library` 字段**，
但 record 级已有 `tags`。约定：
- LTO 投影进 **技术库**（沿用历史 REST 写入位置，零新库），由 am 在 ingest 时
  按 record kind 默认归技术库——LTO 侧不显式指定 library。
- **强制 `lto` tag**：LTO 在投影每条 record 的 `tags` 里加 `lto`（已有 tags 字段，
  小改动），am 据此可隔离 / 过滤 LTO 运行记录，避免稀释技术库的人工经验召回。
- 若日后 LTO 记录量大到污染技术库召回，再议单独立库——但先 tag 隔离够用。

## 7. 接 sink 的 LTO 侧就绪度 —— ✅ 已落地（am 0.7.0）

am 0.7.0 交付 `am ingest` 后，LTO 侧 `AmCliSink` 已实现并真跑验证。最终契约
与原计划略有不同（用 `am ingest` 而非 `am write` 逐条）：

**真实契约**（`am ingest --help`）：
```
am ingest [-f FILE | stdin] [--json] [--database-url URL]
```
- 输入：LTO `memory export` 产的**整个信封**（JSON，stdin 或 `-f`），无需逐条拆。
- `--json`：业务结果 → stdout，tracing → stderr。
- am 自读 `DATABASE_URL`（env/默认）。**LTO 不传 `--database-url`**，PG 连接串
  永不进 LTO 进程参数/日志/仓库。

**LTO 侧实现**（已 commit）：
1. `AmCliSink(MemorySink)`：`publish()` 把 `build_projection` 信封管道喂
   `am ingest -f - --json`，从 `data.summary.{written,updated,skipped,failed}`
   解析三态；`resume()` 调 `am search <q> --library 技术 --json`。
2. `memory.py` 加 `--sink am-cli|legacy-rest`（默认 **am-cli**）+ `--am-bin`；
   timeout 默认提到 60s（am ingest 连 PG 比 REST 慢，实测 ~15s）。
3. am 缺席（`shutil.which` 找不到）→ `MemorySinkError` 优雅降级，提示
   “local .lto/ remains the source of truth”，publish 不是硬依赖。

**真跑验证**（animem-private 的 J run，10 条投影）：
- 首次手动管道 `export | am ingest`：written:3 / updated:2 / skipped:5 / failed:0
- 再跑 `memory publish --sink am-cli`：written:0 / updated:1 / skipped:9 / failed:0
  —— 幂等收敛正确（updated 那条是 `updated_at` 时间戳刷新导致 state_hash 变）。
- 回归测试：am-cli 缺 binary 报错路径 + legacy-rest 兜底路径都进
  `test_orchestration_cmds.py`，`SELFTEST OK`。

**一个观察**（留给 am）：每次 export 都刷新 `updated_at` → state_hash 变 →
lto_run_snapshot 每次 publish 都 updated 一条。若想更纯的幂等，可考虑投影
排除易变时间戳，或 am 侧 dedup 时忽略 `updated_at`。当前不阻塞，数据零损坏。
