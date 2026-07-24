//! Read-only `task` resource projection.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Stable JSON envelope schema version for get/describe output.
pub const SCHEMA_VERSION: u64 = 1;

/// Partial, tolerant view over one task object in `state.tasks`.
///
/// Known fields are projected; anything else is retained in `extra`.
/// Parse failures do not panic: callers receive a degraded view plus a
/// schema warning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub depends_on: Vec<Value>,
    #[serde(default)]
    pub evidence: Vec<Value>,
    #[serde(default)]
    pub blockers: Vec<Value>,
    #[serde(default)]
    pub last_update: String,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
    /// True when the source element could not be fully projected.
    #[serde(skip)]
    pub degraded: bool,
    /// Original JSON kept when degraded (or always available for describe).
    #[serde(skip)]
    pub raw: Option<Value>,
}

/// One schema warning attached to a list or describe result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaWarning {
    pub index: Option<usize>,
    pub id: Option<String>,
    pub message: String,
}

/// Result of projecting + filtering the tasks array.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskList {
    pub items: Vec<TaskView>,
    pub schema_warnings: Vec<SchemaWarning>,
}

/// Filters applied by `lto get task` (exact match, whitelist only).
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<String>,
    pub phase: Option<String>,
}

/// Project raw `state.tasks` JSON into TaskView items with optional filters.
///
/// Non-array `tasks` is treated as empty. Individual element failures become
/// degraded entries (never dropped) and produce schema warnings.
pub fn project_tasks(tasks: &Value, filter: &TaskFilter) -> TaskList {
    let array = match tasks.as_array() {
        Some(items) => items.as_slice(),
        None => {
            if tasks.is_null() {
                return TaskList {
                    items: Vec::new(),
                    schema_warnings: Vec::new(),
                };
            }
            return TaskList {
                items: Vec::new(),
                schema_warnings: vec![SchemaWarning {
                    index: None,
                    id: None,
                    message: format!(
                        "tasks is not an array (got {}); treating as empty",
                        type_name(tasks)
                    ),
                }],
            };
        }
    };

    let mut items = Vec::new();
    let mut schema_warnings = Vec::new();

    for (index, raw) in array.iter().enumerate() {
        let (view, warning) = project_one(index, raw);
        if let Some(warning) = warning {
            schema_warnings.push(warning);
        }
        if matches_filter(&view, filter) {
            items.push(view);
        }
    }

    TaskList {
        items,
        schema_warnings,
    }
}

/// Look up one task by id. Returns the view and any schema warning for that
/// element. `Ok(None)` means the id is absent.
pub fn find_task(tasks: &Value, task_id: &str) -> (Option<TaskView>, Vec<SchemaWarning>) {
    let array = match tasks.as_array() {
        Some(items) => items.as_slice(),
        None => {
            return (
                None,
                if tasks.is_null() {
                    Vec::new()
                } else {
                    vec![SchemaWarning {
                        index: None,
                        id: None,
                        message: format!(
                            "tasks is not an array (got {}); treating as empty",
                            type_name(tasks)
                        ),
                    }]
                },
            );
        }
    };

    let mut warnings = Vec::new();
    for (index, raw) in array.iter().enumerate() {
        let (view, warning) = project_one(index, raw);
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        if view.id == task_id {
            // Only return warnings that relate to this element (or global).
            let related = warnings
                .into_iter()
                .filter(|w| w.index == Some(index) || w.index.is_none())
                .collect();
            return (Some(view), related);
        }
    }
    (None, warnings)
}

/// Available task ids (best-effort) for error messages — first `limit` entries.
pub fn available_ids(tasks: &Value, limit: usize) -> Vec<String> {
    let Some(array) = tasks.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|raw| {
            raw.get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    // Fall back to stringifying a non-string id for the hint.
                    raw.get("id").map(|v| v.to_string())
                })
        })
        .take(limit)
        .collect()
}

/// JSON envelope for `get` list output.
pub fn list_envelope(list: &TaskList) -> Value {
    let items: Vec<Value> = list.items.iter().map(view_to_json).collect();
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "resource": "task",
        "items": items,
        "schema_warnings": list.schema_warnings,
    })
}

/// JSON envelope for `describe` single-object output.
pub fn describe_envelope(view: &TaskView, warnings: &[SchemaWarning]) -> Value {
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "resource": "task",
        "items": [view_to_json(view)],
        "schema_warnings": warnings,
    })
}

