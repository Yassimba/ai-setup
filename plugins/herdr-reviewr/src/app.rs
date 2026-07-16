//! Application state and transitions for the Changes review TUI.
//!
//! See `specs/tui.md` and `specs/review-model.md`. This module is terminal-free:
//! every method is a pure state transition or a read-only git/export call, so the
//! whole interaction model is testable without a backend. `src/main.rs` owns the
//! terminal and maps input events onto these methods.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::diff::{DiffCache, FileDiff, Row, View, range_endpoint};
use crate::export::{ExportTarget, format_all};
use crate::file_list::{self, Annotation, Entry, RowKind};
use crate::forge;
use crate::git;
use crate::highlight::Highlighter;
use crate::logln;
use crate::model::{Comment, CommentStore, Scope, Side};
use crate::switcher;
use crate::theme::{self, Palette};
use crate::turn::{Status, TurnTracker};

/// The file-list pane's default width and resize bounds, as a percent of the body. The
/// bounds keep both panes usable however the reviewer drags the divider.
const DEFAULT_LIST_PCT: u16 = 32;
const MIN_LIST_PCT: u16 = 15;
const MAX_LIST_PCT: u16 = 60;
/// Picker listings are cheap to reuse but should not hide forge-side changes for long.
const PR_LISTING_TTL: Duration = Duration::from_secs(30);

/// Which pane has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Files,
    Diff,
}

/// What the file-list cursor points at, by path, so it can be restored to the same target
/// after the tree rebuilds on a poll.
enum Anchor {
    File(String),
    Dir(String),
}

/// Which top-level tab is active: the changes reviewer, the whole-repo browser, or the
/// PR/MR review mirror.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Changes,
    AllFiles,
    Pr,
}

impl Tab {
    /// Whether this tab uses the file-tree / diff machinery (and so the per-tab stash). The
    /// `PR` tab does not — it holds its own state and never swaps into the diff fields.
    fn is_file_tab(self) -> bool {
        matches!(self, Tab::Changes | Tab::AllFiles)
    }
}

/// The inactive tab's saved navigation and left-pane state, swapped in on a tab switch so
/// each tab keeps its own selection and scroll (specs/tui.md).
#[derive(Debug, Default)]
struct TabStash {
    entries: Vec<Entry>,
    file_rows: Vec<file_list::Row>,
    file_cursor: usize,
    file_scroll: usize,
    toggled_dirs: HashSet<String>,
    diff: FileDiff,
    visible: Vec<Row>,
    expanded_folds: HashSet<u32>,
    diff_path: Option<String>,
    diff_cursor: usize,
    diff_scroll: usize,
    h_scroll: usize,
    select_anchor: Option<usize>,
    line_decorations: HashMap<u32, crate::diff::LineDecoration>,
    pi_marks: PiMarks,
}

/// The deep-session agent's changes to the displayed file since the session baseline —
/// what the `✦` gutter badge renders. Only populated for views built from worktree
/// content.
#[derive(Debug, Default)]
struct PiMarks {
    /// Changed lines, keyed by current (worktree) line number.
    lines: HashMap<u32, crate::diff::LineDecoration>,
    /// The agent removed the file's entire content (deleted or emptied it): no surviving
    /// worktree line can carry a mark, so every content row badges instead.
    removed_file: bool,
}

/// The interaction mode the UI is in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    /// Writing a comment; `editing` is the store index when editing an existing one.
    Composing {
        editing: Option<usize>,
    },
    /// Browsing the comments-list overlay.
    List,
}

/// The PR/MR picker overlay's lifecycle: fetching the open list, showing it, or explaining
/// why it couldn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrPicker {
    Loading,
    Loaded { listing: forge::PrListing, filtered: Vec<usize>, cursor: usize },
    Failed(String),
}

impl PrPicker {
    /// The rows in display order: the open section, then merged & closed.
    pub fn rows(listing: &forge::PrListing) -> impl Iterator<Item = &forge::PrListItem> {
        listing.open.iter().chain(listing.done.iter())
    }

    fn row(listing: &forge::PrListing, index: usize) -> Option<&forge::PrListItem> {
        Self::rows(listing).nth(index)
    }
}

fn filtered_pr_indices(listing: &forge::PrListing, query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, usize, usize, usize)> = PrPicker::rows(listing)
        .enumerate()
        .filter_map(|(index, item)| {
            let state = match item.state {
                forge::PrState::Open => "open",
                forge::PrState::Merged => "merged",
                forge::PrState::Closed => "closed",
            };
            let draft = if item.is_draft { "draft" } else { "" };
            let number = item.number.to_string();
            let hash_number = format!("#{}", item.number);
            let bang_number = format!("!{}", item.number);
            let (tier, at) = [
                number.as_str(),
                hash_number.as_str(),
                bang_number.as_str(),
                item.title.as_str(),
                item.head_ref.as_str(),
                item.author.as_str(),
                state,
                draft,
            ]
            .into_iter()
            .filter_map(|field| switcher::fuzzy_score(field, query))
            .min()?;
            let section = usize::from(index >= listing.open.len());
            Some((section, tier, at, index))
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, _, _, index)| index).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListingRequest {
    pub target: crate::git::RepoTarget,
    generation: u64,
}

#[derive(Debug)]
struct PrListingCache {
    target: crate::git::RepoTarget,
    listing: forge::PrListing,
    loaded_at: Instant,
}

impl PrListingCache {
    fn is_fresh_for(&self, target: &crate::git::RepoTarget) -> bool {
        self.target == *target && self.loaded_at.elapsed() < PR_LISTING_TTL
    }
}

/// A picked PR/MR pinned to the tab. The pin remembers the branch it was made on: a branch
/// switch is a new review seat and drops it, matching the tab's branch-bound contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrPin {
    pub branch: Option<String>,
    pub number: u64,
}

/// The selected review's remote Changes payload. The request identity rides every state so a
/// stale worker result can be rejected after a second pick or project switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteChanges {
    Idle,
    Loading(forge::ReviewDiffRequest),
    Ready { request: forge::ReviewDiffRequest, patch: crate::diff::PatchSet },
    Failed { request: forge::ReviewDiffRequest, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRemoteDraft {
    pub target: crate::git::RepoTarget,
    pub number: u64,
    pub draft: forge::ReviewDraft,
    pub error: Option<String>,
    /// The provider may have accepted the POST but its response was lost. Never retry this item
    /// automatically; the card directs the user to verify on the forge first.
    pub outcome_unknown: bool,
}

/// Aggregated indicators painted beside one file-tree row. File change state stays in its
/// existing [`Annotation`]; `changed` is only the descendant summary used by All-files folders.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TreeBadges {
    pub changed: bool,
    pub commented: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteCompose {
    target: crate::git::RepoTarget,
    number: u64,
    action: forge::ReviewDraftAction,
}

impl RemoteChanges {
    fn request(&self) -> Option<&forge::ReviewDiffRequest> {
        match self {
            Self::Idle => None,
            Self::Loading(request) | Self::Ready { request, .. } | Self::Failed { request, .. } => {
                Some(request)
            }
        }
    }
}

/// The project-switcher overlay (`ctrl-p` on any tab): a typed filter over the candidate
/// projects discovered once on open; `enter` re-points the sidebar at the highlighted one
/// (`specs/tui.md#project-switcher`).
#[derive(Debug, PartialEq, Eq)]
pub struct ProjectSwitcher {
    /// Every discovered candidate, best-ranked first (frecency, then name).
    pub projects: Vec<switcher::Project>,
    /// The filter typed so far; empty matches everything.
    pub query: String,
    /// Indices into `projects` matching `query`, best match first.
    pub filtered: Vec<usize>,
    /// Cursor into `filtered`.
    pub cursor: usize,
    /// A pick held back because unsent comments would be dropped; a second `enter` on the
    /// same project confirms. Any other switcher input withdraws it.
    pending: Option<PathBuf>,
}

impl ProjectSwitcher {
    fn new(projects: Vec<switcher::Project>) -> Self {
        let filtered = (0..projects.len()).collect();
        Self { projects, query: String::new(), filtered, cursor: 0, pending: None }
    }

    /// The highlighted project's path, if any row is under the cursor.
    fn selected(&self) -> Option<&switcher::Project> {
        self.filtered.get(self.cursor).map(|&i| &self.projects[i])
    }

    /// Re-derive `filtered` for the current query; the cursor restarts at the best match.
    fn refilter(&mut self) {
        self.filtered = switcher::filter(&self.projects, &self.query);
        self.cursor = 0;
        self.pending = None;
    }
}

/// A footer action — what the bar offers for the current context. Semantic only: the renderer
/// maps each to its key glyph and label and styles it by [`Tier`] (`specs/tui.md`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FooterAction {
    PickPr,
    PinPr,
    UnpinPr,
    ClosePicker,
    Comment,
    Reply,
    Select,
    ClearSelection,
    EditComment,
    DeleteComment,
    JumpComment,
    ExpandFold,
    ExpandDir,
    CollapseDir,
    /// Switch focus between the file list and the diff; the label names the destination pane.
    TogglePane,
    Scope,
    /// Open the project switcher; `SwitchProject` is its overlay's confirm.
    Projects,
    SwitchProject,
    Send,
    List,
    Copy,
    Save,
    Newline,
    Cancel,
    CloseList,
    OpenPr,
    Refresh,
    Tabs,
    Quit,
    /// `a` — attach the selected comment to the Pi session and focus it.
    AttachPi,
    /// `Shift+A` — toggle the selected comment in the Pi context tray.
    TrayToggle,
    /// `Shift+D` — open (or resume) the Deep Review workspace for this target.
    DeepReview,
    /// `Shift+X` — end Deep Review (two-step, with loss warnings).
    EndDeep,
    /// `U` — apply a detected remote-head move to the review worktree.
    UpdateHead,
}

/// A footer action's visual weight, and its survival priority when the line is too narrow:
/// `Orientation` is dropped first, then trailing `Normal` actions; `Primary` is never dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Primary,
    Normal,
    Orientation,
}

/// The full state of the review session.
// The several bools (wrap, reveal_files, reveal_diff, resizing, should_quit) are independent
// toggles, not a state machine in disguise, so the excessive-bools lint does not apply.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct App {
    pub repo: PathBuf,
    pub base: Option<String>,
    pub scope: Scope,
    /// The active tab; it drives both panes and selects the per-tab state in play.
    pub tab: Tab,
    /// Which file tab (`Changes`/`AllFiles`) currently occupies the diff/file fields. Tracked
    /// apart from `tab` so the `PR` tab can be active while a file tab's state stays frozen in
    /// place, with the other file tab in the stash.
    active_file_tab: Tab,
    pub focus: Focus,
    /// The navigator's source for the active tab: changed files in `Changes`, the whole
    /// worktree in `All files`.
    pub entries: Vec<Entry>,
    /// The flattened directory tree over `entries` — the rows the navigator paints. The
    /// `file_cursor` indexes this, not `entries`.
    pub file_rows: Vec<file_list::Row>,
    pub file_cursor: usize,
    /// Top visible row of the file list, kept so `file_cursor` stays on screen when the
    /// changeset is taller than the pane.
    pub file_scroll: usize,
    /// Set by a navigation that moves `file_cursor`; consumed once per frame to scroll the
    /// cursor into view. The wheel never sets it, so wheel-scrolling moves the viewport alone.
    pub reveal_files: bool,
    /// Set by a navigation that moves `diff_cursor`; consumed once per frame to scroll the
    /// cursor into view. The wheel never sets it.
    pub reveal_diff: bool,
    /// Whether the current compose was opened from the comments-list overlay, so finishing it
    /// returns there rather than dropping to the diff.
    resume_list: bool,
    /// Directory paths toggled away from the tab's resting state — collapsed in `Changes`
    /// (expanded by default), expanded in `All files` (collapsed by default). Keyed by path,
    /// so it survives a poll that rebuilds the tree.
    toggled_dirs: HashSet<String>,
    /// The inactive tab's saved state, swapped in on a tab switch.
    stash: TabStash,
    /// The active scope's changed files, keyed by repo-relative path and recomputed every
    /// reload regardless of tab. Keys back the header count and diff-comment staleness; values
    /// annotate `All files` entries with their marker and stats. Stays correct while `All
    /// files` lists the whole worktree.
    changed: HashMap<String, Annotation>,
    pub diff: FileDiff,
    /// The rows actually shown: `diff.rows` with each fold collapsed to a marker or
    /// expanded to its lines. The cursor, scroll, selection, and hit-testing index this.
    pub visible: Vec<Row>,
    /// Fold anchors (first-hidden-line numbers) currently expanded; survives a poll.
    expanded_folds: HashSet<u32>,
    /// The file the open diff belongs to — the diff title, frozen with the diff
    /// while composing even if `file_cursor` drifts as the file list updates.
    pub diff_path: Option<String>,
    pub diff_cursor: usize,
    /// Top visible diff line. Sticky: only moves to keep the cursor in view, so the
    /// diff does not jump on every cursor step and drag-selection stays stable.
    pub diff_scroll: usize,
    /// Horizontal scroll, in columns, applied to the diff when wrap is off.
    pub h_scroll: usize,
    /// Whether long diff lines wrap (default) or are scrolled horizontally.
    pub wrap: bool,
    /// The file-list pane's width as a percent of the body; the diff takes the rest. The
    /// reviewer resizes it by dragging the divider or with `[` / `]`.
    pub list_pct: u16,
    /// Whether a mouse drag is currently moving the pane divider.
    pub resizing: bool,
    pub select_anchor: Option<usize>,
    /// Source-control gutter markers for the active `All files` content view.
    line_decorations: HashMap<u32, crate::diff::LineDecoration>,
    /// What of the displayed file the deep-session agent changed — worktree content vs
    /// the session baseline ([`Self::collab_baseline`]). Rendered as a `✦` gutter badge
    /// so Pi's edits stand apart from the review's own changes.
    pi_marks: PiMarks,
    /// The Deep Review session baseline: a tree (session start) or commit (`U` update)
    /// everything the agent changed since is diffed against. `None` outside deep mode.
    pub collab_baseline: Option<String>,
    pub store: CommentStore,
    pub list_cursor: usize,
    pub mode: Mode,
    pub input: String,
    /// The comment editor's caret: a char index into `input` (`0..=chars().count()`).
    pub caret: usize,
    pub status: String,
    pub should_quit: bool,
    /// The `PR` tab's fetched view of the pull/merge request (`specs/forge-host.md`).
    pub pr: forge::PrView,
    /// Persistent same-input fetch remedy shown without replacing the visible snapshot.
    pr_notice: Option<String>,
    /// A same-input refresh that crossed the loading-indicator delay.
    pr_refreshing: bool,
    /// The PR navigator's cursor over its rows (checks then comments).
    pub(crate) pr_cursor: usize,
    /// Top visible line of the PR read pane, reset when the selected comment changes.
    pub(crate) pr_read_scroll: usize,
    /// Set when the PR view needs a (re)fetch; the event loop services it after drawing, so a
    /// `loading` frame shows before the blocking `gh` calls run.
    pub pr_pending: bool,
    /// The project-switcher overlay, when open (`ctrl-p` on any tab).
    pub switcher: Option<ProjectSwitcher>,
    /// A confirmed project pick, waiting for the event loop to rebuild the session on it.
    project_switch: Option<PathBuf>,
    /// The PR/MR picker overlay, when open (`p` on the PR tab).
    pub pr_picker: Option<PrPicker>,
    /// Live fuzzy-search input for the picker. Kept outside its loading/loaded lifecycle so the
    /// user can begin typing before a listing worker completes.
    pub pr_picker_query: String,
    /// A picked PR/MR pinned to the tab; cleared by `esc` or a branch switch.
    pub pr_pin: Option<PrPin>,
    /// The pinned review's remote Changes payload.
    pub remote_changes: RemoteChanges,
    /// Set when the event loop should fetch (or refetch) `remote_changes`.
    remote_changes_pending: bool,
    /// Locally composed forge comments/replies, retained across tab and review switches.
    pub remote_drafts: Vec<PendingRemoteDraft>,
    remote_compose: Option<RemoteCompose>,
    next_remote_draft_id: u64,
    remote_sync_pending: bool,
    remote_sync_active: Option<forge::ReviewSyncRequest>,
    /// The last probe's forge target and branch — what the picker lists and pins against.
    pub(crate) pr_context: Option<(crate::git::RepoTarget, Option<String>)>,
    /// A target-scoped picker listing, prefetched and reused for immediate opens.
    pr_listing_cache: Option<PrListingCache>,
    /// A listing request waiting for the event loop to spawn its worker.
    pr_listing_fetch: Option<PrListingRequest>,
    /// The newest listing worker whose result may populate the current target's cache.
    pr_listing_in_flight: Option<PrListingRequest>,
    /// Monotonic tag preventing an old A→B→A completion from replacing a newer A listing.
    pr_listing_generation: u64,
    highlighter: Highlighter,
    /// The active palette every renderer paints from (`specs/theme.md`).
    palette: Palette,
    /// The active theme's name, so re-resolving to the same theme is a no-op.
    theme_name: &'static str,
    /// The `--theme` override name (highest precedence); `None` lets the config file decide.
    cli_theme_name: Option<String>,
    /// The plugin is either ready with one validated snapshot or wholly blocked on its error.
    config: PluginConfigState,
    /// The last theme name requested, so re-resolving the same name skips work and logging.
    requested_theme_name: Option<String>,
    cache: DiffCache,
    /// The `last-turn` baseline lifecycle, driven by polling the agent's status.
    turn: TurnTracker,
    /// This worktree's key for the private baseline ref, fixed for the session.
    turn_key: String,
    /// Where each Pi-staged draft landed, by the extension's draft id, so a revision or a
    /// human edit can find it again.
    collab_refs: Vec<(String, CollabDraftRef)>,
    /// The collaboration link's state for the status surface: `None` before any host runs
    /// (plain tests), else whether a Pi extension holds an accepted link.
    pub collab_link: Option<bool>,
    /// The tray's alias chips, refreshed by the event loop whenever the tray changes.
    pub collab_tray: Vec<String>,
    /// Follow mode's state while a link is up (`None` without one), for the status surface.
    pub collab_follow: Option<bool>,
    /// Milliseconds left of the manual-navigation grace while it holds follow back.
    pub collab_grace_ms: Option<u64>,
    /// Pi's latest `path:line` while follow is off — aware without being moved.
    pub collab_pi_location: Option<String>,
    /// The 1-based edit-history position while browsing it.
    pub collab_history: Option<(usize, usize)>,
    /// The reviewer navigated on their own since the last drain (suppresses reads/searches).
    manual_navigated: bool,
    /// Carry the logical location across the next file-tab switch. A scope switch clears it
    /// once, so the explicit snap-to-top of a new scope wins over location preservation.
    carry_location: bool,
    /// Draft ids whose comments the reviewer edited; drained by the loop to transfer
    /// ownership in the collaboration session.
    pending_collab_edits: Vec<String>,
    /// Tray commands queued by key handlers; drained by the loop into the collaboration
    /// host, mirroring the request/take pattern of the fetch surfaces.
    pending_collab_intents: Vec<CollabIntent>,
    /// A `Shift+D` request waiting for the loop to orchestrate it.
    pending_deep: Option<DeepRequest>,
    /// The second `Shift+X` landed; the loop may execute the End.
    pending_end_confirmed: bool,
    /// A `U` update request waiting for the loop.
    pending_deep_update: bool,
    /// Draft/comment state changed by the reviewer; the deep loop persists on this.
    collab_touched: bool,
    /// Deep Review mode: this instance exclusively serves one collaboration target.
    pub deep: Option<DeepMode>,
    /// This target's drafting is owned by its Deep Review workspace; browsing stays free.
    pub deep_lockout: bool,
}

/// One `Shift+D` invocation, resolved to its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepRequest {
    /// The collaboration key (`github:…#N`, `gitlab:…!N`, or `local:<worktree>`).
    pub key: String,
    /// The remote review to materialize, when the target is not this local worktree.
    pub remote: Option<(crate::git::RepoTarget, u64)>,
}

/// The state a Deep Review instance carries about its exclusive target.
#[derive(Debug)]
pub struct DeepMode {
    pub key: String,
    pub remote: Option<(crate::git::RepoTarget, u64)>,
    /// The materialized review head (remote targets), for head-move detection.
    pub head: Option<String>,
    /// The private review branch materialization created, when it did.
    pub branch: Option<String>,
    pub created_worktree: bool,
    pub store: crate::collab::store::SessionStore,
    /// This process's ownership identity in the store.
    pub owner: String,
    /// `Shift+X` pressed once; the next press ends the session.
    pub end_armed: bool,
    /// A detected head move waiting for the explicit `U` update.
    pub head_moved: Option<String>,
}

/// One queued collaboration command from the key handlers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabIntent {
    /// Replace the tray with this item and focus Pi.
    Attach(crate::collab::session::TrayItem),
    /// Toggle this item's tray membership.
    Toggle(crate::collab::session::TrayItem),
    /// `f` — toggle follow mode.
    FollowToggle,
    /// Step backward through Pi's edit history.
    HistoryBack,
    /// Step forward through Pi's edit history.
    HistoryForward,
}

/// How far [`App::collab_navigate_to`] got; a miss lets the caller refresh and retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavOutcome {
    /// The viewer sits on the location (or the fold/nearest line standing in for it).
    Landed,
    /// The file is not in this scope's tree — perhaps not until the next reload.
    FileMissing,
    /// The file opened but the line is not representable in this projection.
    LineMissing,
}

/// Where one Pi-staged draft lives in the app's draft surfaces.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CollabDraftRef {
    /// An index into the local [`CommentStore`]; kept in step by the delete/export paths.
    LocalComment(usize),
    /// A [`PendingRemoteDraft`]'s `local_id`.
    RemoteDraft(u64),
}

#[derive(Debug)]
enum PluginConfigState {
    Ready(crate::config::PluginConfig),
    Blocked { error: String },
}

impl App {
    pub fn new(repo: PathBuf, scope: Scope, base: Option<String>) -> Self {
        Self::build(repo, scope, base, true)
    }

    /// Construct the error-only sidebar without reading repository state.
    pub(crate) fn blocked(repo: PathBuf, scope: Scope, base: Option<String>) -> Self {
        Self::build(repo, scope, base, false)
    }

