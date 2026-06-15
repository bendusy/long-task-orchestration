use crate::agent_job::{ExitState, Usage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct RunnerResult {
    pub reply: String,
    pub usage: Option<Usage>,
    pub exit: ExitState,
}

pub fn parse_pi_ndjson(stdout: &str, exit: ExitState) -> RunnerResult {
    let mut reply = String::new();
    let mut usage = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<PiEvent>(line) else {
            continue;
        };
        match event {
            PiEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::TextDelta { delta },
            } => reply.push_str(&delta),
            PiEvent::MessageEnd { message } => {
                usage = usage.or_else(|| message.usage.map(usage_from_value));
            }
            PiEvent::MessageUpdate { .. } | PiEvent::Other => {}
        }
    }
    RunnerResult { reply, usage, exit }
}

pub fn parse_codex_ndjson(stdout: &str, reply_file: Option<&str>, exit: ExitState) -> RunnerResult {
    let mut usage = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<CodexEvent>(line) else {
            continue;
        };
        if let CodexEvent::TurnCompleted { usage: value } = event {
            usage = Some(usage_from_value(value));
        }
    }
    RunnerResult {
        reply: reply_file.unwrap_or(stdout).to_string(),
        usage,
        exit,
    }
}

pub fn parse_claude_json(stdout: &str, exit: ExitState) -> RunnerResult {
    match serde_json::from_str::<ClaudeResult>(stdout) {
        Ok(parsed) => RunnerResult {
            reply: parsed.result,
            usage: parsed.usage.map(usage_from_value),
            exit,
        },
        Err(_) => RunnerResult {
            reply: stdout.to_string(),
            usage: None,
            exit,
        },
    }
}

pub fn parse_agy_stdout(stdout: &str, exit: ExitState) -> RunnerResult {
    RunnerResult {
        reply: stdout.to_string(),
        usage: None,
        exit,
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PiEvent {
    MessageUpdate {
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: PiAssistantEvent,
    },
    MessageEnd {
        message: PiMessage,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PiAssistantEvent {
    TextDelta {
        delta: String,
    },
    ThinkingDelta {
        #[serde(rename = "delta")]
        _delta: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct PiMessage {
    #[serde(default)]
    usage: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CodexEvent {
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: Value },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ClaudeResult {
    result: String,
    #[serde(default)]
    usage: Option<Value>,
}

fn usage_from_value(value: Value) -> Usage {
    let mut usage = Usage::default();
    if let Some(n) = value.get("input_tokens").and_then(Value::as_u64) {
        usage.tokens_in = Some(n);
    } else if let Some(n) = value.get("input").and_then(Value::as_u64) {
        usage.tokens_in = Some(n);
    }
    if let Some(n) = value.get("output_tokens").and_then(Value::as_u64) {
        usage.tokens_out = Some(n);
    } else if let Some(n) = value.get("output").and_then(Value::as_u64) {
        usage.tokens_out = Some(n);
    }
    if let Some(n) = value.get("totalTokens").and_then(Value::as_u64) {
        usage.tokens = Some(n);
    } else if let (Some(i), Some(o)) = (usage.tokens_in, usage.tokens_out) {
        usage.tokens = Some(i + o);
    }
    if let Some(obj) = value.as_object() {
        usage.extra = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_reply_is_rebuilt_from_text_delta() {
        let result = parse_pi_ndjson(
            include_str!("../fixtures/runner/pi_text_delta.ndjson"),
            ExitState::Ok(0),
        );
        assert_eq!(result.reply, "Hello");
        assert_eq!(result.usage.unwrap().tokens, Some(12));
    }

    #[test]
    fn codex_usage_comes_from_turn_completed_reply_from_file() {
        let result = parse_codex_ndjson(
            include_str!("../fixtures/runner/codex_turn_completed.ndjson"),
            Some("reply body"),
            ExitState::Ok(0),
        );
        assert_eq!(result.reply, "reply body");
        assert_eq!(result.usage.unwrap().tokens_in, Some(97473));
    }

    #[test]
    fn claude_reply_comes_from_result_field() {
        let result = parse_claude_json(
            include_str!("../fixtures/runner/claude_result.json"),
            ExitState::Ok(0),
        );
        assert_eq!(result.reply, "Four.");
        assert_eq!(result.usage.unwrap().tokens_out, Some(3));
    }

    #[test]
    fn claude_schema_drift_falls_back_to_raw_stdout() {
        let result = parse_claude_json("not json", ExitState::Ok(0));
        assert_eq!(result.reply, "not json");
        assert!(result.usage.is_none());
    }
}
