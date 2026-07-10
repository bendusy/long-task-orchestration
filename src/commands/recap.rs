use crate::budget;
use crate::commands::util;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct RecapOptions {
    pub run_id: Option<String>,
    pub artifacts: bool,
    pub mine: bool,
}

pub fn cmd_recap(repo: &Path, options: RecapOptions) -> anyhow::Result<()> {
    if options.mine {
        return recap_mine(repo);
    }
    let ctx = util::load_run(repo, options.run_id.as_deref())?;
    println!(
        "{}",
        render_recap(&ctx.state, &ctx.run_id, repo, options.artifacts, 120.0)
    );
    Ok(())
}

pub fn render_recap(
    state: &crate::state::LtoState,
    run_id: &str,
    repo: &Path,
    include_artifacts: bool,
    live_window_sec: f64,
) -> String {
    let goal = nonempty(&state.goal, "(未记录目标)");
    let why = if !state.why.trim().is_empty() {
        state.why.as_str()
    } else {
        state.original_user_request.as_str()
    };
    let done_when = state.done_when.as_str();
    let phase = nonempty(&state.current_phase, "?");
    let tasks = util::json_array(&state.tasks);
    let next_action = state.next_action.as_str().unwrap_or_default();
    let blocked_by = state.blocked_by.as_str().unwrap_or("none");

    let mut lines = vec![
        "╭─ LTO Recap ─ 给人看的回顾（不是给 AI 看的状态）".to_string(),
        "│".to_string(),
        format!("│ 你当初要做什么 ── {goal}"),
    ];

    if !why.trim().is_empty() && why != state.goal {
        lines.push(format!("│ 为什么要做 ────── {}", util::single_line(why)));
    } else {
        lines.push("│ 为什么要做 ────── （未记录 — 下次 lto start 加 --why 补上）".to_string());
    }

    let mut duration = format!(
        "│ 跑了多久 ──────── {}",
        util::elapsed_human(&state.started_at)
    );
    let gap = session_gap_human(state);
    if !gap.is_empty() {
        duration.push('，');
        duration.push_str(&gap);
    }
    lines.push(duration);

    let done = tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("done"))
        .cloned()
        .collect::<Vec<_>>();
    let pending = tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("pending"))
        .cloned()
        .collect::<Vec<_>>();
    let blocked = tasks
        .iter()
        .filter(|task| task.get("status").and_then(Value::as_str) == Some("blocked"))
        .cloned()
        .collect::<Vec<_>>();

    lines.push(format!(
        "│ 已经做到哪 ────── {}",
        done_summary(&done, state)
    ));
    lines.push(format!(
        "│ 还剩什么 ──────── {}",
        remaining_summary(&pending, &blocked, done_when)
    ));

    let token_line = token_summary(state);
    if !token_line.is_empty() {
        lines.push(format!("│ 花了多少 token ── {token_line}"));
    }

    let rollup = util::token_rollup(state);
    let budget = budget::check_budget(
        Some(&state.budget),
        &state.started_at,
        rollup.total_tokens,
        &util::iso_now(),
    );
    for warning in budget.warnings {
        lines.push(format!("│ {warning}"));
    }

    lines.push(format!(
        "│ 现在轮到你 ────── {}",
        next_for_human(state, &blocked, next_action, blocked_by)
    ));

    if include_artifacts {
        lines.push(format!(
            "│ 关键产物 ──────── {}",
            artifact_summary(repo, run_id)
        ));
    }

    let running = running_jobs(repo, run_id, live_window_sec);
    if !running.is_empty() {
        lines.push(format!("│ 当前在跑 ──────── {}", running.join("；")));
    }

    lines.push("│".to_string());
    lines.push(format!("╰─ run: {run_id}  phase: {phase}"));
    lines.join("\n")
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn session_gap_human(state: &crate::state::LtoState) -> String {
    let hours = util::max_session_gap_hours(state);
    if hours >= 24.0 {
        format!(
            "中间最长停了 {} 小时（约 {} 天）",
            hours as u64,
            hours as u64 / 24
        )
    } else if hours >= 2.0 {
        format!("中间停过 {} 小时", hours as u64)
    } else {
        String::new()
    }
}

