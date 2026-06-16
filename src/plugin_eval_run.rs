use crate::agent_job::{AgentJob, AgentResult, Budget, JobStatus, PermissionPolicy, Sandbox};
use crate::llm_judge::{self, JudgeDispatchPlan};
use crate::plugin::{self, PluginManifest};
use crate::scheduler::{Scheduler, SchedulerConfig};
use regex::{Regex, RegexBuilder};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Instant;

pub const DEFERRED_V0: &[&str] = &["automatic_promotion"];
const MAX_BRIEF_BYTES: usize = 512 * 1024;
const ENV_KEY_BLOCKLIST: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "PATH",
    "PYTHONPATH",
    "CODEX_SANDBOX",
    "IFS",
    "BASH_ENV",
    "ENV",
];

static PRIVATE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:/Users/[^/\s]+|/home/[^/\s]+|/root/|/private/(?:tmp|var)|/tmp/|/var/folders/|/Volumes/|[A-Za-z]:\\Users\\[^\\\s]+)",
    )
    .expect("invalid eval-run private path regex")
});
static FILE_URI_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"file://|\bsee\s+(?:the\s+)?(?:file|artifact|attachment|output|reply)\b")
        .case_insensitive(true)
        .build()
        .expect("invalid pointer file regex")
});
static POINTER_PHRASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"(?:written to|saved to|output to|results? (?:are )?(?:in|at)|见附件|见文件|详见|结果在|已写入|输出到|保存在|保存至|写到|写入了?|见\s*/)").case_insensitive(true).build().expect("invalid pointer phrase regex")
});
static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^```(?:json)?\s*\n(.*?)\n```\s*$").expect("invalid JSON fence regex")
});
static FENCE_SEARCH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```json\s*\n(.*?)\n```|```\s*\n([\[{].*?)\n```")
        .expect("invalid JSON fence search regex")
});

#[derive(Debug, Clone, Copy, Default)]
pub struct EvalRunOptions<'a> {
    pub eval_id: Option<&'a str>,
    pub only_case: Option<&'a str>,
    pub max_concurrency: usize,
    pub persist: bool,
    pub runners_dir: Option<&'a Path>,
}