    fn build(repo: PathBuf, scope: Scope, base: Option<String>, load_turn: bool) -> Self {
        // Resume any persisted turn baseline for this worktree, so `last-turn` keeps its
        // anchor across a sidebar restart (specs/herdr-host.md).
        let turn_key = git::worktree_key(&repo);
        let turn = if load_turn {
            TurnTracker::with_baseline(git::read_baseline_ref(&repo, &turn_key))
        } else {
            TurnTracker::default()
        };
        let theme = theme::resolve(None);
        Self {
            repo,
            base,
            scope,
            tab: Tab::Changes,
            active_file_tab: Tab::Changes,
            focus: Focus::Files,
            entries: Vec::new(),
            file_rows: Vec::new(),
            file_cursor: 0,
            file_scroll: 0,
            reveal_files: false,
            reveal_diff: false,
            resume_list: false,
            toggled_dirs: HashSet::new(),
            stash: TabStash::default(),
            changed: HashMap::new(),
            diff: FileDiff::empty(),
            visible: Vec::new(),
            expanded_folds: HashSet::new(),
            diff_path: None,
            diff_cursor: 0,
            diff_scroll: 0,
            h_scroll: 0,
            wrap: true,
            list_pct: DEFAULT_LIST_PCT,
            resizing: false,
            select_anchor: None,
            line_decorations: HashMap::new(),
            pi_marks: PiMarks::default(),
            collab_baseline: None,
            store: CommentStore::new(),
            list_cursor: 0,
            mode: Mode::Normal,
            input: String::new(),
            caret: 0,
            status: String::new(),
            should_quit: false,
            pr: forge::PrView::Pending,
            pr_notice: None,
            pr_refreshing: false,
            pr_cursor: 0,
            pr_read_scroll: 0,
            pr_pending: false,
            switcher: None,
            project_switch: None,
            pr_picker: None,
            pr_picker_query: String::new(),
            pr_pin: None,
            remote_changes: RemoteChanges::Idle,
            remote_changes_pending: false,
            remote_drafts: Vec::new(),
            remote_compose: None,
            next_remote_draft_id: 1,
            remote_sync_pending: false,
            remote_sync_active: None,
            pr_context: None,
            pr_listing_cache: None,
            pr_listing_fetch: None,
            pr_listing_in_flight: None,
            pr_listing_generation: 0,
            highlighter: Highlighter::new(theme.syntax),
            palette: theme.palette,
            theme_name: theme.name,
            cli_theme_name: None,
            config: PluginConfigState::Ready(crate::config::PluginConfig::default()),
            requested_theme_name: None,
            cache: DiffCache::new(),
            turn,
            turn_key,
            collab_refs: Vec::new(),
            collab_link: None,
            collab_tray: Vec::new(),
            collab_follow: None,
            collab_grace_ms: None,
            collab_pi_location: None,
            collab_history: None,
            manual_navigated: false,
            carry_location: true,
            pending_collab_edits: Vec::new(),
            pending_collab_intents: Vec::new(),
            pending_deep: None,
            collab_touched: false,
            pending_end_confirmed: false,
            pending_deep_update: false,
            deep: None,
            deep_lockout: false,
        }
    }

    /// Resolve `name` (a CLI or config value; `None` = default) and apply it when it changes:
    /// rebuild the highlighter and drop cached diffs so they re-render. Unknown or
    /// not-yet-supported names fall back to the default (`specs/theme.md`).
    fn set_theme(&mut self, name: Option<&str>) {
        // Re-resolving the same name every poll would redo derivation and re-log an unknown
        // name, so skip when the request is unchanged.
        if self.requested_theme_name.as_deref() == name {
            return;
        }
        self.requested_theme_name = name.map(str::to_owned);
        let theme = theme::resolve(name);
        if theme.name != self.theme_name {
            self.theme_name = theme.name;
            self.palette = theme.palette;
            self.highlighter = Highlighter::new(theme.syntax);
            self.cache = DiffCache::new();
        }
    }

    /// Record the `--theme` override name (highest precedence) and apply the resolved theme now.
    pub fn set_cli_theme(&mut self, name: Option<String>) {
        self.cli_theme_name = name;
        self.refresh_theme();
    }

    /// Apply one complete validated plugin configuration snapshot.
    pub(crate) fn set_plugin_config(&mut self, config: crate::config::PluginConfig) {
        self.config = PluginConfigState::Ready(config);
        self.refresh_theme();
    }

    /// The validated plugin configuration snapshot normal work currently uses.
    pub fn plugin_config(&self) -> Option<&crate::config::PluginConfig> {
        match &self.config {
            PluginConfigState::Ready(config) => Some(config),
            PluginConfigState::Blocked { .. } => None,
        }
    }

    /// Block the sidebar on one whole-file configuration failure.
    pub fn set_config_error(&mut self, error: String) {
        self.config = PluginConfigState::Blocked { error };
        self.pr_pending = false;
    }

    /// The error-only state rendered while plugin configuration is invalid.
    pub fn config_error(&self) -> Option<&str> {
        match &self.config {
            PluginConfigState::Ready(_) => None,
            PluginConfigState::Blocked { error, .. } => Some(error),
        }
    }

    /// Move user-authored review state into a freshly loaded app after config recovery. Saved
    /// comments always survive; an in-progress draft keeps the exact frozen diff it was written
    /// against, matching the ordinary refresh invariant.
    pub(crate) fn carry_authored_state_from(&mut self, old: &mut Self) {
        self.store = std::mem::take(&mut old.store);
        self.remote_drafts = std::mem::take(&mut old.remote_drafts);
        self.remote_compose = old.remote_compose.take();
        self.next_remote_draft_id = old.next_remote_draft_id;
        self.remote_sync_pending = old.remote_sync_pending;
        self.remote_sync_active = old.remote_sync_active.take();
        self.list_cursor = old.list_cursor;
        let old_mode = old.mode.clone();
        match old_mode {
            Mode::Normal => {}
            Mode::List | Mode::Composing { .. } => {
                self.scope = old.scope;
                self.tab = old.tab;
                self.active_file_tab = old.active_file_tab;
                self.focus = old.focus;
                self.entries = std::mem::take(&mut old.entries);
                self.file_rows = std::mem::take(&mut old.file_rows);
                self.file_cursor = old.file_cursor;
                self.file_scroll = old.file_scroll;
                self.reveal_files = old.reveal_files;
                self.reveal_diff = old.reveal_diff;
                self.changed = std::mem::take(&mut old.changed);
                self.diff = std::mem::take(&mut old.diff);
                self.visible = std::mem::take(&mut old.visible);
                self.expanded_folds = std::mem::take(&mut old.expanded_folds);
                self.diff_path = old.diff_path.take();
                self.diff_cursor = old.diff_cursor;
                self.diff_scroll = old.diff_scroll;
                self.h_scroll = old.h_scroll;
                self.select_anchor = old.select_anchor;
                self.line_decorations = std::mem::take(&mut old.line_decorations);
                self.resume_list = old.resume_list;
                self.toggled_dirs = std::mem::take(&mut old.toggled_dirs);
                self.stash = std::mem::take(&mut old.stash);
                self.wrap = old.wrap;
                self.list_pct = old.list_pct;
                self.mode = old.mode.clone();
                self.input = std::mem::take(&mut old.input);
                self.caret = old.caret;
            }
        }
    }

    fn config_snapshot(&self) -> &crate::config::PluginConfig {
        match &self.config {
            PluginConfigState::Ready(config) => config,
            PluginConfigState::Blocked { .. } => {
                unreachable!("normal work is gated while plugin configuration is invalid")
            }
        }
    }

    fn ensure_config_ready(&self) -> Result<()> {
        match &self.config {
            PluginConfigState::Ready(_) => Ok(()),
            PluginConfigState::Blocked { error } => {
                Err(anyhow::anyhow!("plugin configuration is invalid: {error}"))
            }
        }
    }

    /// Re-resolve the active theme from the CLI override or current validated snapshot.
    fn refresh_theme(&mut self) {
        let name = self
            .cli_theme_name
            .clone()
            .unwrap_or_else(|| self.config_snapshot().theme().to_owned());
        self.set_theme(Some(&name));
    }

    /// The active palette every renderer paints from (`specs/theme.md`).
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub fn composing(&self) -> bool {
        matches!(self.mode, Mode::Composing { .. })
    }

    /// The entry under the cursor when the cursor is on a file row; `None` on a directory
    /// row (or an empty list).
    pub fn current_entry(&self) -> Option<&Entry> {
        self.file_under_cursor_index().map(|i| &self.entries[i])
    }

    /// A directory's resting state in the active tab: `Changes` opens expanded, `All files`
    /// collapsed (specs/file-list.md).
    fn default_expanded(&self) -> bool {
        self.tab == Tab::Changes
    }

    /// The `entries` index of the file row under the cursor, or `None` on a directory row.
    fn file_under_cursor_index(&self) -> Option<usize> {
        self.file_rows.get(self.file_cursor).and_then(file_list::Row::file_index)
    }

    /// The visible-row index of the file at `path`, for restoring selection across a poll.
    fn file_row_of_path(&self, path: &str) -> Option<usize> {
        self.file_rows
            .iter()
            .position(|r| r.file_index().is_some_and(|i| self.entries[i].path == path))
    }

    /// The visible-row index of the first file row, the initial selection so a diff shows
    /// at once even when the tree opens on a directory.
    fn first_file_row(&self) -> Option<usize> {
        self.file_rows.iter().position(|r| r.file_index().is_some())
    }

    /// Rebuild the flattened tree from `entries` and the toggled-directory set.
    fn rebuild_file_rows(&mut self) {
        self.file_rows =
            file_list::build(&self.entries, &self.toggled_dirs, self.default_expanded());
    }

    /// What the cursor currently points at — a file (by path) or a directory (by path) — so
    /// the cursor can be put back on the same target after the tree rebuilds.
    fn cursor_anchor(&self) -> Option<Anchor> {
        self.file_rows.get(self.file_cursor).map(|r| match &r.kind {
            RowKind::File { index, .. } => Anchor::File(self.entries[*index].path.clone()),
            RowKind::Dir { path, .. } => Anchor::Dir(path.clone()),
        })
    }

    /// The visible-row index matching `anchor`, for restoring the cursor after a rebuild.
    fn row_of_anchor(&self, anchor: &Anchor) -> Option<usize> {
        self.file_rows.iter().position(|r| match (anchor, &r.kind) {
            (Anchor::File(p), RowKind::File { index, .. }) => &self.entries[*index].path == p,
            (Anchor::Dir(p), RowKind::Dir { path, .. }) => path == p,
            _ => false,
        })
    }

    /// The file whose diff the pane shows: the file under the cursor, or — when the cursor
    /// rests on a directory — the already-open file (matched by `diff_path`), so scanning the
    /// tree never blanks the diff. `None` only when nothing is open.
    fn shown_entry(&self) -> Option<Entry> {
        if let Some(e) = self.current_entry() {
            return Some(e.clone());
        }
        let open = self.diff_path.as_deref()?;
        self.entries.iter().find(|e| e.path == open).cloned()
    }

    /// Reload the changed-files list and (unless composing) the open diff.
    ///
    /// The `All files` entries: every worktree path (ignored dimmed), with the children of
    /// expanded ignored directories loaded lazily (`specs/file-list.md`). Only directories the
    /// user has expanded are walked, so the cost tracks what is on screen, not the whole tree.
    fn all_files_entries(&self) -> Result<Vec<Entry>> {
        let to_entry = |w: git::WorktreeEntry| Entry {
            annotation: self.changed.get(&w.path).cloned(),
            path: w.path,
            previous_path: None,
            ignored: w.ignored,
            is_dir: w.is_dir,
        };
        let mut entries: Vec<Entry> =
            git::all_files(&self.repo)?.into_iter().map(&to_entry).collect();
        let mut i = 0;
        while i < entries.len() {
            if entries[i].is_dir && self.toggled_dirs.contains(&entries[i].path) {
                let path = entries[i].path.clone();
                let children = git::list_ignored_dir(&self.repo, &path).into_iter().map(&to_entry);
                entries.extend(children);
            }
            i += 1;
        }
        Ok(entries)
    }

    /// Never touches the comment store or the in-progress input — that is the
    /// "a comment is never lost to a refresh" invariant (`specs/overview.md`).
    pub fn reload(&mut self) -> Result<()> {
        self.ensure_config_ready()?;
        // The PR tab holds its own state and renders nothing from the file tree, so a poll on
        // it skips the rebuild; switching back to a file tab reloads it then (specs/tui.md).
        if !self.tab.is_file_tab() {
            return Ok(());
        }
        // Outside a git repo, show an empty state rather than failing (herdr-host.md).
        if !git::is_repo(&self.repo) {
            self.entries.clear();
            self.changed.clear();
            self.file_rows.clear();
            self.file_cursor = 0;
            self.file_scroll = 0;
            if !self.composing() {
                self.diff = FileDiff::empty();
                self.diff_path = None;
                self.visible.clear(); // keep `visible` mirroring `diff` so no stale rows paint
                self.reset_diff_view();
            }
            return Ok(());
        }
        // Keep the cursor on the same row target across the rebuild; fall back to the open
        // file, then the first file. The toggled-directory set survives untouched.
        let anchor = self.cursor_anchor();
        let open = self.diff_path.clone();
        // The active scope's changeset, computed regardless of tab so the changed-file count
        // and comment staleness stay correct even while `All files` lists the whole worktree.
        // last-turn diffs the captured baseline; with none yet, it is empty until a turn start
        // is observed (specs/review-model.md).
        let changed = if self.tab == Tab::Changes && self.remote_changes_active() {
            // A selected remote review owns the Changes tab, including while its patch is
            // loading or failed. Scanning a large dirty worktree cannot populate that view and
            // only delays it. Switching to All files or unpinning reloads local annotations.
            Vec::new()
        } else {
            match self.scope {
                Scope::LastTurn => match self.turn.baseline() {
                    Some(t) => git::changed_against_tree(&self.repo, t)?,
                    None => Vec::new(),
                },
                _ => git::changed_files(
                    &self.repo,
                    self.scope,
                    self.base.as_deref(),
                    self.config_snapshot().base_branches(),
                )?,
            }
        };
        self.changed = changed.iter().map(|f| (f.path.clone(), Annotation::from(f))).collect();
        self.entries = match self.tab {
            // The whole worktree (ignored included), with expanded ignored dirs loaded lazily.
            Tab::AllFiles => self.all_files_entries()?,
            Tab::Changes => match &self.remote_changes {
                RemoteChanges::Ready { patch, .. } => {
                    patch.files.iter().map(|f| Entry::from_changed(&f.change)).collect()
                }
                RemoteChanges::Loading(_) | RemoteChanges::Failed { .. } => Vec::new(),
                RemoteChanges::Idle => changed.iter().map(Entry::from_changed).collect(),
            },
            Tab::Pr => unreachable!("PR returned before file reload"),
        };
        self.rebuild_file_rows();
        self.file_cursor = anchor
            .and_then(|a| self.row_of_anchor(&a))
            .or_else(|| open.as_deref().and_then(|p| self.file_row_of_path(p)))
            .or_else(|| self.first_file_row())
            .unwrap_or(0)
            .min(self.file_rows.len().saturating_sub(1));
        // A poll preserves the file-list wheel scroll — it does not reveal the cursor.
        // Explicit actions (navigation, a scope switch) request their own reveal.
        // While a modal is open — composing a comment, or the comments-list overlay — the
        // open diff is frozen, so a poll can't shift the anchor beneath the writer or reset
        // the scroll/selection under the overlay. The file list still updates above.
        if !self.composing() && self.mode != Mode::List {
            // A poll keeps the reader on the same file; only a different shown file resets
            // the diff view to the top.
            if self.shown_entry().map(|e| e.path) != self.diff_path {
                self.reset_diff_view();
            }
            self.load_left();
        }
        Ok(())
    }

    /// Load the left pane for the active tab: the scope diff in `Changes`, the whole-file
    /// content in `All files`. Both flatten into `visible` and settle the cursor/scroll.
    fn load_left(&mut self) {
        let Some(entry) = self.shown_entry() else {
            self.diff = FileDiff::empty();
            self.diff_path = None;
            self.visible.clear();
            self.line_decorations.clear();
            self.reset_diff_view();
            return;
        };
        self.open_path_in_tab(entry.path, entry.previous_path);
    }

    /// Open `path` in the active tab's left pane: the scope diff in `Changes` (rename-aware via
    /// `previous_path`), the whole-file content in `All files`. The one place this dispatch lives,
    /// so opening a file from the tree and from a comment edit can't drift apart.
    fn open_path_in_tab(&mut self, path: String, previous_path: Option<String>) {
        match self.tab {
            Tab::AllFiles => self.set_file_view(path),
            // `Changes` (the `PR` tab never opens a file in the left pane).
            _ => self.set_diff(path, previous_path),
        }
    }

    /// Build the diff for a specific `path` regardless of whether its row is visible in the
    /// tree — so editing a comment can surface its file even from a collapsed directory.
    fn set_diff(&mut self, path: String, previous_path: Option<String>) {
        // A different file opens with all folds collapsed. `expanded_folds` is keyed by line
        // number, so without this a fold in the new file whose first hidden line matches an
        // expanded one in the old file would render pre-expanded. A same-file poll keeps them.
        if self.diff_path.as_deref() != Some(path.as_str()) {
            self.expanded_folds.clear();
        }
        self.diff_path = Some(path.clone());
        let remote = match &self.remote_changes {
            RemoteChanges::Ready { patch, .. } => {
                patch.files.iter().find(|file| file.change.path == path).cloned()
            }
            _ => None,
        };
        // Pi marks only apply to diffs built from worktree content: a pinned remote
        // patch is the forge's text, whose line numbers drift from the worktree as the
        // agent edits — and the agent's uncommitted work is not in that patch at all.
        self.pi_marks =
            if remote.is_none() { self.pi_marks_for(&path) } else { PiMarks::default() };
        self.diff = if let Some(file) = remote {
            FileDiff::from_patch(&file, &self.highlighter)
        } else {
            let (old, new) = self.content_sides(&path, previous_path.as_deref());
            self.cache.get(path, previous_path, &old, &new, &self.highlighter)
        };
        self.line_decorations.clear();
        self.rebuild_visible();
        self.settle_left();
    }

    /// Build the File view for `path`: its current worktree content as `Context` rows, no
    /// folds. The `All files` left pane (specs/diff-view.md). Content is scope-independent.
    fn set_file_view(&mut self, path: String) {
        self.diff_path = Some(path.clone());
        self.expanded_folds.clear(); // the File view has no folds
        // Check the on-disk size before reading: an over-budget blob (a model weight, a vendored
        // bundle) is one keystroke away in `All files`, and reading it whole would spike the UI
        // thread before `build_file`'s budget could discard it (specs/diff-view.md).
        let oversize = std::fs::metadata(self.repo.join(&path))
            .is_ok_and(|m| crate::diff::over_byte_budget(m.len() as usize));
        self.line_decorations = if self.changed.contains_key(&path) {
            let (old, current) = self.content_sides(&path, None);
            crate::diff::line_decorations(&old, &current)
        } else {
            HashMap::new()
        };
        self.pi_marks = self.pi_marks_for(&path);
        self.diff = if oversize {
            FileDiff::too_large_notice(path)
        } else {
            let content = worktree_content(&self.repo, &path);
            self.cache.get_file(path, &content, &self.highlighter)
        };
        self.rebuild_visible();
        self.settle_left();
    }

    /// Clamp the cursor, scroll, and selection to the rebuilt `visible`, keeping the reader's
    /// position. A shrunk view that forced the cursor to move reveals it; a poll that left it
    /// in range does not, so a wheel scroll survives.
    fn settle_left(&mut self) {
        if self.visible.is_empty() {
            self.reset_diff_view();
            return;
        }
        let last = self.visible.len() - 1;
        let clamped = self.diff_cursor.min(last);
        if clamped != self.diff_cursor {
            self.reveal_diff = true;
        }
        self.diff_cursor = clamped;
        self.diff_scroll = self.diff_scroll.min(last);
        self.select_anchor = self.select_anchor.map(|a| a.min(last));
    }

    /// Flatten `diff.rows` into `visible`: an expanded fold becomes its lines, a
    /// collapsed fold stays a single marker row.
    fn rebuild_visible(&mut self) {
        self.visible = self
            .diff
            .rows
            .iter()
            .flat_map(|row| match row {
                Row::Fold { lines }
                    if row.fold_anchor().is_some_and(|a| self.expanded_folds.contains(&a)) =>
                {
                    lines.clone()
                }
                _ => vec![row.clone()],
            })
            .collect();
    }

    /// Expand the fold under the cursor, revealing its hidden lines. Expansion is
    /// permanent for the session — an expand is taken as intentional, so there is no
    /// collapse-back.
    /// Expand the fold under the cursor, keeping the viewport visually still. Where the fold
    /// sits decides which way it grows: a fold in the top half of the diff expands upward (the
    /// lines below it hold their screen position); one in the bottom half expands downward (the
    /// lines above hold theirs). `heights`/`viewport` are this frame's pre-expand diff geometry.
    pub fn expand_fold(&mut self, heights: &[usize], viewport: usize) {
        let fold_idx = self.diff_cursor;
        let Some(anchor) = self.visible.get(fold_idx).and_then(Row::fold_anchor) else {
            return;
        };
        // Expanding replaces the 1 fold row with N context rows; rows below it shift by N-1.
        let shift = self.visible[fold_idx].hidden().saturating_sub(1);
        // Display rows between the viewport top and the fold; < half ⇒ top half. When the fold
        // is wheeled above the viewport (fold_idx < diff_scroll), the range is empty → above 0 →
        // top half, which is correct: the inserted rows land above the viewport, so advancing
        // diff_scroll by `shift` holds the visible content in place.
        let above: usize = heights.get(self.diff_scroll..fold_idx).map_or(0, |s| s.iter().sum());
        let top_half = above < viewport / 2;
        self.expanded_folds.insert(anchor);
        self.rebuild_visible();
        if top_half {
            self.diff_scroll += shift; // hold the content below the fold; grow upward
        }
        // bottom half: leave diff_scroll — the content above the fold stays put, grow downward
    }

