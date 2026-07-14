# LTO 控制论元结构改造 · 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 `references/specs/2026-07-14-goal-cybernetics-metastructure-redesign.md` 完成 Phase 0（reference truth）→ B 前半（INDEX+切分）→ A（SKILL 四层化）→ B 后半（README）→ 验收审计 → C（四份 core goal 文档）。

**Architecture:** 纯文档/skill 重组 + Python checker 增强，不改 Rust 行为。TDD 体现在：先让 checker 能抓到已知漂移（红），再修文档（绿）。迁移顺序不可倒置：先修真源、后建路由、最后压 SKILL。

**Tech Stack:** Markdown、Python 3（scripts/check_docs_consistency.py）、cargo gates、lto CLI（dogfooding run `20260714-043510-skill-lto-skill-readme-references-core-g-f3c7170b`）。

## Global Constraints

- 工作目录一律 `/Users/ben/Projects/lto-release/long-task-orchestration`（下称 repo 根）。
- 每个 commit 前本地必须绿：`python3 scripts/check_docs_consistency.py` + `git diff --check`；改 Rust 相关文档口径不需要 cargo gate，但**最终验收**（Task 10）跑全套：`cargo fmt --all --check && cargo check --locked --all-targets && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets`。
- commit message 用 `docs:`/`ci:`/`chore:` 前缀简洁中文；**禁止任何 AI 署名/Co-Authored-By/自动元数据**（用户规则覆盖系统默认）。
- SKILL.md frontmatter（name/description/触发条件/allowed-tools/metadata）逐字不动。
- README 里 checker 锚点字符串逐字保留：`二进制下载是 release-gated`、`references/open-source-delivery-requirements.md` 链接、Windows paused、Rust-only 口径。
- `.lto/` 永不提交；本计划产物全部登记进 run 的 task/evidence（`lto task add` + `lto collect-agent-run`/artifact 记录由 host 在 Task 10/16 统一做）。
- 六域名称全仓库统一为：`Ⅰ 接管与恢复`、`Ⅱ 立项与契约`、`Ⅲ 执行与派工`、`Ⅳ 验证与收敛`、`Ⅴ 交付与发布`、`Ⅵ 学习与维护`。

---

### Task 1: checker 增强（先红：抓住已知漂移）

**Files:**
- Modify: `scripts/check_docs_consistency.py`

**Interfaces:**
- Produces: `ACTIVE_DOCS`（active 文档显式清单，list[str]）、`check_relative_links()`、`check_stale_flags()`、`check_no_handwritten_command_count()`——Task 2/3 依赖这些检查转绿。

- [ ] **Step 1: 在 checker 顶部加 active 文档清单常量**

```python
# active 文档：ROUTER 允许落地的当前口径文档。specs/、backlog、validation-log、
# dated review 是历史/设计材料，不进 active 集合。
ACTIVE_DOCS = [
    "SKILL.md", "README.md", "COMMANDS.md", "INSTALL.md", "AGENTS.md", "CLAUDE.md",
    "references/onboarding.md", "references/run-state-workflow.md",
    "references/execution-loop.md", "references/workflow-playbook.md",
    "references/control-loop-harness.md", "references/audit-convergence.md",
    "references/long-loop-state.md", "references/decision-logging.md",
    "references/release-workflow.md", "references/deploy-sequencing.md",
    "references/hooks.md", "references/sharing-guide.md",
    "references/cross-runtime-host-notes.md", "references/hs-as-core-tool.md",
    "references/plugin-boundary.md", "references/rust-migration-release.md",
]
```

- [ ] **Step 2: 加三个新检查函数并在 `main()` 里调用**

