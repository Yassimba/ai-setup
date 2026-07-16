use std::env;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

fn herdr_bin() -> OsString {
    env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| OsString::from("herdr"))
}

fn json(args: &[&OsStr]) -> Result<Value> {
    let output = Command::new(herdr_bin())
        .args(args)
        .output()
        .with_context(|| format!("running herdr {}", display_args(args)))?;
    if !output.status.success() {
        bail!(
            "herdr {} failed: {}",
            display_args(args),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_result(&output.stdout, &display_args(args))
}

pub fn pane_list(workspace: &str) -> Result<Vec<Value>> {
    let result = json(&[
        OsStr::new("pane"),
        OsStr::new("list"),
        OsStr::new("--workspace"),
        OsStr::new(workspace),
    ])?;
    Ok(result["panes"].as_array().cloned().unwrap_or_default())
}

pub fn pane_run(pane: &str, command: &str) -> Result<Value> {
    json(&[
        OsStr::new("pane"),
        OsStr::new("run"),
        OsStr::new(pane),
        OsStr::new(command),
    ])
}

pub fn pane_layout(pane: &str) -> Result<Value> {
    let result = json(&[
        OsStr::new("pane"),
        OsStr::new("layout"),
        OsStr::new("--pane"),
        OsStr::new(pane),
    ])?;
    Ok(result["layout"].clone())
}

pub fn pane_focus(pane: &str) -> Result<Value> {
    json(&[
        OsStr::new("plugin"),
        OsStr::new("pane"),
        OsStr::new("focus"),
        OsStr::new(pane),
    ])
}

pub fn plugin_pane_close(pane: &str) -> Result<Value> {
    json(&[
        OsStr::new("plugin"),
        OsStr::new("pane"),
        OsStr::new("close"),
        OsStr::new(pane),
    ])
}

pub struct OpenPluginPane<'a> {
    pub plugin: &'a str,
    pub entrypoint: &'a str,
    pub cwd: &'a str,
    pub target: Option<(&'a str, &'a str)>,
    pub workspace: Option<&'a str>,
}

pub fn plugin_pane_open(options: OpenPluginPane<'_>) -> Result<Value> {
    let mut args = vec![
        OsString::from("plugin"),
        OsString::from("pane"),
        OsString::from("open"),
        OsString::from("--plugin"),
        OsString::from(options.plugin),
        OsString::from("--entrypoint"),
        OsString::from(options.entrypoint),
        OsString::from("--cwd"),
        OsString::from(options.cwd),
        OsString::from("--no-focus"),
    ];
    if let Some((target, direction)) = options.target {
        args.extend([
            OsString::from("--placement"),
            OsString::from("split"),
            OsString::from("--target-pane"),
            OsString::from(target),
            OsString::from("--direction"),
            OsString::from(direction),
        ]);
    } else if let Some(workspace) = options.workspace {
        args.extend([
            OsString::from("--placement"),
            OsString::from("tab"),
            OsString::from("--workspace"),
            OsString::from(workspace),
        ]);
    }
    let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    json(&refs)
}

fn parse_result(stdout: &[u8], invocation: &str) -> Result<Value> {
    // Some successful mutating CLI commands (notably `pane run`) intentionally
    // produce no JSON output.
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    let envelope: Value = serde_json::from_slice(stdout)
        .with_context(|| format!("parsing herdr {invocation} response"))?;
    Ok(envelope["result"].clone())
}

fn display_args(args: &[&OsStr]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_empty_output_from_successful_mutating_commands() {
        assert_eq!(parse_result(b"", "pane run").unwrap(), Value::Null);
        assert_eq!(parse_result(b"  \n", "pane run").unwrap(), Value::Null);
    }
}