    /// The old and new content of `file` for the current scope: old from `HEAD` (or the
    /// merge-base on the branch scope), new from the worktree. A rename reads its old side
    /// from `previous_path`, so the diff shows real edits, not a wholesale delete-and-add.
    fn content_sides(&self, path: &str, previous_path: Option<&str>) -> (String, String) {
        let new_path = path;
        let old_path = previous_path.unwrap_or(new_path);
        match self.scope {
            Scope::Uncommitted => {
                let old = git::file_content(&self.repo, "HEAD", old_path);
                let new = worktree_content(&self.repo, new_path);
                (old, new)
            }
            Scope::Branch => {
                let mb = git::merge_base(
                    &self.repo,
                    self.base.as_deref(),
                    self.config_snapshot().base_branches(),
                );
                let old =
                    mb.map(|m| git::file_content(&self.repo, &m, old_path)).unwrap_or_default();
                (old, worktree_content(&self.repo, new_path))
            }
            Scope::LastTurn => {
                let old = self
                    .turn
                    .baseline()
                    .map(|b| git::file_content(&self.repo, b, old_path))
                    .unwrap_or_default();
                (old, worktree_content(&self.repo, new_path))
            }
        }
    }

    /// Whether the `last-turn` scope is active but no baseline has been captured yet — the
    /// cold-start (or no-herdr) state the UI paints as `waiting for the agent's next turn`.
    pub fn awaiting_turn(&self) -> bool {
        self.scope == Scope::LastTurn && !self.turn.has_baseline()
    }

    /// Sample the agent's status and advance the `last-turn` baseline. Reads the resolved
    /// agent's status over the herdr CLI; absence or ambiguity pauses tracking. Never
    /// propagates — a missing herdr is normal, so failures only log. Returns whether this
    /// sample ended a turn (the agent went idle after acting), the `PR` tab's refetch signal.
    pub fn track_turn(&mut self) -> bool {
        if self.plugin_config().is_none() {
            return false;
        }
        let status = crate::herdr::resolved_agent_status().ok().flatten();
        self.apply_agent_status(status.as_deref())
    }

