"""worktree_exec.py — autopilot 自动执行的安全沙箱内核（四件套）。

三方两轮复核的安全主梁。autopilot 自动执行命令的唯一入口。

四件套：
1. 真建 git worktree 执行路径（命令在隔离副本里跑，炸了只炸 worktree）。
2. cwd 锁定到 worktree + 绝对路径逃逸静态扫描。
3. 绝对路径逃逸/危险操作 → needs_semantic_judgement（不执行，回吐人）。
4. 剪断 worktree 的 origin push（git push 永远不会自动推生产）。

安全认识（三方修正）：
- git worktree 不是 OS chroot——锁不住绝对路径逃逸，所以必须叠加路径扫描。
- effect_class 是「升级依据」不是「放行依据」：命中危险→升级 needs_confirm；
  命中 safe/reversible 不等于放行，仍靠 worktree 隔离兜底。
- 文本分类器对混淆（$(echo rm)/base64|sh）零防御——所以放行靠 worktree
  结构隔离，分类器只负责「能识别的危险直接拦」。

纯标准库。
"""

from __future__ import annotations

import re
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path


# ── effect_class（升级依据，不是放行依据）──

# 需人类语义判断的不可逆操作（命中即 needs_semantic_judgement，不随档位放开）
# 注意：分类器是「升级依据」——能识别的危险直接拦；识别不出的靠 worktree+env 隔离兜底。
_DANGEROUS_PATTERNS = [
    r"\brm\s+-[a-z]*[rf]",             # rm -rf / rm -fr / rm -r 等
    r"\bfind\b.*-delete",             # find . -delete（codex Medium）
    r"\bshred\b",                     # shred（codex Medium）
    # 推远程 / 改 remote / 危险 reset/clean（永远 NEEDS_CONFIRM）。
    # `(\s+\S+)*?` 非贪婪吃掉 git 与子命令间的任意全局 flag 及其值（含
    # `-C .` 这种 flag+空格+值的两 token 形式），堵 `git -C . push` /
    # `git -c k=v push` / `git --git-dir=… push`——codex 审 ③ BLOCKER：
    # 只匹配字面 `git push` 或只吃单 token flag 都会被绕过。
    r"\bgit\b(\s+\S+)*?\s+push\b",
    r"\bgit\b(\s+\S+)*?\s+remote\s+(add|set-url|remove)",
    r"\bgit\b(\s+\S+)*?\s+reset\s+--hard\b",
    r"\bgit\b(\s+\S+)*?\s+clean\s+-[a-z]*[fdx]",
    r"\bdrop\s+(database|table)\b",   # SQL DROP
    r"\bdelete\s+from\b",             # SQL DELETE
    r"\btruncate\b",
    r"\bchmod\s+(-[a-z]*r[a-z]*|--recursive)\b", r"\bchmod\s+0*00\b",  # codex Medium；-R/--recursive 递归。注：cmd 已 .lower()，故写小写
    r"\b(sudo|doas)\b",               # 提权
    r":\(\)\s*\{.*\}",                # fork bomb
    r"\bmkfs\b|\bdd\s+if=",           # 磁盘操作
    r">\s*/dev/sd",                   # 写裸设备
    # curl|sh / wget|sh = 远程代码执行（codex High）：管道给 shell 解释器
    r"\b(curl|wget)\b.*\|\s*(sudo\s+)?(ba|z|da|k|c|tc|fi)?sh\b",
    r"\b(curl|wget)\b.*\|\s*(python|perl|ruby|node)\b",
    # 解释器 -c 内联执行（堵 python -c "chr(47)..." 这类字符拼接混淆绕过路径扫描，
    # codex 漏网点）。autopilot 自动跑场景下，内联代码无法静态审计 → 一律升级。
    r"\b(python[0-9.]*|php)\s+-[A-Za-z]*c\b",        # python/php -c
    r"\b(perl|ruby|node|deno|bun)\s+-[A-Za-z]*e\b",  # perl/ruby/node/deno/bun -e
    r"\beval\b", r"\bexec\b",
    r"\bbase64\b\s+-?-?d",             # base64 解码（常配合 |sh）
    # 执行脚本文件：脚本内容不经分类器静态审计（codex 攻击#1 生成脚本逃逸）。
    # autopilot 自动跑场景下，跑不可见内容的脚本一律升级人确认。
    r"\b(ba|z|da|k|c|tc|fi)?sh\s+[^|&;]*\.sh\b",
    r"\bsource\b", r"^\s*\.\s+\S",     # source / . script
]

