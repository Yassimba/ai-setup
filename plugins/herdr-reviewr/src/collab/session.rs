//! The collaboration session: one pure state machine binding reviewr to one Pi over the
//! local protocol.
//!
//! Everything observable about collaboration behaviour flows through [`CollaborationSession::handle`]:
//! ordered [`SessionEvent`]s go in, [`Effect`]s come out, and no IO, wall clock, or terminal
//! is touched — the event loop owns those. Scenario tests dispatch events and assert effects,
//! exactly as `specs`' testing decisions require. The session owns the context tray, alias
//! registry, draft ownership, and link state; the host owns building context snapshots from
//! viewer state and applying effects to the app, transport, and forge surfaces.

use serde_json::Value;

use super::protocol::{Inbound, Outbound, PROTOCOL_VERSION, StagedDraft};

/// Milliseconds on the host's monotonic clock, injected so timing is testable. Phase 2 of
/// the collaboration work records it; the follow-mode debounce consumes it.
pub type Millis = u64;

/// One reviewable item a tray entry resolves to: its identity within the review target plus
/// the full evidence (anchor, root body, complete replies, patch context) captured when the
/// reviewer attached it — an alias never loses evidence to later navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayItem {
    /// The review target this item belongs to; a mismatch with the session's target is
    /// rejected so one session can never carry another review's context.
    pub target: String,
    /// Stable identity within the target — a remote thread id or a local comment key.
    pub key: String,
    /// The resource as protocol JSON, sent verbatim inside context snapshots.
    pub resource: Value,
}

/// One tray slot: an alias such as `C3` bound to its captured resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayEntry {
    pub alias: String,
    pub item: TrayItem,
}

/// Who currently owns a staged draft's wording. A human edit takes ownership permanently:
/// Pi may propose a separate replacement but can never overwrite the reviewer's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftOwner {
    Pi,
    Human,
}

/// Everything the session can be told, in the order it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// A parsed protocol frame arrived from the extension.
    Frame(Inbound),
    /// The transport lost the extension (socket closed or errored).
    ChannelClosed,
    /// `a` — replace the tray with this item and hand focus to Pi.
    AttachReplace(TrayItem),
    /// `Shift+A` — toggle this item's tray membership.
    TrayToggle(TrayItem),
    /// The reviewer edited an agent-staged draft inside reviewr.
    DraftEdited { draft: String },
    /// The reviewer moved the cursor themselves; reads and searches yield for a while.
    ManualNavigated,
    /// The comment composer opened; all agent navigation freezes until it closes.
    ComposerOpened,
    /// The comment composer closed; the prior follow state resumes.
    ComposerClosed,
    /// `f` — toggle follow mode; re-enabling catches up to Pi's latest location.
    FollowToggled,
    /// Step backward through Pi's edit history (disables follow first).
    HistoryBack,
    /// Step forward through Pi's edit history.
    HistoryForward,
    /// The frame clock; coalesced navigation that came due is emitted here.
    Tick,
}

/// One agent location reviewr may follow to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLocation {
    pub path: String,
    pub line: Option<u32>,
    pub kind: super::protocol::ActivityKind,
}

/// Follow-mode timing, taken from Zed's leader-update throttle as design evidence: a 200 ms
/// trailing-edge window that coalesces rapid locations (last write wins, edits outrank), and
/// a grace period during which the reviewer's own navigation suppresses reads and searches.
pub const FOLLOW_COALESCE_MS: Millis = 200;
pub const MANUAL_GRACE_MS: Millis = 1_500;

/// A location waiting out the coalescing window before it becomes a [`Effect::Navigate`].
#[derive(Debug)]
struct PendingNav {
    location: AgentLocation,
    due: Millis,
    edit: bool,
}

/// One meaningful completed edit, the unit of backward/forward navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub path: String,
    pub line: Option<u32>,
    /// The tool operation that produced it; repeated locations from one operation coalesce.
    op: String,
}

/// Everything the session can ask the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Write one frame to the extension.
    Send(Outbound),
    /// Build the atomic review-context snapshot for this request and send it. The host
    /// builds it from one viewer-state read plus [`CollaborationSession::tray_json`], so a
    /// prompt's context can never mix two moments.
    SnapshotContext { request: u64 },
    /// Focus the Pi pane (after `a` replaced the tray).
    FocusPi,
    /// The tray changed; re-render its chips.
    TrayChanged,
    /// Stage a new agent-authored draft into the app's draft surfaces.
    StageDraft(StagedDraft),
    /// Replace the body of a still-Pi-owned draft the agent revised.
    ReviseDraft(StagedDraft),
    /// The link came up (true) or went down (false); update the status surface.
    LinkChanged(bool),
    /// The agent started a turn (working); feeds last-turn tracking.
    TurnStarted,
    /// The agent finished a turn; refresh what it may have edited.
    TurnSettled,
    /// Move the viewer to the agent's location (already debounced and prioritized).
    Navigate(AgentLocation),
    /// Follow mode flipped; update the status surface.
    FollowChanged(bool),
    /// An event was refused; the reason is render- and log-worthy, never silent.
    Rejected { reason: String },
}

/// The protocol link's current state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
enum Link {
    #[default]
    Disconnected,
    Connected {
        pi_session: String,
    },
}