    /// Advance the baseline from one status sample — the core [`track_turn`](Self::track_turn)
    /// wraps, and the seam tests drive without herdr. On a turn start (a resting→`working`
    /// edge) it snapshots the worktree as the candidate; while a candidate is pending it
    /// promotes once the worktree diverges from it, persisting the new baseline. Git errors
    /// only log, so a transient git failure never crashes the poll. Returns whether this
    /// sample ended a turn (a `working`→resting edge), the `PR` tab's refetch signal.
    pub fn apply_agent_status(&mut self, status: Option<&str>) -> bool {
        if self.plugin_config().is_none() {
            return false;
        }
        let Some(status) = status else { return false };
        let parsed = Status::parse(status);
        // Read the turn-end edge before `observe` advances `prev`.
        let ended = self.turn.ends_turn(parsed);
        if self.turn.observe(parsed) {
            match git::snapshot_worktree(&self.repo) {
                Ok(sha) => self.turn.set_candidate(sha),
                Err(e) => logln!("turn snapshot failed: {e}"),
            }
        }
        // Promote the pending candidate once the turn has changed a file. Compare full
        // snapshots so a new untracked file counts as a change (specs/herdr-host.md).
        let Some(candidate) = self.turn.candidate().map(str::to_string) else { return ended };
        match git::snapshot_worktree(&self.repo) {
            Ok(now) if now != candidate => {
                self.turn.promote();
                if let Err(e) = git::write_baseline_ref(&self.repo, &self.turn_key, &candidate) {
                    logln!("turn baseline ref write failed: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => logln!("turn divergence check failed: {e}"),
        }
        ended
    }

    /// Snap the diff view back to the top, clearing any pending selection.
    fn reset_diff_view(&mut self) {
        self.diff_cursor = 0;
        self.diff_scroll = 0;
        self.h_scroll = 0;
        self.select_anchor = None;
    }

    /// Scroll the diff horizontally by `delta` columns, clamped at the left edge. A no-op
    /// while wrap is on, since the renderer ignores `h_scroll` when wrapping — so the offset
    /// never silently accumulates and then jumps the view when wrap is toggled off.
    pub fn scroll_h(&mut self, delta: isize) {
        if self.wrap {
            return;
        }
        self.h_scroll = if delta >= 0 {
            self.h_scroll + delta as usize
        } else {
            self.h_scroll.saturating_sub(delta.unsigned_abs())
        };
    }

    /// Toggle line wrap; reset the horizontal scroll, which only applies with wrap off.
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.h_scroll = 0;
    }

    /// Widen (`+`) or narrow (`-`) the file-list pane by `delta` percent, clamped so neither
    /// pane collapses. Bound to `]` / `[`.
    pub fn resize_list(&mut self, delta: i16) {
        let next = (self.list_pct as i16 + delta).clamp(MIN_LIST_PCT as i16, MAX_LIST_PCT as i16);
        self.list_pct = next as u16;
    }

    /// Set the file-list width so the divider sits at body column `x` (a mouse drag). `x` is
    /// measured from the body's left edge; the list spans from there to the right edge.
    pub fn drag_divider(&mut self, body_width: u16, x: u16) {
        if body_width == 0 {
            return;
        }
        let list_cols = body_width.saturating_sub(x.min(body_width));
        let pct = (u32::from(list_cols) * 100 / u32::from(body_width)) as u16;
        self.list_pct = pct.clamp(MIN_LIST_PCT, MAX_LIST_PCT);
    }

    // --- Scroll model (shared by both panes) ---------------------------------------
    //
    // Each pane has a cursor (selection) and a scroll offset (viewport top). They are
    // independent: keyboard navigation moves the cursor and requests a reveal; the wheel
    // moves the offset and requests nothing. Every frame the event loop reveals the cursor
    // *only if a move requested it* (so the wheel can leave the cursor off screen) and then
    // bounds the offset (so an over-scroll never shows a blank tail). Both panes run the
    // same `keep_in_view` + `bound`; the file list passes all-height-1 rows.

    /// Scroll the file list so `file_cursor` is on screen — the minimal nudge. Called once
    /// per frame when a navigation requested a reveal, not on a wheel scroll.
    pub fn reveal_file_cursor(&mut self, viewport: usize) {
        if self.file_rows.is_empty() {
            self.file_scroll = 0;
            return;
        }
        let cursor = self.file_cursor.min(self.file_rows.len() - 1);
        let heights = vec![1usize; self.file_rows.len()];
        self.file_scroll = keep_in_view(cursor, self.file_scroll, &heights, viewport);
    }

    /// Clamp `file_scroll` within range (no blank tail). Called every frame.
    pub fn bound_file_scroll(&mut self, viewport: usize) {
        self.file_scroll = bound(self.file_scroll, self.file_rows.len(), viewport);
    }

    /// Scroll the diff so `diff_cursor`'s row fits the `viewport`-display-row window —
    /// `heights` is each visible row's display height (wrap + comment cards). Called once
    /// per frame when a navigation requested a reveal, not on a wheel scroll.
    pub fn reveal_diff_cursor(&mut self, heights: &[usize], viewport: usize) {
        if self.visible.is_empty() {
            self.diff_scroll = 0;
            return;
        }
        let cursor = self.diff_cursor.min(self.visible.len() - 1);
        self.diff_scroll = keep_in_view(cursor, self.diff_scroll, heights, viewport);
    }

    /// Clamp `diff_scroll` within range (no blank tail). Called every frame. Height-aware:
    /// the cap is the offset that shows the LAST row at the bottom — computed from `heights`,
    /// not the row count, so a wrapped diff (tall rows) stays fully reachable. A row-count cap
    /// would stop short of the bottom whenever rows span more than one display line.
    pub fn bound_diff_scroll(&mut self, heights: &[usize], viewport: usize) {
        if heights.is_empty() {
            self.diff_scroll = 0;
            return;
        }
        let max_top = keep_in_view(heights.len() - 1, self.diff_scroll, heights, viewport);
        self.diff_scroll = self.diff_scroll.min(max_top);
    }

    /// Switch the changeset scope and reload. A no-op while composing, so a comment
    /// in progress is never stranded against a different diff.
    pub fn set_scope(&mut self, scope: Scope) -> Result<()> {
        self.ensure_config_ready()?;
        if self.scope != scope
            && !self.composing()
            && !(self.tab == Tab::Changes && self.remote_changes_active())
        {
            self.scope = scope;
            // The next tab switch must not carry the old scope's location over the
            // explicit snap-to-top below.
            self.carry_location = false;
            // A scope switch changes the Changes changeset (and each file's old side), so the
            // Changes tab snaps to the top of the new scope: reset its cursor, folds, and diff
            // scroll, and drop cached diffs. The `All files` listing and File view are
            // scope-independent (only the annotations move), so its own state is held by `reload`.
            // The Changes state is the active one on `Changes` and the stashed one while `All
            // files` is shown — reset whichever holds it, so a return to Changes never lands on a
            // stale scroll or a pre-expanded fold.
            self.cache = DiffCache::new();
            if self.tab == Tab::Changes {
                self.file_cursor = 0;
                self.expanded_folds.clear();
                self.reset_diff_view();
            } else {
                self.stash.file_cursor = 0;
                self.stash.expanded_folds.clear();
                self.stash.diff_cursor = 0;
                self.stash.diff_scroll = 0;
                self.stash.h_scroll = 0;
                self.stash.select_anchor = None;
            }
            self.reload()?;
            // An explicit switch reveals the cursor (a poll, which also calls reload, does not).
            self.reveal_files = true;
        }
        Ok(())
    }

    /// Switch to `tab`, saving the active tab's navigation and left-pane state and restoring the
    /// target's, then reloading it against the current worktree. Each tab keeps its own opened
    /// file and scroll, so returning to a tab lands exactly where you left it (specs/tui.md). A
    /// no-op on the active tab or while composing; focus stays on the same side.
    pub fn set_tab(&mut self, tab: Tab) -> Result<()> {
        self.ensure_config_ready()?;
        if self.tab == tab || self.composing() {
            return Ok(());
        }
        self.tab = tab;
        // Entering the PR tab leaves the file tabs frozen in place and fetches the PR. A
        // `loading` frame draws before the blocking fetch the event loop services, and a
        // re-entry keeps the last snapshot on screen while it refetches.
        if tab == Tab::Pr {
            self.pr_pending = true;
            return Ok(());
        }
        // Entering a file tab: bring its state into the diff fields if the other file tab holds
        // them (a Changes↔AllFiles switch, or a return from PR onto the stashed tab). The
        // logical review location survives the switch: both file tabs are projections of one
        // location, so `1`/`2` re-land on the same file and line rather than the stashed spot.
        let location = std::mem::replace(&mut self.carry_location, true)
            .then(|| self.current_location())
            .flatten();
        if self.active_file_tab != tab {
            self.swap_active_with_stash();
            self.active_file_tab = tab;
        }
        self.reload()?;
        if let Some((path, side, line)) = location {
            self.restore_location(&path, side, line);
        }
        // An empty left pane — a first visit landing on a collapsed tree, or an open file gone
        // empty — focuses the tree, so the cursor keys aren't trapped on a pane with nothing to
        // move (specs/tui.md).
        if self.visible.is_empty() {
            self.focus = Focus::Files;
        }
        self.reveal_files = true; // pull the restored cursor back into view
        Ok(())
    }

    /// The logical review location under the diff cursor, independent of the active tab's
    /// projection: `(path, side, line)`.
    pub fn current_location(&self) -> Option<(String, Side, u32)> {
        let path = self.diff_path.clone()?;
        let row = self.visible.get(self.diff_cursor)?;
        match (row.new_no(), row.old_no()) {
            (Some(line), _) => Some((path, Side::New, line)),
            (None, Some(line)) => Some((path, Side::Old, line)),
            _ => None,
        }
    }

    /// Re-land on a logical location inside the active tab's projection. When the
    /// projection cannot represent the line — outside the change set on `Changes`, or a
    /// removed line on `All files` — the file stays selected and an explicit notice says
    /// why, rather than jumping to an unrelated row.
    fn restore_location(&mut self, path: &str, side: Side, line: u32) {
        if self.diff_path.as_deref() != Some(path) {
            let Some(entry) = self.entries.iter().find(|e| e.path == path).cloned() else {
                if self.tab == Tab::Changes {
                    self.status = format!("{path} has no changes in this scope");
                }
                return;
            };
            self.reset_diff_view();
            self.open_path_in_tab(entry.path, entry.previous_path);
            if let Some(fi) = self.file_row_of_path(path) {
                self.file_cursor = fi;
            }
        }
        let found = self.visible.iter().position(|row| match side {
            Side::New => row.new_no() == Some(line),
            Side::Old => row.old_no() == Some(line),
        });
        if let Some(idx) = found {
            self.diff_cursor = idx;
            self.select_anchor = None;
            self.reveal_diff = true;
            return;
        }
        // The line may sit inside a collapsed fold: land on the fold so `→ expand` reaches it.
        if let Some(idx) = self.visible.iter().position(|row| {
            row.fold_anchor().is_some_and(|anchor| {
                anchor <= line && (line as usize) < anchor as usize + row.hidden()
            })
        }) {
            self.diff_cursor = idx;
            self.reveal_diff = true;
            return;
        }
        self.status = match (self.tab, side) {
            (Tab::Changes, _) => format!("{path}:{line} is outside this change set"),
            (_, Side::Old) => format!("{path}:{line} is a removed line — see Changes"),
            _ => format!("{path}:{line} is not in this view"),
        };
    }

    // ---- PR tab (specs/forge-host.md, specs/tui.md) -------------------------------------

    /// Clear a snapshot whose complete fetch input no longer matches the worktree.
    pub fn clear_pr(&mut self) {
        self.pr = forge::PrView::Pending;
        self.pr_notice = None;
        self.pr_refreshing = false;
        self.pr_cursor = 0;
        self.pr_read_scroll = 0;
    }

    // --- PR/MR picker and pin (specs/forge-host.md "Picker") -------------------------

    /// Whether the Changes tab is currently owned by a picker-selected remote review.
    pub fn remote_changes_active(&self) -> bool {
        !matches!(self.remote_changes, RemoteChanges::Idle)
    }

    /// The review label used by the Changes scope chip, e.g. `MR !42`.
    pub fn remote_changes_label(&self) -> Option<String> {
        let request = self.remote_changes.request()?;
        Some(format!(
            "{} {}{}",
            request.target.provider.unit(),
            request.target.provider.number_prefix(),
            request.number
        ))
    }

    pub fn remote_changes_truncated(&self) -> bool {
        matches!(&self.remote_changes, RemoteChanges::Ready { patch, .. } if patch.truncated)
    }

    /// The loading/failure text for an empty remote Changes view.
    pub fn remote_changes_notice(&self) -> Option<&str> {
        match &self.remote_changes {
            RemoteChanges::Loading(_) => Some("loading remote diff…"),
            RemoteChanges::Failed { message, .. } => Some(message),
            RemoteChanges::Ready { patch, .. } if patch.files.is_empty() => {
                Some("this review has no changed files")
            }
            _ => None,
        }
    }

    /// Take one due remote-diff request. The request stays in state as the stale-result tag.
    pub fn take_remote_changes_fetch(&mut self) -> Option<forge::ReviewDiffRequest> {
        if !self.remote_changes_pending {
            return None;
        }
        self.remote_changes_pending = false;
        self.remote_changes.request().cloned()
    }

    /// Apply a worker result only while it still names the selected review.
    pub fn remote_changes_loaded(
        &mut self,
        request: forge::ReviewDiffRequest,
        result: Result<crate::diff::PatchSet, String>,
    ) -> Result<()> {
        if self.remote_changes.request() != Some(&request) {
            return Ok(());
        }
        self.remote_changes = match result {
            Ok(patch) => RemoteChanges::Ready { request, patch },
            Err(message) => RemoteChanges::Failed { request, message },
        };
        if self.tab == Tab::Changes {
            self.reload()?;
            self.reveal_files = true;
        }
        Ok(())
    }

    /// Refetch the selected review's diff; local scopes use ordinary [`Self::reload`].
    pub fn refresh_remote_changes(&mut self) {
        let Some(current) = self.remote_changes.request().cloned() else { return };
        let request = forge::ReviewDiffRequest::new(current.target, current.number);
        let state = std::mem::replace(&mut self.remote_changes, RemoteChanges::Idle);
        self.remote_changes = match state {
            RemoteChanges::Ready { patch, .. } => RemoteChanges::Ready { request, patch },
            RemoteChanges::Loading(_) | RemoteChanges::Failed { .. } | RemoteChanges::Idle => {
                RemoteChanges::Loading(request)
            }
        };
        self.remote_changes_pending = true;
    }

    pub fn request_remote_sync(&mut self) {
        if self.deep_guard() {
            return;
        }
        if self.remote_sync_active.is_some() {
            self.status = "remote review sync already running".to_string();
        } else if self.syncable_remote_draft_count() == 0 {
            self.status = if self.remote_draft_count() > 0 {
                "draft outcome unknown — verify on the forge before creating a replacement"
                    .to_string()
            } else {
                "no remote review drafts to sync".to_string()
            };
        } else if self.active_review_snapshot().is_none() {
            self.status =
                "wait for this review's current commit details before syncing".to_string();
        } else {
            self.remote_sync_pending = true;
            self.status = "syncing remote review drafts…".to_string();
        }
    }

    pub fn take_remote_sync(&mut self) -> Option<forge::ReviewSyncRequest> {
        if !self.remote_sync_pending || self.remote_sync_active.is_some() {
            return None;
        }
        self.remote_sync_pending = false;
        let (target, number) = self.active_review_target()?;
        let diff_refs = self.active_review_snapshot()?.diff_refs.clone();
        let drafts: Vec<forge::ReviewDraft> = self
            .remote_drafts
            .iter_mut()
            .filter(|pending| {
                pending.target == target && pending.number == number && !pending.outcome_unknown
            })
            .map(|pending| {
                pending.error = None;
                pending.draft.clone()
            })
            .collect();
        let request = forge::ReviewSyncRequest::new(target, number, diff_refs, drafts);
        self.remote_sync_active = Some(request.clone());
        Some(request)
    }

    pub fn remote_sync_finished(
        &mut self,
        request: &forge::ReviewSyncRequest,
        outcome: &forge::ReviewSyncOutcome,
    ) {
        if self.remote_sync_active.as_ref() != Some(request) {
            return;
        }
        self.remote_sync_active = None;
        let success: HashSet<u64> = outcome.succeeded.iter().copied().collect();
        self.remote_drafts.retain(|pending| !success.contains(&pending.draft.local_id));
        for (id, error) in &outcome.failed {
            if let Some(pending) =
                self.remote_drafts.iter_mut().find(|pending| pending.draft.local_id == *id)
            {
                pending.error = Some(error.clone());
            }
        }
        for (id, error) in &outcome.uncertain {
            if let Some(pending) =
                self.remote_drafts.iter_mut().find(|pending| pending.draft.local_id == *id)
            {
                pending.error = Some(error.clone());
                pending.outcome_unknown = true;
            }
        }
        self.status = format!(
            "remote review sync: {} sent · {} failed · {} unknown",
            outcome.succeeded.len(),
            outcome.failed.len(),
            outcome.uncertain.len()
        );
        if !outcome.succeeded.is_empty() || !outcome.uncertain.is_empty() {
            self.pr_pending = true;
            self.refresh_remote_changes();
        }
    }

    fn clear_remote_changes(&mut self) {
        self.remote_changes = RemoteChanges::Idle;
        self.remote_changes_pending = false;
    }

    /// Drop any rows from the old remote source before rebuilding local Changes. If the reload
    /// fails, the pane stays blank rather than exposing stale remote lines with authoring enabled.
    fn restore_local_changes(&mut self) -> Result<()> {
        if self.tab != Tab::Changes {
            return Ok(());
        }
        self.entries.clear();
        self.file_rows.clear();
        self.file_cursor = 0;
        self.file_scroll = 0;
        self.diff = FileDiff::empty();
        self.diff_path = None;
        self.visible.clear();
        self.reset_diff_view();
        self.reload()?;
        self.reveal_files = true;
        Ok(())
    }

    /// Reconcile the pin against a fresh probe input: inject the pinned number when the
    /// branch still matches, drop the pin on a branch switch (a new review seat), and record
    /// the probe's forge target as the picker's context.
    pub fn reconcile_pr_pin(&mut self, input: &mut forge::PrFetchInput) {
        let context = match &input.origin {
            crate::git::OriginIdentity::Repository(target) => {
                Some((target.clone(), input.branch.clone()))
            }
            _ => None,
        };
        let old_target = self.pr_context.as_ref().map(|(target, _)| target);
        let new_target = context.as_ref().map(|(target, _)| target);
        if old_target != new_target {
            self.pr_listing_cache = None;
            self.pr_listing_fetch = None;
            self.pr_listing_in_flight = None;
            // A visible listing belongs to its old repository. Close it before swapping context so
            // Enter can never reinterpret an old review number against the new target.
            self.close_pr_picker();
        }
        self.pr_context = context;
        if let Some(target) = self.pr_context.as_ref().map(|(target, _)| target.clone()) {
            self.schedule_pr_listing(target);
        }
        if let Some(pin) = &self.pr_pin {
            if pin.branch == input.branch {
                input.pinned = Some(pin.number);
            } else {
                self.pr_pin = None;
                self.clear_remote_changes();
                if let Err(error) = self.restore_local_changes() {
                    self.status = format!("local Changes reload failed: {error}");
                }
            }
        }
    }

    fn schedule_pr_listing(&mut self, target: crate::git::RepoTarget) {
        if self.pr_listing_cache.as_ref().is_some_and(|cache| cache.is_fresh_for(&target))
            || self.pr_listing_fetch.as_ref().is_some_and(|request| request.target == target)
            || self.pr_listing_in_flight.as_ref().is_some_and(|request| request.target == target)
        {
            return;
        }
        self.pr_listing_generation = self.pr_listing_generation.wrapping_add(1);
        self.pr_listing_fetch =
            Some(PrListingRequest { target, generation: self.pr_listing_generation });
    }

    /// Open the picker (`p` on the PR tab). A fresh or stale target-matched cache paints
    /// immediately; a missing cache keeps the existing loading state while the prefetch lands.
    pub fn open_pr_picker(&mut self) {
        if self.pr_picker.is_some() {
            return;
        }
        let Some(target) = self.pr_context.as_ref().map(|(target, _)| target.clone()) else {
            self.pr_picker =
                Some(PrPicker::Failed("no forge target yet — wait for the first probe".into()));
            return;
        };
        self.pr_picker_query.clear();
        if let Some(cache) = self.pr_listing_cache.as_ref().filter(|cache| cache.target == target) {
            let listing = cache.listing.clone();
            let filtered = filtered_pr_indices(&listing, &self.pr_picker_query);
            self.pr_picker = Some(PrPicker::Loaded { listing, filtered, cursor: 0 });
            if !cache.is_fresh_for(&target) {
                self.schedule_pr_listing(target);
            }
        } else {
            self.pr_picker = Some(PrPicker::Loading);
            self.schedule_pr_listing(target);
        }
    }

    pub fn close_pr_picker(&mut self) {
        self.pr_picker = None;
        self.pr_picker_query.clear();
    }

    /// Take one target-tagged listing request. Moving it to `in_flight` deduplicates picker
    /// opens and periodic probes while the worker is running.
    pub fn take_pr_picker_fetch(&mut self) -> Option<PrListingRequest> {
        let request = self.pr_listing_fetch.take()?;
        self.pr_listing_in_flight = Some(request.clone());
        Some(request)
    }

    pub fn pr_listing_fetch_active(&self) -> bool {
        self.pr_listing_fetch.is_some() || self.pr_listing_in_flight.is_some()
    }

    /// Deliver a target-tagged listing. A completion for an old project is ignored; a useful
    /// completion populates the cache even when the picker is closed, making the next open free.
    pub fn pr_picker_loaded(
        &mut self,
        request: PrListingRequest,
        result: Result<forge::PrListing, String>,
    ) {
        if self.pr_listing_in_flight.as_ref() != Some(&request) {
            return;
        }
        self.pr_listing_in_flight = None;
        if self.pr_context.as_ref().map(|(current, _)| current) != Some(&request.target) {
            return;
        }
        match result {
            Ok(listing) => {
                self.pr_listing_cache = Some(PrListingCache {
                    target: request.target,
                    listing: listing.clone(),
                    loaded_at: Instant::now(),
                });
                match &mut self.pr_picker {
                    Some(PrPicker::Loading) => {
                        let filtered = filtered_pr_indices(&listing, &self.pr_picker_query);
                        self.pr_picker = Some(PrPicker::Loaded { listing, filtered, cursor: 0 });
                    }
                    Some(PrPicker::Loaded { listing: shown, filtered, cursor }) => {
                        let selected_number = filtered
                            .get(*cursor)
                            .and_then(|&index| PrPicker::row(shown, index))
                            .map(|item| item.number);
                        *shown = listing;
                        *filtered = filtered_pr_indices(shown, &self.pr_picker_query);
                        *cursor = selected_number
                            .and_then(|number| {
                                filtered.iter().position(|&index| {
                                    PrPicker::row(shown, index)
                                        .is_some_and(|item| item.number == number)
                                })
                            })
                            .unwrap_or_else(|| (*cursor).min(filtered.len().saturating_sub(1)));
                    }
                    Some(PrPicker::Failed(_)) | None => {}
                }
            }
            Err(message) if self.pr_picker == Some(PrPicker::Loading) => {
                self.pr_picker = Some(PrPicker::Failed(message));
            }
            Err(_) => {}
        }
    }

    pub fn pr_picker_input(&mut self, character: char) {
        if self.pr_picker.is_none() {
            return;
        }
        self.pr_picker_query.push(character);
        self.refilter_pr_picker();
    }

    pub fn pr_picker_backspace(&mut self) {
        if self.pr_picker.is_none() {
            return;
        }
        self.pr_picker_query.pop();
        self.refilter_pr_picker();
    }

    fn refilter_pr_picker(&mut self) {
        if let Some(PrPicker::Loaded { listing, filtered, cursor }) = &mut self.pr_picker {
            *filtered = filtered_pr_indices(listing, &self.pr_picker_query);
            *cursor = 0;
        }
    }

    pub fn pr_picker_move(&mut self, delta: isize) {
        if let Some(PrPicker::Loaded { filtered, cursor, .. }) = &mut self.pr_picker
            && !filtered.is_empty()
        {
            *cursor = cursor.saturating_add_signed(delta).min(filtered.len() - 1);
        }
    }

    /// Pin the highlighted filtered row (`enter` in the picker) and refetch the tab for it.
    pub fn pr_picker_select(&mut self) {
        let Some(PrPicker::Loaded { listing, filtered, cursor }) = &self.pr_picker else {
            return;
        };
        let Some(source_index) = filtered.get(*cursor) else { return };
        let Some(item) = PrPicker::row(listing, *source_index) else { return };
        let Some((target, branch)) = self.pr_context.clone() else { return };
        let request = forge::ReviewDiffRequest::new(target, item.number);
        self.pr_pin = Some(PrPin { branch, number: item.number });
        self.remote_changes = RemoteChanges::Loading(request);
        self.remote_changes_pending = true;
        self.close_pr_picker();
        self.pr_pending = true;
    }

    /// Pin a known review target and fetch both its snapshot and its remote diff — the
    /// Deep Review startup path, which knows the target without a picker or probe.
    pub fn pin_review(&mut self, target: crate::git::RepoTarget, number: u64) {
        let request = forge::ReviewDiffRequest::new(target, number);
        self.pr_pin = Some(PrPin { branch: git::current_branch(&self.repo), number });
        self.remote_changes = RemoteChanges::Loading(request);
        self.remote_changes_pending = true;
        self.pr_pending = true;
    }

    /// Drop the pin (`esc` on the PR tab) and refetch the branch's own PR/MR.
    pub fn pr_unpin(&mut self) -> Result<()> {
        if self.pr_pin.take().is_some() {
            self.clear_remote_changes();
            self.pr_pending = true;
            self.restore_local_changes()?;
        }
        Ok(())
    }

    /// Open the project switcher (`ctrl-p`). Discovery is one directory listing per root
    /// plus an optional zoxide query — fast enough to run inline, like `r`'s reload.
    pub fn open_switcher(&mut self) {
        if self.switcher.is_some() {
            return;
        }
        self.switcher = Some(ProjectSwitcher::new(switcher::discover(&self.switcher_roots())));
    }

    /// The switcher's search roots: the configured `switcher_roots`, else the repo's
    /// parent — the zero-config default lists the siblings of whatever is under review.
    fn switcher_roots(&self) -> Vec<PathBuf> {
        let configured = self
            .plugin_config()
            .map(crate::config::PluginConfig::switcher_roots)
            .unwrap_or_default();
        if configured.is_empty() {
            return self.repo.parent().map(Path::to_path_buf).into_iter().collect();
        }
        configured.iter().map(|root| switcher::expand_tilde(root)).collect()
    }

    pub fn close_switcher(&mut self) {
        self.switcher = None;
    }

    pub fn switcher_move(&mut self, delta: isize) {
        if let Some(switcher) = &mut self.switcher {
            if !switcher.filtered.is_empty() {
                switcher.cursor =
                    switcher.cursor.saturating_add_signed(delta).min(switcher.filtered.len() - 1);
            }
            switcher.pending = None;
        }
    }

    pub fn switcher_input(&mut self, c: char) {
        if let Some(switcher) = &mut self.switcher {
            switcher.query.push(c);
            switcher.refilter();
        }
    }

    pub fn switcher_backspace(&mut self) {
        if let Some(switcher) = &mut self.switcher
            && switcher.query.pop().is_some()
        {
            switcher.refilter();
        }
    }

    /// Confirm the highlighted project (`enter` in the switcher). Unsent comments live only
    /// in this session, so a non-empty store demands a second `enter` before they are lost.
    pub fn switcher_select(&mut self) {
        let unsent = self.store.len() + self.remote_drafts.len();
        let Some(switcher) = &mut self.switcher else {
            return;
        };
        let Some(path) = switcher.selected().map(|project| project.path.clone()) else {
            return;
        };
        if unsent > 0 && switcher.pending.as_ref() != Some(&path) {
            switcher.pending = Some(path);
            let s = if unsent == 1 { "" } else { "s" };
            self.status = format!("{unsent} unsent comment{s} — enter again to switch");
            return;
        }
        self.project_switch = Some(path);
        self.switcher = None;
    }

    /// The confirmed project pick, if one is due. Taking it clears the request, so the
    /// event loop rebuilds the session exactly once per pick.
    pub fn take_project_switch(&mut self) -> Option<PathBuf> {
        self.project_switch.take()
    }
    /// Apply a snapshot fetched off-thread (`forge::fetch` runs on a worker so the UI never
    /// blocks — `lib.rs`). A transient `Error` keeps the last good snapshot frozen with a status
    /// note, so a failed poll never blanks a populated tab; the cursor clamps to the new rows.
    pub fn apply_pr(&mut self, view: forge::PrView) {
        self.pr_refreshing = false;
        let has_snapshot = matches!(
            self.pr,
            forge::PrView::Pr(_) | forge::PrView::NoPr(_) | forge::PrView::Ambiguous(_)
        );
        if has_snapshot && let Some(message) = view.retry_remedy() {
            self.pr_notice = Some(message);
            self.pr_read_scroll = 0;
            return;
        }
        self.pr_notice = None;
        // Follow the selected comment by identity, not index, so a refresh that inserts a newer
        // comment (the list is newest-first) keeps the cursor on the same one and leaves the read
        // scroll intact — only a vanished or absent selection resets it (mirrors the file tabs'
        // poll-preservation, specs/tui.md).
        let selected = self
            .pr_selected_comment()
            .map(|c| (c.author.clone(), c.created_at.clone(), c.anchor.clone()));
        self.pr = view;
        let restored = selected.as_ref().and_then(|(author, created, anchor)| {
            self.pr_snapshot()?.comments.iter().position(|c| {
                c.author == *author && c.created_at == *created && c.anchor == *anchor
            })
        });
        if let Some(i) = restored {
            self.pr_cursor = i;
        } else if self.pr_cursor >= self.pr_row_count() {
            // The selection vanished (or there was none) and the cursor now points past the end:
            // clamp it back into range and reset the read pane.
            self.pr_cursor = self.pr_row_count().saturating_sub(1);
            self.pr_read_scroll = 0;
        }
    }

    /// Persistent remedy for a failed same-input refresh.
    pub fn pr_notice(&self) -> Option<&str> {
        self.pr_notice.as_deref()
    }

    pub fn set_pr_refreshing(&mut self, refreshing: bool) {
        if refreshing && matches!(self.pr, forge::PrView::Pending) {
            self.pr = forge::PrView::Loading;
            self.pr_refreshing = false;
        } else {
            self.pr_refreshing = refreshing;
        }
    }

    pub fn pr_refreshing(&self) -> bool {
        self.pr_refreshing
    }

    /// The resolved snapshot, or `None` in a loading/degraded view.
    #[must_use]
    pub fn pr_snapshot(&self) -> Option<&forge::PrSnapshot> {
        match &self.pr {
            forge::PrView::Pr(s) => Some(s),
            _ => None,
        }
    }

    /// The navigator's cursor count: comments only. Checks are a status display, not a cursor
    /// stop — landing on one shows nothing the row itself doesn't.
    #[must_use]
    pub fn pr_row_count(&self) -> usize {
        self.pr_snapshot().map_or(0, |s| s.comments.len())
    }

    /// The comment under the navigator cursor, for the read pane.
    #[must_use]
    pub fn pr_selected_comment(&self) -> Option<&forge::Comment> {
        self.pr_snapshot()?.comments.get(self.pr_cursor)
    }

    /// Patch rows around the selected remote finding. This fills GitLab's missing snippet and
    /// gives both providers consistent context from the same patch shown in Changes.
    pub fn pr_selected_context(&self) -> Option<Vec<(Row, bool)>> {
        const RADIUS: usize = 3;
        let snapshot = self.active_review_snapshot()?;
        let comment = snapshot.comments.get(self.pr_cursor)?;
        let anchor = comment.diff_anchor.as_ref()?;
        let RemoteChanges::Ready { patch, .. } = &self.remote_changes else { return None };
        let file = patch.files.iter().find(|file| file.change.path == anchor.path)?;
        let diff = FileDiff::from_patch(file, &self.highlighter);
        let selected = diff.rows.iter().position(|row| match anchor.side {
            Side::New => row.new_no() == Some(anchor.line),
            Side::Old => row.old_no() == Some(anchor.line),
        })?;
        let start = selected.saturating_sub(RADIUS);
        let end = (selected + RADIUS + 1).min(diff.rows.len());
        Some(
            diff.rows[start..end]
                .iter()
                .enumerate()
                .map(|(offset, row)| (row.clone(), start + offset == selected))
                .collect(),
        )
    }

    /// Move the navigator cursor by `delta`, resetting the read pane to the top.
    pub fn pr_move(&mut self, delta: isize) {
        let n = self.pr_row_count();
        if n == 0 {
            return;
        }
        self.pr_select(step(self.pr_cursor, delta, n));
    }

    /// Select navigator row `i`, resetting the read pane to the top — the one place the
    /// cursor-move and the read-scroll reset stay paired (a click and `j`/`k` share it).
    pub(crate) fn pr_select(&mut self, i: usize) {
        self.pr_cursor = i;
        self.pr_read_scroll = 0;
    }

    /// Scroll the read pane by `delta` lines (the wheel and `PageUp`/`PageDown`); the renderer
    /// clamps to the body height.
    pub(crate) fn pr_scroll_read(&mut self, delta: isize) {
        self.pr_read_scroll = self.pr_read_scroll.saturating_add_signed(delta);
    }

    /// Open the pull request in the browser (`specs/tui.md`). A resolved PR always carries a
    /// `url`, so there is nothing to guard against.
    pub fn pr_open(&mut self) {
        let Some(url) = self.pr_snapshot().map(|s| s.url.clone()) else {
            return;
        };
        match crate::browser::open(&url) {
            Ok(()) => self.status = "opened PR in browser".to_string(),
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Exchange the active per-tab fields with the inactive tab's saved snapshot. Every per-tab
    /// field on `App` must be swapped here — a new per-tab field left out silently bleeds one
    /// tab's selection or scroll into the other.
    fn swap_active_with_stash(&mut self) {
        std::mem::swap(&mut self.entries, &mut self.stash.entries);
        std::mem::swap(&mut self.file_rows, &mut self.stash.file_rows);
        std::mem::swap(&mut self.file_cursor, &mut self.stash.file_cursor);
        std::mem::swap(&mut self.file_scroll, &mut self.stash.file_scroll);
        std::mem::swap(&mut self.toggled_dirs, &mut self.stash.toggled_dirs);
        std::mem::swap(&mut self.diff, &mut self.stash.diff);
        std::mem::swap(&mut self.visible, &mut self.stash.visible);
        std::mem::swap(&mut self.expanded_folds, &mut self.stash.expanded_folds);
        std::mem::swap(&mut self.diff_path, &mut self.stash.diff_path);
        std::mem::swap(&mut self.diff_cursor, &mut self.stash.diff_cursor);
        std::mem::swap(&mut self.diff_scroll, &mut self.stash.diff_scroll);
        std::mem::swap(&mut self.h_scroll, &mut self.stash.h_scroll);
        std::mem::swap(&mut self.select_anchor, &mut self.stash.select_anchor);
        std::mem::swap(&mut self.line_decorations, &mut self.stash.line_decorations);
        std::mem::swap(&mut self.pi_marks, &mut self.stash.pi_marks);
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Files => Focus::Diff,
            Focus::Diff => Focus::Files,
        };
    }

    /// Move the cursor in the focused pane by `delta` rows. In the files pane the cursor steps
    /// over the tree's visible rows; landing on a file row opens its diff, while a directory row
    /// keeps the current diff so scanning the tree never blanks the pane. The page/half-page keys
    /// reuse this with a larger `delta`, since paging is just a bigger cursor move in the focus.
    pub fn move_cursor(&mut self, delta: isize) -> Result<()> {
        self.manual_navigated = true;
        self.ensure_config_ready()?;
        match self.focus {
            Focus::Files => {
                if !self.file_rows.is_empty() {
                    self.file_cursor = step(self.file_cursor, delta, self.file_rows.len());
                    self.open_cursor_file();
                    // Reveal even when the index clamps unchanged (e.g. `k` at the top), so a
                    // navigation always pulls the cursor back after a wheel scroll.
                    self.reveal_files = true;
                }
            }
            Focus::Diff => {
                if !self.visible.is_empty() {
                    let mut target = step(self.diff_cursor, delta, self.visible.len());
                    if let Some(a) = self.select_anchor {
                        target = self.fold_clamped(a, target);
                    }
                    self.diff_cursor = target;
                    self.reveal_diff = true;
                }
            }
        }
        Ok(())
    }

    /// Open the diff for the file under the cursor when it differs from the one shown; a
    /// no-op on a directory row, so the current diff stays put.
    fn open_cursor_file(&mut self) {
        if let Some(i) = self.file_under_cursor_index()
            && Some(self.entries[i].path.as_str()) != self.diff_path.as_deref()
        {
            self.reset_diff_view();
            self.load_left();
        }
    }

    /// Act on the file-list row at `index` (a mouse click): a file opens its diff, a
    /// directory toggles its expansion.
    pub fn select_file(&mut self, index: usize) -> Result<()> {
        self.manual_navigated = true;
        self.ensure_config_ready()?;
        if index >= self.file_rows.len() {
            return Ok(());
        }
        self.focus = Focus::Files;
        self.file_cursor = index;
        self.reveal_files = true;
        match self.file_rows[index].kind {
            RowKind::File { .. } => self.open_cursor_file(),
            RowKind::Dir { .. } => self.toggle_dir(),
        }
        Ok(())
    }

    /// Collapse or expand the directory under the cursor, then rebuild the tree. The cursor
    /// stays on the directory row (still present, now toggled).
    fn toggle_dir(&mut self) {
        let Some(path) = self.dir_under_cursor() else { return };
        // Flip its membership in the toggled set (toggled = flipped from the tab's default).
        if !self.toggled_dirs.remove(&path) {
            self.toggled_dirs.insert(path);
        }
        self.apply_dir_change();
    }

    /// Whether directory `path` is currently expanded under the active tab's resting state.
    fn dir_expanded(&self, path: &str) -> bool {
        self.default_expanded() ^ self.toggled_dirs.contains(path)
    }

    /// Force directory `path` to `want` (expanded or collapsed); returns whether it changed.
    fn set_dir_expanded(&mut self, path: &str, want: bool) -> bool {
        if self.dir_expanded(path) == want {
            return false;
        }
        if !self.toggled_dirs.remove(path) {
            self.toggled_dirs.insert(path.to_string());
        }
        true
    }

    /// Whether the cursor is on a directory row in the focused file list — the rows `←`/`→`
    /// collapse and expand (elsewhere those keys scroll the diff).
    pub fn on_folder(&self) -> bool {
        self.focus == Focus::Files
            && self.file_rows.get(self.file_cursor).is_some_and(|r| r.dir_path().is_some())
    }

    /// Whether the diff cursor is on a fold row — the row `→` expands (elsewhere `→` scrolls
    /// the diff sideways). Folds are expand-only, so `←` never collapses one.
    pub fn on_fold(&self) -> bool {
        self.focus == Focus::Diff
            && self.visible.get(self.diff_cursor).and_then(Row::fold_anchor).is_some()
    }

    /// Expand the directory under the cursor (`→`); a no-op if it is a file or already open.
    pub fn expand_dir(&mut self) {
        if self.plugin_config().is_none() {
            return;
        }
        if let Some(path) = self.dir_under_cursor()
            && self.set_dir_expanded(&path, true)
        {
            self.apply_dir_change();
        }
    }

    /// Collapse the directory under the cursor (`←`); a no-op if it is a file or already shut.
    pub fn collapse_dir(&mut self) {
        if self.plugin_config().is_none() {
            return;
        }
        if let Some(path) = self.dir_under_cursor()
            && self.set_dir_expanded(&path, false)
        {
            self.apply_dir_change();
        }
    }

    /// The path of the directory row under the cursor, if any.
    fn dir_under_cursor(&self) -> Option<String> {
        self.file_rows.get(self.file_cursor).and_then(|r| r.dir_path()).map(str::to_string)
    }

    /// Rebuild the tree after a directory's expansion changed, keeping the cursor in range.
    fn apply_dir_change(&mut self) {
        // In `All files`, expanding an ignored directory loads its children lazily, so the
        // entry set is rebuilt before the rows (file-list.md). Other tabs just re-flatten.
        if self.tab == Tab::AllFiles
            && let Ok(entries) = self.all_files_entries()
        {
            self.entries = entries;
        }
        self.rebuild_file_rows();
        self.file_cursor = self.file_cursor.min(self.file_rows.len().saturating_sub(1));
        self.reveal_files = true; // the row may have moved off-screen; pull it back
    }

    /// Wheel-scroll the diff's viewport, leaving `diff_cursor` (the comment anchor) put —
    /// so wheeling to read context never moves what a comment will attach to. The upper
    /// bound is applied each frame by `bound_diff_scroll`.
    pub fn wheel_diff(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        self.diff_scroll = offset_by(self.diff_scroll, delta);
    }

    /// Wheel-scroll the file list's viewport, leaving the selection and the open diff
    /// untouched — so browsing the list never reloads a diff. Bounded each frame.
    pub fn wheel_files(&mut self, delta: isize) {
        if self.file_rows.is_empty() {
            return;
        }
        self.file_scroll = offset_by(self.file_scroll, delta);
    }

    /// Extend a mouse drag-selection to the diff line at `index`, anchoring on first drag.
    pub fn drag_select_to(&mut self, index: usize) {
        if index < self.visible.len() {
            self.focus = Focus::Diff;
            let anchor = *self.select_anchor.get_or_insert(self.diff_cursor);
            self.diff_cursor = self.fold_clamped(anchor, index);
            self.reveal_diff = true;
        }
    }

    /// Clamp `target` so the inclusive range from `anchor` to `target` crosses no fold: a
    /// selection treats a fold as a hard boundary, so its line range and snippet always agree
    /// (never bracketing hidden lines the snippet omits). Stops the moving end shy of the fold.
    fn fold_clamped(&self, anchor: usize, target: usize) -> usize {
        if target > anchor {
            (anchor + 1..=target).find(|&i| !self.visible[i].is_content()).map_or(target, |i| i - 1)
        } else {
            (target..anchor)
                .rev()
                .find(|&i| !self.visible[i].is_content())
                .map_or(target, |i| i + 1)
        }
    }

    /// Toggle a range-selection anchor at the current diff line.
    pub fn toggle_select(&mut self) {
        if self.focus == Focus::Diff && !self.visible.is_empty() {
            self.select_anchor = match self.select_anchor {
                Some(_) => None,
                None => Some(self.diff_cursor),
            };
            self.reveal_diff = true;
        }
    }

    /// Drop the range-selection anchor (the `esc` clear in the diff); a no-op when none is set.
    pub fn clear_selection(&mut self) {
        if self.select_anchor.is_some() {
            self.select_anchor = None;
            self.reveal_diff = true;
        }
    }

    /// The inclusive `[lo, hi]` diff-line range currently selected.
    pub fn selection_range(&self) -> (usize, usize) {
        match self.select_anchor {
            Some(a) => (a.min(self.diff_cursor), a.max(self.diff_cursor)),
            None => (self.diff_cursor, self.diff_cursor),
        }
    }

    pub fn start_comment(&mut self) {
        if self.focus == Focus::Diff && self.has_anchorable_selection() {
            self.remote_compose = None;
            if self.tab == Tab::Changes && self.remote_changes_active() {
                if self.deep_guard() {
                    return;
                }
                let (Some(file), Some((side, start, end, _))) =
                    (self.diff_path.clone(), self.selection_anchor())
                else {
                    return;
                };
                let Some((target, number)) = self.active_review_target() else { return };
                // Both endpoints' parser positions ride the anchor: GitLab's multi-line
                // positions need the (old, new) pair per endpoint for its line codes.
                let endpoints = range_endpoint(&self.diff.rows, side, start).zip(range_endpoint(
                    &self.diff.rows,
                    side,
                    end,
                ));
                self.remote_compose = Some(RemoteCompose {
                    target,
                    number,
                    action: forge::ReviewDraftAction::Inline(forge::DiffAnchor {
                        path: file,
                        old_path: self.diff.previous_path.clone(),
                        side,
                        line: end,
                        start_line: (start != end).then_some(start),
                        endpoints,
                    }),
                });
            }
            // Anchor the cursor at the selection's last line so the scroll keeps it (and
            // the box drawn beneath it) in view.
            self.diff_cursor = self.selection_range().1;
            self.reveal_diff = true; // scroll the anchored line into view before the box opens
            self.input.clear();
            self.caret = 0;
            self.resume_list = false; // a fresh diff comment returns to the diff, not the list
            self.mode = Mode::Composing { editing: None };
        }
    }

    /// Reply to the selected forge comment. Inline threads keep their provider thread id;
    /// unthreaded GitHub comments become a new conversation comment mentioning the author.
    pub fn start_pr_reply(&mut self) {
        if self.deep_guard() {
            return;
        }
        let Some(snapshot) = self.active_review_snapshot() else { return };
        let Some(comment) = snapshot.comments.get(self.pr_cursor) else { return };
        let Some((target, number)) = self.active_review_target() else { return };
        self.remote_compose = Some(RemoteCompose {
            target,
            number,
            action: forge::ReviewDraftAction::Reply {
                remote_id: comment.remote_id.clone(),
                author: comment.author.clone(),
            },
        });
        self.input.clear();
        self.caret = 0;
        self.resume_list = false;
        self.mode = Mode::Composing { editing: None };
    }

    pub fn start_edit(&mut self) {
        if self.tab == Tab::Changes && self.remote_changes_active() {
            return;
        }
        // Editing from the comments-list overlay returns there on finish (else to the diff).
        let from_list = self.mode == Mode::List;
        let Some(i) = self.target_comment() else { return };
        let Some(c) = self.store.get(i) else { return };
        let (file, side, start, end, text) =
            (c.file.clone(), c.side, c.start, c.end, c.text.clone());

        // Bring the comment's file into the diff and land the cursor on its line, so the
        // inline edit box opens over the comment — even when editing from the list, and even
        // when the file's row is hidden inside a collapsed directory (load it by path, not by
        // tree row). Move the list cursor onto its row when one exists.
        if self.diff_path.as_deref() != Some(file.as_str())
            && let Some(e) = self.entries.iter().find(|e| e.path == file).cloned()
        {
            self.reset_diff_view();
            // Open it in the active tab's view — the File view on `All files`, not a diff — so
            // the pane and the comment's anchor kind stay consistent with the tab.
            self.open_path_in_tab(e.path, e.previous_path);
            if let Some(fi) = self.file_row_of_path(&file) {
                self.file_cursor = fi;
            }
        }
        // Only move the cursor when the open diff is actually the comment's file, so a
        // stale comment (file gone from the changeset) never jumps the cursor onto a
        // same-numbered line in a different file.
        if self.diff_path.as_deref() == Some(file.as_str())
            && let Some(idx) = self.visible.iter().position(|row| {
                let no = match side {
                    Side::New => row.new_no(),
                    Side::Old => row.old_no(),
                };
                no.is_some_and(|n| start <= n && n <= end)
            })
        {
            self.diff_cursor = idx;
            self.select_anchor = None;
        }
        self.focus = Focus::Diff;
        self.reveal_diff = true; // scroll the edited line into view before the box opens
        self.caret = text.chars().count(); // edit opens with the caret at the end
        self.input = text;
        self.resume_list = from_list;
        self.mode = Mode::Composing { editing: Some(i) };
    }

    // --- comment editor: a character caret into `input`; edits happen at the caret ---------
    // `caret` is a char index in `0..=input.chars().count()`. Edits round-trip through a
    // `Vec<char>` (comments are short), so every op is character-wise and multi-byte safe.

    /// Run a character-wise edit on the comment input: collect `input` into a `Vec<char>` with
    /// the caret as an in-range index, hand both to `f`, then reassemble and re-clamp the caret.
    /// A no-op when not composing. Every mutating `input_*` op routes through here, so the
    /// guard / collect / reassemble lives once instead of seven times.
    fn edit_input(&mut self, f: impl FnOnce(&mut Vec<char>, &mut usize)) {
        if !self.composing() {
            return;
        }
        let mut v: Vec<char> = self.input.chars().collect();
        let mut caret = self.caret.min(v.len());
        f(&mut v, &mut caret);
        self.caret = caret.min(v.len());
        self.input = v.into_iter().collect();
    }

    /// Move the caret with a function of the current `Vec<char>` view; a no-op when not composing.
    /// The read-only sibling of [`edit_input`](Self::edit_input) for the `caret_*` motions.
    fn move_caret(&mut self, f: impl FnOnce(&[char], usize) -> usize) {
        if self.composing() {
            let v: Vec<char> = self.input.chars().collect();
            self.caret = f(&v, self.caret.min(v.len()));
        }
    }

    /// Insert `ch` at the caret.
    pub fn input_push(&mut self, ch: char) {
        self.edit_input(|v, caret| {
            v.insert(*caret, ch);
            *caret += 1;
        });
    }

    /// Insert pasted `text` at the caret as one unit, normalizing `\r\n`/`\r` to `\n`.
    pub fn input_paste(&mut self, text: &str) {
        let norm: Vec<char> = text.replace("\r\n", "\n").replace('\r', "\n").chars().collect();
        self.edit_input(|v, caret| {
            let n = norm.len();
            v.splice(*caret..*caret, norm);
            *caret += n;
        });
    }

    /// Delete the character before the caret.
    pub fn input_backspace(&mut self) {
        self.edit_input(|v, caret| {
            if *caret > 0 {
                v.remove(*caret - 1);
                *caret -= 1;
            }
        });
    }

    /// Delete the character at the caret (`Delete`).
    pub fn input_delete_forward(&mut self) {
        self.edit_input(|v, caret| {
            if *caret < v.len() {
                v.remove(*caret);
            }
        });
    }

    /// Delete the word before the caret (`Ctrl+W`): the trailing whitespace, then the run of
    /// non-whitespace before it, so one press clears one word.
    pub fn input_delete_word(&mut self) {
        self.edit_input(|v, caret| {
            let start = word_start(v, *caret);
            v.drain(start..*caret);
            *caret = start;
        });
    }

    /// Delete from the start of the logical line to the caret (`Ctrl+U`).
    pub fn input_kill_to_start(&mut self) {
        self.edit_input(|v, caret| {
            let start = line_start(v, *caret);
            v.drain(start..*caret);
            *caret = start;
        });
    }

    /// Delete from the caret to the end of the logical line (`Ctrl+K`).
    pub fn input_kill_to_end(&mut self) {
        self.edit_input(|v, caret| {
            let end = line_end(v, *caret);
            v.drain(*caret..end);
        });
    }

    /// Move the caret one character left / right.
    pub fn caret_left(&mut self) {
        self.move_caret(|_, caret| caret.saturating_sub(1));
    }
    pub fn caret_right(&mut self) {
        self.move_caret(|v, caret| (caret + 1).min(v.len()));
    }

    /// Move the caret to the start / end of the logical line (between newlines).
    pub fn caret_home(&mut self) {
        self.move_caret(line_start);
    }
    pub fn caret_end(&mut self) {
        self.move_caret(line_end);
    }

    /// Move the caret one word left / right.
    pub fn caret_word_left(&mut self) {
        self.move_caret(word_start);
    }
    pub fn caret_word_right(&mut self) {
        self.move_caret(word_end);
    }

    pub fn cancel_comment(&mut self) {
        self.leave_compose();
    }

    /// Leave compose mode, returning to the comments-list overlay if the compose was opened
    /// from it (and any comments remain), else to Normal.
    fn leave_compose(&mut self) {
        self.input.clear();
        self.remote_compose = None;
        self.caret = 0;
        let resume = std::mem::take(&mut self.resume_list);
        if resume && !self.store.is_empty() {
            self.list_cursor = self.list_cursor.min(self.store.len() - 1);
            self.mode = Mode::List;
        } else {
            self.mode = Mode::Normal;
        }
    }

    /// Save the in-progress comment — editing the existing one or anchoring a new one
    /// to the selection — then leave compose mode. Blank text cancels instead.
    pub fn submit_comment(&mut self) {
        let Mode::Composing { editing } = self.mode else { return };
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.cancel_comment();
            return;
        }
        if let Some(compose) = self.remote_compose.take() {
            let local_id = self.next_remote_draft_id;
            self.next_remote_draft_id = self.next_remote_draft_id.saturating_add(1);
            self.remote_drafts.push(PendingRemoteDraft {
                target: compose.target,
                number: compose.number,
                draft: forge::ReviewDraft { local_id, action: compose.action, body: text },
                error: None,
                outcome_unknown: false,
            });
            self.status = "remote review draft added".to_string();
            self.collab_touched = true;
            self.select_anchor = None;
            self.leave_compose();
            return;
        }
        match editing {
            Some(i) => {
                logln!("comment edit [{i}] :: {text}");
                self.store.edit(i, text);
                // Editing an agent-staged draft moves its ownership to the reviewer; the
                // loop forwards this to the session, which then bounces Pi overwrites.
                if let Some((draft, _)) = self
                    .collab_refs
                    .iter()
                    .find(|(_, r)| matches!(r, CollabDraftRef::LocalComment(j) if *j == i))
                {
                    self.pending_collab_edits.push(draft.clone());
                }
                self.status = "comment updated".to_string();
                self.collab_touched = true;
            }
            None => {
                if let Some(c) = self.build_comment(text) {
                    logln!("comment add {} :: {}", c.location(), c.text);
                    self.store.add(c);
                    self.status = "comment added".to_string();
                }
            }
        }
        self.select_anchor = None;
        self.leave_compose();
    }

    fn active_review_target(&self) -> Option<(crate::git::RepoTarget, u64)> {
        if let Some(request) = self.remote_changes.request() {
            return Some((request.target.clone(), request.number));
        }
        let snapshot = self.pr_snapshot()?;
        let target = self.pr_context.as_ref()?.0.clone();
        Some((target, snapshot.number))
    }

    /// The remote review the reviewer is actually looking at: an explicitly pinned pick,
    /// or the `PR` tab's review. The branch's PR, auto-fetched in the background, never
    /// counts — a reviewer browsing local changes is not working that review, so it must
    /// hijack neither a local Shift+D nor the identity a prompt snapshot names.
    fn viewed_review_target(&self) -> Option<(crate::git::RepoTarget, u64)> {
        (self.remote_changes.request().is_some() || self.tab == Tab::Pr)
            .then(|| self.active_review_target())
            .flatten()
    }

    /// The snapshot only when it belongs to the exact active review target. Remote patch and
    /// metadata workers complete independently, so number/provider matching is mandatory before
    /// plotting threads or borrowing commit refs for writes.
    fn active_review_snapshot(&self) -> Option<&forge::PrSnapshot> {
        let (target, number) = self.active_review_target()?;
        let context = &self.pr_context.as_ref()?.0;
        let snapshot = self.pr_snapshot()?;
        (context == &target && snapshot.provider == target.provider && snapshot.number == number)
            .then_some(snapshot)
    }

    /// Whether the selection has at least one content row a comment can attach to —
    /// a fold marker does not qualify.
    fn has_anchorable_selection(&self) -> bool {
        let (lo, hi) = self.selection_range();
        self.visible.get(lo..=hi).is_some_and(|s| s.iter().any(Row::is_content))
    }

    /// The `(side, start, end, snippet)` the current selection anchors to.
    fn selection_anchor(&self) -> Option<(Side, u32, u32, String)> {
        let (lo, hi) = self.selection_range();
        let selected: Vec<&Row> = self.visible.get(lo..=hi)?.iter().collect();
        anchor(&selected)
    }

    fn build_comment(&self, text: String) -> Option<Comment> {
        // Anchor to the file the open diff belongs to (`diff_path`), not the file-list
        // selection — they diverge if the list shifts under a comment in progress.
        let file = self.diff_path.clone()?;
        let (side, start, end, lines) = self.selection_anchor()?;
        // The File view marks every comment as content-anchored, so it ages by file existence,
        // not changeset membership (specs/review-model.md).
        let diff_anchored = self.diff.view == View::Diff;
        Some(Comment { file, side, start, end, lines, text, diff_anchored })
    }

    /// The `path:line` the composer is anchored to (selection for a new comment,
    /// the existing location when editing). `None` when not composing.
    pub fn pending_location(&self) -> Option<String> {
        if matches!(
            self.remote_compose,
            Some(RemoteCompose { action: forge::ReviewDraftAction::Reply { .. }, .. })
        ) {
            return self.pr_selected_comment().map(|comment| comment.anchor.clone());
        }
        match self.mode {
            Mode::Composing { editing: Some(i) } => self.store.get(i).map(Comment::location),
            Mode::Composing { editing: None } => {
                let file = self.diff_path.clone()?;
                let (side, start, end, _) = self.selection_anchor()?;
                // Only `location()` is read here, which ignores `diff_anchored`.
                let c = Comment {
                    file,
                    side,
                    start,
                    end,
                    lines: String::new(),
                    text: String::new(),
                    diff_anchored: true,
                };
                Some(c.location())
            }
            Mode::Normal | Mode::List => None,
        }
    }

    /// Whether comment `c` anchors to the pane's current view — a diff comment to the Diff view,
    /// a content comment to the File view. Stops a comment of one kind rendering on, or being
    /// acted on at, an unrelated line in the other tab's view of the same file (the diff's line
    /// numbering and the File view's worktree line numbering differ; specs/review-model.md).
    fn comment_in_view(&self, c: &Comment) -> bool {
        !(self.tab == Tab::Changes && self.remote_changes_active())
            && c.diff_anchored == (self.diff.view == View::Diff)
    }

    /// Feedback and descendant-change indicators for one visible file-tree row. Local trees
    /// inspect saved agent comments; a pinned remote Changes tree instead inspects forge threads
    /// and this review's pending inline drafts. Folder matching is path-segment aware.
    pub fn tree_badges(&self, row: &file_list::Row) -> TreeBadges {
        match &row.kind {
            RowKind::File { index, .. } => {
                let Some(entry) = self.entries.get(*index) else { return TreeBadges::default() };
                TreeBadges {
                    changed: false,
                    commented: self.feedback_matches(|path| path == entry.path),
                }
            }
            RowKind::Dir { path, .. } => TreeBadges {
                changed: self.tab == Tab::AllFiles
                    && self.changed.keys().any(|candidate| descendant_of(candidate, path)),
                commented: self.feedback_matches(|candidate| descendant_of(candidate, path)),
            },
        }
    }

    fn feedback_matches(&self, matches: impl Fn(&str) -> bool) -> bool {
        if self.tab == Tab::Changes && self.remote_changes_active() {
            let incoming = self.active_review_snapshot().is_some_and(|snapshot| {
                snapshot.comments.iter().any(|comment| {
                    comment.diff_anchor.as_ref().is_some_and(|anchor| matches(&anchor.path))
                })
            });
            if incoming {
                return true;
            }
            let Some((target, number)) = self.active_review_target() else { return false };
            return self.remote_drafts.iter().any(|pending| {
                pending.target == target
                    && pending.number == number
                    && matches!(
                        &pending.draft.action,
                        forge::ReviewDraftAction::Inline(anchor) if matches(&anchor.path)
                    )
            });
        }
        self.store.iter().any(|comment| matches(&comment.file))
    }

    /// Row indices on the open diff's file that review feedback anchors to: local comments in
    /// their own view, plus an active remote review's forge threads and pending inline drafts.
    /// A ranged anchor marks every row it covers. Each source follows its card plotter's
    /// visibility rules, so a marked row always has its card and vice versa.
    pub fn commented_lines(&self) -> HashSet<usize> {
        let Some(file) = self.diff_path.clone() else {
            return HashSet::new();
        };
        let mut remote: Vec<(Side, u32, u32)> = Vec::new();
        if self.tab == Tab::Changes
            && self.remote_changes_active()
            && let Some(snapshot) = self.active_review_snapshot()
        {
            remote.extend(
                snapshot
                    .comments
                    .iter()
                    .filter_map(|comment| comment.diff_anchor.as_ref())
                    .filter(|anchor| anchor.path == file)
                    .map(anchor_range),
            );
        }
        if let Some((target, number)) = self.active_review_target() {
            remote.extend(
                self.remote_drafts
                    .iter()
                    .filter(|pending| pending.target == target && pending.number == number)
                    .filter_map(|pending| match &pending.draft.action {
                        forge::ReviewDraftAction::Inline(anchor) if anchor.path == file => {
                            Some(anchor_range(anchor))
                        }
                        _ => None,
                    }),
            );
        }
        self.visible
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                self.store
                    .iter()
                    .any(|c| c.file == file && self.comment_in_view(c) && line_in(c, row))
                    || remote.iter().any(|&range| row_in_range(row, range))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn pr_selected_reply_drafts(&self) -> Vec<&PendingRemoteDraft> {
        let (Some(selected), Some((target, number))) =
            (self.pr_selected_comment(), self.active_review_target())
        else {
            return Vec::new();
        };
        self.remote_drafts
            .iter()
            .filter(|pending| pending.target == target && pending.number == number)
            .filter(|pending| match &pending.draft.action {
                forge::ReviewDraftAction::Reply { remote_id, author } => {
                    remote_id == &selected.remote_id && author == &selected.author
                }
                forge::ReviewDraftAction::Inline(_) => false,
            })
            .collect()
    }

    pub fn remote_draft_count(&self) -> usize {
        let Some((target, number)) = self.active_review_target() else { return 0 };
        self.remote_drafts
            .iter()
            .filter(|draft| draft.target == target && draft.number == number)
            .count()
    }

    pub fn syncable_remote_draft_count(&self) -> usize {
        let Some((target, number)) = self.active_review_target() else { return 0 };
        self.remote_drafts
            .iter()
            .filter(|draft| {
                draft.target == target && draft.number == number && !draft.outcome_unknown
            })
            .count()
    }

    pub fn remote_draft_cards(&self) -> Vec<Vec<usize>> {
        let mut cards = vec![Vec::new(); self.visible.len()];
        let (Some(path), Some((target, number))) =
            (self.diff_path.as_deref(), self.active_review_target())
        else {
            return cards;
        };
        for (index, pending) in self.remote_drafts.iter().enumerate() {
            if pending.target != target || pending.number != number {
                continue;
            }
            let forge::ReviewDraftAction::Inline(anchor) = &pending.draft.action else { continue };
            if anchor.path != path {
                continue;
            }
            let range = anchor_range(anchor);
            if let Some(row) = self.visible.iter().rposition(|row| row_in_range(row, range)) {
                cards[row].push(index);
            }
        }
        cards
    }

    /// Forge comment indices plotted under their anchored rows in a remote Changes diff.
    pub fn remote_comment_cards(&self) -> Vec<Vec<usize>> {
        let mut cards = vec![Vec::new(); self.visible.len()];
        if self.tab != Tab::Changes || !self.remote_changes_active() {
            return cards;
        }
        let (Some(path), Some(snapshot)) =
            (self.diff_path.as_deref(), self.active_review_snapshot())
        else {
            return cards;
        };
        for (index, comment) in snapshot.comments.iter().enumerate() {
            let Some(anchor) = &comment.diff_anchor else { continue };
            if anchor.path != path {
                continue;
            }
            let range = anchor_range(anchor);
            if let Some(row) = self.visible.iter().rposition(|row| row_in_range(row, range)) {
                cards[row].push(index);
            }
        }
        cards
    }

    /// For each visible diff row, the store indices of comments whose card renders after it.
    /// A comment's card sits under the last visible row its line range covers, so the renderer
    /// can splice it inline (always visible) and the geometry stays anchored to a real row.
    pub fn comment_cards(&self) -> Vec<Vec<usize>> {
        let mut cards = vec![Vec::new(); self.visible.len()];
        let Some(file) = self.diff_path.as_deref() else { return cards };
        for (ci, c) in self.store.iter().enumerate() {
            if c.file == file
                && self.comment_in_view(c)
                && let Some(last) = self.visible.iter().rposition(|row| line_in(c, row))
            {
                cards[last].push(ci);
            }
        }
        cards
    }

    /// The store index to act on: the comment under the diff cursor, or — in the
    /// list overlay — the highlighted row.
    fn target_comment(&self) -> Option<usize> {
        if self.mode == Mode::List {
            return (self.list_cursor < self.store.len()).then_some(self.list_cursor);
        }
        self.comment_under_cursor()
    }

    /// The store index of a comment whose range covers the current diff row, if any.
    fn comment_under_cursor(&self) -> Option<usize> {
        let file = self.diff_path.as_deref()?;
        let row = self.visible.get(self.diff_cursor)?;
        self.store.iter().position(|c| c.file == file && self.comment_in_view(c) && line_in(c, row))
    }

    pub fn delete_comment(&mut self) {
        if self.tab == Tab::Changes && self.remote_changes_active() {
            return;
        }
        if let Some(i) = self.target_comment() {
            logln!("comment delete [{i}]");
            self.store.take(i);
            self.collab_refs_on_local_removal(i);
            self.collab_touched = true;
            self.clamp_list_cursor();
            self.status = "comment deleted".to_string();
            // Don't strand the user in an empty "Comments (0)" overlay, matching `export`.
            if self.store.is_empty() {
                self.close_list();
            }
        }
    }

    /// Move the diff cursor to the next (`dir >= 0`) or previous commented line.
    pub fn jump_comment(&mut self, dir: isize) {
        self.manual_navigated = true;
        let mut idxs: Vec<usize> = self.commented_lines().into_iter().collect();
        if idxs.is_empty() {
            return;
        }
        idxs.sort_unstable();
        self.focus = Focus::Diff;
        let cur = self.diff_cursor;
        let target = if dir >= 0 {
            idxs.iter().copied().find(|&i| i > cur).or_else(|| idxs.first().copied())
        } else {
            idxs.iter().rev().copied().find(|&i| i < cur).or_else(|| idxs.last().copied())
        };
        if let Some(t) = target {
            self.select_anchor = None; // a comment jump is navigation, not a selection extend
            self.diff_cursor = t;
            self.reveal_diff = true;
        }
    }

    pub fn open_list(&mut self) {
        if self.tab == Tab::Changes && self.remote_changes_active() {
            return;
        }
        if !self.store.is_empty() {
            self.list_cursor = 0;
            self.mode = Mode::List;
        }
    }

    pub fn close_list(&mut self) {
        if self.mode == Mode::List {
            self.mode = Mode::Normal;
        }
    }

    /// The actions the footer offers for the current context, most-relevant first, each tagged
    /// with its visual tier. Pure — a context → action mapping, unit-tested without a terminal.
    /// The renderer maps each to a key+label, styles it by tier, and drops the least relevant
    /// (orientation first) to fit one line (`specs/tui.md`).
    #[must_use]
    pub fn footer_actions(&self) -> Vec<(FooterAction, Tier)> {
        use FooterAction as A;
        use Tier::{Normal, Orientation, Primary};

        // A modal sub-task owns the whole bar — no tab/quit orientation while you're in one.
        // The escape action comes right after the primary so the exit hint survives a
        // narrow-width trim (trailing actions are dropped first); modals have no orientation
        // cluster to carry it otherwise.
        match self.mode {
            Mode::Composing { .. } => {
                return vec![(A::Save, Primary), (A::Cancel, Normal), (A::Newline, Normal)];
            }
            Mode::List => {
                return vec![
                    (A::Send, Primary),
                    (A::CloseList, Normal),
                    (A::Copy, Normal),
                    (A::EditComment, Normal),
                    (A::DeleteComment, Normal),
                ];
            }
            Mode::Normal => {}
        }

        // The project switcher owns the bar while open, on whichever tab it was opened from.
        if self.switcher.is_some() {
            return vec![(A::SwitchProject, Primary), (A::ClosePicker, Normal)];
        }

        // The PR tab: the state summary leads (rendered separately); `o open` is the
        // act — available for any resolved PR, not only while a comment is selected, since `o`
        // opens the PR URL itself (`pr_open`). The picker overlay owns the bar while open.
        if self.tab == Tab::Pr {
            if self.pr_picker.is_some() {
                return vec![(A::PinPr, Primary), (A::ClosePicker, Normal)];
            }
            let mut out = Vec::new();
            if self
                .active_review_snapshot()
                .and_then(|snapshot| snapshot.comments.get(self.pr_cursor))
                .is_some()
            {
                out.push((A::Reply, Primary));
                // The Pi hand-off keys surface only while the link is up — without a
                // session they queue into nothing.
                if self.collab_link == Some(true) {
                    out.push((A::AttachPi, Normal));
                    out.push((A::TrayToggle, Normal));
                }
                out.push((A::OpenPr, Normal));
            } else if self.pr_snapshot().is_some() {
                out.push((A::OpenPr, Primary));
            }
            if self.syncable_remote_draft_count() > 0 {
                out.push((A::Send, Normal));
            }
            if self.pr_snapshot().is_some() && self.deep.is_none() && !self.deep_lockout {
                out.push((A::DeepReview, Normal));
            }
            out.push((A::PickPr, Normal));
            if self.pr_pin.is_some() {
                out.push((A::UnpinPr, Normal));
            }
            out.push((A::Tabs, Orientation));
            out.push((A::Projects, Orientation));
            out.push((A::Refresh, Orientation));
            out.push((A::Quit, Orientation));
            return out;
        }

        if self.tab == Tab::Changes && self.remote_changes_active() {
            let mut out = Vec::new();
            if self.file_rows.is_empty() {
                out.push((A::Refresh, Primary));
            } else if self.focus == Focus::Files {
                match self.file_rows.get(self.file_cursor).map(|r| &r.kind) {
                    Some(RowKind::Dir { expanded: true, .. }) => {
                        out.push((A::CollapseDir, Primary));
                    }
                    Some(RowKind::Dir { expanded: false, .. }) => {
                        out.push((A::ExpandDir, Primary));
                    }
                    _ => out.push((A::TogglePane, Primary)),
                }
            } else if self.on_fold() {
                out.push((A::ExpandFold, Primary));
            } else if self.select_anchor.is_some() {
                out.push((A::Comment, Primary));
                out.push((A::ClearSelection, Normal));
            } else {
                out.push((A::Comment, Primary));
                out.push((A::Select, Normal));
            }
            if self.syncable_remote_draft_count() > 0 {
                out.push((A::Send, Normal));
            }
            out.push((A::UnpinPr, Normal));
            if !out.iter().any(|&(action, _)| action == A::Refresh) {
                out.push((A::Refresh, Normal));
            }
            out.push((A::Tabs, Orientation));
            out.push((A::Projects, Orientation));
            out.push((A::Quit, Orientation));
            return out;
        }

        let mut out: Vec<(FooterAction, Tier)> = Vec::new();
        // Whether the diff-jump is already the primary, so orientation doesn't repeat the toggle.
        let mut pane_is_primary = false;

        if self.file_rows.is_empty() {
            // Nothing in scope to review: only switching scope or refreshing is useful.
            out.push((A::Scope, Primary));
            out.push((A::Refresh, Normal));
        } else if self.focus == Focus::Files {
            match self.file_rows.get(self.file_cursor).map(|r| &r.kind) {
                Some(RowKind::Dir { expanded: true, .. }) => out.push((A::CollapseDir, Primary)),
                Some(RowKind::Dir { expanded: false, .. }) => out.push((A::ExpandDir, Primary)),
                _ => {
                    out.push((A::TogglePane, Primary)); // ⇥ into the diff to review
                    pane_is_primary = true;
                }
            }
        } else if self.visible.is_empty() {
            // Diff focused but nothing to show (e.g. a binary): only the scope switch helps.
            out.push((A::Scope, Primary));
        } else if self.on_fold() {
            out.push((A::ExpandFold, Primary));
        } else if self.select_anchor.is_some() {
            out.push((A::Comment, Primary));
            out.push((A::ClearSelection, Normal));
        } else if self.comment_under_cursor().is_some() {
            out.push((A::EditComment, Primary));
            out.push((A::DeleteComment, Normal));
            if self.collab_link == Some(true) {
                out.push((A::AttachPi, Normal));
                out.push((A::TrayToggle, Normal));
            }
            out.push((A::JumpComment, Normal));
        } else {
            out.push((A::Comment, Primary));
            out.push((A::Select, Normal));
        }

        // Switching scope is always available while reviewing, so it shows in every context on
        // the file tabs — unless it's already the primary (the empty / no-diff states above).
        if !out.iter().any(|&(a, _)| a == A::Scope) {
            out.push((A::Scope, Normal));
        }

        // Once a comment is written, sending is the next relevant move — just below the primary
        // (every branch above pushed a primary, so index 1 is in range).
        if !self.store.is_empty() {
            out.insert(1, (A::Send, Normal));
            out.push((A::List, Normal));
        }

        if let Some(deep) = &self.deep {
            if deep.head_moved.is_some() {
                out.push((A::UpdateHead, Normal));
            }
            out.push((A::EndDeep, Normal));
        } else if !self.deep_lockout {
            out.push((A::DeepReview, Normal));
        }
        // The dim, stable orientation cluster: the pane toggle (unless it is already the
        // primary), the tabs, quit.
        if !pane_is_primary && !self.file_rows.is_empty() {
            out.push((A::TogglePane, Orientation));
        }
        out.push((A::Tabs, Orientation));
        out.push((A::Projects, Orientation));
        out.push((A::Quit, Orientation));
        out
    }

    pub fn list_move(&mut self, delta: isize) {
        if self.mode == Mode::List && !self.store.is_empty() {
            self.list_cursor = step(self.list_cursor, delta, self.store.len());
        }
    }

    /// Send/copy every written comment to `target`; consume the whole set only on
    /// success. A failed export leaves all comments in place (`specs/review-model.md`).
    pub fn export(&mut self, target: &dyn ExportTarget) {
        if self.tab == Tab::Changes && self.remote_changes_active() {
            self.status = "remote review drafts sync with s".to_string();
            return;
        }
        if self.store.is_empty() {
            self.status = "no comments to send".to_string();
            return;
        }
        let refs: Vec<&Comment> = self.store.iter().collect();
        let text = format_all(&refs);
        let n = refs.len();
        logln!("export ({n}) -> {} ::\n{text}", target.label());
        match target.export(&text) {
            Ok(()) => {
                self.store.take_all();
                // Consumed comments leave the surfaces; only remote-draft refs remain live.
                self.collab_refs.retain(|(_, r)| matches!(r, CollabDraftRef::RemoteDraft(_)));
                self.status = format!("sent {n} comment(s) to {}", target.label());
                logln!("export OK");
            }
            Err(e) => {
                self.status = format!("{} failed: {e}", target.label());
                logln!("export ERR: {e}");
            }
        }
        self.clamp_list_cursor();
        if self.store.is_empty() {
            self.close_list();
        }
    }

    // --- collaboration: what the Pi session sees and stages -----------------------------

    /// This instance's collaboration identity — the session key a Pi hello must match and
    /// the target its tray items carry. A Deep Review instance serves its bound target
    /// (remote review or branch-qualified local key); a plain sidebar serves its worktree.
    pub fn collab_target_key(&self) -> String {
        match &self.deep {
            Some(deep) => deep.key.clone(),
            None => crate::collab::context::local_target_key(&self.repo),
        }
    }

    /// The selected review item for `a`/`Shift+A`: the PR tab's highlighted comment, else
    /// the local comment under the cursor (or highlighted in the list overlay).
    pub fn collab_selected_item(&self) -> Option<crate::collab::session::TrayItem> {
        use crate::collab::context;
        let target = self.collab_target_key();
        if self.tab == Tab::Pr {
            let comment = self.pr_selected_comment()?;
            return Some(crate::collab::session::TrayItem {
                target,
                key: context::resource_key(comment),
                resource: context::resource_json(comment),
            });
        }
        let comment = self.target_comment().and_then(|i| self.store.get(i))?;
        Some(crate::collab::session::TrayItem {
            target,
            key: local_comment_key(comment),
            resource: local_comment_resource(comment),
        })
    }

    /// One atomic prompt-context snapshot: review identity, live location, selection patch,
    /// selected item, and the tray — all read from this single viewer state, so later pane
    /// navigation cannot retarget an already-submitted prompt.
    pub fn collab_snapshot(&self, tray: serde_json::Value) -> crate::collab::context::Snapshot {
        use crate::collab::context;
        let (target, source) = if let Some((target, number)) = self.viewed_review_target() {
            let source = match target.provider {
                forge::Provider::Github => "github-pr",
                forge::Provider::Gitlab => "gitlab-mr",
            };
            (context::remote_target_key(&target, number), source.to_string())
        } else {
            let source = match self.scope {
                Scope::Uncommitted => "uncommitted",
                Scope::Branch => "branch",
                Scope::LastTurn => "last-turn",
            };
            (self.collab_target_key(), source.to_string())
        };
        let location = self.diff_path.clone().and_then(|path| {
            let (side, start, end, _) = self.selection_anchor()?;
            Some(context::Location {
                path,
                side,
                line: end,
                start_line: (start != end).then_some(start),
            })
        });
        let item = if self.tab == Tab::Pr {
            self.pr_selected_comment().map(context::resource_json)
        } else {
            self.target_comment().and_then(|i| self.store.get(i)).map(local_comment_resource)
        };
        context::Snapshot {
            target,
            source,
            worktree: self.repo.clone(),
            location,
            patch: self.selection_patch(),
            item,
            tray,
        }
    }

    /// The visible content rows around the selection as marker-prefixed text — the hunk
    /// evidence a prompt rides so "this deletion" needs no reconstruction downstream.
    fn selection_patch(&self) -> Option<String> {
        if self.visible.is_empty() || self.diff_path.is_none() {
            return None;
        }
        let (lo, hi) = self.selection_range();
        let lo = lo.saturating_sub(3);
        let hi = (hi + 3).min(self.visible.len() - 1);
        let rows: Vec<String> =
            self.visible[lo..=hi].iter().filter(|r| r.is_content()).map(Row::marker_text).collect();
        (!rows.is_empty()).then(|| rows.join("\n"))
    }

    /// Stage one agent-authored draft. A reply lands as a pending remote draft on the
    /// active review's thread; an anchored finding lands as a local comment (worktree
    /// anchors are local-only until a provider-backed mapping exists, so staging can never
    /// smuggle feedback onto code absent from the remote review). Never publishes.
    pub fn collab_stage_draft(
        &mut self,
        draft: &crate::collab::protocol::StagedDraft,
    ) -> Result<(), String> {
        if let Some(thread) = &draft.reply_to {
            let Some(snapshot) = self.active_review_snapshot() else {
                return Err("no active remote review to reply into".to_string());
            };
            let Some(comment) = snapshot
                .comments
                .iter()
                .find(|c| c.remote_id.as_ref().is_some_and(|id| id.thread_id == *thread))
            else {
                return Err(format!("unknown discussion `{thread}`"));
            };
            let (remote_id, author) = (comment.remote_id.clone(), comment.author.clone());
            let Some((target, number)) = self.active_review_target() else {
                return Err("no active remote review to reply into".to_string());
            };
            let local_id = self.next_remote_draft_id;
            self.next_remote_draft_id = self.next_remote_draft_id.saturating_add(1);
            self.remote_drafts.push(PendingRemoteDraft {
                target,
                number,
                draft: forge::ReviewDraft {
                    local_id,
                    action: forge::ReviewDraftAction::Reply { remote_id, author },
                    body: draft.body.clone(),
                },
                error: None,
                outcome_unknown: false,
            });
            self.collab_refs.push((draft.draft.clone(), CollabDraftRef::RemoteDraft(local_id)));
            self.status = "pi staged a reply draft".to_string();
            return Ok(());
        }
        let Some(anchor) = &draft.anchor else {
            return Err("draft carries neither an anchor nor reply_to".to_string());
        };
        let start = anchor.start_line.unwrap_or(anchor.line).min(anchor.line);
        let Some(snippet) = worktree_snippet(&self.repo, &anchor.path, start, anchor.line) else {
            return Err(format!(
                "{}:{} is not readable in this worktree",
                anchor.path, anchor.line
            ));
        };
        // Marker-prefixed like every content comment, so export and staleness read one shape.
        let lines = snippet.lines().map(|l| format!(" {l}")).collect::<Vec<_>>().join("\n");
        let index = self.store.add(Comment {
            file: anchor.path.clone(),
            side: Side::New,
            start,
            end: anchor.line,
            lines,
            text: draft.body.clone(),
            diff_anchored: false,
        });
        self.collab_refs.push((draft.draft.clone(), CollabDraftRef::LocalComment(index)));
        self.status = "pi staged a finding".to_string();
        Ok(())
    }

    /// Revise a still-Pi-owned draft in place. Ownership was already checked by the
    /// session; this only fails when the draft has left the surfaces (synced, exported,
    /// or deleted), which the extension learns through the ack.
    pub fn collab_revise_draft(
        &mut self,
        draft: &crate::collab::protocol::StagedDraft,
    ) -> Result<(), String> {
        let Some((_, at)) = self.collab_refs.iter().find(|(id, _)| *id == draft.draft) else {
            return Err(format!("draft `{}` is no longer staged here", draft.draft));
        };
        match at.clone() {
            CollabDraftRef::RemoteDraft(local_id) => {
                let Some(pending) =
                    self.remote_drafts.iter_mut().find(|p| p.draft.local_id == local_id)
                else {
                    return Err(format!("draft `{}` was already synced or removed", draft.draft));
                };
                pending.draft.body.clone_from(&draft.body);
                // A new body clears a definite failure; an unknown outcome stays sticky —
                // revising must never re-arm a blind retry.
                pending.error = None;
                Ok(())
            }
            CollabDraftRef::LocalComment(index) => {
                if self.store.edit(index, draft.body.clone()) {
                    Ok(())
                } else {
                    Err(format!("draft `{}` was already removed", draft.draft))
                }
            }
        }
    }

    /// Whether a local comment is an agent-staged draft (for its `pi` badge).
    pub fn collab_owned_comment(&self, index: usize) -> bool {
        self.collab_refs
            .iter()
            .any(|(_, r)| matches!(r, CollabDraftRef::LocalComment(i) if *i == index))
    }

    /// Draft ids whose comments the reviewer edited since the last drain; the loop hands
    /// them to the session so ownership transfers to the human.
    pub fn take_collab_edits(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_collab_edits)
    }

    /// `a` — queue an attach of the selected item (tray replace + Pi focus).
    pub fn collab_attach(&mut self) {
        match self.collab_selected_item() {
            Some(item) => self.pending_collab_intents.push(CollabIntent::Attach(item)),
            None => self.status = "no comment selected to attach".to_string(),
        }
    }

    /// `Shift+A` — queue a tray toggle of the selected item.
    pub fn collab_toggle_tray(&mut self) {
        match self.collab_selected_item() {
            Some(item) => self.pending_collab_intents.push(CollabIntent::Toggle(item)),
            None => self.status = "no comment selected for the tray".to_string(),
        }
    }

    /// The tray commands queued since the last drain.
    pub fn take_collab_intents(&mut self) -> Vec<CollabIntent> {
        std::mem::take(&mut self.pending_collab_intents)
    }

    /// `f` — queue a follow-mode toggle.
    pub fn collab_follow_toggle(&mut self) {
        if self.collab_link == Some(true) {
            self.pending_collab_intents.push(CollabIntent::FollowToggle);
        }
    }

    /// Queue an edit-history step; `delta < 0` walks backward.
    pub fn collab_history_step(&mut self, delta: isize) {
        if self.collab_link.is_some() {
            self.pending_collab_intents.push(if delta < 0 {
                CollabIntent::HistoryBack
            } else {
                CollabIntent::HistoryForward
            });
        }
    }

    /// Whether the reviewer navigated on their own since the last drain.
    pub fn take_manual_nav(&mut self) -> bool {
        std::mem::take(&mut self.manual_navigated)
    }

    /// Move the viewer to an agent-reported location: open the file in the active tab's
    /// projection and land on the line when it is representable — exactly, or on the
    /// collapsed fold that holds it. A miss is never silent: `settle` lands on the nearest
    /// shown line with a status naming the real location (the unavailable-location rule
    /// keeps the retargeting explicit), while `!settle` reports the miss so the caller can
    /// refresh a possibly-lagging diff and retry.
    pub fn collab_navigate_to(
        &mut self,
        path: &str,
        line: Option<u32>,
        settle: bool,
    ) -> NavOutcome {
        // Resolve against the bound worktree: the extension may report absolute paths.
        let relative = std::path::Path::new(path)
            .strip_prefix(&self.repo)
            .map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
        if self.diff_path.as_deref() != Some(relative.as_str()) {
            let Some(entry) = self.entries.iter().find(|e| e.path == relative).cloned() else {
                // Not part of this scope's tree (yet); the caller may reload and retry.
                if settle {
                    self.status = format!("pi is at {relative} — not in this view");
                }
                return NavOutcome::FileMissing;
            };
            self.reset_diff_view();
            self.open_path_in_tab(entry.path, entry.previous_path);
            if let Some(fi) = self.file_row_of_path(&relative) {
                self.file_cursor = fi;
            }
        }
        self.focus = Focus::Diff;
        let Some(line) = line else {
            self.reveal_diff = true;
            return NavOutcome::Landed;
        };
        if let Some(idx) =
            self.visible.iter().position(|row| row.new_no().is_some_and(|n| n == line))
        {
            self.diff_cursor = idx;
            self.select_anchor = None;
            self.reveal_diff = true;
            return NavOutcome::Landed;
        }
        // The line may sit inside a collapsed fold: land on the fold so `→ expand` reaches it.
        if let Some(idx) = self.visible.iter().position(|row| {
            row.fold_anchor().is_some_and(|anchor| {
                anchor <= line && (line as usize) < anchor as usize + row.hidden()
            })
        }) {
            self.diff_cursor = idx;
            self.reveal_diff = true;
            return NavOutcome::Landed;
        }
        if !settle {
            return NavOutcome::LineMissing;
        }
        // Agent-reported lines go stale as later edits shift them; a nearby landing with an
        // explicit status beats a keypress that visibly does nothing.
        let miss = if self.tab == Tab::Changes {
            format!("pi is at {relative}:{line} — outside this change set")
        } else {
            format!("pi is at {relative}:{line} — not in this view")
        };
        let nearest = self
            .visible
            .iter()
            .enumerate()
            .filter_map(|(i, row)| row.new_no().map(|n| (i, n.abs_diff(line))))
            .min_by_key(|&(_, distance)| distance);
        let Some((idx, _)) = nearest else {
            self.status = miss;
            return NavOutcome::LineMissing;
        };
        self.diff_cursor = idx;
        self.select_anchor = None;
        self.reveal_diff = true;
        self.status = format!("{miss}; landed on the nearest shown line");
        NavOutcome::Landed
    }

    /// `Shift+D` — resolve the Deep Review target from the current view and queue the
    /// orchestration. Only a review the reviewer is actually looking at wins: an
    /// explicitly pinned pick, or the `PR` tab's review. A plain local view targets this
    /// worktree — the branch's PR, auto-fetched in the background, must not hijack a
    /// local Shift+D into a remote session.
    pub fn start_deep_review(&mut self) {
        if self.deep.is_some() {
            self.status = "this already is the deep review workspace".to_string();
            return;
        }
        // Ownership does not block reinvocation: the orchestrator finds the labelled
        // workspace and focuses it, which is exactly how Shift+D resumes a session. Only
        // drafting and sync stay locked to the owner.
        let remote = self.viewed_review_target();
        let key = if let Some((target, number)) = &remote {
            crate::collab::context::remote_target_key(target, *number)
        } else {
            // A remote key names the review (the PR); a local session must too, or every
            // branch reviewed in this worktree would revive the previous branch's session.
            let checkout = crate::git::current_branch(&self.repo)
                .or_else(|| crate::git::head_short(&self.repo))
                .unwrap_or_else(|| "unborn".to_string());
            crate::collab::context::local_deep_target_key(&self.repo, &checkout)
        };
        self.pending_deep = Some(DeepRequest { key, remote });
        self.status = "starting deep review…".to_string();
    }

    /// Whether another live process holds this target's draft ownership. Consulted before
    /// drafting or syncing a remote review, so a restarted origin cannot publish duplicates
    /// while a Deep Review workspace owns the target (the claim lives in the store, not in
    /// this process's memory).
    fn deep_owned_elsewhere(&self, key: &str) -> bool {
        if self.deep.is_some() {
            return false; // the deep instance holds the claim itself
        }
        let store = crate::collab::store::SessionStore::for_target(
            &crate::collab::materialize::state_dir(),
            key,
        );
        store.owned_elsewhere("")
    }

    /// Guard a remote-review draft or sync action against Deep Review ownership; sets the
    /// status and returns true when the action must not proceed here.
    fn deep_guard(&mut self) -> bool {
        let Some((target, number)) = self.active_review_target() else { return false };
        let key = crate::collab::context::remote_target_key(&target, number);
        if self.deep_lockout || self.deep_owned_elsewhere(&key) {
            self.deep_lockout = true;
            self.status = "this review's drafts are owned by its deep review workspace".into();
            return true;
        }
        false
    }

    /// `Shift+D` on a highlighted picker row targets that review directly.
    pub fn start_deep_review_from_picker(&mut self) {
        let Some(number) = self.pr_picker_highlighted_number() else { return };
        let Some((target, _)) = self.pr_context.clone() else { return };
        if self.deep.is_some() || self.deep_lockout {
            self.start_deep_review(); // reuse the guard messages
            return;
        }
        let key = crate::collab::context::remote_target_key(&target, number);
        self.pending_deep = Some(DeepRequest { key, remote: Some((target, number)) });
        self.close_pr_picker();
        self.status = "starting deep review…".to_string();
    }

    /// The picker row `Shift+D` would target.
    fn pr_picker_highlighted_number(&self) -> Option<u64> {
        let Some(PrPicker::Loaded { listing, filtered, cursor }) = &self.pr_picker else {
            return None;
        };
        PrPicker::row(listing, *filtered.get(*cursor)?).map(|item| item.number)
    }

    /// The queued `Shift+D` request, for the loop's orchestrator.
    pub fn take_deep_review(&mut self) -> Option<DeepRequest> {
        self.pending_deep.take()
    }

    /// `Shift+X` — end Deep Review, two-step. The first press arms and names exactly what
    /// would be lost (unsynced drafts, uncommitted edits, local-only commits); the second
    /// confirms. Any other key disarms.
    pub fn request_end_deep(&mut self) {
        let Some(deep) = &mut self.deep else { return };
        if deep.end_armed {
            return; // the loop drains it via take_end_deep
        }
        deep.end_armed = true;
        let drafts = self.remote_drafts.len() + self.store.len();
        let dirty = crate::collab::materialize::dirty(&self.repo);
        let commits = deep
            .head
            .as_deref()
            .map_or(0, |head| crate::collab::materialize::local_only_commits(&self.repo, head));
        self.status = format!(
            "end deep review? {drafts} unsynced draft(s), {} edits, {commits} local commit(s) would be deleted — press X again to confirm",
            if dirty { "uncommitted" } else { "no uncommitted" },
        );
    }

    /// The armed-and-confirmed End Deep Review, for the loop to execute.
    pub fn take_end_deep(&mut self) -> Option<DeepMode> {
        if self.deep.as_ref().is_some_and(|deep| deep.end_armed && self.pending_end_confirmed) {
            self.pending_end_confirmed = false;
            return self.deep.take();
        }
        None
    }

    /// Note the second `Shift+X` press.
    pub fn confirm_end_deep(&mut self) {
        if self.deep.as_ref().is_some_and(|deep| deep.end_armed) {
            self.pending_end_confirmed = true;
        }
    }

    /// Any non-`X` key disarms a pending End Deep Review.
    pub fn disarm_end_deep(&mut self) {
        if let Some(deep) = &mut self.deep
            && deep.end_armed
        {
            deep.end_armed = false;
            self.pending_end_confirmed = false;
        }
    }

    /// `U` — queue the explicit review-head update.
    pub fn request_deep_update(&mut self) {
        if self.deep.as_ref().is_some_and(|deep| deep.head_moved.is_some()) {
            self.pending_deep_update = true;
        } else {
            self.status = "review head is current".to_string();
        }
    }

    /// The queued update request.
    pub fn take_deep_update(&mut self) -> bool {
        std::mem::take(&mut self.pending_deep_update)
    }

    /// Whether the reviewer changed persistent draft/comment state since the last drain.
    pub fn take_collab_touched(&mut self) -> bool {
        std::mem::take(&mut self.collab_touched)
    }

    /// The app-owned slice of persistent collaboration state: drafts, comments, refs,
    /// scope, and the logical location.
    pub fn collab_export_app_state(&self) -> serde_json::Value {
        use serde_json::json;
        let drafts: Vec<_> = self
            .remote_drafts
            .iter()
            .map(|pending| {
                let action = match &pending.draft.action {
                    forge::ReviewDraftAction::Inline(anchor) => json!({
                        "type": "inline",
                        "path": anchor.path,
                        "old_path": anchor.old_path,
                        "side": match anchor.side { Side::New => "new", Side::Old => "old" },
                        "line": anchor.line,
                        "start_line": anchor.start_line,
                        "endpoints": anchor.endpoints.map(|(a, b)| json!([
                            endpoint_json(a), endpoint_json(b),
                        ])),
                    }),
                    forge::ReviewDraftAction::Reply { remote_id, author } => json!({
                        "type": "reply",
                        "thread": remote_id.as_ref().map(|id| id.thread_id.clone()),
                        "root_comment_id": remote_id.as_ref().and_then(|id| id.root_comment_id),
                        "author": author,
                    }),
                };
                json!({
                    "provider": match pending.target.provider {
                        forge::Provider::Github => "github",
                        forge::Provider::Gitlab => "gitlab",
                    },
                    "host": pending.target.host,
                    "owner": pending.target.owner,
                    "name": pending.target.name,
                    "number": pending.number,
                    "local_id": pending.draft.local_id,
                    "body": pending.draft.body,
                    "action": action,
                    "error": pending.error,
                    "outcome_unknown": pending.outcome_unknown,
                })
            })
            .collect();
        let comments: Vec<_> = self
            .store
            .iter()
            .map(|c| {
                json!({
                    "file": c.file,
                    "side": match c.side { Side::New => "new", Side::Old => "old" },
                    "start": c.start,
                    "end": c.end,
                    "lines": c.lines,
                    "text": c.text,
                    "diff_anchored": c.diff_anchored,
                })
            })
            .collect();
        let refs: Vec<_> = self
            .collab_refs
            .iter()
            .map(|(draft, at)| match at {
                CollabDraftRef::LocalComment(index) => {
                    json!({"draft": draft, "kind": "local", "at": index})
                }
                CollabDraftRef::RemoteDraft(local_id) => {
                    json!({"draft": draft, "kind": "remote", "at": local_id})
                }
            })
            .collect();
        json!({
            "scope": match self.scope {
                Scope::Uncommitted => "uncommitted",
                Scope::Branch => "branch",
                Scope::LastTurn => "last-turn",
            },
            "location": self.current_location().map(|(path, side, line)| json!({
                "path": path,
                "side": match side { Side::New => "new", Side::Old => "old" },
                "line": line,
            })),
            "drafts": drafts,
            "comments": comments,
            "refs": refs,
        })
    }

    /// Restore [`Self::collab_export_app_state`]. Existing drafts/comments are replaced —
    /// this runs at Deep Review startup, before any interaction.
    pub fn collab_import_app_state(&mut self, doc: &serde_json::Value) {
        use serde_json::Value;
        let side_of = |v: &Value| if v.as_str() == Some("old") { Side::Old } else { Side::New };
        self.remote_drafts = doc["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let provider = match d["provider"].as_str()? {
                    "gitlab" => forge::Provider::Gitlab,
                    _ => forge::Provider::Github,
                };
                let target = crate::git::RepoTarget {
                    provider,
                    host: d["host"].as_str()?.to_string(),
                    owner: d["owner"].as_str()?.to_string(),
                    name: d["name"].as_str()?.to_string(),
                };
                let action = if d["action"]["type"].as_str() == Some("reply") {
                    forge::ReviewDraftAction::Reply {
                        remote_id: d["action"]["thread"].as_str().map(|thread| {
                            forge::RemoteCommentId {
                                thread_id: thread.to_string(),
                                root_comment_id: d["action"]["root_comment_id"].as_u64(),
                            }
                        }),
                        author: d["action"]["author"].as_str().unwrap_or_default().to_string(),
                    }
                } else {
                    forge::ReviewDraftAction::Inline(forge::DiffAnchor {
                        path: d["action"]["path"].as_str()?.to_string(),
                        old_path: d["action"]["old_path"].as_str().map(str::to_string),
                        side: side_of(&d["action"]["side"]),
                        line: d["action"]["line"].as_u64()? as u32,
                        start_line: d["action"]["start_line"].as_u64().map(|l| l as u32),
                        endpoints: endpoints_from_json(&d["action"]["endpoints"]),
                    })
                };
                Some(PendingRemoteDraft {
                    target,
                    number: d["number"].as_u64()?,
                    draft: forge::ReviewDraft {
                        local_id: d["local_id"].as_u64()?,
                        action,
                        body: d["body"].as_str()?.to_string(),
                    },
                    error: d["error"].as_str().map(str::to_string),
                    outcome_unknown: d["outcome_unknown"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        self.next_remote_draft_id = self
            .remote_drafts
            .iter()
            .map(|pending| pending.draft.local_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let comments: Vec<Comment> = doc["comments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                Some(Comment {
                    file: c["file"].as_str()?.to_string(),
                    side: side_of(&c["side"]),
                    start: c["start"].as_u64()? as u32,
                    end: c["end"].as_u64()? as u32,
                    lines: c["lines"].as_str().unwrap_or_default().to_string(),
                    text: c["text"].as_str()?.to_string(),
                    diff_anchored: c["diff_anchored"].as_bool().unwrap_or(false),
                })
            })
            .collect();
        self.store = CommentStore::new();
        for comment in comments {
            self.store.add(comment);
        }
        self.collab_refs = doc["refs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let draft = r["draft"].as_str()?.to_string();
                let at = r["at"].as_u64()?;
                Some(match r["kind"].as_str()? {
                    "local" => (draft, CollabDraftRef::LocalComment(at as usize)),
                    _ => (draft, CollabDraftRef::RemoteDraft(at)),
                })
            })
            .collect();
    }

    /// Keep [`CollabDraftRef::LocalComment`] indices in step after a store removal.
    fn collab_refs_on_local_removal(&mut self, removed: usize) {
        self.collab_refs.retain_mut(|(_, r)| match r {
            CollabDraftRef::LocalComment(i) if *i == removed => false,
            CollabDraftRef::LocalComment(i) => {
                if *i > removed {
                    *i -= 1;
                }
                true
            }
            CollabDraftRef::RemoteDraft(_) => true,
        });
    }

    pub fn line_decoration(&self, row: &Row) -> Option<crate::diff::LineDecoration> {
        (self.tab == Tab::AllFiles)
            .then(|| row.new_no().and_then(|line| self.line_decorations.get(&line).copied()))
            .flatten()
    }

    /// Whether the deep-session agent changed this row's line since the session baseline
    /// (the `✦` gutter badge). Not tab-gated: any view built from worktree content carries
    /// the marks; views that aren't (a pinned remote patch) have an empty `pi_marks`.
    pub fn pi_line_decoration(&self, row: &Row) -> Option<crate::diff::LineDecoration> {
        if self.pi_marks.removed_file {
            // The removal has no worktree lines to key on; the whole view is the agent's
            // deletion, so every content row of it badges.
            return row.is_content().then_some(crate::diff::LineDecoration::Deleted);
        }
        row.new_no().and_then(|line| self.pi_marks.lines.get(&line).copied())
    }

    /// The displayed file's changes since the deep-session baseline: worktree content
    /// against [`Self::collab_baseline`]. A file absent from the baseline (the agent
    /// created it) marks every line; one whose content the agent removed entirely sets
    /// [`PiMarks::removed_file`]. Empty outside a deep session.
    fn pi_marks_for(&self, path: &str) -> PiMarks {
        let Some(baseline) = &self.collab_baseline else {
            return PiMarks::default();
        };
        let old = git::file_content(&self.repo, baseline, path);
        let new = worktree_content(&self.repo, path);
        if old == new {
            return PiMarks::default();
        }
        if new.is_empty() {
            return PiMarks { lines: HashMap::new(), removed_file: true };
        }
        PiMarks { lines: crate::diff::line_decorations(&old, &new), removed_file: false }
    }

    /// The number of files changed in the active scope — the header count, the same on both
    /// tabs (specs/tui.md), since `All files` lists the worktree but counts the changeset.
    pub fn changed_count(&self) -> usize {
        if self.tab == Tab::Changes
            && let RemoteChanges::Ready { patch, .. } = &self.remote_changes
        {
            return patch.files.len();
        }
        if self.tab == Tab::Changes && self.remote_changes_active() {
            return 0;
        }
        self.changed.len()
    }

    /// Whether a comment's anchor may have moved. A diff comment is stale once its file
    /// leaves the changeset. A File-view (content) comment is stale once its file is gone —
    /// or once the lines it captured no longer sit at its anchor, which is how a live agent
    /// edit shifting the code marks the anchor instead of silently binding the comment to
    /// unrelated lines (specs/review-model.md).
    pub fn is_stale(&self, c: &Comment) -> bool {
        if c.diff_anchored {
            return !self.changed.contains_key(&c.file);
        }
        if !self.repo.join(&c.file).exists() {
            return true;
        }
        // A removed-line anchor has nothing on disk to verify against; presence is enough.
        if c.side == Side::Old {
            return false;
        }
        // Stored lines carry their one-character diff marker; the worktree carries none.
        let captured: Vec<&str> = c.lines.lines().map(|l| l.get(1..).unwrap_or("")).collect();
        worktree_snippet(&self.repo, &c.file, c.start, c.end)
            .is_none_or(|current| current.lines().ne(captured.iter().copied()))
    }

    fn clamp_list_cursor(&mut self) {
        if self.list_cursor >= self.store.len() {
            self.list_cursor = self.store.len().saturating_sub(1);
        }
    }
}

/// One range endpoint as store JSON.
fn endpoint_json(endpoint: crate::diff::RangeEndpoint) -> serde_json::Value {
    serde_json::json!({
        "old": endpoint.old_pos,
        "new": endpoint.new_pos,
        "kind": match endpoint.kind {
            crate::diff::EndpointKind::Added => "added",
            crate::diff::EndpointKind::Removed => "removed",
            crate::diff::EndpointKind::Context => "context",
        },
    })
}

/// The endpoint pair back from store JSON.
fn endpoints_from_json(
    value: &serde_json::Value,
) -> Option<(crate::diff::RangeEndpoint, crate::diff::RangeEndpoint)> {
    let pair = value.as_array()?;
    let one = |v: &serde_json::Value| {
        Some(crate::diff::RangeEndpoint {
            old_pos: v["old"].as_u64()? as u32,
            new_pos: v["new"].as_u64()? as u32,
            kind: match v["kind"].as_str()? {
                "added" => crate::diff::EndpointKind::Added,
                "removed" => crate::diff::EndpointKind::Removed,
                _ => crate::diff::EndpointKind::Context,
            },
        })
    };
    Some((one(pair.first()?)?, one(pair.get(1)?)?))
}

/// A local comment's stable tray identity: its anchored location. Editing the text keeps
/// the identity; moving the anchor is a different comment.
fn local_comment_key(comment: &Comment) -> String {
    format!("local:{}", comment.location())
}

/// A local comment as a protocol resource — same shape as a remote discussion, with the
/// captured diff lines standing in as its patch evidence.
fn local_comment_resource(comment: &Comment) -> serde_json::Value {
    serde_json::json!({
        "kind": "local-comment",
        "author": "reviewer",
        "anchor": comment.location(),
        "body": comment.text,
        "patch": comment.lines,
        "resolved": false,
        "outdated": false,
        "replies_complete": true,
        "replies": [],
        "thread": serde_json::Value::Null,
    })
}

/// Lines `start..=end` (1-based) of a worktree file, for an agent finding's snippet.
/// `None` when the file or range does not exist — the stage is rejected, never guessed.
fn worktree_snippet(repo: &std::path::Path, path: &str, start: u32, end: u32) -> Option<String> {
    let content = std::fs::read_to_string(repo.join(path)).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if start == 0 || start > end || (end as usize) > lines.len() {
        return None;
    }
    Some(lines[start as usize - 1..end as usize].join("\n"))
}

/// Step `cur` by `delta` within `0..n`, clamping at both ends.
fn step(cur: usize, delta: isize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let max = n - 1;
    if delta >= 0 {
        (cur + delta as usize).min(max)
    } else {
        cur.saturating_sub(delta.unsigned_abs())
    }
}

/// Move `scroll` the minimal amount so the row at `cursor` fits within a `viewport`-tall
/// window, given each row's display `heights`. Scrolls up when the cursor is above the top,
/// advances the top until the cursor's row fits, then pulls back so the bottom isn't left
/// blank — the shared "keep the cursor visible" rule for both panes (the file list passes
/// all-height-1 rows, where this degenerates to plain row arithmetic).
fn keep_in_view(cursor: usize, scroll: usize, heights: &[usize], viewport: usize) -> usize {
    if viewport == 0 || heights.is_empty() {
        return 0;
    }
    let cursor = cursor.min(heights.len() - 1);
    let mut top = scroll.min(cursor);
    while top < cursor && heights[top..=cursor].iter().sum::<usize>() > viewport {
        top += 1;
    }
    while top > 0 && heights[top - 1..].iter().sum::<usize>() <= viewport {
        top -= 1;
    }
    top
}

/// Clamp a scroll offset so a `viewport`-tall window over `total` rows shows no blank tail
/// (and 0 when the content fits). Called every frame after any reveal.
fn bound(scroll: usize, total: usize, viewport: usize) -> usize {
    scroll.min(total.saturating_sub(viewport))
}

/// The start of the logical line (after the previous `\n`, or 0) containing char `caret`.
fn line_start(v: &[char], caret: usize) -> usize {
    v[..caret].iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1)
}

/// The end of the logical line (the next `\n`, or the end) containing char `caret`.
fn line_end(v: &[char], caret: usize) -> usize {
    v[caret..].iter().position(|&c| c == '\n').map_or(v.len(), |p| caret + p)
}

/// The start of the word before `caret`: skip trailing whitespace, then the word run.
fn word_start(v: &[char], caret: usize) -> usize {
    let mut i = caret;
    while i > 0 && v[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !v[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The end of the word after `caret`: skip leading whitespace, then the word run.
fn word_end(v: &[char], caret: usize) -> usize {
    let mut i = caret;
    while i < v.len() && v[i].is_whitespace() {
        i += 1;
    }
    while i < v.len() && !v[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Move a scroll offset by `delta` rows, saturating at 0. The upper bound is applied
/// separately by `bound` once the frame's viewport is known.
fn offset_by(scroll: usize, delta: isize) -> usize {
    if delta >= 0 {
        scroll.saturating_add(delta.unsigned_abs())
    } else {
        scroll.saturating_sub(delta.unsigned_abs())
    }
}

/// The working-tree content of `path`, lossily as UTF-8; empty when the file is
/// absent (a deletion) or unreadable.
fn worktree_content(repo: &std::path::Path, path: &str) -> String {
    std::fs::read(repo.join(path))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn descendant_of(candidate: &str, directory: &str) -> bool {
    candidate.strip_prefix(directory).is_some_and(|suffix| suffix.starts_with('/'))
}

fn line_in(c: &Comment, row: &Row) -> bool {
    let no = match c.side {
        Side::New => row.new_no(),
        Side::Old => row.old_no(),
    };
    no.is_some_and(|n| c.start <= n && n <= c.end)
}

/// The inclusive `(side, start, end)` line range a forge anchor covers.
fn anchor_range(anchor: &forge::DiffAnchor) -> (Side, u32, u32) {
    let start = anchor.start_line.unwrap_or(anchor.line).min(anchor.line);
    (anchor.side, start, anchor.line.max(start))
}

/// Whether `row` carries a line inside the anchored range on its side.
fn row_in_range(row: &Row, (side, start, end): (Side, u32, u32)) -> bool {
    let no = match side {
        Side::New => row.new_no(),
        Side::Old => row.old_no(),
    };
    no.is_some_and(|n| start <= n && n <= end)
}

/// Compute `(side, start, end, snippet)` for a selection of diff rows.
///
/// New-side numbers win when present (insertion/context rows); a pure deletion
/// anchors to the old side. The snippet keeps each row's `+`/`−`/space marker.
fn anchor(selected: &[&Row]) -> Option<(Side, u32, u32, String)> {
    // A selection may straddle a collapsed fold; anchor only over its content rows.
    let selected: Vec<&Row> = selected.iter().copied().filter(|r| r.is_content()).collect();
    if selected.is_empty() {
        return None;
    }
    let snippet = selected.iter().map(|r| r.marker_text()).collect::<Vec<_>>().join("\n");
    let new_nos: Vec<u32> = selected.iter().filter_map(|r| r.new_no()).collect();
    if let (Some(&min), Some(&max)) = (new_nos.iter().min(), new_nos.iter().max()) {
        return Some((Side::New, min, max, snippet));
    }
    let old_nos: Vec<u32> = selected.iter().filter_map(|r| r.old_no()).collect();
    let min = *old_nos.iter().min()?;
    let max = *old_nos.iter().max()?;
    Some((Side::Old, min, max, snippet))
}

#[cfg(test)]
mod tests {
    use super::{App, Mode, ProjectSwitcher};
    use crate::forge::{CheckStatus, PrFetchInput, PrListItem, PrListing, PrState, Provider};
    use crate::git::{OriginIdentity, RepoTarget};
    use crate::model::{Comment, Scope, Side};
    use crate::switcher::Project;
    use std::path::PathBuf;

    fn switcher_over(names: &[&str]) -> ProjectSwitcher {
        ProjectSwitcher::new(
            names
                .iter()
                .map(|n| Project { name: (*n).to_string(), path: PathBuf::from(format!("/p/{n}")) })
                .collect(),
        )
    }

    fn pr_input(target: RepoTarget) -> PrFetchInput {
        PrFetchInput {
            origin: OriginIdentity::Repository(target),
            branch: Some("feature".into()),
            head_oid: Some("abc".into()),
            candidates: vec!["feature".into()],
            base: None,
            base_branches: vec!["main".into()],
            pinned: None,
        }
    }

    fn listing(number: u64) -> PrListing {
        PrListing {
            open: vec![PrListItem {
                number,
                title: format!("MR {number}"),
                head_ref: "feature".into(),
                author: "reviewer".into(),
                is_draft: false,
                state: PrState::Open,
                ci: Some(CheckStatus::Success),
                created_at: "2026-07-01T00:00:00Z".into(),
                comments: 0,
                threads_open: Some(0),
                threads_resolved: Some(0),
            }],
            done: Vec::new(),
        }
    }

    #[test]
    fn picker_generation_rejects_an_old_completion_after_a_target_round_trip() {
        let target_a = RepoTarget {
            provider: Provider::Gitlab,
            host: "gitlab.example.com".into(),
            owner: "a".into(),
            name: "project".into(),
        };
        let target_b = RepoTarget { owner: "b".into(), ..target_a.clone() };
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);

        app.reconcile_pr_pin(&mut pr_input(target_a.clone()));
        let old_a = app.take_pr_picker_fetch().expect("first A request");
        app.reconcile_pr_pin(&mut pr_input(target_b));
        let _b = app.take_pr_picker_fetch().expect("B request");
        app.reconcile_pr_pin(&mut pr_input(target_a));
        let current_a = app.take_pr_picker_fetch().expect("second A request");

        app.pr_picker_loaded(old_a, Ok(listing(1)));
        app.open_pr_picker();
        assert_eq!(app.pr_picker, Some(super::PrPicker::Loading));
        app.pr_picker_loaded(current_a, Ok(listing(2)));
        assert!(matches!(
            app.pr_picker,
            Some(super::PrPicker::Loaded { ref listing, .. }) if listing.open[0].number == 2
        ));
    }

    #[test]
    fn pr_picker_fuzzy_search_covers_identity_branch_author_and_lifecycle() {
        let mut open = listing(42).open.remove(0);
        open.title = "Improve authentication".into();
        open.head_ref = "feature/login".into();
        open.author = "alice".into();
        open.is_draft = true;
        let mut merged = listing(7).open.remove(0);
        merged.title = "Ship dashboard".into();
        merged.state = PrState::Merged;
        let listing = PrListing { open: vec![open], done: vec![merged] };

        assert_eq!(super::filtered_pr_indices(&listing, "!42"), [0]);
        assert_eq!(super::filtered_pr_indices(&listing, "ftrlogin"), [0]);
        assert_eq!(super::filtered_pr_indices(&listing, "ALICE"), [0]);
        assert_eq!(super::filtered_pr_indices(&listing, "draft"), [0]);
        assert_eq!(super::filtered_pr_indices(&listing, "merged"), [1]);
        assert!(super::filtered_pr_indices(&listing, "no-such-review").is_empty());
    }

    #[test]
    fn picker_refresh_keeps_the_highlighted_review_identity() {
        let target = RepoTarget {
            provider: Provider::Github,
            host: "github.com".into(),
            owner: "owner".into(),
            name: "repo".into(),
        };
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.pr_context = Some((target.clone(), Some("feature".into())));
        let mut first = listing(1);
        first.open.push(listing(2).open.remove(0));
        app.pr_picker =
            Some(super::PrPicker::Loaded { listing: first, filtered: vec![0, 1], cursor: 1 });
        let request = super::PrListingRequest { target, generation: 1 };
        app.pr_listing_in_flight = Some(request.clone());
        let mut refreshed = listing(99);
        refreshed.open.push(listing(1).open.remove(0));
        refreshed.open.push(listing(2).open.remove(0));

        app.pr_picker_loaded(request, Ok(refreshed));

        let Some(super::PrPicker::Loaded { listing, filtered, cursor }) = &app.pr_picker else {
            panic!("loaded picker");
        };
        let source = filtered[*cursor];
        assert_eq!(super::PrPicker::row(listing, source).map(|item| item.number), Some(2));
    }

    #[test]
    fn switcher_typing_filters_and_enter_requests_the_switch() {
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.switcher = Some(switcher_over(&["alpha", "beta", "albatross"]));

        app.switcher_input('a');
        app.switcher_input('l');
        let open = app.switcher.as_ref().unwrap();
        assert_eq!(open.filtered, [0, 2]); // "al" prefixes alpha and albatross
        app.switcher_move(1);
        app.switcher_select();

        assert_eq!(app.take_project_switch(), Some(PathBuf::from("/p/albatross")));
        assert!(app.switcher.is_none());
        assert_eq!(app.take_project_switch(), None); // taking clears the request
    }

    #[test]
    fn switcher_backspace_restores_the_wider_match_and_the_cursor_clamps() {
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.switcher = Some(switcher_over(&["alpha", "beta"]));

        app.switcher_input('z');
        assert!(app.switcher.as_ref().unwrap().filtered.is_empty());
        app.switcher_select(); // nothing highlighted: no request, stays open
        assert!(app.switcher.is_some());
        assert_eq!(app.take_project_switch(), None);

        app.switcher_backspace();
        assert_eq!(app.switcher.as_ref().unwrap().filtered, [0, 1]);
        app.switcher_move(5);
        assert_eq!(app.switcher.as_ref().unwrap().cursor, 1); // clamped to the last row
    }

    #[test]
    fn switcher_holds_the_pick_behind_a_confirm_while_comments_are_unsent() {
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.store.add(Comment {
            file: "src/lib.rs".to_string(),
            side: Side::New,
            start: 1,
            end: 1,
            lines: "+line".to_string(),
            text: "unsent".to_string(),
            diff_anchored: true,
        });
        app.switcher = Some(switcher_over(&["alpha", "beta"]));

        app.switcher_select();
        assert_eq!(app.take_project_switch(), None);
        assert!(app.status.contains("unsent comment"));
        // Moving the cursor withdraws the pending confirm — the next enter re-arms it.
        app.switcher_move(1);
        app.switcher_select();
        assert_eq!(app.take_project_switch(), None);
        // The second enter on the same project confirms.
        app.switcher_select();
        assert_eq!(app.take_project_switch(), Some(PathBuf::from("/p/beta")));
        assert!(app.switcher.is_none());
    }

    #[test]
    fn config_recovery_carries_saved_comments_and_the_live_draft() {
        let mut old = App::blocked(PathBuf::from("."), Scope::Uncommitted, None);
        old.store.add(Comment {
            file: "src/lib.rs".to_string(),
            side: Side::New,
            start: 1,
            end: 1,
            lines: "+line".to_string(),
            text: "saved".to_string(),
            diff_anchored: true,
        });
        old.mode = Mode::Composing { editing: None };
        old.resume_list = true;
        old.input = "draft".to_string();
        old.caret = 3;

        let mut recovered = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        recovered.carry_authored_state_from(&mut old);

        assert_eq!(recovered.store.len(), 1);
        assert_eq!(recovered.input, "draft");
        assert_eq!(recovered.caret, 3);
        assert!(recovered.resume_list);
        assert!(matches!(recovered.mode, Mode::Composing { editing: None }));
    }

    #[test]
    fn config_recovery_keeps_the_comment_list_view_and_navigation() {
        let mut old = App::blocked(PathBuf::from("."), Scope::Branch, None);
        old.mode = Mode::List;
        old.file_cursor = 4;
        old.file_scroll = 2;
        old.diff_cursor = 8;
        old.diff_scroll = 5;
        old.input = "unsent".to_string();

        let mut recovered = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        recovered.carry_authored_state_from(&mut old);

        assert!(matches!(recovered.mode, Mode::List));
        assert_eq!(recovered.scope, Scope::Branch);
        assert_eq!(recovered.file_cursor, 4);
        assert_eq!(recovered.file_scroll, 2);
        assert_eq!(recovered.diff_cursor, 8);
        assert_eq!(recovered.diff_scroll, 5);
        assert_eq!(recovered.input, "unsent");
    }

    #[test]
    fn blocked_app_rejects_normal_repository_work_without_panicking() {
        let mut app = App::blocked(PathBuf::from("."), Scope::Uncommitted, None);
        app.set_config_error("bad config".to_string());

        assert!(app.reload().unwrap_err().to_string().contains("bad config"));
        assert!(app.set_scope(Scope::Branch).is_err());
        assert!(app.set_tab(super::Tab::AllFiles).is_err());
        assert!(app.move_cursor(1).is_err());
        assert!(app.select_file(0).is_err());
        assert!(!app.track_turn());
    }

    fn snapshot_with_thread(target: &RepoTarget) -> crate::forge::PrSnapshot {
        crate::forge::PrSnapshot {
            provider: target.provider,
            number: 42,
            title: "review".into(),
            url: "u".into(),
            state: crate::forge::PrState::Open,
            is_draft: false,
            head_ref: "feature".into(),
            head_is_fork: false,
            base_ref: "main".into(),
            diff_refs: crate::forge::DiffRefs::default(),
            merge: crate::forge::Merge::Clean,
            sync: crate::forge::Sync::Unknown,
            checks: vec![],
            comments: vec![crate::forge::Comment {
                kind: crate::forge::CommentKind::Finding,
                author: "reviewer".into(),
                author_is_bot: false,
                anchor: "src/remote.rs:5".into(),
                body: "existing thread".into(),
                snippet: None,
                created_at: "2026-07-11T00:00:00Z".into(),
                is_resolved: false,
                is_outdated: false,
                reply_count: 0,
                replies: Vec::new(),
                replies_state: crate::forge::RepliesState::Complete,
                diff_anchor: None,
                remote_id: Some(crate::forge::RemoteCommentId {
                    thread_id: "T9".into(),
                    root_comment_id: Some(9),
                }),
            }],
            truncated: false,
            threads_partial: None,
        }
    }

    fn draft(id: &str, body: &str, reply_to: Option<&str>) -> crate::collab::protocol::StagedDraft {
        crate::collab::protocol::StagedDraft {
            draft: id.into(),
            body: body.into(),
            anchor: None,
            reply_to: reply_to.map(str::to_string),
        }
    }

    #[test]
    fn pi_reply_drafts_stage_onto_the_active_review_and_revise_in_place() {
        let target = RepoTarget {
            provider: Provider::Github,
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.pr_context = Some((target.clone(), Some("feature".into())));
        app.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));

        // An unknown thread is a rejection, not a guess.
        let err = app.collab_stage_draft(&draft("d0", "hello", Some("T404"))).unwrap_err();
        assert!(err.contains("T404"), "{err}");
        assert!(app.remote_drafts.is_empty());

        app.collab_stage_draft(&draft("d1", "consider a bound", Some("T9"))).unwrap();
        assert_eq!(app.remote_drafts.len(), 1);
        let pending = &app.remote_drafts[0];
        assert_eq!(pending.number, 42);
        assert_eq!(pending.draft.body, "consider a bound");
        assert!(matches!(
            &pending.draft.action,
            crate::forge::ReviewDraftAction::Reply { remote_id: Some(id), author }
                if id.thread_id == "T9" && author == "reviewer"
        ));

        // A revision rewrites the body and clears a definite failure, but an unknown
        // outcome stays sticky so a lost POST can never be blindly re-armed.
        app.remote_drafts[0].error = Some("HTTP 502".into());
        app.collab_revise_draft(&draft("d1", "tighter wording", Some("T9"))).unwrap();
        assert_eq!(app.remote_drafts[0].draft.body, "tighter wording");
        assert_eq!(app.remote_drafts[0].error, None);
        app.remote_drafts[0].outcome_unknown = true;
        app.collab_revise_draft(&draft("d1", "again", Some("T9"))).unwrap();
        assert!(app.remote_drafts[0].outcome_unknown, "unknown outcomes survive revision");

        // Replying without an active review is a visible failure.
        app.pr = crate::forge::PrView::Pending;
        let err = app.collab_stage_draft(&draft("d2", "x", Some("T9"))).unwrap_err();
        assert!(err.contains("no active remote review"), "{err}");
    }

    #[test]
    fn pi_findings_stage_as_local_comments_with_worktree_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "one\ntwo\nthree\nfour\n").unwrap();
        let mut app = App::new(dir.path().to_path_buf(), Scope::Uncommitted, None);

        let finding = crate::collab::protocol::StagedDraft {
            draft: "f1".into(),
            body: "tighten this".into(),
            anchor: Some(crate::collab::protocol::DraftAnchor {
                path: "src/a.rs".into(),
                line: 3,
                start_line: Some(2),
            }),
            reply_to: None,
        };
        app.collab_stage_draft(&finding).unwrap();
        assert_eq!(app.store.len(), 1);
        let comment = app.store.get(0).unwrap();
        assert_eq!((comment.start, comment.end), (2, 3));
        assert_eq!(comment.lines, " two\n three", "the marker-prefixed snippet is the evidence");
        assert!(app.collab_owned_comment(0), "the card knows it is pi-authored");

        // A range outside the file is rejected, never clamped into meaning something else.
        let ghost = crate::collab::protocol::StagedDraft {
            draft: "f2".into(),
            body: "x".into(),
            anchor: Some(crate::collab::protocol::DraftAnchor {
                path: "src/a.rs".into(),
                line: 99,
                start_line: None,
            }),
            reply_to: None,
        };
        assert!(app.collab_stage_draft(&ghost).is_err());

        // The reviewer editing the pi draft queues an ownership transfer.
        app.mode = Mode::Composing { editing: Some(0) };
        app.input = "my wording".to_string();
        app.submit_comment();
        assert_eq!(app.take_collab_edits(), vec!["f1".to_string()]);
        assert_eq!(app.store.get(0).unwrap().text, "my wording");

        // Revision finds the comment through the ref; deletion re-points refs.
        let revised =
            crate::collab::protocol::StagedDraft { body: "revised".into(), ..finding.clone() };
        app.collab_revise_draft(&revised).unwrap();
        assert_eq!(app.store.get(0).unwrap().text, "revised");
    }

    #[test]
    fn deleting_an_earlier_comment_keeps_later_pi_refs_pointing_home() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\n").unwrap();
        let mut app = App::new(dir.path().to_path_buf(), Scope::Uncommitted, None);
        // A human comment first, then a pi finding behind it.
        app.store.add(Comment {
            file: "a.rs".into(),
            side: Side::New,
            start: 1,
            end: 1,
            lines: "one".into(),
            text: "human note".into(),
            diff_anchored: false,
        });
        let finding = crate::collab::protocol::StagedDraft {
            draft: "f1".into(),
            body: "pi note".into(),
            anchor: Some(crate::collab::protocol::DraftAnchor {
                path: "a.rs".into(),
                line: 2,
                start_line: None,
            }),
            reply_to: None,
        };
        app.collab_stage_draft(&finding).unwrap();
        assert!(app.collab_owned_comment(1));

        // Delete the human comment through the list overlay path.
        app.mode = Mode::List;
        app.list_cursor = 0;
        app.delete_comment();
        assert_eq!(app.store.len(), 1);
        assert!(app.collab_owned_comment(0), "the pi ref followed its comment down one slot");
        let revised = crate::collab::protocol::StagedDraft { body: "still mine".into(), ..finding };
        app.collab_revise_draft(&revised).unwrap();
        assert_eq!(app.store.get(0).unwrap().text, "still mine");
    }

    #[test]
    fn attach_and_toggle_queue_intents_only_with_a_selection() {
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        app.collab_attach();
        assert!(app.take_collab_intents().is_empty());
        assert!(app.status.contains("no comment selected"), "{}", app.status);

        let target = RepoTarget {
            provider: Provider::Github,
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        app.tab = super::Tab::Pr;
        app.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));
        app.pr_cursor = 0;
        app.collab_attach();
        app.collab_toggle_tray();
        let intents = app.take_collab_intents();
        assert_eq!(intents.len(), 2);
        assert!(matches!(&intents[0], super::CollabIntent::Attach(item) if item.key == "T9"));
        assert!(matches!(&intents[1], super::CollabIntent::Toggle(item) if item.key == "T9"));
        assert!(app.take_collab_intents().is_empty(), "the drain empties the queue");
    }

    #[test]
    fn shift_d_resolves_the_target_and_respects_the_guards() {
        let target = RepoTarget {
            provider: Provider::Github,
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);

        // A plain local view targets this worktree — keyed by checkout too, so another
        // branch's parked session is not revived.
        app.start_deep_review();
        let request = app.take_deep_review().expect("queued");
        assert!(request.key.starts_with("local:"));
        assert!(request.key.contains('@'), "a local deep key names its checkout: {}", request.key);
        assert_eq!(request.remote, None);

        // The branch's PR, auto-fetched in the background, does not hijack a local view:
        // a Shift+D from the Changes tab still targets this worktree.
        app.pr_context = Some((target.clone(), Some("feature".into())));
        app.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));
        app.start_deep_review();
        let request = app.take_deep_review().expect("queued");
        assert!(request.key.starts_with("local:"), "a local view stays local: {}", request.key);
        assert_eq!(request.remote, None);

