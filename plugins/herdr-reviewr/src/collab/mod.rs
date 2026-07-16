//! Collaboration between reviewr and a native Pi agent over a local protocol.
//!
//! The cluster splits along testability seams: [`protocol`] is the versioned wire format,
//! [`session`] the pure behavioural machine (events in, effects out — every collaboration
//! invariant is asserted there), [`context`] the normalized `ReviewContext` every surface
//! speaks, [`transport`] the local socket, and this module's [`CollabHost`] the thin IO
//! shell the event loop pumps once per frame. The host translates transport events into
//! session events, applies `Send` effects to the socket, and forwards everything that needs
//! app state — snapshots, staging, focus — to the loop as [`HostAction`]s, following the
//! same drain-per-frame discipline as the PR fetch channels.

pub mod context;
pub mod materialize;
pub mod protocol;
pub mod session;
pub mod store;
pub mod topology;
pub mod transport;

use serde_json::Value;

use protocol::{Outbound, StagedDraft};
use session::{
    AgentLocation, CollaborationSession, Effect, Millis, SessionEvent, TrayEntry, TrayItem,
};
use transport::{CollabTransport, TransportEvent};

/// One thing the event loop must apply against app state on the host's behalf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAction {
    /// Build the atomic context snapshot (viewer state + tray) and answer this request.
    SnapshotContext { request: u64 },
    /// Focus the Pi pane after `a` replaced the tray.
    FocusPi,
    /// Stage a new agent-authored draft; report the outcome via [`CollabHost::draft_staged`].
    StageDraft(StagedDraft),
    /// Replace the body of a Pi-owned draft the agent revised.
    ReviseDraft(StagedDraft),
    /// The tray changed; viewer chrome should re-render.
    TrayChanged,
    /// The agent started a turn (working); feeds last-turn tracking.
    TurnStarted,
    /// The agent settled a turn; refresh what it may have edited.
    TurnSettled,
    /// Move the viewer to the agent's location (already debounced and prioritized).
    Navigate(AgentLocation),
    /// Follow mode flipped; update the status surface.
    FollowChanged(bool),
}

/// See the module docs.
#[derive(Debug)]
pub struct CollabHost {
    session: CollaborationSession,
    transport: Option<CollabTransport>,
    /// The newest transport connection; frames and closes from older readers are stale.
    conn: Option<u64>,
    link_up: bool,
}

impl CollabHost {
    /// Start collaboration for one worktree: bind the deterministic socket and key the
    /// session by the worktree's local target. A failed bind (another reviewr already
    /// serves this worktree) degrades to no collaboration; reviewing continues.
    pub fn start(worktree: &std::path::Path) -> Self {
        // Deep Review workspaces pin both ends explicitly through the environment; a plain
        // sidebar derives them from its worktree, and the extension derives the same pair
        // from `git rev-parse --show-toplevel`.
        let path = std::env::var("REVIEWR_COLLAB_SOCKET")
            .unwrap_or_else(|_| transport::socket_path(worktree));
        let target = std::env::var("REVIEWR_COLLAB_TARGET")
            .unwrap_or_else(|_| context::local_target_key(worktree));
        let transport = CollabTransport::bind(&path);
        if transport.is_none() {
            crate::logln!("collab: bind failed on {path}; collaboration off for this instance");
        }
        Self { session: CollaborationSession::new(target), transport, conn: None, link_up: false }
    }

    /// Whether a Pi extension currently holds an accepted link.
    pub fn link_up(&self) -> bool {
        self.link_up
    }

    /// The tray for chip rendering.
    pub fn tray(&self) -> &[TrayEntry] {
        self.session.tray()
    }

    /// The tray as snapshot JSON.
    pub fn tray_json(&self) -> Value {
        self.session.tray_json()
    }

    /// Drain the socket and advance the session; returns what the loop must apply. The
    /// frame tick also flushes coalesced navigation that came due.
    pub fn pump(&mut self, now: Millis) -> Vec<HostAction> {
        let Some(transport) = &self.transport else { return Vec::new() };
        let mut actions = Vec::new();
        for event in transport.drain() {
            match event {
                TransportEvent::Connected { conn } => {
                    // A newer connection displaces the old link; the fresh hello re-auths.
                    if self.conn.is_some() {
                        let effects = self.session.handle(SessionEvent::ChannelClosed, now);
                        self.apply(effects, &mut actions);
                    }
                    self.conn = Some(conn);
                }
                TransportEvent::Line { conn, line } if self.conn == Some(conn) => {
                    let frame = protocol::parse_inbound(&line);
                    let effects = self.session.handle(SessionEvent::Frame(frame), now);
                    self.apply(effects, &mut actions);
                }
                TransportEvent::Closed { conn } if self.conn == Some(conn) => {
                    self.conn = None;
                    let effects = self.session.handle(SessionEvent::ChannelClosed, now);
                    self.apply(effects, &mut actions);
                }
                // Lines and closes from a replaced connection prove nothing about the link.
                TransportEvent::Line { .. } | TransportEvent::Closed { .. } => {}
            }
        }
        let tick = self.session.handle(SessionEvent::Tick, now);
        self.apply(tick, &mut actions);
        actions
    }

