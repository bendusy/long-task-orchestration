use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

static SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(sk-[A-Za-z0-9_-]{12,}|sk-ant-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|AKIA[0-9A-Z]{16}|-----BEGIN [^-]*PRIVATE KEY-----|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
    )
    .expect("invalid secret regex")
});

static ABS_PRIVATE_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"/(?:Users|home)/[^\s:'"]+"#).expect("invalid path regex"));

static WHITESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("invalid whitespace regex"));

const TRUNCATE: usize = 240;
const FORBIDDEN_KEY_EXACT: &[&str] = &["stdout", "stderr", "reply_text", "transcript", "output"];
const FORBIDDEN_KEY_SUFFIXES: &[&str] = &["_excerpt", "_tail"];
const FORBIDDEN_KEY_SUBSTRINGS: &[&str] = &["output", "stdout", "stderr", "transcript"];

pub fn redact_text(value: &str) -> String {
    let value = SECRET_RE.replace_all(value, "[REDACTED_SECRET]");
    let value = ABS_PRIVATE_PATH_RE.replace_all(&value, "[REDACTED_PATH]");
    let collapsed = WHITESPACE_RE.replace_all(&value, " ");
    collapsed.trim().chars().take(TRUNCATE).collect()
}

pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = Map::new();
            for (key, value) in obj {
                if !is_forbidden_key(key) {
                    out.insert(redact_text(key), redact_value(value));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::String(text) => Value::String(redact_text(text)),
        Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
    }
}

fn is_forbidden_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    FORBIDDEN_KEY_EXACT.contains(&key.as_str())
        || FORBIDDEN_KEY_SUFFIXES
            .iter()
            .any(|suffix| key.ends_with(suffix))
        || FORBIDDEN_KEY_SUBSTRINGS
            .iter()
            .any(|needle| key.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_secrets_paths_and_raw_output_keys() {
        let value = json!({
            "summary": "token sk-123456789012 lives in /Users/ben/private/file.rs",
            "stdout": "must disappear",
            "nested": {"stderr_tail": "must disappear", "keep": "ok"},
        });
        let redacted = redact_value(&value);
        let blob = serde_json::to_string(&redacted).unwrap();
        assert!(!blob.contains("sk-123456789012"));
        assert!(!blob.contains("/Users/ben/private"));
        assert!(!blob.contains("must disappear"));
        assert!(blob.contains("[REDACTED_SECRET]"));
        assert!(blob.contains("[REDACTED_PATH]"));
    }
}
