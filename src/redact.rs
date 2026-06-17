use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use std::sync::LazyLock;

// Single source of truth for secret/path redaction (backlog ⑫). Previously a
// weaker copy lived here and a stronger copy in llm_judge.rs; the weak set leaked
// /root, Windows paths, github_pat_ tokens and `key=value` pairs into events.jsonl
// / telemetry. This is the superset; llm_judge re-exports from here.
//   - PEM: match a full BEGIN..END block (dot_matches_new_line) but keep the END
//     OPTIONAL so a lone BEGIN line (no END) is still redacted.
//   - KV value char class excludes `.` so dot_matches_new_line cannot over-match.
static SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(
        r#"-----BEGIN[^-]*PRIVATE KEY-----.*?-----END[^-]*PRIVATE KEY-----|-----BEGIN[^-]*PRIVATE KEY-----|sk-ant-[A-Za-z0-9_-]{12,}|sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|github_pat_[A-Za-z0-9_]{12,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}|(?i:api[_-]?key|secret|token|password|passwd|pwd|access[_-]?token)\s*[:=]\s*["']?[A-Za-z0-9_\-./+=]{6,}["']?"#,
    )
    .dot_matches_new_line(true)
    .build()
    .expect("invalid secret regex")
});

static ABS_PRIVATE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:\\?/|/)(?:Users|home)(?:\\?/|/)[^\s"'`,;:)\]}]+|/root/[^\s"'`,;:)\]}]+|[A-Za-z]:\\Users\\[^\s"'`,;:)\]}]+"#,
    )
    .expect("invalid path regex")
});

static WHITESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("invalid whitespace regex"));

const TRUNCATE: usize = 240;
const FORBIDDEN_KEY_EXACT: &[&str] = &["stdout", "stderr", "reply_text", "transcript", "output"];
const FORBIDDEN_KEY_SUFFIXES: &[&str] = &["_excerpt", "_tail"];
const FORBIDDEN_KEY_SUBSTRINGS: &[&str] = &["output", "stdout", "stderr", "transcript"];

/// Replace secrets and private paths only — preserve everything else verbatim
/// (newlines, length). Used by callers that must keep the exact text shape, e.g.
/// llm_judge frozen-evidence hashing (backlog ⑫: single redaction source).
pub fn redact_secrets_and_paths(value: &str) -> String {
    let value = SECRET_RE.replace_all(value, "[REDACTED_SECRET]");
    ABS_PRIVATE_PATH_RE
        .replace_all(&value, "[REDACTED_PATH]")
        .into_owned()
}

/// Redact + collapse whitespace + truncate. Used for compact event/telemetry
/// summaries where readability and a size cap matter more than exact shape.
pub fn redact_text(value: &str) -> String {
    let redacted = redact_secrets_and_paths(value);
    let collapsed = WHITESPACE_RE.replace_all(&redacted, " ");
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

    #[test]
    fn redacts_superset_secrets_and_paths_from_backlog_12() {
        // backlog ⑫: the unified set must cover what the old weak copy here missed
        // (github_pat_, key=value pairs, /root, Windows paths) so they never reach
        // events.jsonl / telemetry.
        for raw in [
            "github_pat_11ABCDEFG0aaaaaaaaaaaa_bbbbbbbbbbbb",
            "api_key=supersecretvalue123",
            "TOKEN: abcdef123456",
            "password = hunter2hunter2",
        ] {
            assert_eq!(
                redact_text(raw),
                "[REDACTED_SECRET]",
                "secret not fully redacted: {raw}"
            );
        }
        for raw in ["/root/.ssh/id_rsa", r"C:\Users\ben\secret.txt"] {
            assert!(
                redact_text(raw).contains("[REDACTED_PATH]"),
                "path not redacted: {raw}"
            );
        }
        // A lone PEM BEGIN line (no END) must still be redacted (END is optional).
        assert!(
            redact_text("-----BEGIN OPENSSH PRIVATE KEY-----").contains("[REDACTED_SECRET]"),
            "lone PEM BEGIN line leaked"
        );
        // A full PEM block (multi-line) is redacted as one unit.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIB\n-----END RSA PRIVATE KEY-----";
        assert!(!redact_text(pem).contains("MIIEowIB"), "PEM body leaked");
    }
}
