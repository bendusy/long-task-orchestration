use super::*;

fn round(label: &str, blockers: u64) -> LedgerRound {
    LedgerRound {
        label: label.to_string(),
        high: blockers,
        critical: 0,
        auditors: None,
        coverage: None,
    }
}

fn diagnostics(blockers: &[u64]) -> LedgerDiagnostics {
    let rounds = blockers
        .iter()
        .enumerate()
        .map(|(index, blockers)| round(&format!("R{}", index + 1), *blockers))
        .collect::<Vec<_>>();
    diagnose(&rounds).unwrap()
}

#[test]
fn parse_ledger_reads_rounds_and_additive_counts() {
    let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n|---|---:|---:|---:|---:|\n| R1 | 4 | 0 | 1+1 | 2 |";
    assert_eq!(
        parse_ledger(text).unwrap(),
        vec![LedgerRound {
            label: "R1".into(),
            high: 2,
            critical: 2,
            auditors: None,
            coverage: None,
        }]
    );
}

#[test]
fn parse_ledger_ignores_tables_outside_round_summary() {
    let text = "## Findings\n| Round | Total | Medium | High | Critical |\n| R1 | 1 | 0 | 1 | 0 |";
    assert!(parse_ledger(text).unwrap().is_empty());
}

#[test]
fn parse_ledger_empty_template_has_no_observations() {
    let text =
        "## Round Summary\n| Round | Total | Medium | High | Critical |\n|---|---:|---:|---:|---:|";
    assert!(parse_ledger(text).unwrap().is_empty());
}

#[test]
fn parse_ledger_skips_unfilled_template_rows() {
    let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n| R1 | | | | |";
    assert!(parse_ledger(text).unwrap().is_empty());
}

#[test]
fn parse_ledger_rejects_non_numeric_blockers() {
    let text =
        "## Round Summary\n| Round | Total | Medium | High | Critical |\n| R1 | 1 | 0 | nope | 0 |";
    assert!(parse_ledger(text).is_err());
}

#[test]
fn evaluate_ledger_empty_is_no_observations() {
    assert_eq!(evaluate_ledger(&[], false), LedgerVerdict::NoObservations);
}

#[test]
fn evaluate_ledger_terminal_zero_wins_after_rebound() {
    let rounds = [round("R1", 1), round("R2", 2), round("R3", 0)];
    assert_eq!(evaluate_ledger(&rounds, true), LedgerVerdict::Converged);
}

#[test]
fn evaluate_ledger_improving_nonzero_is_converging() {
    let rounds = [round("R1", 2), round("R2", 1)];
    assert_eq!(evaluate_ledger(&rounds, false), LedgerVerdict::Converging);
}

#[test]
fn evaluate_ledger_increase_is_rebound() {
    let rounds = [round("R1", 1), round("R2", 2)];
    assert_eq!(evaluate_ledger(&rounds, false), LedgerVerdict::Rebound);
}

#[test]
fn evaluate_ledger_flat_is_stalled_only_in_strict_mode() {
    let rounds = [round("R1", 1), round("R2", 1)];
    assert_eq!(evaluate_ledger(&rounds, true), LedgerVerdict::Stalled);
    assert_eq!(evaluate_ledger(&rounds, false), LedgerVerdict::Converging);
}

#[test]
fn ledger_sequence_formats_blocker_totals() {
    let rounds = [
        LedgerRound {
            label: "R1".into(),
            high: 1,
            critical: 2,
            auditors: None,
            coverage: None,
        },
        round("R2", 1),
    ];
    assert_eq!(ledger_sequence(&rounds), "R1=3 -> R2=1");
}

#[test]
fn diagnose_empty_has_no_terminal_value() {
    assert_eq!(diagnose(&[]), None);
}

#[test]
fn diagnose_short_sequences_are_insufficient() {
    let single = diagnostics(&[5]);
    assert_eq!(single.sample_sufficiency, SampleSufficiency::Insufficient);
    assert_eq!(single.terminal, Terminal::Nonzero);
    assert_eq!(single.direction, Direction::Flat);
    assert_eq!(single.oscillation, Oscillation::None);
    assert_eq!(single.envelope, Envelope::Unknown);

    let pair = diagnostics(&[5, 4]);
    assert_eq!(pair.sample_sufficiency, SampleSufficiency::Insufficient);
    assert_eq!(pair.direction, Direction::Improving);
}

#[test]
fn diagnose_equal_and_zero_tail_boundaries() {
    let equal = diagnostics(&[5, 5, 5]);
    assert_eq!(equal.direction, Direction::Flat);
    assert_eq!(equal.oscillation, Oscillation::None);
    assert_eq!(equal.envelope, Envelope::Unknown);

    let zero = diagnostics(&[2, 1, 0]);
    assert_eq!(zero.terminal, Terminal::Zero);
    assert_eq!(zero.direction, Direction::Improving);
}

#[test]
fn diagnose_terminal_zero_preserves_single_rebound() {
    let result = diagnostics(&[1, 2, 0]);
    assert_eq!(result.terminal, Terminal::Zero);
    assert_eq!(result.direction, Direction::Mixed);
    assert_eq!(result.oscillation, Oscillation::SingleRebound);
    assert_eq!(result.envelope, Envelope::Unknown);
}

