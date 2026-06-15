use crate::agent_job::{
    AgentJob, Budget, Pattern, RetryPolicy, TaskSize, readonly_intent_to_policy,
};
use crate::audit::{BlockerQuality, JudgeVerdict, SubjectiveJudgment, same_family};
use crate::scheduler::{HealthProbe, Scheduler};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

pub const VERDICT_SCHEMA_VERSION: u64 = 1;
pub const MAX_JUDGE_INPUT_BYTES: usize = 256 * 1024;
pub const JUDGE_POOL: &[&str] = &["codex", "pi", "agy", "claude"];
const REDACT_SECRET: &str = "[REDACTED_SECRET]";
const REDACT_PATH: &str = "[REDACTED_PATH]";

static SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(
        r#"-----BEGIN[^-]*PRIVATE KEY-----.*?-----END[^-]*PRIVATE KEY-----|sk-ant-[A-Za-z0-9_-]{12,}|sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|github_pat_[A-Za-z0-9_]{12,}|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}|(?i:api[_-]?key|secret|token|password|passwd|pwd|access[_-]?token)\s*[:=]\s*["']?[A-Za-z0-9_\-./+=]{6,}["']?"#,
    )
    .dot_matches_new_line(true)
    .build()
    .unwrap()
});

static FULL_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:\\?/|/)(?:Users|home)(?:\\?/|/)[^\s"'`,;:)\]}]+|/root/[^\s"'`,;:)\]}]+|[A-Za-z]:\\Users\\[^\s"'`,;:)\]}]+"#,
    )
    .unwrap()
});

static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```json\s*(.*?)\s*```").unwrap());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrozenEvidence {
    pub evidence_hash: String,
    pub frozen_inputs: BTreeMap<String, String>,
    pub redaction: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum JudgeDispatchPlan {
    Ready {
        runner: String,
        evidence_hash: String,
        input_bytes: usize,
        job: Box<AgentJob>,
    },
    Skipped {
        reason: String,
        evidence_hash: String,
        #[serde(default)]
        input_bytes: Option<usize>,
        #[serde(default)]
        max_input_bytes: Option<usize>,
    },
}

pub fn judge_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "blocker_quality": {
                "type": "string",
                "enum": ["strong", "adequate", "weak", "none"],
            },
            "false_positive_suspected": {"type": "boolean"},
            "rationale": {"type": "string"},
        },
        "required": ["blocker_quality", "false_positive_suspected"],
    })
}

pub fn redact_text(text: &str) -> String {
    let without_secrets = SECRET_RE.replace_all(text, REDACT_SECRET);
    FULL_PATH_RE
        .replace_all(&without_secrets, REDACT_PATH)
        .into_owned()
}

pub fn freeze_evidence(
    case_dir: &Path,
    brief: &str,
    baseline_reply: &str,
    candidate_reply: &str,
) -> anyhow::Result<FrozenEvidence> {
    let frozen_inputs = canonical_inputs(brief, baseline_reply, candidate_reply);
    let canonical = python_style_json(&json!(frozen_inputs));
    let evidence_hash = sha256_prefixed(&canonical);
    let bundle = FrozenEvidence {
        evidence_hash,
        frozen_inputs,
        redaction: "applied".to_string(),
    };
    std::fs::create_dir_all(case_dir)?;
    std::fs::write(
        case_dir.join("frozen-evidence.json"),
        serde_json::to_string_pretty(&bundle)? + "\n",
    )?;
    Ok(bundle)
}

pub fn build_judge_prompt(frozen: &FrozenEvidence) -> String {
    let fi = &frozen.frozen_inputs;
    [
        "# LTO eval-run 主观判读简报（异构 judge）",
        "",
        &format!("- evidence_hash: {}", frozen.evidence_hash),
        "- 证据已 redact（私有路径/secret 已脱敏）。只依据下面内容判读，不要臆测原始值。",
        "",
        "你是异构质量裁判。任务：判 candidate（应用了 profile 的 reply）相对 baseline，",
        "其指出的 blocker / 问题质量如何，以及是否疑似假阳（无依据的告警）。",
        "你的判读不影响确定性 metrics，也不参与 promote，只作主观参考层。",
        "",
        "## brief（任务简报）",
        "",
        fi.get("brief").map(String::as_str).unwrap_or_default(),
        "",
        "## baseline reply（对照组，无 profile）",
        "",
        fi.get("baseline_reply")
            .map(String::as_str)
            .unwrap_or_default(),
        "",
        "## candidate reply（应用 profile）",
        "",
        fi.get("candidate_reply")
            .map(String::as_str)
            .unwrap_or_default(),
        "",
        "## 输出要求（结构化 JSON，字段必填）",
        "",
        "```json",
        r#"{"blocker_quality": "strong|adequate|weak|none", "false_positive_suspected": true, "rationale": "简短理由"}"#,
        "```",
        "",
        "blocker_quality 取值仅限 strong / adequate / weak / none。",
        "false_positive_suspected 为 bool。rationale 一句话。",
    ]
    .join("\n")
}

