# LTO 外部边界 Hook

> Phase 3：不可逆操作前的自动闸门。

## pre-commit

commit 前检查，自动安装在 `lto start` 时。

```bash
lto hook pre-commit [--force --reason "..."]
```

检查项：
- state.json 中没有 unresolved blocks → 否则 block
- tests 落后于当前 HEAD 且相关文件变更 → warn
- 无 judge review → warn
- `.lto/` 文件或纯文档 commit → 跳过
- WIP commit → 跳过

环境变量 `LTO_HOOK_MODE`：
- `off`：完全关闭
- `warn`（默认）：只提示不阻塞（除 unresolved blocks）
- `block`：warn 也阻塞

绕过：
```bash
git commit --no-verify
# 或
lto hook pre-commit --force --reason "docs-only"
```

## pre-deploy

部署前检查。

```bash
lto hook pre-deploy
```

检查项：
- `lto check --strict` 通过
- 无 unresolved blocks
- phase 不为 closed

## pre-closeout

归档前检查（已有 closeout 命令做硬门禁，此为独立钩子入口）。

```bash
lto hook pre-closeout
```

等价于 `lto check --strict`。

## ANIMEM / memory-flow publish hook 边界

artifact memory publish 是可选增强，不是核心 hook：

- 当前第一片不自动安装 publish hook；
- `lto memory export --dry-run` 纯本地，可随时跑；
- `lto memory publish` 只有用户显式执行才连接 memory-flow/ANIMEM；
- 未来如在 `closeout` / `audit` / `collect-agent-run` / `runner` 后触发 publish，失败也只能记录
  warning 或 retry artifact，不能阻断本地 `.lto` 状态写入；
- `lto resume` 和 `lto memory resume` 都必须保持只读，不触发 publish。

publish 默认走 am-cli sink（am 0.7.0+），`MEMORY_FLOW_URL` / `MEMORY_FLOW_TOKEN` 为 legacy-rest 兜底。没装 ANIMEM 或未配置上述环境变量时，LTO hook 仍应正常工作。

## 触发方式（opt-in，按需手跑）

hook **不安装、不常驻**——`lto hook <gate> [--force] [--reason]` 在你需要边界检查时手动跑
（gate: pre-commit / pre-deploy / pre-closeout）。CLI 不写入 `.git/hooks`：早期版本 start
命令的 `.git/hooks` 安装器已随 Rust 迁移移除，避免撞 husky /
pre-commit framework / 已有自定义 hook。想在 git 提交前自动触发，自己在仓库 hook 里
调 `lto hook pre-commit` 即可（是否接线由你决定，LTO 不擅自动你的 git）。
