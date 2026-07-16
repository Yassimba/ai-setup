//! The sidebar actions and event hook (`specs/herdr-host.md#sidebar-actions`), as a binary
//! subcommand — one cross-platform code path where a bash script (plus jq) used to be.
//!
//!   herdr-reviewr sidebar toggle      open the sidebar, or close it if open
//!   herdr-reviewr sidebar open        open the sidebar, no-op if one is open
//!   herdr-reviewr sidebar close       close every reviewr pane, no-op if none
//!   herdr-reviewr sidebar auto-open   worktree.created hook: open, gated by `auto_open`/placement
//!
//! The workspace's sidebar is any pane labeled "reviewr" in the live pane list. There is no
//! state file. Actions refuse loudly (exit 1, one stderr line) and report successes on stdout;
//! a refused event stays silent after successful validation, but a config error always reports
//! through stderr for herdr's plugin log.

use std::env;
use std::process::Command;

use serde_json::Value;

use crate::config::{PluginConfig, TogglePlacement};

/// Run one sidebar mode; the process exit code (0/1) is the whole contract.
pub fn run(mode: &str) -> i32 {
    let Some(mode) = Mode::parse(mode) else {
        eprintln!("reviewr: unknown mode '{mode}' (toggle | open | close | auto-open)");
        return 1;
    };

    // Validate the whole plugin config before reading workspace state or taking any action,
    // sharing exactly one contract with every other entry point. A config error is loud even
    // for the event — it is the one refusal that must reach herdr's plugin log.
    let config = match crate::config::plugin_config() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };

    // Event policy gates the event alone: explicit actions ignore it. This is after
    // validation but before workspace or pane inspection, so a disabled event does no work.
    if mode == Mode::AutoOpen {
        if !config.auto_open() {
            return 0;
        }
        if !matches!(config.toggle_placement(), TogglePlacement::Split | TogglePlacement::Tab) {
            return 0;
        }
    }

    match Sidebar::from_env(mode, &config).and_then(|sidebar| sidebar.act()) {
        Ok(()) => 0,
        Err(Refusal(message)) => {
            // The event fires on every worktree; a runtime refusal (no workspace, not a git
            // repo) is normal there and stays silent. Manual actions refuse loudly.
            if mode == Mode::AutoOpen {
                return 0;
            }
            eprintln!("reviewr: {message}");
            1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Toggle,
    Open,
    Close,
    AutoOpen,
}

impl Mode {
    fn parse(mode: &str) -> Option<Self> {
        match mode {
            "toggle" => Some(Self::Toggle),
            "open" => Some(Self::Open),
            "close" => Some(Self::Close),
            "auto-open" => Some(Self::AutoOpen),
            _ => None,
        }
    }
}

/// A runtime refusal: one stderr line for a manual action, silence for the event.
struct Refusal(String);

fn refuse<T>(message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(message.into()))
}

struct Sidebar {
    mode: Mode,
    placement: TogglePlacement,
    direction: &'static str,
    workspace: String,
    pane: Option<String>,
    cwd: Option<String>,
}

