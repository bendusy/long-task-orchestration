# Goal: 独立判断 lto 还能不能再简化（codex 席）

**只审计，不改任何 src 文件。**只写 `./findings-codex-round.md`。

## 你的角色

host（claude）与 pi 已做过两轮精简审计，两人都倾向"保持现状"。**你是独立的第三票**——如果你认为他们收手太早、还有真活可干，就指出来；如果你也认为到头了，明确说到头了。

**不要为了显得有产出而编造重构建议**。lto 已经零死代码、零 `allow(dead_code)`，硬找活干是负价值。但**也不要因为前两人说"保持现状"就附和**——你的价值在于独立判断。

## 已完成的精简（别重复建议）

1. `shell_single_quote` 三份合一 → `src/process.rs:37`（22 处调用）
2. `dispatch-and-wait` 的 77 行业务逻辑从 `cli.rs` 下沉到 `dispatch_goal.rs::cmd_dispatch_and_wait`，CLI 臂现在是 23 行纯转发
3. closeout 里一处冗余 `clone()` + 单元素 `join()`

## 已明确否决的（除非你有强于以下理由的新论据，否则别再提）

| 项 | 否决理由 |
|---|---|
| `enforce_gates` 10 道闸抽 `refuse_gate` helper | 每道闸守不同不变量，文本像但概念独立；条件形态各异（`!force` / `!allow_dirty` / 三条件 / 嵌套）；抽完只能吃掉 emit+bail 两行，反而丢失 `bail!` 的控制流直观性 |
| `truncate` 5 份合并 | 4 份是一行本地展示裁剪，第 5 份加省略号是有意变体 |
| `tail_lines` 2 份合并 | 返回 `String` vs `Vec<String>`，各自服务不同消费者 |
| `absolutize` 3 份 | 两个语义族（cwd vs repo-root）+ 两套错误类型 |
| `now_millis` 3 份 | scheduler 的 `u64` 是配合 `AtomicU64` 的有意约束 |
| 拆分 `ops.rs`(6489行) | 边界不清（cmd_autopilot ↔ auto_exec_tasks ↔ cmd_runner 事件路径交叉），风险大于收益 |
| `with_run(\|ctx\| ...)` 高阶封装 | 会把错误处理与事件顺序藏进闭包 |
| `cli.rs:1856 current_run_id` 合并进 `util::resolve_run_id` | 后者会 validate，合并给 5 个调用点新增校验＝行为变更 |

## 你要做的

在**保留现有功能与架构工作流思路**的前提下（这是硬约束：不许改变 CLI 契约、不许改变 `.lto/` 文件协议、不许动 gate 语义），独立找还有没有值得做的简化。

建议方向（不限于，你自己找更好）：
- `src/commands/ops.rs` 6489 行里，pi 提过一个 `submit_jobs_emitting` 候选（收 `:1367-1410`、`:1465-1505`、`:2133-2175`、`:2185-2225` 四处 runner 提交样板），它判"可做但收益薄、不急"。**你复核这个判断**：四处真的同构吗？抽了调用点会不会真变短变清楚？还是像 pi 说的会带一堆 `Option`？
- `src/cli.rs` 还有没有别的臂在做本该下沉的业务（Start 55行 / Plugin 三处 55/41/35行 / Task 45行）？pi 判 Start 是"合法的输入门"、Plugin 只有 2 处不够门槛——你同意吗？
- 有没有**逻辑上的**冗余（不是文本重复）：多余的中间变量、可以早返回的深嵌套、算了两遍的值、不必要的 clone/to_string？
- 有没有过度设计：只有一个实现的 trait、只被调用一次的抽象层、为假想扩展留的钩子？

## 硬要求

- 每条结论 grep/读码实证，给 `file:line`。凭印象的不写。
- **明确区分"该现在做"和"等第三个用例"**。只有 2 处的倾向不动。
- 给优先级排序。
- **最后必须明确表态**：`还有值得做的` 还是 `已到头，可以停止`。这是 host 决定是否继续迭代的依据，别含糊。

## 完成条件

`./findings-codex-round.md` 存在，含：findings（可以为空）+ 优先级 + 一句明确表态（还有值得做的 / 已到头）。