/// Compact human table for `get task`.
pub fn render_list_human(list: &TaskList) -> String {
    let mut lines = Vec::new();
    if list.items.is_empty() {
        lines.push("No tasks matched.".to_string());
    } else {
        lines.push(format!(
            "{:<20} {:<12} {:<14} {}",
            "ID", "STATUS", "PHASE", "TITLE"
        ));
        lines.push(format!(
            "{:<20} {:<12} {:<14} {}",
            "----", "------", "-----", "-----"
        ));
        for item in &list.items {
            let id = truncate(&item.id, 20);
            let status = if item.degraded && item.status.is_empty() {
                "?".to_string()
            } else {
                truncate(&item.status, 12)
            };
            let phase = truncate(&item.phase, 14);
            let title = if item.title.is_empty() && item.degraded {
                "(degraded)".to_string()
            } else {
                item.title.clone()
            };
            lines.push(format!("{id:<20} {status:<12} {phase:<14} {title}"));
        }
    }
    if !list.schema_warnings.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "schema_warnings: {} (see --json for details)",
            list.schema_warnings.len()
        ));
        for w in list.schema_warnings.iter().take(3) {
            let where_ = match (&w.id, w.index) {
                (Some(id), Some(i)) => format!("[{i}] id={id}"),
                (Some(id), None) => format!("id={id}"),
                (None, Some(i)) => format!("[{i}]"),
                (None, None) => "tasks".to_string(),
            };
            lines.push(format!("  - {where_}: {}", w.message));
        }
        if list.schema_warnings.len() > 3 {
            lines.push(format!("  … and {} more", list.schema_warnings.len() - 3));
        }
    }
    lines.join("\n")
}

/// Segmented human output for `describe task`.
pub fn render_describe_human(view: &TaskView, warnings: &[SchemaWarning]) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Task: {}", nonempty(&view.id, "(missing id)")));
    if view.degraded {
        lines.push("  (degraded projection — some fields may be incomplete)".to_string());
    }
    lines.push(String::new());
    lines.push("Fields".to_string());
    lines.push(format!("  status:      {}", nonempty(&view.status, "-")));
    lines.push(format!("  phase:       {}", nonempty(&view.phase, "-")));
    lines.push(format!("  title:       {}", nonempty(&view.title, "-")));
    lines.push(format!(
        "  last_update: {}",
        nonempty(&view.last_update, "-")
    ));
    if !view.depends_on.is_empty() {
        let deps: Vec<String> = view
            .depends_on
            .iter()
            .map(|d| match d {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        lines.push(format!("  depends_on:  {}", deps.join(", ")));
    } else {
        lines.push("  depends_on:  (none)".to_string());
    }

    lines.push(String::new());
    lines.push(format!("Evidence ({})", view.evidence.len()));
    if view.evidence.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (i, ev) in view.evidence.iter().enumerate() {
            lines.push(format!("  [{}] {}", i, evidence_summary(ev)));
        }
    }

    lines.push(String::new());
    lines.push(format!("Blockers ({})", view.blockers.len()));
    if view.blockers.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (i, b) in view.blockers.iter().enumerate() {
            lines.push(format!("  [{}] {}", i, blocker_summary(b)));
        }
    }

    // Surface a few well-known extra keys as compact notes (not full dump).
    let notable = ["planned_command", "instrument_ref", "retry_count"];
    let mut extras: Vec<String> = Vec::new();
    for key in notable {
        if let Some(val) = view.extra.get(key) {
            extras.push(format!("{key}={}", compact_value(val)));
        }
    }
    if !extras.is_empty() {
        lines.push(String::new());
        lines.push(format!("Notes: {}", extras.join("; ")));
    }

    if !warnings.is_empty() {
        lines.push(String::new());
        lines.push(format!("schema_warnings: {}", warnings.len()));
        for w in warnings {
            lines.push(format!("  - {}", w.message));
        }
    }

    lines.join("\n")
}

