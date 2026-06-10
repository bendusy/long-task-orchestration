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
  - **幂等保证**：LTO 可能对同一 run 多次 publish（重跑 closeout）。am 需按
    `sha256` 或 `(project_key, run_id, kind)` 去重，重复写入不产生重复条目
    （返回"已存在/已更新"而非报错或叠加）。
  - **退出码契约**：写入失败（PG 不可达、schema 不符）要用非零退出码 + stderr
    诊断，不要 exit 0 掩盖失败（这是今天另一处 am 反馈的通病：部分错误返回 0）。

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

### 2.4 library 写入校验缺失（写错库）

- **现象**：观察到大量条目 **slug 里标着正确库名（如 `2026-06-09-技术-...`），
  但 PG 的 `library` 字段却是另一个值（`工作`）**——写入时 library 被错误覆盖，
  slug 保留了原始正确库名。导致按 library 过滤的 search 召回失真。
- **对接要求**：`am write` 应校验 `library` 参数与 slug 前缀的库名一致（或至少
  在不一致时 warn），防止批量写入时 library 字段被串味。这是 am 内部数据完整性
  问题，但会直接影响 LTO publish 的条目能否被正确召回。

## 3. LTO 侧承诺（对接契约的 LTO 半边）

- 投影**已脱敏**：`memory export/publish` 出的 records 是 redacted projection，
  本地 `.lto/` 仍是真源（投影里只有 sha256 + summary + 机器字段，不带原始产物
  正文）。am 收到的不含敏感内容。
- 每条 record 带稳定标识：`project_key` + `run_id` + `kind` + `sha256`，
  供 am 去重 / 关联。
- LTO **不依赖 am 才能跑**：core 命令（start/runner/audit/closeout）零 am 依赖，
  publish 是可选的收尾动作。am 不可用时 LTO 照常工作。

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