        // Actually looking at the review — the PR tab — targets the review itself.
        app.tab = crate::app::Tab::Pr;
        app.start_deep_review();
        let request = app.take_deep_review().expect("queued");
        assert_eq!(request.key, "github:github.com/o/r#42");
        assert_eq!(request.remote, Some((target, 42)));

        // Ownership does NOT block reinvocation — Shift+D resumes by focusing the
        // labelled workspace; only drafting and sync stay with the owner.
        app.deep_lockout = true;
        app.start_deep_review();
        assert!(app.take_deep_review().is_some(), "repeated Shift+D resumes the session");
    }

    #[test]
    fn the_prompt_snapshot_names_only_the_viewed_review() {
        let target = RepoTarget {
            provider: Provider::Github,
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);

        // The branch's PR, auto-fetched in the background, does not retarget a prompt
        // composed over local changes: Pi's hello, tray items, and snapshot must all
        // speak the same local identity.
        app.pr_context = Some((target.clone(), Some("feature".into())));
        app.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));
        let snapshot = app.collab_snapshot(serde_json::json!([]));
        assert!(
            snapshot.target.starts_with("local:"),
            "a local view keeps its local identity: {}",
            snapshot.target
        );
        assert_eq!(snapshot.source, "uncommitted");

        // Actually looking at the review — the PR tab — names the review itself.
        app.tab = super::Tab::Pr;
        let snapshot = app.collab_snapshot(serde_json::json!([]));
        assert_eq!(snapshot.target, "github:github.com/o/r#42");
        assert_eq!(snapshot.source, "github-pr");
    }

    #[test]
    fn a_deep_instance_speaks_its_bound_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(PathBuf::from("."), Scope::Uncommitted, None);
        let key = "local:/repo@feat/x";
        app.deep = Some(super::DeepMode {
            key: key.into(),
            remote: None,
            head: None,
            branch: None,
            created_worktree: false,
            store: crate::collab::store::SessionStore::for_target(dir.path(), key),
            owner: "deep-test".into(),
            end_armed: false,
            head_moved: None,
        });
        // The session binds by `REVIEWR_COLLAB_TARGET` — the deep key — so hellos, tray
        // items, and snapshots must all carry that key, not the bare worktree's.
        assert_eq!(app.collab_target_key(), key);
    }

    #[test]
    fn collab_app_state_round_trips_drafts_comments_and_refs() {
        let target = RepoTarget {
            provider: Provider::Gitlab,
            host: "git.example.com".into(),
            owner: "g".into(),
            name: "p".into(),
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\n").unwrap();
        let mut app = App::new(dir.path().to_path_buf(), Scope::Uncommitted, None);
        app.pr_context = Some((target.clone(), None));
        app.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));
        app.collab_stage_draft(&draft("d1", "a reply", Some("T9"))).unwrap();
        app.remote_drafts[0].outcome_unknown = true; // must survive the round trip
        let finding = crate::collab::protocol::StagedDraft {
            draft: "f1".into(),
            body: "a finding".into(),
            anchor: Some(crate::collab::protocol::DraftAnchor {
                path: "a.rs".into(),
                line: 2,
                start_line: None,
            }),
            reply_to: None,
        };
        app.collab_stage_draft(&finding).unwrap();

        let doc = app.collab_export_app_state();
        let mut restored = App::new(dir.path().to_path_buf(), Scope::Uncommitted, None);
        restored.collab_import_app_state(&doc);

        assert_eq!(restored.remote_drafts.len(), 1);
        let pending = &restored.remote_drafts[0];
        assert_eq!(pending.number, 42);
        assert_eq!(pending.draft.body, "a reply");
        assert!(pending.outcome_unknown, "lost-POST protection survives restarts");
        assert!(matches!(
            &pending.draft.action,
            crate::forge::ReviewDraftAction::Reply { remote_id: Some(id), .. }
                if id.thread_id == "T9"
        ));
        assert_eq!(restored.store.len(), 1);
        assert_eq!(restored.store.get(0).unwrap().text, "a finding");
        assert!(restored.collab_owned_comment(0), "pi ownership refs survive");
        // New drafts never collide with restored ids.
        assert!(restored.remote_drafts.iter().all(|p| p.draft.local_id < 100));
        restored.pr_context = Some((target.clone(), None));
        restored.pr = crate::forge::PrView::Pr(Box::new(snapshot_with_thread(&target)));
        let next = restored.collab_stage_draft(&draft("d2", "post-restore", Some("T9")));
        assert!(next.is_ok());
        let ids: Vec<_> = restored.remote_drafts.iter().map(|p| p.draft.local_id).collect();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped, "restored and new local ids stay distinct");
    }
}
