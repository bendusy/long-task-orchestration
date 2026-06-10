"""Profile rendering, source-note creation, and static eval helpers for plugins."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

from . import plugins as core
from . import state as st

def load_profile(plugin_dir: Path, profile_id: str) -> dict[str, Any]:
    validation = core.validate_plugin(plugin_dir)
    if not validation.ok or validation.manifest is None:
        raise ValueError("plugin validation failed: " + "; ".join(validation.errors))
    for rel in (validation.manifest.get("provides", {}) or {}).get("profiles", []) or []:
        data = json.loads((plugin_dir.resolve() / rel).read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            continue
        if data.get("id") == profile_id:
            data["_relative_path"] = rel
            return data
    raise ValueError(f"profile not found: {profile_id}")


def render_profile(plugin_dir: Path, profile_id: str, input_path: Path, output_path: Path) -> dict[str, Any]:
    """Render a profile prompt by appending prompt_suffix/ref to input brief."""
    plugin_dir = plugin_dir.resolve()
    profile = load_profile(plugin_dir, profile_id)
    base = input_path.read_text(encoding="utf-8")
    chunks = [base.rstrip(), ""]
    suffix_ref = profile.get("prompt_suffix_ref")
    if suffix_ref:
        _ensure_rel_path(suffix_ref)
        chunks.extend(["", "# LTO plugin profile instructions", (plugin_dir / suffix_ref).read_text(encoding="utf-8").rstrip()])
    suffix = profile.get("prompt_suffix")
    if isinstance(suffix, str) and suffix.strip():
        chunks.extend(["", "# LTO plugin profile instructions", suffix.strip()])
    rendered = "\n".join(chunks).rstrip() + "\n"
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(rendered, encoding="utf-8")
    return {
        "profile_id": profile_id,
        "profile_path": profile.get("_relative_path"),
        "input": str(input_path),
        "output": str(output_path),
        "rendered_bytes": len(rendered.encode("utf-8")),
        "prompt_suffix_ref": suffix_ref,
        "output_schema_ref": profile.get("output_schema_ref"),
        "env_keys": sorted((profile.get("env") or {}).keys()),
        "permission": profile.get("permission", {}),
    }


def static_eval(plugin_dir: Path, eval_id: str | None = None) -> dict[str, Any]:
    """Run static eval-pack checks: refs, profiles, metrics, safety ceilings."""
    plugin_dir = plugin_dir.resolve()
    validation = core.validate_plugin(plugin_dir)
    report: dict[str, Any] = {
        "plugin_dir": str(plugin_dir),
        "validation": validation.to_dict(),
        "evals": [],
        "ok": validation.ok,
        "errors": list(validation.errors),
    }
    if not validation.ok or validation.manifest is None:
        return report
    manifest = validation.manifest
    profiles = _load_declared_profiles(plugin_dir, manifest, errors=report["errors"])
    profile_ids = {p.get("id") for p in profiles}
    required_safety_metrics = {"permission_violations", "private_path_leaks"}
    for rel in (manifest.get("provides", {}) or {}).get("evals", []) or []:
        errors: list[str] = []
        try:
            data = json.loads((plugin_dir / rel).read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            report["errors"].append(f"eval invalid JSON: {rel}: {exc}")
            continue
        if not isinstance(data, dict):
            report["errors"].append(f"eval must be an object: {rel}")
            continue
        if eval_id and data.get("id") != eval_id:
            continue
        cases = data.get("cases", []) or []
        if not isinstance(cases, list) or not cases:
            errors.append(f"eval {data.get('id')} must include non-empty cases list")
            cases = []
        seen_case_ids: set[str] = set()
        for case in cases:
            if not isinstance(case, dict):
                errors.append(f"eval {data.get('id')} case must be an object")
                continue
            cid = case.get("id")
            if not cid or cid in seen_case_ids:
                errors.append(f"eval {data.get('id')} case id missing or duplicate: {cid}")
            seen_case_ids.add(str(cid))
            profile = case.get("profile")
            if profile and profile not in profile_ids:
                errors.append(f"case {case.get('id')} references unknown profile {profile}")
        metrics = data.get("metrics", []) or []
        if not isinstance(metrics, list) or not metrics:
            errors.append(f"eval {data.get('id')} has no metrics")
            metrics = []
        missing_safety = sorted(required_safety_metrics - set(metrics))
        if missing_safety:
            errors.append(f"eval {data.get('id')} missing required safety metrics: {missing_safety}")
        if int(data.get("safety_regressions_allowed", 0)) != 0:
            errors.append(f"eval {data.get('id')} must set safety_regressions_allowed=0")
        if int(data.get("minimum_runs_before_promotion", 0)) < 1:
            errors.append(f"eval {data.get('id')} must set minimum_runs_before_promotion >= 1")
        report["evals"].append({
            "id": data.get("id"),
            "path": rel,
            "cases": len(cases),
            "metrics": metrics,
            "ok": not errors,
            "errors": errors,
        })
        report["errors"].extend(errors)
    if eval_id and not report["evals"]:
        report["errors"].append(f"eval not found: {eval_id}")
    report["ok"] = report["ok"] and not report["errors"]
    return report


def create_source_note(
    plugin_dir: Path,
    *,
    note_id: str,
    title: str,
    url: str,
    claims: list[str],
    hypotheses: list[str],
    append_manifest: bool = False,
) -> Path:
    plugin_dir = plugin_dir.resolve()
    if not core.ID_RE.match(note_id):
        raise ValueError("source note id must match plugin id pattern")
    sources_dir = plugin_dir / "sources"
    if sources_dir.exists() and sources_dir.is_symlink():
        raise ValueError("sources directory must not be a symlink")
    sources_dir.mkdir(parents=True, exist_ok=True)
    path = sources_dir / f"{note_id}.json"
    _ensure_inside(plugin_dir, path)
    data = {
        "id": note_id,
        "title": title,
        "url": url,
        "captured_at": st.iso_now(),
        "claims": [{"id": f"c{i+1}", "text": c, "status": "unverified"} for i, c in enumerate(claims)],
        "hypotheses": [{"id": f"h{i+1}", "text": h} for i, h in enumerate(hypotheses)],
        "lto_status": "source-note-only; inert until referenced by an experimental plugin",
    }
    _atomic_write_json(path, data)
    if append_manifest:
        manifest_path = plugin_dir / "plugin.json"
        _ensure_inside(plugin_dir, manifest_path)
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if not isinstance(manifest, dict):
            raise ValueError("plugin.json must be an object")
        rel = str(path.relative_to(plugin_dir))
        notes = manifest.setdefault("source_notes", [])
        if rel not in notes:
            notes.append(rel)
        _atomic_write_json(manifest_path, manifest)
    return path


# 跨族约束的机读枚举（2026-06-10 dev-workflow spec W4-2，三方审 q4 共识）。
# 与 auditors._FAMILY 的值域一致；派工侧同族排除在 auditors._pick_auditors。
KNOWN_FAMILIES = {"openai", "anthropic", "google", "deepseek", "meta"}


def validate_profile_refs(plugin_dir: Path, manifest: dict[str, Any], errors: list[str]) -> None:
    security = manifest.get("security", {}) or {}
    env_allowlist = set(security.get("env_allowlist", []) or [])
    max_sandbox = security.get("max_sandbox", "read-only")
    for profile in _load_declared_profiles(plugin_dir, manifest, errors=errors):
        pid = profile.get("id", "<unknown>")
        family = profile.get("family")
        if family is not None and family not in KNOWN_FAMILIES:
            errors.append(
                f"profile {pid} family {family!r} not in known enum {sorted(KNOWN_FAMILIES)}"
            )
        rc = profile.get("runner_constraints")
        if rc is not None:
            if not isinstance(rc, dict):
                errors.append(f"profile {pid} runner_constraints must be an object")
            else:
                ehf = rc.get("exclude_host_family")
                if ehf is not None and not isinstance(ehf, bool):
                    errors.append(f"profile {pid} runner_constraints.exclude_host_family must be bool")
                mdf = rc.get("min_distinct_families")
                if mdf is not None and (not isinstance(mdf, int) or isinstance(mdf, bool) or mdf < 1):
                    errors.append(f"profile {pid} runner_constraints.min_distinct_families must be int >= 1")
                unknown = set(rc) - {"exclude_host_family", "min_distinct_families"}
                if unknown:
                    errors.append(f"profile {pid} runner_constraints unknown keys: {sorted(unknown)}")
        for key in (profile.get("env") or {}).keys():
            if key not in env_allowlist:
                errors.append(f"profile {pid} env key not allowlisted: {key}")
        sandbox = ((profile.get("permission") or {}).get("sandbox") or "read-only")
        if sandbox not in core.ALLOWED_SANDBOX:
            errors.append(f"profile {pid} has invalid sandbox: {sandbox}")
        if _sandbox_rank(sandbox) > _sandbox_rank(max_sandbox):
            errors.append(f"profile {pid} sandbox {sandbox} exceeds plugin max_sandbox {max_sandbox}")
        for ref_key in ("prompt_suffix_ref", "output_schema_ref"):
            ref = profile.get(ref_key)
            if ref:
                if ref_key == "prompt_suffix_ref" and not str(ref).endswith(".md"):
                    errors.append(f"profile {pid} prompt_suffix_ref must be .md: {ref}")
                if ref_key == "output_schema_ref" and not str(ref).endswith(".json"):
                    errors.append(f"profile {pid} output_schema_ref must be .json: {ref}")
                core._validate_rel_file(plugin_dir, ref, errors, must_parse_json=str(ref).endswith(".json"))


def _load_declared_profiles(plugin_dir: Path, manifest: dict[str, Any], errors: list[str] | None = None) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for rel in (manifest.get("provides", {}) or {}).get("profiles", []) or []:
        try:
            data = json.loads((plugin_dir / rel).read_text(encoding="utf-8"))
            if isinstance(data, dict):
                data["_relative_path"] = rel
                out.append(data)
            elif errors is not None:
                errors.append(f"profile must be an object: {rel}")
        except Exception as exc:
            if errors is not None:
                errors.append(f"profile invalid: {rel}: {exc}")
    return out


def _sandbox_rank(sandbox: str) -> int:
    order = {"read-only": 0, "workspace-write": 1, "danger-full-access": 2}
    return order.get(sandbox, 99)


def _ensure_rel_path(rel: str) -> None:
    if rel.startswith("/") or ".." in Path(rel).parts:
        raise ValueError(f"path escapes plugin dir: {rel}")


def _ensure_inside(root: Path, path: Path) -> None:
    root = root.resolve()
    try:
        parent = path.parent.resolve(strict=True)
        parent.relative_to(root)
    except (FileNotFoundError, RuntimeError, ValueError):
        raise ValueError(f"path escapes plugin dir: {path}")
    if path.exists() and path.is_symlink():
        raise ValueError(f"path must not be a symlink: {path}")


def _atomic_write_json(path: Path, data: dict[str, Any]) -> None:
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
        os.replace(tmp, path)
    finally:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
