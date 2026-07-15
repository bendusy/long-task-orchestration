use crate::commands::util;
use crate::event_emit::{self, ContractFieldCounts};
use crate::state::{self, DeliveryContract};
use anyhow::Context;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ContractSetOptions {
    pub run_id: Option<String>,
    pub goal: Option<String>,
    pub done_when: Option<String>,
    pub host: Option<String>,
    pub targets: Vec<String>,
    pub constraints: Vec<String>,
    pub instruments: Vec<String>,
    pub replacement_instruments: Vec<String>,
    pub entropy_checks: Vec<String>,
}

pub fn cmd_contract_set(repo: &Path, options: ContractSetOptions) -> anyhow::Result<()> {
    let run_id = util::resolve_run_id(repo, options.run_id.as_deref())?;
    let _run_lock = util::lock_existing_run(repo, &run_id)?;
    let mut ctx = util::load_run(repo, Some(&run_id))?;
    let original_state = ctx.state.clone();
    if !options.instruments.is_empty() && !options.replacement_instruments.is_empty() {
        anyhow::bail!("--instrument cannot be used with --replace-instrument");
    }
    let replace_instruments = !options.replacement_instruments.is_empty();
    let delta = DeliveryContract::new(
        options.targets,
        options.constraints,
        options.instruments,
        options.entropy_checks,
    );
    let replacements = DeliveryContract::new(
        Vec::new(),
        Vec::new(),
        options.replacement_instruments,
        Vec::new(),
    )
    .instruments;
    let mut changed_fields = Vec::new();

    if let Some(goal) = options.goal {
        ctx.state.goal = goal;
        changed_fields.push("goal");
    }
    if let Some(done_when) = options.done_when {
        ctx.state.done_when = done_when;
        changed_fields.push("done_when");
    }
    if let Some(host) = options.host {
        let host = host.trim();
        ctx.state.host_runtime = if host.is_empty() { "unknown" } else { host }.to_string();
        changed_fields.push("host_runtime");
    }
    append_values(
        &mut ctx.state.delivery_contract.targets,
        delta.targets,
        "targets",
        &mut changed_fields,
    );
    append_values(
        &mut ctx.state.delivery_contract.constraints,
        delta.constraints,
        "constraints",
        &mut changed_fields,
    );
    if replace_instruments {
        ctx.state.delivery_contract.instruments = replacements;
        changed_fields.push("instruments");
    } else {
        append_values(
            &mut ctx.state.delivery_contract.instruments,
            delta.instruments,
            "instruments",
            &mut changed_fields,
        );
    }
    append_values(
        &mut ctx.state.delivery_contract.forced_entropy,
        delta.forced_entropy,
        "forced_entropy",
        &mut changed_fields,
    );

    let readiness = state::assess_run_readiness(
        &ctx.state.goal,
        &ctx.state.done_when,
        &ctx.state.why,
        &ctx.state.host_runtime,
    );
    if !readiness.is_ready() {
        anyhow::bail!(
            "需补充: {}\n（信息不足禁猜：没有完成标准的 run 无法判收敛，recap/closeout 都会退化）",
            format_flag_hints(&readiness.missing)
        );
    }
    let completeness = ctx.state.delivery_contract.completeness_missing();
    if !completeness.is_complete() {
        anyhow::bail!(
            "需补充: {}\n（信息不足禁猜：目标与测量手段必须成对，目标必须有可验证的测量手段）",
            format_flag_hints(&completeness.missing)
        );
    }
    for flag in readiness.advisory {
        eprintln!("WARN 需补充: {}", flag_hint(flag));
    }
    for flag in completeness.advisory {
        eprintln!("WARN delivery contract 可补充: {}", flag_hint(flag));
    }
    if changed_fields.is_empty() {
        println!("contract unchanged: 0 field(s)");
        return Ok(());
    }

    let counts = ContractFieldCounts {
        targets: ctx.state.delivery_contract.targets.len(),
        constraints: ctx.state.delivery_contract.constraints.len(),
        instruments: ctx.state.delivery_contract.instruments.len(),
        forced_entropy: ctx.state.delivery_contract.forced_entropy.len(),
    };
    let run_state_path = ctx.run_dir.join("run-state.md");
    let original_run_state = match fs::read_to_string(&run_state_path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", run_state_path.display()));
        }
    };
    let run_state_seed = original_run_state
        .as_deref()
        .unwrap_or(include_str!("../../templates/run-state.md"));
    let updated_run_state = util::render_synced_run_state_md(run_state_seed, &ctx.state);

    util::save_run_locked(&ctx)?;
    let finish_result = (|| -> anyhow::Result<()> {
        util::atomic_write(&run_state_path, updated_run_state.as_bytes())?;
        event_emit::emit_contract_updated(
            repo,
            &ctx.run_id,
            &ctx.state.current_phase,
            &changed_fields,
            counts,
        )
    })();
    if let Err(error) = finish_result {
        return Err(rollback_contract_update(
            &ctx,
            &original_state,
            &run_state_path,
            original_run_state.as_deref(),
            error,
        ));
    }
    println!("contract updated: {} field(s)", changed_fields.len());
    Ok(())
}

