use crate::agent_job::{AgentResult, JobStatus};
use crate::audit::{
    Finding, family, parse_findings_text, parse_findings_values, parse_valid_findings_values,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatePool {
    pub candidates: Vec<DirectionVote>,
    pub invalid_candidates: Vec<DirectionVote>,
    pub sources: Vec<String>,
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
    #[serde(default = "default_diverge")]
    pub diverge: bool,
    #[serde(default)]
    pub spawned_escalate_keys: BTreeSet<String>,
    #[serde(default)]
    pub valid_task_ids: Option<BTreeSet<String>>,
    #[serde(default)]
    pub task_universe: BTreeSet<String>,
    #[serde(default)]
    pub candidate_results: Vec<AgentResult>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_pool: Option<CandidatePool>,
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
    pub invalid_candidates: Vec<DirectionVote>,
    #[serde(default)]
    pub needs_human_votes: usize,
    #[serde(default)]
    pub findings_for_host_judgment: Vec<Finding>,
}

pub fn run_decision(request: DecisionRunRequest) -> DecisionOutcome {
    let est_cost =
        EST_TOKENS_PER_ROUND.saturating_mul(rounds_needed(request.kind, request.diverge));
    if request.budget_remaining < est_cost {
        return edge_outcome(
            request.kind,
            "budget_exhausted",
            build_budget_brief(
                request.kind,
                &request.auditors,
                request.budget_remaining,
                est_cost,
            ),
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
                &request.auditors,
                "same escalate point already spawned once - refusing re-spawn (G5 limit)",
                request.budget_remaining,
                est_cost,
            ),
        );
    }

    let mut direction = None;
    let mut review = None;
    let mut dissent = DecisionDissent::default();

    if matches!(request.kind, DecisionKind::Direction | DecisionKind::Both) {
        let (tally, candidate_pool, fallback_reason) = run_direction_phases(&request);
        if !has_minimum_vote_families(&tally.votes) {
            return edge_outcome(
                request.kind,
                "direction track: fewer than two valid heterogeneous reviewers",
                build_needs_human_brief(
                    request.kind,
                    &request.auditors,
                    "direction track: fewer than two valid heterogeneous reviewers",
                    request.budget_remaining,
                    est_cost,
                ),
            )
            .with_record(request.escalate_key);
        }
        if let Some(reason) = fallback_reason {
            dissent.reason = Some(reason);
        }
        dissent.minority_votes = tally.minority.clone();
        dissent.invalid_votes = tally.invalid_votes.clone();
        if let Some(pool) = candidate_pool.as_ref() {
            dissent.invalid_candidates = pool.invalid_candidates.clone();
        }
        dissent.needs_human_votes = tally.needs_human_votes;
        direction = Some(direction_payload(&tally, candidate_pool));
    }

    if matches!(request.kind, DecisionKind::Review | DecisionKind::Both) {
        if !has_minimum_valid_reviewers(&request.review_results) {
            return edge_outcome(
                request.kind,
                "review track: fewer than two valid heterogeneous reviewers",
                build_needs_human_brief(
                    request.kind,
                    &request.auditors,
                    "review track: fewer than two valid heterogeneous reviewers",
                    request.budget_remaining,
                    est_cost,
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
    let brief = build_decision_brief(
        request.kind,
        status,
        result.as_ref(),
        &dissent,
        &request.auditors,
        est_cost,
        request.budget_remaining,
    );
    DecisionOutcome {
        status,
        kind: request.kind,
        result,
        dissent,
        brief,
        dispatched_to: request.auditors,
        budget_consumed_est: est_cost,
        record_escalate_key: Some(request.escalate_key),
    }
}

fn default_diverge() -> bool {
    true
}

fn run_direction_phases(
    request: &DecisionRunRequest,
) -> (Tally, Option<CandidatePool>, Option<String>) {
    if !request.diverge || request.candidate_results.is_empty() {
        return (
            converge_phase(&request.direction_votes, request.valid_task_ids.as_ref()),
            None,
            None,
        );
    }

    let candidate_pool = diverge_phase(request);
    if !has_minimum_vote_families(&candidate_pool.candidates) {
        return (
            converge_phase(&request.direction_votes, request.valid_task_ids.as_ref()),
            Some(candidate_pool),
            Some(
                "direction diverge: fewer than two heterogeneous candidate sources; fell back to single-stage convergence"
                    .to_string(),
            ),
        );
    }

    let candidate_task_ids = candidate_task_ids(&candidate_pool);
    let tally = converge_phase(&request.direction_votes, Some(&candidate_task_ids));
    (tally, Some(candidate_pool), None)
}

fn diverge_phase(request: &DecisionRunRequest) -> CandidatePool {
    let proposed = merge_candidates(&request.candidate_results);
    let task_base = candidate_task_base(request);
    let mut candidates = Vec::new();
    let mut invalid_candidates = Vec::new();

    for candidate in proposed {
        if validate_candidate(&candidate, task_base) {
            candidates.push(candidate);
        } else {
            invalid_candidates.push(candidate);
        }
    }

    CandidatePool {
        sources: candidate_sources(&candidates, &invalid_candidates),
        candidates,
        invalid_candidates,
    }
}

fn converge_phase(votes: &[DirectionVote], valid_task_ids: Option<&BTreeSet<String>>) -> Tally {
    tally_votes(votes, valid_task_ids)
}

pub fn merge_candidates(results: &[AgentResult]) -> Vec<DirectionVote> {
    let mut out = Vec::new();
    for result in results {
        if result.status != JobStatus::Ok {
            continue;
        }
        if !result.findings.is_empty()
            && let Some(mut candidates) = parse_direction_vote_values(&result.findings)
        {
            fill_missing_sources(&mut candidates, &result.runner);
            out.extend(candidates);
            continue;
        }
        if let Some(mut candidates) = parse_direction_votes_text(&result.reply_text) {
            fill_missing_sources(&mut candidates, &result.runner);
            out.extend(candidates);
        }
    }
    out
}

fn validate_candidate(candidate: &DirectionVote, task_base: Option<&BTreeSet<String>>) -> bool {
    match candidate.decision {
        DirectionDecision::PickTask => task_base
            .map(|ids| ids.contains(&candidate.value))
            .unwrap_or(false),
        DirectionDecision::PickPattern => is_legal_pattern(&candidate.value),
        DirectionDecision::NeedsHuman => true,
    }
}

fn candidate_task_base(request: &DecisionRunRequest) -> Option<&BTreeSet<String>> {
    if !request.task_universe.is_empty() {
        Some(&request.task_universe)
    } else {
        request.valid_task_ids.as_ref()
    }
}

fn candidate_task_ids(pool: &CandidatePool) -> BTreeSet<String> {
    pool.candidates
        .iter()
        .filter(|candidate| candidate.decision == DirectionDecision::PickTask)
        .map(|candidate| candidate.value.clone())
        .collect()
}

fn candidate_sources(
    candidates: &[DirectionVote],
    invalid_candidates: &[DirectionVote],
) -> Vec<String> {
    candidates
        .iter()
        .chain(invalid_candidates)
        .map(|candidate| candidate.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn fill_missing_sources(candidates: &mut [DirectionVote], runner: &str) {
    for candidate in candidates {
        if candidate.source.trim().is_empty() {
            candidate.source = runner.to_string();
        }
    }
}

fn parse_direction_votes_text(text: &str) -> Option<Vec<DirectionVote>> {
    parse_direction_votes_json(text.trim()).or_else(|| {
        JSON_FENCE_RE.captures_iter(text).find_map(|captures| {
            captures
                .get(1)
                .and_then(|body| parse_direction_votes_json(body.as_str()))
        })
    })
}

fn parse_direction_votes_json(text: &str) -> Option<Vec<DirectionVote>> {
    let value = serde_json::from_str::<Value>(text.trim()).ok()?;
    parse_direction_vote_value(&value)
}

fn parse_direction_vote_values(values: &[Value]) -> Option<Vec<DirectionVote>> {
    if values.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    for value in values {
        candidates.extend(parse_direction_vote_value(value)?);
    }
    Some(candidates)
}

fn parse_direction_vote_value(value: &Value) -> Option<Vec<DirectionVote>> {
    if let Some(items) = value.as_array() {
        let mut candidates = Vec::new();
        for item in items {
            candidates.push(parse_direction_vote_object(item)?);
        }
        return Some(candidates);
    }
    if let Some(items) = value.get("candidates").and_then(Value::as_array) {
        return parse_direction_vote_values(items);
    }
    if value.get("decision").is_some() {
        return Some(vec![parse_direction_vote_object(value)?]);
    }
    None
}

fn parse_direction_vote_object(value: &Value) -> Option<DirectionVote> {
    #[derive(Deserialize)]
    struct RawDirectionVote {
        decision: DirectionDecision,
        value: String,
        reasoning: String,
        #[serde(default)]
        source: String,
    }

    serde_json::from_value::<RawDirectionVote>(value.clone())
        .ok()
        .map(|raw| DirectionVote {
            decision: raw.decision,
            value: raw.value,
            reasoning: raw.reasoning,
            source: raw.source,
        })
}

fn is_legal_pattern(value: &str) -> bool {
    matches!(
        value,
        "linear" | "fan-out" | "adversarial" | "tournament" | "loop"
    )
}

pub fn direction_convergence_instruction(pool: &CandidatePool) -> String {
    let candidates = pool
        .candidates
        .iter()
        .map(vote_key)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Evaluate every merged candidate, including candidates proposed by other agents. Do not favor your own proposal. Valid candidates: {}",
        if candidates.is_empty() {
            "none".to_string()
        } else {
            candidates
        }
    )
}

static JSON_FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```json\s*(.*?)\s*```").expect("invalid JSON fence regex"));

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
        if !result.findings.is_empty() {
            let mut findings = parse_findings_values(&result.findings)
                .unwrap_or_else(|| parse_valid_findings_values(&result.findings));
            for finding in &mut findings {
                if finding.source.is_none() {
                    finding.source = Some(result.runner.clone());
                }
            }
            if !findings.is_empty() {
                out.extend(findings);
                continue;
            }
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
                reported_confidence: None,
                invalidated_when: None,
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

fn rounds_needed(kind: DecisionKind, diverge: bool) -> u64 {
    match (kind, diverge) {
        (DecisionKind::Both, true) => 3,
        (DecisionKind::Both, false) => 2,
        (DecisionKind::Direction, true) => 2,
        (DecisionKind::Direction, false) | (DecisionKind::Review, _) => 1,
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

fn direction_payload(tally: &Tally, candidate_pool: Option<CandidatePool>) -> DirectionPayload {
    DirectionPayload {
        converged: tally.supermajority_met,
        pick: tally.majority_pick.clone(),
        count: tally.majority_count,
        total: tally.total_voters,
        votes: tally.votes.clone(),
        minority: tally.minority.clone(),
        candidate_pool,
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

fn build_budget_brief(
    kind: DecisionKind,
    auditors: &[String],
    remaining: u64,
    required: u64,
) -> String {
    [
        "# LTO Decision Convergence Brief".to_string(),
        String::new(),
        "## Budget Exhausted".to_string(),
        String::new(),
        "- **Status**: NEEDS_HUMAN".to_string(),
        format!("- **Kind**: {}", decision_kind_label(kind)),
        format!("- **Available agents**: {}", join_or_none(auditors)),
        format!("- **Budget remaining**: ~{remaining} tokens"),
        format!("- **Estimated cost for this round**: ~{required} tokens"),
        String::new(),
        "Insufficient budget to spawn this deterministic decision round.".to_string(),
        "Host judgment is required; no decision has been made by the tool.".to_string(),
        budget_footer(0, remaining, required),
    ]
    .join("\n")
}

fn build_needs_human_brief(
    kind: DecisionKind,
    auditors: &[String],
    reason: &str,
    budget_remaining: u64,
    est_cost: u64,
) -> String {
    [
        "# LTO Decision Convergence Brief".to_string(),
        String::new(),
        "## Needs Human".to_string(),
        String::new(),
        "- **Status**: NEEDS_HUMAN".to_string(),
        format!("- **Kind**: {}", decision_kind_label(kind)),
        format!("- **Reason**: {}", md_inline(reason)),
        format!("- **Available agents**: {}", join_or_none(auditors)),
        String::new(),
        "The deterministic gate could not continue. The host must decide the next action."
            .to_string(),
        budget_footer(0, budget_remaining, est_cost),
    ]
    .join("\n")
}

fn build_decision_brief(
    kind: DecisionKind,
    status: DecisionStatus,
    result: Option<&DecisionResultPayload>,
    dissent: &DecisionDissent,
    auditors: &[String],
    budget_consumed_est: u64,
    budget_available_before: u64,
) -> String {
    let mut lines = vec![
        "# LTO Decision Convergence Brief".to_string(),
        String::new(),
        "This is a deterministic fact brief. It presents votes, findings, and budget; the host owns the final judgment.".to_string(),
        String::new(),
        format!("- **Status**: {}", decision_status_label(status)),
        format!("- **Kind**: {}", decision_kind_label(kind)),
        format!("- **Dispatched to**: {}", join_or_none(auditors)),
        format!(
            "- **Budget consumed (est)**: ~{} tokens",
            budget_consumed_est
        ),
        String::new(),
    ];

    match result {
        Some(DecisionResultPayload::Direction(direction)) => {
            append_direction_brief(&mut lines, direction, dissent)
        }
        Some(DecisionResultPayload::Review(review)) => append_review_brief(&mut lines, review),
        Some(DecisionResultPayload::Both(both)) => {
            append_direction_brief(&mut lines, &both.direction, dissent);
            append_review_brief(&mut lines, &both.review);
        }
        None => {}
    }

    lines.push("## Host Readout".to_string());
    lines.push(String::new());
    if status == DecisionStatus::Converged {
        lines.push(
            "The deterministic gates converged. Review the facts above before acting.".to_string(),
        );
    } else {
        lines.push(
            "The deterministic gates did not converge cleanly. Host judgment is required."
                .to_string(),
        );
    }
    lines.push(budget_footer(
        budget_consumed_est,
        budget_available_before,
        budget_consumed_est,
    ));
    lines.join("\n")
}

fn append_direction_brief(
    lines: &mut Vec<String>,
    direction: &DirectionPayload,
    dissent: &DecisionDissent,
) {
    lines.push("## Direction Track".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- **Majority pick**: {}",
        direction.pick.as_deref().unwrap_or("none")
    ));
    lines.push(format!(
        "- **Majority count**: {}/{}",
        direction.count, direction.total
    ));
    lines.push(format!(
        "- **NEEDS_HUMAN votes**: {}",
        dissent.needs_human_votes
    ));
    if dissent.needs_human_votes >= 2 {
        lines.push(
            "- **Escalation signal**: >=2 reviewers voted NEEDS_HUMAN; this is a strong display signal, distinct from the >=1 hard veto gate.".to_string(),
        );
    }
    if let Some(reason) = dissent.reason.as_deref() {
        lines.push(format!("- **Divergence note**: {}", md_inline(reason)));
    }
    if let Some(pool) = direction.candidate_pool.as_ref() {
        append_candidate_pool_brief(lines, pool, direction.votes.len());
    }
    lines.push(String::new());
    lines.push("| source | decision | value | reasoning |".to_string());
    lines.push("|---|---|---|---|".to_string());
    for vote in &direction.votes {
        lines.push(format!(
            "| {} | {} | {} | {} |",
            md_cell(&vote.source),
            direction_decision_label(&vote.decision),
            md_cell(&vote.value),
            md_cell(&vote.reasoning)
        ));
    }
    if !dissent.invalid_votes.is_empty() {
        lines.push(String::new());
        lines.push("### Invalid Votes".to_string());
        for vote in &dissent.invalid_votes {
            lines.push(format!(
                "- {}: {} -> {} ({})",
                md_inline(&vote.source),
                direction_decision_label(&vote.decision),
                md_inline(&vote.value),
                md_inline(&vote.reasoning)
            ));
        }
    }
    if !direction.minority.is_empty() {
        lines.push(String::new());
        lines.push("### Minority / Dissent".to_string());
        for vote in &direction.minority {
            lines.push(format!(
                "- {}: {} -> {} ({})",
                md_inline(&vote.source),
                direction_decision_label(&vote.decision),
                md_inline(&vote.value),
                md_inline(&vote.reasoning)
            ));
        }
    }
    lines.push(String::new());
}

fn append_candidate_pool_brief(
    lines: &mut Vec<String>,
    pool: &CandidatePool,
    convergence_votes: usize,
) {
    let proposed = pool.candidates.len() + pool.invalid_candidates.len();
    lines.push(String::new());
    lines.push("### Divergence Candidate Pool".to_string());
    lines.push(format!("- **Sources**: {}", join_or_none(&pool.sources)));
    lines.push(format!("- **Produced candidates**: {proposed}"));
    lines.push(format!(
        "- **Accepted into pool**: {}",
        pool.candidates.len()
    ));
    lines.push(format!(
        "- **Rejected by candidate gate**: {}",
        pool.invalid_candidates.len()
    ));
    lines.push(format!("- **Convergence votes**: {convergence_votes}"));
    lines.push(format!(
        "- **Convergence instruction**: {}",
        md_inline(&direction_convergence_instruction(pool))
    ));
    lines.push(String::new());
    lines.push("| source | proposal | value | reasoning |".to_string());
    lines.push("|---|---|---|---|".to_string());
    for candidate in &pool.candidates {
        lines.push(format!(
            "| {} | {} | {} | {} |",
            md_cell(&candidate.source),
            direction_decision_label(&candidate.decision),
            md_cell(&candidate.value),
            md_cell(&candidate.reasoning)
        ));
    }
    if !pool.invalid_candidates.is_empty() {
        lines.push(String::new());
        lines.push("#### Invalid Candidates".to_string());
        for candidate in &pool.invalid_candidates {
            lines.push(format!(
                "- {}: {} -> {} ({})",
                md_inline(&candidate.source),
                direction_decision_label(&candidate.decision),
                md_inline(&candidate.value),
                md_inline(&candidate.reasoning)
            ));
        }
    }
}

fn append_review_brief(lines: &mut Vec<String>, review: &ReviewPayload) {
    lines.push("## Review Track".to_string());
    lines.push(String::new());
    lines.push(format!("- **Merged findings**: {}", review.findings.len()));
    for finding in &review.findings {
        lines.push(format!(
            "- **{:?}** [{}] {}",
            finding.severity,
            finding.source.as_deref().unwrap_or("unknown"),
            md_inline(&finding.claim)
        ));
        if let Some(file) = &finding.file {
            lines.push(format!("  - File: `{}`", md_inline(file)));
        }
        if let Some(evidence) = &finding.evidence_to_check {
            lines.push(format!("  - Evidence: {}", md_inline(evidence)));
        }
        if let Some(confidence) = &finding.reported_confidence {
            let level = confidence
                .level
                .as_ref()
                .map(crate::audit::ReportedConfidenceLevel::as_str)
                .unwrap_or("unknown");
            let rationale = confidence
                .rationale
                .as_deref()
                .map(|rationale| format!(" — {}", md_inline(rationale)))
                .unwrap_or_default();
            lines.push(format!("  - [confidence: {level}{rationale}]"));
        }
        if let Some(invalidated_when) = &finding.invalidated_when {
            lines.push(format!("  - 失效条件: {}", md_inline(invalidated_when)));
        }
    }
    lines.push(String::new());
}

fn budget_footer(consumed: u64, available_before: u64, next_round_cost: u64) -> String {
    let remaining_after = available_before.saturating_sub(consumed);
    let enough_next_round = next_round_cost == 0 || remaining_after >= next_round_cost;
    [
        String::new(),
        "---".to_string(),
        format!(
            "**Budget note**: consumed_est={} tokens; remaining_est={} tokens; enough_for_next_round={}. Token counts are approximate runner estimates.",
            consumed, remaining_after, enough_next_round
        ),
    ]
    .join("\n")
}

fn decision_kind_label(kind: DecisionKind) -> &'static str {
    match kind {
        DecisionKind::Direction => "direction",
        DecisionKind::Review => "review",
        DecisionKind::Both => "both",
    }
}

fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Converged => "converged",
        DecisionStatus::NeedsInfo => "needs_human",
        DecisionStatus::NeedsHuman => "needs_human",
    }
}

fn direction_decision_label(decision: &DirectionDecision) -> &'static str {
    match decision {
        DirectionDecision::PickTask => "pick_task",
        DirectionDecision::PickPattern => "pick_pattern",
        DirectionDecision::NeedsHuman => "needs_human",
    }
}

fn join_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn md_cell(value: &str) -> String {
    md_inline(value).replace('|', "\\|")
}

fn md_inline(value: &str) -> String {
    value.replace('\n', " ").trim().to_string()
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
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T1".into()])),
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
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
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
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
            diverge: false,
            spawned_escalate_keys: BTreeSet::from(["same".into()]),
            valid_task_ids: None,
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
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
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
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
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
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

    #[test]
    fn decision_brief_expands_votes_strong_signal_and_budget() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Direction,
            auditors: vec!["codex".into(), "pi".into(), "agy".into()],
            budget_remaining: 100_000,
            escalate_key: "brief".into(),
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T1".into()])),
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
            direction_votes: vec![
                vote("codex", DirectionDecision::NeedsHuman, "ambiguous"),
                vote("pi", DirectionDecision::NeedsHuman, "missing evidence"),
                vote("agy", DirectionDecision::PickTask, "T1"),
            ],
            review_results: vec![],
        });
        assert_eq!(outcome.status, DecisionStatus::NeedsInfo);
        assert!(
            outcome
                .brief
                .contains("| source | decision | value | reasoning |")
        );
        assert!(outcome.brief.contains(">=2 reviewers voted NEEDS_HUMAN"));
        assert!(outcome.brief.contains("Budget note"));
        assert!(outcome.brief.contains("enough_for_next_round=true"));
    }

    #[test]
    fn budget_exhausted_brief_uses_budget_template() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Both,
            auditors: vec!["codex".into(), "pi".into()],
            budget_remaining: 1,
            escalate_key: "budget".into(),
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: None,
            task_universe: BTreeSet::new(),
            candidate_results: vec![],
            direction_votes: vec![],
            review_results: vec![],
        });
        assert_eq!(outcome.status, DecisionStatus::NeedsHuman);
        assert!(outcome.brief.contains("## Budget Exhausted"));
        assert!(outcome.brief.contains("Estimated cost for this round"));
    }

    #[test]
    fn request_deserialization_defaults_diverge_to_true() {
        let request = serde_json::from_value::<DecisionRunRequest>(serde_json::json!({
            "kind": "direction",
            "auditors": ["codex", "pi"],
            "budget_remaining": 100000,
            "escalate_key": "k"
        }))
        .unwrap();
        assert!(request.diverge);
    }

    #[test]
    fn merge_candidates_unions_sources_without_deduplication() {
        let candidates = merge_candidates(&[
            result_with_text(
                "codex",
                r#"[{"decision":"pick_task","value":"T1","reasoning":"first"}]"#,
            ),
            result_with_text(
                "pi",
                r#"{"candidates":[{"decision":"pick_task","value":"T1","reasoning":"duplicate from another source"}]}"#,
            ),
            result_with_text(
                "agy",
                r#"```json
{"decision":"pick_pattern","value":"fan-out","reasoning":"parallelize"}
```"#,
            ),
        ]);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].source, "codex");
        assert_eq!(candidates[1].source, "pi");
        assert_eq!(candidates[2].source, "agy");
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.value == "T1")
                .count(),
            2
        );
    }

    #[test]
    fn diverge_filters_invalid_candidates_and_uses_pool_task_whitelist() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Direction,
            auditors: vec!["codex".into(), "pi".into(), "agy".into()],
            budget_remaining: 100_000,
            escalate_key: "diverge".into(),
            diverge: true,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T1".into(), "T2".into(), "T99".into()])),
            task_universe: BTreeSet::from(["T1".into(), "T2".into()]),
            candidate_results: vec![
                result_with_text(
                    "codex",
                    r#"[{"decision":"pick_task","value":"T1","reasoning":"primary"}]"#,
                ),
                result_with_text(
                    "pi",
                    r#"[{"decision":"pick_task","value":"T2","reasoning":"alternate"}]"#,
                ),
                result_with_text(
                    "agy",
                    r#"[{"decision":"pick_task","value":"DROP","reasoning":"bad task"},{"decision":"pick_pattern","value":"chaos","reasoning":"bad pattern"}]"#,
                ),
            ],
            direction_votes: vec![
                vote("codex", DirectionDecision::PickTask, "T1"),
                vote("pi", DirectionDecision::PickTask, "T1"),
                vote("agy", DirectionDecision::PickTask, "T99"),
            ],
            review_results: vec![],
        });
        assert_eq!(outcome.status, DecisionStatus::Converged);
        assert_eq!(outcome.budget_consumed_est, EST_TOKENS_PER_ROUND * 2);
        assert_eq!(outcome.dissent.invalid_candidates.len(), 2);
        assert_eq!(outcome.dissent.invalid_votes.len(), 1);
        assert!(outcome.brief.contains("### Divergence Candidate Pool"));
        assert!(outcome.brief.contains("Produced candidates**: 4"));
        assert!(outcome.brief.contains("Accepted into pool**: 2"));
        assert!(outcome.brief.contains("Rejected by candidate gate**: 2"));
        assert!(outcome.brief.contains("Do not favor your own proposal"));
        match outcome.result {
            Some(DecisionResultPayload::Direction(DirectionPayload {
                candidate_pool: Some(pool),
                pick: Some(pick),
                count,
                total,
                ..
            })) => {
                assert_eq!(pick, "pick_task:T1");
                assert_eq!(count, 2);
                assert_eq!(total, 2);
                assert_eq!(pool.sources, vec!["agy", "codex", "pi"]);
                assert_eq!(pool.candidates.len(), 2);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn diverge_with_too_few_candidate_families_falls_back_to_single_stage_convergence() {
        let outcome = run_decision(DecisionRunRequest {
            kind: DecisionKind::Direction,
            auditors: vec!["codex".into(), "pi".into(), "agy".into()],
            budget_remaining: 100_000,
            escalate_key: "fallback".into(),
            diverge: true,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T2".into()])),
            task_universe: BTreeSet::from(["T1".into()]),
            candidate_results: vec![result_with_text(
                "codex",
                r#"[{"decision":"pick_task","value":"T1","reasoning":"only one family"}]"#,
            )],
            direction_votes: vec![
                vote("codex", DirectionDecision::PickTask, "T2"),
                vote("pi", DirectionDecision::PickTask, "T2"),
            ],
            review_results: vec![],
        });
        assert_eq!(outcome.status, DecisionStatus::Converged);
        assert_eq!(
            outcome.dissent.reason.as_deref(),
            Some(
                "direction diverge: fewer than two heterogeneous candidate sources; fell back to single-stage convergence"
            )
        );
        assert!(outcome.brief.contains("Divergence note"));
        assert!(matches!(
            outcome.result,
            Some(DecisionResultPayload::Direction(DirectionPayload {
                pick: Some(_),
                candidate_pool: Some(_),
                ..
            }))
        ));
    }

    #[test]
    fn finding_metadata_isolation_keeps_direction_status_pick_and_gate_verdict() {
        let baseline = both_decision(vec![
            result_with_text("codex", r#"[{"severity":"high","claim":"A"}]"#),
            result_with_text("pi", r#"[{"severity":"medium","claim":"B"}]"#),
        ]);
        let enriched = both_decision(vec![
            result_with_text(
                "codex",
                r#"[{"severity":"high","claim":"A","reported_confidence":{"level":"low","rationale":"limited evidence"},"invalidated_when":"counterexample A"}]"#,
            ),
            result_with_text(
                "pi",
                r#"[{"severity":"medium","claim":"B","reported_confidence":"high","invalidated_when":"counterexample B"}]"#,
            ),
        ]);

        assert_same_control_outputs(&baseline, &enriched);
        assert!(!baseline.brief.contains("[confidence:"));
        assert!(
            enriched
                .brief
                .contains("[confidence: low — limited evidence]")
        );
        assert!(enriched.brief.contains("失效条件: counterexample A"));
    }

    #[test]
    fn structured_finding_metadata_isolation_keeps_control_outputs() {
        let mut baseline_codex = result_with_text("codex", "structured baseline");
        baseline_codex.findings = vec![serde_json::json!({"severity": "high", "claim": "A"})];
        let mut baseline_pi = result_with_text("pi", "structured baseline");
        baseline_pi.findings = vec![serde_json::json!({"severity": "medium", "claim": "B"})];

        let mut enriched_codex = result_with_text("codex", "structured enriched");
        enriched_codex.findings = vec![serde_json::json!({
            "severity": "high",
            "claim": "A",
            "reported_confidence": {"level": "medium", "rationale": "typed result"},
            "invalidated_when": "typed counterexample A"
        })];
        let mut enriched_pi = result_with_text("pi", "structured enriched");
        enriched_pi.findings = vec![serde_json::json!({
            "severity": "medium",
            "claim": "B",
            "reported_confidence": "low",
            "invalidated_when": "typed counterexample B"
        })];

        let baseline = both_decision(vec![baseline_codex, baseline_pi]);
        let enriched = both_decision(vec![enriched_codex, enriched_pi]);
        assert_same_control_outputs(&baseline, &enriched);
        assert!(
            enriched
                .brief
                .contains("[confidence: medium — typed result]")
        );
        assert!(enriched.brief.contains("失效条件: typed counterexample A"));
    }

    #[test]
    fn unstructured_review_fallback_defaults_c3_metadata_without_panicking() {
        let findings = merge_findings(&[result_with_text("pi", "unstructured review")]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].reported_confidence, None);
        assert_eq!(findings[0].invalidated_when, None);
    }

    #[test]
    fn merge_findings_keeps_valid_structured_items_from_mixed_batch() {
        let mut result = result_with_text("pi", "fallback text");
        result.findings = vec![
            serde_json::json!({
                "severity": "high",
                "claim": "valid",
                "reported_confidence": "High"
            }),
            serde_json::json!({"severity": "INVALID", "claim": "bad"}),
        ];
        let findings = merge_findings(&[result]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].claim, "valid");
        assert_eq!(
            findings[0]
                .reported_confidence
                .as_ref()
                .and_then(|confidence| confidence.level.as_ref()),
            Some(&crate::audit::ReportedConfidenceLevel::High)
        );
    }

    fn both_decision(review_results: Vec<AgentResult>) -> DecisionOutcome {
        run_decision(DecisionRunRequest {
            kind: DecisionKind::Both,
            auditors: vec!["codex".into(), "pi".into()],
            budget_remaining: 100_000,
            escalate_key: "c3-isolation".into(),
            diverge: false,
            spawned_escalate_keys: BTreeSet::new(),
            valid_task_ids: Some(BTreeSet::from(["T1".into()])),
            task_universe: BTreeSet::from(["T1".into()]),
            candidate_results: vec![],
            direction_votes: vec![
                vote("codex", DirectionDecision::PickTask, "T1"),
                vote("pi", DirectionDecision::PickTask, "T1"),
            ],
            review_results,
        })
    }

    fn assert_same_control_outputs(baseline: &DecisionOutcome, enriched: &DecisionOutcome) {
        assert_eq!(baseline.status, enriched.status);
        assert_eq!(baseline.dispatched_to, enriched.dispatched_to);
        assert_eq!(baseline.budget_consumed_est, enriched.budget_consumed_est);
        assert_eq!(baseline.record_escalate_key, enriched.record_escalate_key);
        assert_eq!(
            baseline.dissent.needs_human_votes,
            enriched.dissent.needs_human_votes
        );
        let baseline_direction = match baseline.result.as_ref() {
            Some(DecisionResultPayload::Both(payload)) => &payload.direction,
            other => panic!("unexpected baseline result: {other:?}"),
        };
        let enriched_direction = match enriched.result.as_ref() {
            Some(DecisionResultPayload::Both(payload)) => &payload.direction,
            other => panic!("unexpected enriched result: {other:?}"),
        };
        assert_eq!(baseline_direction, enriched_direction);
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
