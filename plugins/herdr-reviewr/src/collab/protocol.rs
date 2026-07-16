//! The versioned local collaboration protocol between reviewr and the Pi extension.
//!
//! Frames are newline-delimited JSON objects, every one carrying `v` (protocol version) and
//! `type`. The extension speaks first with `hello`; reviewr answers `hello_ack` and rejects
//! the connection when the version or review target does not match — a stale or unrelated Pi
//! process must never mutate another review session. Parsing is total: any malformed or
//! unknown frame becomes [`Inbound::Invalid`] with a reason, never a panic or a silent drop.

use serde_json::{Value, json};

/// The one protocol version this build speaks. Version negotiation is exact-match: a newer
/// or older extension gets a reject naming both versions rather than a guessed dialect.
pub const PROTOCOL_VERSION: u64 = 1;

/// What a tool location reports the agent doing — the follow-mode priority classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    Read,
    Search,
    Edit,
}

impl ActivityKind {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Self::Read),
            "search" => Some(Self::Search),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }
}

/// A draft the agent stages: an inline finding on a worktree location, or a reply to an
/// existing remote discussion. Staging is local-only by protocol design — there is no frame
/// that publishes anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedDraft {
    /// The extension's identity for this draft, echoed in the ack and reused to revise it.
    pub draft: String,
    pub body: String,
    /// `Some` stages an inline finding at this location; `None` with `reply_to` stages a reply.
    pub anchor: Option<DraftAnchor>,
    /// The remote thread id a reply belongs to.
    pub reply_to: Option<String>,
}

/// Where an agent-staged finding points in the bound worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftAnchor {
    pub path: String,
    pub line: u32,
    pub start_line: Option<u32>,
}

/// One frame from the extension. `Invalid` carries why, so protocol errors are observable
/// in the log and the contract tests instead of vanishing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inbound {
    Hello { version: u64, target: String, pi_session: String },
    PromptContext { request: u64 },
    ToolLocation { kind: ActivityKind, path: String, line: Option<u32>, op: String },
    EditCompleted { path: String, line: Option<u32>, op: String },
    TurnStarted,
    TurnSettled,
    StageDraft(StagedDraft),
    Bye,
    Invalid { reason: String },
}

/// Parse one newline-delimited frame. Never fails: garbage becomes `Invalid`.
pub fn parse_inbound(line: &str) -> Inbound {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Inbound::Invalid { reason: "not JSON".to_string() };
    };
    let Some(kind) = value["type"].as_str() else {
        return Inbound::Invalid { reason: "missing type".to_string() };
    };
    // `hello` carries the version being negotiated, so it parses before the version gate;
    // every later frame must match the negotiated version exactly.
    if kind != "hello" && value["v"].as_u64() != Some(PROTOCOL_VERSION) {
        return Inbound::Invalid { reason: format!("unsupported version in `{kind}`") };
    }
    match kind {
        "hello" => {
            let (Some(version), Some(target), Some(pi_session)) =
                (value["v"].as_u64(), value["target"].as_str(), value["pi_session"].as_str())
            else {
                return Inbound::Invalid { reason: "hello missing v/target/pi_session".into() };
            };
            Inbound::Hello {
                version,
                target: target.to_string(),
                pi_session: pi_session.to_string(),
            }
        }
        "prompt_context" => match value["request"].as_u64() {
            Some(request) => Inbound::PromptContext { request },
            None => Inbound::Invalid { reason: "prompt_context missing request".into() },
        },
        "tool_location" => {
            let (Some(kind), Some(path)) = (
                value["kind"].as_str().and_then(ActivityKind::parse),
                value["path"].as_str().filter(|p| !p.is_empty()),
            ) else {
                return Inbound::Invalid { reason: "tool_location missing kind/path".into() };
            };
            Inbound::ToolLocation {
                kind,
                path: path.to_string(),
                line: value["line"].as_u64().map(|l| l as u32),
                op: value["op"].as_str().unwrap_or_default().to_string(),
            }
        }
        "edit_completed" => match value["path"].as_str().filter(|p| !p.is_empty()) {
            Some(path) => Inbound::EditCompleted {
                path: path.to_string(),
                line: value["line"].as_u64().map(|l| l as u32),
                op: value["op"].as_str().unwrap_or_default().to_string(),
            },
            None => Inbound::Invalid { reason: "edit_completed missing path".into() },
        },
        "turn_started" => Inbound::TurnStarted,
        "turn_settled" => Inbound::TurnSettled,
        "stage_draft" => parse_stage_draft(&value),
        "bye" => Inbound::Bye,
        other => Inbound::Invalid { reason: format!("unknown type `{other}`") },
    }
}

fn parse_stage_draft(value: &Value) -> Inbound {
    let (Some(draft), Some(body)) = (
        value["draft"].as_str().filter(|d| !d.is_empty()),
        value["body"].as_str().filter(|b| !b.trim().is_empty()),
    ) else {
        return Inbound::Invalid { reason: "stage_draft missing draft/body".into() };
    };
    let anchor = value["path"].as_str().filter(|p| !p.is_empty()).and_then(|path| {
        value["line"].as_u64().map(|line| DraftAnchor {
            path: path.to_string(),
            line: line as u32,
            start_line: value["start_line"].as_u64().map(|l| l as u32),
        })
    });
    let reply_to = value["reply_to"].as_str().filter(|t| !t.is_empty()).map(str::to_string);
    if anchor.is_none() && reply_to.is_none() {
        return Inbound::Invalid { reason: "stage_draft needs an anchor or reply_to".into() };
    }
    Inbound::StageDraft(StagedDraft {
        draft: draft.to_string(),
        body: body.to_string(),
        anchor,
        reply_to,
    })
}

