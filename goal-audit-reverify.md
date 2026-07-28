# Goal: 对抗性审计 commit 59e08ef（closeout 复跑明卷）

**只审计，不实现、不改任何 src 文件。**只写你的 findings 文件。

## 背景

commit `59e08ef feat(closeout): reverify delivery instruments` 给 LTO 加了一个功能：`closeout` 时 host 侧独立复跑 delivery contract 的 `instruments`，rc 非 0 就拒绝 closeout。

设计意图：执行 agent 看得见验收标准（好事），但**最终判定不能由它自签**——host 在自己的上下文里复跑一遍。

## 你的任务

1. `git show 59e08ef` 读全 diff。重点看 `src/commands/closeout.rs` 的 `enforce_gates`、`src/state.rs` 的 `split_instrument`、`src/cli.rs` 的两个新 flag。
2. **对抗性地找问题**——你的价值在于找出 host 亲验漏掉的，不在于确认它没问题。已知 host 已验过的（不用重复）：
   - `run_task_command` 未被误用（grep 0 匹配）
   - 阉割 gate 后测试会变红（反向验证已做）
   - 改动未越界（6 文件全在白名单）
   - `check_python_rust_ownership.py` 的 FAIL 是先于本轮的既有漂移（已在基线复现）
3. 重点怀疑这些方向（不限于）：
   - **绕过路径**：除了 `--force` 和 `--no-reverify`，还有没有别的方式让这个 gate 静默不生效？
   - **误伤**：`.lto/` 下 20+ 个历史 run，有没有哪个的 instruments 会让 closeout 突然被拦？实际去读几个历史 run 的 `state.json` 的 `delivery_contract.instruments` 字段验证，不要推测。
   - **副作用**：复跑的命令是任意 shell。如果某条 instrument 会改工作树（比如跑格式化、生成文件），gate 与后续 dirty-check 的先后关系有没有事故？
   - **错误吞噬**：`unwrap_or_else(|error| (1, ...))` 把执行错误压成 rc=1，会不会把"命令不存在"和"测试失败"混为一谈，让 host 误判？
   - **事件泄漏**：`emit_closeout_gate_blocked` 的 fields 里放了什么？会不会把不该进事件日志的东西写进去？
   - **超时语义**：`reverify_timeout` 是每条还是总共？多条 instrument 时总耗时上界是多少？会不会让 closeout 挂很久？
4. findings 写进 `./findings-audit-pi.md`。每条给：严重度（CRITICAL/HIGH/MEDIUM/LOW）、file:line、**可复现的触发条件**、建议修法。

## 硬要求

- **每条 finding 必须有 file:line 实证**，凭印象的不要写。
- **明确区分"真 bug"和"设计取舍"**。你觉得设计不好但它是有意为之的，标 MEDIUM 以下并说明。
- 找不到 CRITICAL/HIGH 就如实说没有——**不要为了凑数编 finding**。凑数的结论算作弊。
- 不改 src、不跑 `cargo build`（只读 + `cargo test` 可以）。

## 完成条件

`./findings-audit-pi.md` 存在，含 findings 列表（可以为空但要写明"无 HIGH+"）+ 一句总体判断（可 closeout / 需先修）。
