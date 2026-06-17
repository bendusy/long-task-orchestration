# 发版工作流（host SOP）

> 发版是 **host owned** 的确定性流程。不靠临场记忆改哪些文件——按本清单走，`release_preflight.sh` 把人会犯的疏漏挡在脚本里。
> 工具分工：`lto release` 出版本计划（纯计划器，不写文件）；`release_preflight.sh` 做检查闸门；本文档串起完整流程。

## 为什么要这个 SOP

历史踩过的坑（每条都已固化进 preflight）：
- **漏改 VERSION 文件**：只 bump 了 `Cargo.toml`，漏了 `VERSION` → CI `check_docs_consistency` 校验"两者一致"失败 → release 没建成。
- **本地只跑 cargo test 就发**：没跑 `check_docs_consistency.py`（也是 CI 闸门），CI 才暴露问题。
- **GOAL spec 带 yh 字样**：开源 repo 混入私有领域命名族（即使是指令文字也不行）。
- **手动 sed 改版本绕过 `lto release`**：`lto release` 本就知道要改 VERSION，手搓反而漏。

## 版本号怎么定（semver）

- **patch**（x.y.Z）：纯 bug 修复、性能、文档，无新命令/新行为。
- **minor**（x.Y.0）：新增功能/命令，向后兼容（如 v0.6.0 的 L3/L4）。
- **major**（X.0.0）：破坏性变更（删命令、改命令行为、改产物格式不兼容）。
- 不确定就看 `git log <上个tag>..HEAD`：有 `feat:` → 至少 minor；只有 `fix:`/`perf:` → patch。

## 完整步骤

### 1. 写 CHANGELOG（说人话）
在 `CHANGELOG.md` 顶部 `# Changelog` 后加一节 `## vX.Y.Z — <一句话主题>（日期）`，**讲清这版给用户带来什么**，不是机器生成的 run-id 流水账。结构：一句话总览 + 新功能 + 修复与性能 + 架构。每条用用户视角（"派工跑完自动通知"而非 "add agent.turn.completed event"）。

### 2. 同步版本号（三处！）
三处必须一致，否则 CI 挂：
```bash
NEW=X.Y.Z
sed -i '' "s/^version = \".*\"/version = \"$NEW\"/" Cargo.toml   # macOS sed
echo "$NEW" > VERSION
cargo build --release    # 同步 Cargo.lock（去掉 --locked 才会更新 lock）
```

### 3. 跑 preflight（全绿才继续）
```bash
bash scripts/release_preflight.sh --version X.Y.Z
```
检查：版本三处一致 / yh 隐私扫描 / 凭据 / CI 全部红线（fmt+clippy+test+docs_consistency+ownership）/ self-test / 工作树 / 分支安全。**有 FAIL 就停下修，别发。**

### 4. commit + push main
```bash
git add Cargo.toml Cargo.lock VERSION CHANGELOG.md <其他改动>
git commit -m "release: vX.Y.Z — <主题>"
# commit message 不加 Co-Authored-By / AI 署名
git push origin main
```

### 5. 打 tag 触发 CI 构建
```bash
git tag -a vX.Y.Z -m "vX.Y.Z — <主题>"
git push origin vX.Y.Z
```
tag push 触发 `.github/workflows/rust-v2.yml` 的 `release-binaries` job（`if: startsWith(github.ref, 'refs/tags/')`）：构建 3 平台（linux-musl x86_64 / darwin aarch64 / darwin x86_64）→ tar.gz + sha256 → `softprops/action-gh-release` 建 release + 上传 + 自验 checksum。

### 6. 验证 release（不信"触发了"，要看真上传）
```bash
gh run list --limit 3                       # CI 全绿?
gh release view vX.Y.Z --json assets -q '.assets[].name'  # 6 个 asset(3 tar.gz + 3 sha256)?
```
应看到 6 个 assets。`test` job 挂 → `release-binaries` 被 skip → release 不会建（CI 闸门正确）。

## 如果 CI 挂了要重发（移 tag）

CI 在 test 阶段挂 → release **没建成**（被 skip）。修完后移 tag 重触发：
```bash
# 先核实 release 真没建成(动 tag 前必查,别破坏已发布物)
gh release view vX.Y.Z   # 报 "release not found" 才安全移 tag
# 修复 + commit + push main 后:
git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z   # 删旧 tag
git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z     # 重打到新 commit
```
**红线**：若 release 已建成（有 assets），不要随意移 tag——会让已下载的二进制与 tag 对不上。先 `gh release delete` 再重来，或直接发 patch 版。

## 一句话
**写人话 CHANGELOG → 同步三处版本 → `release_preflight.sh` 全绿 → commit/push → tag → 看 6 个 asset 真上传。** 别手搓绕过 preflight，别只跑 cargo test。