pub fn plan_judge_dispatch(
    repo: &Path,
    case_name: &str,
    candidate_runner: &str,
    frozen: &FrozenEvidence,
    judge_runner: Option<&str>,
    runners_dir: Option<&Path>,
) -> JudgeDispatchPlan {
    let prompt = build_judge_prompt(frozen);
    let input_bytes = prompt.len();
    if input_bytes > MAX_JUDGE_INPUT_BYTES {
        return JudgeDispatchPlan::Skipped {
            reason: format!(
                "judge input {input_bytes} bytes exceeds limit {MAX_JUDGE_INPUT_BYTES}"
            ),
            evidence_hash: frozen.evidence_hash.clone(),
            input_bytes: Some(input_bytes),
            max_input_bytes: Some(MAX_JUDGE_INPUT_BYTES),
        };
    }

    let chosen = if let Some(runner) = judge_runner {
        if same_family(runner, candidate_runner) {
            return JudgeDispatchPlan::Skipped {
                reason: format!(
                    "judge runner {runner:?} same family as candidate runner {candidate_runner:?}"
                ),
                evidence_hash: frozen.evidence_hash.clone(),
                input_bytes: Some(input_bytes),
                max_input_bytes: Some(MAX_JUDGE_INPUT_BYTES),
            };
        }
        Some(runner.to_string())
    } else {
        match runners_dir {
            Some(dir) => pick_healthy_judge_runner_with_runners_dir(repo, candidate_runner, dir),
            None => pick_healthy_judge_runner(repo, candidate_runner),
        }
    };

    let Some(chosen) = chosen else {
        return JudgeDispatchPlan::Skipped {
            reason: "no heterogeneous runner".to_string(),
            evidence_hash: frozen.evidence_hash.clone(),
            input_bytes: Some(input_bytes),
            max_input_bytes: Some(MAX_JUDGE_INPUT_BYTES),
        };
    };

    let permission_policy = readonly_intent_to_policy(&chosen);
    JudgeDispatchPlan::Ready {
        runner: chosen.clone(),
        evidence_hash: frozen.evidence_hash.clone(),
        input_bytes,
        job: Box::new(AgentJob {
            job_id: format!("judge-{case_name}-{chosen}"),
            prompt_ref: prompt,
            runner: chosen,
            prompt_is_inline: true,
            model: None,
            env: BTreeMap::new(),
            permission_policy,
            isolation: "none".to_string(),
            output_schema: Some(judge_output_schema()),
            parent_pattern: Pattern::Adversarial,
            budget: Budget {
                timeout_sec: 300,
                max_tokens: None,
            },
            retry_policy: RetryPolicy::default(),
            verifier_of: None,
            children: Vec::new(),
            task_type: Some("judge".to_string()),
            size: TaskSize::Small,
            test_cmd: None,
            needs_worktree: false,
            meta: BTreeMap::from([
                ("role".to_string(), json!("judge")),
                ("evidence_hash".to_string(), json!(frozen.evidence_hash)),
                ("candidate_runner".to_string(), json!(candidate_runner)),
            ]),
        }),
    }
}

