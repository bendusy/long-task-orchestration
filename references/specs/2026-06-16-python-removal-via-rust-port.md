# Spec: Python Fallback 整套退役(经 Rust port 双 legacy)

> Status: 实现 spec,交 codex 施工。主 agent(架构师)写,host 亲验代 commit。
> Run: `.lto/20260616-015522-python-removal-gate-rust-fallback-legacy-e51661fe`
> 决策: 用户拍板「先用 Rust 补 eval-run/source-note 再全删 Python」。

## 0. 背景与不可协商的边界

本机 Rust 已完整接管 LTO 24 个 top-level 命令 + 5 个 plugin 静态子命令。目标:**彻底退役 Python fallback(`LTO_USE_PYTHON=1`)与整套 `scripts/lto/`**。

**为什么不能裸删**(已盘清,codex 以此为前提,勿重复推翻):
- `scripts/lto/` 是单体耦合网:`plugin_eval_run.py`(legacy)深 import `agent_exec/agent_job/scheduler/state/llm_judge/plugins/plugin_extra`(A 类 fallback 基础设施);`smoke_test.py` 依赖整个包。逻辑可分,物理不可分 → 0 文件能单独删。
- spec 创建时 `plugin eval-run` 和 `plugin source-note` 在 Rust 侧**确无对等实现**(`src/cli.rs::PluginCommand` 只有 `List/Validate/RenderProfile/Eval/Mount`)。裸删 = 丢功能 = 违反 `references/python-rust-ownership.md` 的 "Port or retire separately"。
- 交付契约 `references/open-source-delivery-requirements.md` 的 Publish Blocker:「Python fallback is broken and not intentionally removed with migration notes」是硬停项。

> Implementation note 2026-06-16:阶段 A 的 Rust `plugin source-note` 与阶段 B 的 Rust `plugin eval-run` 已在当前 worktree 接管命令面，B.5 对等证据已记录在 `references/validation-log.md`；完整退役仍必须等待 wrapper/docs/tests 清理方案和人工 gate。

**唯一干净路径(本 spec)**:先 port 双 legacy 到 Rust(零功能损失)→ 再整套退役 Python → 同步 wrapper/ownership/docs/CI gate。

**红线**:
- 不得弱化任何现有 Rust 验证(clippy `-D warnings` / fmt / `cargo test --locked` 全绿)。
- 不得动 `scripts/delegate/runners/*.sh`(Rust 现役运行时 scheduler.rs spawn 的,非 Python)。
- 退役后 `check_docs_consistency.py`/`check_python_rust_ownership.py`/`smoke_test.py` 这些**纯 Python gate 也会随之失效**——必须用 Rust 等价 gate 顶上或显式退役并记录,不能留悬空引用。

---

## 阶段 A:Rust port `plugin source-note`(先做,小且独立)

### A.1 行为契约(对等 Python `plugin_extra.create_source_note`)

输入(CLI):`plugin source-note <plugin_dir> --id <id> --title <t> --url <u> [--claim <c>]* [--hypothesis <h>]* [--append-manifest] [--json]`

行为:
1. `plugin_dir` resolve;校验 `note_id` 匹配 plugin id 正则(对等 `core.ID_RE`,见 `src/plugin.rs` 已有的 id 校验)。不匹配 → 错误退出 rc=2,stderr `plugin source-note failed: source note id must match plugin id pattern`。
2. `sources/` 目录:若存在且是 symlink → 报错 rc=2。否则 mkdir -p。
3. 写 `sources/<id>.json`(原子写,先写 tmp 再 rename),内容**逐字段对等**:
   ```json
   {
     "id": "<id>",
     "title": "<title>",
     "url": "<url>",
     "captured_at": "<iso_now,对等 st.iso_now()>",
     "claims": [{"id":"c1","text":"<claim1>","status":"unverified"}, ...],
     "hypotheses": [{"id":"h1","text":"<hyp1>"}, ...],
     "lto_status": "source-note-only; inert until referenced by an experimental plugin"
   }
   ```
   注意 claims 编号 `c1/c2...`、hypotheses 编号 `h1/h2...`,`lto_status` 是固定串(逐字节抄)。
4. `--append-manifest`:读 `plugin.json`(必须是 object,否则报错)→ `source_notes` 数组 append 相对路径 `sources/<id>.json`(已存在则不重复)→ 原子写回。
5. 路径安全:产出文件必须在 plugin_dir 内(对等 `_ensure_inside`)。
6. 输出:`--json` 打印 `{"path":..,"id":..,"appended_manifest":bool}`(sort_keys);否则打印 `source note: <path>`。成功 rc=0。

