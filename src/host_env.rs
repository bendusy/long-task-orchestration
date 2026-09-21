//! Which multiplexer/agent-shell is hosting this LTO process.
//!
//! LTO dispatches into whatever terminal manager owns the host agent, but
//! until a run records that manager it cannot tell, after the fact, where its
//! windows went — `state.json` carried a repo and a branch and nothing about
//! the surrounding runtime, so `--host` fell back to "unknown" even while
//! `$TERM_PROGRAM` named the app. These markers are ids the host tools set for
//! their own children; they are recorded as evidence, never as a routing
//! decision (control-loop principle 1).

use serde_json::{Map, Value};

/// A marker read off the environment: which tool set it, and the id it set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnv {
    /// Short name of the managing runtime: orca, paseo, herdr, tmux.
    pub runtime: Option<String>,
    /// Ids the runtime exposes for the pane/workspace holding this process.
    pub markers: Vec<(String, String)>,
}

/// Env vars worth recording, grouped by the runtime that sets them. The first
/// variable of each group doubles as the presence probe for that runtime.
const MARKERS: &[(&str, &[&str])] = &[
    (
        "orca",
        &[
            "ORCA_TERMINAL_HANDLE",
            "ORCA_WORKSPACE_ID",
            "ORCA_WORKTREE_ID",
            "ORCA_TAB_ID",
        ],
    ),
    ("paseo", &["PASEO_TERMINAL_ID", "PASEO_AGENT_ID"]),
    ("herdr", &["HERDR_ENV", "HERDR_SOCKET_PATH"]),
];

impl HostEnv {
    pub fn detect() -> Self {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }

    /// `lookup` is injected so tests do not mutate the process environment,
    /// which is shared across parallel test threads.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut markers = Vec::new();
        let mut runtime = None;
        for (name, keys) in MARKERS {
            let mut present = false;
            for key in *keys {
                // An exported-but-empty marker means the runtime set nothing
                // useful; treating it as presence would mislabel the host.
                if let Some(value) = lookup(key).filter(|value| !value.trim().is_empty()) {
                    present = true;
                    markers.push(((*key).to_string(), value));
                }
            }
            if present && runtime.is_none() {
                runtime = Some((*name).to_string());
            }
        }
        // An inner tmux owns the pane even inside a GUI multiplexer, so it wins
        // the runtime label while the outer markers stay recorded above.
        if lookup("TMUX").is_some_and(|value| !value.trim().is_empty()) {
            runtime = Some("tmux".to_string());
        }
        HostEnv { runtime, markers }
    }

    /// Flattened into `WorkspaceSnapshot.extra`, which is a free-form map, so
    /// this adds no schema and older runs stay readable.
    pub fn to_extra(&self) -> Map<String, Value> {
        let mut extra = Map::new();
        if let Some(runtime) = &self.runtime {
            extra.insert(
                "host_multiplexer".to_string(),
                Value::String(runtime.clone()),
            );
        }
        for (key, value) in &self.markers {
            extra.insert(
                format!("env_{}", key.to_ascii_lowercase()),
                Value::String(value.clone()),
            );
        }
        extra
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn detects_orca_and_records_every_marker() {
        let env = HostEnv::from_lookup(&lookup(&[
            ("ORCA_TERMINAL_HANDLE", "term_abc"),
            ("ORCA_WORKSPACE_ID", "ws::/repo"),
        ]));
        assert_eq!(env.runtime.as_deref(), Some("orca"));
        let extra = env.to_extra();
        assert_eq!(extra["host_multiplexer"], Value::String("orca".into()));
        assert_eq!(
            extra["env_orca_terminal_handle"],
            Value::String("term_abc".into())
        );
        assert_eq!(
            extra["env_orca_workspace_id"],
            Value::String("ws::/repo".into())
        );
    }

    #[test]
    fn inner_tmux_wins_the_label_but_outer_markers_persist() {
        let env = HostEnv::from_lookup(&lookup(&[
            ("ORCA_TERMINAL_HANDLE", "term_abc"),
            ("TMUX", "/tmp/tmux-501/default,123,0"),
        ]));
        assert_eq!(env.runtime.as_deref(), Some("tmux"));
        // The orca pane still holds the tmux server; losing that id would make
        // a dispatched window unattributable after the fact.
        assert_eq!(
            env.to_extra()["env_orca_terminal_handle"],
            Value::String("term_abc".into())
        );
    }

    #[test]
    fn empty_marker_is_not_presence() {
        let env = HostEnv::from_lookup(&lookup(&[("ORCA_TERMINAL_HANDLE", "   ")]));
        assert_eq!(env.runtime, None);
        assert!(env.to_extra().is_empty());
    }

    #[test]
    fn bare_shell_records_nothing() {
        let env = HostEnv::from_lookup(&lookup(&[]));
        assert_eq!(env.runtime, None);
        assert!(env.to_extra().is_empty());
    }
}
