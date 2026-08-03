use crate::agent_job::{
    AgentJob, AgentResult, Budget, Pattern, RetryPolicy, TaskSize, readonly_intent_to_policy,
};
use crate::audit::same_family;
use crate::scheduler::{HealthProbe, HealthcheckError, Scheduler, SchedulerError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const AUDITOR_POOL: &[&str] = &["codex", "pi", "agy"];

pub fn pick_auditors(host: &str) -> Vec<String> {
    pick_auditors_with(host, false)
}

pub fn pick_auditors_with(host: &str, allow_same_family: bool) -> Vec<String> {
    let pool = AUDITOR_POOL
        .iter()
        .map(|runner| (*runner).to_string())
        .collect::<Vec<_>>();
    if allow_same_family || host.trim().eq_ignore_ascii_case("unknown") {
        return pool;
    }
    let picked = pool
        .iter()
        .filter(|runner| !same_family(runner, host))
        .cloned()
        .collect::<Vec<_>>();
    if picked.is_empty() { pool } else { picked }
}

/// Host-controllable knob (CLAUDE.md 原则1): when `prefer` is non-empty the audit
/// pool is RESTRICTED to and ORDERED by `prefer`, intersected with the normal
/// cross-family pool. Empty `prefer` leaves today's behavior unchanged.
///
/// Restrict (not just reorder) is intentional: the scheduler runs an
/// auto-dispatch batch concurrently and `submit` only returns once every job is
/// done, so the batch blocks on the slowest auditor. Reordering codex/agy ahead
/// of a heavy-thinking `pi` does not unblock closeout while pi is still in the
/// batch (bug #8) -- removing pi from the pool does. Selection stays an explicit
/// host flag, never historical-telemetry auto-routing (原则3/原则5).
pub fn pick_auditors_preferred(
    host: &str,
    allow_same_family: bool,
    prefer: &[String],
) -> Vec<String> {
    let base = pick_auditors_with(host, allow_same_family);
    if prefer.is_empty() {
        return base;
    }
    let ordered = prefer
        .iter()
        .filter(|name| base.iter().any(|b| b == *name))
        .cloned()
        .collect::<Vec<_>>();
    if ordered.is_empty() { base } else { ordered }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditorSelection {
    pub auditors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Filter only the current run's audit failures. An explicit preference is a
/// host override and therefore bypasses this historical failure filter while
/// retaining the existing heterogeneous-pool selection rules.
pub fn pick_auditors_for_run(
    repo: &Path,
    run_id: &str,
    host: &str,
    allow_same_family: bool,
    prefer: &[String],
) -> anyhow::Result<AuditorSelection> {
    let base = pick_auditors_preferred(host, allow_same_family, prefer);
    if !prefer.is_empty() {
        return Ok(AuditorSelection {
            auditors: base,
            warnings: Vec::new(),
        });
    }

    let failure_streaks = audit_failure_streaks(repo, run_id)?;
    let mut auditors = Vec::new();
    let mut warnings = Vec::new();
    for runner in base {
        let Some(failures) = failure_streaks.get(&runner).copied() else {
            auditors.push(runner);
            continue;
        };
        if failures < 2 {
            auditors.push(runner);
            continue;
        }
        warnings.push(format!(
            "WARN auto-dispatch skipped runner {runner}: {failures} consecutive audit failures in run {run_id}; override with --prefer-runner {runner}"
        ));
    }

    Ok(AuditorSelection { auditors, warnings })
}

fn audit_failure_streaks(repo: &Path, run_id: &str) -> anyhow::Result<BTreeMap<String, usize>> {
    let mut streaks = BTreeMap::new();
    for event in crate::events::read(repo, run_id)? {
        if event.get("type").and_then(Value::as_str) != Some("runner.finished") {
            continue;
        }
        let fields = event.get("fields").and_then(Value::as_object);
        let Some(fields) = fields else { continue };
        let Some(context) = fields.get("context").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(context, "audit.auto_dispatch" | "audit.risk_discovery") {
            continue;
        }
        let Some(runner) = fields.get("runner").and_then(Value::as_str).or_else(|| {
            event
                .get("actor")
                .and_then(|actor| actor.get("id"))
                .and_then(Value::as_str)
        }) else {
            continue;
        };
        let Some(status) = fields.get("status").and_then(Value::as_str) else {
            continue;
        };
        let streak = streaks.entry(runner.to_string()).or_insert(0);
        if status == "ok" {
            *streak = 0;
        } else {
            *streak += 1;
        }
    }
    Ok(streaks)
}

pub fn pick_healthy_discoverer(repo: &Path, auditors: &[String], host: &str) -> Option<String> {
    let runners_dir = default_runners_dir(repo);
    pick_healthy_discoverer_with_runners_dir(repo, auditors, host, &runners_dir)
}

pub fn pick_healthy_discoverer_with_runners_dir(
    repo: &Path,
    auditors: &[String],
    _host: &str,
    runners_dir: &Path,
) -> Option<String> {
    if auditors.is_empty() {
        return None;
    }
    let health = healthcheck_blocking(repo, runners_dir, auditors).ok()?;
    first_healthy(auditors, &health)
}

pub fn auto_dispatch_output_schema() -> Value {
    findings_schema(&["critical", "high", "medium", "low"])
}

pub fn risk_discovery_output_schema() -> Value {
    findings_schema(&["high", "critical", "medium"])
}

pub fn build_auto_dispatch_jobs(
    brief_path: &Path,
    auditors: &[String],
    host: &str,
) -> Vec<AgentJob> {
    let output_schema = auto_dispatch_output_schema();
    auditors
        .iter()
        .map(|auditor| audit_job(auditor, brief_path, host, output_schema.clone()))
        .collect()
}

pub fn submit_auto_dispatch(
    repo: &Path,
    runners_dir: &Path,
    brief_path: &Path,
    auditors: &[String],
    host: &str,
    run_id: &str,
) -> Result<Vec<AgentResult>, SchedulerError> {
    let mut jobs = build_auto_dispatch_jobs(brief_path, auditors, host);
    for job in &mut jobs {
        job.meta.insert("run_id".to_string(), json!(run_id));
        // Session reuse (backlog ⑪ 治本): a stable per-(run, auditor) session id
        // lets the SAME auditor across audit rounds resume its persistent session
        // and hit the prompt cache (host-verified: pi resume → cacheRead>0, input
        // does not bloat). runner.sh only honors it if the CLI supports clean
        // session reuse (pi does; codex resume bloats input so pi.sh is the only
        // translator today). Backward compatible: unset = today's ephemeral behavior.
        job.env.insert(
            "LTO_SESSION_ID".to_string(),
            audit_session_id(run_id, &job.runner),
        );
    }
    crate::event_emit::emit_runner_started_jobs(
        repo,
        run_id,
        None,
        None,
        "audit.auto_dispatch",
        &jobs,
    );
    match Scheduler::new(repo, runners_dir).submit_blocking(jobs.clone()) {
        Ok(results) => Ok(results),
        Err(err) => {
            crate::event_emit::emit_runner_submission_failed_jobs(
                repo,
                run_id,
                None,
                None,
                "audit.auto_dispatch",
                &jobs,
                &err.to_string(),
            );
            Err(err)
        }
    }
}

pub fn build_risk_discovery_job(brief_path: &Path, discoverer: &str, host: &str) -> AgentJob {
    audit_job(discoverer, brief_path, host, risk_discovery_output_schema())
}

/// Stable session id for an auditor within a run, so repeated audit rounds reuse
/// the same persistent session and warm the prompt cache (backlog ⑪ 治本).
pub fn audit_session_id(run_id: &str, auditor: &str) -> String {
    format!("lto-{run_id}-audit-{auditor}")
}

fn audit_job(runner: &str, brief_path: &Path, host: &str, output_schema: Value) -> AgentJob {
    AgentJob {
        job_id: format!("audit-{runner}"),
        prompt_ref: brief_path.display().to_string(),
        runner: runner.to_string(),
        prompt_is_inline: false,
        model: None,
        // Audit is a one-shot read-only review — it does not need the runner's
        // skill/extension/context-file ecosystem. LTO_LEAN_CONTEXT tells each
        // runner.sh to disable that heavy context load (~40k→~400 tokens on pi,
        // backlog ⑪). Orthogonal to permission_policy (read-only allowlist).
        env: BTreeMap::from([("LTO_LEAN_CONTEXT".to_string(), "1".to_string())]),
        permission_policy: readonly_intent_to_policy(runner),
        isolation: "none".to_string(),
        output_schema: Some(output_schema),
        parent_pattern: Pattern::Adversarial,
        budget: Budget {
            timeout_sec: 300,
            max_tokens: None,
        },
        retry_policy: RetryPolicy::default(),
        verifier_of: None,
        children: Vec::new(),
        task_type: Some("audit".to_string()),
        size: TaskSize::Small,
        test_cmd: None,
        needs_worktree: false,
        meta: BTreeMap::from([
            ("host".to_string(), json!(host)),
            ("brief".to_string(), json!(brief_path.display().to_string())),
        ]),
    }
}

fn findings_schema(severities: &[&str]) -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "severity": {
                    "type": "string",
                    "enum": severities,
                },
                "claim": {"type": "string"},
                "reported_confidence": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": {
                                "level": {
                                    "type": "string",
                                    "enum": ["high", "medium", "low"],
                                },
                                "rationale": {"type": "string"},
                            },
                        },
                        {
                            "type": "string",
                            "enum": ["high", "medium", "low"],
                        },
                    ],
                },
                "invalidated_when": {"type": "string"},
                "evidence_to_check": {"type": "string"},
                "file": {"type": "string"},
            },
            "required": ["severity", "claim"],
        },
    })
}

