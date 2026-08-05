use crate::state::{DeliveryContract, LtoState};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const HASH_PREFIX: &str = "sha256:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentSpec {
    pub reference: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservabilityStatus {
    ObservableVerified,
    SignalDeclared,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservabilityReport {
    pub status: ObservabilityStatus,
    pub reason: String,
    pub missing: Vec<String>,
}

pub fn parse_instruments(contract: &DeliveryContract) -> Vec<InstrumentSpec> {
    contract
        .instruments
        .iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            let (label, command) = raw
                .split_once("::")
                .map(|(label, command)| (Some(label.trim()), command.trim()))
                .unwrap_or((None, raw));
            if command.is_empty() {
                return None;
            }
            let reference = label
                .filter(|label| !label.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| command_reference(command));
            Some(InstrumentSpec {
                reference,
                command: command.to_string(),
            })
        })
        .collect()
}

pub fn validate_instrument_ref(
    contract: &DeliveryContract,
    instrument_ref: &str,
) -> anyhow::Result<String> {
    let instrument_ref = instrument_ref.trim();
    if instrument_ref.is_empty() {
        anyhow::bail!("--instrument-ref must not be empty");
    }
    let specs = parse_instruments(contract);
    if specs
        .iter()
        .any(|instrument| instrument.reference == instrument_ref)
    {
        return Ok(instrument_ref.to_string());
    }
    anyhow::bail!(
        "--instrument-ref {instrument_ref:?} does not match a current delivery contract instrument"
    )
}

pub fn resolve_instrument_ref(
    contract: &DeliveryContract,
    explicit_ref: Option<&str>,
    inherited_ref: Option<&str>,
    command: &str,
) -> anyhow::Result<Option<String>> {
    if let Some(instrument_ref) = explicit_ref.or(inherited_ref) {
        return validate_instrument_ref(contract, instrument_ref).map(Some);
    }
    let normalized = normalize_command(command);
    Ok(parse_instruments(contract)
        .into_iter()
        .find(|instrument| normalize_command(&instrument.command) == normalized)
        .map(|instrument| instrument.reference))
}

pub fn assess(state: &LtoState) -> ObservabilityReport {
    let mut missing = Vec::new();
    if state.goal.trim().is_empty() {
        missing.push("goal".to_string());
    }
    if state.done_when.trim().is_empty() {
        missing.push("done_when".to_string());
    }
    let instruments = parse_instruments(&state.delivery_contract);
    if instruments.is_empty() {
        missing.push("delivery_contract.instruments".to_string());
    }
    if !missing.is_empty() {
        return ObservabilityReport {
            status: ObservabilityStatus::Missing,
            reason: format!("current run is missing {}", missing.join(", ")),
            missing,
        };
    }

    let Some(evidence) = latest_command_evidence(&state.tasks) else {
        return declared("latest runner evidence with command and numeric rc");
    };
    let referenced = evidence
        .get("instrument_ref")
        .and_then(Value::as_str)
        .is_some_and(|reference| {
            instruments
                .iter()
                .any(|instrument| instrument.reference == reference.trim())
        });
    let legacy_command_match =
        evidence
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                let normalized = normalize_command(command);
                instruments
                    .iter()
                    .any(|instrument| normalize_command(&instrument.command) == normalized)
            });
    if referenced || legacy_command_match {
        let association = if referenced {
            "structured instrument_ref"
        } else {
            "legacy normalized command fallback"
        };
        return ObservabilityReport {
            status: ObservabilityStatus::ObservableVerified,
            reason: format!("latest runner evidence is parseable and linked by {association}"),
            missing: Vec::new(),
        };
    }
    declared("instrument_ref or normalized legacy command association")
}

fn declared(missing_evidence: &str) -> ObservabilityReport {
    ObservabilityReport {
        status: ObservabilityStatus::SignalDeclared,
        reason: format!(
            "instrument signal is declared but not verified; missing {missing_evidence}"
        ),
        missing: vec![missing_evidence.to_string()],
    }
}

fn latest_command_evidence(tasks: &Value) -> Option<&Value> {
    tasks
        .as_array()?
        .iter()
        .flat_map(|task| {
            task.get("evidence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|evidence| {
            evidence.get("command").and_then(Value::as_str).is_some()
                && evidence.get("rc").and_then(Value::as_i64).is_some()
        })
        .max_by_key(|evidence| {
            evidence
                .get("ended_at")
                .or_else(|| evidence.get("recorded_at"))
                .and_then(Value::as_str)
                .unwrap_or("")
        })
}

pub fn normalize_command(command: &str) -> String {
    command
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '\'' | '"'))
        .collect()
}

