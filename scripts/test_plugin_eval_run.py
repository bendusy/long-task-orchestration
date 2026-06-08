"""Tests for `lto plugin eval-run` — the eval-pack A/B compiler.

用 fake runner（不调真 LLM）验证：
- compile 出 baseline + candidate 两条腿，各落证据；
- 确定性指标从 AgentResult 正确计算（parse_ok / private_path_leak / timeout）；
- candidate brief 真的注入了 profile 指令（与 baseline 不同）；
- 单 case 失败（profile 缺失）不崩整个 run；
- deferred 字段诚实声明 v0 未做的能力。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lto import plugin_eval_run, state as st  # noqa: E402


def _fake_runner_dir(root: Path, reply: str) -> Path:
    """造一个把固定 reply 写进 reply_file 的 fake runner（codex）。"""
    runners = root / "runners"
    runners.mkdir(parents=True, exist_ok=True)
    fake = root / "fake_runner.py"
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "reply_file = sys.argv[2]\n"
        f"open(reply_file, 'w').write({reply!r})\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    sh = runners / "codex.sh"
    sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8")
    sh.chmod(0o755)
    hc = runners / "healthcheck.sh"
    hc.write_text('#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"}]\'\nexit 0\n', encoding="utf-8")
    hc.chmod(0o755)
    return runners


def _mini_plugin(root: Path, *, with_schema: bool = True) -> Path:
    """造一个最小 data-only 插件：1 profile + 1 eval pack + 1 case。"""
    pdir = root / "plugins" / "mini"
    (pdir / "profiles").mkdir(parents=True)
    (pdir / "evals").mkdir(parents=True)
    (pdir / "sources").mkdir(parents=True)
    (pdir / "schemas").mkdir(parents=True)
    (pdir / "sources" / "note.json").write_text(
        json.dumps({"id": "note.mini", "url": "https://example.com", "claims": []}), encoding="utf-8"
    )
    profile = {
        "id": "mini-profile-v1",
        "runner": "codex",
        "prompt_suffix": "OUTPUT MUST BE JSON FINDINGS.",
        "permission": {"sandbox": "read-only"},
    }
    if with_schema:
        profile["output_schema_ref"] = "schemas/findings.json"
        (pdir / "schemas" / "findings.json").write_text(
            json.dumps({"type": "object"}), encoding="utf-8"
        )
    (pdir / "profiles" / "p.json").write_text(json.dumps(profile), encoding="utf-8")
    (pdir / "evals" / "cases.json").write_text(
        json.dumps(
            {
                "id": "mini-cases-v1",
                "metrics": ["parse_rate", "private_path_leaks"],
                "cases": [
                    {"id": "c1", "runner": "codex", "profile": "mini-profile-v1", "brief": "Audit this spec."}
                ],
            }
        ),
        encoding="utf-8",
    )
    manifest = {
        "id": "mini",
        "version": "0.1.0",
        "stage": "experimental",
        "kind": "path-plugin",
        "description": "mini test plugin",
        "source_notes": ["sources/note.json"],
        "provides": {
            "profiles": ["profiles/p.json"],
            "evals": ["evals/cases.json"],
        },
        "security": {"executable_code": False, "max_sandbox": "read-only"},
        "default_enabled": False,
    }
    (pdir / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    return pdir


def _init_run(repo: Path, run_id: str) -> None:
    state_dir = repo / ".lto" / run_id
    state_dir.mkdir(parents=True)
    st.save_state(state_dir / "state.json", {"run_id": run_id, "tasks": {}})


def test_ab_compiles_two_legs_and_metrics(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r1"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    runners = _fake_runner_dir(tmp_path, reply='{"findings": []}')

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )

    assert report["ok"] is True, report
    assert len(report["cases"]) == 1
    case = report["cases"][0]
    # 两条腿都落证据
    cdir = repo / ".lto" / run_id / "plugin-eval" / "c1"
    assert (cdir / "baseline-result.json").exists()
    assert (cdir / "candidate-result.json").exists()
    assert (cdir / "comparison.json").exists()
    # candidate brief 注入了 profile 指令，与 baseline 不同
    base_brief = (cdir / "baseline-brief.md").read_text()
    cand_brief = (cdir / "candidate-brief.md").read_text()
    assert "OUTPUT MUST BE JSON FINDINGS." in cand_brief
    assert "OUTPUT MUST BE JSON FINDINGS." not in base_brief
    # candidate 声明 schema → JSON reply 应 parse_ok=True
    assert case["candidate"]["parse_ok"] is True
    # baseline 无 schema → parse_ok 为 None（不评）
    assert case["baseline"]["parse_ok"] is None


def test_private_path_leak_detected(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r2"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo, with_schema=False)
    # reply 里塞一个本机绝对路径 → 应被 leak 扫描命中
    runners = _fake_runner_dir(tmp_path, reply="see /Users/secret/private.key for token")

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    case = report["cases"][0]
    assert case["candidate"]["private_path_leak"] is True
    assert case["baseline"]["private_path_leak"] is True


def test_missing_profile_fails_case_not_run(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r3"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    # 篡改 case 指向不存在的 profile
    pack = pdir / "evals" / "cases.json"
    data = json.loads(pack.read_text())
    data["cases"][0]["profile"] = "does-not-exist"
    pack.write_text(json.dumps(data), encoding="utf-8")
    runners = _fake_runner_dir(tmp_path, reply="{}")

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    assert report["ok"] is False
    assert report["cases"][0]["ok"] is False
    assert "render_profile failed" in report["cases"][0]["error"]


def test_deferred_v0_declared(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r4"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    runners = _fake_runner_dir(tmp_path, reply='{"findings":[]}')
    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    # v0 诚实声明未做的能力，不静默截断
    assert "llm_judge_blocker_quality" in report["deferred"]
    assert "automatic_promotion" in report["deferred"]


def test_eval_pack_not_found(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r5"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    report = plugin_eval_run.eval_run(repo, run_id, pdir, eval_id="no-such-pack")
    assert report["ok"] is False
    assert "eval pack not found" in report["error"]


def test_unknown_runner_fails_case_not_crash(tmp_path: Path) -> None:
    """C 回归：case.runner 不在 KNOWN_RUNNERS 时降级为 case 失败，不崩整个 run。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r6"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    pack = pdir / "evals" / "cases.json"
    data = json.loads(pack.read_text())
    data["cases"][0]["runner"] = "totally-fake-runner"
    pack.write_text(json.dumps(data), encoding="utf-8")
    runners = _fake_runner_dir(tmp_path, reply="{}")

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    assert report["ok"] is False
    assert "unknown runner" in report["cases"][0]["error"]


