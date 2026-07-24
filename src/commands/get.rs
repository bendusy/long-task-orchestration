//! `lto get <resource>` — list/filter read-only resource projections.

use crate::commands::util;
use crate::resources::task::{self, TaskFilter};
use anyhow::Context;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct GetOptions {
    pub resource: String,
    pub run_id: Option<String>,
    pub status: Option<String>,
    pub phase: Option<String>,
    pub json: bool,
}

pub fn cmd_get(repo: &Path, options: GetOptions) -> anyhow::Result<()> {
    match options.resource.as_str() {
        "task" => get_task(repo, &options),
        other => anyhow::bail!("resource '{other}' is not yet supported (supported: task)"),
    }
}

fn get_task(repo: &Path, options: &GetOptions) -> anyhow::Result<()> {
    let ctx =
        util::load_run(repo, options.run_id.as_deref()).with_context(|| "load run for get task")?;
    let filter = TaskFilter {
        status: options.status.clone(),
        phase: options.phase.clone(),
    };
    let list = task::project_tasks(&ctx.state.tasks, &filter);
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&task::list_envelope(&list))?
        );
    } else {
        println!("{}", task::render_list_human(&list));
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
                goal: "get tests".into(),
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
    fn get_task_json_envelope_is_stable() {
        let h = Harness::new();
        h.write_state(json!([
            {"id": "T1", "title": "one", "status": "pending", "phase": "intake"},
            {"id": "T2", "title": "two", "status": "done", "phase": "spec"},
        ]));
        // Capture via re-running projection path (cmd prints to stdout).
        let ctx = util::load_run(&h.repo, Some("r1")).unwrap();
        let list = task::project_tasks(
            &ctx.state.tasks,
            &TaskFilter {
                status: Some("pending".into()),
                phase: None,
            },
        );
        let envelope = task::list_envelope(&list);
        assert_eq!(envelope["schema_version"], 1);
        assert_eq!(envelope["resource"], "task");
        assert_eq!(envelope["items"].as_array().unwrap().len(), 1);
        assert_eq!(envelope["items"][0]["id"], "T1");
        assert!(envelope.get("schema_warnings").unwrap().is_array());

        cmd_get(
            &h.repo,
            GetOptions {
                resource: "task".into(),
                run_id: Some("r1".into()),
                status: None,
                phase: None,
                json: true,
            },
        )
        .unwrap();
    }

    #[test]
    fn unsupported_resource_errors() {
        let h = Harness::new();
        h.write_state(json!([]));
        let err = cmd_get(
            &h.repo,
            GetOptions {
                resource: "run".into(),
                run_id: Some("r1".into()),
                status: None,
                phase: None,
                json: false,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not yet supported"),
            "unexpected: {err}"
        );
    }
}
