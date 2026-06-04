"""Data-only plugin validation and mount locks for LTO.

Plugin v0 is deliberately non-executable: JSON manifests, Markdown prompts,
JSON schemas/eval packs, and source notes only. Plugins compile into existing
LTO contracts outside this module; validate/mount only records provenance.
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import state as st

PLUGIN_SCHEMA_VERSION = 1
ALLOWED_STAGE = {"experimental", "blessed", "rejected"}
ALLOWED_KIND = {"path-plugin"}
ALLOWED_SANDBOX = {"read-only", "workspace-write", "danger-full-access"}
ENV_KEY_RE = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{1,80}$")
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9._-]+)?$")


@dataclass
class PluginValidation:
    ok: bool
    plugin_id: str | None
    errors: list[str]
    warnings: list[str]
    manifest: dict[str, Any] | None = None
    manifest_hash: str = ""

    def to_dict(self) -> dict[str, Any]:
        return {
            "ok": self.ok,
            "plugin_id": self.plugin_id,
            "errors": self.errors,
            "warnings": self.warnings,
            "manifest_hash": self.manifest_hash,
        }


def discover_plugins(repo: Path) -> list[Path]:
    """Return plugin directories under repo/plugins and repo/.lto/plugins."""
    repo = repo.resolve()
    roots = [repo / "plugins", repo / ".lto" / "plugins"]
    out: list[Path] = []
    for root in roots:
        if not root.exists():
            continue
        for child in sorted(root.iterdir()):
            if child.is_dir() and (child / "plugin.json").exists():
                out.append(child)
    return out


def validate_plugin(plugin_dir: Path) -> PluginValidation:
    plugin_dir = plugin_dir.resolve()
    manifest_path = plugin_dir / "plugin.json"
    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    manifest_hash = ""

    if not manifest_path.exists():
        return PluginValidation(False, None, [f"missing plugin.json: {manifest_path}"], [])

    try:
        raw = manifest_path.read_text(encoding="utf-8")
        manifest = json.loads(raw)
        manifest_hash = "sha256:" + hashlib.sha256(raw.encode("utf-8")).hexdigest()
    except json.JSONDecodeError as exc:
        return PluginValidation(False, None, [f"plugin.json invalid JSON: {exc}"], [])

    if not isinstance(manifest, dict):
        return PluginValidation(False, None, ["plugin.json must be an object"], [])

    plugin_id = _str(manifest.get("id"))
    if not plugin_id or not ID_RE.match(plugin_id):
        errors.append("id must match ^[a-z0-9][a-z0-9._-]{1,80}$")

    version = _str(manifest.get("version"))
    if not version or not VERSION_RE.match(version):
        errors.append("version must be semver-like, e.g. 0.1.0")

    stage = _str(manifest.get("stage"))
    if stage not in ALLOWED_STAGE:
        errors.append(f"stage must be one of {sorted(ALLOWED_STAGE)}")
    if stage == "rejected" and not _str(manifest.get("rejection_reason")):
        warnings.append("rejected plugin should include rejection_reason")

    kind = _str(manifest.get("kind"))
    if kind not in ALLOWED_KIND:
        errors.append(f"kind must be one of {sorted(ALLOWED_KIND)}")

    security = manifest.get("security")
    if not isinstance(security, dict):
        errors.append("security must be an object")
        security = {}
    if security.get("executable_code") is not False:
        errors.append("security.executable_code must be false in plugin v0")
    max_sandbox = _str(security.get("max_sandbox", "read-only"))
    if max_sandbox not in ALLOWED_SANDBOX:
        errors.append(f"security.max_sandbox must be one of {sorted(ALLOWED_SANDBOX)}")
    for key in security.get("env_allowlist", []) or []:
        if not isinstance(key, str) or not ENV_KEY_RE.match(key):
            errors.append(f"invalid env_allowlist key: {key!r}")

    source_notes = manifest.get("source_notes")
    if not isinstance(source_notes, list) or not source_notes:
        errors.append("source_notes must be a non-empty list")
    else:
        for rel in source_notes:
            _validate_rel_file(plugin_dir, rel, errors, must_parse_json=rel.endswith(".json") if isinstance(rel, str) else False)

    provides = manifest.get("provides")
    if not isinstance(provides, dict):
        errors.append("provides must be an object")
        provides = {}
    for section in ("paths", "profiles", "evals"):
        values = provides.get(section, []) or []
        if not isinstance(values, list):
            errors.append(f"provides.{section} must be a list")
            continue
        for rel in values:
            _validate_rel_file(plugin_dir, rel, errors, must_parse_json=rel.endswith(".json") if isinstance(rel, str) else False)

    for rel in _all_declared_refs(manifest):
        if isinstance(rel, str) and rel.endswith((".py", ".sh", ".js", ".ts")):
            errors.append(f"executable plugin files are not allowed in v0: {rel}")

    return PluginValidation(not errors, plugin_id, errors, warnings, manifest, manifest_hash)


def mount_plugin(repo: Path, plugin_dir: Path, run_id: str, *, approved_by: str = "host") -> dict[str, Any]:
    repo = repo.resolve()
    run_id = st.validate_run_id(run_id)
    validation = validate_plugin(plugin_dir)
    if not validation.ok or validation.manifest is None:
        raise ValueError("plugin validation failed: " + "; ".join(validation.errors))

    manifest = validation.manifest
    security = manifest.get("security", {}) or {}
    entry = {
        "schema_version": PLUGIN_SCHEMA_VERSION,
        "mounted_at": st.iso_now(),
        "run_id": run_id,
        "plugin_id": manifest.get("id"),
        "plugin_version": manifest.get("version"),
        "stage": manifest.get("stage"),
        "kind": manifest.get("kind"),
        "plugin_path": _display_path(repo, plugin_dir.resolve()),
        "manifest_hash": validation.manifest_hash,
        "source_notes": list(manifest.get("source_notes", [])),
        "provides": manifest.get("provides", {}),
        "approved_permissions": {
            "max_sandbox": security.get("max_sandbox", "read-only"),
            "requires_human_approval_for": list(security.get("requires_human_approval_for", []) or []),
            "approved_by": approved_by,
        },
        "default_enabled": bool(manifest.get("default_enabled", False)),
    }

    lock_path = mount_lock_path(repo, run_id)
    data = _load_mount_lock(lock_path, run_id)
    mounts = data.setdefault("mounts", [])
    mounts[:] = [m for m in mounts if m.get("plugin_id") != entry["plugin_id"]]
    mounts.append(entry)
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock_path.write_text(json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return entry


def mount_lock_path(repo: Path, run_id: str) -> Path:
    return repo.resolve() / ".lto" / st.validate_run_id(run_id) / "plugin-mounts.json"


def _load_mount_lock(path: Path, run_id: str) -> dict[str, Any]:
    if path.exists():
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                data.setdefault("schema_version", PLUGIN_SCHEMA_VERSION)
                data.setdefault("run_id", st.validate_run_id(run_id))
                data.setdefault("mounts", [])
                return data
        except json.JSONDecodeError:
            pass
    return {"schema_version": PLUGIN_SCHEMA_VERSION, "run_id": st.validate_run_id(run_id), "mounts": []}


def _validate_rel_file(plugin_dir: Path, rel: Any, errors: list[str], *, must_parse_json: bool) -> None:
    if not isinstance(rel, str) or not rel:
        errors.append(f"declared path must be non-empty string: {rel!r}")
        return
    if rel.startswith("/") or ".." in Path(rel).parts:
        errors.append(f"declared path must stay inside plugin dir: {rel!r}")
        return
    path = plugin_dir / rel
    if not path.exists() or not path.is_file():
        errors.append(f"declared file missing: {rel}")
        return
    if must_parse_json:
        try:
            json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            errors.append(f"declared JSON invalid: {rel}: {exc}")


def _all_declared_refs(manifest: dict[str, Any]) -> list[Any]:
    refs: list[Any] = []
    refs.extend(manifest.get("source_notes", []) or [])
    provides = manifest.get("provides", {}) or {}
    if isinstance(provides, dict):
        for values in provides.values():
            if isinstance(values, list):
                refs.extend(values)
    return refs


def _str(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _display_path(repo: Path, path: Path) -> str:
    try:
        return str(path.relative_to(repo))
    except ValueError:
        return str(path)
