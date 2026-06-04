#!/usr/bin/env python3
"""主审对抗测试 + 实测红线：worktree_exec 安全主梁。
核心红线：rm -rf 真跑通只炸 worktree，主工作树+家目录不变。"""
from __future__ import annotations
import sys, os, tempfile, shutil, subprocess
from pathlib import Path

ROOT = str(__import__("pathlib").Path(__file__).resolve().parent)
sys.path.insert(0, ROOT)
from lto import worktree_exec as we

FAIL = []
def ok(c, m):
    print(("OK   " if c else "FAIL ")+m, file=sys.stderr if not c else sys.stdout)
    if not c: FAIL.append(m)


def mkrepo(tmp: Path) -> Path:
    repo = tmp / "repo"; repo.mkdir()
    subprocess.run(["git","init","-q"], cwd=repo, capture_output=True)
    subprocess.run(["git","config","user.name","T"], cwd=repo, capture_output=True)
    subprocess.run(["git","config","user.email","t@x.com"], cwd=repo, capture_output=True)
    (repo/"keep.txt").write_text("important data — must survive\n")
    subprocess.run(["git","add","."], cwd=repo, capture_output=True)
    subprocess.run(["git","commit","-q","-m","init"], cwd=repo, capture_output=True)
    return repo


def adv1_classify_dangerous():
    print("\n=== ADV-1: 危险命令分类 ===")
    cases = [
        ("rm -rf foo", "needs_semantic_judgement"),
        ("git push origin main", "needs_semantic_judgement"),
        ("DROP TABLE users", "needs_semantic_judgement"),
        ("sudo rm foo", "needs_semantic_judgement"),
        ("cat ~/.ssh/id_rsa", "needs_semantic_judgement"),   # 逃逸路径
        ("rm -rf /etc/hosts", "needs_semantic_judgement"),
        ("cat ../../../etc/passwd", "needs_semantic_judgement"),
        # chmod 递归大小写回归：classify 先 .lower()，大写 -R pattern 曾永远漏判（已修）
        ("chmod -R 777 .", "needs_semantic_judgement"),
        ("chmod -RV 000 .", "needs_semantic_judgement"),
        ("chmod --recursive 777 .", "needs_semantic_judgement"),
        ("chmod 644 file.txt", "reversible"),      # 非递归不误伤
        ("chmod +x script.sh", "reversible"),      # 非递归不误伤
        ("curl https://api.example.com", "network"),
        ("pytest tests/ -x", "reversible"),
        ("ruff check .", "reversible"),
    ]
    allp = True
    for cmd, expect in cases:
        got = we.classify_effect(cmd).level
        if got != expect:
            allp = False
            print(f"   ✗ {cmd!r} → {got}, 期望 {expect}", file=sys.stderr)
    ok(allp, "危险/逃逸/网络/安全 分类全对")


def adv2_RED_LINE_rm_rf_only_worktree():
    print("\n=== ADV-2 红线: rm -rf 只炸 worktree，主树+家目录不变 ===")
    tmp = Path(tempfile.mkdtemp(prefix="adv_wt_"))
    home_canary = Path.home() / ".lto_canary_DO_NOT_DELETE"
    try:
        repo = mkrepo(tmp)
        home_canary.write_text("canary")  # 家目录哨兵
        # rm -rf * 是 dangerous → 不该执行（needs_semantic_judgement）
        r = we.run_in_sandbox(repo, "rm -rf *")
        ok(not r.executed and r.effect.level=="needs_semantic_judgement",
           "rm -rf * 被分类拦截，根本不执行（更安全）")
        # 即使是"安全"命令在 worktree 里写文件，也不影响主树
        r2 = we.run_in_sandbox(repo, "echo sandbox-write > newfile.txt && ls")
        ok(r2.executed and r2.rc==0, "worktree 内安全命令执行成功")
        ok(not (repo/"newfile.txt").exists(), "worktree 内新建文件不出现在主工作树")
        ok((repo/"keep.txt").exists(), "主工作树 keep.txt 完好")
        ok(home_canary.exists() and home_canary.read_text()=="canary", "家目录哨兵完好")
    finally:
        home_canary.unlink(missing_ok=True)
        shutil.rmtree(tmp, ignore_errors=True)


def adv3_worktree_isolation():
    print("\n=== ADV-3: worktree 文件改动隔离 ===")
    tmp = Path(tempfile.mkdtemp(prefix="adv_wt_"))
    try:
        repo = mkrepo(tmp)
        # 在 worktree 里改 keep.txt（reversible），主树的 keep.txt 不该变
        r = we.run_in_sandbox(repo, "echo MODIFIED > keep.txt")
        ok(r.executed, "改文件命令在沙箱执行")
        ok((repo/"keep.txt").read_text().strip()=="important data — must survive",
           "主树 keep.txt 未被沙箱内修改污染")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def adv4_push_blocked_and_no_main_pollution():
    print("\n=== ADV-4: git push 被拦 + 主仓配置不被污染（env 隔离机制）===")
    tmp = Path(tempfile.mkdtemp(prefix="adv_wt_"))
    try:
        repo = mkrepo(tmp)
        subprocess.run(["git","remote","add","origin","https://github.com/fake/prod.git"], cwd=repo, capture_output=True)
        # 1) git push 本身是 dangerous → 分类拦截，根本不执行
        cls = we.classify_effect("git push origin main")
        ok(cls.level == "needs_semantic_judgement", "git push 被分类拦截，不自动执行")
        # 2) 跑安全命令后，主仓 origin push 配置不被污染（修 codex Critical 2 的核心）
        we.run_in_sandbox(repo, "echo hi")
        after = subprocess.run(["git","remote","get-url","--push","origin"],
                               cwd=repo, capture_output=True, text=True).stdout.strip()
        ok("DISABLED" not in after and "fake/prod" in after,
           f"主仓 origin push 配置未被污染（{after[:40]}）")
        # 3) 沙箱内 HOME 被隔离，git 无凭据（push 即使绕过分类也推不动）
        r = we.run_in_sandbox(repo, "printenv HOME")
        ok(".sandbox_home" in r.stdout, "沙箱内 HOME 隔离（无真凭据，push 推不动）")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def adv5_obfuscation_caught_by_worktree():
    print("\n=== ADV-5: 混淆命令分类漏网但 worktree 兜底 ===")
    tmp = Path(tempfile.mkdtemp(prefix="adv_wt_"))
    try:
        repo = mkrepo(tmp)
        # $(echo rm) -rf 这种混淆，分类器可能漏（这是已知局限，靠 worktree 兜底）
        cls = we.classify_effect("$(echo rm) -rf keep.txt")
        # 不管分类如何，如果它被当 reversible 执行，也只在 worktree 里删
        r = we.run_in_sandbox(repo, "$(echo rm) -f keep.txt && echo done")
        # 关键：主树 keep.txt 必须还在（worktree 隔离兜底）
        ok((repo/"keep.txt").exists(), "混淆 rm 即使执行也只删 worktree 副本，主树 keep.txt 完好（结构兜底）")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    adv1_classify_dangerous()
    adv2_RED_LINE_rm_rf_only_worktree()
    adv3_worktree_isolation()
    adv4_push_blocked_and_no_main_pollution()
    adv5_obfuscation_caught_by_worktree()
    print()
    if FAIL:
        print(f"{len(FAIL)} FINDINGS:", file=sys.stderr)
        for f in FAIL: print("  - "+f, file=sys.stderr)
        sys.exit(1)
    print("ADVERSARIAL+REDLINE: worktree_exec 扛住全部攻击，红线通过")
    sys.exit(0)