pub fn eval_run(
    repo: &Path,
    run_id: &str,
    plugin_dir: &Path,
    options: EvalRunOptions<'_>,
) -> anyhow::Result<Value> {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let plugin_dir = plugin_dir.canonicalize()?;
    let validation = plugin::validate_plugin(&plugin_dir)?;
    if !validation.ok {
        return Ok(json!({
            "ok": false,
            "error": format!("plugin validation failed: {}", validation.errors.join("; ")),
            "plugin": plugin_dir,
        }));
    }
    let manifest = plugin::load_manifest(&plugin_dir)?;
    let Some(pack) = load_eval_pack(&plugin_dir, &manifest, options.eval_id)? else {
        return Ok(json!({
            "ok": false,
            "error": format!("eval pack not found (eval_id={})", options.eval_id.unwrap_or("null")),
            "plugin": plugin_dir,
        }));
    };
    let plugin_id = manifest.id.clone();
    let (approved_sandbox, mount_present) = mounted_sandbox(&repo, run_id, &plugin_id);
    let env_allowlist = manifest
        .security
        .env_allowlist
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let metrics = string_array(pack.get("metrics"));

    let mut cases = pack
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(only_case) = options.only_case {
        cases.retain(|case| case.get("id").and_then(Value::as_str) == Some(only_case));
        if cases.is_empty() {
            return Ok(json!({
                "ok": false,
                "error": format!("case not found: {only_case}"),
                "plugin": plugin_dir,
            }));
        }
    }

    let out_root = repo.join(".lto").join(run_id).join("plugin-eval");
    fs::create_dir_all(&out_root)?;
    let runners_dir = options
        .runners_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.join("scripts").join("delegate").join("runners"));
    let mut case_reports = Vec::new();
    let mut all_ok = true;
    for case in &cases {
        let report = run_case(
            &repo,
            run_id,
            &plugin_dir,
            case,
            &out_root,
            &approved_sandbox,
            &env_allowlist,
            &metrics,
            options.max_concurrency,
            &runners_dir,
        )?;
        all_ok = all_ok && report.get("ok").and_then(Value::as_bool) == Some(true);
        case_reports.push(report);
    }

    let mut warnings = Vec::new();
    if !mount_present {
        warnings.push("plugin not mounted for this run - ran at default read-only sandbox without a mount-lock provenance record; run `lto plugin mount` first for an auditable approval trail".to_string());
    }

    let report = json!({
        "ok": all_ok,
        "run_id": run_id,
        "plugin": plugin_dir.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
        "plugin_id": plugin_id,
        "eval_id": pack.get("id").cloned().unwrap_or(Value::Null),
        "mount_present": mount_present,
        "approved_sandbox": approved_sandbox,
        "metrics_declared": metrics,
        "cases": case_reports,
        "warnings": warnings,
        "deferred": DEFERRED_V0,
    });
    fs::write(
        out_root.join("eval-run-report.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn run_case(
    repo: &Path,
    run_id: &str,
    plugin_dir: &Path,
    case: &Value,
    out_root: &Path,
    approved_sandbox: &str,
    env_allowlist: &BTreeSet<String>,
    metrics: &[String],
    max_concurrency: usize,
    runners_dir: &Path,
) -> anyhow::Result<Value> {
    let case_id = case.get("id").and_then(Value::as_str).unwrap_or("case");
    let runner = case
        .get("runner")
        .and_then(Value::as_str)
        .unwrap_or("codex");
    let profile_id = case.get("profile").and_then(Value::as_str);
    let brief = case.get("brief").and_then(Value::as_str).unwrap_or("");
    if !crate::agent_job::KNOWN_RUNNERS.contains(&runner) {
        return Ok(json!({
            "ok": false,
            "case_id": case_id,
            "error": format!("unknown runner: {runner:?}"),
        }));
    }
    if brief.len() > MAX_BRIEF_BYTES {
        return Ok(json!({
            "ok": false,
            "case_id": case_id,
            "error": format!("brief exceeds {MAX_BRIEF_BYTES} bytes"),
        }));
    }

    let case_dir = out_root.join(case_id);
    fs::create_dir_all(&case_dir)?;
    let baseline_brief = case_dir.join("baseline-brief.md");
    fs::write(&baseline_brief, format!("{}\n", brief.trim_end()))?;
    let candidate_brief = case_dir.join("candidate-brief.md");
    let profile_id = match profile_id {
        Some(profile_id) => profile_id,
        None => {
            return Ok(json!({
                "ok": false,
                "case_id": case_id,
                "error": "render_profile failed: profile not found: null",
            }));
        }
    };
    let render_meta =
        match plugin::render_profile(plugin_dir, profile_id, &baseline_brief, &candidate_brief) {
            Ok(meta) => meta,
            Err(err) => {
                return Ok(json!({
                    "ok": false,
                    "case_id": case_id,
                    "error": format!("render_profile failed: {err}"),
                }));
            }
        };
    let output_schema = load_output_schema(plugin_dir, render_meta.get("output_schema_ref"))?;

    let mut warnings = Vec::new();
    let candidate_profile_env = candidate_env(plugin_dir, profile_id, env_allowlist, &mut warnings);
    let base_env = token_env(runner, BTreeMap::new());
    let cand_env = token_env(runner, candidate_profile_env);
    let baseline_job = make_job(
        run_id,
        case_id,
        "baseline",
        runner,
        &baseline_brief,
        base_env,
        approved_sandbox,
        None,
        None,
    )?;
    let candidate_job = make_job(
        run_id,
        case_id,
        "candidate",
        runner,
        &candidate_brief,
        cand_env,
        approved_sandbox,
        output_schema.clone(),
        Some(profile_id),
    )?;

    if case.get("case_type").and_then(Value::as_str) == Some("negative") {
        return run_negative_case(
            case,
            case_id,
            profile_id,
            &baseline_job,
            &candidate_job,
            &case_dir,
            warnings,
        );
    }

    let scheduler = Scheduler::with_config(
        repo,
        runners_dir,
        SchedulerConfig {
            max_concurrency: max_concurrency.max(1),
            ..SchedulerConfig::default()
        },
    );
    let started = Instant::now();
    let jobs = vec![baseline_job.clone(), candidate_job.clone()];
    crate::event_emit::emit_runner_started_jobs(repo, run_id, None, None, "plugin.eval_run", &jobs);
    let results = match scheduler.submit_blocking(jobs.clone()) {
        Ok(results) => {
            crate::event_emit::emit_runner_results(
                repo,
                run_id,
                None,
                None,
                "plugin.eval_run",
                &results,
            );
            results
        }
        Err(err) => {
            crate::event_emit::emit_runner_submission_failed_jobs(
                repo,
                run_id,
                None,
                None,
                "plugin.eval_run",
                &jobs,
                &err.to_string(),
            );
            return Ok(json!({
                "ok": false,
                "case_id": case_id,
                "error": err.to_string(),
            }));
        }
    };
    let wall = started.elapsed().as_secs_f64();
    let by_id = results
        .iter()
        .map(|result| (result.job_id.as_str(), result))
        .collect::<BTreeMap<_, _>>();
    let base_res = by_id.get(baseline_job.job_id.as_str()).copied();
    let cand_res = by_id.get(candidate_job.job_id.as_str()).copied();
    let base_m = deterministic_metrics(base_res, approved_sandbox, false);
    let cand_m = deterministic_metrics(cand_res, approved_sandbox, output_schema.is_some());
    let case_ok = base_res.is_some_and(AgentResult::ok) && cand_res.is_some_and(AgentResult::ok);
    let token_available = base_m.get("tokens").and_then(Value::as_f64).is_some()
        && cand_m.get("tokens").and_then(Value::as_f64).is_some();
    let base_reply = base_res
        .map(|result| result.reply_text.as_str())
        .unwrap_or("");
    let cand_reply = cand_res
        .map(|result| result.reply_text.as_str())
        .unwrap_or("");
    let frozen = llm_judge::freeze_evidence(&case_dir, brief, base_reply, cand_reply)?;
    let judge = run_judge(
        repo,
        run_id,
        case_id,
        runner,
        &case_dir,
        &frozen,
        runners_dir,
    )?;

    let comparison = json!({
        "ok": case_ok,
        "case_id": case_id,
        "runner": runner,
        "profile": profile_id,
        "wall_clock_sec": (wall * 1000.0).round() / 1000.0,
        "baseline": base_m,
        "candidate": cand_m,
        "deltas": deltas(&base_m, &cand_m),
        "metrics_declared": metrics,
        "token_metering_available": token_available,
        "evidence_hash": frozen.evidence_hash,
        "judge": judge,
        "warnings": warnings,
        "deferred": DEFERRED_V0,
    });
    fs::write(
        case_dir.join("comparison.json"),
        serde_json::to_string_pretty(&comparison)? + "\n",
    )?;
    dump_result(&case_dir.join("baseline-result.json"), base_res)?;
    dump_result(&case_dir.join("candidate-result.json"), cand_res)?;
    Ok(comparison)
}

fn run_negative_case(
    case: &Value,
    case_id: &str,
    profile_id: &str,
    baseline_job: &AgentJob,
    candidate_job: &AgentJob,
    case_dir: &Path,
    warnings: Vec<String>,
) -> anyhow::Result<Value> {
    let expected_outcome = case
        .get("expected_outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    if expected_outcome != "scheduler_reject" {
        return Ok(json!({
            "ok": false,
            "case_id": case_id,
            "error": format!("unsupported negative expected_outcome: {expected_outcome:?}"),
        }));
    }
    let needle = case
        .get("expected_error_contains")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rejection_error = baseline_job
        .validate()
        .and_then(|_| candidate_job.validate())
        .err()
        .map(|err| err.to_string())
        .unwrap_or_default();
    let rejected = !rejection_error.is_empty();
    let ok = rejected && (needle.is_empty() || rejection_error.contains(needle));
    let comparison = json!({
        "ok": ok,
        "case_id": case_id,
        "case_type": "negative",
        "runner": baseline_job.runner,
        "profile": profile_id,
        "expected_outcome": expected_outcome,
        "expected_error_contains": needle,
        "rejected": rejected,
        "rejection_error": rejection_error,
        "note": "negative case: passes when dispatch is rejected fail-closed at validate stage; no agents spawned, no judge layer",
        "warnings": warnings,
        "deferred": DEFERRED_V0,
    });
    fs::write(
        case_dir.join("comparison.json"),
        serde_json::to_string_pretty(&comparison)? + "\n",
    )?;
    Ok(comparison)
}

#[allow(clippy::too_many_arguments)]
fn make_job(
    run_id: &str,
    case_id: &str,
    leg: &str,
    runner: &str,
    prompt_ref: &Path,
    env: BTreeMap<String, String>,
    approved_sandbox: &str,
    output_schema: Option<Value>,
    profile_id: Option<&str>,
) -> anyhow::Result<AgentJob> {
    let sandbox = Sandbox::from_str(approved_sandbox)?;
    let permission_policy = PermissionPolicy {
        sandbox,
        reason: if sandbox == Sandbox::ReadOnly {
            String::new()
        } else {
            format!("eval-run mount-approved sandbox for case {case_id}")
        },
        user_approved: sandbox == Sandbox::DangerFullAccess,
        tools: Vec::new(),
    };
    let mut meta = BTreeMap::from([
        ("run_id".to_string(), json!(run_id)),
        ("eval_case".to_string(), json!(case_id)),
        ("leg".to_string(), json!(leg)),
    ]);
    if let Some(profile_id) = profile_id {
        meta.insert("profile".to_string(), json!(profile_id));
    }
    Ok(AgentJob {
        job_id: format!("eval-{case_id}-{leg}"),
        prompt_ref: prompt_ref.display().to_string(),
        runner: runner.to_string(),
        prompt_is_inline: false,
        model: None,
        env,
        permission_policy,
        isolation: "none".to_string(),
        output_schema,
        parent_pattern: Default::default(),
        budget: Budget::default(),
        retry_policy: Default::default(),
        verifier_of: None,
        children: Vec::new(),
        task_type: Some("eval".to_string()),
        size: Default::default(),
        test_cmd: None,
        needs_worktree: false,
        meta,
    })
}

fn run_judge(
    repo: &Path,
    run_id: &str,
    case_id: &str,
    runner: &str,
    case_dir: &Path,
    frozen: &llm_judge::FrozenEvidence,
    runners_dir: &Path,
) -> anyhow::Result<Value> {
    let plan =
        llm_judge::plan_judge_dispatch(repo, case_id, runner, frozen, None, Some(runners_dir));
    let JudgeDispatchPlan::Ready {
        runner: judge_runner,
        mut job,
        ..
    } = plan
    else {
        let payload = serde_json::to_value(plan)?;
        crate::event_emit::emit_judge_skipped(
            repo,
            run_id,
            case_id,
            payload.get("reason").and_then(Value::as_str),
        );
        let _ = llm_judge::freeze_verdict(
            case_dir,
            &frozen.evidence_hash,
            None,
            "skipped",
            None,
            payload.get("reason").and_then(Value::as_str),
        )?;
        return Ok(payload);
    };
    job.meta.insert("run_id".to_string(), json!(run_id));
    let jobs = vec![*job];
    crate::event_emit::emit_runner_started_jobs(
        repo,
        run_id,
        None,
        None,
        "plugin.eval_run.judge",
        &jobs,
    );
    let scheduler = Scheduler::new(repo, runners_dir);
    let results = scheduler.submit_blocking(jobs.clone());
    let result = match results {
        Ok(mut results) => {
            crate::event_emit::emit_runner_results(
                repo,
                run_id,
                None,
                None,
                "plugin.eval_run.judge",
                &results,
            );
            results.pop()
        }
        Err(err) => {
            crate::event_emit::emit_runner_submission_failed_jobs(
                repo,
                run_id,
                None,
                None,
                "plugin.eval_run.judge",
                &jobs,
                &err.to_string(),
            );
            let judgment_hash = llm_judge::freeze_verdict(
                case_dir,
                &frozen.evidence_hash,
                Some(&judge_runner),
                "failed",
                None,
                Some(&err.to_string()),
            )?;
            crate::event_emit::emit_decision_escalated(
                repo,
                run_id,
                "judge scheduler failed",
                json!({
                    "case_id": case_id,
                    "judge_runner": judge_runner,
                    "error": err.to_string(),
                }),
            );
            return Ok(json!({
                "status": "failed",
                "judge_runner": judge_runner,
                "evidence_hash": frozen.evidence_hash,
                "judgment_hash": judgment_hash,
                "error": err.to_string(),
            }));
        }
    };
    let Some(result) = result else {
        crate::event_emit::emit_decision_escalated(
            repo,
            run_id,
            "judge result missing",
            json!({
                "case_id": case_id,
                "judge_runner": judge_runner,
            }),
        );
        return Ok(json!({
            "status": "failed",
            "judge_runner": judge_runner,
            "evidence_hash": frozen.evidence_hash,
            "error": "judge result missing",
        }));
    };
    let judgment = llm_judge::parse_judge_reply(&result.reply_text);
    let status = if result.status == JobStatus::Ok && judgment.is_some() {
        "ok"
    } else {
        "failed"
    };
    let error = (!result.error.is_empty()).then_some(result.error.as_str());
    let judgment_hash = llm_judge::freeze_verdict(
        case_dir,
        &frozen.evidence_hash,
        Some(&judge_runner),
        status,
        judgment.as_ref(),
        error,
    )?;
    if let Some(judgment) = &judgment {
        crate::event_emit::emit_decision_voted(
            repo,
            run_id,
            &judge_runner,
            "llm_judge",
            json!({
                "case_id": case_id,
                "judge_runner": judge_runner,
                "status": status,
                "blocker_quality": format!("{:?}", judgment.blocker_quality).to_ascii_lowercase(),
                "false_positive_suspected": judgment.false_positive_suspected,
                "evidence_hash": frozen.evidence_hash,
            }),
        );
    } else if status == "failed" {
        crate::event_emit::emit_decision_escalated(
            repo,
            run_id,
            "judge reply did not parse",
            json!({
                "case_id": case_id,
                "judge_runner": judge_runner,
                "result_status": result.status.as_str(),
            }),
        );
    }
    Ok(json!({
        "status": status,
        "judge_runner": judge_runner,
        "evidence_hash": frozen.evidence_hash,
        "judgment_hash": judgment_hash,
        "parsed_judgment": judgment,
        "result_status": result.status.as_str(),
        "error": error,
    }))
}

fn load_eval_pack(
    plugin_dir: &Path,
    manifest: &PluginManifest,
    eval_id: Option<&str>,
) -> anyhow::Result<Option<Value>> {
    for rel in manifest
        .provides
        .get("evals")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let path = plugin::safe_plugin_file(plugin_dir, rel)?;
        let data = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
        if data.as_object().is_none() {
            continue;
        }
        if eval_id.is_none() || data.get("id").and_then(Value::as_str) == eval_id {
            return Ok(Some(data));
        }
    }
    Ok(None)
}