fn project_one(index: usize, raw: &Value) -> (TaskView, Option<SchemaWarning>) {
    if !raw.is_object() {
        let degraded = TaskView {
            id: format!("<index:{index}>"),
            title: String::new(),
            status: String::new(),
            phase: String::new(),
            depends_on: Vec::new(),
            evidence: Vec::new(),
            blockers: Vec::new(),
            last_update: String::new(),
            extra: Map::new(),
            degraded: true,
            raw: Some(raw.clone()),
        };
        return (
            degraded,
            Some(SchemaWarning {
                index: Some(index),
                id: None,
                message: format!("element is not an object (got {})", type_name(raw)),
            }),
        );
    }

    match serde_json::from_value::<TaskView>(raw.clone()) {
        Ok(mut view) => {
            view.raw = Some(raw.clone());
            let mut messages = Vec::new();
            if view.id.is_empty() {
                // Prefer a synthetic id so the row is still addressable in lists.
                if let Some(Value::String(s)) = raw.get("id") {
                    // empty string id
                    let _ = s;
                }
                messages.push("missing or empty id".to_string());
                view.id = format!("<index:{index}>");
                view.degraded = true;
            }
            // status non-string is coerced to empty by serde default / type error path.
            // Detect via raw if present but wrong type.
            if let Some(status_val) = raw.get("status")
                && !status_val.is_string()
                && !status_val.is_null()
            {
                messages.push(format!(
                    "status is not a string (got {})",
                    type_name(status_val)
                ));
                view.degraded = true;
            }
            let warning = if messages.is_empty() {
                None
            } else {
                Some(SchemaWarning {
                    index: Some(index),
                    id: Some(view.id.clone()).filter(|s| !s.starts_with('<')),
                    message: messages.join("; "),
                })
            };
            // Re-extract known fields carefully if serde put wrong-typed values into extra
            // via flatten — already handled by typed fields with defaults.
            (view, warning)
        }
        Err(err) => {
            // Hard failure: build a degraded view from raw keys we can salvage.
            let id = raw
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("<index:{index}>"));
            let title = raw
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let status = raw
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let phase = raw
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let degraded = TaskView {
                id: id.clone(),
                title,
                status,
                phase,
                depends_on: Vec::new(),
                evidence: Vec::new(),
                blockers: Vec::new(),
                last_update: String::new(),
                extra: raw.as_object().cloned().unwrap_or_default(),
                degraded: true,
                raw: Some(raw.clone()),
            };
            (
                degraded,
                Some(SchemaWarning {
                    index: Some(index),
                    id: Some(id).filter(|s| !s.starts_with('<')),
                    message: format!("failed to project task: {err}"),
                }),
            )
        }
    }
}

fn matches_filter(view: &TaskView, filter: &TaskFilter) -> bool {
    if let Some(status) = &filter.status
        && view.status != *status
    {
        return false;
    }
    if let Some(phase) = &filter.phase
        && view.phase != *phase
    {
        return false;
    }
    true
}

fn view_to_json(view: &TaskView) -> Value {
    // Serialize without the skip fields, then attach a degraded marker if needed.
    let mut value = serde_json::to_value(view).unwrap_or_else(|_| {
        serde_json::json!({
            "id": view.id,
            "title": view.title,
            "status": view.status,
            "phase": view.phase,
        })
    });
    if view.degraded
        && let Some(obj) = value.as_object_mut()
    {
        obj.insert("degraded".to_string(), Value::Bool(true));
        if let Some(raw) = &view.raw {
            obj.insert("raw".to_string(), raw.clone());
        }
    }
    value
}

fn evidence_summary(ev: &Value) -> String {
    match ev {
        Value::Object(map) => {
            let kind = map.get("kind").and_then(Value::as_str).unwrap_or("?");
            let summary = map
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| map.get("note").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            let path = map
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| map.get("artifact").and_then(Value::as_str));
            let recorded = map
                .get("recorded_at")
                .and_then(Value::as_str)
                .or_else(|| map.get("at").and_then(Value::as_str));
            let mut parts = vec![format!("kind={kind}")];
            if !summary.is_empty() {
                parts.push(format!("summary={}", truncate(&summary, 80)));
            }
            if let Some(path) = path {
                parts.push(format!("path={path}"));
            }
            if let Some(recorded) = recorded {
                parts.push(format!("at={recorded}"));
            }
            parts.join(" ")
        }
        Value::String(s) => truncate(s, 80),
        other => compact_value(other),
    }
}

fn blocker_summary(b: &Value) -> String {
    match b {
        Value::Object(map) => {
            if let Some(reason) = map.get("reason").and_then(Value::as_str) {
                return truncate(reason, 100);
            }
            if let Some(summary) = map.get("summary").and_then(Value::as_str) {
                return truncate(summary, 100);
            }
            compact_value(b)
        }
        Value::String(s) => truncate(s, 100),
        other => compact_value(other),
    }
}

