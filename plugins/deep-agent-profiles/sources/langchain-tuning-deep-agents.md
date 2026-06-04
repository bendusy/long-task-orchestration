# Source note: LangChain tuning deep agents

URL: https://www.langchain.com/blog/tuning-deep-agents-different-models
Captured: 2026-06-04

## Useful idea

A single generic harness underperforms model-specific harness profiles. Profiles can vary prompt suffix/prefix, tool names, middleware, subagent config, and skills.

## LTO adaptation

LTO should not become LangChain. LTO can use this as an experimental path plugin:

- Codex audit profile: read-only, batch file/resource planning, structured JSON findings.
- Claude audit profile: tool-result reflection, direct file/test/source observation.
- Pi profile: longer thinking timeout, explicit no-stale-context rule.
- Agy profile: read-only, full-answer output contract, no pointer-only replies.

## Boundary

These are hypotheses until evaluated. Plugin may not grant write permission or change convergence logic.
