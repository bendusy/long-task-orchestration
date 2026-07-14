use anyhow::Context;
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRound {
    pub label: String,
    pub high: u64,
    pub critical: u64,
    pub auditors: Option<String>,
    pub coverage: Option<String>,
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

macro_rules! diagnostic_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }
    };
}

diagnostic_enum!(SampleSufficiency {
    Insufficient => "insufficient",
    Sufficient => "sufficient",
});
diagnostic_enum!(Terminal { Zero => "zero", Nonzero => "nonzero" });
diagnostic_enum!(Direction {
    Improving => "improving",
    Flat => "flat",
    Worsening => "worsening",
    Mixed => "mixed",
});
diagnostic_enum!(Oscillation {
    None => "none",
    SingleRebound => "single_rebound",
    Alternating => "alternating",
});
diagnostic_enum!(Envelope {
    Shrinking => "shrinking",
    Flat => "flat",
    Expanding => "expanding",
    Unknown => "unknown",
});
diagnostic_enum!(DiagnosticConfidence {
    LowNoLineage => "low (no lineage)",
    AdvisoryLineageRecorded => "advisory (lineage recorded)",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerDiagnostics {
    pub sample_sufficiency: SampleSufficiency,
    pub terminal: Terminal,
    pub direction: Direction,
    pub oscillation: Oscillation,
    pub envelope: Envelope,
    pub confidence: DiagnosticConfidence,
}

impl LedgerDiagnostics {
    pub fn summary(self) -> String {
        format!(
            "sample_sufficiency={}, terminal={}, direction={}, oscillation={}, envelope={}, confidence={}",
            self.sample_sufficiency.as_str(),
            self.terminal.as_str(),
            self.direction.as_str(),
            self.oscillation.as_str(),
            self.envelope.as_str(),
            self.confidence.as_str()
        )
    }

    pub fn suggests_entropy_review(self) -> bool {
        self.sample_sufficiency == SampleSufficiency::Sufficient
            && self.oscillation != Oscillation::SingleRebound
            && (self.oscillation == Oscillation::Alternating
                || self.envelope != Envelope::Shrinking)
    }
}

#[derive(Clone, Copy)]
struct SummaryColumns {
    high: usize,
    critical: usize,
    auditors: Option<usize>,
    coverage: Option<usize>,
}

impl SummaryColumns {
    fn from_header(cells: &[String]) -> Option<Self> {
        let position = |name: &str| {
            cells
                .iter()
                .position(|cell| cell.eq_ignore_ascii_case(name))
        };
        Some(Self {
            high: position("high")?,
            critical: position("critical")?,
            auditors: position("auditors"),
            coverage: position("coverage"),
        })
    }

    fn for_width(width: usize) -> Self {
        if width >= 9 {
            Self {
                high: 4,
                critical: 5,
                auditors: Some(2),
                coverage: Some(3),
            }
        } else if width >= 8 {
            Self {
                high: 3,
                critical: 4,
                auditors: Some(2),
                coverage: None,
            }
        } else {
            Self {
                high: 3,
                critical: 4,
                auditors: None,
                coverage: None,
            }
        }
    }
}

pub fn parse_ledger(text: &str) -> anyhow::Result<Vec<LedgerRound>> {
    let mut rounds = Vec::new();
    let mut in_summary = false;
    let mut columns = None;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_summary = line.trim().eq_ignore_ascii_case("## Round Summary");
            columns = None;
            continue;
        }
        if !in_summary || !line.contains('|') {
            continue;
        }
        let cells = split_cells(line);
        if cells.len() <= 4 || is_separator_row(&cells) {
            continue;
        }
        if is_header_row(&cells) {
            columns = SummaryColumns::from_header(&cells);
            continue;
        }
        let columns = if cells.len() >= 9 {
            columns.unwrap_or_else(|| SummaryColumns::for_width(cells.len()))
        } else {
            SummaryColumns::for_width(cells.len())
        };
        let label = cells.first().cloned().unwrap_or_default();
        let high_raw = cells.get(columns.high).cloned().unwrap_or_default();
        let critical_raw = cells.get(columns.critical).cloned().unwrap_or_default();
        if high_raw.is_empty() && critical_raw.is_empty() {
            continue;
        }
        rounds.push(LedgerRound {
            high: parse_count(&high_raw, &label, "high")?,
            critical: parse_count(&critical_raw, &label, "critical")?,
            auditors: optional_cell(&cells, columns.auditors),
            coverage: optional_cell(&cells, columns.coverage),
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

pub fn diagnose(rounds: &[LedgerRound]) -> Option<LedgerDiagnostics> {
    // An empty ledger has no honest value for the deliberately binary terminal dimension.
    let terminal = match rounds.last()?.blockers() {
        0 => Terminal::Zero,
        _ => Terminal::Nonzero,
    };
    let blockers = rounds.iter().map(LedgerRound::blockers).collect::<Vec<_>>();
    let changes = blockers
        .windows(2)
        .map(|pair| pair[1].cmp(&pair[0]))
        .collect::<Vec<_>>();
    let has_improving = changes.contains(&Ordering::Less);
    let has_worsening = changes.contains(&Ordering::Greater);
    let direction = match (has_improving, has_worsening) {
        (true, false) => Direction::Improving,
        (false, true) => Direction::Worsening,
        (true, true) => Direction::Mixed,
        (false, false) => Direction::Flat,
    };
    let mut previous = None;
    let mut turns = 0;
    for change in changes
        .into_iter()
        .filter(|change| *change != Ordering::Equal)
    {
        if previous.is_some_and(|value| value != change) {
            turns += 1;
        }
        previous = Some(change);
    }
    let oscillation = match turns {
        0 => Oscillation::None,
        1 => Oscillation::SingleRebound,
        _ => Oscillation::Alternating,
    };
    let envelope = classify_envelope(&peak_values(&blockers));
    let confidence = if rounds.iter().all(has_lineage) {
        DiagnosticConfidence::AdvisoryLineageRecorded
    } else {
        DiagnosticConfidence::LowNoLineage
    };
    Some(LedgerDiagnostics {
        sample_sufficiency: if rounds.len() < 3 {
            SampleSufficiency::Insufficient
        } else {
            SampleSufficiency::Sufficient
        },
        terminal,
        direction,
        oscillation,
        envelope,
        confidence,
    })
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

fn optional_cell(cells: &[String], index: Option<usize>) -> Option<String> {
    index
        .and_then(|index| cells.get(index))
        .filter(|cell| !cell.is_empty())
        .cloned()
}

fn has_lineage(round: &LedgerRound) -> bool {
    [&round.auditors, &round.coverage].into_iter().all(|value| {
        value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    })
}

fn peak_values(values: &[u64]) -> Vec<u64> {
    let mut deduped = Vec::new();
    for value in values {
        if deduped.last() != Some(value) {
            deduped.push(*value);
        }
    }
    if deduped.len() < 2 {
        return deduped;
    }
    let mut peaks = Vec::new();
    if deduped[0] > deduped[1] {
        peaks.push(deduped[0]);
    }
    for window in deduped.windows(3) {
        if window[1] > window[0] && window[1] > window[2] {
            peaks.push(window[1]);
        }
    }
    let last = deduped.len() - 1;
    if deduped[last] > deduped[last - 1] {
        peaks.push(deduped[last]);
    }
    peaks
}

fn classify_envelope(peaks: &[u64]) -> Envelope {
    if peaks.len() < 2 {
        Envelope::Unknown
    } else if peaks.windows(2).all(|pair| pair[1] < pair[0]) {
        Envelope::Shrinking
    } else if peaks.windows(2).all(|pair| pair[1] > pair[0]) {
        Envelope::Expanding
    } else if peaks.windows(2).all(|pair| pair[1] == pair[0]) {
        Envelope::Flat
    } else {
        Envelope::Unknown
    }
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
#[path = "ledger_tests.rs"]
mod diagnostics_tests;
