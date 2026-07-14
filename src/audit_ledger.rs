use crate::ledger::{self, LedgerDiagnostics, LedgerVerdict};
use std::fs;
use std::path::Path;

pub struct AppendInput<'a> {
    pub artifact: &'a str,
    pub auditors: &'a [String],
    pub coverage: &'a str,
    pub high: u64,
    pub critical: u64,
    pub minor: u64,
}

pub struct AppendOutcome {
    pub label: String,
    pub verdict: LedgerVerdict,
    pub diagnostics: LedgerDiagnostics,
}

pub fn append(path: &Path, input: AppendInput<'_>) -> anyhow::Result<AppendOutcome> {
    ensure_exists(path)?;
    let content = upgrade_schema(&fs::read_to_string(path)?);
    let label = next_round_label(&content);
    let row = format!(
        "| {label} | {} | {} | {} | {} | {} | {} | {} | open |",
        clean_cell(input.artifact),
        clean_cell(&input.auditors.join(" ")),
        clean_cell(input.coverage),
        input.high,
        input.critical,
        input.minor,
        if label == "R1" { "start" } else { "flat" },
    );
    let placeholder = "| R1 |  |  |  |  |  |  | start | open |";
    let updated = if label == "R1" && content.contains(placeholder) {
        content.replacen(placeholder, &row, 1)
    } else {
        insert_round_row(&content, &row)
    };
    let rounds = ledger::parse_ledger(&updated)?;
    let diagnostics = ledger::diagnose(&rounds)
        .ok_or_else(|| anyhow::anyhow!("appended audit ledger has no observation"))?;
    let verdict = ledger::evaluate_ledger(&rounds, false);
    fs::write(path, updated)?;
    Ok(AppendOutcome {
        label,
        verdict,
        diagnostics,
    })
}

fn ensure_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, include_str!("../templates/audit-ledger.md"))?;
    Ok(())
}

fn upgrade_schema(content: &str) -> String {
    let content = content
        .replace(
            "| round | artifact | auditors | high | critical | minor | trend | status |",
            "| round | artifact | auditors | coverage | high | critical | minor | trend | status |",
        )
        .replace(
            "|---|---|---|---:|---:|---:|---|---|",
            "|---|---|---|---|---:|---:|---:|---|---|",
        )
        .replace(
            "| R1 |  |  |  |  |  | start | open |",
            "| R1 |  |  |  |  |  |  | start | open |",
        );
    let mut in_summary = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        if line.starts_with("## ") {
            in_summary = line.trim().eq_ignore_ascii_case("## Round Summary");
        }
        if in_summary {
            let mut cells = split_row(line);
            let is_round = cells.first().is_some_and(|cell| {
                cell.strip_prefix('R')
                    .is_some_and(|n| n.parse::<u64>().is_ok())
            });
            if is_round && cells.len() == 8 {
                cells.insert(3, String::new());
                lines.push(format!("| {} |", cells.join(" | ")));
                continue;
            }
        }
        lines.push(line.to_string());
    }
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn next_round_label(content: &str) -> String {
    let max_round = content
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("| R"))
        .filter_map(|tail| tail.split('|').next()?.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    if content.contains("| R1 |  |  |  |  |  |  | start | open |") || max_round == 0 {
        "R1".to_string()
    } else {
        format!("R{}", max_round + 1)
    }
}

fn insert_round_row(content: &str, row: &str) -> String {
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let index = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("| R"))
        .map(|index| index + 1)
        .unwrap_or(lines.len());
    lines.insert(index, row.to_string());
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

fn split_row(line: &str) -> Vec<String> {
    if !line.trim_start().starts_with('|') {
        return Vec::new();
    }
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn clean_cell(value: &str) -> String {
    value
        .replace('|', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_creates_r1_with_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("audit-ledger.md");
        let outcome = append(
            &path,
            AppendInput {
                artifact: "replies",
                auditors: &["agy".into(), "pi".into()],
                coverage: "T1,T2",
                high: 1,
                critical: 0,
                minor: 2,
            },
        )
        .unwrap();
        assert_eq!(outcome.label, "R1");
        assert_eq!(outcome.verdict, LedgerVerdict::Converging);
        assert_eq!(
            outcome.diagnostics.confidence.as_str(),
            "advisory (lineage recorded)"
        );
    }

    #[test]
    fn append_upgrades_legacy_header_and_round_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("audit-ledger.md");
        fs::write(
            &path,
            "## Round Summary\n| round | artifact | auditors | high | critical | minor | trend | status |\n|---|---|---|---:|---:|---:|---|---|\n| R1 | old | pi | 2 | 1 | 0 | start | open |\n",
        )
        .unwrap();
        append(
            &path,
            AppendInput {
                artifact: "new",
                auditors: &["agy".into()],
                coverage: "T2",
                high: 1,
                critical: 0,
                minor: 0,
            },
        )
        .unwrap();
        let text = fs::read_to_string(path).unwrap();
        let rows = text
            .lines()
            .filter(|line| line.starts_with("| R"))
            .collect::<Vec<_>>();
        assert!(rows.iter().all(|row| split_row(row).len() == 9));
        let rounds = ledger::parse_ledger(&text).unwrap();
        assert_eq!(rounds[0].blockers(), 3);
        assert_eq!(rounds[1].blockers(), 1);
    }
}