```python
STALE_FLAG_DENYLIST = [
    "--request ", "--with-audit", "--profile audit", "--profile deploy",
    "--install-hooks", "--auto-commit",
]

def check_stale_flags(errors: list[str]) -> None:
    for rel in ACTIVE_DOCS:
        hits = contains_any(read(rel), STALE_FLAG_DENYLIST)
        check(not hits, f"{rel} has no stale CLI flags: {hits}", errors)

def check_no_handwritten_command_count(errors: list[str]) -> None:
    # 手写“N 个可见业务命令”必然漂移；总数只允许出现在 COMMANDS.md（由 cli.rs 派生校验）
    pat = re.compile(r"\d+\s*个可见业务命令")
    for rel in ACTIVE_DOCS:
        if rel == "COMMANDS.md":
            continue
        check(not pat.search(read(rel)),
              f"{rel} has no handwritten business-command count", errors)

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)#\s]+)(#[^)\s]*)?\)")

def _anchor_ok(target_text: str, anchor: str) -> bool:
    # GitHub 风格宽松校验：标题去掉非字母数字汉字后与 anchor 同法归一比较
    norm = lambda s: re.sub(r"[^0-9a-zA-Z一-鿿]+", "", s).lower()
    want = norm(anchor.lstrip("#"))
    return any(norm(line.lstrip("#")) == want
               for line in target_text.splitlines() if line.startswith("#"))

def check_relative_links(errors: list[str]) -> None:
    for rel in ACTIVE_DOCS:
        base = (ROOT / rel).parent
        for m in LINK_RE.finditer(read(rel)):
            target, anchor = m.group(1), m.group(2)
            if target.startswith(("http://", "https://", "mailto:")):
                continue
            path = (base / target).resolve()
            check(path.exists(), f"{rel} link target exists: {target}", errors)
            if anchor and path.suffix == ".md" and path.exists():
                check(_anchor_ok(path.read_text(encoding='utf-8'), anchor),
                      f"{rel} anchor resolves: {target}{anchor}", errors)
```

`main()` 里 `if errors:` 之前追加：

```python
    check_stale_flags(errors)
    check_no_handwritten_command_count(errors)
    check_relative_links(errors)
```

- [ ] **Step 3: 跑 checker，验证「红」——必须抓到全部已知漂移**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"`
Expected: FAIL 至少含——
- `references/run-state-workflow.md has no stale CLI flags: ['--request ', '--with-audit', '--profile deploy', '--install-hooks']`
- `references/execution-loop.md has no stale CLI flags: ['--auto-commit']`
- `SKILL.md has no handwritten business-command count`
- `references/onboarding.md has no handwritten business-command count`
- 以及既有的 `Cargo.toml version == VERSION` 失败项（0.9.3 vs 0.9.2）

抓不到任一项 → 修检查逻辑，不许弱化断言。**本 task 不 commit**（保持 CI 绿），与 Task 2 一起提交。

### Task 2: 修四处漂移 + VERSION 基线对齐（转绿并提交）

**Files:**
- Modify: `references/run-state-workflow.md:13-38`
- Modify: `references/execution-loop.md`（`--auto-commit` 全部出现处，`grep -n auto-commit` 定位）
- Modify: `SKILL.md:315`、`references/onboarding.md:100`
- Modify: `VERSION`（或 `Cargo.toml`，按 Step 4 判定）

**Interfaces:**
- Consumes: Task 1 的三个新检查。
- Produces: checker 全绿基线——后续所有 task 的回归底线。

- [ ] **Step 1: 修 run-state-workflow.md 的 start 示例**

`:13-38` 的示例命令改为当前 `Start` 真实参数（真源 `src/cli.rs:55-76`：`--run-id/--goal/--why/--done-when/--host/--target/--constraint/--instrument/--entropy-check/--force`）。示例替换为：

```bash
lto start --goal "做用户登录" \
  --why "降低登录失败率" --done-when "失败率<5%，三端覆盖"
# /goal 型长交付：契约四件套
lto start --goal "提升检索召回" \
  --target "hidden eval recall >= 95%" \
  --constraint "wall clock <= 4h" \
  --instrument "python3 eval/search_recall.py --hidden" \
  --entropy-check "on stall, change hypothesis and log overfit reflection"
```

删除 `--request/--with-audit/--profile/--install-hooks` 的教学段；`audit-ledger.md` 的生成条件改为按当前实现描述（`grep -rn "with_audit\|audit-ledger" src/cli.rs src/commands/ | head` 核实后照实写；若 ledger 现由 `audit` 路径生成就写 audit 路径）。hook 安装指引改链 `references/hooks.md`。

- [ ] **Step 2: 修 execution-loop.md**

`grep -n 'auto-commit' references/execution-loop.md` 逐处删除或改为当前真实做法（runner 不自动 commit；commit 是 host 收口动作）。

- [ ] **Step 3: 删手写命令总数**

- `SKILL.md:315`：`（21 个可见业务命令；…）` → `（业务命令清单见 COMMANDS.md；旧 task/run 入口隐藏兼容）`
- `references/onboarding.md:100`：`## 21 个可见业务命令速查` 整节压成一句：`## 命令速查\n\n命令面以 [COMMANDS.md](../COMMANDS.md) 为准（真源 src/cli.rs）。` 原速查表删除。