fn load_output_schema(
    plugin_dir: &Path,
    schema_ref: Option<&Value>,
) -> anyhow::Result<Option<Value>> {
    let Some(schema_ref) = schema_ref.and_then(Value::as_str) else {
        return Ok(None);
    };
    let path = plugin::safe_plugin_file(plugin_dir, schema_ref)?;
    let data = serde_json::from_str::<Value>(&fs::read_to_string(path)?)?;
    Ok(data.as_object().is_some().then_some(data))
}

fn mounted_sandbox(repo: &Path, run_id: &str, plugin_id: &str) -> (String, bool) {
    let lock_path = repo.join(".lto").join(run_id).join("plugin-mounts.json");
    let Ok(text) = fs::read_to_string(lock_path) else {
        return ("read-only".to_string(), false);
    };
    let Ok(lock) = serde_json::from_str::<Value>(&text) else {
        return ("read-only".to_string(), false);
    };
    for entry in lock
        .get("mounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if entry.get("plugin_id").and_then(Value::as_str) == Some(plugin_id) {
            let sandbox = entry
                .get("approved_permissions")
                .and_then(|value| value.get("max_sandbox"))
                .and_then(Value::as_str)
                .unwrap_or("read-only");
            return (sandbox.to_string(), true);
        }
    }
    ("read-only".to_string(), false)
}