impl Sidebar {
    /// Read the invocation context from the herdr-injected environment. The event fires
    /// without a focused pane and targets the fresh workspace from its payload instead.
    fn from_env(mode: Mode, config: &PluginConfig) -> Result<Self, Refusal> {
        let mut workspace = env::var("HERDR_WORKSPACE_ID").ok().filter(|s| !s.is_empty());
        let mut pane = env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty());
        let mut cwd = env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|context| {
                string_at(&context, &["focused_pane_cwd"])
                    .or_else(|| string_at(&context, &["workspace_cwd"]))
            });

        if mode == Mode::AutoOpen
            && let Ok(raw) = env::var("HERDR_PLUGIN_EVENT_JSON")
            && let Ok(event) = serde_json::from_str::<Value>(&raw)
        {
            // worktree.created shape: .data.workspace.workspace_id,
            // .data.workspace.worktree.checkout_path (with older fallbacks).
            workspace = string_at(&event, &["data", "workspace", "workspace_id"])
                .or_else(|| string_at(&event, &["data", "worktree", "open_workspace_id"]));
            cwd = string_at(&event, &["data", "workspace", "worktree", "checkout_path"])
                .or_else(|| string_at(&event, &["data", "worktree", "path"]));
            pane = None;
        }

        let Some(workspace) = workspace else {
            return refuse("no workspace context (invoke from inside herdr)");
        };
        Ok(Self {
            mode,
            placement: config.toggle_placement(),
            direction: config.toggle_direction().as_str(),
            workspace,
            pane,
            cwd,
        })
    }

    fn act(&self) -> Result<(), Refusal> {
        // One pane-list snapshot serves the whole run. A failed listing must not read as
        // "no sidebar" — that would stack a duplicate on toggle and false-succeed a close.
        let panes = self.pane_list()?;
        let existing: Vec<String> = panes["result"]["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|pane| pane["label"].as_str() == Some("reviewr"))
            .filter_map(|pane| pane["pane_id"].as_str().map(str::to_string))
            .collect();

        match self.mode {
            Mode::Close => {
                if existing.is_empty() {
                    println!("close: nothing open in {}", self.workspace);
                    return Ok(());
                }
                self.close_all(&existing)
            }
            Mode::Toggle if !existing.is_empty() => self.close_all(&existing),
            Mode::Open | Mode::AutoOpen if !existing.is_empty() => {
                if self.mode == Mode::Open {
                    println!("open: already open ({}) in {}", existing.join(" "), self.workspace);
                }
                Ok(())
            }
            _ => self.open(&panes),
        }
    }

    fn pane_list(&self) -> Result<Value, Refusal> {
        let out = herdr(&["pane", "list", "--workspace", &self.workspace]);
        match out {
            Some(stdout) if !stdout.trim().is_empty() => serde_json::from_str(&stdout)
                .map_err(|_| Refusal(format!("herdr pane list failed for {}", self.workspace))),
            _ => refuse(format!("herdr pane list failed for {}", self.workspace)),
        }
    }

    /// Plain `pane close`, not `plugin pane close`: the plugin-pane registry does not
    /// survive a herdr restart and would strand the pane (spec A7).
    fn close_all(&self, existing: &[String]) -> Result<(), Refusal> {
        let mut closed = String::new();
        let mut failed = String::new();
        for pane in existing {
            if herdr(&["pane", "close", pane]).is_some() {
                closed.push(' ');
                closed.push_str(pane);
            } else {
                failed.push(' ');
                failed.push_str(pane);
            }
        }
        if !failed.is_empty() {
            return refuse(format!("failed to close{failed} in {}", self.workspace));
        }
        println!("closed{closed} in {}", self.workspace);
        Ok(())
    }

    fn open(&self, panes: &Value) -> Result<(), Refusal> {
        // Opening from here on. Only inside a git repo.
        let cwd = self.cwd.as_deref().unwrap_or("");
        if cwd.is_empty() || crate::git::toplevel(std::path::Path::new(cwd)).is_none() {
            return refuse(format!(
                "not a git repo: '{}'",
                if cwd.is_empty() { "<no cwd>" } else { cwd }
            ));
        }

        // Focus follows the placement on a manual open; the event never takes it (spec A3,
        // P5, P6).
        let focus = if self.mode != Mode::AutoOpen && self.placement != TogglePlacement::Split {
            "--focus"
        } else {
            "--no-focus"
        };

        // Placement decides the pane-open shape (spec: Sidebar placement). A split or zoomed
        // open attaches to the focused pane, else the workspace's first pane.
        let placement = self.placement.as_str();
        let mut shape: Vec<String> = Vec::new();
        match self.placement {
            TogglePlacement::Split | TogglePlacement::Zoomed => {
                let target = self
                    .pane
                    .clone()
                    .or_else(|| string_at(&panes["result"]["panes"][0], &["pane_id"]));
                let Some(target) = target else {
                    return refuse(format!("no pane to attach to in {}", self.workspace));
                };
                shape.extend(["--placement".into(), placement.into()]);
                shape.extend(["--target-pane".into(), target]);
                if self.placement == TogglePlacement::Split {
                    shape.extend(["--direction".into(), self.direction.into()]);
                }
            }
            TogglePlacement::Tab => {
                shape.extend(["--placement".into(), "tab".into()]);
                shape.extend(["--workspace".into(), self.workspace.clone()]);
            }
            TogglePlacement::Overlay => {
                shape.extend(["--placement".into(), "overlay".into()]);
            }
        }

        let plugin = env::var("HERDR_PLUGIN_ID").unwrap_or_else(|_| "yassimba.reviewr".to_string());
        let mut args: Vec<String> = vec![
            "plugin".into(),
            "pane".into(),
            "open".into(),
            "--plugin".into(),
            plugin,
            "--entrypoint".into(),
            entrypoint(cfg!(windows)).into(),
        ];
        args.extend(shape);
        args.extend(["--cwd".into(), cwd.to_string(), focus.to_string()]);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let new = herdr(&arg_refs)
            .and_then(|stdout| serde_json::from_str::<Value>(&stdout).ok())
            .and_then(|v| string_at(&v, &["result", "plugin_pane", "pane", "pane_id"]));
        let Some(new) = new else {
            return refuse("herdr plugin pane open failed");
        };
        if self.mode != Mode::AutoOpen {
            println!("opened {new} ({placement}) in {}", self.workspace);
        }
        Ok(())
    }
}

/// The pane entrypoint for this build's platform. The Windows pane is a separate entrypoint:
/// pane ids are globally unique per plugin, and the two platforms need different shells for
/// `$HERDR_PLUGIN_ROOT` expansion.
fn entrypoint(windows: bool) -> &'static str {
    if windows { "sidebar-win" } else { "sidebar" }
}

/// Run the herdr CLI (`$HERDR_BIN_PATH`, else `herdr` from `PATH`) and return stdout on
/// success; a spawn failure or non-zero exit is `None` — callers decide the refusal wording.
fn herdr(args: &[&str]) -> Option<String> {
    let bin = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The non-empty string at a JSON path, if any.
fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut node = value;
    for key in path {
        node = &node[*key];
    }
    node.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::entrypoint;

    #[test]
    fn each_platform_opens_its_own_pane_entrypoint() {
        // herdr-plugin.toml declares both panes; picking the wrong one hands Windows a
        // bash-shaped command line it cannot run (and vice versa).
        assert_eq!(entrypoint(false), "sidebar");
        assert_eq!(entrypoint(true), "sidebar-win");
    }
}
