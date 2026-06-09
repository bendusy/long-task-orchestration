"""Tests for llm_judge — eval-run 的主观判读层（与确定性层严格隔离）。

照 test_plugin_eval_run.py 风格用 fake runner（不调真 LLM）验证三铁律：
- frozen hash 确定性（同 redacted 输入 → 同 hash；改一字符 → hash 变）；
- redact 真生效（塞 /Users/ben/secret + 假 token → frozen_inputs 被脱敏）；
- judge 异构选择（同族跳过、异构正常派）；
- judge 层隔离（judge 报质量差 → 确定性 metrics / promote 路径不受影响）。
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from lto import llm_judge, plugin_eval_run, state as st  # noqa: E402


# ---------------------------------------------------------------------------
# A. frozen hash 确定性
# ---------------------------------------------------------------------------

def test_frozen_hash_deterministic_same_input(tmp_path: Path) -> None:
    d1 = tmp_path / "c1"
    d2 = tmp_path / "c2"
    d1.mkdir()
    d2.mkdir()
    b1 = llm_judge.freeze_evidence(d1, "brief X", "base reply", "cand reply")
    b2 = llm_judge.freeze_evidence(d2, "brief X", "base reply", "cand reply")
    assert b1["evidence_hash"] == b2["evidence_hash"]
    assert b1["evidence_hash"].startswith("sha256:")


def test_frozen_hash_changes_on_one_char(tmp_path: Path) -> None:
    d1 = tmp_path / "c1"
    d2 = tmp_path / "c2"
    d1.mkdir()
    d2.mkdir()
    b1 = llm_judge.freeze_evidence(d1, "brief", "base", "candidate reply")
    b2 = llm_judge.freeze_evidence(d2, "brief", "base", "candidate replX")
    assert b1["evidence_hash"] != b2["evidence_hash"]


def test_frozen_hash_normalizes_line_endings(tmp_path: Path) -> None:
    d1 = tmp_path / "c1"
    d2 = tmp_path / "c2"
    d1.mkdir()
    d2.mkdir()
    b1 = llm_judge.freeze_evidence(d1, "a\nb", "x\ny", "p\nq")
    b2 = llm_judge.freeze_evidence(d2, "a\r\nb", "x\ry", "p\nq\n")
    assert b1["evidence_hash"] == b2["evidence_hash"]


# ---------------------------------------------------------------------------
# B. redact 真生效
# ---------------------------------------------------------------------------

def test_redact_strips_private_path_and_token(tmp_path: Path) -> None:
    d = tmp_path / "c"
    d.mkdir()
    raw_cand = "found secret at /Users/ben/secret/key.txt token=sk-ant-abcdefghijkl1234567890"
    bundle = llm_judge.freeze_evidence(d, "brief", "baseline", raw_cand)
    fi = bundle["frozen_inputs"]["candidate_reply"]
    # 私有路径被脱敏
    assert "/Users/ben/secret" not in fi
    assert "[REDACTED_PATH]" in fi
    # 假 token 被脱敏
    assert "sk-ant-abcdefghijkl1234567890" not in fi
    assert "[REDACTED_SECRET]" in fi
    # 写盘的 frozen-evidence.json 也不含原始 secret/path
    on_disk = (d / "frozen-evidence.json").read_text()
    assert "/Users/ben/secret" not in on_disk
    assert "sk-ant-abcdefghijkl1234567890" not in on_disk
    assert bundle["redaction"] == "applied"


def test_redact_eats_whole_path_not_just_prefix(tmp_path: Path) -> None:
    """BLOCKER2: 整条绝对路径被吃掉，目录结构 + 文件名都不残留。"""
    d = tmp_path / "c"
    d.mkdir()
    # POSIX /Users + /home + Windows + JSON-escaped 形式
    raw = (
        "leak1 /Users/ben/secret/key.txt end1 "
        "leak2 /home/alice/private/id_rsa end2 "
        r"leak3 C:\Users\bob\secret\key.txt end3 "
        r"leak4 \/Users\/ben\/secret\/key.txt end4"
    )
    bundle = llm_judge.freeze_evidence(d, "brief", "baseline", raw)
    fi = bundle["frozen_inputs"]["candidate_reply"]
    on_disk = (d / "frozen-evidence.json").read_text()
    for blob in (fi, on_disk):
        # 不再只挡前缀：目录结构 + 文件名整条消失
        assert "secret/key.txt" not in blob, blob
        assert "private/id_rsa" not in blob, blob
        assert r"\secret\key.txt" not in blob, blob
        assert "alice" not in blob, blob
        assert "bob" not in blob, blob
    assert "[REDACTED_PATH]" in fi


def test_redact_full_pem_block_and_kv_secrets(tmp_path: Path) -> None:
    """BLOCKER3: 完整 PEM block（含正文 + END 行）+ key-value 型 secret 全被 redact。"""
    d = tmp_path / "c"
    d.mkdir()
    pem = (
        "-----BEGIN RSA PRIVATE KEY-----\n"
        "MIIEpAIBAAKCAQEAusecretbase64line1\n"
        "secretbase64line2plusmore==\n"
        "-----END RSA PRIVATE KEY-----"
    )
    raw = (
        f"here is a key:\n{pem}\n"
        "and github_pat_11ABCDEFG0123456789abc more\n"
        "api_key=supersecretvalue123 and token: anothersecret456 done"
    )
    bundle = llm_judge.freeze_evidence(d, "brief", "baseline", raw)
    fi = bundle["frozen_inputs"]["candidate_reply"]
    on_disk = (d / "frozen-evidence.json").read_text()
    for blob in (fi, on_disk):
        # PEM 正文 + END 行不再穿透
        assert "MIIEpAIBAAKCAQEA" not in blob, blob
        assert "secretbase64line2plusmore" not in blob, blob
        assert "END RSA PRIVATE KEY" not in blob, blob
        assert "github_pat_11ABCDEFG0123456789abc" not in blob, blob
        assert "supersecretvalue123" not in blob, blob
        assert "anothersecret456" not in blob, blob
    assert "[REDACTED_SECRET]" in fi


# ---------------------------------------------------------------------------
# 测试用 fake runner（把固定 JSON judge reply 写进 reply_file）
# ---------------------------------------------------------------------------

def _fake_judge_runner_dir(root: Path, runner_name: str, reply: str) -> Path:
    runners = root / "runners"
    runners.mkdir(parents=True, exist_ok=True)
    fake = root / f"fake_{runner_name}.py"
    fake.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "reply_file = sys.argv[2]\n"
        f"open(reply_file, 'w').write({reply!r})\n"
        "sys.exit(0)\n",
        encoding="utf-8",
    )
    fake.chmod(0o755)
    sh = runners / f"{runner_name}.sh"
    sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8")
    sh.chmod(0o755)
    hc = runners / "healthcheck.sh"
    hc.write_text(
        f'#!/usr/bin/env bash\necho \'[{{"agent":"{runner_name}","verdict":"OK"}}]\'\nexit 0\n',
        encoding="utf-8",
    )
    hc.chmod(0o755)
    return runners


def _init_run(repo: Path, run_id: str) -> Path:
    state_dir = repo / ".lto" / run_id
    state_dir.mkdir(parents=True, exist_ok=True)
    st.save_state(state_dir / "state.json", {"run_id": run_id, "tasks": {}})
    return state_dir


# ---------------------------------------------------------------------------
# C. judge 异构选择
# ---------------------------------------------------------------------------

def test_judge_skips_same_family(tmp_path: Path) -> None:
    """显式给同族 judge runner → 跳过（不报错、不派工）。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r1"
    case_dir = repo / ".lto" / run_id / "plugin-eval" / "c1"
    case_dir.mkdir(parents=True)
    _init_run(repo, run_id)
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")

    # candidate runner = codex (openai)，judge 也指定 codex → 同族 → 跳过
    layer = llm_judge.judge_case(
        repo, run_id, case_dir,
        candidate_runner="codex", frozen=frozen,
        persist=False, judge_runner="codex",
    )
    assert layer["status"] == "skipped"
    assert layer["kind"] == "subjective_judgment"
    assert "evidence_hash" in layer