def test_mount_lock_key_parsed(tmp_path: Path) -> None:
    """A 回归：mount lock 顶层 key 是 'mounts'，sandbox 在 approved_permissions.max_sandbox。
    未 mount 时 report.mount_present=False 且有 warning（取证链声明）。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r7"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    runners = _fake_runner_dir(tmp_path, reply='{"findings":[]}')
    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    # 没 mount → present False + warning + 默认 read-only
    assert report["mount_present"] is False
    assert report["approved_sandbox"] == "read-only"
    assert any("not mounted" in w for w in report["warnings"])

    # 手写一个 mounts-key lock，确认 _mounted_sandbox 能读到（不再永远 read-only）
    lock = {
        "schema_version": 1,
        "run_id": run_id,
        "mounts": [
            {"plugin_id": "mini", "approved_permissions": {"max_sandbox": "read-only"}}
        ],
    }
    from lto import plugins as core

    core.mount_lock_path(repo, run_id).write_text(json.dumps(lock), encoding="utf-8")
    sandbox, present = plugin_eval_run._mounted_sandbox(repo, run_id, "mini")
    assert present is True
    assert sandbox == "read-only"
    # plugin_id 精确匹配：别的 id 不命中
    _, present2 = plugin_eval_run._mounted_sandbox(repo, run_id, "ximini")
    assert present2 is False


def test_comparison_ok_reflects_runner_failure(tmp_path: Path) -> None:
    """B 回归：runner 非零退出 → comparison.ok=False，污染不到的 report.ok 也 False。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r8"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo, with_schema=False)
    # fake runner 写 reply 但 exit 1 → status 非 ok
    runners = tmp_path / "runners"
    runners.mkdir()
    fake = tmp_path / "fail_runner.py"
    fake.write_text(
        "#!/usr/bin/env python3\nimport sys\nopen(sys.argv[2],'w').write('partial')\nsys.exit(1)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    (runners / "codex.sh").write_text(
        f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8"
    )
    (runners / "codex.sh").chmod(0o755)
    (runners / "healthcheck.sh").write_text(
        '#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"}]\'\nexit 0\n', encoding="utf-8"
    )
    (runners / "healthcheck.sh").chmod(0o755)

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    assert report["ok"] is False
    assert report["cases"][0]["ok"] is False


