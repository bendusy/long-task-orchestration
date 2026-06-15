use chrono::{DateTime, NaiveDateTime, ParseError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Ok,
    Warn,
    Exceeded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionStatus<T> {
    pub limit: Option<T>,
    pub used: T,
    pub ratio: Option<f64>,
    pub status: BudgetStatus,
}

pub fn dimension_status(limit: Option<f64>, used: f64, warn_ratio: f64) -> DimensionStatus<f64> {
    let Some(limit) = limit else {
        return DimensionStatus {
            limit: None,
            used,
            ratio: None,
            status: BudgetStatus::Ok,
        };
    };
    let ratio = if limit == 0.0 {
        f64::INFINITY
    } else {
        used / limit
    };
    let status = if ratio >= 1.0 {
        BudgetStatus::Exceeded
    } else if ratio >= warn_ratio {
        BudgetStatus::Warn
    } else {
        BudgetStatus::Ok
    };
    DimensionStatus {
        limit: Some(limit),
        used,
        ratio: Some(ratio),
        status,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeadlineStatus {
    pub limit: Option<String>,
    pub used: String,
    pub ratio: Option<f64>,
    pub status: BudgetStatus,
}

pub fn deadline_status(
    deadline: Option<&str>,
    started_at: &str,
    now: &str,
    warn_ratio: f64,
) -> DeadlineStatus {
    deadline_status_with_warnings(deadline, started_at, now, warn_ratio).0
}

fn deadline_status_with_warnings(
    deadline: Option<&str>,
    started_at: &str,
    now: &str,
    warn_ratio: f64,
) -> (DeadlineStatus, Vec<String>) {
    let Some(deadline) = deadline.filter(|s| !s.is_empty()) else {
        return (
            DeadlineStatus {
                limit: None,
                used: now.to_string(),
                ratio: None,
                status: BudgetStatus::Ok,
            },
            Vec::new(),
        );
    };
    let mut warnings = Vec::new();
    let dl = match parse_iso_naive(deadline) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!(
                "budget: invalid hard_deadline {deadline:?} ignored: {err}"
            ));
            return (
                DeadlineStatus {
                    limit: Some(deadline.to_string()),
                    used: now.to_string(),
                    ratio: None,
                    status: BudgetStatus::Ok,
                },
                warnings,
            );
        }
    };
    let nw = match parse_iso_naive(now) {
        Ok(value) => value,
        Err(err) => {
            warnings.push(format!("budget: invalid now {now:?} ignored: {err}"));
            return (
                DeadlineStatus {
                    limit: Some(deadline.to_string()),
                    used: now.to_string(),
                    ratio: None,
                    status: BudgetStatus::Ok,
                },
                warnings,
            );
        }
    };
    if nw >= dl {
        return (
            DeadlineStatus {
                limit: Some(deadline.to_string()),
                used: now.to_string(),
                ratio: Some(1.0),
                status: BudgetStatus::Exceeded,
            },
            warnings,
        );
    }
    let st = if started_at.is_empty() {
        None
    } else {
        match parse_iso_naive(started_at) {
            Ok(value) => Some(value),
            Err(err) => {
                warnings.push(format!(
                    "budget: invalid started_at {started_at:?} ignored: {err}"
                ));
                None
            }
        }
    };
    let Some(st) = st else {
        return (
            DeadlineStatus {
                limit: Some(deadline.to_string()),
                used: now.to_string(),
                ratio: Some(0.0),
                status: BudgetStatus::Ok,
            },
            warnings,
        );
    };
    if dl <= st {
        return (
            DeadlineStatus {
                limit: Some(deadline.to_string()),
                used: now.to_string(),
                ratio: Some(0.0),
                status: BudgetStatus::Ok,
            },
            warnings,
        );
    }
    let ratio = (nw - st).num_seconds() as f64 / (dl - st).num_seconds() as f64;
    (
        DeadlineStatus {
            limit: Some(deadline.to_string()),
            used: now.to_string(),
            ratio: Some(ratio),
            status: if ratio >= warn_ratio {
                BudgetStatus::Warn
            } else {
                BudgetStatus::Ok
            },
        },
        warnings,
    )
}

