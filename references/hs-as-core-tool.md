# hs as a core LTO tool (external-lookup discipline)

> Status: adopted 2026-07-01. Scope: how LTO-orchestrated agents look up
> external docs / API / tool capabilities. Not a code dependency — hs is an
> optional host tool; LTO never fails because hs is absent.

## Why

When a dispatched runner (or the host) needs to learn an external capability —
a CLI flag, a library API, whether a tool supports a completion hook — the
reliable path is **hs (Hybrid Search router) first, local evidence second**.

This was learned the hard way while wiring runner completion signals: an hs
web lookup for "agy" returned facts about *Google Antigravity CLI* (no official
RPC, issue #31 open), but the locally installed `agy` is a Gemini-CLI build with
a working `~/.gemini` SessionEnd hook. The web named the wrong project; only the
local `--help`/`strings`/config settled it. Raw web fetches alone would have
shipped a wrong design.

## The discipline

1. **Route external lookups through `hs`** (`hs do "..."`, `hs fetch URL`,
   `hs github docs ...`) rather than raw web fetches. hs unifies search /
   fetch / docs / source routes and avoids bot-blocked direct scrapes.
2. **Confirm against local evidence before acting**: `<tool> --help`, the
   config files it actually reads, or `strings`/source of the installed binary.
   External sources name the *wrong project* often enough that local evidence
   must decide.
3. **hs is advisory, never a gate**: `lto preflight` reports `tool:hs` as an
   informational (`advisory`) check — present ⇒ `OK`, absent ⇒ `INFO`, neither
   affects pass/fail. Work proceeds without hs; you just fall back to built-in
   search and lean harder on local verification.

## Where this shows up in the code

- `lto preflight` — advisory `tool:hs` probe (`src/commands/ops.rs`,
  `which_in_path`). Missing hs never fails preflight.
- `GOAL_CONSTRAINT_SUMMARY` (`src/dispatch_goal.rs`) — dispatched runners are
  told to route external lookups through hs, then verify locally.

## Non-goals

- LTO does not bundle, install, or version hs.
- LTO does not call hs from core control logic — the discipline is guidance to
  the host/runner, not an automated fetch pipeline inside LTO.