### A.2 落点
- `src/cli.rs`:`PluginCommand` 新增 `SourceNote { dir, id, title, url, claim:Vec<String>, hypothesis:Vec<String>, append_manifest:bool, json:bool }`。
- `src/cli.rs::cmd_plugin` 加 dispatch 分支。
- `src/plugin.rs`:新增 `pub fn create_source_note(...) -> anyhow::Result<PathBuf>` + CLI handler。复用已有的 id 正则、`_ensure_inside` 等价逻辑、原子写 helper(若无则加一个 `atomic_write_json`)。

### A.3 测试(Rust,`#[test]` in `src/plugin.rs` 或 `tests/`)
- 正常写 + 字段逐项断言(claims 编号、lto_status 固定串、captured_at 非空)。
- id 不合法 → Err。
- symlink sources/ → Err。
- `--append-manifest` 幂等(跑两次 source_notes 不重复)。
- 路径逃逸防御(id 含 `../` 之类被 id 正则挡掉)。

---

## 阶段 B:Rust port `plugin eval-run`(大头,real baseline-vs-candidate A/B)

### B.1 行为契约(对等 `scripts/lto/plugin_eval_run.py::eval_run`,642 行,codex 必须通读该文件)

签名等价:`eval_run(repo, run_id, plugin_dir, eval_id?, only_case?, max_concurrency=4, persist=true, runners_dir?)`

核心流程(codex 以 Python 源为准逐步对齐):
1. `validate_plugin(plugin_dir)` 失败 → 返回 `{ok:false, error:"plugin validation failed: ...", plugin}`。
2. `_load_eval_pack(plugin_dir, manifest, eval_id)` 找不到 → `{ok:false, error:"eval pack not found ..."}`。
3. `env_allowlist` = manifest `security.env_allowlist`;只有白名单内的 key 能进 candidate job env(白名单外的 profile env 丢弃 + warning)。这是安全边界,**必须对齐**。
4. `only_case` 过滤;找不到 → `{ok:false, error:"case not found: <id>"}`。
5. 每个 case 跑 `_run_case`:baseline job + candidate job(candidate 应用 profile),都经 scheduler spawn runner(复用 Rust 已有 `src/scheduler.rs` + `AgentJob`)。落盘到 `.lto/<run-id>/plugin-eval/<case-id>/`。
6. 返回 `{ok, run_id, plugin, eval_id, cases:[{baseline, candidate, comparison}...], deferred}`。
7. CLI handler(对等 `commands/plugin.py::_eval_run`):`--output` 写文件;`--json` 或无 output 时打印;rc = `0 if report.ok else 2`。

### B.2 关键复用(Rust 侧已存在,勿重写)
- `src/scheduler.rs`:并发 job 调度、runner spawn、三元退出码判定。
- `src/plugin.rs`:`validate_plugin`、manifest 解析、mount 检查(`_mounted_sandbox` 对等)。
- `AgentJob`/`AgentResult`/`Budget`/`Permission` 等价类型。

### B.3 落点
- `src/cli.rs`:`PluginCommand` 新增 `EvalRun { dir, run_id:Option, eval_id:Option, only_case:Option, max_concurrency:usize(default 4), persist:bool(default true), runners_dir:Option, output:Option, json:bool }`。
- `src/plugin.rs` 或新 `src/plugin_eval_run.rs`:`pub fn eval_run(...) -> Result<serde_json::Value>` + handler。
- `DEFERRED_V0`:Python `plugin_eval_run.py` 有个 `DEFERRED_V0` 常量(smoke 引用)。Rust 侧对应概念要保留(哪些能力 v0 故意不做),并在 docs 标注。

### B.4 测试(Rust)
- plugin validation 失败路径。
- eval pack 缺失/case 缺失路径。
- env_allowlist 过滤:白名单外 env 不进 candidate(安全断言,关键)。
- 至少一个 end-to-end:用 fixture plugin + fake runner(.sh)跑通 baseline/candidate,断言 report 结构 + 落盘文件存在。可复用 `src/scheduler.rs` 测试里造 fake python runner 的模式(注意:fake runner 用 .sh 即可,**不引入 Python 依赖**)。

### B.5 对等验证(port 正确性的硬证据)
跑同一 fixture plugin,对比 Python `eval_run` 与 Rust `eval-run` 的 report(忽略时间戳/绝对路径后)结构与 verdict 一致。这是「Rust 对等」的 removal gate 证据,必须产出。

