# Blocked

## `check_python_rust_ownership.py` 既有 ownership 漂移

- 命令 `python3 scripts/check_python_rust_ownership.py` 连续三次 rc=1，唯一失败皆为 `FAIL Rust top-level help matches ownership manifest`。
- 现场 Rust help 含 `get`、`describe`，`references/python-rust-ownership.json` 的 `top_level_commands` 未列二者。
- 此非本轮所生：`git show HEAD:src/cli.rs` 已在第 44、45 行列 `get`、`describe`；`git show HEAD:references/python-rust-ownership.json` 无二者。
- manifest 不在 goal 白名单，故本轮不可修。后续须另行获准更新 ownership manifest，再复跑该 checker。