/// One frame to the extension, encoded as a single JSON line (no interior newlines).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    HelloAck {
        ok: bool,
        reason: Option<String>,
    },
    /// The atomic review-context snapshot answering one `prompt_context` request.
    Context {
        request: u64,
        context: Value,
    },
    DraftAck {
        draft: String,
        ok: bool,
        reason: Option<String>,
    },
}

impl Outbound {
    pub fn encode(&self) -> String {
        let value = match self {
            Self::HelloAck { ok, reason } => json!({
                "v": PROTOCOL_VERSION, "type": "hello_ack", "ok": ok, "reason": reason,
            }),
            Self::Context { request, context } => json!({
                "v": PROTOCOL_VERSION, "type": "context", "request": request,
                "context": context,
            }),
            Self::DraftAck { draft, ok, reason } => json!({
                "v": PROTOCOL_VERSION, "type": "draft_ack", "draft": draft, "ok": ok,
                "reason": reason,
            }),
        };
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_parses_and_carries_its_own_version_for_negotiation() {
        let frame = r#"{"v": 2, "type": "hello", "target": "github:o/r#7", "pi_session": "abc"}"#;
        assert_eq!(
            parse_inbound(frame),
            Inbound::Hello { version: 2, target: "github:o/r#7".into(), pi_session: "abc".into() },
            "hello parses even on a version this build does not speak — the reject must name it"
        );
    }

    #[test]
    fn non_hello_frames_require_the_exact_negotiated_version() {
        let stale = r#"{"v": 0, "type": "turn_settled"}"#;
        assert!(matches!(parse_inbound(stale), Inbound::Invalid { .. }));
        let current = format!(r#"{{"v": {PROTOCOL_VERSION}, "type": "turn_settled"}}"#);
        assert_eq!(parse_inbound(&current), Inbound::TurnSettled);
    }

    #[test]
    fn malformed_frames_become_invalid_with_a_reason_never_a_panic() {
        for (frame, needle) in [
            ("not json at all", "not JSON"),
            (r#"{"v": 1}"#, "missing type"),
            (r#"{"v": 1, "type": "warp"}"#, "unknown type"),
            (r#"{"v": 1, "type": "prompt_context"}"#, "missing request"),
            (r#"{"v": 1, "type": "tool_location", "kind": "dance", "path": "a.rs"}"#, "kind"),
            (r#"{"v": 1, "type": "tool_location", "kind": "read", "path": ""}"#, "path"),
            (r#"{"v": 1, "type": "edit_completed"}"#, "missing path"),
            (r#"{"v": 1, "type": "stage_draft", "draft": "d1", "body": "  "}"#, "draft/body"),
            (
                r#"{"v": 1, "type": "stage_draft", "draft": "d1", "body": "x"}"#,
                "anchor or reply_to",
            ),
        ] {
            match parse_inbound(frame) {
                Inbound::Invalid { reason } => {
                    assert!(reason.contains(needle), "{frame} → {reason}");
                }
                other => panic!("{frame} parsed as {other:?}"),
            }
        }
    }

    #[test]
    fn tool_locations_parse_all_three_activity_kinds() {
        for (kind, expected) in [
            ("read", ActivityKind::Read),
            ("search", ActivityKind::Search),
            ("edit", ActivityKind::Edit),
        ] {
            let frame = format!(
                r#"{{"v": 1, "type": "tool_location", "kind": "{kind}", "path": "src/a.rs", "line": 12, "op": "t1"}}"#
            );
            assert_eq!(
                parse_inbound(&frame),
                Inbound::ToolLocation {
                    kind: expected,
                    path: "src/a.rs".into(),
                    line: Some(12),
                    op: "t1".into(),
                }
            );
        }
    }

    #[test]
    fn stage_draft_parses_findings_and_replies() {
        let finding = r#"{"v": 1, "type": "stage_draft", "draft": "d1", "body": "off by one",
                          "path": "src/a.rs", "line": 9, "start_line": 4}"#;
        assert_eq!(
            parse_inbound(finding),
            Inbound::StageDraft(StagedDraft {
                draft: "d1".into(),
                body: "off by one".into(),
                anchor: Some(DraftAnchor { path: "src/a.rs".into(), line: 9, start_line: Some(4) }),
                reply_to: None,
            })
        );
        let reply = r#"{"v": 1, "type": "stage_draft", "draft": "d2", "body": "agreed",
                        "reply_to": "T99"}"#;
        assert_eq!(
            parse_inbound(reply),
            Inbound::StageDraft(StagedDraft {
                draft: "d2".into(),
                body: "agreed".into(),
                anchor: None,
                reply_to: Some("T99".into()),
            })
        );
    }

    #[test]
    fn outbound_frames_encode_as_single_lines_and_round_trip_key_fields() {
        let ack = Outbound::HelloAck { ok: false, reason: Some("target mismatch".into()) };
        let line = ack.encode();
        assert!(!line.contains('\n'), "one frame per line");
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], "hello_ack");
        assert_eq!(value["v"].as_u64(), Some(PROTOCOL_VERSION));
        assert_eq!(value["ok"], false);
        assert_eq!(value["reason"], "target mismatch");

        let ctx = Outbound::Context { request: 7, context: json!({"target": "github:o/r#7"}) };
        let value: Value = serde_json::from_str(&ctx.encode()).unwrap();
        assert_eq!(value["request"].as_u64(), Some(7));
        assert_eq!(value["context"]["target"], "github:o/r#7");
    }
}
