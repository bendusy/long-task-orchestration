# release playbook

> 状态：active/current——LTO 调度先验（playbook 不替 host 选路）。
> 从 `workflow-playbook.md` 切出（2026-07-14）；通用哲学/调度循环/前置闸门见原文件。


> 定版与对外发布。push 永远是人类闸门。

触发信号：

- 版本定版；对外 push；公开仓库同步；向他人交付。

可用 primitive：

- changelog 定版（版本号与条目对应）。
- `bash scripts/privacy_self_check.sh --repo . --strict`（gitleaks 不可用
  时加 `--no-gitleaks` 并在 run state 显式记录降级——dry-run 默认 exit 0，
  不能冒充 strict 通过）。
- 敏感扫描（私有项目名 / 内部路径 / 对话原文）。
- `lto closeout --summary`；push 前 human gate。

期望 artifact：

- 版本号对应的 changelog 段
- 隐私自检输出
- closeout handoff
- push 确认记录

停止条件：

- 隐私自检 strict 通过（或降级被显式记录且人类接受）。
- 人工确认 push。
- 沉淀完成（验收闸门第 5 条在 release 复查）。

反模式：

- push 与沉淀脱节。
- 版本号无 changelog 对应。
- 私有内容混入公开仓（gitignore + 敏感扫描双防线）。
- 用 dry-run 的 exit 0 冒充 strict 通过。

