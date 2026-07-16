mod herdr;
mod history;
mod store;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use history::Direction;

fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "record" => record(),
        "back" => jump(Direction::Back),
        "forward" => jump(Direction::Forward),
        other => bail!("usage: herdr-jumplist <record|back|forward>, got '{other}'"),
    }
}

fn record() -> Result<()> {
    let pane = focused_pane_from_event()?;
    store::update(state_dir()?, |history| history.record(&pane))
}

fn jump(direction: Direction) -> Result<()> {
    let target = store::update(state_dir()?, |history| {
        history.jump(direction, |pane| herdr::pane_focus(pane).is_ok())
    })?;
    if target.is_none() {
        // Not an error: the edge of history is a normal place to be.
        let label = match direction {
            Direction::Back => "back",
            Direction::Forward => "forward",
        };
        eprintln!("herdr-jumplist: no pane to go {label} to");
    }
    Ok(())
}

fn focused_pane_from_event() -> Result<String> {
    let raw = std::env::var("HERDR_PLUGIN_EVENT_JSON")
        .context("HERDR_PLUGIN_EVENT_JSON is not set; run via a herdr event hook")?;
    let envelope: serde_json::Value =
        serde_json::from_str(&raw).context("parsing HERDR_PLUGIN_EVENT_JSON")?;
    envelope["data"]["pane_id"]
        .as_str()
        .map(str::to_string)
        .context("no data.pane_id in HERDR_PLUGIN_EVENT_JSON")
}

fn state_dir() -> Result<PathBuf> {
    let dir = std::env::var_os("HERDR_PLUGIN_STATE_DIR")
        .context("HERDR_PLUGIN_STATE_DIR is not set; run via herdr")?;
    Ok(PathBuf::from(dir))
}