---

## 阶段 C:整套退役 Python(A/B 验证对等后才动)

### C.0 安全删除 gate(必须写进 docs/closeout)

删除 Python 只能发生在 Rust 已经接管外部行为之后。host 在进入 C 前必须逐项记录:

1. ownership manifest 中每个 Python surface 已分类:`rust-core` / `compatibility-fallback` / `python-legacy` / `removal-candidate`。
2. Rust 已实现对应 CLI/JSON/文件输出契约,并有成功、失败、路径安全、兼容 fixture 测试。
3. 同一 fixture 跑 Python 与 Rust 的 parity 证据已落盘；若某行为不保留,必须有 explicit retirement decision。
4. wrapper 不再路由到 Python；active docs 不再教 Python fallback；CI/tests/gates 不再 import `scripts/lto/`。
5. rollback 已保留:old-run fixture、release note 或前一版本 tag 至少有一项可用。
6. 删除清单不得包含 `scripts/delegate/runners/*.sh` 和 `healthcheck.sh`,这些是 Rust scheduler 现役 runner adapter。
7. `LTO_USE_PYTHON=1 lto self-test` 退役后必须清晰报错,不能静默 fallback 或半路 import 失败。

### C.1 删除清单(约 90 个 .py)
- 整个 `scripts/lto/`(含 `__init__.py` + ~44 命令镜像 + 6 个 lto/test_*)。
- `scripts/lto_run.py`。
- `scripts/test_*.py` 里**测 Python fallback 的**(约 76 个 test 的大部分)。

### C.2 必须保留或 Rust 化的
- `scripts/delegate/runners/*.sh` + healthcheck.sh:**保留**(Rust 现役运行时调)。
- 测 runner .sh 的逻辑(原 `test_codex_runner.py` 等):若仍需,**用 Rust 重写**(它们测的是现役基础设施)。
- 纯文本 gate 不 import lto 的(`check_docs_consistency.py` 部分逻辑):评估能否独立存活或 Rust 化。

### C.3 同步改动(删后不能留悬空)
1. `scripts/install.sh`:删 `LTO_USE_PYTHON` 分支(:115/:133)、删 `LTO_RUN_TARGET=lto_run.py`(:8)。`lto` 只指向 `lto-rs`,缺失则清晰报错。
2. `references/python-rust-ownership.md` + `.json`:整体退役,改为「Python fully removed; Rust owns all commands」声明,或删除该 gate。
3. `references/open-source-delivery-requirements.md`:删/改所有 `--use-python`/`LTO_USE_PYTHON`/Python fallback 的「must remain tested」表述,改为「Python removed at <commit>, migration note」。Hard Non-Goals 的「No hidden Python default」可保留为历史。
4. `tests/python_rust_compat.rs`:它依赖 Python 写 run 做对照。改为只测 legacy fixture(`tests/fixtures/legacy-run/state.json`)可被 Rust 读 —— **保留 old-run 兼容测试**(这是 .lto 协议兼容,与 Python 死活无关),删掉「Python 写的 run 可被 Rust 读」那半。
5. `check_docs_consistency.py` 等本地 gate:Rust 化或显式退役并在 CHANGELOG 记录失去了什么。
6. CI `rust-v2.yml`:本来就不跑 Python,无需改 —— 但可考虑把退役后的 Rust gate(若 C.5 做了)加进 CI。
7. 所有 docs(README/INSTALL/AGENTS/CLAUDE/COMMANDS/SKILL/onboarding):删 Python fallback 安装/使用段。

### C.4 隐私/版本
- 退役后 `privacy_self_check.sh` 若依赖 Python 测试 fixture(`test_eval_judge.py` 等被删),它的 redact fixture 断言要相应更新。
- 版本:这是 breaking change(fallback 消失),bump 到 **v0.5.0**(MINOR,因 fallback 本是 documented 能力)。同步 `Cargo.toml`/`VERSION`/`CHANGELOG.md`,version drift gate 会校验。

### C.5 Phase C 执行交接单(仅人工批准后执行)

进入 C 前先把本小节复制到 closeout 或 release PR 描述里逐项打勾。任何一项不能打勾,停在 B 后状态,不要做删除。