fn rollback_contract_update(
    ctx: &util::RunContext,
    original_state: &state::LtoState,
    run_state_path: &Path,
    original_run_state: Option<&str>,
    cause: anyhow::Error,
) -> anyhow::Error {
    let mut rollback_errors = Vec::new();
    if let Err(error) = state::save_state(&ctx.state_path, original_state) {
        rollback_errors.push(format!("state rollback failed: {error}"));
    }
    if let Some(content) = original_run_state
        && let Err(error) = util::atomic_write(run_state_path, content.as_bytes())
    {
        rollback_errors.push(format!("run-state rollback failed: {error}"));
    } else if original_run_state.is_none()
        && run_state_path.exists()
        && let Err(error) = fs::remove_file(run_state_path)
    {
        rollback_errors.push(format!("run-state rollback failed: {error}"));
    }
    if rollback_errors.is_empty() {
        cause.context("contract update rolled back after a persistence failure")
    } else {
        anyhow::anyhow!("{cause}; {}", rollback_errors.join("; "))
    }
}

fn append_values(
    destination: &mut Vec<String>,
    values: Vec<String>,
    field: &'static str,
    changed_fields: &mut Vec<&'static str>,
) {
    if values.is_empty() {
        return;
    }
    destination.extend(values);
    changed_fields.push(field);
}

fn format_flag_hints(flags: &[&str]) -> String {
    flags
        .iter()
        .map(|flag| flag_hint(flag))
        .collect::<Vec<_>>()
        .join(" ")
}

