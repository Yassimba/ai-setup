//! The Herdr topology adapter: create, find, focus, and close the Deep Review workspace.
//!
//! Herdr's socket API is the only surface that can create a stacked pair of command panes
//! in one shot (`layout.apply` has no CLI). This module speaks that newline-JSON protocol
//! directly through [`HerdrApi`]; the orchestration is written against the trait so the
//! contract tests drive it with recorded responses, exactly as the testing decisions ask.
//! Workspace identity is the persistent label `review:<target-key>` — labels survive Herdr
//! restarts, unlike plugin-pane records — and creation is idempotent: an existing labelled
//! workspace is focused, never duplicated.

use serde_json::{Value, json};

/// One request/response exchange with Herdr. The socket implementation connects per call;
/// tests replay recorded responses.
pub trait HerdrApi {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
}

/// The live implementation over `HERDR_SOCKET_PATH`.
#[derive(Debug)]
pub struct SocketApi {
    path: String,
    next_id: u64,
}

impl SocketApi {
    /// `None` outside a Herdr session.
    pub fn from_env() -> Option<Self> {
        Some(Self { path: std::env::var("HERDR_SOCKET_PATH").ok()?, next_id: 1 })
    }
}

impl HerdrApi for SocketApi {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        use interprocess::local_socket::traits::Stream as _;
        use interprocess::local_socket::{GenericFilePath, Stream, ToFsName};
        use std::io::{BufRead, BufReader, Write};

        let name = self
            .path
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| error.to_string())?;
        let mut stream = Stream::connect(name).map_err(|error| error.to_string())?;
        let id = format!("reviewr:{}", self.next_id);
        self.next_id += 1;
        let request = json!({"id": id, "method": method, "params": params});
        stream.write_all(request.to_string().as_bytes()).map_err(|error| error.to_string())?;
        stream.write_all(b"\n").map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).map_err(|error| error.to_string())?;
        let response: Value =
            serde_json::from_str(&line).map_err(|error| format!("{method}: {error}"))?;
        if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
            let message = error["message"].as_str().unwrap_or("unknown herdr error");
            return Err(format!("{method}: {message}"));
        }
        Ok(response["result"].clone())
    }
}

/// Everything needed to build (or find) one Deep Review workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSpec {
    /// The persistent identity: `review:<target-key>`.
    pub label: String,
    /// The bound worktree both panes run in.
    pub worktree: String,
    /// The top pane: this reviewr in Deep Review mode.
    pub reviewr_argv: Vec<String>,
    /// The bottom pane: the native interactive Pi TUI.
    pub pi_argv: Vec<String>,
    /// Environment for both panes (collaboration target/socket pins).
    pub env: Vec<(String, String)>,
}

/// The workspace serving a Deep Review target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepWorkspace {
    pub workspace_id: String,
    /// False when an existing labelled workspace was focused instead of built.
    pub created: bool,
}

/// The label for one collaboration target key.
pub fn workspace_label(target_key: &str) -> String {
    format!("review:{target_key}")
}

