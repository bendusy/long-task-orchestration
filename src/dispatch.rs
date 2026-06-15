use crate::agent_job::{AgentResult, JobStatus, TaskSize};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PRIOR_WEIGHT_K: f64 = 5.0;
const HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityCell {
    pub runner: String,
    pub task_type: String,
    pub size: TaskSize,
    pub n: u32,
    pub posterior: f64,
    pub prior: f64,
    pub last_seen: Option<DateTime<Utc>>,
}

impl CapabilityCell {
    pub fn score(&self, now: DateTime<Utc>) -> f64 {
        let decayed_n = self.decayed_n(now);
        (decayed_n / (decayed_n + PRIOR_WEIGHT_K)) * self.posterior
            + (PRIOR_WEIGHT_K / (decayed_n + PRIOR_WEIGHT_K)) * self.prior
    }

    fn decayed_n(&self, now: DateTime<Utc>) -> f64 {
        let Some(last_seen) = self.last_seen else {
            return self.n as f64;
        };
        let age_days = (now - last_seen).num_seconds().max(0) as f64 / 86_400.0;
        self.n as f64 * 0.5_f64.powf(age_days / HALF_LIFE_DAYS)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDescriptor {
    pub task_type: String,
    pub size: TaskSize,
    pub prompt_tokens: u64,
    pub expected_output_tokens: u64,
}

impl TaskDescriptor {
    pub fn from_tokens(
        task_type: impl Into<String>,
        prompt_tokens: u64,
        expected_output_tokens: u64,
    ) -> Self {
        let total = prompt_tokens.saturating_add(expected_output_tokens);
        let size = match total {
            0..=8_000 => TaskSize::Small,
            8_001..=40_000 => TaskSize::Medium,
            _ => TaskSize::Large,
        };
        Self {
            task_type: task_type.into(),
            size,
            prompt_tokens,
            expected_output_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub runner: String,
    pub rank: usize,
    pub score: f64,
    pub p_complete: f64,
    pub expected_seconds_p50: Option<f64>,
    pub hard_constraints: Vec<String>,
    pub facts: BTreeMap<String, serde_json::Value>,
}

pub fn build_cells_from_results(results: &[AgentResult]) -> Vec<CapabilityCell> {
    let mut grouped: BTreeMap<(String, String, TaskSize), (u32, u32)> = BTreeMap::new();
    for result in results {
        let task_type = result
            .task_type
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let key = (result.runner.clone(), task_type, result.size);
        let entry = grouped.entry(key).or_insert((0, 0));
        entry.0 += 1;
        if result.status == JobStatus::Ok {
            entry.1 += 1;
        }
    }
    grouped
        .into_iter()
        .map(|((runner, task_type, size), (n, ok))| CapabilityCell {
            runner,
            task_type,
            size,
            n,
            posterior: ok as f64 / n.max(1) as f64,
            prior: 0.55,
            last_seen: None,
        })
        .collect()
}

pub fn dispatch_candidates(
    task: &TaskDescriptor,
    cells: &[CapabilityCell],
    priors: &BTreeMap<String, f64>,
    now: DateTime<Utc>,
) -> Vec<RankedCandidate> {
    let mut candidates = Vec::new();
    for (runner, prior) in priors {
        let matching = cells.iter().find(|cell| {
            cell.runner == *runner && cell.task_type == task.task_type && cell.size == task.size
        });
        let (score, n, posterior) = if let Some(cell) = matching {
            (cell.score(now), cell.n, cell.posterior)
        } else {
            (*prior, 0, *prior)
        };
        let mut facts = BTreeMap::new();
        facts.insert("sample_count".to_string(), serde_json::json!(n));
        facts.insert("posterior".to_string(), serde_json::json!(posterior));
        facts.insert("prior".to_string(), serde_json::json!(prior));
        candidates.push(RankedCandidate {
            runner: runner.clone(),
            rank: 0,
            score,
            p_complete: score,
            expected_seconds_p50: None,
            hard_constraints: Vec::new(),
            facts,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.runner.cmp(&b.runner))
    });
    for (idx, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = idx + 1;
    }
    candidates
}

pub fn should_escalate_same_cell(
    failures: &[AgentResult],
    runner: &str,
    task_type: &str,
    size: TaskSize,
) -> bool {
    failures
        .iter()
        .rev()
        .filter(|r| {
            r.runner == runner
                && r.task_type.as_deref().unwrap_or("unknown") == task_type
                && r.size == size
                && r.status != JobStatus::Ok
        })
        .take(2)
        .count()
        >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_size_does_not_pollute_known_size_lookup() {
        let results = vec![AgentResult {
            job_id: "j1".to_string(),
            runner: "pi".to_string(),
            status: JobStatus::Ok,
            task_type: Some("audit".to_string()),
            size: TaskSize::Unknown,
            model: None,
            exit_code: None,
            findings: vec![],
            reply_text: String::new(),
            cost: BTreeMap::new(),
            permissions: BTreeMap::new(),
            artifacts: vec![],
            attempts: 1,
            error: String::new(),
            merge_review: None,
        }];
        let cells = build_cells_from_results(&results);
        let task = TaskDescriptor {
            task_type: "audit".to_string(),
            size: TaskSize::Small,
            prompt_tokens: 1,
            expected_output_tokens: 1,
        };
        let priors = BTreeMap::from([("pi".to_string(), 0.4)]);
        let ranked = dispatch_candidates(&task, &cells, &priors, Utc::now());
        assert_eq!(ranked[0].facts["sample_count"], serde_json::json!(0));
    }

    #[test]
    fn consecutive_failures_in_same_cell_escalate() {
        let failures = (0..2)
            .map(|idx| AgentResult {
                job_id: format!("j{idx}"),
                runner: "pi".to_string(),
                status: JobStatus::Timeout,
                task_type: Some("write".to_string()),
                size: TaskSize::Large,
                model: None,
                exit_code: Some(124),
                findings: vec![],
                reply_text: String::new(),
                cost: BTreeMap::new(),
                permissions: BTreeMap::new(),
                artifacts: vec![],
                attempts: 1,
                error: String::new(),
                merge_review: None,
            })
            .collect::<Vec<_>>();
        assert!(should_escalate_same_cell(
            &failures,
            "pi",
            "write",
            TaskSize::Large
        ));
        assert!(!should_escalate_same_cell(
            &failures,
            "pi",
            "write",
            TaskSize::Small
        ));
    }

    #[test]
    fn token_size_estimate_saturates_on_extreme_inputs() {
        let task = TaskDescriptor::from_tokens("write", u64::MAX, 1);
        assert_eq!(task.size, TaskSize::Large);
    }
}