pub fn parse_judge_reply(reply: &str) -> Option<JudgeVerdict> {
    let text = reply.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(verdict) = parse_judge_value(&value)
    {
        return Some(verdict);
    }
    FENCE_RE.captures_iter(text).find_map(|captures| {
        captures.get(1).and_then(|body| {
            serde_json::from_str::<Value>(body.as_str())
                .ok()
                .and_then(|value| parse_judge_value(&value))
        })
    })
}

pub fn subjective_judgment_from_reply(reply: &str) -> Option<SubjectiveJudgment> {
    parse_judge_reply(reply).map(SubjectiveJudgment::new)
}

pub fn freeze_verdict(
    case_dir: &Path,
    evidence_hash: &str,
    judge_runner: Option<&str>,
    status: &str,
    parsed_judgment: Option<&JudgeVerdict>,
    error: Option<&str>,
) -> anyhow::Result<String> {
    let mut payload = Map::new();
    payload.insert("schema_version".to_string(), json!(VERDICT_SCHEMA_VERSION));
    payload.insert("evidence_hash".to_string(), json!(evidence_hash));
    payload.insert("judge_runner".to_string(), json!(judge_runner));
    payload.insert("status".to_string(), json!(status));
    if let Some(judgment) = parsed_judgment {
        payload.insert(
            "parsed_judgment".to_string(),
            serde_json::to_value(judgment)?,
        );
    }
    if let Some(error) = error {
        payload.insert("error".to_string(), json!(error));
    }

    let canonical = python_style_json(&Value::Object(payload.clone()));
    let judgment_hash = sha256_prefixed(&canonical);
    payload.insert("judgment_hash".to_string(), json!(judgment_hash));
    std::fs::create_dir_all(case_dir)?;
    std::fs::write(
        case_dir.join("judge-verdict.json"),
        serde_json::to_string_pretty(&Value::Object(payload))? + "\n",
    )?;
    Ok(judgment_hash)
}

pub fn pick_healthy_judge_runner(repo: &Path, candidate_runner: &str) -> Option<String> {
    let runners_dir = repo.join("scripts").join("delegate").join("runners");
    pick_healthy_judge_runner_with_runners_dir(repo, candidate_runner, &runners_dir)
}

pub fn pick_healthy_judge_runner_with_runners_dir(
    repo: &Path,
    candidate_runner: &str,
    runners_dir: &Path,
) -> Option<String> {
    let heterogeneous = JUDGE_POOL
        .iter()
        .filter(|runner| !same_family(runner, candidate_runner))
        .map(|runner| (*runner).to_string())
        .collect::<Vec<_>>();
    if heterogeneous.is_empty() {
        return None;
    }
    let health = healthcheck_blocking(repo, runners_dir, &heterogeneous).ok()?;
    first_healthy(&heterogeneous, &health)
}

fn canonical_inputs(
    brief: &str,
    baseline_reply: &str,
    candidate_reply: &str,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("brief".to_string(), normalize_and_redact(brief)),
        (
            "baseline_reply".to_string(),
            normalize_and_redact(baseline_reply),
        ),
        (
            "candidate_reply".to_string(),
            normalize_and_redact(candidate_reply),
        ),
    ])
}

fn normalize_and_redact(text: &str) -> String {
    redact_text(&text.replace("\r\n", "\n").replace('\r', "\n"))
        .trim()
        .to_string()
}

