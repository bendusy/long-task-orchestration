# LTO：长任务的导航仪

> 做一个大功能要几十轮对话。LTO 是帮你**不迷路、不做过头、知道什么时候停**的导航仪。

LTO 不替你写代码。它告诉你六件事：
1. **该不该做**——缺的东西现在真需要吗？答不上就停
2. **写方案**——把可能出问题的地方标出来
3. **让不同 AI 审**——用跟你不一样的 AI 来找盲区
4. **写代码**——不同模块同时写，写完同样审
5. **部署上线**——按顺序走，真的测过新功能能用吗？
6. **记下来**——踩了什么坑、做了什么决定

## 快速开始

```bash
# 开一个长任务
python3 scripts/lto_run.py start --goal "做用户登录"

# 续接（上次 compact 之后）
python3 scripts/lto_run.py check

# 完成
python3 scripts/lto_run.py closeout --summary "做了什么，验证了什么"
```

## 让不同 AI 帮你审

```bash
# 找跟你不一样的 AI 来审（你用 DeepSeek 就让 GPT 和 Gemini 审）
AD=~/Projects/agent-skills/skills/agent-delegate/scripts/runners
$AD/codex.sh  方案.md 回复-codex.md  300 &
$AD/agy.sh    方案.md 回复-agy.md    300 &
$AD/claude.sh 方案.md 回复-claude.md 300 &
wait
```

## 什么情况不要用

| 你要做 | 用这个 |
|---|---|
| 修个 bug | diagnose |
| 让人审代码 | review |
| 部署上线 | ship |

## 安装

把整个 `long-task-orchestration/` 文件夹放到你的 agent skills 目录里就行。详见 [INSTALL.md](./INSTALL.md)。

## 更多

- [SKILL.md](./SKILL.md) — 完整导航手册
- [references/sharing-guide.md](./references/sharing-guide.md) — 怎么装依赖、怎么给朋友用
- [references/cross-runtime-host-notes.md](./references/cross-runtime-host-notes.md) — 不同 AI 工具的具体用法
