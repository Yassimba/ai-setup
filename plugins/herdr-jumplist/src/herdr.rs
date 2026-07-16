use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use interprocess::local_socket::Stream;

/// Focus a regular pane by id via the `pane.focus` socket method. The CLI has
/// no wrapper for it (`herdr plugin pane focus` only reaches plugin-opened
/// panes), so we speak to the server socket directly, the way herdr's own
/// clients do. Err means the pane could not be focused (typically it no
/// longer exists), which callers use to prune history entries.
pub fn pane_focus(pane: &str) -> Result<()> {
    let socket_path = std::env::var("HERDR_SOCKET_PATH")
        .context("HERDR_SOCKET_PATH is not set; run via herdr")?;
    let mut stream = connect(Path::new(&socket_path))
        .with_context(|| format!("connecting to herdr socket {socket_path}"))?;

    let request = serde_json::json!({
        "id": "herdr-jumplist",
        "method": "pane.focus",
        "params": { "pane_id": pane },
    });
    let mut line = serde_json::to_string(&request).context("serializing pane.focus request")?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .context("sending pane.focus request")?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .context("reading pane.focus response")?;
    let envelope: Value = serde_json::from_str(&response).context("parsing pane.focus response")?;
    match envelope.get("error") {
        None | Some(Value::Null) => Ok(()),
        Some(error) => bail!("pane.focus {pane} failed: {error}"),
    }
}

#[cfg(unix)]
fn connect(path: &Path) -> std::io::Result<Stream> {
    use interprocess::local_socket::{prelude::*, GenericFilePath};

    let name = path.to_fs_name::<GenericFilePath>()?;
    Stream::connect(name)
}

#[cfg(windows)]
fn connect(path: &Path) -> std::io::Result<Stream> {
    use interprocess::local_socket::{prelude::*, GenericNamespaced};

    let name = path.to_string_lossy().to_string();
    let name = name.to_ns_name::<GenericNamespaced>()?;
    Stream::connect(name)
}