def test_judge_skips_when_no_heterogeneous(tmp_path: Path, monkeypatch) -> None:
    """候选池全是 candidate 同族 → status=skipped no heterogeneous runner。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r1b"
    case_dir = repo / ".lto" / run_id / "plugin-eval" / "c1"
    case_dir.mkdir(parents=True)
    _init_run(repo, run_id)
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")

    # 把健康异构选择器压成 None → 对 codex candidate 选不到健康异构
    monkeypatch.setattr(
        llm_judge, "_pick_healthy_judge_runner",
        lambda repo, cr, runners_dir=None: None,
    )
    layer = llm_judge.judge_case(
        repo, run_id, case_dir,
        candidate_runner="codex", frozen=frozen, persist=False,
    )
    assert layer["status"] == "skipped"
    assert layer["reason"] == "no heterogeneous runner"


def test_judge_runs_heterogeneous(tmp_path: Path) -> None:
    """异构 judge（candidate=codex, judge=pi）正常派工并解析结构化输出。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r2"
    case_dir = repo / ".lto" / run_id / "plugin-eval" / "c1"
    case_dir.mkdir(parents=True)
    _init_run(repo, run_id)
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")

    reply = json.dumps(
        {"blocker_quality": "adequate", "false_positive_suspected": False, "rationale": "ok"}
    )
    runners = _fake_judge_runner_dir(tmp_path, "pi", reply)

    layer = llm_judge.judge_case(
        repo, run_id, case_dir,
        candidate_runner="codex", frozen=frozen,
        persist=False, runners_dir=runners, judge_runner="pi",
    )
    assert layer["status"] == "ok", layer
    assert layer["runner"] == "pi"
    assert layer["blocker_quality"] == "adequate"
    assert layer["false_positive_suspected"] is False
    assert layer["evidence_hash"] == frozen["evidence_hash"]
    assert "promote" in layer["note"].lower()