fn token_summary(state: &crate::state::LtoState) -> String {
    let rollup = util::token_rollup(state);
    if rollup.runs_total == 0 {
        return String::new();
    }
    if rollup.total_tokens == 0 {
        return format!(
            "未计量（{} 次派工，无 runner 上报 token；agy 等 CLI 不暴露用量）",
            rollup.runs_total
        );
    }
    let mut parts = rollup
        .by_runner
        .iter()
        .filter(|(_, slot)| slot.tokens > 0)
        .map(|(runner, slot)| (runner, slot.tokens))
        .collect::<Vec<_>>();
    parts.sort_by_key(|part| std::cmp::Reverse(part.1));
    let by = parts
        .into_iter()
        .map(|(runner, tokens)| format!("{runner} {}", util::format_tokens(tokens)))
        .collect::<Vec<_>>()
        .join("，");
    let coverage = if rollup.runs_with_tokens == rollup.runs_total {
        String::new()
    } else {
        format!(
            "（{}/{} 次派工有计量）",
            rollup.runs_with_tokens, rollup.runs_total
        )
    };
    let elapsed = if rollup.total_elapsed_sec > 0.0 {
        format!(
            " · 派工累计 {}",
            util::format_duration(rollup.total_elapsed_sec)
        )
    } else {
        String::new()
    };
    format!(
        "约 {} tokens{}：{}{}",
        util::format_tokens(rollup.total_tokens),
        coverage,
        by,
        elapsed
    )
}