/// See the module docs. One session is bound to one immutable review target key.
#[derive(Debug)]
pub struct CollaborationSession {
    target: String,
    link: Link,
    tray: Vec<TrayEntry>,
    /// Every alias ever issued, by item key. An item that returns to the tray keeps the
    /// alias earlier conversation already used for it; the counter never runs backwards, so
    /// an alias can never silently rebind to a different resource.
    aliases: std::collections::HashMap<String, String>,
    next_alias: u32,
    /// Draft ownership by the extension's draft id.
    drafts: std::collections::HashMap<String, DraftOwner>,
    // --- follow mode ---
    follow: bool,
    composing: bool,
    /// Reads and searches yield to the reviewer's own navigation until this instant.
    manual_until: Millis,
    pending_nav: Option<PendingNav>,
    /// Pi's latest reported location — shown while follow is off, and the catch-up target.
    latest: Option<AgentLocation>,
    /// Completed edits, oldest first; `history_cursor` is `Some` while browsing them.
    history: Vec<HistoryEntry>,
    history_cursor: Option<usize>,
}

impl CollaborationSession {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            link: Link::default(),
            tray: Vec::new(),
            aliases: std::collections::HashMap::new(),
            next_alias: 1,
            drafts: std::collections::HashMap::new(),
            follow: true,
            composing: false,
            manual_until: 0,
            pending_nav: None,
            latest: None,
            history: Vec::new(),
            history_cursor: None,
        }
    }

    /// Whether follow mode is on (it starts on).
    pub fn follow_enabled(&self) -> bool {
        self.follow
    }

    /// How much of the reviewer's manual-navigation grace remains — the window during which
    /// reads and searches will not move the pane. `None` once it expires, and while follow
    /// is off, where nothing is being held back.
    pub fn manual_grace_remaining(&self, now: Millis) -> Option<Millis> {
        (self.follow && now < self.manual_until).then(|| self.manual_until - now)
    }

    /// Pi's latest reported location, independent of whether the viewer moved to it.
    pub fn pi_location(&self) -> Option<&AgentLocation> {
        self.latest.as_ref()
    }

    /// The 1-based position and length of the edit history, present whenever there is any.
    /// Live (not browsing) pins the position to the newest entry, so the footer reads
    /// `⟲ 14/14` and a step back visibly moves the position while the total keeps growing.
    pub fn history_position(&self) -> Option<(usize, usize)> {
        if self.history.is_empty() {
            return None;
        }
        let at = self.history_cursor.map_or(self.history.len(), |at| at + 1);
        Some((at, self.history.len()))
    }

    /// Everything worth persisting across process restarts, as one JSON value.
    pub fn export_state(&self) -> Value {
        serde_json::json!({
            "follow": self.follow,
            "next_alias": self.next_alias,
            "aliases": self.aliases,
            "tray": self.tray.iter().map(|entry| serde_json::json!({
                "alias": entry.alias,
                "target": entry.item.target,
                "key": entry.item.key,
                "resource": entry.item.resource,
            })).collect::<Vec<_>>(),
            "history": self.history.iter().map(|entry| serde_json::json!({
                "path": entry.path, "line": entry.line, "op": entry.op,
            })).collect::<Vec<_>>(),
            "drafts": self.drafts.iter().map(|(id, owner)| serde_json::json!({
                "id": id,
                "owner": match owner { DraftOwner::Pi => "pi", DraftOwner::Human => "human" },
            })).collect::<Vec<_>>(),
        })
    }

    /// Restore a persisted [`Self::export_state`] document. Unknown fields are ignored and
    /// missing ones keep their defaults, so an older document still restores what it holds.
    pub fn import_state(&mut self, doc: &Value) {
        if let Some(follow) = doc["follow"].as_bool() {
            self.follow = follow;
        }
        if let Some(next) = doc["next_alias"].as_u64() {
            self.next_alias = next as u32;
        }
        if let Some(aliases) = doc["aliases"].as_object() {
            self.aliases = aliases
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|alias| (k.clone(), alias.to_string())))
                .collect();
        }
        self.tray = doc["tray"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some(TrayEntry {
                    alias: entry["alias"].as_str()?.to_string(),
                    item: TrayItem {
                        target: entry["target"].as_str()?.to_string(),
                        key: entry["key"].as_str()?.to_string(),
                        resource: entry["resource"].clone(),
                    },
                })
            })
            .collect();
        self.history = doc["history"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some(HistoryEntry {
                    path: entry["path"].as_str()?.to_string(),
                    line: entry["line"].as_u64().map(|l| l as u32),
                    op: entry["op"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect();
        self.drafts = doc["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let owner = match entry["owner"].as_str()? {
                    "human" => DraftOwner::Human,
                    _ => DraftOwner::Pi,
                };
                Some((entry["id"].as_str()?.to_string(), owner))
            })
            .collect();
    }

    /// The immutable review target key this session serves.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Whether a Pi extension is currently linked.
    pub fn connected(&self) -> bool {
        matches!(self.link, Link::Connected { .. })
    }

    /// The linked Pi session id, once a hello was accepted.
    pub fn pi_session(&self) -> Option<&str> {
        match &self.link {
            Link::Connected { pi_session } => Some(pi_session),
            Link::Disconnected => None,
        }
    }

    /// The current tray, for chip rendering.
    pub fn tray(&self) -> &[TrayEntry] {
        &self.tray
    }

    /// Who owns a staged draft, if the session knows it.
    pub fn draft_owner(&self, draft: &str) -> Option<DraftOwner> {
        self.drafts.get(draft).copied()
    }

    /// Drop a draft's ownership record after the host failed to stage it, so a corrected
    /// retry under the same id is a fresh stage rather than a "revision" of nothing.
    pub fn forget_draft(&mut self, draft: &str) {
        self.drafts.remove(draft);
    }

    /// The tray as protocol JSON for a context snapshot: `[{alias, …resource}]`.
    pub fn tray_json(&self) -> Value {
        Value::Array(
            self.tray
                .iter()
                .map(|entry| {
                    let mut resource = entry.item.resource.clone();
                    if let Some(map) = resource.as_object_mut() {
                        map.insert("alias".to_string(), Value::String(entry.alias.clone()));
                    }
                    resource
                })
                .collect(),
        )
    }

    /// Advance the machine by one event at `now` on the host's monotonic clock.
    pub fn handle(&mut self, event: SessionEvent, now: Millis) -> Vec<Effect> {
        match event {
            SessionEvent::Frame(frame) => self.on_frame(frame, now),
            SessionEvent::ChannelClosed => {
                if self.connected() {
                    self.link = Link::Disconnected;
                    vec![Effect::LinkChanged(false)]
                } else {
                    Vec::new()
                }
            }
            SessionEvent::AttachReplace(item) => {
                if let Err(effects) = self.check_target(&item) {
                    return effects;
                }
                self.tray.clear();
                self.tray_insert(item);
                vec![Effect::TrayChanged, Effect::FocusPi]
            }
            SessionEvent::TrayToggle(item) => {
                if let Err(effects) = self.check_target(&item) {
                    return effects;
                }
                if let Some(at) = self.tray.iter().position(|e| e.item.key == item.key) {
                    self.tray.remove(at);
                } else {
                    self.tray_insert(item);
                }
                vec![Effect::TrayChanged]
            }
            SessionEvent::DraftEdited { draft } => {
                // The reviewer's wording now owns this draft; Pi revisions bounce from here on.
                self.drafts.insert(draft, DraftOwner::Human);
                Vec::new()
            }
            SessionEvent::ManualNavigated => {
                self.manual_until = now + MANUAL_GRACE_MS;
                // A pending read/search yields immediately; a pending edit still lands.
                if self.pending_nav.as_ref().is_some_and(|p| !p.edit) {
                    self.pending_nav = None;
                }
                Vec::new()
            }
            SessionEvent::ComposerOpened => {
                // Composition freezes agent navigation so Pi cannot move the anchor
                // under the reviewer's cursor while they type.
                self.composing = true;
                self.pending_nav = None;
                Vec::new()
            }
            SessionEvent::ComposerClosed => {
                self.composing = false;
                Vec::new()
            }
            SessionEvent::FollowToggled => {
                self.follow = !self.follow;
                let mut effects = vec![Effect::FollowChanged(self.follow)];
                if self.follow {
                    // Catch up to Pi's latest meaningful location, immediately.
                    self.history_cursor = None;
                    if let Some(latest) = self.latest.clone() {
                        effects.push(Effect::Navigate(latest));
                    }
                } else {
                    self.pending_nav = None;
                }
                effects
            }
            SessionEvent::HistoryBack => self.history_step(-1),
            SessionEvent::HistoryForward => self.history_step(1),
            SessionEvent::Tick => {
                if self.follow
                    && !self.composing
                    && self.pending_nav.as_ref().is_some_and(|p| now >= p.due)
                {
                    let pending = self.pending_nav.take().expect("checked above");
                    return vec![Effect::Navigate(pending.location)];
                }
                Vec::new()
            }
        }
    }

    /// Feed one reported agent location through the priority and coalescing rules.
    fn observe(&mut self, location: AgentLocation, now: Millis) -> Vec<Effect> {
        use super::protocol::ActivityKind;
        self.latest = Some(location.clone());
        if !self.follow || self.composing {
            return Vec::new();
        }
        let edit = location.kind == ActivityKind::Edit;
        // The reviewer's own navigation holds reads and searches at bay; edits override.
        if !edit && now < self.manual_until {
            return Vec::new();
        }
        match &mut self.pending_nav {
            None => {
                self.pending_nav =
                    Some(PendingNav { location, due: now + FOLLOW_COALESCE_MS, edit });
            }
            // Within the window the newest location wins its class, but a read or search
            // can never displace a pending edit — a background search must not hide an
            // active modification. The window keeps its original deadline (trailing edge).
            Some(pending) => {
                if edit || !pending.edit {
                    pending.location = location;
                    pending.edit = pending.edit || edit;
                }
            }
        }
        Vec::new()
    }

    /// Record one completed edit into the history, coalescing noise: repeated locations
    /// from the same tool operation collapse into that operation's entry, and an exact
    /// repeat of the last location adds nothing.
    fn record_edit(&mut self, path: &str, line: Option<u32>, op: &str) {
        if let Some(last) = self.history.last_mut() {
            if !op.is_empty() && last.op == op {
                if last.path == path {
                    last.line = line;
                    return;
                }
            } else if last.path == path && last.line == line {
                return;
            }
        }
        // History is one linear log: an edit landing while the reviewer browses appends
        // without moving their cursor, so no entry is ever lost to a forward-branch
        // truncation — `f` catches up to live, forward steps walk into the new entries.
        self.history.push(HistoryEntry { path: path.to_string(), line, op: op.to_string() });
    }

    /// Step through the edit history. Backward disables follow first, so Pi cannot
    /// immediately pull the viewer away from what they are inspecting; forward past the
    /// newest entry resumes following live, so forward always undoes what backward began.
    fn history_step(&mut self, delta: isize) -> Vec<Effect> {
        if self.history.is_empty() {
            return Vec::new();
        }
        let last = self.history.len() - 1;
        let Some(cursor) = self.history_cursor else {
            if delta >= 0 {
                // Live with nothing ahead; a forward press must not yank the viewer
                // backward or silently turn follow off.
                return Vec::new();
            }
            return self.begin_browsing(last);
        };
        if delta >= 0 && cursor == last {
            return self.resume_live();
        }
        let next = if delta < 0 { cursor.saturating_sub(1) } else { cursor + 1 };
        self.history_cursor = Some(next);
        vec![self.navigate_to_entry(next)]
    }

    /// The first backward step from live. While following, the viewer already sits at the
    /// newest edit, so landing on it again would read as a swallowed keypress — the first
    /// step skips straight past it.
    fn begin_browsing(&mut self, last: usize) -> Vec<Effect> {
        let at_newest = self.follow
            && self.latest.as_ref().is_some_and(|l| {
                let newest = &self.history[last];
                l.path == newest.path && l.line == newest.line
            });
        let mut effects = Vec::new();
        if self.follow {
            self.follow = false;
            self.pending_nav = None;
            effects.push(Effect::FollowChanged(false));
        }
        let start = if at_newest { last.saturating_sub(1) } else { last };
        self.history_cursor = Some(start);
        effects.push(self.navigate_to_entry(start));
        effects
    }

    /// Forward walked past the newest entry: leave browsing and follow live again —
    /// the same catch-up `f` performs, reached without a mode change of key.
    fn resume_live(&mut self) -> Vec<Effect> {
        self.history_cursor = None;
        self.follow = true;
        let mut effects = vec![Effect::FollowChanged(true)];
        if let Some(latest) = self.latest.clone() {
            effects.push(Effect::Navigate(latest));
        }
        effects
    }

    fn navigate_to_entry(&self, index: usize) -> Effect {
        let entry = self.history[index].clone();
        Effect::Navigate(AgentLocation {
            path: entry.path,
            line: entry.line,
            kind: super::protocol::ActivityKind::Edit,
        })
    }

    fn on_frame(&mut self, frame: Inbound, now: Millis) -> Vec<Effect> {
        match frame {
            Inbound::Hello { version, target, pi_session } => {
                if version != PROTOCOL_VERSION {
                    return vec![Effect::Send(Outbound::HelloAck {
                        ok: false,
                        reason: Some(format!(
                            "protocol v{version} not supported; this reviewr speaks v{PROTOCOL_VERSION}"
                        )),
                    })];
                }
                if target != self.target {
                    // A Pi bound to another review (or a stale worktree) must never join.
                    return vec![Effect::Send(Outbound::HelloAck {
                        ok: false,
                        reason: Some(format!(
                            "target mismatch: this session reviews {}, hello names {target}",
                            self.target
                        )),
                    })];
                }
                self.link = Link::Connected { pi_session };
                vec![
                    Effect::Send(Outbound::HelloAck { ok: true, reason: None }),
                    Effect::LinkChanged(true),
                ]
            }
            // Every other frame requires an accepted hello: a process that skipped the
            // handshake has proven nothing about its version or target.
            frame if !self.connected() => {
                let reason = format!("frame before hello: {frame:?}");
                vec![Effect::Rejected { reason }]
            }
            Inbound::PromptContext { request } => vec![Effect::SnapshotContext { request }],
            Inbound::ToolLocation { kind, path, line, .. } => {
                self.observe(AgentLocation { path, line, kind }, now)
            }
            Inbound::EditCompleted { path, line, op } => {
                self.record_edit(&path, line, &op);
                // The completed edit carries the precise changed line — the meaningful
                // navigation signal for an edit, ahead of its call-time location.
                self.observe(
                    AgentLocation { path, line, kind: super::protocol::ActivityKind::Edit },
                    now,
                )
            }
            Inbound::TurnStarted => vec![Effect::TurnStarted],
            Inbound::TurnSettled => vec![Effect::TurnSettled],
            // Ownership is decided here; the final ack is the host's, sent after staging
            // actually succeeded or failed against the app's draft surfaces.
            Inbound::StageDraft(draft) => match self.drafts.get(&draft.draft) {
                Some(DraftOwner::Human) => vec![Effect::Send(Outbound::DraftAck {
                    draft: draft.draft.clone(),
                    ok: false,
                    reason: Some(
                        "draft is owned by the reviewer; propose a new draft instead".to_string(),
                    ),
                })],
                Some(DraftOwner::Pi) => vec![Effect::ReviseDraft(draft)],
                None => {
                    self.drafts.insert(draft.draft.clone(), DraftOwner::Pi);
                    vec![Effect::StageDraft(draft)]
                }
            },
            Inbound::Bye => {
                self.link = Link::Disconnected;
                vec![Effect::LinkChanged(false)]
            }
            Inbound::Invalid { reason } => vec![Effect::Rejected { reason }],
        }
    }

    /// Reject items from another review target outright.
    fn check_target(&self, item: &TrayItem) -> Result<(), Vec<Effect>> {
        if item.target == self.target {
            Ok(())
        } else {
            Err(vec![Effect::Rejected {
                reason: format!(
                    "item belongs to {}, this session reviews {}",
                    item.target, self.target
                ),
            }])
        }
    }

    /// Insert an item, reusing the alias it already earned or minting the next one.
    fn tray_insert(&mut self, item: TrayItem) {
        let alias = self
            .aliases
            .entry(item.key.clone())
            .or_insert_with(|| {
                let alias = format!("C{}", self.next_alias);
                self.next_alias += 1;
                alias
            })
            .clone();
        self.tray.push(TrayEntry { alias, item });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(key: &str) -> TrayItem {
        TrayItem {
            target: "github:o/r#7".into(),
            key: key.into(),
            resource: json!({"key": key, "body": format!("body of {key}")}),
        }
    }

    fn connected() -> CollaborationSession {
        let mut s = CollaborationSession::new("github:o/r#7");
        s.handle(
            SessionEvent::Frame(Inbound::Hello {
                version: PROTOCOL_VERSION,
                target: "github:o/r#7".into(),
                pi_session: "pi-1".into(),
            }),
            0,
        );
        s
    }

    #[test]
    fn a_matching_hello_connects_and_acks() {
        let mut s = CollaborationSession::new("github:o/r#7");
        let effects = s.handle(
            SessionEvent::Frame(Inbound::Hello {
                version: PROTOCOL_VERSION,
                target: "github:o/r#7".into(),
                pi_session: "pi-1".into(),
            }),
            0,
        );
        assert_eq!(
            effects,
            vec![
                Effect::Send(Outbound::HelloAck { ok: true, reason: None }),
                Effect::LinkChanged(true),
            ]
        );
        assert!(s.connected());
        assert_eq!(s.pi_session(), Some("pi-1"));
    }

    #[test]
    fn version_and_target_mismatches_are_rejected_with_named_reasons() {
        let mut s = CollaborationSession::new("github:o/r#7");
        let effects = s.handle(
            SessionEvent::Frame(Inbound::Hello {
                version: 99,
                target: "github:o/r#7".into(),
                pi_session: "pi-1".into(),
            }),
            0,
        );
        let Effect::Send(Outbound::HelloAck { ok: false, reason: Some(reason) }) = &effects[0]
        else {
            panic!("expected a version reject, got {effects:?}");
        };
        assert!(reason.contains("v99") && reason.contains("v1"), "{reason}");
        assert!(!s.connected());

        let effects = s.handle(
            SessionEvent::Frame(Inbound::Hello {
                version: PROTOCOL_VERSION,
                target: "gitlab:other/repo!3".into(),
                pi_session: "pi-2".into(),
            }),
            0,
        );
        let Effect::Send(Outbound::HelloAck { ok: false, reason: Some(reason) }) = &effects[0]
        else {
            panic!("expected a target reject, got {effects:?}");
        };
        assert!(reason.contains("gitlab:other/repo!3"), "{reason}");
        assert!(!s.connected(), "a stale or unrelated Pi must never join");
    }

    #[test]
    fn frames_before_a_hello_are_rejected() {
        let mut s = CollaborationSession::new("github:o/r#7");
        let effects = s.handle(SessionEvent::Frame(Inbound::TurnSettled), 0);
        assert!(matches!(&effects[0], Effect::Rejected { .. }), "{effects:?}");
    }

    #[test]
    fn attach_replaces_the_tray_and_focuses_pi() {
        let mut s = connected();
        s.handle(SessionEvent::TrayToggle(item("old")), 0);
        let effects = s.handle(SessionEvent::AttachReplace(item("T1")), 0);
        assert_eq!(effects, vec![Effect::TrayChanged, Effect::FocusPi]);
        let aliases: Vec<_> = s.tray().iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, ["C2"], "replace empties the tray; the item keeps its own alias");
        assert_eq!(s.tray()[0].item.key, "T1");
    }

    #[test]
    fn toggle_accumulates_and_removes_and_aliases_stay_stable() {
        let mut s = connected();
        s.handle(SessionEvent::TrayToggle(item("T1")), 0);
        s.handle(SessionEvent::TrayToggle(item("T2")), 0);
        let aliases: Vec<_> = s.tray().iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, ["C1", "C2"]);

        // Removing and re-adding T1 keeps the alias the conversation already used for it,
        // and never hands C1 to any other item.
        s.handle(SessionEvent::TrayToggle(item("T1")), 0);
        assert_eq!(s.tray().len(), 1);
        s.handle(SessionEvent::TrayToggle(item("T3")), 0);
        s.handle(SessionEvent::TrayToggle(item("T1")), 0);
        let aliases: Vec<_> = s.tray().iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, ["C2", "C3", "C1"], "aliases are stable across removal");
    }

    #[test]
    fn items_from_another_review_target_are_rejected() {
        let mut s = connected();
        let foreign = TrayItem {
            target: "gitlab:other/repo!3".into(),
            key: "T1".into(),
            resource: json!({}),
        };
        for event in
            [SessionEvent::AttachReplace(foreign.clone()), SessionEvent::TrayToggle(foreign)]
        {
            let effects = s.handle(event, 0);
            assert!(matches!(&effects[0], Effect::Rejected { .. }), "{effects:?}");
            assert!(s.tray().is_empty(), "a foreign item never lands in the tray");
        }
    }

    #[test]
    fn tray_json_carries_the_alias_with_the_full_resource() {
        let mut s = connected();
        s.handle(SessionEvent::TrayToggle(item("T1")), 0);
        let tray = s.tray_json();
        assert_eq!(tray[0]["alias"], "C1");
        assert_eq!(tray[0]["body"], "body of T1", "the captured evidence rides along");
    }

    #[test]
    fn a_prompt_context_request_snapshots_atomically_via_one_effect() {
        let mut s = connected();
        let effects = s.handle(SessionEvent::Frame(Inbound::PromptContext { request: 42 }), 0);
        assert_eq!(effects, vec![Effect::SnapshotContext { request: 42 }]);
    }

    #[test]
    fn agent_drafts_stage_then_revise_until_a_human_edit_takes_ownership() {
        let mut s = connected();
        let draft = StagedDraft {
            draft: "d1".into(),
            body: "consider a bound".into(),
            anchor: None,
            reply_to: Some("T9".into()),
        };
        // First stage: recorded as Pi-owned and handed to the host, which owns the ack.
        let effects = s.handle(SessionEvent::Frame(Inbound::StageDraft(draft.clone())), 0);
        assert_eq!(effects, vec![Effect::StageDraft(draft.clone())]);
        assert_eq!(s.draft_owner("d1"), Some(DraftOwner::Pi));

        // A host-side staging failure forgets the record, so a corrected retry under the
        // same id stages fresh instead of "revising" nothing.
        s.forget_draft("d1");
        assert_eq!(s.draft_owner("d1"), None);
        let effects = s.handle(SessionEvent::Frame(Inbound::StageDraft(draft.clone())), 0);
        assert_eq!(effects, vec![Effect::StageDraft(draft.clone())]);

        // Pi may revise its own draft.
        let revised = StagedDraft { body: "consider an upper bound".into(), ..draft.clone() };
        let effects = s.handle(SessionEvent::Frame(Inbound::StageDraft(revised.clone())), 0);
        assert_eq!(effects, vec![Effect::ReviseDraft(revised)]);

        // A human edit transfers ownership; Pi's next revision bounces with a reason.
        s.handle(SessionEvent::DraftEdited { draft: "d1".into() }, 0);
        assert_eq!(s.draft_owner("d1"), Some(DraftOwner::Human));
        let effects = s.handle(SessionEvent::Frame(Inbound::StageDraft(draft)), 0);
        let Effect::Send(Outbound::DraftAck { ok: false, reason: Some(reason), .. }) = &effects[0]
        else {
            panic!("expected an ownership reject, got {effects:?}");
        };
        assert!(reason.contains("owned by the reviewer"), "{reason}");

        // A separate replacement under a new id is still welcome.
        let replacement = StagedDraft {
            draft: "d2".into(),
            body: "alternative wording".into(),
            anchor: None,
            reply_to: Some("T9".into()),
        };
        let effects = s.handle(SessionEvent::Frame(Inbound::StageDraft(replacement.clone())), 0);
        assert_eq!(effects[0], Effect::StageDraft(replacement));
    }

    #[test]
    fn disconnects_and_byes_mark_the_link_down_once() {
        let mut s = connected();
        assert_eq!(s.handle(SessionEvent::ChannelClosed, 0), vec![Effect::LinkChanged(false)]);
        assert!(!s.connected());
        assert_eq!(s.handle(SessionEvent::ChannelClosed, 0), Vec::new(), "no duplicate signal");

        let mut s = connected();
        assert_eq!(
            s.handle(SessionEvent::Frame(Inbound::Bye), 0),
            vec![Effect::LinkChanged(false)]
        );
    }

    #[test]
    fn a_reconnect_after_disconnect_is_a_fresh_handshake_on_the_same_state() {
        let mut s = connected();
        s.handle(SessionEvent::TrayToggle(item("T1")), 0);
        s.handle(SessionEvent::ChannelClosed, 0);
        // The tray and aliases survive the outage; only the link resets.
        let effects = s.handle(
            SessionEvent::Frame(Inbound::Hello {
                version: PROTOCOL_VERSION,
                target: "github:o/r#7".into(),
                pi_session: "pi-2".into(),
            }),
            0,
        );
        assert!(matches!(&effects[0], Effect::Send(Outbound::HelloAck { ok: true, .. })));
        assert_eq!(s.pi_session(), Some("pi-2"), "a new Pi process may resume the session");
        assert_eq!(s.tray()[0].alias, "C1", "collaboration state survived the outage");
    }

    #[test]
    fn malformed_frames_surface_as_rejections() {
        let mut s = connected();
        let effects =
            s.handle(SessionEvent::Frame(Inbound::Invalid { reason: "not JSON".into() }), 0);
        assert_eq!(effects, vec![Effect::Rejected { reason: "not JSON".into() }]);
    }

    use super::super::protocol::ActivityKind;

    fn read_at(path: &str, line: u32) -> Inbound {
        Inbound::ToolLocation {
            kind: ActivityKind::Read,
            path: path.into(),
            line: Some(line),
            op: String::new(),
        }
    }

    fn nav_effects(effects: &[Effect]) -> Vec<(String, Option<u32>)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Navigate(at) => Some((at.path.clone(), at.line)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rapid_locations_coalesce_to_one_navigation_after_the_window() {
        let mut s = connected();
        assert!(s.handle(SessionEvent::Frame(read_at("a.rs", 1)), 1_000).is_empty());
        assert!(s.handle(SessionEvent::Frame(read_at("b.rs", 2)), 1_050).is_empty());
        assert!(s.handle(SessionEvent::Frame(read_at("c.rs", 3)), 1_100).is_empty());
        // Not yet due: the window runs from the first location.
        assert!(s.handle(SessionEvent::Tick, 1_150).is_empty());
        let effects = s.handle(SessionEvent::Tick, 1_200);
        assert_eq!(nav_effects(&effects), [("c.rs".to_string(), Some(3))], "last write wins");
        assert!(s.handle(SessionEvent::Tick, 1_400).is_empty(), "emitted once");
    }

    #[test]
    fn an_edit_outranks_reads_and_a_read_cannot_displace_a_pending_edit() {
        let mut s = connected();
        s.handle(SessionEvent::Frame(read_at("a.rs", 1)), 1_000);
        s.handle(
            SessionEvent::Frame(Inbound::EditCompleted {
                path: "edited.rs".into(),
                line: Some(9),
                op: "t1".into(),
            }),
            1_050,
        );
        // A later read must not hide the active modification.
        s.handle(SessionEvent::Frame(read_at("noise.rs", 7)), 1_100);
        let effects = s.handle(SessionEvent::Tick, 1_200);
        assert_eq!(nav_effects(&effects), [("edited.rs".to_string(), Some(9))]);
    }

    #[test]
    fn manual_navigation_suppresses_reads_for_the_grace_but_edits_override() {
        let mut s = connected();
        s.handle(SessionEvent::ManualNavigated, 1_000);
        s.handle(SessionEvent::Frame(read_at("a.rs", 1)), 1_100);
        assert!(s.handle(SessionEvent::Tick, 1_400).is_empty(), "reads yield to the reviewer");
        // An edit lands even inside the grace.
        s.handle(
            SessionEvent::Frame(Inbound::EditCompleted {
                path: "edited.rs".into(),
                line: Some(3),
                op: "t1".into(),
            }),
            1_200,
        );
        let effects = s.handle(SessionEvent::Tick, 1_400);
        assert_eq!(nav_effects(&effects), [("edited.rs".to_string(), Some(3))]);
        // After the grace expires, reads flow again.
        s.handle(SessionEvent::Frame(read_at("b.rs", 2)), 1_000 + MANUAL_GRACE_MS + 1);
        let effects =
            s.handle(SessionEvent::Tick, 1_000 + MANUAL_GRACE_MS + FOLLOW_COALESCE_MS + 1);
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
    }

    #[test]
    fn the_manual_grace_is_reportable_while_it_holds_follow_back() {
        let mut s = connected();
        assert_eq!(s.manual_grace_remaining(1_000), None, "no grace before manual navigation");
        s.handle(SessionEvent::ManualNavigated, 1_000);
        assert_eq!(s.manual_grace_remaining(1_400), Some(MANUAL_GRACE_MS - 400));
        assert_eq!(s.manual_grace_remaining(1_000 + MANUAL_GRACE_MS), None, "expired");
        // With follow off nothing is being held back, so no remainder is reported.
        s.handle(SessionEvent::ManualNavigated, 2_000);
        s.handle(SessionEvent::FollowToggled, 2_100);
        assert_eq!(s.manual_grace_remaining(2_200), None);
    }

    #[test]
    fn composition_freezes_agent_navigation_until_the_composer_closes() {
        let mut s = connected();
        s.handle(SessionEvent::ComposerOpened, 1_000);
        s.handle(SessionEvent::Frame(read_at("a.rs", 1)), 1_100);
        assert!(s.handle(SessionEvent::Tick, 2_000).is_empty(), "no movement while typing");
        assert_eq!(s.pi_location().map(|at| at.path.as_str()), Some("a.rs"), "still recorded");
        s.handle(SessionEvent::ComposerClosed, 2_100);
        // Only locations reported after the close move the viewer.
        s.handle(SessionEvent::Frame(read_at("b.rs", 2)), 2_200);
        let effects = s.handle(SessionEvent::Tick, 2_200 + FOLLOW_COALESCE_MS);
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
    }

    #[test]
    fn follow_off_records_without_moving_and_reenabling_catches_up() {
        let mut s = connected();
        let effects = s.handle(SessionEvent::FollowToggled, 1_000);
        assert_eq!(effects, vec![Effect::FollowChanged(false)]);
        s.handle(SessionEvent::Frame(read_at("a.rs", 5)), 1_100);
        assert!(s.handle(SessionEvent::Tick, 2_000).is_empty(), "off means not moved");
        assert_eq!(s.pi_location().map(|at| at.path.as_str()), Some("a.rs"));

        let effects = s.handle(SessionEvent::FollowToggled, 3_000);
        assert_eq!(effects[0], Effect::FollowChanged(true));
        assert_eq!(nav_effects(&effects), [("a.rs".to_string(), Some(5))], "catch-up is immediate");
    }

    #[test]
    fn edit_history_traverses_backward_and_forward_and_disables_follow_first() {
        let mut s = connected();
        for (path, line, op) in [("a.rs", 1, "t1"), ("b.rs", 2, "t2"), ("c.rs", 3, "t3")] {
            s.handle(
                SessionEvent::Frame(Inbound::EditCompleted {
                    path: path.into(),
                    line: Some(line),
                    op: op.into(),
                }),
                1_000,
            );
        }
        // Following at the newest edit: the first step must visibly move, so it skips
        // the entry the viewer is already on.
        let effects = s.handle(SessionEvent::HistoryBack, 5_000);
        assert_eq!(effects[0], Effect::FollowChanged(false), "history nav disables follow");
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
        assert_eq!(s.history_position(), Some((2, 3)));

        let effects = s.handle(SessionEvent::HistoryBack, 5_100);
        assert_eq!(nav_effects(&effects), [("a.rs".to_string(), Some(1))]);
        // Clamped at the oldest entry.
        let effects = s.handle(SessionEvent::HistoryBack, 5_200);
        assert_eq!(nav_effects(&effects), [("a.rs".to_string(), Some(1))]);
        let effects = s.handle(SessionEvent::HistoryForward, 5_300);
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
        let effects = s.handle(SessionEvent::HistoryForward, 5_400);
        assert_eq!(nav_effects(&effects), [("c.rs".to_string(), Some(3))]);
        // Forward past the newest entry resumes following live.
        let effects = s.handle(SessionEvent::HistoryForward, 5_500);
        assert_eq!(effects[0], Effect::FollowChanged(true), "forward past the end refollows");
        assert_eq!(nav_effects(&effects), [("c.rs".to_string(), Some(3))], "catch-up to latest");
        assert!(s.follow_enabled());
        assert_eq!(s.history_position(), Some((3, 3)), "live again");
    }

    #[test]
    fn forward_while_live_is_a_noop_that_keeps_follow() {
        let mut s = connected();
        for (path, line) in [("a.rs", 1), ("b.rs", 2)] {
            s.handle(
                SessionEvent::Frame(Inbound::EditCompleted {
                    path: path.into(),
                    line: Some(line),
                    op: String::new(),
                }),
                1_000,
            );
        }
        let effects = s.handle(SessionEvent::HistoryForward, 2_000);
        assert_eq!(effects, Vec::new(), "nothing ahead of live to walk into");
        assert!(s.follow_enabled(), "a dead forward press must not turn follow off");
        assert_eq!(s.history_position(), Some((2, 2)));
    }

    #[test]
    fn back_with_follow_off_starts_at_the_newest_entry() {
        let mut s = connected();
        s.handle(SessionEvent::FollowToggled, 500); // off — viewer is somewhere of their own
        for (path, line) in [("a.rs", 1), ("b.rs", 2)] {
            s.handle(
                SessionEvent::Frame(Inbound::EditCompleted {
                    path: path.into(),
                    line: Some(line),
                    op: String::new(),
                }),
                1_000,
            );
        }
        // Not following, so the newest edit is a genuine move — no skip.
        let effects = s.handle(SessionEvent::HistoryBack, 2_000);
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
        assert_eq!(s.history_position(), Some((2, 2)));
    }

    #[test]
    fn history_coalesces_one_operations_repeats_and_appends_while_browsing() {
        let mut s = connected();
        // Three completions from one tool operation collapse to one entry (latest line).
        for line in [4, 9, 12] {
            s.handle(
                SessionEvent::Frame(Inbound::EditCompleted {
                    path: "a.rs".into(),
                    line: Some(line),
                    op: "t1".into(),
                }),
                1_000,
            );
        }
        s.handle(
            SessionEvent::Frame(Inbound::EditCompleted {
                path: "b.rs".into(),
                line: Some(2),
                op: "t2".into(),
            }),
            1_100,
        );
        // Following at the newest entry (b.rs), so the first back skips it and lands on
        // the coalesced operation directly.
        let effects = s.handle(SessionEvent::HistoryBack, 2_000);
        assert_eq!(
            nav_effects(&effects),
            [("a.rs".to_string(), Some(12))],
            "one operation, one entry, at its final location"
        );

        // A new edit while browsing appends: the cursor holds its place while the
        // total grows, and forward steps walk into the new entries — nothing is lost.
        assert_eq!(s.history_position(), Some((1, 2)), "browsing at the oldest of two");
        s.handle(
            SessionEvent::Frame(Inbound::EditCompleted {
                path: "d.rs".into(),
                line: Some(7),
                op: "t9".into(),
            }),
            3_000,
        );
        assert_eq!(s.history_position(), Some((1, 3)), "cursor stays, total grows");
        let effects = s.handle(SessionEvent::HistoryForward, 3_100);
        assert_eq!(nav_effects(&effects), [("b.rs".to_string(), Some(2))]);
        let effects = s.handle(SessionEvent::HistoryForward, 3_200);
        assert_eq!(
            nav_effects(&effects),
            [("d.rs".to_string(), Some(7))],
            "forward reaches the edit that landed mid-browse"
        );
    }

    #[test]
    fn history_position_is_pinned_to_the_newest_entry_while_live() {
        let mut s = connected();
        assert_eq!(s.history_position(), None, "no indicator before any edit");
        for (path, line) in [("a.rs", 1), ("b.rs", 2)] {
            s.handle(
                SessionEvent::Frame(Inbound::EditCompleted {
                    path: path.into(),
                    line: Some(line),
                    op: format!("t{line}"),
                }),
                1_000,
            );
        }
        assert_eq!(s.history_position(), Some((2, 2)), "live reads N/N");
        // Following at the newest edit, the first step skips it — the position moves.
        s.handle(SessionEvent::HistoryBack, 2_000);
        assert_eq!(s.history_position(), Some((1, 2)), "first step visibly moves");
        s.handle(SessionEvent::HistoryBack, 2_100);
        assert_eq!(s.history_position(), Some((1, 2)), "clamped at the oldest");
        // Refollow returns the indicator to live.
        s.handle(SessionEvent::FollowToggled, 3_000);
        assert_eq!(s.history_position(), Some((2, 2)));
    }
}
