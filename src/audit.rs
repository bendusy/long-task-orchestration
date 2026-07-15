use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub claim: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_confidence: Option<ReportedConfidence>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub invalidated_when: Option<String>,
    #[serde(default)]
    pub evidence_to_check: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedConfidenceLevel {
    High,
    Medium,
    Low,
}

impl ReportedConfidenceLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReportedConfidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<ReportedConfidenceLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

impl<'de> Deserialize<'de> for ReportedConfidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let (level, rationale) = match value {
            Value::String(level) => (normalize_reported_confidence_level(&level), None),
            Value::Object(mut object) => {
                let level = object
                    .remove("level")
                    .and_then(|value| value.as_str().map(str::to_string))
                    .and_then(|level| normalize_reported_confidence_level(&level));
                let rationale = object
                    .remove("rationale")
                    .and_then(|value| value.as_str().map(str::to_string));
                (level, rationale)
            }
            other => {
                eprintln!(
                    "WARN: reported_confidence must be an object or string, got {}",
                    value_kind(&other)
                );
                (None, None)
            }
        };
        Ok(Self { level, rationale })
    }
}

pub(crate) fn normalize_reported_confidence_level(input: &str) -> Option<ReportedConfidenceLevel> {
    let normalized = input.trim().to_ascii_lowercase();
    let level = match normalized.as_str() {
        "high" => Some(ReportedConfidenceLevel::High),
        "medium" => Some(ReportedConfidenceLevel::Medium),
        "low" => Some(ReportedConfidenceLevel::Low),
        _ => None,
    };
    if level.is_none() {
        eprintln!("WARN: unsupported reported_confidence level: {input}");
    }
    level
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(optional_string_value(Some(&value), "invalidated_when"))
}

fn optional_string_value(value: Option<&Value>, field: &str) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(other) => {
            eprintln!("WARN: {field} must be a string, got {}", value_kind(other));
            None
        }
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerFamily {
    Claude,
    Codex,
    Pi,
    Agy,
    Unknown(String),
}

pub fn family(runtime: &str) -> RunnerFamily {
    let lower = runtime.to_ascii_lowercase();
    for (key, family) in [
        ("claude", RunnerFamily::Claude),
        ("anthropic", RunnerFamily::Claude),
        ("codex", RunnerFamily::Codex),
        ("gpt", RunnerFamily::Codex),
        ("openai", RunnerFamily::Codex),
        ("pi", RunnerFamily::Pi),
        ("deepseek", RunnerFamily::Pi),
        ("agy", RunnerFamily::Agy),
        ("gemini", RunnerFamily::Agy),
        ("google", RunnerFamily::Agy),
    ] {
        if lower.contains(key) {
            return family;
        }
    }
    RunnerFamily::Unknown(lower)
}

pub fn same_family(a: &str, b: &str) -> bool {
    family(a) == family(b)
}

pub fn normalize_severity(input: &str) -> Option<Severity> {
    match input.trim().to_ascii_lowercase().as_str() {
        "critical" | "严重" | "致命" | "阻断" => Some(Severity::Critical),
        "high" | "警告" | "高危" => Some(Severity::High),
        "medium" | "中危" => Some(Severity::Medium),
        "low" | "提示" | "建议" => Some(Severity::Low),
        _ => None,
    }
}

pub fn parse_findings_text(text: &str) -> Option<Vec<Finding>> {
    parse_findings_json(text).or_else(|| {
        FENCE_RE.captures_iter(text).find_map(|captures| {
            captures
                .get(1)
                .and_then(|body| parse_findings_json(body.as_str()))
        })
    })
}

fn parse_findings_json(text: &str) -> Option<Vec<Finding>> {
    let value = serde_json::from_str::<Value>(text.trim()).ok()?;
    let items = value
        .as_array()
        .or_else(|| value.get("findings").and_then(Value::as_array))?;
    parse_findings_values(items)
}

pub fn parse_findings_values(items: &[Value]) -> Option<Vec<Finding>> {
    if items.is_empty() {
        return None;
    }
    items.iter().map(parse_finding_value).collect()
}

pub fn parse_valid_findings_values(items: &[Value]) -> Vec<Finding> {
    items.iter().filter_map(parse_finding_value).collect()
}

