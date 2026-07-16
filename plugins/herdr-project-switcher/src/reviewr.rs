use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::herdr;

const REVIEWR_PLUGIN_ID: &str = "yassimba.reviewr";
type Rect = (u16, u16, u16, u16);
type PaneRect = (String, Rect);

#[derive(Debug, PartialEq, Eq)]
pub enum Placement {
    Split {
        anchor: String,
        direction: &'static str,
    },
    Tab {
        workspace: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReplacementPlan {
    pub old_pane_id: String,
    pub placement: Placement,
}

/// Capture where the invocation reviewr pane sits before it is closed. Only
/// left/upper anchors are representable because Herdr creates splits right/down.
pub fn plan(pane_id: &str, layout: &Value) -> Result<ReplacementPlan> {
    let panes = layout["panes"]
        .as_array()
        .context("reviewr layout has no panes")?;
    let sidebar = panes
        .iter()
        .find(|pane| pane["pane_id"].as_str() == Some(pane_id))
        .and_then(rect_of)
        .context("reviewr pane is missing from its layout")?;
    let others: Vec<_> = panes
        .iter()
        .filter(|pane| pane["pane_id"].as_str() != Some(pane_id))
        .filter_map(|pane| Some((pane["pane_id"].as_str()?.to_owned(), rect_of(pane)?)))
        .collect();

    let placement = if others.is_empty() {
        Placement::Tab {
            workspace: layout["workspace_id"]
                .as_str()
                .context("reviewr layout has no workspace")?
                .to_owned(),
        }
    } else {
        adjacent_anchor(sidebar, &others).context(
            "cannot preserve this reviewr placement; it must be right of or below another pane",
        )?
    };
    Ok(ReplacementPlan {
        old_pane_id: pane_id.to_owned(),
        placement,
    })
}

/// Replace exactly one reviewr pane with the same plugin rooted at `project`.
/// Other reviewr panes and every shell/agent pane are untouched.
pub fn execute(plan: &ReplacementPlan, project: &Path) -> Result<String> {
    herdr::plugin_pane_close(&plan.old_pane_id)
        .with_context(|| format!("closing reviewr pane {}", plan.old_pane_id))?;
    let entrypoint = if cfg!(windows) {
        "sidebar-win"
    } else {
        "sidebar"
    };
    let cwd = project.to_string_lossy();
    let options = match &plan.placement {
        Placement::Split { anchor, direction } => herdr::OpenPluginPane {
            plugin: REVIEWR_PLUGIN_ID,
            entrypoint,
            cwd: &cwd,
            target: Some((anchor, direction)),
            workspace: None,
        },
        Placement::Tab { workspace } => herdr::OpenPluginPane {
            plugin: REVIEWR_PLUGIN_ID,
            entrypoint,
            cwd: &cwd,
            target: None,
            workspace: Some(workspace),
        },
    };
    let result = herdr::plugin_pane_open(options).context("reopening reviewr")?;
    result["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .map(str::to_owned)
        .context("reopened reviewr response has no pane id")
}

fn adjacent_anchor(sidebar: Rect, panes: &[PaneRect]) -> Option<Placement> {
    let (sidebar_x, sidebar_y, sidebar_width, sidebar_height) = sidebar;
    let mut best: Option<(u32, Placement)> = None;
    for (id, (x, y, width, height)) in panes {
        if x + width == sidebar_x {
            let span = overlap(*y, y + height, sidebar_y, sidebar_y + sidebar_height);
            keep_largest(
                &mut best,
                span,
                Placement::Split {
                    anchor: id.clone(),
                    direction: "right",
                },
            );
        }
        if y + height == sidebar_y {
            let span = overlap(*x, x + width, sidebar_x, sidebar_x + sidebar_width);
            keep_largest(
                &mut best,
                span,
                Placement::Split {
                    anchor: id.clone(),
                    direction: "down",
                },
            );
        }
    }
    best.map(|(_, placement)| placement)
}

fn keep_largest(best: &mut Option<(u32, Placement)>, span: u32, placement: Placement) {
    if span > 0 && best.as_ref().is_none_or(|(current, _)| span > *current) {
        *best = Some((span, placement));
    }
}

fn overlap(a0: u16, a1: u16, b0: u16, b1: u16) -> u32 {
    u32::from(a1.min(b1).saturating_sub(a0.max(b0)))
}

fn rect_of(pane: &Value) -> Option<Rect> {
    let rect = &pane["rect"];
    Some((
        rect["x"].as_u64()? as u16,
        rect["y"].as_u64()? as u16,
        rect["width"].as_u64()? as u16,
        rect["height"].as_u64()? as u16,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_a_right_sidebar_split() {
        let layout = json!({
            "workspace_id": "w1",
            "panes": [
                { "pane_id": "main", "rect": { "x": 0, "y": 0, "width": 70, "height": 30 } },
                { "pane_id": "reviewr", "rect": { "x": 70, "y": 0, "width": 30, "height": 30 } }
            ]
        });
        assert_eq!(
            plan("reviewr", &layout).unwrap().placement,
            Placement::Split {
                anchor: "main".into(),
                direction: "right"
            }
        );
    }

    #[test]
    fn preserves_a_bottom_sidebar_split() {
        let layout = json!({
            "workspace_id": "w1",
            "panes": [
                { "pane_id": "main", "rect": { "x": 0, "y": 0, "width": 100, "height": 20 } },
                { "pane_id": "reviewr", "rect": { "x": 0, "y": 20, "width": 100, "height": 10 } }
            ]
        });
        assert_eq!(
            plan("reviewr", &layout).unwrap().placement,
            Placement::Split {
                anchor: "main".into(),
                direction: "down"
            }
        );
    }

    #[test]
    fn preserves_a_reviewr_only_tab() {
        let layout = json!({
            "workspace_id": "w1",
            "panes": [
                { "pane_id": "reviewr", "rect": { "x": 0, "y": 0, "width": 100, "height": 30 } }
            ]
        });
        assert_eq!(
            plan("reviewr", &layout).unwrap().placement,
            Placement::Tab {
                workspace: "w1".into()
            }
        );
    }

    #[test]
    fn refuses_unrepresentable_left_sidebars_before_closing() {
        let layout = json!({
            "workspace_id": "w1",
            "panes": [
                { "pane_id": "reviewr", "rect": { "x": 0, "y": 0, "width": 30, "height": 30 } },
                { "pane_id": "main", "rect": { "x": 30, "y": 0, "width": 70, "height": 30 } }
            ]
        });
        assert!(plan("reviewr", &layout).is_err());
    }
}