fn healthcheck_blocking(
    repo: &Path,
    runners_dir: &Path,
    auditors: &[String],
) -> Result<HealthProbe, HealthcheckError> {
    let scheduler = Scheduler::new(repo, runners_dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| HealthcheckError::Io(err.to_string()))?;
    runtime.block_on(scheduler.healthcheck_checked(auditors))
}

fn first_healthy(auditors: &[String], health: &HealthProbe) -> Option<String> {
    auditors
        .iter()
        .find(|runner| health.get(*runner).copied().unwrap_or(false))
        .cloned()
}

fn default_runners_dir(repo: &Path) -> PathBuf {
    repo.join("scripts").join("delegate").join("runners")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_job::Sandbox;
    use std::fs;
    use std::io::Write;

    #[test]
    fn pick_auditors_excludes_host_family_and_can_fallback() {
        assert_eq!(pick_auditors("codex"), vec!["pi", "agy"]);
        assert_eq!(pick_auditors("pi"), vec!["codex", "agy"]);
        assert_eq!(
            pick_auditors_with("codex", true),
            vec!["codex", "pi", "agy"]
        );
        assert_eq!(pick_auditors("unknown"), vec!["codex", "pi", "agy"]);
    }

    #[test]
    fn prefer_runner_restricts_and_orders_pool_and_keeps_pi_off_critical_path() {
        // empty prefer => today's behavior unchanged
        assert_eq!(
            pick_auditors_preferred("claude", false, &[]),
            vec!["codex", "pi", "agy"]
        );
        // restrict to fast runners, pi excluded (bug #8 fix), prefer order preserved
        assert_eq!(
            pick_auditors_preferred("claude", false, &["agy".into(), "codex".into()]),
            vec!["agy", "codex"]
        );
        // single fast runner honored
        assert_eq!(
            pick_auditors_preferred("claude", false, &["codex".into()]),
            vec!["codex"]
        );
        // all-invalid prefer => fall back to base, never empty
        assert_eq!(
            pick_auditors_preferred("claude", false, &["bogus".into()]),
            vec!["codex", "pi", "agy"]
        );
        // prefer cannot resurrect a same-family runner the cross-family filter dropped:
        // host=codex base = [pi, agy]; preferring codex is invalid here -> fall back to base
        assert_eq!(
            pick_auditors_preferred("codex", false, &["codex".into()]),
            vec!["pi", "agy"]
        );
    }

    #[test]
    fn two_consecutive_audit_failures_skip_runner_and_warn() {
        let tmp = tempfile::tempdir().unwrap();
        emit_audit_result(tmp.path(), "r1", "codex", "failed", "audit.auto_dispatch");
        emit_audit_result(tmp.path(), "r1", "codex", "timeout", "audit.risk_discovery");

        let selection = pick_auditors_for_run(tmp.path(), "r1", "claude", false, &[]).unwrap();
        assert!(!selection.auditors.contains(&"codex".to_string()));
        assert_eq!(selection.warnings.len(), 1);
        assert!(selection.warnings[0].contains("WARN"));
        assert!(selection.warnings[0].contains("2 consecutive audit failures"));
        assert!(selection.warnings[0].contains("--prefer-runner codex"));
    }

    #[test]
    fn one_audit_failure_does_not_skip_runner() {
        let tmp = tempfile::tempdir().unwrap();
        emit_audit_result(tmp.path(), "r1", "codex", "failed", "audit.auto_dispatch");

        let selection = pick_auditors_for_run(tmp.path(), "r1", "claude", false, &[]).unwrap();
        assert!(selection.auditors.contains(&"codex".to_string()));
        assert!(selection.warnings.is_empty());
    }

    #[test]
    fn explicit_prefer_runner_bypasses_failure_filter() {
        let tmp = tempfile::tempdir().unwrap();
        emit_audit_result(tmp.path(), "r1", "codex", "failed", "audit.auto_dispatch");
        emit_audit_result(tmp.path(), "r1", "codex", "failed", "audit.auto_dispatch");

        let selection =
            pick_auditors_for_run(tmp.path(), "r1", "claude", false, &["codex".to_string()])
                .unwrap();
        assert_eq!(selection.auditors, vec!["codex"]);
        assert!(selection.warnings.is_empty());
    }

    #[test]
    fn discoverer_fails_closed_for_all_unhealthy_and_probe_failure_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let runners = tmp.path().join("runners");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&runners).unwrap();
        write_healthcheck(
            &runners,
            r#"[{"agent":"codex","verdict":"TIMEOUT"},{"agent":"pi","verdict":"OK"}]"#,
        );
        let auditors = vec!["codex".to_string(), "pi".to_string(), "agy".to_string()];
        assert_eq!(
            pick_healthy_discoverer_with_runners_dir(&repo, &auditors, "claude", &runners),
            Some("pi".to_string())
        );

        write_healthcheck(
            &runners,
            r#"[{"agent":"codex","verdict":"ERROR"},{"agent":"pi","verdict":"TIMEOUT"},{"agent":"agy","verdict":"ERROR"}]"#,
        );
        assert_eq!(
            pick_healthy_discoverer_with_runners_dir(&repo, &auditors, "claude", &runners),
            None
        );

        fs::remove_file(runners.join("healthcheck.sh")).unwrap();
        assert_eq!(
            pick_healthy_discoverer_with_runners_dir(&repo, &auditors, "claude", &runners),
            None
        );
    }

    #[test]
    fn audit_and_risk_paths_keep_distinct_severity_schemas() {
        let audit = auto_dispatch_output_schema();
        let risk = risk_discovery_output_schema();
        assert!(
            audit["items"]["properties"]["severity"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "low")
        );
        assert!(
            !risk["items"]["properties"]["severity"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "low")
        );
        for schema in [&audit, &risk] {
            assert!(schema["items"]["properties"]["reported_confidence"].is_object());
            assert!(schema["items"]["properties"]["invalidated_when"].is_object());
            assert_eq!(schema["items"]["required"], json!(["severity", "claim"]));
        }
    }

    #[test]
    fn jobs_use_scheduler_contract_and_readonly_intent_policy() {
        let brief = Path::new("audit/brief.md");
        let jobs =
            build_auto_dispatch_jobs(brief, &["codex".to_string(), "agy".to_string()], "claude");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].output_schema, Some(auto_dispatch_output_schema()));
        assert_eq!(jobs[0].permission_policy.sandbox, Sandbox::ReadOnly);
        assert_eq!(jobs[1].permission_policy.sandbox, Sandbox::WorkspaceWrite);
        assert!(jobs.iter().all(|job| job.validate().is_ok()));
        assert_eq!(jobs[1].parent_pattern, Pattern::Adversarial);
        // backlog ⑪: audit jobs carry LTO_LEAN_CONTEXT so runner.sh skips the
        // heavy skill/context cold-load (~40k→~400 tokens on pi).
        assert_eq!(
            jobs[0].env.get("LTO_LEAN_CONTEXT").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            jobs[1].env.get("LTO_LEAN_CONTEXT").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn audit_session_id_is_stable_per_run_and_auditor() {
        // backlog ⑪ 治本: same (run, auditor) → same id (warm cache across rounds);
        // different auditor or run → different id (no cross-contamination).
        assert_eq!(audit_session_id("run-1", "pi"), "lto-run-1-audit-pi");
        assert_eq!(
            audit_session_id("run-1", "pi"),
            audit_session_id("run-1", "pi")
        );
        assert_ne!(
            audit_session_id("run-1", "pi"),
            audit_session_id("run-1", "codex")
        );
        assert_ne!(
            audit_session_id("run-1", "pi"),
            audit_session_id("run-2", "pi")
        );
    }

    fn write_healthcheck(runners: &Path, payload: &str) {
        let script = runners.join("healthcheck.sh");
        let mut file = fs::File::create(&script).unwrap();
        writeln!(file, "#!/usr/bin/env bash").unwrap();
        writeln!(file, "cat <<'JSON'").unwrap();
        writeln!(file, "{payload}").unwrap();
        writeln!(file, "JSON").unwrap();
        make_executable(&script);
    }

    fn emit_audit_result(repo: &Path, run_id: &str, runner: &str, status: &str, context: &str) {
        crate::events::emit(
            repo,
            run_id,
            crate::events::EventRecord {
                event_type: "runner.finished".to_string(),
                actor_kind: "runner".to_string(),
                actor_id: Some(runner.to_string()),
                summary: format!("{runner} {status}"),
                fields: json!({
                    "runner": runner,
                    "status": status,
                    "context": context,
                }),
                ..crate::events::EventRecord::default()
            },
        )
        .unwrap();
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