fn flag_hint(flag: &str) -> String {
    match flag {
        "--goal" => "--goal \"<一句话目标>\"".to_string(),
        "--done-when" => "--done-when \"<怎么算做完>\"".to_string(),
        "--why" => "--why \"<为什么要做>\"".to_string(),
        "--host" => "--host \"<当前 host runtime>\"（当前按 unknown 记录）".to_string(),
        "--target" => "--target \"<可验证目标>\"".to_string(),
        "--constraint" => "--constraint \"<交付约束>\"".to_string(),
        "--instrument" => "--instrument \"<label>::<测量命令>\"".to_string(),
        "--entropy-check" => "--entropy-check \"<停滞时的换假设检查>\"".to_string(),
        _ => flag.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LtoState;
    use serde_json::json;

    #[test]
    fn contract_set_preserves_flattened_extra_and_labeled_instrument() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto/r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(repo.join(".lto/current"), "r1\n").unwrap();
        let mut state = LtoState {
            run_id: "r1".to_string(),
            goal: "ship".to_string(),
            why: "user value".to_string(),
            done_when: "tests pass".to_string(),
            host_runtime: "codex".to_string(),
            ..LtoState::default()
        };
        state
            .delivery_contract
            .extra
            .insert("future_contract_key".into(), json!({"kept": true}));
        crate::state::save_state(run_dir.join("state.json"), &state).unwrap();

        cmd_contract_set(
            repo,
            ContractSetOptions {
                run_id: Some("r1".to_string()),
                goal: None,
                done_when: None,
                host: None,
                targets: vec!["measurable target".to_string()],
                constraints: Vec::new(),
                instruments: vec!["smoke::cargo test --locked".to_string()],
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap();

        let state = crate::state::load_state(run_dir.join("state.json")).unwrap();
        assert_eq!(
            state.delivery_contract.extra["future_contract_key"]["kept"],
            true
        );
        assert_eq!(
            state.delivery_contract.instruments,
            vec!["smoke::cargo test --locked"]
        );
    }

    #[test]
    fn contract_set_rebuilds_missing_run_state_from_the_template() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto/r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let state = LtoState {
            run_id: "r1".into(),
            goal: "ship".into(),
            why: "user value".into(),
            done_when: "tests pass".into(),
            host_runtime: "codex".into(),
            ..LtoState::default()
        };
        crate::state::save_state(run_dir.join("state.json"), &state).unwrap();

        cmd_contract_set(
            repo,
            ContractSetOptions {
                run_id: Some("r1".into()),
                goal: None,
                done_when: None,
                host: None,
                targets: vec!["target".into()],
                constraints: Vec::new(),
                instruments: vec!["smoke::true".into()],
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap();

        let run_state = std::fs::read_to_string(run_dir.join("run-state.md")).unwrap();
        assert!(run_state.contains("- run_id: r1"));
        assert!(run_state.contains("- delivery_targets: target"));
        assert!(run_state.contains("- delivery_instruments: smoke::true"));
    }

    #[test]
    fn contract_set_noop_does_not_write_or_emit_an_event() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto/r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let state = LtoState {
            run_id: "r1".into(),
            goal: "ship".into(),
            why: "user value".into(),
            done_when: "tests pass".into(),
            host_runtime: "codex".into(),
            ..LtoState::default()
        };
        let state_path = run_dir.join("state.json");
        let run_state_path = run_dir.join("run-state.md");
        crate::state::save_state(&state_path, &state).unwrap();
        std::fs::write(&run_state_path, "unchanged\n").unwrap();
        let state_before = std::fs::read(&state_path).unwrap();
        let run_state_before = std::fs::read(&run_state_path).unwrap();

        cmd_contract_set(
            repo,
            ContractSetOptions {
                run_id: Some("r1".into()),
                goal: None,
                done_when: None,
                host: None,
                targets: Vec::new(),
                constraints: Vec::new(),
                instruments: Vec::new(),
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(std::fs::read(state_path).unwrap(), state_before);
        assert_eq!(std::fs::read(run_state_path).unwrap(), run_state_before);
        assert!(!run_dir.join("events.jsonl").exists());
    }

    #[test]
    fn contract_set_does_not_create_a_missing_run_for_its_lock() {
        let tmp = tempfile::tempdir().unwrap();

        let error = cmd_contract_set(
            tmp.path(),
            ContractSetOptions {
                run_id: Some("missing".into()),
                goal: None,
                done_when: None,
                host: None,
                targets: vec!["target".into()],
                constraints: Vec::new(),
                instruments: vec!["true".into()],
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("no state.json for missing"));
        assert!(!tmp.path().join(".lto").exists());
    }

    #[test]
    fn contract_set_does_not_emit_success_when_run_state_sync_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto/r1");
        std::fs::create_dir_all(run_dir.join("run-state.md")).unwrap();
        let state = LtoState {
            run_id: "r1".into(),
            goal: "ship".into(),
            why: "user value".into(),
            done_when: "tests pass".into(),
            host_runtime: "codex".into(),
            ..LtoState::default()
        };
        crate::state::save_state(run_dir.join("state.json"), &state).unwrap();

        cmd_contract_set(
            repo,
            ContractSetOptions {
                run_id: Some("r1".into()),
                goal: None,
                done_when: None,
                host: None,
                targets: vec!["target".into()],
                constraints: Vec::new(),
                instruments: vec!["smoke::true".into()],
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap_err();

        assert!(!run_dir.join("events.jsonl").exists());
        let persisted = crate::state::load_state(run_dir.join("state.json")).unwrap();
        assert!(persisted.delivery_contract.targets.is_empty());
    }

    #[test]
    fn contract_set_rolls_back_state_and_run_state_when_event_emit_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let run_dir = repo.join(".lto/r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let state = LtoState {
            run_id: "r1".into(),
            goal: "ship".into(),
            why: "user value".into(),
            done_when: "tests pass".into(),
            host_runtime: "codex".into(),
            ..LtoState::default()
        };
        let state_path = run_dir.join("state.json");
        let run_state_path = run_dir.join("run-state.md");
        crate::state::save_state(&state_path, &state).unwrap();
        std::fs::write(
            &run_state_path,
            "# Run\n\n- run_id: r1\n\n## Delivery Contract\n\n## Host Preconditions\n",
        )
        .unwrap();
        let state_before = std::fs::read(&state_path).unwrap();
        let run_state_before = std::fs::read(&run_state_path).unwrap();
        std::fs::create_dir(run_dir.join("events.jsonl")).unwrap();

        let error = cmd_contract_set(
            repo,
            ContractSetOptions {
                run_id: Some("r1".into()),
                goal: None,
                done_when: None,
                host: None,
                targets: vec!["target".into()],
                constraints: Vec::new(),
                instruments: vec!["smoke::true".into()],
                replacement_instruments: Vec::new(),
                entropy_checks: Vec::new(),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("rolled back"), "{error:#}");
        assert_eq!(std::fs::read(state_path).unwrap(), state_before);
        assert_eq!(std::fs::read(run_state_path).unwrap(), run_state_before);
    }
}
