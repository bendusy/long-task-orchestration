"""Terminal status contract for lto autopilot."""

from __future__ import annotations

import json
from enum import Enum


class AutopilotStatus(str, Enum):
    DONE = "done"
    NEEDS_CONFIRM = "needs_confirm"
    NEEDS_HOST = "needs_host"
    NEEDS_HUMAN = "needs_human"
    STALLED = "stalled"
    BUDGET_EXHAUSTED = "budget_exhausted"
    ERROR = "error"


EXIT_CODES = {
    AutopilotStatus.DONE: 0,
    AutopilotStatus.NEEDS_HOST: 10,
    AutopilotStatus.NEEDS_CONFIRM: 11,
    AutopilotStatus.NEEDS_HUMAN: 12,
    AutopilotStatus.STALLED: 20,
    AutopilotStatus.BUDGET_EXHAUSTED: 21,
    AutopilotStatus.ERROR: 30,
}

STATUS_PRIORITY = {
    AutopilotStatus.DONE: 0,
    AutopilotStatus.NEEDS_CONFIRM: 10,
    AutopilotStatus.NEEDS_HOST: 20,
    AutopilotStatus.NEEDS_HUMAN: 30,
    AutopilotStatus.STALLED: 40,
    AutopilotStatus.BUDGET_EXHAUSTED: 50,
    AutopilotStatus.ERROR: 60,
}


def stronger_status(a: AutopilotStatus, b: AutopilotStatus) -> AutopilotStatus:
    return a if STATUS_PRIORITY[a] >= STATUS_PRIORITY[b] else b


def emit_terminal_status(status: AutopilotStatus, reason: str) -> None:
    payload = {
        "status": status.value,
        "exit_code": EXIT_CODES[status],
        "reason": reason,
    }
    print()
    print("[lto autopilot terminal]")
    print(json.dumps(payload, ensure_ascii=False, sort_keys=True))
