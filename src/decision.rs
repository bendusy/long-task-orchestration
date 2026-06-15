use crate::agent_job::{AgentResult, JobStatus};
use crate::audit::{Finding, family, parse_findings_text, parse_findings_values};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const EST_TOKENS_PER_ROUND: u64 = 18_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Direction,
    Review,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Converged,
    NeedsInfo,
    NeedsHuman,
}

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
    pub total_voters: usize,
    pub votes: Vec<DirectionVote>,
    pub minority: Vec<DirectionVote>,
    pub needs_human_votes: usize,
    pub needs_info: bool,
    pub invalid_votes: Vec<DirectionVote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRunRequest {
    pub kind: DecisionKind,
    pub auditors: Vec<String>,
    pub budget_remaining: u64,
    pub escalate_key: String,
    #[serde(default)]
    pub spawned_escalate_keys: BTreeSet<String>,
    #[serde(default)]
    pub valid_task_ids: Option<BTreeSet<String>>,
    #[serde(default)]
    pub direction_votes: Vec<DirectionVote>,
    #[serde(default)]
    pub review_results: Vec<AgentResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOutcome {
    pub status: DecisionStatus,
    pub kind: DecisionKind,
    pub result: Option<DecisionResultPayload>,
    pub dissent: DecisionDissent,
    pub brief: String,
    pub dispatched_to: Vec<String>,
    pub budget_consumed_est: u64,
    #[serde(default)]
    pub record_escalate_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "track")]
