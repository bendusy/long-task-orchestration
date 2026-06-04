"""lto plugin — validate/list/mount data-only path plugins."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .. import artifacts as af
from .. import plugins
from .. import state as st


def run(args: argparse.Namespace) -> int:
    action = getattr(args, "plugin_action", "")
    if action == "validate":
        return _validate(args)
    if action == "list":
        return _list(args)
    if action == "mount":
        return _mount(args)
    raise SystemExit(f"unknown plugin action: {action}")


def _validate(args: argparse.Namespace) -> int:
    result = plugins.validate_plugin(args.plugin_dir)
    if args.json:
        print(json.dumps(result.to_dict(), ensure_ascii=False, indent=2, sort_keys=True))
    else:
        status = "OK" if result.ok else "FAIL"
        print(f"{status} {args.plugin_dir}")
        if result.plugin_id:
            print(f"id: {result.plugin_id}")
        if result.manifest_hash:
            print(f"manifest_hash: {result.manifest_hash[:19]}...")
        for w in result.warnings:
            print(f"warning: {w}", file=sys.stderr)
        for e in result.errors:
            print(f"error: {e}", file=sys.stderr)
    return 0 if result.ok else 2


def _list(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    rows = []
    for path in plugins.discover_plugins(repo):
        v = plugins.validate_plugin(path)
        stage = (v.manifest or {}).get("stage") if v.manifest else "?"
        version = (v.manifest or {}).get("version") if v.manifest else "?"
        rows.append({
            "id": v.plugin_id or path.name,
            "version": version,
            "stage": stage,
            "ok": v.ok,
            "path": str(path.relative_to(repo)) if _is_relative_to(path, repo) else str(path),
            "errors": v.errors,
        })
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        if not rows:
            print("no plugins found")
            return 0
        print(f"{'ID':<32} {'VERSION':<10} {'STAGE':<12} {'OK':<3} PATH")
        for r in rows:
            print(f"{r['id']:<32} {str(r['version']):<10} {str(r['stage']):<12} {str(r['ok']):<3} {r['path']}")
    return 0


def _mount(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    run_id = st.resolve_run_id(repo, args.run_id)
    try:
        entry = plugins.mount_plugin(repo, args.plugin_dir, run_id, approved_by=args.approved_by)
    except ValueError as exc:
        print(f"plugin mount failed: {exc}", file=sys.stderr)
        return 2

    lock_path = plugins.mount_lock_path(repo, run_id)
    state = st.load_state(repo / ".lto" / run_id / "state.json") or {}
    af.register_path(
        repo,
        run_id,
        lock_path,
        kind="other",
        producer="lto.commands.plugin",
        state=state,
        summary=f"plugin mount lock: {entry.get('plugin_id')}@{entry.get('plugin_version')}",
        tags=["plugin", "mount-lock"],
    )
    if args.json:
        print(json.dumps(entry, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        print(f"mounted {entry.get('plugin_id')}@{entry.get('plugin_version')} for {run_id}")
        print(f"lock: {lock_path.relative_to(repo)}")
    return 0


def _is_relative_to(path: Path, parent: Path) -> bool:
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def add_parser(subparsers) -> None:
    p = subparsers.add_parser("plugin", help="validate/list/mount data-only LTO plugins")
    plug = p.add_subparsers(dest="plugin_action", required=True)

    validate = plug.add_parser("validate", help="validate a plugin directory")
    validate.add_argument("plugin_dir", type=Path)
    validate.add_argument("--json", action="store_true")
    validate.set_defaults(func=run)

    list_cmd = plug.add_parser("list", help="list plugins under plugins/ and .lto/plugins/")
    list_cmd.add_argument("--json", action="store_true")
    list_cmd.set_defaults(func=run)

    mount = plug.add_parser("mount", help="validate and record a run-scoped plugin mount lock")
    mount.add_argument("plugin_dir", type=Path)
    mount.add_argument("--run-id")
    mount.add_argument("--approved-by", default="host")
    mount.add_argument("--json", action="store_true")
    mount.set_defaults(func=run)