fn done_summary(done: &[Value], state: &crate::state::LtoState) -> String {
    if done.is_empty() {
        let phases = util::json_array(&state.phase_transitions)
            .iter()
            .filter_map(|transition| transition.get("to").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if phases.is_empty() {
            "还没有完成的任务".to_string()
        } else {
            format!("走过阶段：{}", phases.join(" → "))
        }
    } else {
        let titles = done
            .iter()
            .map(|task| {
                task.get("title")
                    .or_else(|| task.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            })
            .collect::<Vec<_>>();
        let head = titles
            .iter()
            .take(4)
            .copied()
            .collect::<Vec<_>>()
            .join("、");
        let more = if titles.len() > 4 {
            format!(" 等 {} 项", titles.len())
        } else {
            String::new()
        };
        format!("已完成 {} 项：{head}{more}", done.len())
    }
}

fn remaining_summary(pending: &[Value], blocked: &[Value], done_when: &str) -> String {
    let mut parts = Vec::new();
    if let Some(first) = blocked.first() {
        let reason = first
            .get("blockers")
            .and_then(Value::as_array)
            .and_then(|items| items.last())
            .and_then(|blocker| blocker.get("reason"))
            .and_then(Value::as_str)
            .map(util::single_line)
            .unwrap_or_default();
        if reason.is_empty() {
            parts.push(format!("{} 项卡住", blocked.len()));
        } else {
            parts.push(format!(
                "{} 项卡住（卡在：{}）",
                blocked.len(),
                truncate(&reason, 40)
            ));
        }
    }
    if !pending.is_empty() {
        parts.push(format!("{} 项待做", pending.len()));
    }
    if parts.is_empty() {
        if done_when.trim().is_empty() {
            "看起来任务都完成了".to_string()
        } else {
            format!("看起来都做完了。验收标准：{}", util::single_line(done_when))
        }
    } else {
        let tail = if done_when.trim().is_empty() {
            String::new()
        } else {
            format!("。算做完的标准：{}", util::single_line(done_when))
        };
        format!("{}{}", parts.join("；"), tail)
    }
}

fn next_for_human(
    state: &crate::state::LtoState,
    blocked: &[Value],
    next_action: &str,
    blocked_by: &str,
) -> String {
    if state.current_phase == "closed" {
        return "这个任务已经收尾（closed）。可以开新的了。".to_string();
    }
    if !blocked_by.trim().is_empty() && blocked_by != "none" {
        return format!("需要你处理：{}", util::single_line(blocked_by));
    }
    if !blocked.is_empty() {
        return format!(
            "决定怎么处理那 {} 个卡住的任务（修、跳过、还是换思路）",
            blocked.len()
        );
    }
    if !next_action.trim().is_empty() {
        return util::single_line(next_action);
    }
    "跑 `lto next` 看系统建议的下一步，或继续推进待做项".to_string()
}

fn artifact_summary(repo: &Path, run_id: &str) -> String {
    let entries = util::latest_artifacts(repo, run_id, 5);
    if entries.is_empty() {
        return "未发现已登记产物".to_string();
    }
    entries
        .iter()
        .map(|entry| {
            let marker = if entry.get("source").and_then(Value::as_str) == Some("synthesized") {
                "*"
            } else {
                ""
            };
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("other");
            let path = entry
                .get("run_relative_path")
                .or_else(|| entry.get("relative_path"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            format!("{kind}:{path}{marker}")
        })
        .collect::<Vec<_>>()
        .join("；")
}

fn running_jobs(repo: &Path, run_id: &str, window_sec: f64) -> Vec<String> {
    let live_dir = repo.join(".lto").join(run_id).join("live");
    let Ok(entries) = fs::read_dir(live_dir) else {
        return Vec::new();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default();
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("log") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(modified_secs) = modified.duration_since(UNIX_EPOCH) else {
            continue;
        };
        let age = now - modified_secs.as_secs_f64();
        if age <= window_sec {
            let job_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("?");
            let label = if age < 1.0 {
                "刚有输出".to_string()
            } else {
                format!("{}秒前有输出", age as u64)
            };
            out.push(format!("{job_id}（{label}）"));
        }
    }
    out.sort();
    out
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub fn recap_mine(repo: &Path) -> anyhow::Result<()> {
    let mining = crate::telemetry::cross_run_mining(repo)?;
    println!("{}", render_mining_brief(&mining));
    Ok(())
}

fn render_mining_brief(mining: &crate::telemetry::CrossRunMining) -> String {
    let mut lines = vec![
        "=== LTO Cross-Run Tuning Brief ===".to_string(),
        "只读分析：不写配置、不改 runner 优先级、不自动 route/promote。".to_string(),
        format!("runs_scanned: {}", mining.run_count),
        String::new(),
    ];
    if mining.entries.is_empty() {
        lines.push("未发现可挖掘的 runner.finished 或 agent.dispatch.completed 事件。".to_string());
        return lines.join("\n");
    }
    lines.push("| Runner | Model | 任务类型 | 时间窗 | distinct runs | 失败率 | 平均耗时 | 平均 retry | 平均 audit 轮次 | dispatch.completed | 评估类型 |".to_string());
    lines.push(
        "| :--- | :--- | :--- | :--- | ---: | ---: | ---: | ---: | ---: | ---: | :--- |"
            .to_string(),
    );
    for entry in &mining.entries {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {:.1}% | {} | {} | {} | {} | {} |",
            entry.runner,
            entry.model,
            entry.task_type,
            entry.time_window,
            entry.distinct_runs,
            failure_rate(entry) * 100.0,
            format_opt_seconds(entry.avg_elapsed_sec),
            format_opt_float(entry.avg_retry),
            format_opt_float(entry.avg_audit_rounds),
            entry.agent_dispatch_completed,
            if entry.subjective_non_measurement {
                "主观非测量"
            } else {
                "客观测量"
            }
        ));
    }
    lines.push(String::new());
    lines.push("=== 派生信号 ===".to_string());
    let mut emitted = false;
    for entry in &mining.entries {
        let rate = failure_rate(entry);
        if rate >= 0.3 && entry.distinct_runs >= 3 {
            emitted = true;
            lines.push(format!(
                "WARN {} ({}) 在 {} 类任务 failure_rate={:.1}% over {} distinct runs。",
                entry.runner,
                entry.model,
                entry.task_type,
                rate * 100.0,
                entry.distinct_runs
            ));
        }
        if entry.avg_audit_rounds.unwrap_or(0.0) >= 3.0 {
            emitted = true;
            lines.push(format!(
                "WARN {} ({}) 关联 runs 平均 audit 收敛轮次 {:.1}。",
                entry.runner,
                entry.model,
                entry.avg_audit_rounds.unwrap_or(0.0)
            ));
        }
    }
    if !emitted {
        lines.push("未发现达到阈值的高失败率或审计反复翻车信号。".to_string());
    }
    lines.join("\n")
}

fn failure_rate(entry: &crate::telemetry::CrossRunMiningEntry) -> f64 {
    if entry.distinct_runs == 0 {
        0.0
    } else {
        entry.failed as f64 / entry.distinct_runs as f64
    }
}

fn format_opt_seconds(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}s"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_opt_float(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LtoState;
    use serde_json::json;

    fn base_state() -> LtoState {
        LtoState {
            run_id: "r1".to_string(),
            goal: "ship rust switch".to_string(),
            why: "reduce Python wrapper risk".to_string(),
            done_when: "cargo test passes".to_string(),
            current_phase: "implementation".to_string(),
            next_action: json!("run verification"),
            blocked_by: json!("none"),
            tasks: json!([
                {"id": "T1", "title": "plugin mount", "status": "done"},
                {"id": "T2", "title": "docs", "status": "pending"}
            ]),
            ..LtoState::default()
        }
    }

    #[test]
    fn render_recap_main_path_includes_human_questions() {
        let tmp = tempfile::tempdir().unwrap();
        let out = render_recap(&base_state(), "r1", tmp.path(), false, 120.0);
        assert!(out.contains("你当初要做什么"));
        assert!(out.contains("ship rust switch"));
        assert!(out.contains("为什么要做"));
        assert!(out.contains("reduce Python wrapper risk"));
        assert!(out.contains("已经做到哪"));
        assert!(out.contains("还剩什么"));
        assert!(out.contains("现在轮到你"));
        assert!(out.contains("run: r1  phase: implementation"));
    }

    #[test]
    fn token_summary_reports_metered_and_unmetered_runs() {
        let mut state = base_state();
        state.agent_runs = json!({
            "j1": [{
                "job_id": "j1",
                "runner": "codex",
                "status": "ok",
                "cost": {"tokens_in": 1000, "tokens_out": 500, "elapsed_sec": 90.0}
            }],
            "j2": [{
                "job_id": "j2",
                "runner": "agy",
                "status": "ok",
                "cost": {}
            }]
        });
        let summary = token_summary(&state);
        assert!(summary.contains("约 1.5k tokens"));
        assert!(summary.contains("1/2 次派工有计量"));
        assert!(summary.contains("codex 1.5k"));
        assert!(summary.contains("派工累计 1m30s"));

        state.agent_runs = json!({"j3": [{"job_id": "j3", "runner": "agy", "status": "ok"}]});
        assert!(token_summary(&state).contains("未计量"));
    }

    #[test]
    fn render_mining_brief_is_readonly_and_includes_completed_counts() {
        let brief = render_mining_brief(&crate::telemetry::CrossRunMining {
            run_count: 2,
            entries: vec![crate::telemetry::CrossRunMiningEntry {
                runner: "pi".to_string(),
                model: "deepseek-v4-pro".to_string(),
                task_type: "implementation".to_string(),
                time_window: "2026-06-17".to_string(),
                dispatches: 2,
                ok: 1,
                failed: 1,
                timeout: 0,
                rate_limited: 0,
                skipped: 0,
                avg_elapsed_sec: Some(12.0),
                avg_retry: Some(0.5),
                avg_audit_rounds: Some(1.0),
                agent_dispatch_completed: 2,
                distinct_runs: 2,
                subjective_non_measurement: false,
            }],
        });

        assert!(brief.contains("只读分析：不写配置、不改 runner 优先级、不自动 route/promote。"));
        assert!(brief.contains(
            "| pi | deepseek-v4-pro | implementation | 2026-06-17 | 2 | 50.0% | 12.0s | 0.5 | 1.0 | 2 | 客观测量 |"
        ));
    }

    #[test]
    fn done_and_remaining_summaries_cover_empty_blocked_and_done_branches() {
        let state = LtoState {
            phase_transitions: json!([
                {"from": "intake", "to": "spec"},
                {"from": "spec", "to": "implementation"}
            ]),
            ..LtoState::default()
        };
        assert!(done_summary(&[], &state).contains("走过阶段：spec → implementation"));

        let done = json!([
            {"id": "T1", "title": "one"},
            {"id": "T2", "title": "two"},
            {"id": "T3", "title": "three"},
            {"id": "T4", "title": "four"},
            {"id": "T5", "title": "five"}
        ]);
        assert!(done_summary(done.as_array().unwrap(), &state).contains("已完成 5 项"));

        let pending = json!([{"id": "P1"}]);
        let blocked = json!([{"id": "B1", "blockers": [{"reason": "waiting on CI logs"}]}]);
        let remaining = remaining_summary(
            pending.as_array().unwrap(),
            blocked.as_array().unwrap(),
            "green CI",
        );
        assert!(remaining.contains("1 项卡住"));
        assert!(remaining.contains("1 项待做"));
        assert!(remaining.contains("green CI"));
        assert!(remaining_summary(&[], &[], "green CI").contains("看起来都做完了"));
    }
}
