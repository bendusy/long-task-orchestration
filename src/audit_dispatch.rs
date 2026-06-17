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
    if allow_same_family {
        return pool;
    }
    let picked = pool
        .iter()
        .filter(|runner| !same_family(runner, host))
        .cloned()
        .collect::<Vec<_>>();
    if picked.is_empty() { pool } else { picked }
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