fn compact_value(value: &Value) -> String {
    let s = value.to_string();
    truncate(&s, 60)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl fmt::Display for SchemaWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_by_status_and_phase() {
        let tasks = json!([
            {"id": "T1", "title": "one", "status": "pending", "phase": "intake"},
            {"id": "T2", "title": "two", "status": "done", "phase": "intake"},
            {"id": "T3", "title": "three", "status": "pending", "phase": "implementation"},
        ]);

        let all = project_tasks(&tasks, &TaskFilter::default());
        assert_eq!(all.items.len(), 3);
        assert!(all.schema_warnings.is_empty());

        let pending = project_tasks(
            &tasks,
            &TaskFilter {
                status: Some("pending".into()),
                phase: None,
            },
        );
        assert_eq!(
            pending
                .items
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["T1", "T3"]
        );

        let intake_pending = project_tasks(
            &tasks,
            &TaskFilter {
                status: Some("pending".into()),
                phase: Some("intake".into()),
            },
        );
        assert_eq!(intake_pending.items.len(), 1);
        assert_eq!(intake_pending.items[0].id, "T1");

        let miss = project_tasks(
            &tasks,
            &TaskFilter {
                status: Some("blocked".into()),
                phase: None,
            },
        );
        assert!(miss.items.is_empty());
    }

    #[test]
    fn malformed_elements_go_to_schema_warnings_not_panic() {
        let tasks = json!([
            {"id": "ok", "title": "fine", "status": "pending", "phase": "intake"},
            {"title": "no id", "status": "pending", "phase": "intake"},
            {"id": "bad-status", "status": 42, "phase": "intake"},
            "not-an-object",
            null,
        ]);

        let list = project_tasks(&tasks, &TaskFilter::default());
        // All five elements appear (degraded ones retained).
        assert_eq!(list.items.len(), 5);
        assert!(!list.schema_warnings.is_empty());
        // At least the missing-id and non-object cases warn.
        assert!(
            list.schema_warnings
                .iter()
                .any(|w| w.message.contains("id") || w.message.contains("object")),
            "expected id/object warnings, got {:?}",
            list.schema_warnings
        );
        // Envelope still serializes.
        let envelope = list_envelope(&list);
        assert_eq!(envelope["schema_version"], SCHEMA_VERSION);
        assert_eq!(envelope["resource"], "task");
        assert_eq!(envelope["items"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn unknown_fields_retained_in_extra() {
        let tasks = json!([{
            "id": "T1",
            "title": "x",
            "status": "pending",
            "phase": "intake",
            "depends_on": ["T0"],
            "planned_command": "cargo test",
            "custom_marker": {"n": 1},
        }]);
        let list = project_tasks(&tasks, &TaskFilter::default());
        assert_eq!(list.items.len(), 1);
        let view = &list.items[0];
        assert_eq!(view.extra.get("planned_command").unwrap(), "cargo test");
        assert_eq!(view.extra["custom_marker"]["n"], 1);
        assert_eq!(view.depends_on, vec![json!("T0")]);

        let as_json = view_to_json(view);
        assert_eq!(as_json["planned_command"], "cargo test");
        assert_eq!(as_json["custom_marker"]["n"], 1);
    }

    #[test]
    fn find_task_returns_match_and_miss() {
        let tasks = json!([
            {"id": "a", "title": "A", "status": "pending", "phase": "intake"},
            {"id": "b", "title": "B", "status": "done", "phase": "spec"},
        ]);
        let (found, _) = find_task(&tasks, "b");
        assert_eq!(found.unwrap().title, "B");
        let (miss, _) = find_task(&tasks, "zzz");
        assert!(miss.is_none());
        assert_eq!(available_ids(&tasks, 5), vec!["a", "b"]);
    }

    #[test]
    fn describe_envelope_is_stable() {
        let tasks = json!([{
            "id": "a",
            "title": "A",
            "status": "pending",
            "phase": "intake",
            "evidence": [{"kind": "manual", "summary": "ok", "path": "notes.md"}],
            "blockers": [],
        }]);
        let (view, warnings) = find_task(&tasks, "a");
        let view = view.unwrap();
        let envelope = describe_envelope(&view, &warnings);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["resource"], "task");
        assert_eq!(envelope["items"][0]["id"], "a");
        assert!(envelope["schema_warnings"].as_array().unwrap().is_empty());

        let human = render_describe_human(&view, &warnings);
        assert!(human.contains("Task: a"));
        assert!(human.contains("kind=manual"));
        assert!(human.contains("path=notes.md"));
        // No decision advice.
        assert!(!human.to_lowercase().contains("should"));
        assert!(!human.to_lowercase().contains("recommend"));
    }
}