fn candidate_env(
    plugin_dir: &Path,
    profile_id: &str,
    env_allowlist: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let Ok(profile) = plugin::load_profile(plugin_dir, profile_id) else {
        return BTreeMap::new();
    };
    let Some(env) = profile.get("env").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    filter_candidate_env(env, env_allowlist, warnings)
}

fn filter_candidate_env(
    env: &serde_json::Map<String, Value>,
    env_allowlist: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, value) in env {
        let Some(value) = value.as_str() else {
            continue;
        };
        if ENV_KEY_BLOCKLIST.contains(&key.as_str()) {
            warnings.push(format!(
                "profile env key {key:?} dropped (blocklisted privilege/loader key)"
            ));
            continue;
        }
        if !env_allowlist.contains(key) {
            warnings.push(format!(
                "profile env key {key:?} dropped (not in plugin env_allowlist)"
            ));
            continue;
        }
        if key == "CODEX_JSON" && value.trim() == "0" {
            warnings.push(
                "profile sets CODEX_JSON=0; token metering disabled for candidate".to_string(),
            );
        }
        out.insert(key.clone(), value.to_string());
    }
    out
}

fn token_env(runner: &str, mut env: BTreeMap<String, String>) -> BTreeMap<String, String> {
    if runner == "codex" {
        env.entry("CODEX_JSON".to_string())
            .or_insert_with(|| "1".to_string());
    }
    env
}

