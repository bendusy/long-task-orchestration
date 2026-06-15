use regex::Regex;
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub evidence_to_check: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
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
    let mut findings = Vec::with_capacity(items.len());
    for item in items {
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
        findings.push(Finding {
            severity,
            claim,
            evidence_to_check: obj
                .get("evidence_to_check")
                .and_then(Value::as_str)
                .map(str::to_string),
            file,
            source: obj
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Some(findings)
}

static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```json\s*(.*?)\s*```").unwrap());

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
