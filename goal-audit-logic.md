# Goal: 审计 lto 的逻辑层简化空间——哪里该抽象，哪里该留着

**只审计，不改任何 src 文件。**只写 `./findings-logic-pi.md`。

## 背景与前情

上一轮你已审过**工具函数重复**，结论是"值得做的极少"，唯一该做的 `shell_single_quote` 三合一已由 host 完成（现在全仓只剩 `src/process.rs:37` 一处定义，22 处调用）。

这轮换靶：**逻辑层**。lto 3.5 万行、零死代码、零 `allow(dead_code)`，所以问题不在"有垃圾"，而在"有没有把同一个概念写了 N 遍"。

host 已机械扫出以下候选（都亲自数过，行号可信）：

### 候选 A：`enforce_gates` 的 10 道闸重复同一模式（最大嫌疑）

`src/commands/closeout.rs:168-387`，219 行，里面**恰好 10 个 `anyhow::bail!` 配 10 个 `emit_closeout_gate_blocked`，一一对应**。

每道闸的形状（去读，别信我复述）：
```
if <条件> && !options.force {
    emit_closeout_gate_blocked(repo, &ctx.run_id, "<原因>", json!({...}));
    anyhow::bail!("closeout refused: ... (use --force to override)");
}
```

**要你回答的**：
1. 这 10 道闸是否真的同构？逐条读，列出哪些完全同构、哪些有实质差异（比如 reverify 那道要跑命令、dirty 那道用 `--allow-dirty` 而非 `--force`、already-closed 那道的文案是 `use --force to rewrite`）。
2. 如果同构的有 N 道，抽成一个 helper（比如 `refuse_gate(repo, run_id, reason, fields, message) -> anyhow::Error`）**值不值**？给出抽象后 10 道闸各自长什么样。
3. **反对意见也要给**：抽象后会不会让"每道闸在做什么"更难读？Rust 里把 `bail!` 包进函数会不会丢掉 `?` 的控制流直观性？

### 候选 B：`run_args` 的 40 个 match 臂里，哪些做了不该做的事

`src/cli.rs:923-1854`，931 行，40 个 `Commands::` 臂。长本身不是问题（分发表本性），问题是**有的臂塞了业务逻辑而非纯转发**。

最肥的几个：
- `Commands::DispatchAndWait` — `cli.rs:1426`，**78 行**
- `Commands::Start` — 55 行
- `Commands::Plugin` — 55/41/35 行（三处）
- `Commands::Task` — 45 行

**要你回答的**：这些臂里哪些是"构造 options 结构体然后调 cmd_xxx"（正常），哪些是"在 cli.rs 里实现了本该在 commands/ 里的逻辑"（该下沉）？给出具体该搬到哪个模块。

### 候选 C：你自己找

其余逻辑重复。建议方向（不限于）：
- `src/commands/ops.rs` 6489 行是最大文件，`phase_report`(940, 175行) / `cmd_llm_judge`(2545, 174行) / `auto_exec_tasks`(2936, 163行) 三个长函数里有没有可提取的重复段
- 多处"读 run → 改 state → save_run"的样板是否已有 helper（先 grep `commands/util.rs` 有没有）
- 事件发射（`events::safe_emit` / `emit_*`）的调用点是否有重复的字段拼装

## 判断标准（重要）

1. **只有 2 处的不要抽**。等第三个用例。
2. **抽象必须让调用点更短更清楚**，否则是负价值。给出抽象前后的对比代码。
3. **别为了消除重复而引入间接层**：Rust 里多一层函数调用没有运行时代价，但有阅读代价。如果抽完要跳转才能理解，就别抽。
4. **区分「机械重复」和「概念重复」**：10 道闸文本像但语义各异（每道守不同的不变量），这种重复可能是**好的**——它让每道闸独立可读可改。这一点你要正面表态。

## 硬要求

- 每条结论 grep/读码实证，给 `file:line`。
- **给优先级排序**：只做一件的话做哪件。
- **允许并鼓励结论是"都不值得做"**。lto 零死代码说明维护得不错，硬找活干是负价值。上一轮你就正确地说了"保持现状是主基调"——这轮如果也是，直说。
- 不要建议"拆分 ops.rs 成多个文件"这种大手术，除非你能说清拆分边界且论证收益大于风险。

## 完成条件

`./findings-logic-pi.md` 存在，含：每个候选的判断 + 理由 + file:line；优先级排序；一句总体结论。