# ---------------------------------------------------------------------------
# BLOCKER1: 裁决一起冻结（judgment_hash），改裁决 hash 变
# ---------------------------------------------------------------------------

def _make_case(repo: Path, run_id: str) -> Path:
    case_dir = repo / ".lto" / run_id / "plugin-eval" / "c1"
    case_dir.mkdir(parents=True)
    _init_run(repo, run_id)
    return case_dir


def test_verdict_hash_frozen_and_changes_with_judgment(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    case_dir = _make_case(repo, "rv1")
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")

    # 裁决 A
    reply_a = json.dumps(
        {"blocker_quality": "strong", "false_positive_suspected": False, "rationale": "good"}
    )
    runners_a = _fake_judge_runner_dir(tmp_path / "ra", "pi", reply_a)
    layer_a = llm_judge.judge_case(
        repo, "rv1", case_dir, candidate_runner="codex", frozen=frozen,
        persist=False, runners_dir=runners_a, judge_runner="pi",
    )
    assert layer_a["status"] == "ok"
    assert "judgment_hash" in layer_a and layer_a["judgment_hash"].startswith("sha256:")
    # judge-verdict.json 真落盘，含裁决主体 + judgment_hash
    verdict = json.loads((case_dir / "judge-verdict.json").read_text())
    assert verdict["judgment_hash"] == layer_a["judgment_hash"]
    assert verdict["evidence_hash"] == frozen["evidence_hash"]
    assert verdict["parsed_judgment"]["blocker_quality"] == "strong"
    hash_a = layer_a["judgment_hash"]

    # 同证据、不同裁决 B → judgment_hash 必变（裁决被冻进 hash）
    repo2 = tmp_path / "repo2"
    repo2.mkdir()
    case_dir2 = _make_case(repo2, "rv1")
    frozen2 = llm_judge.freeze_evidence(case_dir2, "b", "base", "cand")
    assert frozen2["evidence_hash"] == frozen["evidence_hash"]  # 输入 hash 相同
    reply_b = json.dumps(
        {"blocker_quality": "none", "false_positive_suspected": True, "rationale": "bad"}
    )
    runners_b = _fake_judge_runner_dir(tmp_path / "rb", "pi", reply_b)
    layer_b = llm_judge.judge_case(
        repo2, "rv1", case_dir2, candidate_runner="codex", frozen=frozen2,
        persist=False, runners_dir=runners_b, judge_runner="pi",
    )
    assert layer_b["judgment_hash"] != hash_a  # 输入同、裁决不同 → judgment_hash 变


def test_verdict_hash_present_on_skip_and_fail(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    case_dir = _make_case(repo, "rv2")
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")
    # 同族 → skipped，仍冻 verdict
    layer = llm_judge.judge_case(
        repo, "rv2", case_dir, candidate_runner="codex", frozen=frozen,
        persist=False, judge_runner="codex",
    )
    assert layer["status"] == "skipped"
    assert layer["judgment_hash"].startswith("sha256:")
    assert (case_dir / "judge-verdict.json").exists()


# ---------------------------------------------------------------------------
# MEDIUM4: 异构 fallback 跳过不健康 runner
# ---------------------------------------------------------------------------

def test_fallback_skips_unhealthy_runner(tmp_path: Path) -> None:
    """pi 未装/不健康，但 agy 健康 → fallback 到 agy，不直接 failed。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    case_dir = _make_case(repo, "rf1")
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")

    runners = tmp_path / "runners"
    runners.mkdir()
    # 只有 agy 有 runner 脚本 + 健康；pi 不健康（healthcheck 只对 agy 报 OK）
    reply = json.dumps(
        {"blocker_quality": "weak", "false_positive_suspected": False, "rationale": "ok"}
    )
    fake = tmp_path / "fake_agy.py"
    fake.write_text(
        "#!/usr/bin/env python3\nimport sys\n"
        f"open(sys.argv[2],'w').write({reply!r})\nsys.exit(0)\n", encoding="utf-8"
    )
    fake.chmod(0o755)
    (runners / "agy.sh").write_text(
        f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8"
    )
    (runners / "agy.sh").chmod(0o755)
    # healthcheck：pi → ERROR，agy → OK
    (runners / "healthcheck.sh").write_text(
        '#!/usr/bin/env bash\n'
        'out="["\n'
        'for a in "$@"; do\n'
        '  [ "$a" = "--json" ] && continue\n'
        '  if [ "$a" = "agy" ]; then v=OK; else v=ERROR; fi\n'
        '  out="$out{\\"agent\\":\\"$a\\",\\"verdict\\":\\"$v\\"},"\n'
        'done\n'
        'echo "${out%,}]"\n',
        encoding="utf-8",
    )
    (runners / "healthcheck.sh").chmod(0o755)

    # candidate=codex(openai)。异构序：pi(deepseek)→不健康, agy(google)→健康
    layer = llm_judge.judge_case(
        repo, "rf1", case_dir, candidate_runner="codex", frozen=frozen,
        persist=False, runners_dir=runners,  # 不传 judge_runner → 走健康选择
    )
    assert layer["status"] == "ok", layer
    assert layer["runner"] == "agy", layer
    assert layer["blocker_quality"] == "weak"


# ---------------------------------------------------------------------------
# MEDIUM5: false_positive_suspected 严格 bool
# ---------------------------------------------------------------------------

def test_strict_bool_rejects_string_false(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    case_dir = _make_case(repo, "rb1")
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", "cand")
    # 字符串 "false" 不是 bool → 整条裁决判非法 → judge failed（不进主观层）
    reply = json.dumps(
        {"blocker_quality": "strong", "false_positive_suspected": "false", "rationale": "x"}
    )
    runners = _fake_judge_runner_dir(tmp_path, "pi", reply)
    layer = llm_judge.judge_case(
        repo, "rb1", case_dir, candidate_runner="codex", frozen=frozen,
        persist=False, runners_dir=runners, judge_runner="pi",
    )
    assert layer["status"] == "failed", layer
    assert "blocker_quality" not in layer  # 非法裁决不进主观层
    # parser 直接验证
    assert llm_judge._parse_judge_reply(reply) is None
    # 合法 bool 仍通过
    good = json.dumps(
        {"blocker_quality": "strong", "false_positive_suspected": True, "rationale": "x"}
    )
    assert llm_judge._parse_judge_reply(good)["false_positive_suspected"] is True


# ---------------------------------------------------------------------------
# MEDIUM6: judge 输入大小上限
# ---------------------------------------------------------------------------

def test_oversized_input_skips_without_dispatch(tmp_path: Path, monkeypatch) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    case_dir = _make_case(repo, "rs1")
    huge = "X" * (llm_judge._MAX_JUDGE_INPUT_BYTES + 10)
    frozen = llm_judge.freeze_evidence(case_dir, "b", "base", huge)

    # 若误派工，spawn_agents 应被调用——监控它确保没被调
    called = {"n": 0}

    def _boom(*a, **k):
        called["n"] += 1
        raise AssertionError("spawn must not be called when input oversized")

    monkeypatch.setattr(llm_judge.agent_exec, "spawn_agents", _boom)
    layer = llm_judge.judge_case(
        repo, "rs1", case_dir, candidate_runner="codex", frozen=frozen,
        persist=False, judge_runner="pi",
    )
    assert layer["status"] == "skipped", layer
    assert called["n"] == 0
    assert layer["input_bytes"] > llm_judge._MAX_JUDGE_INPUT_BYTES
    assert layer["max_input_bytes"] == llm_judge._MAX_JUDGE_INPUT_BYTES
    assert layer["judgment_hash"].startswith("sha256:")


# ---------------------------------------------------------------------------
# D. judge 层隔离：judge 报质量差 → 确定性 metrics / promote 不受影响
# ---------------------------------------------------------------------------

def _mini_plugin(root: Path) -> Path:
    pdir = root / "plugins" / "mini"
    (pdir / "profiles").mkdir(parents=True)
    (pdir / "evals").mkdir(parents=True)
    (pdir / "schemas").mkdir(parents=True)
    (pdir / "sources").mkdir(parents=True)
    (pdir / "sources" / "note.json").write_text(
        json.dumps({"id": "note.mini", "url": "https://example.com", "claims": []}), encoding="utf-8"
    )
    profile = {
        "id": "mini-profile-v1",
        "runner": "codex",
        "prompt_suffix": "OUTPUT MUST BE JSON FINDINGS.",
        "permission": {"sandbox": "read-only"},
        "output_schema_ref": "schemas/findings.json",
    }
    (pdir / "schemas" / "findings.json").write_text(json.dumps({"type": "object"}), encoding="utf-8")
    (pdir / "profiles" / "p.json").write_text(json.dumps(profile), encoding="utf-8")
    (pdir / "evals" / "cases.json").write_text(
        json.dumps(
            {
                "id": "mini-cases-v1",
                "metrics": ["parse_rate"],
                "cases": [
                    {"id": "c1", "runner": "codex", "profile": "mini-profile-v1", "brief": "Audit this."}
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
        "provides": {"profiles": ["profiles/p.json"], "evals": ["evals/cases.json"]},
        "security": {"executable_code": False, "max_sandbox": "read-only"},
        "default_enabled": False,
    }
    (pdir / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    return pdir


def _eval_runner_dir(root: Path, eval_reply: str, judge_reply: str) -> Path:
    """codex.sh 产 eval candidate/baseline reply；pi.sh 产 judge reply。"""
    runners = root / "runners"
    runners.mkdir(parents=True, exist_ok=True)
    for name, payload in (("codex", eval_reply), ("pi", judge_reply)):
        fake = root / f"fake_{name}.py"
        fake.write_text(
            "#!/usr/bin/env python3\n"
            "import sys\n"
            "reply_file = sys.argv[2]\n"
            f"open(reply_file, 'w').write({payload!r})\n"
            "sys.exit(0)\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        sh = runners / f"{name}.sh"
        sh.write_text(f'#!/usr/bin/env bash\nexec python3 "{fake}" "$@"\n', encoding="utf-8")
        sh.chmod(0o755)
    hc = runners / "healthcheck.sh"
    hc.write_text(
        '#!/usr/bin/env bash\necho \'[{"agent":"codex","verdict":"OK"},{"agent":"pi","verdict":"OK"}]\'\nexit 0\n',
        encoding="utf-8",
    )
    hc.chmod(0o755)
    return runners


def test_judge_layer_isolated_from_deterministic(tmp_path: Path) -> None:
    """judge 报 blocker_quality=none + false_positive → 确定性 metrics / case_ok 不变。"""
    repo = tmp_path / "repo"
    repo.mkdir()
    run_id = "r3"
    _init_run(repo, run_id)
    pdir = _mini_plugin(repo)
    # eval candidate 产合法 JSON（parse_ok=True, 无 leak）；judge 唱衰
    eval_reply = json.dumps({"findings": []})
    judge_reply = json.dumps(
        {"blocker_quality": "none", "false_positive_suspected": True, "rationale": "garbage"}
    )
    runners = _eval_runner_dir(tmp_path, eval_reply, judge_reply)

    report = plugin_eval_run.eval_run(
        repo, run_id, pdir, runners_dir=runners, persist=False, max_concurrency=1
    )
    case = report["cases"][0]

    # judge 唱衰但确定性层完全不受影响
    assert case["ok"] is True, case
    assert case["candidate"]["parse_ok"] is True
    assert case["candidate"]["private_path_leak"] is False
    assert case["deltas"]["candidate_new_permission_violation"] is False
    # judge 层确实标了差，且明确独立成层
    assert case["judge"]["status"] == "ok", case["judge"]
    assert case["judge"]["blocker_quality"] == "none"
    assert case["judge"]["kind"] == "subjective_judgment"
    # judge 字段绝不混进确定性 candidate/baseline metrics
    assert "blocker_quality" not in case["candidate"]
    assert "blocker_quality" not in case["baseline"]
    assert "false_positive_suspected" not in case["deltas"]
    # promote 仍 human-gated：automatic_promotion 留 deferred
    assert "automatic_promotion" in report["deferred"]
    # judge 不进确定性聚合 report.ok
    assert report["ok"] is True


def _run_all() -> int:
    import pytest
    return pytest.main([__file__, "-v"])


if __name__ == "__main__":
    sys.exit(_run_all())
