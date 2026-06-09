"""LTO 命令包。"""

from __future__ import annotations

import warnings
from pathlib import Path
from typing import Any


def safe_emit(repo: Path, run_id: str, **kwargs: Any) -> dict[str, Any] | None:
    """Fail-safe Phase 1 event emit for integration points.

    The sensor layer must never break the plant. ``events`` is imported lazily
    *inside* this function so that a broken/missing events.py cannot crash the
    core commands (start/closeout/runner/task_add) at their module-import time
    (review #2). Any failure — import error, bad run id, disk error, size
    hard-stop — is swallowed into a warning and returns None.
    """
    try:
        from . import events as _ev
        return _ev.emit(repo, run_id, **kwargs)
    except (Exception, SystemExit) as exc:  # SystemExit: validate_run_id raises it
        warnings.warn(f"safe_emit failed ({kwargs.get('type', '?')}): {exc}")
        return None
