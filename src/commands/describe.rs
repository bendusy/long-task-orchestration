//! `lto describe <resource> <id>` — single-object read-only projection.

use crate::commands::util;
use crate::resources::task;
use anyhow::Context;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DescribeOptions {
    pub resource: String,
    pub id: String,
    pub run_id: Option<String>,
    pub json: bool,
}

pub fn cmd_describe(repo: &Path, options: DescribeOptions) -> anyhow::Result<()> {
    match options.resource.as_str() {
        "task" => describe_task(repo, &options),
        other => anyhow::bail!("resource '{other}' is not yet supported (supported: task)"),
    }
}

fn describe_task(repo: &Path, options: &DescribeOptions) -> anyhow::Result<()> {
    let ctx = util::load_run(repo, options.run_id.as_deref())
        .with_context(|| "load run for describe task")?;
    let (view, warnings) = task::find_task(&ctx.state.tasks, &options.id);
    let Some(view) = view else {
        let available = task::available_ids(&ctx.state.tasks, 5);
        let hint = if available.is_empty() {
            " (no tasks in run)".to_string()
        } else {
            format!(" (available: {})", available.join(", "))
        };
        anyhow::bail!("no such task: {}{hint}", options.id);
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&task::describe_envelope(&view, &warnings))?
        );
    } else {
        println!("{}", task::render_describe_human(&view, &warnings));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, LtoState, WorkspaceSnapshot};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;

    struct Harness {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let repo = tmp.path().join("repo");
            fs::create_dir_all(&repo).unwrap();
            Self { _tmp: tmp, repo }
        }

        fn write_state(&self, tasks: Value) {
            let run_id = "r1";
            let run_dir = self.repo.join(".lto").join(run_id);
            fs::create_dir_all(&run_dir).unwrap();
            let state = LtoState {
                run_id: run_id.to_string(),
                goal: "describe tests".into(),
                why: "cli contract".into(),
                done_when: "ok".into(),
                host_runtime: "test".into(),
                current_phase: "intake".into(),
                workspace: WorkspaceSnapshot::default(),
                tasks,
                ..LtoState::default()
            };
            state::save_state(run_dir.join("state.json"), &state).unwrap();
            fs::write(self.repo.join(".lto").join("current"), "r1\n").unwrap();
        }
    }

    #[test]
    fn describe_missing_id_lists_available() {
        let h = Harness::new();
        h.write_state(json!([
            {"id": "a1", "title": "A", "status": "pending", "phase": "intake"},
            {"id": "b2", "title": "B", "status": "pending", "phase": "intake"},
            {"id": "c3", "title": "C", "status": "pending", "phase": "intake"},
            {"id": "d4", "title": "D", "status": "pending", "phase": "intake"},
            {"id": "e5", "title": "E", "status": "pending", "phase": "intake"},
            {"id": "f6", "title": "F", "status": "pending", "phase": "intake"},
        ]));
        let err = cmd_describe(
            &h.repo,
            DescribeOptions {
                resource: "task".into(),
                id: "missing".into(),
                run_id: Some("r1".into()),
                json: false,
            },
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no such task: missing"), "msg={msg}");
        assert!(msg.contains("available:"), "msg={msg}");
        // Only first 5 ids listed.
        assert!(msg.contains("a1"));
        assert!(msg.contains("e5"));
        assert!(!msg.contains("f6"), "should list only first 5: {msg}");
    }

    #[test]
    fn describe_json_envelope_is_stable() {
        let h = Harness::new();
        h.write_state(json!([{
            "id": "T1",
            "title": "one",
            "status": "pending",
            "phase": "intake",
            "evidence": [{"kind": "manual", "summary": "note"}],
            "blockers": [],
            "extra_field": 7,
        }]));
        let ctx = util::load_run(&h.repo, Some("r1")).unwrap();
        let (view, warnings) = task::find_task(&ctx.state.tasks, "T1");
        let view = view.unwrap();
        let envelope = task::describe_envelope(&view, &warnings);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["resource"], "task");
        assert_eq!(envelope["items"].as_array().unwrap().len(), 1);
        assert_eq!(envelope["items"][0]["id"], "T1");
        assert_eq!(envelope["items"][0]["extra_field"], 7);

        cmd_describe(
            &h.repo,
            DescribeOptions {
                resource: "task".into(),
                id: "T1".into(),
                run_id: Some("r1".into()),
                json: true,
            },
        )
        .unwrap();
    }
}