- [ ] **Step 4: VERSION 基线判定并对齐**

Run: `git tag -l 'v0.9.*'; grep -n '^## ' CHANGELOG.md | head -5; git log --oneline -3 -- VERSION Cargo.toml`
判定规则：CHANGELOG 已有 `0.9.3` 条目或存在 `v0.9.3` tag → `VERSION` 写 `0.9.3`；否则说明 Cargo.toml 提前 bump，回退 `Cargo.toml` 为 `0.9.2` 并在 run-state 记一条 decision。预期是前者（0.9.3 已发）。

- [ ] **Step 5: 验证转绿**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"`
Expected: `DOCS CONSISTENCY OK`，exit=0

- [ ] **Step 6: Commit（Task 1+2 一起）**

```bash
git add scripts/check_docs_consistency.py references/run-state-workflow.md references/execution-loop.md SKILL.md references/onboarding.md VERSION Cargo.toml
git commit -m "docs+ci: 修 active reference 漂移并让 checker 能抓住它们（stale flags/手写命令数/相对链接/VERSION 基线）"
```

### Task 3: fenced 命令 clap 派生核对

**Files:**
- Modify: `scripts/check_docs_consistency.py`

**Interfaces:**
- Consumes: `ACTIVE_DOCS`。
- Produces: `check_fenced_lto_flags()`——从二进制 `--help` 派生 flag 集合核对文档示例。

- [ ] **Step 1: 实现检查**

```python
import subprocess, shutil, itertools

def _lto_bin() -> list[str] | None:
    rel = ROOT / "target/release/lto-rs"
    if rel.exists():
        return [str(rel)]
    if shutil.which("cargo"):
        return ["cargo", "run", "--quiet", "--"]
    return None

FENCE_RE = re.compile(r"```(?:bash|sh)\n(.*?)```", re.S)
LTO_LINE_RE = re.compile(r"^\s*(?:\$LTO|\$L|lto|cargo run --quiet --)\s+([a-z][a-z0-9-]*)\s*(.*)$")

def check_fenced_lto_flags(errors: list[str]) -> None:
    bin_cmd = _lto_bin()
    if bin_cmd is None:
        print("SKIP fenced-command flag check (no lto binary/cargo)")
        return
    help_cache: dict[str, str] = {}
    for rel in ACTIVE_DOCS:
        for block in FENCE_RE.findall(read(rel)):
            for raw in block.splitlines():
                m = LTO_LINE_RE.match(raw.split("#")[0])
                if not m:
                    continue
                sub, rest = m.group(1), m.group(2)
                if sub not in help_cache:
                    proc = subprocess.run([*bin_cmd, sub, "--help"],
                                          capture_output=True, text=True, cwd=ROOT)
                    help_cache[sub] = proc.stdout + proc.stderr
                for flag in re.findall(r"(--[a-z][a-z0-9-]*)", rest):
                    check(flag in help_cache[sub],
                          f"{rel}: `lto {sub}` supports {flag}", errors)
```

`main()` 里追加 `check_fenced_lto_flags(errors)`。多行续行（`\` 结尾）要先拼接再匹配——实现时把 block 内以 `\` 结尾的行与下一行合并。

- [ ] **Step 2: 自测（红→绿）**

先在任一 active 文档临时插入 ```` ```bash\nlto start --bogus-flag x\n``` ````，跑 checker 预期 FAIL 并指名该文件；删掉后转绿。

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"`
Expected: `DOCS CONSISTENCY OK`（临时行删除后）

- [ ] **Step 3: Commit**

```bash
git add scripts/check_docs_consistency.py
git commit -m "ci: 文档 fenced lto 命令 flag 从二进制 --help 派生核对"
```

### Task 4: `references/INDEX.md`（路由/状态/权威层）

**Files:**
- Create: `references/INDEX.md`

**Interfaces:**
- Produces: 六域→文档映射表（六域详表的**唯一真源**）；文档状态标注；权威级别表。Task 5-9 的切分产物都要回填到这里；SKILL/README 只放压缩映射。

- [ ] **Step 1: 写入四块内容**

