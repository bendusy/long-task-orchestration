"""Aggregated audit self-test entrypoint."""

from __future__ import annotations

import sys

from lto import test_audit_dispatch, test_audit_parse


def _run_selftest() -> int:
    passed = 0
    total = 0
    for runner in (test_audit_parse.run, test_audit_dispatch.run):
        got, count = runner()
        passed += got
        total += count
    print(f"\nResults: {passed}/{total} passed")
    if passed == total:
        print("AUDIT SELFTEST OK")
        return 0
    print(f"AUDIT SELFTEST FAILED ({total - passed} failures)")
    return 1


if __name__ == "__main__":
    sys.exit(_run_selftest())
