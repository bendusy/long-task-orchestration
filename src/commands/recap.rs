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
}

pub fn cmd_recap(repo: &Path, options: RecapOptions) -> anyhow::Result<()> {
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
