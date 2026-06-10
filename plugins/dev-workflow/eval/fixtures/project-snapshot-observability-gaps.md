# Project snapshot — tidepool CLI (FROZEN SYNTHETIC FIXTURE)

> This file is a frozen, fully fictional eval fixture for the dev-workflow plugin.
> The project ("tidepool") and every detail below are invented for testing.
> Do not fix or update this file; eval cases depend on its exact content,
> including its deliberately planted observability gaps.

## Overview

`tidepool` is a fictional command-line tool that syncs bookmark collections
between a local SQLite store and a remote endpoint. Single maintainer, ~3k lines
of Python, packaged with a `pyproject.toml`.

## Module layout

- `tidepool/sync.py` — diff computation and push/pull engine.
- `tidepool/store.py` — SQLite persistence layer.
- `tidepool/remote.py` — HTTP client with retry/backoff.
- `tidepool/cli.py` — argument parsing; subcommands `pull`, `push`, `diff`.
- `tests/` — 41 unit tests, run via `pytest`; CI runs lint + tests on every push.

## Runtime behavior

- On errors, modules print free-form messages to stderr, e.g.
  `print(f"sync failed: {exc}", file=sys.stderr)`; there is no log file and no
  structured event stream. Verbosity is a single `--verbose` flag that adds more
  prose to stderr.
- There is no `doctor` or `healthcheck` subcommand. The README's troubleshooting
  section tells users to "re-run with --verbose and read the output".
- There is no way to ask the tool what failed recently: no failure-query
  subcommand, no stats, no persisted error history. After a crash, the only
  record is whatever scrolled past in the terminal.
- Sync conflicts are written to `conflicts.txt` as free-form prose paragraphs
  appended in arbitrary order.

## Documentation

- `README.md` covers install, the three subcommands, and a FAQ.
- No schema documentation exists for any output the tool produces.
