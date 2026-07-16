//! Open a URL in the user's browser — the `PR` tab's only outward action.
//!
//! See `specs/forge-host.md` (external links). Mirrors the clipboard-tool probe in
//! `export.rs`: the first platform opener on `PATH` wins; none present errors clearly.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Platform openers with their leading args, tried in order: macOS `open`, Linux `xdg-open`,
/// then the Windows shell handler via `rundll32` (on every Windows install; `start` is a cmd
/// built-in, not a probeable executable).
const OPENERS: &[(&str, &[&str])] =
    &[("open", &[]), ("xdg-open", &[]), ("rundll32", &["url.dll,FileProtocolHandler"])];

/// Open `url` in the default browser via the first available opener. Errors when none is on
/// `PATH` (the caller surfaces it to the status line). The opener hands the URL to the browser
/// and exits at once, so this waits for it — reaping the child rather than leaving a zombie, and
/// returning fast enough for a click handler (mirrors the codebase's synchronous tool calls).
pub fn open(url: &str) -> Result<()> {
    let (tool, args) = OPENERS
        .iter()
        .copied()
        .find(|(t, _)| crate::proc::on_path(t))
        .context("no URL opener found (need `open`, `xdg-open`, or `rundll32`)")?;
    let status = Command::new(tool)
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("spawning {tool}"))?;
    if !status.success() {
        anyhow::bail!("{tool} failed to open the URL");
    }
    Ok(())
}