```markdown
# references 索引（ROUTER 落地页）

> 六域详表唯一真源。SKILL.md/README 只保留压缩映射，域名集合与此处一致（checker 校验）。

## 1. 六域 → 主 reference

| 域 | 核心动作 | 主 reference | 何时加载 | 不适用 |
|---|---|---|---|---|
| Ⅰ 接管与恢复 | runs/resume/recap/check | onboarding.md、long-loop-state.md | 进入陌生项目 / compact 后恢复 | 新 run 立项（→Ⅱ） |
| Ⅱ 立项与契约 | 适用性判断/start/task/preflight/开发四证据 | run-state-workflow.md | 决定开新 run、写契约 | 已有 active run 的恢复（→Ⅰ） |
| Ⅲ 执行与派工 | runner/dispatch-goal/events/autopilot | execution-loop.md、cross-runtime-host-notes.md | 派外部 agent、等完成事件 | 只读评审派工细节（也在Ⅲ，注意 tmux/headless 边界） |
| Ⅳ 验证与收敛 | audit/judge/check/ledger | audit-convergence.md、playbooks/review.md | 多模型审、判收敛 | 确定性测试（直接跑，不需异构审） |
| Ⅴ 交付与发布 | 部署实测（真实用户路径）/closeout/release | deploy-sequencing.md、release-workflow.md | 上线、发版、收尾 | 未过Ⅳ收敛闸门时 |
| Ⅵ 学习与维护 | decision/memory/telemetry/budget/prune/plugin | decision-logging.md、events-telemetry-contract.md | 拍板落盘、跨 run 挖掘、清理 | 把历史 telemetry 当自动路由依据 |

## 2. 跨域场景 → 加载顺序（默认 1 个主 reference；跨域/安全 ≤2）

| 场景 | 顺序 |
|---|---|
| 接手陌生项目并续跑 | Ⅰ onboarding → Ⅰ long-loop-state |
| 新长交付立项 | Ⅱ run-state-workflow（契约四件套） |
| 派 codex 改代码并等完成 | Ⅲ execution-loop →（跨 runtime 时）cross-runtime-host-notes |
| 方案多模型审到收敛 | Ⅳ audit-convergence → playbooks/review |
| 上线并收尾 | Ⅴ deploy-sequencing → release-workflow |
| autopilot 升档评估 | Ⅲ execution-loop → Ⅵ events-telemetry-contract |

## 3. 文档状态

| 状态 | 含义 | 文件 |
|---|---|---|
| active/current | 当前口径，ROUTER 可落地 | （列 ACTIVE_DOCS 中 references 下全部 + playbooks/*） |
| design/future | 设计目标，未实现，不得当现状引用 | control-loop-roadmap.md、specs/*、backlog.md |
| historical/dated | 历史证据 | validation-log.md、python-rust-ownership.md、dated reviews |

## 4. 权威级别（冲突时高层胜出，文档冲突判漂移不做兼容解释）

| 级别 | 载体 |
|---|---|
| 1 runtime/source | 安装后二进制 --help / src/cli.rs / 实现与测试 |
| 2 command contract | COMMANDS.md |
| 3 operating policy | SKILL.md、AGENTS.md、已拍板 ADR |
| 4 explanation | active references |
| 5 history/design | specs、backlog、dated docs |
```