pub enum DecisionResultPayload {
    Direction(DirectionPayload),
    Review(ReviewPayload),
    Both(BothPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectionPayload {
    pub converged: bool,
    pub pick: Option<String>,
    pub count: usize,
    pub total: usize,
    pub votes: Vec<DirectionVote>,
    pub minority: Vec<DirectionVote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPayload {
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BothPayload {
    pub direction: DirectionPayload,
    pub review: ReviewPayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionDissent {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub minority_votes: Vec<DirectionVote>,
    #[serde(default)]
    pub invalid_votes: Vec<DirectionVote>,
    #[serde(default)]
    pub needs_human_votes: usize,
    #[serde(default)]
    pub findings_for_host_judgment: Vec<Finding>,
}

pub fn run_decision(request: DecisionRunRequest) -> DecisionOutcome {
    let est_cost = EST_TOKENS_PER_ROUND.saturating_mul(rounds_needed(request.kind));
    if request.budget_remaining < est_cost {
        return edge_outcome(
            request.kind,
            "budget_exhausted",
            build_budget_brief(request.kind, request.budget_remaining, est_cost),
        );
    }
    if request
        .spawned_escalate_keys
        .contains(&request.escalate_key)
    {
        return edge_outcome(
            request.kind,
            "same escalate point already spawned once - refusing re-spawn (G5 limit)",
            build_needs_human_brief(
                request.kind,
                "same escalate point already spawned once - refusing re-spawn (G5 limit)",
            ),
        );
    }

    let mut direction = None;
    let mut review = None;
    let mut dissent = DecisionDissent::default();

    if matches!(request.kind, DecisionKind::Direction | DecisionKind::Both) {
        let tally = tally_votes(&request.direction_votes, request.valid_task_ids.as_ref());
        if !has_minimum_vote_families(&tally.votes) {
            return edge_outcome(
                request.kind,
                "direction track: fewer than two valid heterogeneous reviewers",
                build_needs_human_brief(
                    request.kind,
                    "direction track: fewer than two valid heterogeneous reviewers",
                ),
            )
            .with_record(request.escalate_key);
        }
        dissent.minority_votes = tally.minority.clone();
        dissent.invalid_votes = tally.invalid_votes.clone();
        dissent.needs_human_votes = tally.needs_human_votes;
        direction = Some(direction_payload(&tally));
    }

    if matches!(request.kind, DecisionKind::Review | DecisionKind::Both) {
        if !has_minimum_valid_reviewers(&request.review_results) {
            return edge_outcome(
                request.kind,
                "review track: fewer than two valid heterogeneous reviewers",
                build_needs_human_brief(
                    request.kind,
                    "review track: fewer than two valid heterogeneous reviewers",
                ),
            )
            .with_record(request.escalate_key);
        }
        let findings = merge_findings(&request.review_results);
        dissent.findings_for_host_judgment = findings.clone();
        review = Some(ReviewPayload { findings });
    }

    let status = compose_status(request.kind, direction.as_ref());
    let result = compose_payload(request.kind, direction, review);
    DecisionOutcome {
        status,
        kind: request.kind,
        result,
        dissent,
        brief: build_decision_brief(request.kind, status, &request.auditors, est_cost),
        dispatched_to: request.auditors,
        budget_consumed_est: est_cost,
        record_escalate_key: Some(request.escalate_key),
    }
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
    let total_voters = valid.len();
    let minority = valid
        .iter()
        .filter(|vote| Some(vote_key(vote)) != majority_pick)
        .cloned()
        .collect::<Vec<_>>();
    Tally {
        supermajority_met,
        majority_pick,
        majority_count,
        total_voters,
        votes: valid,
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

fn rounds_needed(kind: DecisionKind) -> u64 {
    match kind {
        DecisionKind::Both => 2,
        DecisionKind::Direction | DecisionKind::Review => 1,
    }
}

fn has_minimum_vote_families(votes: &[DirectionVote]) -> bool {
    let mut families = Vec::new();
    for vote in votes {
        let family = family(&vote.source);
        if !families.contains(&family) {
            families.push(family);
        }
    }
    families.len() >= 2
}

fn direction_payload(tally: &Tally) -> DirectionPayload {
    DirectionPayload {
        converged: tally.supermajority_met,
        pick: tally.majority_pick.clone(),
        count: tally.majority_count,
        total: tally.total_voters,
        votes: tally.votes.clone(),
        minority: tally.minority.clone(),
    }
}

fn compose_status(kind: DecisionKind, direction: Option<&DirectionPayload>) -> DecisionStatus {
    match kind {
        DecisionKind::Review => DecisionStatus::Converged,
        DecisionKind::Direction | DecisionKind::Both => {
            let Some(direction) = direction else {
                return DecisionStatus::NeedsInfo;
            };
            let has_human_veto = direction
                .minority
                .iter()
                .any(|vote| vote.decision == DirectionDecision::NeedsHuman);
            if !direction.converged || has_human_veto {
                DecisionStatus::NeedsInfo
            } else {
                DecisionStatus::Converged
            }
        }
    }
}

fn compose_payload(
    kind: DecisionKind,
    direction: Option<DirectionPayload>,
    review: Option<ReviewPayload>,
) -> Option<DecisionResultPayload> {
    match kind {
        DecisionKind::Direction => direction.map(DecisionResultPayload::Direction),
        DecisionKind::Review => review.map(DecisionResultPayload::Review),
        DecisionKind::Both => Some(DecisionResultPayload::Both(BothPayload {
            direction: direction?,
            review: review?,
        })),
    }
}

fn edge_outcome(kind: DecisionKind, reason: &str, brief: String) -> DecisionOutcome {
    DecisionOutcome {
        status: DecisionStatus::NeedsHuman,
        kind,
        result: None,
        dissent: DecisionDissent {
            reason: Some(reason.to_string()),
            ..DecisionDissent::default()
        },
        brief,
        dispatched_to: Vec::new(),
        budget_consumed_est: 0,
        record_escalate_key: None,
    }
}

impl DecisionOutcome {
    fn with_record(mut self, key: String) -> Self {
        self.record_escalate_key = Some(key);
        self
    }
}

fn build_budget_brief(kind: DecisionKind, remaining: u64, required: u64) -> String {
    format!(
        "decision {kind:?}: budget exhausted; remaining={remaining}, required={required}; host decision required"
    )
}

fn build_needs_human_brief(kind: DecisionKind, reason: &str) -> String {
    format!("decision {kind:?}: needs human; reason={reason}")
}

fn build_decision_brief(
    kind: DecisionKind,
    status: DecisionStatus,
    auditors: &[String],
    budget: u64,
) -> String {
    format!(
        "decision {kind:?}: status={status:?}; dispatched_to={}; budget_consumed_est={budget}",
        auditors.join(",")
    )
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
    fn run_decision_direction_returns_top_level_payload_and_budget() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Direction,
            auditors: vec!["codex".into(), "pi".into(), "agy".into()],
            budget_remaining: 100_000,
            escalate_key: "phase|blocked=|pending=T1".into(),
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T1".into()])),
            direction_votes: vec![
                vote("codex", DirectionDecision::PickTask, "T1"),
                vote("pi", DirectionDecision::PickTask, "T1"),
                vote("agy", DirectionDecision::PickPattern, "linear"),
            ],
            review_results: vec![],
        });
        assert_eq!(outcome.status, DecisionStatus::Converged);
        assert_eq!(outcome.budget_consumed_est, EST_TOKENS_PER_ROUND);
        assert_eq!(outcome.dispatched_to.len(), 3);
        assert_eq!(
            outcome.record_escalate_key.as_deref(),
            Some("phase|blocked=|pending=T1")
        );
        assert!(matches!(
            outcome.result,
            Some(DecisionResultPayload::Direction(DirectionPayload {
                pick: Some(_),
                count: 2,
                total: 3,
                ..
            }))
        ));
    }

    #[test]
    fn run_decision_budget_gate_and_g5_dedup_need_human_without_dispatch() {
        let low_budget = run_decision(DecisionRunRequest {
            kind: DecisionKind::Both,
            auditors: vec!["codex".into()],
            budget_remaining: EST_TOKENS_PER_ROUND,
            escalate_key: "k".into(),
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            direction_votes: vec![],
            review_results: vec![],
        });
        assert_eq!(low_budget.status, DecisionStatus::NeedsHuman);
        assert_eq!(low_budget.budget_consumed_est, 0);
        assert_eq!(
            low_budget.dissent.reason.as_deref(),
            Some("budget_exhausted")
        );

        let dedup = run_decision(DecisionRunRequest {
            kind: DecisionKind::Direction,
            auditors: vec!["codex".into(), "pi".into()],
            budget_remaining: 100_000,
            escalate_key: "same".into(),
            spawned_escalate_keys: BTreeSet::from(["same".into()]),
            valid_task_ids: None,
            direction_votes: vec![vote("codex", DirectionDecision::PickPattern, "linear")],
            review_results: vec![],
        });
        assert_eq!(dedup.status, DecisionStatus::NeedsHuman);
        assert!(dedup.dispatched_to.is_empty());
        assert!(dedup.record_escalate_key.is_none());
    }

    #[test]
    fn run_decision_both_keeps_direction_and_review_tracks_separate() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Both,
            auditors: vec!["codex".into(), "pi".into(), "agy".into()],
            budget_remaining: 100_000,
            escalate_key: "both".into(),
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            direction_votes: vec![
                vote("codex", DirectionDecision::PickPattern, "linear"),
                vote("pi", DirectionDecision::PickPattern, "linear"),
                vote("agy", DirectionDecision::NeedsHuman, "risk"),
            ],
            review_results: vec![
                result_with_text("codex", r#"[{"severity":"high","claim":"A"}]"#),
                result_with_text("pi", r#"[{"severity":"中危","claim":"B"}]"#),
            ],
        });
        assert_eq!(outcome.status, DecisionStatus::NeedsInfo);
        assert_eq!(outcome.budget_consumed_est, EST_TOKENS_PER_ROUND * 2);
        assert_eq!(outcome.dissent.needs_human_votes, 1);
        assert_eq!(outcome.dissent.findings_for_host_judgment.len(), 2);
        assert!(matches!(
            outcome.result,
            Some(DecisionResultPayload::Both(_))
        ));
    }

    #[test]
    fn run_decision_insufficient_valid_reviewers_needs_human() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Review,
            auditors: vec!["codex".into(), "openai-gpt".into()],
            budget_remaining: 100_000,
            escalate_key: "review".into(),
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            direction_votes: vec![],
            review_results: vec![
                result_with_text("codex", r#"[{"severity":"high","claim":"A"}]"#),
                result_with_text("openai-gpt", r#"[{"severity":"medium","claim":"B"}]"#),
            ],
        });
        assert_eq!(outcome.status, DecisionStatus::NeedsHuman);
        assert!(outcome.record_escalate_key.is_some());
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
            merge_review: None,
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

    fn vote(source: &str, decision: DirectionDecision, value: &str) -> DirectionVote {
        DirectionVote {
            decision,
            value: value.to_string(),
            reasoning: String::new(),
            source: source.to_string(),
        }
    }

    fn result_with_text(runner: &str, reply_text: &str) -> AgentResult {
        AgentResult {
            job_id: format!("j-{runner}"),
            runner: runner.to_string(),
            status: JobStatus::Ok,
            task_type: None,
            size: crate::agent_job::TaskSize::Unknown,
            model: None,
            exit_code: Some(0),
            findings: vec![],
            reply_text: reply_text.to_string(),
            cost: BTreeMap::new(),
            permissions: BTreeMap::new(),
            artifacts: vec![],
            attempts: 1,
            error: String::new(),
            merge_review: None,
        }
    }
}
