# Goal: 最终判断——lto 精简还能不能再往前

**只审计，不改任何 src 文件。**只写 `./findings-final.md`。

## 背景

三轮精简已完成并各自 commit：

| commit | 做了什么 | 净行数 |
|---|---|---|
| `3fc0f62` | `dispatch-and-wait` 的 77 行业务从 `cli.rs` 下沉到 `dispatch_goal.rs::cmd_dispatch_and_wait`，CLI 臂变 23 行纯转发 | cli −57 |
| `17a5934` | `cmd_runner`/`cmd_parallel`/`cmd_pipeline` 三份 job-file 生命周期合并成 `run_job_file`；删一处真冗余 clone | **−83** |
| `c6b0fcc` | 补 job-file 生命周期事件测试（你上一轮做的） | 测试 +1 |

更早还做过：`shell_single_quote` 三合一 → `src/process.rs:37`。

**你上一轮的表态是**：「先做 P1 的三条 job-file 窄合并，再做 P2 的冗余 clone 清理；此两项完成后，可停止本轮精简。」——**这两项都已做完**。

## 你要做的

在**保留现有功能与架构工作流思路**的硬约束下（不许改 CLI 契约、不许改 `.lto/` 文件协议、不许动 gate 语义），做最终判断：**还有值得做的，还是已到头？**

### 必须先做的自我复核（重要）

上一轮你报了 7 处「redundant clone，编译器已证」。host 逐处核实后发现**只有 1 处成立**（`ops.rs:3227`，job move 进单元素 Vec），其余 6 处是 clippy 在借用参数上的误报——`options.next_action` / `options.runner` / `sequence` / `fields` 等在后续行还要用，clone 删不掉。

**请你自己复跑 `cargo clippy --locked --all-targets -- -W clippy::redundant_clone` 并逐处核实**，确认 host 的判断对不对。如果你认为 host 错了、某处确实能删，指出来并给出删掉后能编译通过的理由。这一条是校准：**引用 lint 不等于验证 lint**。

### 然后找还有没有真活

建议方向（不限于，你自己找更好）：
- 三轮改完后，有没有**新**出现的重复或死代码？（比如 `run_job_file` 抽出来后，`cli.rs` 或 `ops.rs` 里有没有变成孤儿的 helper）
- `cargo clippy` 默认 lint 之外，试试 `-W clippy::pedantic` 或 `-W clippy::nursery`，看有没有**真正值得修**的（绝大多数 pedantic 建议是噪音，只挑真有价值的报）
- 逻辑冗余：算了两遍的值、可以早返回的深嵌套、多余的中间变量
- 过度设计：只有一个实现的 trait、只被调用一次的抽象层、为假想扩展留的钩子

## 硬要求

- 每条结论 grep/读码/跑命令实证，给 `file:line`。**引用 lint 必须逐处核实 lint 成立**（这是上一轮的教训）。
- 只有 2 处的倾向不动，等第三个用例。
- **允许并鼓励结论是「已到头」**。lto 零死代码、零 `allow(dead_code)`、408 个测试全绿，硬找活干是负价值。
- **最后必须明确表态**：`还有值得做的（列出具体项）` 还是 `已到头，可以停止`。host 靠这个决定是否继续，别含糊。

## 完成条件

`./findings-final.md` 存在，含：
1. 对 host「7 处 clone 只有 1 处成立」判断的复核结论
2. findings（可以为空）+ 优先级
3. 一句明确表态