状态表第一块「文件」列写实际文件名（Task 5/6 切分后回填 playbooks/* 与新文件名）。

- [ ] **Step 2: 验证 + Commit**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"` → OK

```bash
git add references/INDEX.md && git commit -m "docs: 新增 references/INDEX.md（六域路由/状态标注/权威层级唯一真源）"
```

### Task 5: 切分 control-loop-harness.md

**Files:**
- Create: `references/events-telemetry-contract.md`（原 `## 4. Signals and metrics` + `## 5. Run log and telemetry design`，现状合同）
- Create: `references/control-loop-roadmap.md`（原 `## 6. Typed workspace objects` + `## 10. Implementation plan` + `## 11. Review questions`，页首标 `> 状态：design/future——本文件是设计目标，不是现状`）
- Modify: `references/control-loop-harness.md`（保留 §1-3、§7-9、§12：Purpose/mapping/feedback loops/actuator limits/paving briefs/anti-patterns/non-goals；被移走的节留一行 stub：`## 4. Signals and metrics\n\n已迁至 [events-telemetry-contract.md](events-telemetry-contract.md)。`）
- Modify: `references/INDEX.md`（状态表回填两个新文件）

- [ ] **Step 1: 按上述边界移动内容**（剪切原文 verbatim，不改写；roadmap 文件页首加 future 标注；harness 留存节里「current vs future」混述句按现状改写清楚）
- [ ] **Step 2: 全仓修入链**

Run: `grep -rn 'control-loop-harness.md#' --include='*.md' . | grep -v '.lto/'`
指向被移节的 anchor 改指新文件；无 anchor 的链接不动（stub 兜底）。

- [ ] **Step 3: 验证 + Commit**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"` → OK（链接/anchor 检查会抓断链）

```bash
git add references/control-loop-harness.md references/events-telemetry-contract.md references/control-loop-roadmap.md references/INDEX.md
git commit -m "docs: 切分 control-loop-harness——现状合同与 future roadmap 分离"
```

### Task 6: 切分 workflow-playbook.md → references/playbooks/

**Files:**
- Create: `references/playbooks/{review,enterprise-audit,debug,migration,claim-verify,research,feature-dev,tmux-goal-loop,docs-sync,release,direction-review}.md`（11 个，对应原 `:70-533` 的 11 个 `###` 节，内容 verbatim 迁移，每文件首行 `# <name> playbook` + 一句适用条件）
- Modify: `references/workflow-playbook.md`：保留 `:7-59`（架构哲学/通用调度循环/两个前置闸门）+ `:534-544`（何时可抽 CLI）；`## Playbooks` 节改为 11 行链接表，并给每个原 `### <name>` 留 heading stub + 链接（外链 anchor 兼容）。
- Modify: `references/INDEX.md`（状态表回填 playbooks/*）

- [ ] **Step 1: 迁移**（逐个 `###` 剪切；`sed -n '70,109p'` 等按行段核对无遗漏——迁完 `wc -l references/workflow-playbook.md` 应 <120）
- [ ] **Step 2: 全仓修入链**

Run: `grep -rn 'workflow-playbook' --include='*.md' . | grep -v '.lto/'`——指向具体 playbook 的引用改指 `playbooks/<name>.md`。

- [ ] **Step 3: 验证 + Commit**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"` → OK

```bash
git add references/workflow-playbook.md references/playbooks/ references/INDEX.md
git commit -m "docs: workflow-playbook 按 11 个 playbook 真实切分，旧 heading 保 stub"
```

### Task 7: onboarding.md 瘦身

**Files:**
- Modify: `references/onboarding.md`

- [ ] **Step 1: 按保留/外链清单重写**

保留：一句话、术语表、进项目先看 `.lto`、为什么装、最小跑通流程（`:189-251` 核对命令后保留）。
外链化：安装细节→`INSTALL.md`；命令表→`COMMANDS.md`（Task 2 已做）；autopilot/recap 详解→`run-state-workflow.md`；hook→`hooks.md`；深入阅读表→`INDEX.md`。
目标体量 ≤8K（`wc -c`），且零 stale flag（checker 兜底）。

- [ ] **Step 2: 验证 + Commit**

Run: `python3 scripts/check_docs_consistency.py && wc -c references/onboarding.md` → OK 且 ≤8192

```bash
git add references/onboarding.md && git commit -m "docs: onboarding 瘦身为纯上手文档，细节外链化"
```

### Task 8: SKILL.md 四层重写（核心交付）

**Files:**
- Modify: `SKILL.md`（frontmatter 1-16 行逐字不动，正文全部重排）

**Interfaces:**
- Consumes: `references/INDEX.md`（六域表压缩映射自它）、Task 5-7 的切分产物路径。
- Produces: 新四层正文结构——Task 10 的 12 个路由用例对它验收。

- [ ] **Step 1: 按此骨架重写正文**（各节内容来源标注为旧 SKILL.md 行号，压缩不改语义）

```markdown
# LTO：长任务 harness（四层引擎）

<开场 3 行：迷路三问 + harness≠planner，源自旧 :18-22>

## ① ROUTER · 先路由再读文档
入口顺序（P1）：
1. 先看有无 active run：`lto runs` / `.lto/current` —— 有 → 域Ⅰ接管；没有 → 才谈域Ⅱ立项
2. 判动作意图 → 域（下表）；3. 高风险才跨域，默认读 1 个 reference，跨域/安全 ≤2
| 意图关键词 | 域 | 主 reference |
<六域压缩映射表，与 INDEX.md §1 域名逐字一致，详表链 INDEX.md>
跨域场景导航：<INDEX.md §2 的 6 行压缩版>

## ② OPERATING POLICY · 推理纪律
**P1 必须**：人说了算（phase 切换/不可逆/语义争议/closeout）｜先观测后控制（先读 .lto/git/runtime 再动作）｜
审者≠host（限主观/对抗审计）｜信息不足先自助补证、影响方案才问人（缺 goal/done-when 必须补齐才 start）｜
证据先于断言（区分源码存在/二进制存在/runtime 可用）｜调优必须有 baseline/pass line｜turn 完成≠goal 完成
**P2 建议**：最小版本优先｜先定标准再看数｜快 runner 优先收口（host 明确选择，不按历史 telemetry 自动路由）
**P3 可选**：输出附局限/失效条件
**停止规则**：已收敛→停｜证据齐→停｜三刹车（旧 :94-100 表 verbatim）
**常见错觉**（旧 :302-309 表 verbatim，5 行）

## ③ DOMAIN MAP · 六域卡
<每域 7 行固定卡：目标|首个安全观测|primitive|进入证据|human/stop gate|不适用/失效|权威源>
（Ⅰ-Ⅵ 内容按 spec §4 的域定义压缩；开发前四证据/收尾四证据各保留 4 个关键词一行，详情链 run-state-workflow）
派工边界（P1，源自旧 :33-36,133-157 压缩）：开发型派工必须 tmux 真 TUI（dispatch-goal / dispatch-and-wait）；
headless 只读评审兜底；agy --print 假成功陷阱；完成等 agent.dispatch.completed 不轮询。

## ④ AUTHORITY & SOURCE · 权威层级
| 要核对什么 | 权威顺序 |
| 命令/参数 | 二进制 --help → src/cli.rs → COMMANDS.md |
| gate/状态语义 | Rust 实现 → 回归测试 → state/event 实物 |
| workflow | active reference（见 INDEX.md 状态表） |
| 设计稿/backlog/历史 | 只作历史证据，不证明现状 |
文档与 runtime 冲突 = 文档漂移，修文档不做兼容解释。
LOOKUP→COMMANDS.md｜ROUTE→references/INDEX.md｜`.lto/` 是本项目真源（旧 :38 压缩）

## 什么时候不用 LTO
<旧 :293-300 表 verbatim>

## Workload Profile
<旧 :344-346 verbatim>
```

**下沉去向**（内容不丢，去处必须已存在）：audit 派工命令全家桶（旧 :102-174）→ `playbooks/review.md` 与 `audit-convergence.md`；部署清单（旧 :175-184）→ `deploy-sequencing.md`；记录/ANIMEM（旧 :185-220）→ `run-state-workflow.md` memory 节；多轮命令示例（旧 :222-291）→ `run-state-workflow.md`；Resources 清单（旧 :312-342）→ `INDEX.md`。下沉时核对目标文件已有等价内容，没有就把该段搬过去（不是删除）。

- [ ] **Step 2: 体量与一致性验证**

Run: `wc -c SKILL.md && python3 scripts/check_docs_consistency.py; echo "exit=$?"`
Expected: ≤10240（超出 ≤10% 需在 run-state 记原因）；checker OK。
Run: `grep -c '接管与恢复\|立项与契约\|执行与派工\|验证与收敛\|交付与发布\|学习与维护' SKILL.md references/INDEX.md` —— 两文件域名集合一致。

- [ ] **Step 3: Commit**

```bash
git add SKILL.md references/ && git commit -m "docs: SKILL.md 四层化（ROUTER/POLICY/DOMAIN MAP/AUTHORITY），细节下沉 references"
```

### Task 9: README 对齐

**Files:**
- Modify: `README.md`

- [ ] **Step 1:** `:60-79` runtime 拓扑不动；其后新增六域闭环小节（6 行表，域名同 INDEX）；`:88-99` L1-L4 段压成一句 + 链接 `references/control-loop-harness.md`；核对 checker 锚点字符串仍逐字在。
- [ ] **Step 2: 验证 + Commit**

Run: `python3 scripts/check_docs_consistency.py; echo "exit=$?"` → OK

```bash
git add README.md && git commit -m "docs: README 增六域坐标，压缩 L1-L4 叙述"
```

### Task 10: 12 路由用例验收 + 全套 gate

**Files:**
- Create: `.lto/20260714-.../route-cases-verdict.md`（run 内证据，不进 git）

- [ ] **Step 1: 逐例核对**——只读新 SKILL.md，对每例回答「进哪个域、读哪个 reference、触发哪条 P1」，与期望比对：

| # | 输入意图 | 期望域 | 期望主 reference / P1 |
|---|---|---|---|
| 1 | 接手别人跑过 LTO 的项目 | Ⅰ | onboarding.md |
| 2 | compact 后恢复上下文 | Ⅰ | long-loop-state.md |
| 3 | 开一个新的长交付 run | Ⅱ | run-state-workflow.md |
| 4 | 只修一个小 bug 该不该用 LTO | — | 「不用 LTO」表命中 |
| 5 | 派 codex 改代码 | Ⅲ | execution-loop.md + tmux 边界 P1 |
| 6 | 派工完成怎么检测 | Ⅲ | execution-loop.md（turn≠goal） |
| 7 | 方案要多模型审 | Ⅳ | playbooks/review.md，审者≠host |
| 8 | 审了三轮怎么判收敛 | Ⅳ | audit-convergence.md |
| 9 | 要上线部署 | Ⅴ | deploy-sequencing.md（真实用户路径） |
| 10 | 发版本 | Ⅴ | release-workflow.md |
| 11 | 三个 AI 都说好能合吗 | Ⅳ→P1 | 人说了算刹车 |
| 12 | autopilot 能不能全自动 | Ⅲ+Ⅵ | 证据闸门 + human gate 不取消 |

Expected: 12/12 命中；任何 miss → 修 ROUTER 表后复测全部 12 例。

- [ ] **Step 2: 全套 gate**

Run: `cargo fmt --all --check && cargo check --locked --all-targets && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked --all-targets && python3 scripts/check_docs_consistency.py && python3 scripts/check_python_rust_ownership.py && git diff --check`
Expected: 全绿（涉及 logs/telemetry 文档口径变化，追加跑 `scripts/privacy_self_check.sh`）。

- [ ] **Step 3:** `lto task add` 登记 T2-docs 任务并把 route-cases-verdict 记为 evidence；`lto check` 过一遍。

### Task 11: 异构审计收口（docs 部分）

- [ ] **Step 1:** `lto audit --auto-dispatch --prefer-runner codex --prefer-runner agy`（慢 runner 移出关键路径）
- [ ] **Step 2:** 亲验每条 finding（看源文件，不信自述），修复 → 复审到 ledger 收敛（CONVERGED）。
- [ ] **Step 3:** 修复产生的变更按所属文件补 commit；`lto check --strict` 过。

### Task 12: C1 goal 文档（稳定性信号）

**Files:**
- Create: `goal-2026-07-14-c1-stability-signal.md`（repo 根，沿 goal-2026-07-10-* 惯例）

- [ ] **Step 1: 按 goal-doc-for-codex 固定骨架写全**，内容=spec §6 C1 全部条目，落点/判据必须含：
  - 顶部：致 codex 约束沿用 + 「这份只做 C1，做完停」+ untracked 中间态是预期。
  - 核心架构裁决：硬 verdict 不动（closeout 唯一依赖）；diagnostics 五维正交只读（sample_sufficiency/terminal/direction/oscillation/envelope）；evaluator 从 `src/commands/util.rs:60-111,649-701` 下沉 `src/ledger.rs`，ops/closeout/telemetry 共用。
  - 三个前置修复各成 Phase：①砍 `scripts/audit_ledger_check.py` 重复实现（改薄壳调 Rust 或共享 golden fixtures；SKILL/文档同步）；②空 ledger ≠ 零 blocker 语义统一（`ops.rs:563-570` vs `closeout.rs:157-175`，裁决：空轮次 → 不是 Converged，closeout 需 `--force` 或补审计）；③事件更名 `audit.round.recorded`/`audit.ledger.evaluated`（`cli.rs:1995-2034`、`event_emit.rs:305-330`），旧 `audit.converged` 按历史 schema 读、不再写。
  - 观测可比性：ledger 行增记 coverage/auditor set/finding hash lineage，缺失 → diagnostics 标 low-confidence advisory。
  - forced_entropy 仅 advisory 展示。
  - 完成判据：`cargo test --locked ledger` 新增测试全绿（含 `[1,2,0]→Converged`、空 ledger、振荡序列 `[5,2,4,1,3]→oscillation=alternating`）；`grep -rn 'audit.converged' src/` 只剩历史读路径；checker/privacy/全套 gate 绿。
- [ ] **Step 2: Commit**

```bash
git add goal-2026-07-14-c1-stability-signal.md && git commit -m "docs: C1 goal——ledger 稳定性信号（硬 verdict+正交 diagnostics，砍 Python 重复）"
```

### Task 13: C2 goal 文档（禁猜闸门）

**Files:**
- Create: `goal-2026-07-14-c2-readiness-gate.md`

- [ ] **Step 1: 写全**，必须含：base readiness（`--goal/--done-when` 空 → 写盘前 fail，输出 `需补充: --goal/--done-when`；落点 `cli.rs:1093-1118` + `start_run :2219-2259`）；extended contract（全空放行、partial 写盘前 fail 并输出真实 flags；复用 `state.rs:92-111 missing_sections()`）；typed update 入口（`lto contract set --target ... --instrument ...`，服务旧 run 后补）；preflight 显式 run 时独立 readiness 子结果、与 `--record` 解耦（`ops.rs:234-350,328-343`）；回归矩阵六类（spec §6 C2 verbatim）；`--why` 保持 advisory；legacy state 可读。
- [ ] **Step 2: Commit**：`git add … && git commit -m "docs: C2 goal——run readiness + contract completeness 两层禁猜闸门"`

### Task 14: C3 goal 文档（finding 元数据）

**Files:**
- Create: `goal-2026-07-14-c3-finding-metadata.md`

- [ ] **Step 1: 写全**，必须含：字段定义 `reported_confidence{level,rationale}` + `invalidated_when`（自报/非校准/非概率/永不进 gate·排序·severity）；**全消费链改造清单**七处 verbatim（`audit.rs:15-25,89-121`；`audit_dispatch.rs:194-202`；`cli.rs:1903-1925,2182-2189`；`decision.rs:559-565,900-917`；`event_emit.rs:270-299`——event 只记 level/presence/hash）；serde optional-load+new-write+旧 fixture 回归；新增隔离回归测试（改 confidence/invalidated_when 只影响 brief 渲染，不改 direction/status/pick/gate verdict——现有 `judge_verdict_has_no_numeric_score_and_is_isolated` 不够，写明）。
- [ ] **Step 2: Commit**：`git commit -m "docs: C3 goal——finding 自报置信度与失效条件，贯通全消费链"`

### Task 15: C4 goal 文档（可观性子检查）

**Files:**
- Create: `goal-2026-07-14-c4-observability-subgate.md`

- [ ] **Step 1: 写全**，必须含：不新增平行 gate，`autonomous_gate`（`ops.rs:3192-3291`）返回 `operational_reliability` + `current_run_observability` 两个命名子结果；`cmd_autopilot`（`ops.rs:1215-1241`）传入当前 run state；`signal_declared` vs `observable_verified` 判据（instrument 与最新 evidence 结构化关联 + 结果可解析 + pass/stop 可判）；顺手修 reliability：any-历史-timeout 永久污染（`ops.rs:3256-3267`）改按 runner/model/task type + 有界近期样本 + 比例/连续失败；`mining_dispatches` 名实不符（`ops.rs:3230-3237`，实为 distinct_runs 求和）先改名/改指标再调阈值；未证实 → 降级 supervised/NEEDS_CONFIRM；不自动补契约不自动 route。
- [ ] **Step 2: Commit**：`git commit -m "docs: C4 goal——autonomous_gate 增 current-run observability 子检查"`

### Task 16: run 内收口（不 closeout）

- [ ] **Step 1:** `scripts/write_decision.py` 落 ADR：本轮关键裁决（权威层级倒置、迁移顺序、C4 并入 gate、Python evaluator 退役方向）。
- [ ] **Step 2:** 四份 goal 文档登记为 run artifacts/tasks；`lto recap` 核对 run 叙事完整。
- [ ] **Step 3:** `lto check --to closed` 预查（**不执行 closeout**——run 待 C1-C4 实现派工完成后再收）。剩余 untracked（engineering-cybernetics-essence/ 等）在 run-state 里点名为「人工接受的 dirt」。

---

## 并行性说明

Task 12-15（四份 goal 文档）互不依赖可并行写；但都依赖 Task 11 完成（审计可能改口径）。Task 5/6/7 文件不相交，理论可并行，但都要回填 INDEX.md（Task 4 产物）——串行执行避免合并冲突，单文件改动本就快。后续 C1-C4 实现派工：C1 与 C4 都碰 `telemetry.rs`，不可同时派；C2、C3 与两者文件不相交可并行。

## Self-Review 结论

- spec 覆盖：Phase 0→T1-3；A→T8；B→T4-7,9；C→T12-15；验收→T10-11；风险 1(三真源)→T8 Step2 域名集合校验；风险 2(外链)→T5/6 stub+grep 修入链；风险 3(Goodhart)→T10 路由用例是 pass line；风险 4(混载)→T5 roadmap 标注；风险 5(checker 自误)→T1 红验证+T3 自测；风险 6(lineage)→T12 内容。VERSION 单列→T2 Step4。无 TBD/占位符。域名/文件名前后一致。