#[test]
fn diagnose_alternating_shrinking_peak_envelope() {
    let result = diagnostics(&[5, 2, 4, 1, 3]);
    assert_eq!(result.sample_sufficiency, SampleSufficiency::Sufficient);
    assert_eq!(result.terminal, Terminal::Nonzero);
    assert_eq!(result.direction, Direction::Mixed);
    assert_eq!(result.oscillation, Oscillation::Alternating);
    assert_eq!(result.envelope, Envelope::Shrinking);
}

#[test]
fn diagnose_improving_nonzero_sequence() {
    let result = diagnostics(&[5, 4, 3]);
    assert_eq!(result.terminal, Terminal::Nonzero);
    assert_eq!(result.direction, Direction::Improving);
    assert_eq!(result.oscillation, Oscillation::None);
    assert_eq!(result.envelope, Envelope::Unknown);
}

#[test]
fn diagnose_distinguishes_expanding_flat_and_unknown_envelopes() {
    assert_eq!(diagnostics(&[3, 1, 4, 2, 5]).envelope, Envelope::Expanding);
    assert_eq!(diagnostics(&[5, 1, 5, 1, 5]).envelope, Envelope::Flat);
    assert_eq!(diagnostics(&[5, 1, 4, 1, 6]).envelope, Envelope::Unknown);
}

#[test]
fn diagnose_collapses_plateaus_before_counting_turns_and_peaks() {
    let result = diagnostics(&[5, 5, 4, 5, 5]);
    assert_eq!(result.direction, Direction::Mixed);
    assert_eq!(result.oscillation, Oscillation::SingleRebound);
    assert_eq!(result.envelope, Envelope::Flat);
}

#[test]
fn parse_ledger_supports_old_and_new_lineage_columns() {
    let old = "## Round Summary\n| round | artifact | auditors | high | critical | minor | trend | status |\n|---|---|---|---:|---:|---:|---|---|\n| R1 | reply | pi agy | 2 | 1 | 0 | start | open |";
    let old_round = parse_ledger(old).unwrap().remove(0);
    assert_eq!(old_round.auditors.as_deref(), Some("pi agy"));
    assert_eq!(old_round.coverage, None);

    let new = "## Round Summary\n| round | artifact | auditors | coverage | high | critical | minor | trend | status |\n|---|---|---|---|---:|---:|---:|---|---|\n| R1 | reply | pi agy | src/ledger.rs | 2 | 1 | 0 | start | open |";
    let new_rounds = parse_ledger(new).unwrap();
    assert_eq!(new_rounds[0].auditors.as_deref(), Some("pi agy"));
    assert_eq!(new_rounds[0].coverage.as_deref(), Some("src/ledger.rs"));
    assert_eq!(
        diagnose(&new_rounds).unwrap().confidence,
        DiagnosticConfidence::AdvisoryLineageRecorded
    );

    let missing = "## Round Summary\n| round | artifact | auditors | coverage | high | critical | minor | trend | status |\n|---|---|---|---|---:|---:|---:|---|---|\n| R1 | reply | pi agy | | 2 | 1 | 0 | start | open |";
    assert_eq!(
        diagnose(&parse_ledger(missing).unwrap())
            .unwrap()
            .confidence,
        DiagnosticConfidence::LowNoLineage
    );
}

#[test]
fn parse_ledger_supports_mixed_rows_after_header_upgrade() {
    let text = "## Round Summary\n| round | artifact | auditors | coverage | high | critical | minor | trend | status |\n|---|---|---|---|---:|---:|---:|---|---|\n| R1 | old | pi | 2 | 1 | 0 | start | open |\n| R2 | new | pi agy | T1,T2 | 1 | 0 | 0 | down | open |";
    let rounds = parse_ledger(text).unwrap();
    assert_eq!(rounds[0].blockers(), 3);
    assert_eq!(rounds[0].coverage, None);
    assert_eq!(rounds[1].blockers(), 1);
    assert_eq!(rounds[1].coverage.as_deref(), Some("T1,T2"));
}

#[test]
fn parse_ledger_uses_reordered_legacy_header_for_same_width_rows() {
    let text = "## Round Summary\n| round | artifact | auditors | critical | high | minor | trend | status |\n|---|---|---|---:|---:|---:|---|---|\n| R1 | old | pi | 1 | 2 | 0 | start | open |";
    let round = parse_ledger(text).unwrap().remove(0);
    assert_eq!(round.high, 2);
    assert_eq!(round.critical, 1);
    assert_eq!(round.auditors.as_deref(), Some("pi"));
    assert_eq!(round.coverage, None);
}

#[test]
fn diagnostics_summary_and_enum_strings_are_stable() {
    let result = diagnostics(&[5, 4, 3]);
    assert_eq!(
        DiagnosticConfidence::LowNoLineage.as_str(),
        "low (no lineage)"
    );
    assert_eq!(
        result.summary(),
        "sample_sufficiency=sufficient, terminal=nonzero, direction=improving, oscillation=none, envelope=unknown, confidence=low (no lineage)"
    );
}

#[test]
fn entropy_review_is_advisory_and_excludes_single_rebound() {
    assert!(!diagnostics(&[1, 2, 0]).suggests_entropy_review());
    assert!(diagnostics(&[5, 2, 4, 1, 3]).suggests_entropy_review());
    assert!(!diagnostics(&[5, 4]).suggests_entropy_review());
}