/// Find the labelled workspace, if Herdr already runs one.
pub fn find_workspace(api: &mut dyn HerdrApi, label: &str) -> Result<Option<String>, String> {
    let listing = api.call("workspace.list", json!({}))?;
    Ok(listing["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["label"].as_str() == Some(label))
        .and_then(|w| w["workspace_id"].as_str().map(str::to_owned)))
}

/// Create or focus the Deep Review workspace: reviewr stacked above an idle Pi, both bound
/// to the worktree. Reuse focuses the existing workspace and never rebuilds its panes; a
/// creation failure closes the half-built workspace so a retry starts clean.
pub fn ensure_workspace(
    api: &mut dyn HerdrApi,
    spec: &WorkspaceSpec,
) -> Result<DeepWorkspace, String> {
    if let Some(existing) = find_workspace(api, &spec.label)? {
        api.call("workspace.focus", json!({"workspace_id": existing}))?;
        return Ok(DeepWorkspace { workspace_id: existing, created: false });
    }
    let created = api.call(
        "workspace.create",
        json!({"cwd": spec.worktree, "label": spec.label, "focus": true, "env": {}}),
    )?;
    // Live-recorded shape: the id nests under `workspace` (workspace_created envelope).
    let Some(workspace_id) = created["workspace"]["workspace_id"]
        .as_str()
        .or_else(|| created["workspace_id"].as_str())
        .map(str::to_owned)
    else {
        return Err("workspace.create returned no workspace_id".to_string());
    };
    // Reuse the create's default tab so the workspace holds exactly the stacked pair,
    // not the pair plus a stray shell tab.
    let default_tab = created["workspace"]["active_tab_id"].as_str().map(str::to_owned);
    let env: serde_json::Map<String, Value> =
        spec.env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    // One declarative tree: reviewr is `first` (top) of a `down` split — exact stacking,
    // no post-hoc swap. The layout replaces the create's default shell tab.
    // Herdr takes either target, never both: replace the create's default tab when its id
    // is known, else target the workspace.
    let target = match &default_tab {
        Some(tab) => json!({"tab_id": tab}),
        None => json!({"workspace_id": workspace_id}),
    };
    let mut layout = json!({
        "focus": true,
        "root": {
            "type": "split",
            "direction": "down",
            "ratio": 0.6,
            "first": {
                "type": "pane",
                "label": "reviewr",
                "cwd": spec.worktree,
                "command": spec.reviewr_argv,
                "env": env,
            },
            "second": {
                "type": "pane",
                "label": "pi",
                "cwd": spec.worktree,
                "command": spec.pi_argv,
                "env": env,
            },
        },
    });
    if let Some(map) = layout.as_object_mut()
        && let Some(target) = target.as_object()
    {
        for (k, v) in target {
            map.insert(k.clone(), v.clone());
        }
    }
    if let Err(error) = api.call("layout.apply", layout) {
        // Roll back the half-built workspace so the next attempt reuses nothing broken.
        let _ = api.call("workspace.close", json!({"workspace_id": workspace_id}));
        return Err(error);
    }
    // Setup succeeded: hand the keyboard to the idle Pi so the reviewer chooses the first
    // question. Focus failure is non-fatal — the workspace itself is already focused.
    if let Ok(panes) = api.call("pane.list", json!({"workspace_id": workspace_id}))
        && let Some(pi) = panes["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|pane| pane["label"].as_str() == Some("pi"))
        && let Some(pane_id) = pi["pane_id"].as_str()
    {
        let _ = api.call("pane.focus", json!({"pane_id": pane_id}));
    }
    Ok(DeepWorkspace { workspace_id, created: true })
}

/// Focus an existing Deep Review workspace.
pub fn focus_workspace(api: &mut dyn HerdrApi, workspace_id: &str) -> Result<(), String> {
    api.call("workspace.focus", json!({"workspace_id": workspace_id}))?;
    Ok(())
}

/// Close the workspace (processes stop; collaboration state is preserved elsewhere).
pub fn close_workspace(api: &mut dyn HerdrApi, workspace_id: &str) -> Result<(), String> {
    api.call("workspace.close", json!({"workspace_id": workspace_id}))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded-response fake: asserts the request stream, replays canned results.
    struct FakeApi {
        calls: Vec<(String, Value)>,
        responses: std::collections::VecDeque<Result<Value, String>>,
    }

    impl FakeApi {
        fn new(responses: Vec<Result<Value, String>>) -> Self {
            Self { calls: Vec::new(), responses: responses.into() }
        }
    }

    impl HerdrApi for FakeApi {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.push((method.to_string(), params));
            self.responses.pop_front().expect("scripted response")
        }
    }

    fn spec() -> WorkspaceSpec {
        WorkspaceSpec {
            label: workspace_label("github:github.com/o/r#7"),
            worktree: "/work/deep".into(),
            reviewr_argv: vec!["herdr-reviewr".into(), "--deep".into(), "k".into()],
            pi_argv: vec!["pi".into(), "--session-id".into(), "abc".into()],
            env: vec![("REVIEWR_COLLAB_TARGET".into(), "github:github.com/o/r#7".into())],
        }
    }

    #[test]
    fn first_creation_builds_the_stacked_pair_and_focuses() {
        let mut api = FakeApi::new(vec![
            Ok(json!({"workspaces": []})),
            // The live-recorded workspace_created envelope: the id nests under `workspace`.
            Ok(json!({"type": "workspace_created", "workspace": {
                "workspace_id": "w9", "active_tab_id": "w9:t1", "label": "review:x",
            }})),
            Ok(json!({"type": "layout_applied"})),
            Ok(json!({"panes": [
                {"pane_id": "w9:p1", "label": "reviewr"},
                {"pane_id": "w9:p2", "label": "pi"},
            ]})),
            Ok(json!({"type": "pane_focused"})),
        ]);
        let ws = ensure_workspace(&mut api, &spec()).unwrap();
        assert_eq!(ws, DeepWorkspace { workspace_id: "w9".into(), created: true });
        let (method, params) = api.calls.last().unwrap();
        assert_eq!(method, "pane.focus");
        assert_eq!(params["pane_id"], "w9:p2", "the idle Pi pane gets the keyboard");

        let (method, params) = &api.calls[1];
        assert_eq!(method, "workspace.create");
        assert_eq!(params["label"], "review:github:github.com/o/r#7");
        assert_eq!(params["focus"], true);

        let (method, layout) = &api.calls[2];
        assert_eq!(method, "layout.apply");
        assert_eq!(layout["tab_id"], "w9:t1", "the default tab is replaced, not kept");
        assert!(
            layout.get("workspace_id").is_none(),
            "herdr takes either target, never both (live-verified reject)"
        );
        assert_eq!(layout["root"]["direction"], "down");
        assert_eq!(
            layout["root"]["first"]["label"], "reviewr",
            "reviewr is the top pane of the split"
        );
        assert_eq!(layout["root"]["second"]["label"], "pi");
        assert_eq!(layout["root"]["second"]["command"][0], "pi");
        assert_eq!(
            layout["root"]["second"]["env"]["REVIEWR_COLLAB_TARGET"],
            "github:github.com/o/r#7"
        );
    }

    #[test]
    fn an_existing_labelled_workspace_is_focused_never_duplicated() {
        let mut api = FakeApi::new(vec![
            Ok(json!({"workspaces": [
                {"workspace_id": "w3", "label": "other"},
                {"workspace_id": "w7", "label": "review:github:github.com/o/r#7"},
            ]})),
            Ok(json!({"type": "workspace_focused"})),
        ]);
        let ws = ensure_workspace(&mut api, &spec()).unwrap();
        assert_eq!(ws, DeepWorkspace { workspace_id: "w7".into(), created: false });
        assert_eq!(api.calls.len(), 2, "no create, no layout — reuse is exact");
        assert_eq!(api.calls[1].0, "workspace.focus");
        assert_eq!(api.calls[1].1["workspace_id"], "w7");
    }

    #[test]
    fn a_failed_layout_rolls_back_the_half_built_workspace() {
        let mut api = FakeApi::new(vec![
            Ok(json!({"workspaces": []})),
            Ok(json!({"type": "workspace_created", "workspace": {
                "workspace_id": "w9", "active_tab_id": "w9:t1",
            }})),
            Err("layout.apply: pane spawn failed".into()),
            Ok(json!({"type": "workspace_closed"})),
        ]);
        let error = ensure_workspace(&mut api, &spec()).unwrap_err();
        assert!(error.contains("pane spawn failed"));
        let last = api.calls.last().unwrap();
        assert_eq!(last.0, "workspace.close");
        assert_eq!(last.1["workspace_id"], "w9", "the retry starts from nothing");
    }

    #[test]
    fn a_stale_workspace_id_surfaces_as_the_herdr_error() {
        let mut api = FakeApi::new(vec![Err("workspace.focus: unknown workspace".into())]);
        let error = focus_workspace(&mut api, "w404").unwrap_err();
        assert!(error.contains("unknown workspace"));
    }

    #[test]
    fn close_targets_exactly_the_given_workspace() {
        let mut api = FakeApi::new(vec![Ok(json!({"type": "workspace_closed"}))]);
        close_workspace(&mut api, "w7").unwrap();
        assert_eq!(api.calls[0].0, "workspace.close");
        assert_eq!(api.calls[0].1["workspace_id"], "w7");
    }
}
