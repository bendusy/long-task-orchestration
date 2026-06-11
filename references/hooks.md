# LTO 外部边界 Hook

> Phase 3：不可逆操作前的自动闸门。

## pre-commit

commit 前检查，自动安装在 `lto start` 时。

```bash
python3 scripts/lto_run.py hook pre-commit [--force --reason "..."]
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
python3 scripts/lto_run.py hook pre-commit --force --reason "docs-only"
```

## pre-deploy

部署前检查。

```bash
python3 scripts/lto_run.py hook pre-deploy
```

检查项：
- `lto_run.py check --strict` 通过
- 无 unresolved blocks
- phase 不为 closed

## pre-closeout

归档前检查（已有 closeout 命令做硬门禁，此为独立钩子入口）。

```bash
python3 scripts/lto_run.py hook pre-closeout
```

等价于 `lto_run.py check --strict`。

## ANIMEM / memory-flow publish hook 边界

artifact memory publish 是可选增强，不是核心 hook：

- 当前第一片不自动安装 publish hook；
- `lto memory export --dry-run` 纯本地，可随时跑；
- `lto memory publish` 只有用户显式执行才连接 memory-flow/ANIMEM；
- 未来如在 `closeout` / `audit --collect` / `runner` 后触发 publish，失败也只能记录
  warning 或 retry artifact，不能阻断本地 `.lto` 状态写入；
- `lto resume` 和 `lto memory resume` 都必须保持只读，不触发 publish。

publish 默认走 am-cli sink（am 0.7.0+），`MEMORY_FLOW_URL` / `MEMORY_FLOW_TOKEN` 为 legacy-rest 兜底。没装 ANIMEM 或未配置上述环境变量时，LTO hook 仍应正常工作。

## 安装（opt-in，2026-06-03 改）

hook **不默认安装**——`lto start --install-hooks` 才装进 `.git/hooks/pre-commit`：
- 检测到 husky / pre-commit framework / 已有自定义 pre-commit → **跳过并警告**，不覆盖你的设置
- 干净环境 → 创建 LTO pre-commit 闸门
- 已有 LTO 钩子 → 跳过

不传 `--install-hooks` 时 LTO 不碰你的 `.git/hooks`（早期版本盲目追加，会撞 husky，已改）。
