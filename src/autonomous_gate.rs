use crate::run_observability::{ObservabilityReport, ObservabilityStatus};
use crate::telemetry::{CompletionSample, CrossRunEvidence};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReliabilityStatus {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReliabilityReport {
    pub status: ReliabilityStatus,
    pub reason: String,
    pub warnings: Vec<String>,
}

impl ReliabilityReport {
    pub fn pass(reason: impl Into<String>, warnings: Vec<String>) -> Self {
        Self {
            status: ReliabilityStatus::Pass,
            reason: reason.into(),
            warnings,
        }
    }

    pub fn fail(reason: impl Into<String>) -> Self {
        Self {
            status: ReliabilityStatus::Fail,
            reason: reason.into(),
            warnings: Vec::new(),
        }
    }

    pub fn passes(&self) -> bool {
        self.status == ReliabilityStatus::Pass
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateReport {
    pub operational_reliability: ReliabilityReport,
    pub current_run_observability: ObservabilityReport,
}

impl GateReport {
    pub fn passes(&self) -> bool {
        self.operational_reliability.passes()
            && self.current_run_observability.status == ObservabilityStatus::ObservableVerified
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecentReliabilityAssessment {
    pub failure: Option<String>,
    pub warnings: Vec<String>,
}

pub fn assess_recent_reliability(
    evidence: &CrossRunEvidence,
    window: usize,
    minimum_rate_samples: usize,
    failure_rate_limit: f64,
    cold_failure_streak: usize,
    warning_streak: usize,
) -> RecentReliabilityAssessment {
    let mut grouped =
        BTreeMap::<(String, String, String), BTreeMap<String, CompletionSample>>::new();
    for entry in &evidence.entries {
        let slot = grouped
            .entry((
                entry.runner.clone(),
                entry.model.clone(),
                entry.task_type.clone(),
            ))
            .or_default();
        for sample in &entry.recent_completions {
            slot.insert(sample.completion_id.clone(), sample.clone());
        }
    }

    let mut warnings = Vec::new();
    for ((runner, model, task_type), samples) in grouped {
        let mut samples = samples
            .into_values()
            .filter(|sample| is_completed_outcome(&sample.status))
            .collect::<Vec<_>>();
        samples.sort_by(|left, right| {
            (&left.at, left.event_id, &left.run_id, &left.completion_id).cmp(&(
                &right.at,
                right.event_id,
                &right.run_id,
                &right.completion_id,
            ))
        });
        let recent = &samples[samples.len().saturating_sub(window)..];
        let failed = recent
            .iter()
            .filter(|sample| is_failure(&sample.status))
            .count();
        let trailing_failures = recent
            .iter()
            .rev()
            .take_while(|sample| is_failure(&sample.status))
            .count();
        let slot_name = format!("{runner}/{model}/{task_type}");
        if trailing_failures >= warning_streak {
            warnings.push(format!(
                "{slot_name}: {trailing_failures} consecutive recent failures; host intervention advised"
            ));
        }
        if recent.len() >= minimum_rate_samples {
            let failure_rate = failed as f64 / recent.len() as f64;
            if failure_rate >= failure_rate_limit {
                return RecentReliabilityAssessment {
                    failure: Some(format!(
                        "cross-run reliability risk for {slot_name}: failure_rate={:.1}% over {} recent completions",
                        failure_rate * 100.0,
                        recent.len()
                    )),
                    warnings,
                };
            }
        } else if trailing_failures >= cold_failure_streak {
            return RecentReliabilityAssessment {
                failure: Some(format!(
                    "cross-run reliability risk for {slot_name}: {trailing_failures} consecutive failures in {} cold-start completions",
                    recent.len()
                )),
                warnings,
            };
        }
    }
    RecentReliabilityAssessment {
        failure: None,
        warnings,
    }
}

fn is_completed_outcome(status: &str) -> bool {
    matches!(status, "ok" | "failed" | "timeout" | "rate_limited")
}

fn is_failure(status: &str) -> bool {
    matches!(status, "failed" | "timeout" | "rate_limited")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::CrossRunEvidenceEntry;

    fn evidence(statuses: &[&str]) -> CrossRunEvidence {
        CrossRunEvidence {
            run_count: statuses.len(),
            entries: vec![CrossRunEvidenceEntry {
                runner: "codex".into(),
                model: "gpt-5".into(),
                task_type: "implementation".into(),
                recent_completions: statuses
                    .iter()
                    .enumerate()
                    .map(|(index, status)| CompletionSample {
                        completion_id: format!("r{index}:job"),
                        run_id: format!("r{index}"),
                        at: format!("2026-07-{index:02}T00:00:00Z"),
                        event_id: index as u64,
                        status: status.to_string(),
                    })
                    .collect(),
                ..CrossRunEvidenceEntry::default()
            }],
        }
    }

    fn assess(statuses: &[&str]) -> RecentReliabilityAssessment {
        assess_recent_reliability(&evidence(statuses), 20, 5, 0.5, 3, 2)
    }

    #[test]
    fn one_old_timeout_in_twenty_does_not_block() {
        let mut statuses = vec!["timeout"];
        statuses.extend(std::iter::repeat_n("ok", 19));
        let report = assess(&statuses);
        assert_eq!(report.failure, None);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn three_consecutive_cold_start_failures_block() {
        let report = assess(&["failed", "timeout", "rate_limited"]);
        assert!(report.failure.unwrap().contains("3 consecutive failures"));
    }

    #[test]
    fn half_failure_rate_is_the_inclusive_boundary() {
        let report = assess(&[
            "ok", "failed", "ok", "failed", "ok", "failed", "ok", "failed", "ok", "failed",
        ]);
        assert!(report.failure.unwrap().contains("failure_rate=50.0%"));
    }

    #[test]
    fn two_consecutive_failures_warn_without_blocking() {
        let report = assess(&["ok", "failed", "failed"]);
        assert_eq!(report.failure, None);
        assert_eq!(report.warnings.len(), 1);
    }
}
