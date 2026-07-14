use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRound {
    pub label: String,
    pub high: u64,
    pub critical: u64,
}

impl LedgerRound {
    pub fn blockers(&self) -> u64 {
        self.high.saturating_add(self.critical)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerVerdict {
    NoObservations,
    Converged,
    Converging,
    Rebound,
    Stalled,
}

impl LedgerVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoObservations => "NO_OBSERVATIONS",
            Self::Converged => "CONVERGED",
            Self::Converging => "CONVERGING",
            Self::Rebound => "REBOUND",
            Self::Stalled => "STALLED",
        }
    }
}

pub fn parse_ledger(text: &str) -> anyhow::Result<Vec<LedgerRound>> {
    let mut rounds = Vec::new();
    let mut in_summary = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_summary = line.trim().eq_ignore_ascii_case("## Round Summary");
            continue;
        }
        if !in_summary || !line.contains('|') {
            continue;
        }
        let cells = split_cells(line);
        if cells.len() <= 4 || is_separator_row(&cells) || is_header_row(&cells) {
            continue;
        }
        let label = cells.first().cloned().unwrap_or_default();
        let high_raw = cells.get(3).cloned().unwrap_or_default();
        let critical_raw = cells.get(4).cloned().unwrap_or_default();
        if high_raw.is_empty() && critical_raw.is_empty() {
            continue;
        }
        rounds.push(LedgerRound {
            high: parse_count(&high_raw, &label, "high")?,
            critical: parse_count(&critical_raw, &label, "critical")?,
            label,
        });
    }
    Ok(rounds)
}

pub fn evaluate_ledger(rounds: &[LedgerRound], strict: bool) -> LedgerVerdict {
    if rounds.is_empty() {
        return LedgerVerdict::NoObservations;
    }
    let blockers = rounds.iter().map(LedgerRound::blockers).collect::<Vec<_>>();
    if blockers.last().copied().unwrap_or_default() == 0 {
        return LedgerVerdict::Converged;
    }
    for pair in blockers.windows(2) {
        if pair[1] > pair[0] {
            return LedgerVerdict::Rebound;
        }
        if strict && pair[1] == pair[0] && pair[1] > 0 {
            return LedgerVerdict::Stalled;
        }
    }
    LedgerVerdict::Converging
}

pub fn ledger_sequence(rounds: &[LedgerRound]) -> String {
    rounds
        .iter()
        .map(|round| format!("{}={}", round.label, round.blockers()))
        .collect::<Vec<_>>()
        .join(" -> ")
}

pub fn has_real_ledger_rounds(text: &str) -> bool {
    parse_ledger(text)
        .map(|rounds| !rounds.is_empty())
        .unwrap_or(false)
}

fn split_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_separator_row(cells: &[String]) -> bool {
    let joined = cells.join("");
    !joined.is_empty() && joined.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
}

fn is_header_row(cells: &[String]) -> bool {
    cells.iter().any(|cell| cell.eq_ignore_ascii_case("round"))
}

fn parse_count(raw: &str, label: &str, column: &str) -> anyhow::Result<u64> {
    let mut total = 0_u64;
    for part in raw
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let value = part
            .parse::<u64>()
            .with_context(|| format!("non-numeric {column} count in {label}: {raw:?}"))?;
        total = total.saturating_add(value);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(label: &str, blockers: u64) -> LedgerRound {
        LedgerRound {
            label: label.to_string(),
            high: blockers,
            critical: 0,
        }
    }

    #[test]
    fn parse_ledger_reads_rounds_and_additive_counts() {
        let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n|---|---:|---:|---:|---:|\n| R1 | 4 | 0 | 1+1 | 2 |";
        let rounds = parse_ledger(text).unwrap();
        assert_eq!(
            rounds,
            vec![LedgerRound {
                label: "R1".into(),
                high: 2,
                critical: 2
            }]
        );
    }

    #[test]
    fn parse_ledger_ignores_tables_outside_round_summary() {
        let text =
            "## Findings\n| Round | Total | Medium | High | Critical |\n| R1 | 1 | 0 | 1 | 0 |";
        assert!(parse_ledger(text).unwrap().is_empty());
    }

    #[test]
    fn parse_ledger_empty_template_has_no_observations() {
        let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n|---|---:|---:|---:|---:|";
        assert!(parse_ledger(text).unwrap().is_empty());
    }

    #[test]
    fn parse_ledger_skips_unfilled_template_rows() {
        let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n| R1 | | | | |";
        assert!(parse_ledger(text).unwrap().is_empty());
    }

    #[test]
    fn parse_ledger_rejects_non_numeric_blockers() {
        let text = "## Round Summary\n| Round | Total | Medium | High | Critical |\n| R1 | 1 | 0 | nope | 0 |";
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
            },
            round("R2", 1),
        ];
        assert_eq!(ledger_sequence(&rounds), "R1=3 -> R2=1");
    }
}
