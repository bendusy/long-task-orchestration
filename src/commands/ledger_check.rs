use crate::ledger::{self, LedgerDiagnostics, LedgerRound, LedgerVerdict};
use anyhow::Context;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerCheckReport {
    pub rounds: Vec<LedgerRound>,
    pub verdict: LedgerVerdict,
    pub diagnostics: Option<LedgerDiagnostics>,
}

impl LedgerCheckReport {
    pub fn exit_code(&self) -> i32 {
        match self.verdict {
            LedgerVerdict::NoObservations
            | LedgerVerdict::Converged
            | LedgerVerdict::Converging => 0,
            LedgerVerdict::Rebound | LedgerVerdict::Stalled => 1,
        }
    }

    pub fn render(&self) -> String {
        let mut lines = if self.rounds.is_empty() {
            vec!["no filled rounds yet".to_string()]
        } else {
            vec![format!(
                "blocker sequence: {}",
                ledger::ledger_sequence(&self.rounds)
            )]
        };
        lines.push(format!("verdict: {}", self.verdict.as_str()));
        lines.push(match self.diagnostics {
            Some(diagnostics) => format!("diagnostics: {}", diagnostics.summary()),
            None => "diagnostics: unavailable (no observations)".to_string(),
        });
        format!("{}\n", lines.join("\n"))
    }
}

pub fn evaluate_text(text: &str, strict: bool) -> anyhow::Result<LedgerCheckReport> {
    if !text
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("## Round Summary"))
    {
        anyhow::bail!("no Round Summary section");
    }
    let rounds = ledger::parse_ledger(text)?;
    Ok(LedgerCheckReport {
        verdict: ledger::evaluate_ledger(&rounds, strict),
        diagnostics: ledger::diagnose(&rounds),
        rounds,
    })
}

pub fn evaluate_path(path: &Path, strict: bool) -> anyhow::Result<LedgerCheckReport> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("cannot read ledger {}", path.display()))?;
    evaluate_text(&text, strict).with_context(|| format!("cannot parse ledger {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_rows(rows: &[&str]) -> String {
        format!(
            "## Round Summary\n\n| round | artifact | auditors | high | critical | minor | trend | status |\n|---|---|---|---:|---:|---:|---|---|\n{}\n",
            rows.join("\n")
        )
    }

    #[test]
    fn no_observations_is_success_without_fabricated_diagnostics() {
        let report = evaluate_text(&ledger_rows(&[]), false).unwrap();
        assert_eq!(report.verdict, LedgerVerdict::NoObservations);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(
            report.render(),
            "no filled rounds yet\nverdict: NO_OBSERVATIONS\ndiagnostics: unavailable (no observations)\n"
        );
    }

    #[test]
    fn terminal_zero_wins_after_rebound() {
        let report = evaluate_text(
            &ledger_rows(&[
                "| R1 | v1 | codex | 1 | 0 | 0 | start | open |",
                "| R2 | v2 | codex | 2 | 0 | 0 | up | open |",
                "| R3 | v3 | codex | 0 | 0 | 0 | down | closed |",
            ]),
            true,
        )
        .unwrap();
        assert_eq!(report.verdict, LedgerVerdict::Converged);
        assert_eq!(report.exit_code(), 0);
        assert!(
            report
                .render()
                .contains("blocker sequence: R1=1 -> R2=2 -> R3=0")
        );
        assert!(
            report
                .render()
                .contains("diagnostics: sample_sufficiency=sufficient")
        );
    }

    #[test]
    fn rebound_and_strict_stall_are_hard_failures() {
        let rebound = evaluate_text(
            &ledger_rows(&[
                "| R1 | v1 | codex | 1 | 0 | 0 | start | open |",
                "| R2 | v2 | codex | 2 | 0 | 0 | up | open |",
            ]),
            false,
        )
        .unwrap();
        assert_eq!(rebound.verdict, LedgerVerdict::Rebound);
        assert_eq!(rebound.exit_code(), 1);

        let flat = ledger_rows(&[
            "| R1 | v1 | codex | 2 | 0 | 0 | start | open |",
            "| R2 | v2 | codex | 2 | 0 | 0 | flat | open |",
        ]);
        let non_strict = evaluate_text(&flat, false).unwrap();
        assert_eq!(non_strict.verdict, LedgerVerdict::Converging);
        assert_eq!(non_strict.exit_code(), 0);
        let strict = evaluate_text(&flat, true).unwrap();
        assert_eq!(strict.verdict, LedgerVerdict::Stalled);
        assert_eq!(strict.exit_code(), 1);
    }

    #[test]
    fn invalid_count_is_a_parse_error() {
        let error = evaluate_text(
            &ledger_rows(&["| R1 | v1 | codex | nope | 0 | 0 | start | open |"]),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("non-numeric high count"));
    }

    #[test]
    fn missing_round_summary_is_a_parse_error() {
        let error = evaluate_text("# not a ledger\n", false).unwrap_err();
        assert!(error.to_string().contains("no Round Summary section"));
    }
}