fn parse_judge_value(value: &Value) -> Option<JudgeVerdict> {
    let obj = value.as_object()?;
    let quality = match obj
        .get("blocker_quality")?
        .as_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "strong" => BlockerQuality::Strong,
        "adequate" => BlockerQuality::Adequate,
        "weak" => BlockerQuality::Weak,
        "none" => BlockerQuality::None,
        _ => return None,
    };
    let false_positive_suspected = obj.get("false_positive_suspected")?.as_bool()?;
    let rationale = obj.get("rationale").map(|value| {
        value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string())
            .chars()
            .take(500)
            .collect::<String>()
    });
    Some(JudgeVerdict {
        blocker_quality: quality,
        false_positive_suspected,
        rationale,
    })
}

fn healthcheck_blocking(
    repo: &Path,
    runners_dir: &Path,
    runners: &[String],
) -> anyhow::Result<HealthProbe> {
    let scheduler = Scheduler::new(repo, runners_dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(runtime.block_on(scheduler.healthcheck_checked(runners))?)
}

fn first_healthy(runners: &[String], health: &HealthProbe) -> Option<String> {
    runners
        .iter()
        .find(|runner| health.get(*runner).copied().unwrap_or(false))
        .cloned()
}

fn sha256_prefixed(canonical: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn python_style_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(python_style_json)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let inner = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap(),
                        python_style_json(&map[key])
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::Sandbox;
    use std::fs;
    use std::io::Write;

    #[test]
    fn redaction_strips_secrets_and_whole_paths() {
        let raw = "token: abcdefghijk /Users/ben/private/project/file.rs \\/home\\/ben\\/x.pem";
        let redacted = redact_text(raw);
        assert!(!redacted.contains("abcdefghijk"));
        assert!(!redacted.contains("/Users/ben/private"));
        assert!(!redacted.contains("\\/home\\/ben"));
        assert!(redacted.contains(REDACT_SECRET));
        assert!(redacted.contains(REDACT_PATH));
    }

    #[test]
    fn redaction_eats_full_pem_block() {
        let raw = "a\n-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\nb";
        let redacted = redact_text(raw);
        assert_eq!(redacted, format!("a\n{REDACT_SECRET}\nb"));
    }

    #[test]
    fn freeze_evidence_is_stable_and_writes_redacted_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let a = freeze_evidence(tmp.path(), "a\r\nb", "base", "sk-ant-abcdefghijklmno").unwrap();
        let b = freeze_evidence(tmp.path(), "a\nb\n", "base", "sk-ant-abcdefghijklmno").unwrap();
        assert_eq!(a.evidence_hash, b.evidence_hash);
        assert_eq!(a.redaction, "applied");
        assert!(
            fs::read_to_string(tmp.path().join("frozen-evidence.json"))
                .unwrap()
                .contains(REDACT_SECRET)
        );
    }

    #[test]
    fn parse_judge_reply_rejects_string_false_positive() {
        assert!(
            parse_judge_reply(r#"{"blocker_quality":"strong","false_positive_suspected":"false"}"#)
                .is_none()
        );
        let parsed = parse_judge_reply(
            r#"```json
{"blocker_quality":"weak","false_positive_suspected":false,"rationale":"ok"}
```"#,
        )
        .unwrap();
        assert_eq!(parsed.blocker_quality, BlockerQuality::Weak);
        assert!(!parsed.false_positive_suspected);
        assert!(
            subjective_judgment_from_reply(
                r#"{"blocker_quality":"none","false_positive_suspected":true}"#
            )
            .is_some()
        );
    }

    #[test]
    fn freeze_verdict_hash_changes_when_judgment_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let a = JudgeVerdict {
            blocker_quality: BlockerQuality::Strong,
            false_positive_suspected: false,
            rationale: Some("a".to_string()),
        };
        let b = JudgeVerdict {
            blocker_quality: BlockerQuality::None,
            false_positive_suspected: true,
            rationale: Some("b".to_string()),
        };
        let ha = freeze_verdict(tmp.path(), "sha256:e", Some("pi"), "ok", Some(&a), None).unwrap();
        let hb = freeze_verdict(tmp.path(), "sha256:e", Some("pi"), "ok", Some(&b), None).unwrap();
        assert_ne!(ha, hb);
        assert!(ha.starts_with("sha256:"));
    }

    #[test]
    fn judge_runner_selection_filters_candidate_family_and_healthchecks() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let runners = tmp.path().join("runners");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&runners).unwrap();
        write_healthcheck(
            &runners,
            r#"[{"agent":"pi","verdict":"TIMEOUT"},{"agent":"agy","verdict":"OK"},{"agent":"claude","verdict":"OK"}]"#,
        );
        assert_eq!(
            pick_healthy_judge_runner_with_runners_dir(&repo, "codex", &runners),
            Some("agy".to_string())
        );
        write_healthcheck(
            &runners,
            r#"[{"agent":"pi","verdict":"TIMEOUT"},{"agent":"agy","verdict":"ERROR"},{"agent":"claude","verdict":"TIMEOUT"}]"#,
        );
        assert_eq!(
            pick_healthy_judge_runner_with_runners_dir(&repo, "codex", &runners),
            None
        );
    }

    #[test]
    fn judge_dispatch_plan_skips_oversize_and_same_family() {
        let tmp = tempfile::tempdir().unwrap();
        let frozen = FrozenEvidence {
            evidence_hash: "sha256:e".to_string(),
            frozen_inputs: BTreeMap::from([
                ("brief".to_string(), "b".to_string()),
                ("baseline_reply".to_string(), "base".to_string()),
                (
                    "candidate_reply".to_string(),
                    "X".repeat(MAX_JUDGE_INPUT_BYTES + 10),
                ),
            ]),
            redaction: "applied".to_string(),
        };
        assert!(matches!(
            plan_judge_dispatch(tmp.path(), "case", "codex", &frozen, None, None),
            JudgeDispatchPlan::Skipped {
                max_input_bytes: Some(MAX_JUDGE_INPUT_BYTES),
                ..
            }
        ));

        let small = FrozenEvidence {
            frozen_inputs: BTreeMap::from([
                ("brief".to_string(), "b".to_string()),
                ("baseline_reply".to_string(), "base".to_string()),
                ("candidate_reply".to_string(), "cand".to_string()),
            ]),
            ..frozen
        };
        assert!(matches!(
            plan_judge_dispatch(
                tmp.path(),
                "case",
                "codex",
                &small,
                Some("openai-gpt"),
                None
            ),
            JudgeDispatchPlan::Skipped { .. }
        ));
        if let JudgeDispatchPlan::Ready { job, .. } =
            plan_judge_dispatch(tmp.path(), "case", "codex", &small, Some("pi"), None)
        {
            assert_eq!(job.permission_policy.sandbox, Sandbox::ReadOnly);
            assert_eq!(job.output_schema, Some(judge_output_schema()));
        } else {
            panic!("explicit heterogeneous judge should be ready");
        }
    }

    fn write_healthcheck(runners: &Path, payload: &str) {
        let helper = runners.join("fake_healthcheck.py");
        let payload_literal = serde_json::to_string(payload).unwrap();
        fs::write(
            &helper,
            format!(
                r#"#!/usr/bin/env python3
import json, sys
data = json.loads({payload_literal})
requested = [arg for arg in sys.argv[1:] if arg != "--json"]
if requested:
    data = [entry for entry in data if entry.get("agent") in requested]
print(json.dumps(data))
"#
            ),
        )
        .unwrap();
        make_executable(&helper);
        let script = runners.join("healthcheck.sh");
        let mut file = fs::File::create(&script).unwrap();
        writeln!(file, "#!/usr/bin/env bash").unwrap();
        writeln!(file, "exec python3 \"{}\" \"$@\"", helper.display()).unwrap();
        make_executable(&script);
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
