use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("missing plugin.json: {0}")]
    MissingManifest(PathBuf),
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
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{1,80}$").unwrap());
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9._-]+)?$").unwrap());
static ENV_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]{0,63}$").unwrap());
const HOST_ENV_ALLOWLIST: &[&str] = &["CODEX_MODEL", "CODEX_PROFILE", "CODEX_JSON", "CODEX_IMAGES"];

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
}