fn command_reference(command: &str) -> String {
    format!(
        "{HASH_PREFIX}{:x}",
        Sha256::digest(normalize_command(command).as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state_with(instruments: &[&str], evidence: Value) -> LtoState {
        LtoState {
            goal: "ship C4".into(),
            done_when: "gate is observable".into(),
            delivery_contract: DeliveryContract::new(
                vec!["gate".into()],
                vec!["fail closed".into()],
                instruments.iter().map(|value| value.to_string()).collect(),
                vec!["audit".into()],
            ),
            tasks: json!([{"id": "T1", "evidence": evidence}]),
            ..LtoState::default()
        }
    }

    #[test]
    fn reports_missing_base_fields_and_instrument() {
        let report = assess(&LtoState::default());
        assert_eq!(report.status, ObservabilityStatus::Missing);
        assert_eq!(
            report.missing,
            ["goal", "done_when", "delivery_contract.instruments"]
        );
    }

    #[test]
    fn reports_missing_instrument_for_ready_legacy_run() {
        let state = state_with(&[], json!([]));
        let report = assess(&state);
        assert_eq!(report.status, ObservabilityStatus::Missing);
        assert_eq!(report.missing, ["delivery_contract.instruments"]);
    }

    #[test]
    fn declared_signal_without_runner_evidence_is_not_verified() {
        let report = assess(&state_with(&["tests::cargo test"], json!([])));
        assert_eq!(report.status, ObservabilityStatus::SignalDeclared);
    }

    #[test]
    fn unparseable_or_unmatched_evidence_stays_declared() {
        let state = state_with(
            &["tests::cargo test"],
            json!([
                {"command": "cargo test", "rc": "zero", "instrument_ref": "tests"},
                {"command": "cargo check", "rc": 0, "instrument_ref": "other"}
            ]),
        );
        assert_eq!(assess(&state).status, ObservabilityStatus::SignalDeclared);
    }

    #[test]
    fn structured_label_reference_verifies_latest_evidence() {
        let state = state_with(
            &["tests::cargo test"],
            json!([{"command": "cargo test", "rc": 0, "instrument_ref": "tests"}]),
        );
        assert_eq!(
            assess(&state).status,
            ObservabilityStatus::ObservableVerified
        );
    }

    #[test]
    fn one_matching_instrument_verifies_the_whole_contract() {
        // Documents current behaviour, which is weaker than the name suggests:
        // assess looks only at the newest evidence entry, so a contract of eight
        // instruments reports ObservableVerified once any single one of them has
        // run. autonomous_gate.rs:51 gates autonomous mode on this status.
        // Whether that is the intended contract is open -- see
        // references/specs/2026-08-05-replan-triggers.md.
        let state = state_with(
            &[
                "baseline::cargo test --locked autonomous_gate",
                "phase1::cargo test --locked run_observability",
                "full-rust::cargo test --locked --all-targets",
            ],
            json!([{"command": "cargo test --locked autonomous_gate", "rc": 0, "instrument_ref": "baseline"}]),
        );
        assert_eq!(
            assess(&state).status,
            ObservabilityStatus::ObservableVerified,
            "one instrument out of three currently verifies the contract"
        );
    }

    #[test]
    fn normalized_legacy_command_is_a_read_compatibility_fallback() {
        let state = state_with(
            &["cargo test --locked"],
            json!([{"command": "cargo  test \"--locked\"", "rc": 1}]),
        );
        assert_eq!(
            assess(&state).status,
            ObservabilityStatus::ObservableVerified
        );
    }

    #[test]
    fn command_hash_ignores_whitespace_and_quotes() {
        let first = parse_instruments(&DeliveryContract::new(
            vec!["gate".into()],
            vec![],
            vec!["cargo test --locked".into()],
            vec![],
        ));
        let second = parse_instruments(&DeliveryContract::new(
            vec!["gate".into()],
            vec![],
            vec!["cargo  test \"--locked\"".into()],
            vec![],
        ));
        assert_eq!(first[0].reference, second[0].reference);
        assert!(first[0].reference.starts_with(HASH_PREFIX));
    }
}
