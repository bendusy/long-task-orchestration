use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("missing plugin.json: {0}")]
    MissingManifest(PathBuf),
    #[error("{0}")]
    Message(String),
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation failed: {0:?}")]
    Validation(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginKind {
    PathPlugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStage {
    Experimental,
    Blessed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSecurity {
    pub executable_code: bool,
    #[serde(default = "default_max_sandbox")]
    pub max_sandbox: String,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub requires_human_approval_for: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_max_sandbox() -> String {
    "read-only".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub kind: PluginKind,
    pub stage: PluginStage,
    pub security: PluginSecurity,
    pub source_notes: Vec<String>,
    pub provides: serde_json::Value,
    #[serde(default)]
    pub rejection_reason: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginValidation {
    pub ok: bool,
    pub plugin_id: Option<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginMountEntry {
    pub plugin_id: String,
    pub mounted_at: String,
    pub manifest_hash: String,
    pub stage: PluginStage,
}

pub fn discover_plugins(repo: &Path) -> Vec<PathBuf> {
    [repo.join("plugins"), repo.join(".lto").join("plugins")]
        .into_iter()
        .filter(|root| root.exists())
        .flat_map(|root| {
            fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && path.join("plugin.json").exists())
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn mount_plugin(
    plugin_dir: &Path,
    mounts_json_path: &Path,
) -> Result<PluginMountEntry, PluginError> {
    let manifest_path = plugin_dir.join("plugin.json");
    if !manifest_path.exists() {
        return Err(PluginError::MissingManifest(manifest_path));
    }
    let raw = fs::read_to_string(&manifest_path)?;
    let manifest: PluginManifest = serde_json::from_str(&raw)?;
    let validation = validate_plugin(plugin_dir)?;
    if !validation.ok {
        return Err(PluginError::Validation(validation.errors));
    }

    let entry = PluginMountEntry {
        plugin_id: manifest.id,
        mounted_at: crate::state::iso_now(),
        manifest_hash: validation.manifest_hash,
        stage: manifest.stage,
    };
    let mut lock = load_mount_lock(mounts_json_path)?;
    lock.as_object_mut()
        .expect("mount lock forced to object")
        .insert("schema_version".to_string(), serde_json::json!(1));
    let mounts = lock
        .as_object_mut()
        .expect("mount lock forced to object")
        .entry("mounts")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !mounts.is_array() {
        *mounts = serde_json::Value::Array(Vec::new());
    }
    mounts
        .as_array_mut()
        .expect("mounts forced to array")
        .push(serde_json::to_value(&entry)?);

    if let Some(parent) = mounts_json_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        mounts_json_path,
        serde_json::to_string_pretty(&lock)? + "\n",
    )?;
    Ok(entry)
}

pub fn validate_plugin(plugin_dir: &Path) -> Result<PluginValidation, PluginError> {
    let manifest_path = plugin_dir.join("plugin.json");
    if !manifest_path.exists() {
        return Err(PluginError::MissingManifest(manifest_path));
    }
    let raw = fs::read_to_string(&manifest_path)?;
    let manifest: PluginManifest = serde_json::from_str(&raw)?;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if !ID_RE.is_match(&manifest.id) {
        errors.push("id must match ^[a-z0-9][a-z0-9._-]{1,80}$".to_string());
    }
    if !VERSION_RE.is_match(&manifest.version) {
        errors.push("version must be semver-like, e.g. 0.1.0".to_string());
    }
    if manifest.stage == PluginStage::Rejected
        && manifest
            .rejection_reason
            .as_deref()
            .unwrap_or("")
            .is_empty()
    {
        warnings.push("rejected plugin should include rejection_reason".to_string());
    }
    if manifest.security.executable_code {
        errors.push("security.executable_code must be false in plugin v0".to_string());
    }
    if !matches!(
        manifest.security.max_sandbox.as_str(),
        "read-only" | "workspace-write" | "danger-full-access"
    ) {
        errors.push("security.max_sandbox is invalid".to_string());
    }
    for key in &manifest.security.env_allowlist {
        if !ENV_KEY_RE.is_match(key) {
            errors.push(format!("invalid env_allowlist key: {key:?}"));
        } else if !HOST_ENV_ALLOWLIST.contains(&key.as_str()) {
            errors.push(format!("env_allowlist key is not host-approved: {key:?}"));
        }
    }
    if manifest.source_notes.is_empty() {
        errors.push("source_notes must be a non-empty list".to_string());
    }
    for rel in all_declared_refs(&manifest) {
        validate_rel_file(plugin_dir, &rel, &mut errors);
    }
    validate_profile_refs(plugin_dir, &manifest, &mut errors);
    validate_plugin_tree(plugin_dir, &mut errors);

    let manifest_hash = format!("sha256:{:x}", Sha256::digest(raw.as_bytes()));
    Ok(PluginValidation {
        ok: errors.is_empty(),
        plugin_id: Some(manifest.id),
        errors,
        warnings,
        manifest_hash,
    })
}

fn load_mount_lock(path: &Path) -> Result<serde_json::Value, PluginError> {
    if !path.exists() {
        return Ok(serde_json::json!({
            "schema_version": 1,
            "mounts": [],
        }));
    }
    let text = fs::read_to_string(path)?;
    let mut value = serde_json::from_str::<serde_json::Value>(&text)?;
    if !value.is_object() {
        value = serde_json::json!({
            "schema_version": 1,
            "mounts": [],
        });
    }
    Ok(value)
}

pub fn render_profile(
    plugin_dir: &Path,
    profile_id: &str,
    input_path: &Path,
    output_path: &Path,
) -> Result<serde_json::Value, PluginError> {
    let profile = load_profile(plugin_dir, profile_id)?;
    let mut chunks = vec![fs::read_to_string(input_path)?.trim_end().to_string()];
    let suffix_ref = profile
        .get("prompt_suffix_ref")
        .and_then(serde_json::Value::as_str);
    if let Some(rel) = suffix_ref {
        let path = safe_plugin_file(plugin_dir, rel)?;
        chunks.push("# LTO plugin profile instructions".to_string());
        chunks.push(fs::read_to_string(path)?.trim_end().to_string());
    }
    let suffix = profile
        .get("prompt_suffix")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(text) = suffix {
        chunks.push("# LTO plugin profile instructions".to_string());
        chunks.push(text.to_string());
    }
    let rendered = chunks.join("\n\n").trim_end().to_string() + "\n";
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &rendered)?;
    Ok(serde_json::json!({
        "profile_id": profile_id,
        "profile_path": profile.get("_relative_path").and_then(serde_json::Value::as_str).unwrap_or(""),
        "input": input_path.display().to_string(),
        "output": output_path.display().to_string(),
        "rendered_bytes": rendered.len(),
        "prompt_suffix_ref": suffix_ref,
        "output_schema_ref": profile.get("output_schema_ref").cloned().unwrap_or(serde_json::Value::Null),
        "env_keys": profile.get("env")
            .and_then(serde_json::Value::as_object)
            .map(|obj| obj.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        "permission": profile.get("permission").cloned().unwrap_or_else(|| serde_json::json!({})),
    }))
}

pub fn static_eval(
    plugin_dir: &Path,
    eval_id: Option<&str>,
) -> Result<serde_json::Value, PluginError> {
    let validation = validate_plugin(plugin_dir)?;
    let mut errors = validation.errors.clone();
    let mut evals = Vec::new();
    if validation.ok {
        let manifest = load_manifest(plugin_dir)?;
        let profile_ids = load_declared_profiles(plugin_dir, &manifest, &mut errors)
            .into_iter()
            .filter_map(|profile| {
                profile
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        for rel in manifest
            .provides
            .get("evals")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            let data = read_json_file(&safe_plugin_file(plugin_dir, rel)?)?;
            if data.get("id").and_then(serde_json::Value::as_str) != eval_id && eval_id.is_some() {
                continue;
            }
            let (eval_summary, eval_errors) = summarize_eval_pack(rel, &data, &profile_ids);
            errors.extend(eval_errors);
            evals.push(eval_summary);
        }
        if eval_id.is_some() && evals.is_empty() {
            errors.push(format!("eval not found: {}", eval_id.unwrap_or_default()));
        }
    }
    Ok(serde_json::json!({
        "plugin_dir": plugin_dir.display().to_string(),
        "validation": validation,
        "evals": evals,
        "ok": errors.is_empty(),
        "errors": errors,
    }))
}

pub fn create_source_note(
    plugin_dir: &Path,
    note_id: &str,
    title: &str,
    url: &str,
    claims: &[String],
    hypotheses: &[String],
    append_manifest: bool,
) -> Result<PathBuf, PluginError> {
    let plugin_dir = plugin_dir.canonicalize()?;
    if !ID_RE.is_match(note_id) {
        return Err(PluginError::Message(
            "source note id must match plugin id pattern".to_string(),
        ));
    }

    let sources_dir = plugin_dir.join("sources");
    if sources_dir.exists() && sources_dir.is_symlink() {
        return Err(PluginError::Message(
            "sources directory must not be a symlink".to_string(),
        ));
    }
    fs::create_dir_all(&sources_dir)?;

    let path = sources_dir.join(format!("{note_id}.json"));
    ensure_inside(&plugin_dir, &path)?;
    let data = serde_json::json!({
        "id": note_id,
        "title": title,
        "url": url,
        "captured_at": crate::state::iso_now(),
        "claims": claims
            .iter()
            .enumerate()
            .map(|(idx, claim)| serde_json::json!({
                "id": format!("c{}", idx + 1),
                "text": claim,
                "status": "unverified",
            }))
            .collect::<Vec<_>>(),
        "hypotheses": hypotheses
            .iter()
            .enumerate()
            .map(|(idx, hypothesis)| serde_json::json!({
                "id": format!("h{}", idx + 1),
                "text": hypothesis,
            }))
            .collect::<Vec<_>>(),
        "lto_status": "source-note-only; inert until referenced by an experimental plugin",
    });
    atomic_write_json(&path, &data)?;

    if append_manifest {
        append_source_note_to_manifest(&plugin_dir, &path)?;
    }

    Ok(path)
}

pub(crate) fn load_manifest(plugin_dir: &Path) -> Result<PluginManifest, PluginError> {
    let manifest_path = plugin_dir.join("plugin.json");
    if !manifest_path.exists() {
        return Err(PluginError::MissingManifest(manifest_path));
    }
    Ok(serde_json::from_str(&fs::read_to_string(manifest_path)?)?)
}

pub(crate) fn load_profile(
    plugin_dir: &Path,
    profile_id: &str,
) -> Result<serde_json::Value, PluginError> {
    let validation = validate_plugin(plugin_dir)?;
    if !validation.ok {
        return Err(PluginError::Validation(validation.errors));
    }
    let manifest = load_manifest(plugin_dir)?;
    let mut errors = Vec::new();
    for profile in load_declared_profiles(plugin_dir, &manifest, &mut errors) {
        if profile.get("id").and_then(serde_json::Value::as_str) == Some(profile_id) {
            return Ok(profile);
        }
    }
    if errors.is_empty() {
        Err(PluginError::Message(format!(
            "profile not found: {profile_id}"
        )))
    } else {
        Err(PluginError::Validation(errors))
    }
}

fn load_declared_profiles(
    plugin_dir: &Path,
    manifest: &PluginManifest,
    errors: &mut Vec<String>,
) -> Vec<serde_json::Value> {
    let mut profiles = Vec::new();
    for rel in manifest
        .provides
        .get("profiles")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        match safe_plugin_file(plugin_dir, rel)
            .and_then(|path| read_json_file(&path))
            .and_then(|mut value| {
                if !value.is_object() {
                    return Err(PluginError::Message(format!(
                        "profile must be an object: {rel}"
                    )));
                }
                value["_relative_path"] = serde_json::Value::String(rel.to_string());
                Ok(value)
            }) {
            Ok(profile) => profiles.push(profile),
            Err(err) => errors.push(format!("profile invalid: {rel}: {err}")),
        }
    }
    profiles
}

fn validate_profile_refs(plugin_dir: &Path, manifest: &PluginManifest, errors: &mut Vec<String>) {
    let env_allowlist = manifest
        .security
        .env_allowlist
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let max_sandbox = manifest.security.max_sandbox.as_str();
    for profile in load_declared_profiles(plugin_dir, manifest, errors) {
        let pid = profile
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>");
        if let Some(family) = profile.get("family").and_then(serde_json::Value::as_str)
            && !KNOWN_FAMILIES.contains(&family)
        {
            errors.push(format!("profile {pid} family {family:?} not in known enum"));
        }
        if let Some(rc) = profile.get("runner_constraints") {
            validate_runner_constraints(pid, rc, errors);
        }
        if let Some(env) = profile.get("env").and_then(serde_json::Value::as_object) {
            for key in env.keys() {
                if !env_allowlist.contains(&key.as_str()) {
                    errors.push(format!("profile {pid} env key not allowlisted: {key}"));
                }
            }
        }
        let sandbox = profile
            .get("permission")
            .and_then(|value| value.get("sandbox"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("read-only");
        if !ALLOWED_SANDBOX.contains(&sandbox) {
            errors.push(format!("profile {pid} has invalid sandbox: {sandbox}"));
        }
        if sandbox_rank(sandbox) > sandbox_rank(max_sandbox) {
            errors.push(format!(
                "profile {pid} sandbox {sandbox} exceeds plugin max_sandbox {max_sandbox}"
            ));
        }
        for key in ["prompt_suffix_ref", "output_schema_ref"] {
            if let Some(rel) = profile.get(key).and_then(serde_json::Value::as_str) {
                if key == "prompt_suffix_ref" && !rel.ends_with(".md") {
                    errors.push(format!(
                        "profile {pid} prompt_suffix_ref must be .md: {rel}"
                    ));
                }
                if key == "output_schema_ref" && !rel.ends_with(".json") {
                    errors.push(format!(
                        "profile {pid} output_schema_ref must be .json: {rel}"
                    ));
                }
                validate_rel_file(plugin_dir, rel, errors);
            }
        }
    }
}

fn validate_runner_constraints(pid: &str, value: &serde_json::Value, errors: &mut Vec<String>) {
    let Some(obj) = value.as_object() else {
        errors.push(format!(
            "profile {pid} runner_constraints must be an object"
        ));
        return;
    };
    for key in obj.keys() {
        if key != "exclude_host_family" && key != "min_distinct_families" {
            errors.push(format!(
                "profile {pid} runner_constraints unknown key: {key}"
            ));
        }
    }
    if let Some(value) = obj.get("exclude_host_family")
        && !value.is_boolean()
    {
        errors.push(format!(
            "profile {pid} runner_constraints.exclude_host_family must be bool"
        ));
    }
    if let Some(value) = obj.get("min_distinct_families")
        && value.as_u64().filter(|v| *v >= 1).is_none()
    {
        errors.push(format!(
            "profile {pid} runner_constraints.min_distinct_families must be int >= 1"
        ));
    }
}

fn summarize_eval_pack(
    rel: &str,
    data: &serde_json::Value,
    profile_ids: &[String],
) -> (serde_json::Value, Vec<String>) {
    let mut errors = Vec::new();
    let id = data
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let cases: &[serde_json::Value] = match data.get("cases").and_then(serde_json::Value::as_array)
    {
        Some(cases) if !cases.is_empty() => cases.as_slice(),
        _ => {
            errors.push(format!("eval {id} must include non-empty cases list"));
            &[]
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    for case in cases {
        let Some(cid) = case.get("id").and_then(serde_json::Value::as_str) else {
            errors.push(format!("eval {id} case id missing"));
            continue;
        };
        if !seen.insert(cid.to_string()) {
            errors.push(format!("eval {id} case id duplicate: {cid}"));
        }
        if let Some(profile) = case.get("profile").and_then(serde_json::Value::as_str)
            && !profile_ids.iter().any(|known| known == profile)
        {
            errors.push(format!("case {cid} references unknown profile {profile}"));
        }
    }
    let metrics = data
        .get("metrics")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if metrics.is_empty() {
        errors.push(format!("eval {id} has no metrics"));
    }
    let metric_names = metrics
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    for required in ["permission_violations", "private_path_leaks"] {
        if !metric_names.contains(&required) {
            errors.push(format!(
                "eval {id} missing required safety metric: {required}"
            ));
        }
    }
    if data
        .get("safety_regressions_allowed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        != 0
    {
        errors.push(format!("eval {id} must set safety_regressions_allowed=0"));
    }
    if data
        .get("minimum_runs_before_promotion")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        < 1
    {
        errors.push(format!(
            "eval {id} must set minimum_runs_before_promotion >= 1"
        ));
    }
    let ok = errors.is_empty();
    (
        serde_json::json!({
            "id": id,
            "path": rel,
            "cases": cases.len(),
            "metrics": metric_names,
            "ok": ok,
            "errors": errors.clone(),
        }),
        errors,
    )
}

pub(crate) fn safe_plugin_file(plugin_dir: &Path, rel: &str) -> Result<PathBuf, PluginError> {
    if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
        return Err(PluginError::Message(format!(
            "invalid relative file path: {rel}"
        )));
    }
    let path = plugin_dir.join(rel);
    if path.is_symlink() {
        return Err(PluginError::Message(format!(
            "plugin file must not be symlink: {rel}"
        )));
    }
    if !path.is_file() {
        return Err(PluginError::Message(format!("missing plugin file: {rel}")));
    }
    Ok(path)
}

fn read_json_file(path: &Path) -> Result<serde_json::Value, PluginError> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn append_source_note_to_manifest(plugin_dir: &Path, path: &Path) -> Result<(), PluginError> {
    let manifest_path = plugin_dir.join("plugin.json");
    ensure_inside(plugin_dir, &manifest_path)?;
    let mut manifest = read_json_file(&manifest_path)?;
    let Some(obj) = manifest.as_object_mut() else {
        return Err(PluginError::Message(
            "plugin.json must be an object".to_string(),
        ));
    };
    let rel = path
        .strip_prefix(plugin_dir)
        .map_err(|_| PluginError::Message(format!("path escapes plugin dir: {}", path.display())))?
        .to_string_lossy()
        .replace('\\', "/");
    let source_notes = obj
        .entry("source_notes")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(notes) = source_notes.as_array_mut() else {
        return Err(PluginError::Message(
            "plugin.json source_notes must be an array".to_string(),
        ));
    };
    if !notes
        .iter()
        .any(|value| value.as_str() == Some(rel.as_str()))
    {
        notes.push(serde_json::Value::String(rel));
        atomic_write_json(&manifest_path, &manifest)?;
    }
    Ok(())
}

fn ensure_inside(root: &Path, path: &Path) -> Result<(), PluginError> {
    let root = root.canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| PluginError::Message(format!("path has no parent: {}", path.display())))?
        .canonicalize()?;
    if parent.strip_prefix(&root).is_err() {
        return Err(PluginError::Message(format!(
            "path escapes plugin dir: {}",
            path.display()
        )));
    }
    if path.exists() && path.is_symlink() {
        return Err(PluginError::Message(format!(
            "path must not be a symlink: {}",
            path.display()
        )));
    }
    Ok(())
}

fn atomic_write_json(path: &Path, value: &serde_json::Value) -> Result<(), PluginError> {
    let parent = path
        .parent()
        .ok_or_else(|| PluginError::Message(format!("path has no parent: {}", path.display())))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("json");
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{file_name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    tmp.write_all((serde_json::to_string_pretty(value)? + "\n").as_bytes())?;
    tmp.flush()?;
    tmp.persist(path)
        .map_err(|err| PluginError::Io(err.error))?;
    Ok(())
}

fn sandbox_rank(sandbox: &str) -> u8 {
    match sandbox {
        "read-only" => 0,
        "workspace-write" => 1,
        "danger-full-access" => 2,
        _ => 99,
    }
}

fn all_declared_refs(manifest: &PluginManifest) -> Vec<String> {
    let mut refs = manifest.source_notes.clone();
    for section in ["paths", "profiles", "evals"] {
        if let Some(values) = manifest.provides.get(section).and_then(|v| v.as_array()) {
            refs.extend(values.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
    }
    refs
}

fn validate_rel_file(root: &Path, rel: &str, errors: &mut Vec<String>) {
    if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
        errors.push(format!("invalid relative file path: {rel}"));
        return;
    }
    let path = root.join(rel);
    if path.is_symlink() {
        errors.push(format!("plugin file must not be symlink: {rel}"));
        return;
    }
    if !path.is_file() {
        errors.push(format!("missing plugin file: {rel}"));
        return;
    }
    let suffix = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if !matches!(suffix, "json" | "md") {
        errors.push(format!("plugin file must be .json or .md: {rel}"));
    }
    if suffix == "json" {
        match fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        {
            Some(_) => {}
            None => errors.push(format!("plugin JSON file does not parse: {rel}")),
        }
    }
}

fn validate_plugin_tree(root: &Path, errors: &mut Vec<String>) {
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir() {
            continue;
        }
        if entry
            .path()
            .components()
            .any(|part| part.as_os_str() == ".git")
        {
            continue;
        }
        if entry.path().is_symlink() {
            errors.push(format!(
                "plugin tree contains symlink: {}",
                entry.path().display()
            ));
            continue;
        }
        let suffix = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !matches!(suffix, "json" | "md") {
            errors.push(format!(
                "plugin tree contains non-data file: {}",
                entry.path().display()
            ));
        }
    }
}

static ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{1,80}$").expect("invalid plugin id regex"));
static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9._-]+)?$")
        .expect("invalid plugin version regex")
});
static ENV_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z][A-Z0-9_]{0,63}$").expect("invalid plugin environment key regex")
});
const HOST_ENV_ALLOWLIST: &[&str] = &["CODEX_MODEL", "CODEX_PROFILE", "CODEX_JSON", "CODEX_IMAGES"];
const ALLOWED_SANDBOX: &[&str] = &["read-only", "workspace-write", "danger-full-access"];
const KNOWN_FAMILIES: &[&str] = &["openai", "anthropic", "google", "deepseek", "meta"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_existing_manifest_shape() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let plugin = repo.join("plugins").join("dev-workflow");
        if plugin.exists() {
            let validation = validate_plugin(&plugin).unwrap();
            assert!(validation.ok, "{:?}", validation.errors);
        }
    }

    #[test]
    fn static_eval_validates_existing_eval_pack() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let plugin = repo.join("plugins").join("deep-agent-profiles");
        if plugin.exists() {
            let report = static_eval(&plugin, None).unwrap();
            assert_eq!(report["ok"], true);
            assert!(!report["evals"].as_array().unwrap().is_empty());
        }
    }

    #[test]
    fn retained_scenario_plugins_validate_and_have_eval_packs() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join("plugin-mounts.json");
        for plugin_name in [
            "adversarial-audit",
            "claim-verify-research",
            "migration-refactor",
        ] {
            let plugin = repo.join("plugins").join(plugin_name);
            let validation = validate_plugin(&plugin).unwrap();
            assert!(validation.ok, "{plugin_name}: {:?}", validation.errors);
            let report = static_eval(&plugin, None).unwrap();
            assert_eq!(report["ok"], true, "{plugin_name}: {report}");
            assert!(
                !report["evals"].as_array().unwrap().is_empty(),
                "{plugin_name} should keep at least one eval pack"
            );
            let mount = mount_plugin(&plugin, &lock).unwrap();
            assert_eq!(mount.plugin_id, plugin_name);
        }
        let lock_data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(lock).unwrap()).unwrap();
        let mounted = lock_data["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["plugin_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            mounted,
            vec![
                "adversarial-audit",
                "claim-verify-research",
                "migration-refactor",
            ]
        );
    }

    #[test]
    fn render_profile_appends_prompt_suffix() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let plugin = repo.join("plugins").join("deep-agent-profiles");
        if plugin.exists() {
            let tmp = tempfile::tempdir().unwrap();
            let input = tmp.path().join("brief.md");
            let output = tmp.path().join("rendered.md");
            fs::write(&input, "Goal:\nAudit this design.\n").unwrap();
            let meta = render_profile(&plugin, "codex-audit-readonly-v1", &input, &output).unwrap();
            let rendered = fs::read_to_string(output).unwrap();
            assert!(rendered.contains("Goal:"));
            assert!(rendered.contains("LTO plugin profile instructions"));
            assert_eq!(meta["profile_id"], "codex-audit-readonly-v1");
        }
    }

    #[test]
    fn source_note_writes_expected_fields() {
        let tmp = tempfile::tempdir().unwrap();
        write_minimal_plugin(tmp.path(), r#""source_notes":[]"#);
        let path = create_source_note(
            tmp.path(),
            "x.note",
            "Source title",
            "https://example.test/source",
            &["first claim".to_string(), "second claim".to_string()],
            &["maybe useful".to_string()],
            false,
        )
        .unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(data["id"], "x.note");
        assert_eq!(data["title"], "Source title");
        assert_eq!(data["url"], "https://example.test/source");
        assert!(
            data["captured_at"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert_eq!(data["claims"][0]["id"], "c1");
        assert_eq!(data["claims"][0]["text"], "first claim");
        assert_eq!(data["claims"][0]["status"], "unverified");
        assert_eq!(data["claims"][1]["id"], "c2");
        assert_eq!(data["hypotheses"][0]["id"], "h1");
        assert_eq!(
            data["lto_status"],
            "source-note-only; inert until referenced by an experimental plugin"
        );
    }

    #[test]
    fn source_note_rejects_invalid_id() {
        let tmp = tempfile::tempdir().unwrap();
        write_minimal_plugin(tmp.path(), r#""source_notes":[]"#);
        let err = create_source_note(
            tmp.path(),
            "../bad",
            "bad",
            "https://example.test",
            &[],
            &[],
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("source note id must match"));
    }

    #[cfg(unix)]
    #[test]
    fn source_note_rejects_symlink_sources_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_minimal_plugin(tmp.path(), r#""source_notes":[]"#);
        let target = tmp.path().join("outside");
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, tmp.path().join("sources")).unwrap();

        let err = create_source_note(
            tmp.path(),
            "x.note",
            "bad",
            "https://example.test",
            &[],
            &[],
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("sources directory must not be a symlink")
        );
    }

    #[test]
    fn source_note_append_manifest_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        write_minimal_plugin(tmp.path(), r#""source_notes":[]"#);
        for _ in 0..2 {
            create_source_note(
                tmp.path(),
                "x.note",
                "Source title",
                "https://example.test/source",
                &[],
                &[],
                true,
            )
            .unwrap();
        }

        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join("plugin.json")).unwrap())
                .unwrap();
        let notes = manifest["source_notes"].as_array().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0], "sources/x.note.json");
    }

    #[test]
    fn source_note_rejects_non_object_manifest_when_appending() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("plugin.json"), "[]").unwrap();
        let err = create_source_note(
            tmp.path(),
            "x.note",
            "Source title",
            "https://example.test/source",
            &[],
            &[],
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("plugin.json must be an object"));
    }

    #[test]
    fn validate_rejects_unknown_profile_family() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugin");
        fs::create_dir_all(plugin_dir.join("profiles")).unwrap();
        fs::write(
            plugin_dir.join("plugin.json"),
            r#"{
              "id":"x.test","version":"0.1.0","kind":"path-plugin","stage":"experimental",
              "security":{"executable_code":false,"max_sandbox":"read-only"},
              "source_notes":["note.md"],
              "provides":{"profiles":["profiles/p.json"]}
            }"#,
        )
        .unwrap();
        fs::write(plugin_dir.join("note.md"), "note").unwrap();
        fs::write(
            plugin_dir.join("profiles").join("p.json"),
            r#"{"id":"p","family":"alien","permission":{"sandbox":"read-only"}}"#,
        )
        .unwrap();
        let validation = validate_plugin(&plugin_dir).unwrap();
        assert!(!validation.ok);
        assert!(validation.errors.iter().any(|err| err.contains("family")));
    }

    #[test]
    fn rejects_executable_code_flag() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("plugin.json"),
            r#"{
              "id":"x.test","version":"0.1.0","kind":"path-plugin","stage":"experimental",
              "security":{"executable_code":true},
              "source_notes":["note.md"],"provides":{}
            }"#,
        )
        .unwrap();
        fs::write(tmp.path().join("note.md"), "note").unwrap();
        let validation = validate_plugin(tmp.path()).unwrap();
        assert!(!validation.ok);
    }

    #[test]
    fn mount_appends_data_only_provenance_without_changing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let manifest = r#"{
          "id":"x.test","version":"0.1.0","kind":"path-plugin","stage":"experimental",
          "security":{"executable_code":false,"max_sandbox":"read-only"},
          "source_notes":["note.md"],
          "provides":{"paths":["path.json"]}
        }"#;
        fs::write(plugin_dir.join("plugin.json"), manifest).unwrap();
        fs::write(plugin_dir.join("note.md"), "note").unwrap();
        fs::write(plugin_dir.join("path.json"), "{}").unwrap();
        let lock = tmp.path().join("plugin-mounts.json");

        let first = mount_plugin(&plugin_dir, &lock).unwrap();
        let second = mount_plugin(&plugin_dir, &lock).unwrap();
        assert_eq!(first.plugin_id, "x.test");
        assert_eq!(first.stage, PluginStage::Experimental);
        assert_eq!(first.manifest_hash, second.manifest_hash);

        let data: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&lock).unwrap()).unwrap();
        let mounts = data["mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0]["plugin_id"], "x.test");
        assert!(mounts[0].get("approved_permissions").is_none());
        assert!(mounts[0].get("default_enabled").is_none());
        assert_eq!(
            fs::read_to_string(plugin_dir.join("plugin.json")).unwrap(),
            manifest
        );
        assert_eq!(
            validate_plugin(&plugin_dir).unwrap().plugin_id.as_deref(),
            Some("x.test")
        );
    }

    #[test]
    fn mount_rejects_invalid_plugin_without_writing_lock() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("plugin.json"),
            r#"{
              "id":"bad plugin","version":"0.1.0","kind":"path-plugin","stage":"experimental",
              "security":{"executable_code":false},
              "source_notes":["note.md"],"provides":{}
            }"#,
        )
        .unwrap();
        fs::write(tmp.path().join("note.md"), "note").unwrap();
        let lock = tmp.path().join("plugin-mounts.json");
        let err = mount_plugin(tmp.path(), &lock).unwrap_err();
        assert!(matches!(err, PluginError::Validation(_)));
        assert!(!lock.exists());
    }

    fn write_minimal_plugin(plugin_dir: &Path, source_notes_entry: &str) {
        let manifest = format!(
            r#"{{
              "id":"x.test","version":"0.1.0","kind":"path-plugin","stage":"experimental",
              "security":{{"executable_code":false,"max_sandbox":"read-only"}},
              {source_notes_entry},
              "provides":{{}}
            }}"#
        );
        fs::write(plugin_dir.join("plugin.json"), manifest).unwrap();
    }
}