fn deterministic_metrics(
    res: Option<&AgentResult>,
    approved_sandbox: &str,
    has_schema: bool,
) -> Value {
    let Some(res) = res else {
        return json!({
            "ran": false,
            "status": "missing",
            "parse_ok": Value::Null,
            "timeout": false,
            "permission_violation": Value::Null,
            "private_path_leak": false,
            "pointer_only": Value::Null,
            "elapsed_sec": Value::Null,
            "tokens": Value::Null,
        });
    };
    let parsed_substantive = json_parses(&res.reply_text);
    json!({
        "ran": res.status != JobStatus::Skipped,
        "status": res.status.as_str(),
        "exit_code": res.exit_code,
        "parse_ok": if has_schema { Value::Bool(parsed_substantive) } else { Value::Null },
        "timeout": res.exit_code == Some(124) || res.status == JobStatus::Timeout,
        "permission_violation": sandbox_exceeds(&res.permissions, approved_sandbox),
        "private_path_leak": PRIVATE_PATH_RE.is_match(&res.reply_text),
        "pointer_only": is_pointer_only(&res.reply_text, parsed_substantive),
        "elapsed_sec": res.cost.get("elapsed_sec").cloned().unwrap_or(Value::Null),
        "tokens": res.cost.get("tokens").cloned().unwrap_or(Value::Null),
    })
}