def test_json_parses_fenced_and_garbage() -> None:
    """G 回归：fence 提取精确，尾随垃圾不被误判为合法 JSON。"""
    assert plugin_eval_run._json_parses('{"a":1}') is True
    assert plugin_eval_run._json_parses('```json\n{"a":1}\n```') is True
    assert plugin_eval_run._json_parses('```\n{"a":1}\n```') is True
    assert plugin_eval_run._json_parses('{"a":1}\ntrailing garbage') is False
    assert plugin_eval_run._json_parses("not json at all") is False
    assert plugin_eval_run._json_parses("") is False


def test_private_path_regex_covers_linux_and_windows() -> None:
    """J 回归：补的路径前缀真命中。"""
    rx = plugin_eval_run._PRIVATE_PATH_RE
    assert rx.search("token at /root/.ssh/id_rsa")
    assert rx.search("tmp /tmp/lto_reply_xyz")
    assert rx.search("/var/folders/ab/cd/T/x")
    assert rx.search(r"C:\Users\ben\secret.txt")
    assert not rx.search("just a relative ./path/file")


def test_pointer_only_detection() -> None:
    """post-exec 闸补全：pointer-only reply 检测（agy/gemini 已知会只回指针）。"""
    f = plugin_eval_run._is_pointer_only
    # 真 pointer-only：短 + 文件指针/引用短语
    assert f("see file:///tmp/result.txt", parsed_substantive=False) is True
    assert f("done, see the artifact", parsed_substantive=False) is True
    assert f("结果在 /tmp/lto_reply_x 见文件", parsed_substantive=False) is True
    assert f("/Users/ben/out.json", parsed_substantive=False) is True
    # 有实质 JSON 内容 → 不算 pointer-only（即便含路径）
    assert f('{"findings":[{"t":"x"}]} written to /tmp/x', parsed_substantive=True) is False
    # 长实质回复 → 不算，即便提到路径
    long_reply = "The audit found 3 issues. " * 20 + "details saved to /tmp/x"
    assert f(long_reply, parsed_substantive=False) is False
    # 空回复 → 不混进 pointer-only（另算 empty failure）
    assert f("", parsed_substantive=False) is False
    # 正常短实质回复（无路径无指针短语）→ 不算
    assert f("No issues found in the spec.", parsed_substantive=False) is False
    # 审计反馈补的漏判案例：agy 中文输出风格 + 多行 "Result saved.\nFile: /path"
    assert f("输出到 /tmp/x", parsed_substantive=False) is True
    assert f("保存在 /tmp/result.json", parsed_substantive=False) is True
    assert f("Result saved.\nFile: /tmp/lto-dev/.lto/r1/candidate-result.json", parsed_substantive=False) is True
    # 空 findings 的合法 JSON → 不算 pointer-only（属另一类"没干活"，见 DEFERRED quality）
    assert f('{"findings":[]}', parsed_substantive=True) is False


def test_pointer_only_in_metrics_and_deltas(tmp_path: Path) -> None:
    """pointer-only 进 metrics + deltas，且 deferred 不再列它。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "rp"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo, with_schema=False)
    runners = _fake_runner_dir(tmp_path, reply="see /tmp/result.txt")
    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    case = report["cases"][0]
    assert case["candidate"]["pointer_only"] is True
    assert "candidate_new_pointer_only" in case["deltas"]
    # 已实现 → 不再挂在 deferred
    assert "pointer_only_reply_detection" not in report["deferred"]


if __name__ == "__main__":
    import pytest

    sys.exit(pytest.main([__file__, "-v"]))
