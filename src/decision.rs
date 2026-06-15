use crate::agent_job::{AgentResult, JobStatus};
use crate::audit::{Finding, family, parse_findings_text, parse_findings_values};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectionDecision {
    PickTask,
    PickPattern,
    NeedsHuman,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectionVote {
    pub decision: DirectionDecision,
    pub value: String,
    pub reasoning: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub supermajority_met: bool,
    pub majority_pick: Option<String>,
    pub majority_count: usize,
    pub minority: Vec<DirectionVote>,
    pub needs_human_votes: usize,
    pub needs_info: bool,
    pub invalid_votes: Vec<DirectionVote>,
}

pub fn tally_votes(votes: &[DirectionVote], valid_task_ids: Option<&BTreeSet<String>>) -> Tally {
    let mut invalid_votes = Vec::new();
    let mut valid = Vec::new();
    for vote in votes {
        if is_valid_vote(vote, valid_task_ids) {
            valid.push(vote.clone());
        } else {
            invalid_votes.push(vote.clone());
        }
    }
    let needs_human_votes = valid
        .iter()
        .filter(|vote| vote.decision == DirectionDecision::NeedsHuman)
        .count();
    let mut counts = BTreeMap::<String, usize>::new();
    for vote in &valid {
        if vote.decision != DirectionDecision::NeedsHuman {
            *counts.entry(vote_key(vote)).or_default() += 1;
        }
    }
    let (majority_pick, majority_count) = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map_or((None, 0), |(key, count)| (Some(key), count));
    let supermajority_met = majority_count >= 2;
    let minority = valid
        .into_iter()
        .filter(|vote| Some(vote_key(vote)) != majority_pick)
        .collect::<Vec<_>>();
    Tally {
        supermajority_met,
        majority_pick,
        majority_count,
        minority,
        needs_human_votes,
        needs_info: !supermajority_met || needs_human_votes >= 1,
        invalid_votes,
    }
}

fn is_valid_vote(vote: &DirectionVote, valid_task_ids: Option<&BTreeSet<String>>) -> bool {
    match vote.decision {
        DirectionDecision::PickTask => valid_task_ids
            .map(|ids| ids.contains(&vote.value))
            .unwrap_or(true),
        DirectionDecision::PickPattern => matches!(
            vote.value.as_str(),
            "linear" | "fan-out" | "adversarial" | "tournament" | "loop"
        ),
        DirectionDecision::NeedsHuman => true,
    }
}

fn vote_key(vote: &DirectionVote) -> String {
    match vote.decision {
        DirectionDecision::PickTask => format!("pick_task:{}", vote.value),
        DirectionDecision::PickPattern => format!("pick_pattern:{}", vote.value),
        DirectionDecision::NeedsHuman => "needs_human".to_string(),
    }
}

pub fn merge_findings(results: &[AgentResult]) -> Vec<Finding> {
    let mut out = Vec::new();
    for result in results {
        if result.status != JobStatus::Ok {
            continue;
        }
        if !result.findings.is_empty()
            && let Some(mut findings) = parse_findings_values(&result.findings)
        {
            for finding in &mut findings {
                if finding.source.is_none() {
                    finding.source = Some(result.runner.clone());
                }
            }
            out.extend(findings);
            continue;
        }
        if let Some(mut findings) = parse_findings_text(&result.reply_text) {
            for finding in &mut findings {
                if finding.source.is_none() {
                    finding.source = Some(result.runner.clone());
                }
            }
            out.extend(findings);
        } else if !result.reply_text.trim().is_empty() {
            out.push(Finding {
                severity: crate::audit::Severity::Medium,
                claim: result.reply_text.clone(),
                evidence_to_check: None,
                file: None,
                source: Some(result.runner.clone()),
            });
        }
    }
    out
}

pub fn has_minimum_valid_reviewers(results: &[AgentResult]) -> bool {
    let mut families = Vec::new();
    for result in results
        .iter()
        .filter(|r| r.status == JobStatus::Ok && !r.reply_text.trim().is_empty())
    {
        let family = family(&result.runner);
        if !families.contains(&family) {
            families.push(family);
        }
    }
    families.len() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_absolute_two_not_ratio() {
        let votes = vec![
            DirectionVote {
                decision: DirectionDecision::PickTask,
                value: "T1".to_string(),
                reasoning: String::new(),
                source: "codex".to_string(),
            },
            DirectionVote {
                decision: DirectionDecision::PickTask,
                value: "T1".to_string(),
                reasoning: String::new(),
                source: "pi".to_string(),
            },
        ];
        let tally = tally_votes(&votes, None);
        assert!(tally.supermajority_met);
        assert_eq!(tally.majority_count, 2);
    }

    #[test]
    fn one_needs_human_vote_vetoes_even_with_two_agreeing() {
        let votes = vec![
            DirectionVote {
                decision: DirectionDecision::PickTask,
                value: "T1".into(),
                reasoning: "".into(),
                source: "codex".into(),
            },
            DirectionVote {
                decision: DirectionDecision::PickTask,
                value: "T1".into(),
                reasoning: "".into(),
                source: "pi".into(),
            },
            DirectionVote {
                decision: DirectionDecision::NeedsHuman,
                value: "ambiguous".into(),
                reasoning: "".into(),
                source: "agy".into(),
            },
        ];
        let tally = tally_votes(&votes, None);
        assert!(tally.supermajority_met);
        assert!(tally.needs_info);
        assert_eq!(tally.needs_human_votes, 1);
    }

    #[test]
    fn invalid_task_votes_are_removed() {
        let votes = vec![DirectionVote {
            decision: DirectionDecision::PickTask,
            value: "DROP".to_string(),
            reasoning: String::new(),
            source: "codex".to_string(),
        }];
        let valid = BTreeSet::from(["T1".to_string()]);
        let tally = tally_votes(&votes, Some(&valid));
        assert_eq!(tally.invalid_votes.len(), 1);
        assert!(tally.needs_info);
    }

    #[test]
    fn valid_reviewer_gate_requires_distinct_families() {
        let result = |runner: &str| AgentResult {
            job_id: format!("j-{runner}"),
            runner: runner.to_string(),
            status: JobStatus::Ok,
            task_type: None,
            size: crate::agent_job::TaskSize::Unknown,
            model: None,
            exit_code: Some(0),
            findings: vec![],
            reply_text: "review".to_string(),
            cost: BTreeMap::new(),
            permissions: BTreeMap::new(),
            artifacts: vec![],
            attempts: 1,
            error: String::new(),
        };
        assert!(!has_minimum_valid_reviewers(&[
            result("codex"),
            result("openai-gpt-5")
        ]));
        assert!(has_minimum_valid_reviewers(&[
            result("codex"),
            result("pi")
        ]));
    }
}