    /// The session's follow flag (on by default).
    pub fn follow_enabled(&self) -> bool {
        self.session.follow_enabled()
    }

    /// How much of the reviewer's manual-navigation grace remains, if any.
    pub fn manual_grace_remaining(&self, now: session::Millis) -> Option<session::Millis> {
        self.session.manual_grace_remaining(now)
    }

    /// Pi's latest reported location, as `path:line` display text.
    pub fn pi_location_label(&self) -> Option<String> {
        self.session.pi_location().map(|at| match at.line {
            Some(line) => format!("{}:{line}", at.path),
            None => at.path.clone(),
        })
    }

    /// The 1-based edit-history position while browsing it.
    pub fn history_position(&self) -> Option<(usize, usize)> {
        self.session.history_position()
    }

    /// The linked Pi session id, once a hello was accepted.
    pub fn pi_session(&self) -> Option<String> {
        self.session.pi_session().map(str::to_string)
    }

    /// The session's persistent slice, for the store document.
    pub fn export_session_state(&self) -> Value {
        self.session.export_state()
    }

    /// Restore the session's persistent slice from a store document.
    pub fn import_session_state(&mut self, doc: &Value) {
        self.session.import_state(doc);
    }

    /// Forward one session event that needs no result plumbing (manual navigation,
    /// composer edges, follow toggle, history steps).
    pub fn signal(&mut self, event: SessionEvent, now: Millis) -> Vec<HostAction> {
        let effects = self.session.handle(event, now);
        let mut actions = Vec::new();
        self.apply(effects, &mut actions);
        actions
    }

    /// `a` — replace the tray with the selected item and focus Pi.
    pub fn attach_replace(&mut self, item: TrayItem, now: Millis) -> Vec<HostAction> {
        let effects = self.session.handle(SessionEvent::AttachReplace(item), now);
        let mut actions = Vec::new();
        self.apply(effects, &mut actions);
        actions
    }

    /// `Shift+A` — toggle the selected item in the tray.
    pub fn tray_toggle(&mut self, item: TrayItem, now: Millis) -> Vec<HostAction> {
        let effects = self.session.handle(SessionEvent::TrayToggle(item), now);
        let mut actions = Vec::new();
        self.apply(effects, &mut actions);
        actions
    }

    /// The reviewer edited an agent-staged draft: ownership moves to the human.
    pub fn draft_edited(&mut self, draft: &str, now: Millis) {
        let _ = self.session.handle(SessionEvent::DraftEdited { draft: draft.into() }, now);
    }

    /// Answer one context request with the snapshot the loop built.
    pub fn send_context(&self, request: u64, snapshot: &context::Snapshot) {
        self.send(&Outbound::Context { request, context: snapshot.to_json() });
    }

    /// Report a staging outcome back to the extension; a failure forgets the ownership
    /// record so a corrected retry stages fresh.
    pub fn draft_staged(&mut self, draft: &str, ok: bool, reason: Option<String>) {
        if !ok {
            self.session.forget_draft(draft);
        }
        self.send(&Outbound::DraftAck { draft: draft.to_string(), ok, reason });
    }

    fn send(&self, frame: &Outbound) {
        if let Some(transport) = &self.transport {
            transport.send(&frame.encode());
        }
    }

    /// Route one batch of session effects: socket writes and link state are handled here;
    /// everything needing app state is queued for the loop.
    fn apply(&mut self, effects: Vec<Effect>, actions: &mut Vec<HostAction>) {
        for effect in effects {
            match effect {
                Effect::Send(frame) => self.send(&frame),
                Effect::LinkChanged(up) => {
                    self.link_up = up;
                    crate::logln!("collab: link {}", if up { "up" } else { "down" });
                }
                Effect::Rejected { reason } => crate::logln!("collab: rejected — {reason}"),
                Effect::SnapshotContext { request } => {
                    actions.push(HostAction::SnapshotContext { request });
                }
                Effect::FocusPi => actions.push(HostAction::FocusPi),
                Effect::TrayChanged => actions.push(HostAction::TrayChanged),
                Effect::StageDraft(draft) => actions.push(HostAction::StageDraft(draft)),
                Effect::ReviseDraft(draft) => actions.push(HostAction::ReviseDraft(draft)),
                Effect::TurnStarted => actions.push(HostAction::TurnStarted),
                Effect::TurnSettled => actions.push(HostAction::TurnSettled),
                Effect::Navigate(location) => actions.push(HostAction::Navigate(location)),
                Effect::FollowChanged(on) => actions.push(HostAction::FollowChanged(on)),
            }
        }
    }
}
