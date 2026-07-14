# Goal: C3 finding 元数据——reported_confidence + invalidated_when（贯通全消费链）

> 致 codex：沿用约束（LTO 自管 / 每 Phase 异构审计 / dogfooding / 红线不弱化 / commit 你写 release 归 host）。
> **这份只做 C3，做完就停，别做 C1/C2/C4**。
> 以 `src/audit.rs` / `src/audit_dispatch.rs` 实证为准，别信历史文档。

## 为什么做（目标 + 第一性）

工程控制论输出模板 2.7 要求每个结论附「置信度 + 何时失效」。LTO 的审计 finding
（`src/audit.rs:15-25`：severity/claim/evidence_to_check/file/source）没有这两样，
导致 host 收到 findings 后无法区分「审计方拍胸脯」和「审计方自己都标 low」——只能
逐条同权核验，浪费收敛轮次。`invalidated_when` 让「结论何时失效」显式化，host 核验
时直接对着失效条件找反证。

**这是 sensors-are-fallible（LTO 原则 5）的 schema 化**：judged metrics 永远不是
ground truth，所以新字段只进 host review 信息层，绝不进 gate。

## ⚠️ 必读：前提（红线）

- **不做数值分数**：现有测试 `judge_verdict_has_no_numeric_score_and_is_isolated`
  （`src/audit.rs:191-199`）断言无数值分数是设计意图。confidence 是三档分类 + 理由文本。
- **字段名就叫 `reported_confidence`**：强调「审计方自报、非校准、非概率」；文档写明
  不得当 severity 或概率用。
- **不进 promote/gate/排序**：改变 reported_confidence/invalidated_when 只能改变
  review payload / host brief 渲染，不得改变 direction/status/pick/gate verdict。
- **隐私**：事件层只记 level / 字段 presence / hash，不写原始 rationale 与 invalidated
  文本（`events.jsonl` 纪律，见 `src/redact.rs` 与 `event_emit.rs` 现有模式）。
- **兼容**：两字段 `#[serde(default)]` optional-load；旧 artifact/state 可读
  （`tests/fixtures/legacy-run/` 回归）；新 dispatch schema 可要求新审计回复提供。

## 核心架构裁决

```json
{
  "severity": "high",
  "claim": "...",
  "reported_confidence": { "level": "high|medium|low", "rationale": "为什么这样判断" },
  "invalidated_when": "什么证据出现时该 claim 不再成立"
}
```

**全消费链一次改完**（只改 struct 字段会在下列任一层静默丢失——这是本 goal 的核心工程量）：

| # | 消费方 | 落点 | 改法 |
|---|---|---|---|
| 1 | Finding struct | `src/audit.rs:15-25` | 加两个 Option 字段 + serde default |
| 2 | typed parser 白名单 | `src/audit.rs:89-121` | `parse_findings_values` 提取新字段（容错：缺省 None；**兼容简化字符串形态** `"reported_confidence": "high"` → `{level: high, rationale: None}`——异构 agent 常回简化 JSON，不兼容会静默丢失，异构评审 R2-F4）。**容错实现在类型层**：给 `ReportedConfidence` 写 custom `Deserialize`（untagged：对象或纯字符串都接受），而非只在手动 parser 打补丁——否则任何 `#[derive(Deserialize)]` 直接解析 `Finding` 的路径（telemetry/事件分发）遇简化形态会解析失败（异构评审 R3-F3）。**非标字面量安全降级**：`"very high"`/`"extremely confident"` 等幻觉字面量 → level=None + WARN，绝不 Err/panic——fail-closed 流程（check/collect-agent-run）不能被一条自报元数据炸断（异构评审 R5-F3）；加测试：非标字面量解析成功且 level 为 None |
| 3 | dispatch JSON schema | `src/audit_dispatch.rs:194-202` | properties 增两字段；required 不加（审计方可缺省） |
| 4 | audit prompt 示例 | `src/cli.rs:2182-2189` | 示例 JSON 带新字段 + 一句「自报置信度与失效条件」 |
| 5 | risk discovery 复制 | `src/cli.rs:1903-1925` | 复制链带上新字段 |
| 6 | decision brief 渲染 | `src/decision.rs:900-917` | 渲染 `[confidence: high — rationale]` 与 `失效条件:` 行 |
| 7 | fallback initializer | `src/decision.rs:559-565` | 非结构化 fallback 补 None 默认 |
| 8 | 事件 | `src/event_emit.rs:270-299` | 增 `confidence_level` + `has_invalidated_when`（bool/hash），不写原文 |

## Phase 划分

### Phase 1：schema + parser + 事件（1-3、8）
- 测试：JSON 带新字段 → 解析出；不带 → None；中文别名不扩展（level 只认 high/medium/low，
  容错小写/首字母大写）；事件断言无 rationale 原文。
- 收口：cargo 全绿 + `lto audit --auto-dispatch`。

### Phase 2：prompt + risk + brief + fallback（4-7）
- 测试：brief 渲染含 confidence 行；risk 链字段不丢；fallback 不 panic。
- **隔离回归测试（核心交付）**：构造两组 findings 仅 confidence/invalidated_when 不同 →
  断言 decision 的 direction/status/pick 与 gate verdict 完全一致（现有
  `judge_verdict_has_no_numeric_score_and_is_isolated` 只查 note 文案，不够——新测试
  证明「不存在数据流把新字段接进决策」）。
- 收口：cargo 全绿。

### Phase 3：文档 + 收尾
- audit-convergence.md 逐 blocker 处置模板增「自报置信度」行；SKILL.md 域Ⅳ卡一句
  （审者自报置信度只是元数据，核验仍逐条）；COMMANDS.md 若输出面变化同步。
- 收口：全套 gate + docs checker + 异构审计 + ledger 收敛 + privacy 自检（动了 events）。

## 复用（勿重写）

- severity 归一化模式 `audit.rs:61-69`（level 解析照此容错风格）。
- 事件 hash/presence 模式 `event_emit.rs:270-299` 现有 `claim` hash 做法。
- schema 分叉纪律：audit 与 risk 路径 severity schema 保持 distinct
  （`audit_dispatch.rs:312-323` 测试），新字段两边都加但别合并 schema。

## 完成判据（可验证）

- 新增 ≥6 测试全绿（解析/缺省/事件隐私/brief 渲染/隔离回归 ×2 组）。
- `grep -n 'reported_confidence' src/ | wc -l` ≥ 8 处（八个消费点全接线）。
- 隔离性：`cargo test --locked isolation` （新测试名含 isolation）证明字段变化不影响
  决策输出。
- legacy fixture 回归：旧 artifacts/state 加载零失败。
- 全套 gate + privacy 自检绿。

## 不可自动化的安全阀

- host 亲验：跑一轮真实 `audit --auto-dispatch`，看新字段从审计方回复到 brief 渲染全链
  可见、事件里无原文。
- 「confidence 不得当 severity 用」写进 audit-convergence.md，由 host 终审文案。