# 绝对路径逃逸：命令引用 worktree 外的任何绝对路径（codex Critical 1：原来漏了
# /Users /private 等真实路径）。策略改为：任何绝对路径 + 父目录穿越 + HOME 变量都升级。
_ESCAPE_PATH_PATTERNS = [
    r"(^|\s|=|['\"(])~/",              # 家目录 ~/
    r"(^|\s|=|['\"(])/[A-Za-z]",      # 任何绝对路径 /xxx（含 /Users /private /etc ...）
    r"(^|\s|=|['\"(])\.\./",          # 父目录穿越 ../
    r"\$HOME\b", r"\$\{HOME\}",
    r"\bcd\s+/",                      # cd 到绝对路径（codex Critical 1 逃逸手法）
    r"\bln\s+-s\b",                   # 符号链接（codex High 逃逸手法）
]

# 网络类（用户接受放开 curl/调 API；但 push / curl|sh 已在 dangerous 单列）
_NETWORK_PATTERNS = [r"\bcurl\b", r"\bwget\b", r"\bnc\b", r"\bssh\b", r"\bscp\b"]


@dataclass
class EffectClass:
    level: str          # safe | reversible | network | needs_semantic_judgement
    reason: str


def classify_effect(command: str) -> EffectClass:
    """分类命令的副作用等级。命中危险/逃逸 → needs_semantic_judgement。

    这是升级依据：能识别的危险直接拦；识别不出的（混淆）靠 worktree 隔离兜底。
    """
    cmd = command.lower()

    # ⚠️ cmd 已小写化——_DANGEROUS_PATTERNS / _NETWORK_PATTERNS 里的字母必须全小写，
    # 写大写（如 -R）会永远匹配不到。需大小写敏感的（路径）用下方原始 command 匹配。
    for pat in _DANGEROUS_PATTERNS:
        if re.search(pat, cmd):
            return EffectClass("needs_semantic_judgement", f"dangerous pattern: {pat}")

    for pat in _ESCAPE_PATH_PATTERNS:
        if re.search(pat, command):  # 路径用原始大小写匹配
            return EffectClass("needs_semantic_judgement", f"absolute/escape path: {pat}")

    for pat in _NETWORK_PATTERNS:
        if re.search(pat, cmd):
            return EffectClass("network", f"network op: {pat}")

    # 识别不出危险的 → 标 reversible，但执行仍在 worktree 沙箱里（结构兜底）
    return EffectClass("reversible", "no recognized dangerous/escape/network pattern")


# ── worktree 沙箱执行 ──

@dataclass
class SandboxResult:
    executed: bool          # 是否真执行了（needs_semantic_judgement 时为 False）
    effect: EffectClass
    rc: int | None = None
    stdout: str = ""
    stderr: str = ""
    worktree: str | None = None
    note: str = ""