fn parse_finding_value(item: &Value) -> Option<Finding> {
    let obj = item.as_object()?;
    let sev = obj.get("severity")?.as_str()?;
    let severity = normalize_severity(sev)?;
    let claim = obj.get("claim")?.as_str()?.to_string();
    let file = obj
        .get("file")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            obj.get("location")
                .and_then(|v| v.get("file"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Some(Finding {
        severity,
        claim,
        reported_confidence: obj
            .get("reported_confidence")
            .filter(|value| !value.is_null())
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        invalidated_when: optional_string_value(obj.get("invalidated_when"), "invalidated_when"),
        evidence_to_check: obj
            .get("evidence_to_check")
            .and_then(Value::as_str)
            .map(str::to_string),
        file,
        source: obj
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```json\s*(.*?)\s*```").expect("invalid audit JSON fence regex")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerQuality {
    Strong,
    Adequate,
    Weak,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeVerdict {
    pub blocker_quality: BlockerQuality,
    pub false_positive_suspected: bool,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectiveJudgment {
    pub kind: String,
    pub note: String,
    pub verdict: JudgeVerdict,
}

impl SubjectiveJudgment {
    pub fn new(verdict: JudgeVerdict) -> Self {
        Self {
            kind: "subjective_judgment".to_string(),
            note: "judge does NOT affect promote".to_string(),
            verdict,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_detection_uses_substrings_and_aliases() {
        assert_eq!(family("openai-gpt-5"), RunnerFamily::Codex);
        assert_eq!(family("deepseek-v4-pro"), RunnerFamily::Pi);
        assert!(same_family("gemini", "google-2.5"));
    }

    #[test]
    fn parses_fenced_json_and_chinese_severity() {
        let text = r#"```json
{"findings":[{"severity":"严重","claim":"blocking defect","location":{"file":"x.rs"}}]}
```"#;
        let findings = parse_findings_text(text).unwrap();
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[0].file.as_deref(), Some("x.rs"));
    }

    #[test]
    fn rejects_unknown_severity() {
        assert!(parse_findings_text(r#"[{"severity":"CRITICAL!!!","claim":"x"}]"#).is_none());
    }

    #[test]
    fn lossy_parser_keeps_valid_findings_without_bypassing_typed_normalization() {
        let values = serde_json::from_str::<Vec<Value>>(
            r#"[{"severity":"high","claim":"valid","reported_confidence":"High"},{"severity":"HIGH!!!","claim":"invalid"}]"#,
        )
        .unwrap();
        assert!(parse_findings_values(&values).is_none());
        let findings = parse_valid_findings_values(&values);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].claim, "valid");
        assert_eq!(
            findings[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.level.as_ref()),
            Some(&ReportedConfidenceLevel::High)
        );
    }

    #[test]
    fn parses_reported_confidence_object_and_optional_metadata() {
        let findings = parse_findings_text(
            r#"[{"severity":"high","claim":"x","reported_confidence":{"level":"High","rationale":"source inspection"},"invalidated_when":"the source changes"}]"#,
        )
        .unwrap();
        assert_eq!(
            findings[0].reported_confidence,
            Some(ReportedConfidence {
                level: Some(ReportedConfidenceLevel::High),
                rationale: Some("source inspection".to_string()),
            })
        );
        assert_eq!(
            findings[0].invalidated_when.as_deref(),
            Some("the source changes")
        );
    }

    #[test]
    fn parses_simplified_confidence_and_defaults_missing_metadata() {
        let simplified = parse_findings_text(
            r#"[{"severity":"medium","claim":"x","reported_confidence":"low"}]"#,
        )
        .unwrap();
        assert_eq!(
            simplified[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.level.as_ref()),
            Some(&ReportedConfidenceLevel::Low)
        );
        assert_eq!(
            simplified[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.rationale.as_ref()),
            None
        );

        let missing = parse_findings_text(r#"[{"severity":"low","claim":"y"}]"#).unwrap();
        assert_eq!(missing[0].reported_confidence, None);
        assert_eq!(missing[0].invalidated_when, None);
        let serialized = serde_json::to_value(&missing[0]).unwrap();
        assert!(serialized.get("reported_confidence").is_none());
        assert!(serialized.get("invalidated_when").is_none());

        let explicit_null =
            parse_findings_text(r#"[{"severity":"low","claim":"z","reported_confidence":null}]"#)
                .unwrap();
        assert_eq!(explicit_null[0].reported_confidence, None);
    }

    #[test]
    fn nonstandard_confidence_literal_degrades_without_dropping_finding() {
        let findings = serde_json::from_str::<Vec<Finding>>(
            r#"[{"severity":"high","claim":"x","reported_confidence":"extremely confident"}]"#,
        )
        .unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.level.as_ref()),
            None
        );
    }

    #[test]
    fn nonstring_invalidation_degrades_without_dropping_typed_finding() {
        let finding = serde_json::from_str::<Finding>(
            r#"{"severity":"high","claim":"x","invalidated_when":123}"#,
        )
        .unwrap();
        assert_eq!(finding.claim, "x");
        assert_eq!(finding.invalidated_when, None);
    }

    #[test]
    fn fenced_nested_confidence_uses_json_parser_across_layout_variations() {
        let text = r#"```json
{
  "findings": [
    {
        "severity": "critical",
        "claim": "nested metadata survives",
        "reported_confidence":
          {
             "level": "medium",
             "rationale": "multi-line object"
          },
        "invalidated_when": "counterexample appears"
    }
  ]
}
```"#;
        let findings = parse_findings_text(text).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.level.as_ref()),
            Some(&ReportedConfidenceLevel::Medium)
        );
    }

    #[test]
    fn judge_verdict_has_no_numeric_score_and_is_isolated() {
        let layer = SubjectiveJudgment::new(JudgeVerdict {
            blocker_quality: BlockerQuality::Strong,
            false_positive_suspected: false,
            rationale: None,
        });
        assert_eq!(layer.kind, "subjective_judgment");
        assert!(layer.note.contains("does NOT affect"));
    }
}