**进入条件**:
- [ ] 用户明确确认进入 Phase C,并接受 `LTO_USE_PYTHON=1` / `--use-python` 消失。
- [ ] `references/validation-log.md` 已记录阶段 A/B parity,且本机重跑通过。
- [ ] `references/python-rust-ownership.json` 不再有 `python-legacy` 仍需保留的外部 CLI surface；若有,先 port 或写 explicit retirement decision。
- [ ] 已确认 rollback:上一版本 tag/release note 或 old-run fixture 至少一种可用。

**删除候选(批准后删除)**:
- [ ] `scripts/lto/` 整目录。
- [ ] `scripts/lto_run.py`。
- [ ] `scripts/test_*.py` 中只验证 Python fallback 或 import `scripts/lto/` 的测试。
- [ ] Python fallback 专用 fixture / helper / doc smoke,除非被改造成 Rust gate。

**必须保留**:
- [ ] `scripts/delegate/runners/*.sh`。
- [ ] `scripts/delegate/runners/healthcheck.sh`。
- [ ] 旧 `.lto` state fixture 兼容测试；目标是 Rust 能读历史协议,不是 Python 还能写新 state。
- [ ] 不依赖 `scripts/lto/` 的发布、隐私、文档一致性 gate；若仍有价值,先 Rust 化或独立化。

**同步改口径**:
- [ ] `scripts/install.sh` 删除 `LTO_RUN_TARGET`, `--use-python`, `LTO_USE_PYTHON` 分支；缺 Rust binary 时只报清晰安装错误。
- [ ] `README.md`, `INSTALL.md`, `AGENTS.md`, `CLAUDE.md`, `COMMANDS.md`, `SKILL.md`, `references/onboarding.md`, `references/rust-migration-release.md`, `references/sharing-guide.md`, `references/run-state-workflow.md` 不再把 Python fallback 写成 active path。
- [ ] `references/open-source-delivery-requirements.md` 改为记录 Python 已在具体 commit/tag 退役,不再要求 fallback smoke。
- [ ] `references/python-rust-ownership.{md,json}` 改为 Rust fully owns all commands,或把该 gate 改名为 historical compatibility gate。
- [ ] `CHANGELOG.md`, `Cargo.toml`, `VERSION` 同步 v0.5.0 breaking change。

**Phase C 验证命令**:
```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
bash scripts/install.sh
lto self-test
LTO_USE_PYTHON=1 lto self-test  # 期望清晰报错,不能 silent fallback
lto plugin source-note <fixture-plugin> --id <id> --title <title> --url <url> --append-manifest --json
lto plugin eval-run <fixture-plugin> --run-id <run-id> --runners-dir <fake-runners> --json
git grep -n -- '--use-python\|LTO_USE_PYTHON\|scripts/lto_run.py\|python3 scripts/lto_run.py'
git diff --check
```

**停止条件**:
- `scripts/delegate/runners/*.sh` 或 `healthcheck.sh` 出现在 delete diff 里:立即停。
- 任一 active doc 仍教 `--use-python` / `LTO_USE_PYTHON=1` 作为可用路径:立即停。
- `LTO_USE_PYTHON=1 lto self-test` 变成 import traceback 或静默走 Rust:立即停。
- `cargo test --locked --all-targets` 需要通过删除断言来变绿:立即停,先补 Rust 等价测试。

---

## 验证矩阵(host 亲验,每阶段闭环)

阶段 A 后:`cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked` 全绿 + Rust source-note 与 Python 产出逐字段 diff 一致。
阶段 B 后:同上 + eval-run 对等验证(B.5)report 一致。
阶段 C 后:
- `cargo test --locked --all-targets` 全绿(含改造后的 python_rust_compat 只测 legacy fixture)。
- `git ls-files '*.py' | wc -l` 降到预期(只剩保留/Rust 化后的少数)。
- `LTO_USE_PYTHON=1 lto self-test` **应清晰报错**(fallback 已退役),不能静默。
- `bash scripts/install.sh` + `lto self-test` + `lto plugin source-note ...` + `lto plugin eval-run ...` 全跑通(证明 port 的 legacy 真可用)。
- 隐私扫描 clean。docs grep 无悬空 `--use-python` 引用。
- 打 v0.5.0 tag 前过完整 Push Candidate Freeze Gate。

## 阶段顺序铁律
A → B(各自验证对等)→ **人工确认 port 对等证据** → C(删)→ host 亲验全绿 → closeout 记 removal gate → 打 tag。
**A/B 没证明对等前不许进 C。** 这是 ownership 的 "Prove parity → Move owner → Delete after rollback preserved"。