fn parse_iso_naive(value: &str) -> Result<NaiveDateTime, ParseError> {
    let normalized = value.replace('Z', "+00:00");
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Ok(dt.naive_local());
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RunBudget {
    pub max_turns: Option<u64>,
    pub max_tokens: Option<u64>,
    pub hard_deadline: Option<String>,
    #[serde(default)]
    pub turns_used: u64,
    #[serde(default = "default_warn_ratio")]
    pub warn_ratio: f64,
}

pub fn default_warn_ratio() -> f64 {
    0.8
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetCheck {
    pub overall: BudgetStatus,
    pub dimensions: BTreeMap<String, serde_json::Value>,
    pub warnings: Vec<String>,
}

pub fn check_budget(
    budget: Option<&RunBudget>,
    started_at: &str,
    token_total: u64,
    now_iso: &str,
) -> BudgetCheck {
    let default_budget;
    let budget = match budget {
        Some(b) => b,
        None => {
            default_budget = RunBudget {
                warn_ratio: default_warn_ratio(),
                ..RunBudget::default()
            };
            &default_budget
        }
    };
    let warn_ratio = if budget.warn_ratio == 0.0 {
        default_warn_ratio()
    } else {
        budget.warn_ratio
    };
    let turns = dimension_status(
        budget.max_turns.map(|v| v as f64),
        budget.turns_used as f64,
        warn_ratio,
    );
    let tokens = dimension_status(
        budget.max_tokens.map(|v| v as f64),
        token_total as f64,
        warn_ratio,
    );
    let (deadline, mut deadline_warnings) = deadline_status_with_warnings(
        budget.hard_deadline.as_deref(),
        started_at,
        now_iso,
        warn_ratio,
    );

    let overall = [turns.status, tokens.status, deadline.status]
        .into_iter()
        .max()
        .unwrap_or(BudgetStatus::Ok);

    let mut warnings = Vec::new();
    warnings.append(&mut deadline_warnings);
    push_warning(
        &mut warnings,
        "turns",
        turns.ratio,
        turns.used,
        turns.limit,
        warn_ratio,
    );
    push_warning(
        &mut warnings,
        "tokens",
        tokens.ratio,
        tokens.used,
        tokens.limit,
        warn_ratio,
    );
    if matches!(deadline.status, BudgetStatus::Warn | BudgetStatus::Exceeded)
        && let Some(ratio) = deadline.ratio
    {
        warnings.push(format!(
            "budget: deadline {}% ({}/{})",
            (ratio * 100.0) as u64,
            deadline.used,
            deadline.limit.clone().unwrap_or_default()
        ));
    }

    let dimensions = BTreeMap::from([
        ("turns".to_string(), serde_json::to_value(turns).unwrap()),
        ("tokens".to_string(), serde_json::to_value(tokens).unwrap()),
        (
            "deadline".to_string(),
            serde_json::to_value(deadline).unwrap(),
        ),
    ]);
    BudgetCheck {
        overall,
        dimensions,
        warnings,
    }
}

fn push_warning(
    warnings: &mut Vec<String>,
    name: &str,
    ratio: Option<f64>,
    used: f64,
    limit: Option<f64>,
    warn_ratio: f64,
) {
    if let (Some(ratio), Some(limit)) = (ratio, limit)
        && ratio >= warn_ratio
    {
        warnings.push(format!(
            "budget: {name} {}% ({}/{})",
            (ratio * 100.0) as u64,
            used as u64,
            limit as u64
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_dimension_is_ok() {
        let got = dimension_status(None, 99.0, 0.8);
        assert_eq!(got.status, BudgetStatus::Ok);
        assert!(got.ratio.is_none());
    }

    #[test]
    fn deadline_drops_timezone_like_python_contract() {
        let got = deadline_status(
            Some("2026-06-15T10:00:00+08:00"),
            "2026-06-15T00:00:00+08:00",
            "2026-06-15T09:00:00+08:00",
            0.8,
        );
        assert_eq!(got.status, BudgetStatus::Warn);
        assert_eq!(got.ratio.unwrap(), 0.9);
    }

    #[test]
    fn overall_takes_strictest_dimension() {
        let budget = RunBudget {
            max_turns: Some(10),
            max_tokens: Some(1_000_000),
            hard_deadline: Some("2026-06-15T10:00:00".to_string()),
            turns_used: 9,
            warn_ratio: 0.8,
        };
        let got = check_budget(
            Some(&budget),
            "2026-06-15T00:00:00",
            100,
            "2026-06-15T11:00:00",
        );
        assert_eq!(got.overall, BudgetStatus::Exceeded);
    }

    #[test]
    fn invalid_deadline_is_warning_not_panic() {
        let budget = RunBudget {
            hard_deadline: Some("not-a-date".to_string()),
            ..RunBudget::default()
        };
        let got = check_budget(
            Some(&budget),
            "2026-06-15T00:00:00",
            0,
            "2026-06-15T01:00:00",
        );
        assert_eq!(got.overall, BudgetStatus::Ok);
        assert!(
            got.warnings
                .iter()
                .any(|w| w.contains("invalid hard_deadline"))
        );
    }

    #[test]
    fn invalid_started_at_is_warning_not_panic() {
        let budget = RunBudget {
            hard_deadline: Some("2026-06-15T10:00:00".to_string()),
            ..RunBudget::default()
        };
        let got = check_budget(Some(&budget), "bad-start", 0, "2026-06-15T01:00:00");
        assert_eq!(got.overall, BudgetStatus::Ok);
        assert!(
            got.warnings
                .iter()
                .any(|w| w.contains("invalid started_at"))
        );
    }
}