fn deltas(base: &Value, cand: &Value) -> Value {
    let elapsed_delta = match (num(base.get("elapsed_sec")), num(cand.get("elapsed_sec"))) {
        (Some(base), Some(cand)) => Some(cand - base),
        _ => None,
    };
    let token_delta = match (num(base.get("tokens")), num(cand.get("tokens"))) {
        (Some(base), Some(cand)) => Some(cand - base),
        _ => None,
    };
    json!({
        "elapsed_delta_sec": maybe_num(elapsed_delta),
        "token_delta": maybe_num(token_delta),
        "candidate_new_timeout": bool_v(cand.get("timeout")) && !bool_v(base.get("timeout")),
        "candidate_new_permission_violation": bool_v(cand.get("permission_violation")) && !bool_v(base.get("permission_violation")),
        "candidate_new_private_path_leak": bool_v(cand.get("private_path_leak")) && !bool_v(base.get("private_path_leak")),
        "candidate_new_pointer_only": bool_v(cand.get("pointer_only")) && !bool_v(base.get("pointer_only")),
    })
}

fn num(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn maybe_num(value: Option<f64>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

fn bool_v(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn sandbox_exceeds(snapshot: &BTreeMap<String, Value>, approved: &str) -> bool {
    let rank = |sandbox: &str| match sandbox {
        "read-only" => Some(0),
        "workspace-write" => Some(1),
        "danger-full-access" => Some(2),
        _ => None,
    };
    let Some(approved_rank) = rank(approved) else {
        return true;
    };
    let Some(actual_rank) = snapshot
        .get("sandbox")
        .and_then(Value::as_str)
        .and_then(rank)
    else {
        return true;
    };
    actual_rank > approved_rank
}

fn is_pointer_only(reply: &str, parsed_substantive: bool) -> bool {
    if parsed_substantive {
        return false;
    }
    let stripped = reply.trim();
    if stripped.is_empty() || stripped.len() > 200 {
        return false;
    }
    FILE_URI_RE.is_match(stripped)
        || (POINTER_PHRASE_RE.is_match(stripped) && PRIVATE_PATH_RE.is_match(stripped))
        || (PRIVATE_PATH_RE.is_match(stripped) && stripped.len() < 110)
}

fn json_parses(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let candidate = FENCE_RE
        .captures(text)
        .and_then(|captures| captures.get(1).map(|body| body.as_str().trim().to_string()))
        .or_else(|| {
            FENCE_SEARCH_RE.captures(text).and_then(|captures| {
                captures
                    .get(1)
                    .or_else(|| captures.get(2))
                    .map(|body| body.as_str().trim().to_string())
            })
        })
        .unwrap_or_else(|| text.to_string());
    serde_json::from_str::<Value>(&candidate).is_ok()
}

fn dump_result(path: &Path, res: Option<&AgentResult>) -> anyhow::Result<()> {
    if let Some(res) = res {
        fs::write(path, serde_json::to_string_pretty(res)? + "\n")?;
    } else {
        fs::write(path, "null\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn eval_run_compiles_two_legs_and_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".lto").join("r1")).unwrap();
        fs::write(repo.join(".lto").join("r1").join("state.json"), "{}\n").unwrap();
        let plugin = mini_plugin(&repo, true);
        let runners = fake_runner_dir(tmp.path(), r#"{"findings":[]}"#, None, 0);

        let report = eval_run(
            &repo,
            "r1",
            &plugin,
            EvalRunOptions {
                max_concurrency: 1,
                runners_dir: Some(&runners),
                ..EvalRunOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report["ok"], true);
        let case = &report["cases"][0];
        assert_eq!(case["candidate"]["parse_ok"], true);
        assert_eq!(case["baseline"]["parse_ok"], Value::Null);
        let case_dir = repo.join(".lto").join("r1").join("plugin-eval").join("c1");
        assert!(case_dir.join("baseline-result.json").exists());
        assert!(case_dir.join("candidate-result.json").exists());
        assert!(case_dir.join("comparison.json").exists());
        assert!(
            fs::read_to_string(case_dir.join("candidate-brief.md"))
                .unwrap()
                .contains("OUTPUT MUST BE JSON FINDINGS.")
        );
        let events = crate::events::read(&repo, "r1").unwrap();
        assert!(
            events.iter().any(|event| {
                event.get("type").and_then(Value::as_str) == Some("runner.started")
            })
        );
        assert!(
            events.iter().any(|event| {
                event.get("type").and_then(Value::as_str) == Some("runner.finished")
            })
        );
        assert!(events.iter().any(|event| matches!(
            event.get("type").and_then(Value::as_str),
            Some("judge.skipped" | "decision.voted" | "decision.escalated")
        )));
    }

    #[test]
    fn eval_run_reports_token_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".lto").join("rt")).unwrap();
        fs::write(repo.join(".lto").join("rt").join("state.json"), "{}\n").unwrap();
        let plugin = mini_plugin(&repo, false);
        let runners = fake_runner_dir(
            tmp.path(),
            r#"{"findings":[]}"#,
            Some(r#"{"tokens_in":1000,"tokens_out":200,"tokens":1200}"#),
            0,
        );

        let report = eval_run(
            &repo,
            "rt",
            &plugin,
            EvalRunOptions {
                max_concurrency: 1,
                runners_dir: Some(&runners),
                ..EvalRunOptions::default()
            },
        )
        .unwrap();
        let case = &report["cases"][0];
        assert_eq!(case["token_metering_available"], true);
        assert_eq!(case["candidate"]["tokens"], 1200);
        assert_eq!(case["deltas"]["token_delta"], 0.0);
    }

    #[test]
    fn eval_run_negative_case_passes_on_scheduler_reject() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".lto").join("rn")).unwrap();
        fs::write(repo.join(".lto").join("rn").join("state.json"), "{}\n").unwrap();
        let plugin = mini_plugin(&repo, false);
        let pack = plugin.join("evals").join("cases.json");
        fs::write(
            &pack,
            r#"{"id":"mini-cases-v1","metrics":["parse_rate"],"cases":[{"id":"cneg","runner":"agy","profile":"mini-profile-v1","brief":"x","case_type":"negative","expected_outcome":"scheduler_reject","expected_error_contains":"cannot enforce read-only"}]}"#,
        )
        .unwrap();
        let runners = fake_runner_dir(tmp.path(), "{}", None, 0);

        let report = eval_run(
            &repo,
            "rn",
            &plugin,
            EvalRunOptions {
                max_concurrency: 1,
                runners_dir: Some(&runners),
                ..EvalRunOptions::default()
            },
        )
        .unwrap();
        let case = &report["cases"][0];
        assert_eq!(case["ok"], true);
        assert_eq!(case["rejected"], true);
        assert!(
            case["rejection_error"]
                .as_str()
                .unwrap()
                .contains("cannot enforce read-only")
        );
    }

    #[test]
    fn eval_run_passes_allowlisted_candidate_env() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let plugin = mini_plugin(&repo, false);
        let profile = plugin.join("profiles").join("p.json");
        fs::write(
            &profile,
            r#"{"id":"mini-profile-v1","prompt_suffix":"x","env":{"CODEX_PROFILE":"ok"},"permission":{"sandbox":"read-only"}}"#,
        )
        .unwrap();
        let manifest = plugin.join("plugin.json");
        let mut data =
            serde_json::from_str::<Value>(&fs::read_to_string(&manifest).unwrap()).unwrap();
        data["security"]["env_allowlist"] = json!(["CODEX_PROFILE"]);
        fs::write(&manifest, serde_json::to_string(&data).unwrap()).unwrap();
        let mut warnings = Vec::new();

        let env = candidate_env(
            &plugin,
            "mini-profile-v1",
            &BTreeSet::from(["CODEX_PROFILE".to_string(), "PATH".to_string()]),
            &mut warnings,
        );
        assert_eq!(env.get("CODEX_PROFILE").map(String::as_str), Some("ok"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn eval_run_filter_blocks_dangerous_env_even_if_allowlisted() {
        let env = serde_json::Map::from_iter([
            ("CODEX_PROFILE".to_string(), json!("ok")),
            ("PATH".to_string(), json!("/bad")),
        ]);
        let mut warnings = Vec::new();
        let filtered = filter_candidate_env(
            &env,
            &BTreeSet::from(["CODEX_PROFILE".to_string(), "PATH".to_string()]),
            &mut warnings,
        );
        assert_eq!(
            filtered.get("CODEX_PROFILE").map(String::as_str),
            Some("ok")
        );
        assert!(!filtered.contains_key("PATH"));
        assert!(warnings.iter().any(|warning| warning.contains("PATH")));
    }

    #[test]
    fn eval_run_pack_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let plugin = mini_plugin(&repo, false);
        let report = eval_run(
            &repo,
            "r",
            &plugin,
            EvalRunOptions {
                eval_id: Some("missing"),
                max_concurrency: 1,
                ..EvalRunOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report["ok"], false);
        assert!(
            report["error"]
                .as_str()
                .unwrap()
                .contains("eval pack not found")
        );
    }

    fn mini_plugin(repo: &Path, with_schema: bool) -> PathBuf {
        let plugin = repo.join("plugins").join("mini");
        fs::create_dir_all(plugin.join("profiles")).unwrap();
        fs::create_dir_all(plugin.join("evals")).unwrap();
        fs::create_dir_all(plugin.join("sources")).unwrap();
        fs::create_dir_all(plugin.join("schemas")).unwrap();
        fs::write(
            plugin.join("sources").join("note.json"),
            r#"{"id":"note.mini","url":"https://example.test","claims":[]}"#,
        )
        .unwrap();
        let schema_ref = if with_schema {
            fs::write(
                plugin.join("schemas").join("findings.json"),
                r#"{"type":"object"}"#,
            )
            .unwrap();
            r#","output_schema_ref":"schemas/findings.json""#
        } else {
            ""
        };
        fs::write(
            plugin.join("profiles").join("p.json"),
            format!(
                r#"{{"id":"mini-profile-v1","prompt_suffix":"OUTPUT MUST BE JSON FINDINGS."{schema_ref},"permission":{{"sandbox":"read-only"}}}}"#
            ),
        )
        .unwrap();
        fs::write(
            plugin.join("evals").join("cases.json"),
            r#"{"id":"mini-cases-v1","metrics":["parse_rate","private_path_leaks"],"cases":[{"id":"c1","runner":"codex","profile":"mini-profile-v1","brief":"Audit this spec."}]}"#,
        )
        .unwrap();
        fs::write(
            plugin.join("plugin.json"),
            r#"{"id":"mini","version":"0.1.0","kind":"path-plugin","stage":"experimental","security":{"executable_code":false,"max_sandbox":"read-only"},"source_notes":["sources/note.json"],"provides":{"profiles":["profiles/p.json"],"evals":["evals/cases.json"]}}"#,
        )
        .unwrap();
        plugin
    }

    fn fake_runner_dir(root: &Path, reply: &str, meta: Option<&str>, exit_code: i32) -> PathBuf {
        let runners = root.join("runners");
        fs::create_dir_all(&runners).unwrap();
        let script = runners.join("codex.sh");
        let meta_write = meta
            .map(|meta| {
                format!(
                    "printf '%s\\n' '{}' > \"$2.meta.json\"\n",
                    meta.replace('\'', "'\\''")
                )
            })
            .unwrap_or_default();
        fs::write(
            &script,
            format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' '{}' > \"$2\"\n{}exit {}\n",
                reply.replace('\'', "'\\''"),
                meta_write,
                exit_code
            ),
        )
        .unwrap();
        make_executable(&script);
        let health = runners.join("healthcheck.sh");
        fs::write(
            &health,
            "#!/usr/bin/env bash\nprintf '%s\\n' '[{\"agent\":\"codex\",\"verdict\":\"OK\"}]'\n",
        )
        .unwrap();
        make_executable(&health);
        runners
    }

    fn make_executable(path: &Path) {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}
