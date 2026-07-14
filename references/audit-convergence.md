# 异构审计收敛 — 轮次记账模板

> SKILL.md 异构审计收敛纪律的执行细节。每一轮审计回来后填这张表，blocker 计数单调非增才算在收敛。

## 一、每轮记账表

落地产物：用 `../templates/audit-ledger.md` 写 `.lto/<run-id>/audit-ledger.md`。每轮审计回来先更新 ledger，再决定修、否决、返工或问用户拍板。

| 轮次 | 审计对象 | HIGH | CRITICAL | minor | 本轮处置 | 计数趋势 |
|---|---|---|---|---|---|---|
| R1 | spec v0 | 3 | 5 | … | 修 X/Y/Z | 起点 |
| R2 | spec v1 | 2 | 1+2 | … | 修 …，否决 agy 性能 blocker | ↓ |
| R3 | spec v2 | 1 | 1 | … | 修最后两点 | ↓ |
| R4 | spec v2' | 0 | 0 | 3 | 全 minor 入 backlog | ✅ 收敛 |

**收敛判定**：`HIGH+CRITICAL` 单调非增，末轮 → 0 或仅剩 minor 且无新增 HIGH。

## 二、逐 blocker 分档处置（不投票）

每个 blocker 一行，标收敛形态 + 档位 + 结论：

```
[blocker] <一句话描述>
  收敛形态: 三方一致高置信 / 两方+一方漏 / 三方矛盾 / 单方独占
  档位:     确定 / 需核验 / 裁决 / 否决
  核验:     <亲核了什么源码:行号 / 什么数据>
  结论:     采纳(怎么修) / 否决(证伪依据)
```

**实证记录（W1/W3）**：
- agy「superseded_by 索引劣化」→ 单方独占 → 亲查 schema 无此 index + 生产实测 0.19s → **否决**。
- pi「forced 加 X-lite」→ 单方建议 → 核验 forced 必命中 authoritative，X-lite 退化 no-op 且现状更安全 → **否决**。
- codex「supersedes 未去重 / 未绑实际 experience」→ 两方收敛 → 亲核代码确认 → **采纳**，HashSet 去重 + INSERT…SELECT FROM experience。
- agy「存量无伴生对象降级空转」→ 单方高置信 → 亲核 X-lite 真值表确认 NULL authority 退化 FALSE → **采纳**，降级前兜底 INSERT established。

## 三、反弹处理

- 修 A 又冒出 B（计数反弹）→ **暂停，回退 debug，重审上一轮**。不靠「再硬修一版」推。
- 连续 2 轮不降 → 怀疑审计标准或需求本身，回头质疑前提，别在错误前提上继续修。

## 四、机器判收敛（Rust core 算，不手判）

每轮填完 Round Summary 后调用唯一 evaluator：

```bash
lto check --ledger .lto/<run-id>/audit-ledger.md
lto check --ledger .lto/<run-id>/audit-ledger.md --strict
```

权威实现是 `src/ledger.rs`。输出的硬 verdict 为：无已填轮次 **NO_OBSERVATIONS**；末轮
降到 0 **CONVERGED**；仍在下降但非零 **CONVERGING**；出现上升 **REBOUND**；非零平轮在
`--strict` 下为 **STALLED**。run 模式的反弹/停滞默认 WARN、`--strict` 才 ERROR；高风险
closeout 要求有真实轮次且末轮为 0，除非人显式 `--force` 越过。Closure Gate 手填字段只是
辅助记录，不能替代 Round Summary 的机器判定。

同一解析还输出五个正交 diagnostics：

```text
sample_sufficiency: insufficient | sufficient
terminal:           zero | nonzero
direction:          improving | flat | worsening | mixed
oscillation:        none | single_rebound | alternating
envelope:           shrinking | flat | expanding | unknown
```

diagnostics 只提示 host，不改变 verdict、phase gate、route 或 promote。轮次缺 `auditors` 或
`coverage` lineage 时 confidence 为 `low (no lineage)`；样本足够且出现交替振荡或非收缩包络时，
`lto check` 可展示 delivery contract 的 `forced_entropy` advisory，但仍由 host/human 决定是否换假设。

`scripts/audit_ledger_check.py` 仅为一个版本的兼容薄壳：它解析旧参数后 `exec` 到
`lto check --ledger`，原样继承 Rust 输出和退出码，不含第二份 parser/evaluator。

**审计派工可控优先级**：`lto audit --auto-dispatch --prefer-runner codex --prefer-runner agy`
限定并排序审计 runner 池，把慢的重 thinking runner（pi）挪出收口关键路径。这是 host
可控旋钮，不按历史 telemetry 自动路由。

## 五、预埋待审点（自我证伪）

起草 spec 时，把自己嗅到「可能空转 / 可能接不上」的点，显式写进 spec 的「待审点」清单，让三方**严判**而不是替自己圆场。

实证：W3 spec 第一轮特意让三方严判「方案 Z 的 superseded_by 隐藏会不会让降级空转」+「D0 低频下接通有无真实价值」+「连续第五次 premature 是否成立」。三方 100% 判方案 Z 空转 → 升级 X-lite。