def run_in_sandbox(
    repo: Path,
    command: str,
    *,
    timeout: int = 300,
    allow_network: bool = True,
) -> SandboxResult:
    """在 worktree 沙箱里执行命令。四件套全套。

    - needs_semantic_judgement → 不执行，返回 executed=False（autopilot 据此 NEEDS_CONFIRM）。
    - network 且 allow_network=False → 不执行。
    - 其余 → 建 worktree、剪断 origin push、cwd 锁 worktree 内执行、回收。
    """
    repo = Path(repo).resolve()
    effect = classify_effect(command)

    if effect.level == "needs_semantic_judgement":
        return SandboxResult(executed=False, effect=effect,
                             note=f"refused (needs human): {effect.reason}")

    if effect.level == "network" and not allow_network:
        return SandboxResult(executed=False, effect=effect,
                             note="refused: network disabled")

    if not _is_git_repo(repo):
        return SandboxResult(executed=False, effect=effect,
                             note="refused: not a git worktree (sandbox requires git)")

    wt_parent = Path(tempfile.mkdtemp(prefix="lto_wt_"))
    wt_dir = wt_parent / "wt"
    try:
        # 四件套-1：真建 worktree（detached，不占分支名）
        add = subprocess.run(
            ["git", "worktree", "add", "--detach", str(wt_dir), "HEAD"],
            cwd=repo, capture_output=True, text=True,
        )
        if add.returncode != 0:
            return SandboxResult(executed=False, effect=effect,
                                 note=f"worktree add failed: {(add.stderr or add.stdout).strip()[:200]}")

        # 四件套-4：禁推 + 凭据隔离（修 codex Critical 2：不改共享 .git/config
        # 污染主仓，改用执行时的环境隔离——没凭据 + 禁交互，git push 推不动）。
        env = _sandboxed_env(wt_dir)

        # 四件套-2：cwd 锁 worktree 内执行（env 隔离 HOME/git 凭据）
        try:
            proc = subprocess.run(
                command, shell=True, cwd=wt_dir, env=env,
                capture_output=True, text=True, timeout=timeout,
            )
            return SandboxResult(
                executed=True, effect=effect, rc=proc.returncode,
                stdout=proc.stdout, stderr=proc.stderr, worktree=str(wt_dir),
                note="executed in worktree sandbox (env-isolated)",
            )
        except subprocess.TimeoutExpired:
            return SandboxResult(executed=True, effect=effect, rc=124,
                                 worktree=str(wt_dir), note=f"timeout after {timeout}s")
    finally:
        # 回收 worktree（修 codex Medium：失败则 prune 清 stale entry）
        rm = subprocess.run(["git", "worktree", "remove", "--force", str(wt_dir)],
                            cwd=repo, capture_output=True)
        if rm.returncode != 0:
            subprocess.run(["git", "worktree", "prune"], cwd=repo, capture_output=True)
        shutil.rmtree(wt_parent, ignore_errors=True)


def _sandboxed_env(wt_dir: Path) -> dict[str, str]:
    """构造隔离的执行环境（修 Critical 2 + 凭据外泄）。

    - HOME 指向 worktree 内的空目录：命令读 ~/.ssh/~/.config 读不到真东西，
      git 也找不到全局 config / 凭据，push 自然推不动。
    - GIT_TERMINAL_PROMPT=0 / GIT_ASKPASS=true：git 不会交互要密码，无凭据即失败。
    - 清掉可能携带凭据的变量。
    """
    import os
    fake_home = wt_dir / ".sandbox_home"
    fake_home.mkdir(exist_ok=True)
    env = dict(os.environ)
    env["HOME"] = str(fake_home)
    env["GIT_TERMINAL_PROMPT"] = "0"
    env["GIT_ASKPASS"] = "true"
    env["GIT_CONFIG_GLOBAL"] = str(fake_home / "gitconfig-none")  # 不存在 = 空全局配置
    env["GIT_CONFIG_SYSTEM"] = "/dev/null"
    for k in ("GITHUB_TOKEN", "GH_TOKEN", "GIT_TOKEN", "AWS_SECRET_ACCESS_KEY",
              "SSH_AUTH_SOCK", "GIT_SSH_COMMAND"):
        env.pop(k, None)
    return env


def _is_git_repo(repo: Path) -> bool:
    r = subprocess.run(["git", "rev-parse", "--is-inside-work-tree"],
                       cwd=repo, capture_output=True, text=True)
    return r.returncode == 0 and r.stdout.strip() == "true"
