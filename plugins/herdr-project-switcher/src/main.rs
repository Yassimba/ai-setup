mod config;
mod git_status;
mod herdr;
mod projects;
mod reviewr;
mod shell;
mod ui;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use config::Config;
use serde_json::Value;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--focus-after") {
        if let Some(pane_id) = args.next() {
            for delay_ms in [200_u64, 500] {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                if herdr::pane_focus(&pane_id).is_ok() {
                    break;
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("project-switcher: {error:#}");
            eprintln!("press enter to close");
            let _ = std::io::stdin().read_line(&mut String::new());
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut config = Config::load();
    let mut roots = config.resolved_roots();
    let mut projects = projects::discover(&roots);

    if projects.is_empty() {
        let Some(root) = ui::choose_root()? else {
            return Ok(());
        };
        roots = vec![root];
        config.save_roots(&roots)?;
        projects = projects::discover(&roots);
    }
    if projects.is_empty() {
        bail!(
            "no project directories found under {}",
            roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let Some(path) = ui::pick(&projects)? else {
        return Ok(());
    };
    execute(&path, &config)
}

/// Change exactly the pane over which the picker was opened. Shell panes get
/// `cd`; reviewr panes are replaced in place at the selected project. No other
/// pane follows the switch.
fn execute(path: &Path, config: &Config) -> Result<()> {
    let context = plugin_context()?;
    let workspace_id = context["workspace_id"].as_str().unwrap_or_default();
    let panes = herdr::pane_list(workspace_id)?;
    match switch_target(&context, &panes)? {
        SwitchTarget::Shell(pane_id) => {
            let command = shell::cd_command(path, config.windows_shell);
            let result = herdr::pane_run(&pane_id, &command);
            log_debug(&format!(
                "pick={} shell={pane_id} cmd={command} result={result:?}",
                path.display()
            ));
            result?;
        }
        SwitchTarget::Reviewr(pane_id) => {
            let layout = herdr::pane_layout(&pane_id)?;
            let plan = reviewr::plan(&pane_id, &layout)?;
            let replacement = reviewr::execute(&plan, path);
            log_debug(&format!(
                "pick={} reviewr={pane_id} plan={plan:?} result={replacement:?}",
                path.display()
            ));
            focus_after_close(&replacement?);
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum SwitchTarget {
    Shell(String),
    Reviewr(String),
}

fn switch_target(context: &Value, panes: &[Value]) -> Result<SwitchTarget> {
    let focused_id = context["focused_pane_id"]
        .as_str()
        .context("no focused pane in plugin context")?;
    let focused = panes
        .iter()
        .find(|pane| pane["pane_id"].as_str() == Some(focused_id))
        .context("the invocation pane no longer exists")?;
    if is_shell_pane(focused) {
        return Ok(SwitchTarget::Shell(focused_id.to_owned()));
    }
    if focused["label"].as_str() == Some("reviewr") {
        return Ok(SwitchTarget::Reviewr(focused_id.to_owned()));
    }
    bail!("the invocation pane is unsupported; no other pane was changed")
}

fn is_shell_pane(pane: &Value) -> bool {
    pane["label"].as_str().unwrap_or("").is_empty()
        && pane["agent"].as_str().unwrap_or("").is_empty()
}

fn focus_after_close(pane_id: &str) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg("--focus-after")
        .arg(pane_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    configure_detached(&mut command);
    let _ = command.spawn();
}

#[cfg(unix)]
fn configure_detached(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_detached(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

fn plugin_context() -> Result<Value> {
    let context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .context("HERDR_PLUGIN_CONTEXT_JSON not set (not running as a herdr plugin pane?)")?;
    serde_json::from_str(&context).context("parsing HERDR_PLUGIN_CONTEXT_JSON")
}

fn log_debug(line: &str) {
    let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PathBuf::from(dir).join("switcher.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn targets_only_the_invocation_pane() {
        let context = json!({ "focused_pane_id": "focused" });
        let panes = vec![
            json!({ "pane_id": "focused" }),
            json!({ "pane_id": "larger-shell" }),
        ];
        assert_eq!(
            switch_target(&context, &panes).unwrap(),
            SwitchTarget::Shell("focused".into())
        );
    }

    #[test]
    fn never_redirects_from_an_agent_to_another_pane() {
        let context = json!({ "focused_pane_id": "agent" });
        let panes = vec![
            json!({ "pane_id": "agent", "agent": "pi" }),
            json!({ "pane_id": "shell" }),
        ];
        assert!(switch_target(&context, &panes)
            .unwrap_err()
            .to_string()
            .contains("no other pane was changed"));
    }

    #[test]
    fn targets_only_the_invocation_reviewr() {
        let context = json!({ "focused_pane_id": "reviewr-2" });
        let panes = vec![
            json!({ "pane_id": "reviewr-1", "label": "reviewr" }),
            json!({ "pane_id": "reviewr-2", "label": "reviewr" }),
            json!({ "pane_id": "shell" }),
        ];
        assert_eq!(
            switch_target(&context, &panes).unwrap(),
            SwitchTarget::Reviewr("reviewr-2".into())
        );
    }
}
