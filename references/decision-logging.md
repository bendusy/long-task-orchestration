# 决策落盘 — ADR 先行 + artifact memory 复利

> 主文件 §4 的执行细节。第一层先写 repo-local ADR，保证
> codex/pi/agy/claude 换宿主也能接手；有 ANIMEM / memory-flow compatible
> sink 时再把值得复利的经验写入 artifact memory。

## 一、为什么「即时落盘」而非事后补

每个关键决策**拍板后就写**，不等 MVP 完成才补。补的会丢掉当时的判停依据（为什么否决、阈值多少、推翻了什么假设），只剩成功路径流水账——而反例才是最值钱的。

## 二、记什么（重点是反例与天花板）

| 记 | 不记 |
|---|---|
| 为什么判 premature（缺的那个 X） | 「本次开发使用了 X 技术」散文 |
| blocker 递减序列 + 被证伪/否决的 blocker | 「修好了所有问题」笼统 |
| 真数据闸门结果 + 预设阈值（如 0/12 < 15%） | 可从 git 推出的「改了哪个文件」 |
| 对标项目的天花板 = 本项目机会点 | 一次性调试命令 |
| 反直觉的坑 / 自己被纠正了什么 | — |

## 三、observation 标记（别写散文）

正文每行带方括号标记，人和 agent 一眼看出条目性质：
- `[决策]` 拍板的选择 + 理由（含被推翻的备选）
- `[坑]` 反直觉陷阱、踩过的雷
- `[范式]` 可复用的套路
- `[要点]` 关键事实/做法

## 四、两层条目 + slug 外键

一个 MVP 沉淀两层，互相 wikilink 外键：

```
里程碑层 (project 库, type=milestone)
  2026-05-31-example-rollout-state-backfill-kept-audit-trail
  ├─ links: [[previous milestone]] [[backfill decision]]   ← slug 外键互链
  └─ 记: 三断言、状态修复、存量需 backfill、ssh 引号坑

backlog 层 (research-absorbed-backlog 等)
  └─ 记: 还没做的缺口、待验证假设、下一个 MVP 的入口
```

**commit ↔ 决策追踪桥**：commit message 引经验 slug，形成「代码改动 ↔ 为什么这么改」的可追溯链。

## 五、写经验的铁律（适用于任何 memory sink）

- **supersede 而非覆盖**：旧结论被推翻 → 写新条目带 `supersedes=[旧slug]`，不 PATCH 覆盖（保留知识版本）。
- **reinforce 而非复制**：同事实再次确认 → `experience_reinforce`，不新写一条。
- **type 决定衰减**：决策/范式 τ=365d（衰减慢）；坑 τ=30d（过期重验）。选对 type。
- **source_agent 溯源**：写时带宿主 runtime，多客户端共用记忆库时谁写的一目了然。

## 六、LTO ADR helper

没有 artifact-memory sink，或还没指定凭据/库映射时，先用脚本写 ADR：

```bash
python3 scripts/write_decision.py \
  --repo . \
  --run-id <id> \
  --title "why keep wrapper opt in" \
  --slug "keep-wrapper-opt-in" \
  --context "全局命令可能撞用户已有 lto" \
  --decision "只覆盖 sentinel-managed wrapper；非托管同名文件退出 2" \
  --consequences "仓库移动后需要重跑 scripts/install.sh"
```

行为：

- 写 `docs/decisions/YYYY-MM-DD-<slug>.md`。
- 更新 `.lto/<run-id>/state.json` 的 `user_decisions`。
- 在 `.lto/<run-id>/artifacts.json` 登记 `decision_record`。
- `--slug` 只允许小写 ASCII 字母、数字和连字符；中文标题请显式给英文 slug。
- 没有对应 `state.json` 时仍写 ADR 并 warning，不伪装成已登记。

脚本**不直接调用外部 memory sink**。原因是 sink 凭据、库名、MCP/REST 可达性属于另一个集成面；本层只保证 repo-local 可接手。

## 七、降级（无 artifact memory）

没有 ANIMEM / memory-flow compatible sink 时，落盘降级到：
- `docs/decisions/` ADR 文件（一决策一文件，带 status/context/decision/consequences），优先用 `write_decision.py` 生成。
- 项目根 `MEMORY.md` 索引（一行一条 + 链接）。
- **纪律不变**：即时写、记反例与天花板、条目互链、commit 引文件名。丢的只是检索命中率/衰减/复利，不丢纪律。
