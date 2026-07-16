//! herdr-reviewr — a herdr-native review sidebar.
//!
//! Browse an agent's changes (uncommitted / branch), leave line-range comments,
//! and send them back to the agent (or the clipboard) — entirely in a herdr pane.
//!
//! This crate is split into a thin binary (`src/main.rs`) and this library so the
//! interaction logic in [`app`] stays terminal-free and unit-testable. This module
//! owns the terminal lifecycle and the event loop; it maps input events onto
//! [`app::App`] methods and renders with [`ui`].

pub mod app;
pub mod browser;
pub mod collab;
pub mod config;
pub mod diff;
pub mod export;
pub mod file_list;
pub mod forge;
pub mod git;
pub mod herdr;
pub mod highlight;
#[macro_use]
pub mod log;
pub mod model;
pub mod proc;
pub mod sidebar;
pub mod switcher;
pub mod theme;
pub mod turn;
pub mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::Rect;

use crate::app::{App, Focus, Mode};
use crate::config::{Config, PluginConfig};
use crate::export::{Agent, Clipboard};
use crate::model::Scope;

/// Entry point: parse config, set up the terminal, run the loop, restore.
pub fn run() -> Result<()> {
    let cfg = Config::from_env();
    log::init();
    let initial_config = config::plugin_config();
    let mut app = match &initial_config {
        Ok(plugin_config) => ready_app(&cfg, plugin_config.clone()),
        Err(error) => {
            let mut app = App::blocked(cfg.repo.clone(), Scope::Uncommitted, cfg.base.clone());
            app.set_config_error(error.to_string());
            app
        }
    };

    let mut terminal = ratatui::init();
    // Bracketed paste so a multi-line paste arrives as one event, not raw keystrokes whose
    // embedded newlines would submit the comment early.
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    // The kitty keyboard protocol reports modifiers on keys the legacy encoding drops — most
    // notably Ctrl/Alt+arrows — so word-jump by arrow works where the terminal supports it.
    let kbd = supports_keyboard_enhancement().unwrap_or(false);
    logln!("keyboard enhancement supported={kbd}");
    if kbd {
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    // Render before the first load, so a slow, failing, or hung `git` scan shows the reviewr UI
    // instead of the blank pane herdr leaves when the process blocks or exits before it renders
    // (issue #4). Paint the empty frame first; then the initial load, non-fatal — an error opens
    // the sidebar with the reason in the status line, the same contract as a failed poll refresh.
    terminal.draw(|f| ui::render(f, &app))?;
    if initial_config.is_ok()
        && let Err(e) = app.reload()
    {
        logln!("startup reload failed: {e:#}");
        app.status = format!("load failed: {e}");
    }
    let result = event_loop(&mut terminal, &mut app, &cfg);
    if kbd {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Build a fresh working sidebar only after the plugin configuration has validated.
fn ready_app(cfg: &Config, plugin_config: PluginConfig) -> App {
    // A non-repo path is not an error — the sidebar opens to an empty state and starts showing
    // changes if the directory becomes a repo (specs/herdr-host.md).
    let repo = git::toplevel(&cfg.repo).unwrap_or_else(|| cfg.repo.clone());
    logln!("start repo={} poll={:?} base={:?}", repo.display(), cfg.poll, cfg.base);
    let mut app = App::new(repo, Scope::Uncommitted, cfg.base.clone());
    app.set_plugin_config(plugin_config);
    app.set_cli_theme(cfg.theme.clone());
    if let Some(wrap) = cfg.wrap {
        app.wrap = wrap;
    }
    if let Some(key) = &cfg.deep {
        init_deep_mode(&mut app, key);
    }
    app
}

/// Bring a `--deep` instance up on its persisted session: claim exclusive draft ownership,
/// restore drafts/comments/refs, pin the remote review, and record the End/Update identity.
/// A refused claim (another live owner) leaves this instance browse-only and says so.
fn init_deep_mode(app: &mut App, key: &str) {
    use crate::collab::{materialize, store::SessionStore};
    let state = materialize::state_dir();
    let store = SessionStore::for_target(&state, key);
    let owner = format!("deep-{}", std::process::id());
    let doc = match store.claim(&owner) {
        Ok(doc) => doc,
        Err(reason) => {
            app.deep_lockout = true;
            app.status = reason;
            return;
        }
    };
    app.collab_import_app_state(&doc["app"]);
    let remote = doc["remote"].as_object().and_then(|r| {
        let provider = match r.get("provider")?.as_str()? {
            "gitlab" => crate::forge::Provider::Gitlab,
            _ => crate::forge::Provider::Github,
        };
        Some((
            crate::git::RepoTarget {
                provider,
                host: r.get("host")?.as_str()?.to_string(),
                owner: r.get("owner")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
            },
            r.get("number")?.as_u64()?,
        ))
    });
    if let Some((target, number)) = &remote {
        // Pin the review and fetch its snapshot and remote diff immediately — the deep
        // pane's Changes tab is the materialized review, not the (clean) worktree.
        app.pin_review(target.clone(), *number);
    }
    if let Some(location) = doc["app"]["location"].as_object()
        && let (Some(path), Some(line)) = (
            location.get("path").and_then(serde_json::Value::as_str),
            location.get("line").and_then(serde_json::Value::as_u64),
        )
    {
        app.collab_navigate_to(path, Some(line as u32), true);
    }
    // The session baseline everything the agent changes is diffed against (the `✦`
    // gutter marks): restored on resume, else a non-disruptive snapshot of the worktree
    // as it stands right now — pre-existing local WIP lands *inside* the baseline, so
    // only what changes after this moment is ever attributed to the agent. Pinned under
    // a private ref so gc cannot prune the otherwise-unreferenced tree mid-session.
    let baseline = doc["pi_baseline"].as_str().map(str::to_string).or_else(|| {
        let tree = crate::git::snapshot_worktree(&app.repo).ok()?;
        let _ = store.update(|doc| doc["pi_baseline"] = serde_json::json!(&tree));
        Some(tree)
    });
    if let Some(sha) = &baseline {
        let _ = crate::git::pin_deep_baseline(&app.repo, &materialize::key_hash(key), sha);
    }
    app.collab_baseline = baseline;
    app.deep = Some(crate::app::DeepMode {
        key: key.to_string(),
        remote,
        head: doc["head_sha"].as_str().map(str::to_string),
        branch: doc["branch"].as_str().map(str::to_string),
        created_worktree: doc["created_worktree"].as_bool().unwrap_or(false),
        store,
        owner,
        end_armed: false,
        head_moved: None,
    });
    // If the exact Pi session cannot be restored, the fresh one still receives the
    // recoverable context through the store; the loss is labelled in the status line.
    if doc["pi_session_live"].is_null() && doc["session"]["tray"].as_array().is_some() {
        app.status = "deep review resumed — earlier Pi conversation may be lost; context restored"
            .to_string();
    }
}

/// A transient status message (e.g. "sent 3 comments") fades after this long idle.
const STATUS_TTL: Duration = Duration::from_secs(4);

/// While the `PR` tab is active, refetch GitHub at least this often — a fallback for forge-side
/// changes with no local signal (a reviewer's comment). Local pushes and `gh` PR actions refresh
/// sooner, on the agent's turn-end, so this cadence is the slow safety net (specs/forge-host.md).
const PR_POLL: Duration = Duration::from_secs(60);
const PR_LOADING_DELAY: Duration = Duration::from_millis(150);

#[derive(Debug)]
struct TaggedPr {
    generation: u64,
    config_epoch: u64,
    input: crate::forge::PrFetchInput,
    view: crate::forge::PrView,
}

#[derive(Debug)]
enum PrEffect {
    Clear,
    Apply(crate::forge::PrView),
}

/// Owns PR refresh convergence. Generations supersede mid-flight triggers, config epochs reject
/// work from another validated snapshot, and a fresh input probe must match before a completion
/// can paint. An off-tab input change clears stale state but defers its replacement fetch.
#[derive(Debug)]
struct PrRefresh {
    generation: u64,
    current_input: Option<crate::forge::PrFetchInput>,
    pending: Option<TaggedPr>,
    fetch_needed: bool,
}

#[derive(Debug)]
struct PrCoordinator {
    refresh: PrRefresh,
    wait_started: Option<Instant>,
    active_probe_epoch: Option<u64>,
    active_fetch: Option<ActiveFetch>,
    discard_probe_result: bool,
    probe_pending: bool,
}

#[derive(Debug)]
struct ActiveFetch {
    tag: (u64, u64),
    cancelled: Arc<AtomicBool>,
}

impl PrCoordinator {
    fn new(ready: bool) -> Self {
        Self {
            refresh: PrRefresh::new(ready),
            wait_started: ready.then(Instant::now),
            active_probe_epoch: None,
            active_fetch: None,
            discard_probe_result: false,
            probe_pending: ready,
        }
    }

    fn stop(&mut self) {
        self.refresh.invalidate();
        self.wait_started = None;
        self.cancel_fetch();
        self.discard_probe_result = false;
        self.probe_pending = false;
    }

    fn recover(&mut self) {
        self.refresh.invalidate();
        self.refresh.trigger();
        self.wait_started = Some(Instant::now());
        self.cancel_fetch();
        self.discard_probe_result = false;
        self.probe_pending = true;
    }

    fn config_changed(&mut self, active: bool) {
        self.cancel_fetch();
        self.discard_probe_result = false;
        self.refresh.config_changed(active);
        self.probe_pending = true;
    }

    fn cancel_fetch(&self) {
        if let Some(active) = &self.active_fetch {
            active.cancelled.store(true, Ordering::Release);
        }
    }

    fn active_fetch_tag(&self) -> Option<(u64, u64)> {
        self.active_fetch.as_ref().map(|active| active.tag)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigGate {
    Blocked,
    Unchanged,
    Changed { file_reloaded: bool, pr_changed: bool },
}

impl ConfigGate {
    fn ready(self) -> bool {
        self != Self::Blocked
    }

    fn pr_unchanged(self) -> bool {
        !matches!(self, Self::Blocked | Self::Changed { pr_changed: true, .. })
    }

    fn file_reloaded(self) -> bool {
        matches!(self, Self::Changed { file_reloaded: true, .. })
    }
}

impl PrRefresh {
    fn new(ready: bool) -> Self {
        Self { generation: 1, current_input: None, pending: None, fetch_needed: ready }
    }

    fn trigger(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.fetch_needed = true;
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.current_input = None;
        self.pending = None;
        self.fetch_needed = false;
    }

    fn config_changed(&mut self, active: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.fetch_needed = active;
    }

    fn completed(&mut self, completion: TaggedPr, epoch: u64, active: bool) -> bool {
        if completion.generation == self.generation && completion.config_epoch == epoch {
            self.pending = Some(completion);
            true
        } else {
            self.pending = None;
            self.fetch_needed = self.fetch_needed || active;
            false
        }
    }

    fn observed(
        &mut self,
        input: crate::forge::PrFetchInput,
        epoch: u64,
        active: bool,
    ) -> Option<PrEffect> {
        let changed = self.current_input.as_ref().is_some_and(|old| old != &input);
        if changed {
            self.generation = self.generation.wrapping_add(1);
            self.pending = None;
            self.current_input = Some(input);
            self.fetch_needed = active;
            return Some(PrEffect::Clear);
        }
        self.current_input = Some(input.clone());
        if let Some(completion) = self.pending.take() {
            if completion.generation == self.generation
                && completion.config_epoch == epoch
                && completion.input == input
            {
                self.fetch_needed = false;
                return Some(PrEffect::Apply(completion.view));
            }
            self.fetch_needed = true;
        }
        None
    }

    fn take_fetch(&mut self) -> Option<(u64, crate::forge::PrFetchInput)> {
        if !self.fetch_needed {
            return None;
        }
        let input = self.current_input.clone()?;
        self.fetch_needed = false;
        Some((self.generation, input))
    }
}

/// Draw, then wait up to the poll deadline for input; refresh on each tick.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, cfg: &Config) -> Result<()> {
    let poll = cfg.poll;
    let mut last_poll = Instant::now();
    let mut last_pr_poll = Instant::now();
    // Local input probes and GitHub reads run on workers. A completed fetch is applied only after
    // a fresh probe proves its complete input still matches (`specs/forge-host.md`).
    let (probe_tx, probe_rx) = mpsc::channel::<(u64, Result<crate::forge::PrFetchInput, String>)>();
    let (recovery_tx, recovery_rx) = mpsc::channel::<(u64, PluginConfig, App)>();
    let mut recovery_inflight = false;
    let (pr_tx, pr_rx) = mpsc::channel::<TaggedPr>();
    let (picker_tx, picker_rx) =
        mpsc::channel::<(crate::app::PrListingRequest, Result<crate::forge::PrListing, String>)>();
    let (review_diff_tx, review_diff_rx) =
        mpsc::channel::<(crate::forge::ReviewDiffRequest, Result<crate::diff::PatchSet, String>)>();
    let mut review_diff_inflight = 0_usize;
    let (review_sync_tx, review_sync_rx) =
        mpsc::channel::<(crate::forge::ReviewSyncRequest, crate::forge::ReviewSyncOutcome)>();
    let mut pr = PrCoordinator::new(app.plugin_config().is_some());
    // The collaboration host: the Pi extension's socket, session machine, and tray. Pumped
    // once per frame like the fetch channels; its monotonic clock is this loop's epoch.
    let mut collab = crate::collab::CollabHost::start(&app.repo);
    let collab_clock = Instant::now();
    let mut collab_composing = false;
    // Deep Review orchestration runs on a worker; results land here.
    let (deep_tx, deep_rx) = mpsc::channel::<Result<DeepLaunch, String>>();
    let mut collab_dirty = false;
    // A restored Deep Review session re-imports its persistent session slice.
    if let Some(deep) = &app.deep
        && let Some(doc) = deep.store.load()
    {
        collab.import_session_state(&doc["session"]);
        app.collab_tray = collab.tray().iter().map(|e| e.alias.clone()).collect();
    }
    let mut config_epoch = 0_u64;
    let mut validate_before_draw = true;
    let mut status_at = Instant::now();
    let mut last_status = String::new();
    // Fetch the PR snapshot as soon as the panel opens, not on first switching to the tab, so the
    // tab is already populated when the user gets there (specs/forge-host.md).
    app.pr_pending = false;
    while !app.should_quit {
        if let Ok((epoch, target, mut recovered)) = recovery_rx.try_recv() {
            recovery_inflight = false;
            if epoch == config_epoch {
                match config::plugin_config() {
                    Ok(current) if current == target => {
                        recovered.carry_authored_state_from(app);
                        *app = recovered;
                        pr.recover();
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let message = error.to_string();
                        if app.config_error() != Some(message.as_str()) {
                            config_epoch = config_epoch.wrapping_add(1);
                        }
                        app.set_config_error(message);
                        pr.stop();
                    }
                }
            }
        }

        // Collaboration first: protocol frames may stage drafts or request context, and the
        // key handlers may have queued tray commands or ownership transfers — all of which
        // this frame's draw should already reflect.
        let collab_now = collab_clock.elapsed().as_millis() as u64;
        let actions = collab.pump(collab_now);
        collab_dirty |= actions.iter().any(collab_action_persists);
        apply_collab_actions(app, &mut collab, actions);
        for intent in app.take_collab_intents() {
            use crate::collab::session::SessionEvent;
            collab_dirty = true;
            let actions = match intent {
                crate::app::CollabIntent::Attach(item) => collab.attach_replace(item, collab_now),
                crate::app::CollabIntent::Toggle(item) => collab.tray_toggle(item, collab_now),
                crate::app::CollabIntent::FollowToggle => {
                    collab.signal(SessionEvent::FollowToggled, collab_now)
                }
                crate::app::CollabIntent::HistoryBack => {
                    collab.signal(SessionEvent::HistoryBack, collab_now)
                }
                crate::app::CollabIntent::HistoryForward => {
                    collab.signal(SessionEvent::HistoryForward, collab_now)
                }
            };
            apply_collab_actions(app, &mut collab, actions);
        }
        for draft in app.take_collab_edits() {
            collab.draft_edited(&draft, collab_now);
            collab_dirty = true;
        }
        collab_dirty |= app.take_collab_touched();
        if let Some(request) = app.take_deep_review() {
            launch_deep_review(app, &collab, &deep_tx, request);
        }
        if let Ok(result) = deep_rx.try_recv() {
            match result {
                Ok(launch) => {
                    // Setup succeeded: draft ownership moves to the new workspace, and this
                    // pane keeps browsing rights only.
                    app.remote_drafts.retain(|pending| {
                        crate::collab::context::remote_target_key(&pending.target, pending.number)
                            != launch.key
                    });
                    if launch.local {
                        app.store.take_all();
                    }
                    app.deep_lockout = true;
                    app.status = if launch.created {
                        "deep review workspace ready".to_string()
                    } else {
                        "deep review workspace resumed".to_string()
                    };
                }
                Err(error) => app.status = format!("deep review failed: {error}"),
            }
        }
        if let Some(pending_end) = app.take_end_deep() {
            end_deep_review(app, &pending_end);
        }
        deep_head_watch(app);
        if app.take_deep_update() {
            run_deep_update(app);
        }
        if collab_dirty && app.deep.is_some() {
            persist_deep_state(app, &collab);
            collab_dirty = false;
        }
        if app.take_manual_nav() {
            let actions =
                collab.signal(crate::collab::session::SessionEvent::ManualNavigated, collab_now);
            apply_collab_actions(app, &mut collab, actions);
        }
        // Composition edges freeze and release agent navigation.
        if app.composing() != collab_composing {
            collab_composing = app.composing();
            let event = if collab_composing {
                crate::collab::session::SessionEvent::ComposerOpened
            } else {
                crate::collab::session::SessionEvent::ComposerClosed
            };
            let actions = collab.signal(event, collab_now);
            apply_collab_actions(app, &mut collab, actions);
        }
        app.collab_link = Some(collab.link_up());
        app.collab_follow = collab.link_up().then(|| collab.follow_enabled());
        app.collab_grace_ms =
            collab.link_up().then(|| collab.manual_grace_remaining(collab_now)).flatten();
        app.collab_pi_location = (collab.link_up() && !collab.follow_enabled())
            .then(|| collab.pi_location_label())
            .flatten();
        app.collab_history = collab.history_position();

        // Revalidate after synchronous work before its result may paint. Worker completions,
        // input dispatch, and the ordinary poll each validate at their own boundary below, so a
        // slow `gh` request does not turn the 100 ms completion wake-up into repeated TOML I/O.
        if validate_before_draw {
            reconcile_plugin_config(
                app,
                cfg,
                &mut config_epoch,
                &recovery_tx,
                &mut recovery_inflight,
                &mut pr,
            );
            validate_before_draw = false;
        }
        if pr.wait_started.is_some_and(|started| started.elapsed() >= PR_LOADING_DELAY) {
            app.set_pr_refreshing(true);
            pr.wait_started = None;
        }
        // Expire a stale status line: restart the timer when the message changes, and clear
        // it once it has lingered past the TTL, so a notification doesn't stay up forever.
        if app.status != last_status {
            last_status.clone_from(&app.status);
            status_at = Instant::now();
        }
        if !app.status.is_empty() && status_at.elapsed() >= STATUS_TTL {
            app.status.clear();
            last_status.clear();
        }
        // Settle both panes' scroll for this frame's viewport before painting, so the
        // diff window matches what mouse hit-testing will map against. Each pane reveals its
        // cursor only when a navigation requested it (so the wheel can scroll freely), then
        // bounds the offset every frame. While composing, reserve the inline box's rows and
        // keep revealing so the anchored line stays above the growing box.
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let viewport = ui::diff_viewport_height(area, app.list_pct);
        let effective = if app.composing() {
            let box_h = ui::composer_height(app, ui::diff_inner_width(area, app.list_pct));
            viewport.saturating_sub(box_h).max(1)
        } else {
            viewport
        };
        let heights = ui::diff_row_heights(app, area);
        if std::mem::take(&mut app.reveal_diff) || app.composing() {
            app.reveal_diff_cursor(&heights, effective);
        }
        app.bound_diff_scroll(&heights, effective);
        let file_vp = ui::file_viewport_height(area, app.list_pct);
        if std::mem::take(&mut app.reveal_files) {
            app.reveal_file_cursor(file_vp);
        }
        app.bound_file_scroll(file_vp);
        terminal.draw(|f| ui::render(f, app))?;
        // A fetch completion waits for a fresh local-input probe before it may paint.
        if let Ok(completion) = pr_rx.try_recv() {
            let tag = (completion.generation, completion.config_epoch);
            if pr.active_fetch_tag() != Some(tag) {
                continue;
            }
            pr.active_fetch = None;
            let config_gate = reconcile_plugin_config(
                app,
                cfg,
                &mut config_epoch,
                &recovery_tx,
                &mut recovery_inflight,
                &mut pr,
            );
            if config_gate.pr_unchanged() {
                let accepted =
                    pr.refresh.completed(completion, config_epoch, app.tab == crate::app::Tab::Pr);
                if accepted && pr.active_probe_epoch.is_some() {
                    pr.discard_probe_result = true;
                }
                pr.probe_pending = true;
            }
        }

        // A probe result is the authority for the current input. Input changes blank the old PR;
        // a fetch result paints only when this probe exactly matches its tagged input.
        if let Ok((epoch, result)) = probe_rx.try_recv() {
            if pr.active_probe_epoch != Some(epoch) {
                continue;
            }
            pr.active_probe_epoch = None;
            let config_gate = reconcile_plugin_config(
                app,
                cfg,
                &mut config_epoch,
                &recovery_tx,
                &mut recovery_inflight,
                &mut pr,
            );
            if !config_gate.pr_unchanged() || epoch != config_epoch {
                if config_gate == ConfigGate::Unchanged && epoch != config_epoch {
                    pr.config_changed(app.tab == crate::app::Tab::Pr);
                }
            } else if pr.discard_probe_result {
                pr.discard_probe_result = false;
                pr.probe_pending = true;
            } else {
                match result {
                    Err(error) => {
                        app.apply_pr(crate::forge::PrView::Error(error));
                        pr.wait_started = None;
                    }
                    Ok(mut input) => {
                        // The pin rides the probe input: inject it while the branch matches,
                        // drop it on a branch switch; equality then drives the refetch.
                        app.reconcile_pr_pin(&mut input);
                        match pr.refresh.observed(
                            input,
                            config_epoch,
                            app.tab == crate::app::Tab::Pr,
                        ) {
                            Some(PrEffect::Clear) => {
                                app.clear_pr();
                                pr.wait_started =
                                    (app.tab == crate::app::Tab::Pr).then(Instant::now);
                            }
                            Some(PrEffect::Apply(view)) => {
                                app.apply_pr(view);
                                pr.wait_started = None;
                            }
                            None => {}
                        }
                    }
                }
            }
        }

        // Record refresh triggers even while a fetch is in flight; the generation makes the old
        // completion superseded and a new fetch starts after it exits.
        let fallback_poll = app.tab == crate::app::Tab::Pr && last_pr_poll.elapsed() >= PR_POLL;
        if app.pr_pending || fallback_poll {
            app.pr_pending = false;
            last_pr_poll = Instant::now();
            pr.refresh.trigger();
            pr.wait_started.get_or_insert_with(Instant::now);
            pr.probe_pending = true;
        }

        if pr.probe_pending && pr.active_probe_epoch.is_none() && app.plugin_config().is_some() {
            pr.probe_pending = false;
            let (tx, repo, base, plugin_config, epoch) = (
                probe_tx.clone(),
                app.repo.clone(),
                app.base.clone(),
                app.plugin_config().expect("config checked above").clone(),
                config_epoch,
            );
            pr.active_probe_epoch = Some(epoch);
            thread::spawn(move || {
                let input = crate::forge::fetch_input(&repo, base.as_deref(), &plugin_config);
                let _ = tx.send((epoch, input));
            });
        }

        // Target-tagged picker prefetch. It can finish before the overlay opens; App caches the
        // result and rejects completions from a project that has since been switched away.
        if let Some(request) = app.take_pr_picker_fetch() {
            let (tx, repo) = (picker_tx.clone(), app.repo.clone());
            thread::spawn(move || {
                let result = crate::forge::list_prs(&repo, &request.target);
                let _ = tx.send((request, result));
            });
        }
        if let Ok((request, result)) = picker_rx.try_recv() {
            app.pr_picker_loaded(request, result);
        }

        if let Some(request) = app.take_remote_changes_fetch() {
            let (tx, repo) = (review_diff_tx.clone(), app.repo.clone());
            review_diff_inflight += 1;
            thread::spawn(move || {
                let result = crate::forge::fetch_review_diff(&repo, &request);
                let _ = tx.send((request, result));
            });
        }
        if let Ok((request, result)) = review_diff_rx.try_recv() {
            review_diff_inflight = review_diff_inflight.saturating_sub(1);
            app.remote_changes_loaded(request, result)?;
        }

        if let Some(request) = app.take_remote_sync() {
            let (tx, repo) = (review_sync_tx.clone(), app.repo.clone());
            thread::spawn(move || {
                let outcome = crate::forge::sync_review(&repo, &request);
                let _ = tx.send((request, outcome));
            });
        }
        if let Ok((request, outcome)) = review_sync_rx.try_recv() {
            app.remote_sync_finished(&request, &outcome);
        }

        if pr.active_fetch.is_none()
            && pr.active_probe_epoch.is_none()
            && !pr.probe_pending
            && let Some((generation, input)) = pr.refresh.take_fetch()
        {
            let (tx, repo, epoch) = (pr_tx.clone(), app.repo.clone(), config_epoch);
            let cancelled = Arc::new(AtomicBool::new(false));
            pr.active_fetch =
                Some(ActiveFetch { tag: (generation, epoch), cancelled: cancelled.clone() });
            thread::spawn(move || {
                let view = crate::forge::fetch_cancellable(&repo, &input, &cancelled);
                let _ = tx.send(TaggedPr { generation, config_epoch: epoch, input, view });
            });
        }
        // Wake at the status-expiry boundary too, so it clears on time when idle.
        let poll_left = poll.saturating_sub(last_poll.elapsed());
        let mut timeout = if app.status.is_empty() {
            poll_left
        } else {
            poll_left.min(STATUS_TTL.saturating_sub(status_at.elapsed()))
        };
        // While a fetch is in flight, wake often so its result paints promptly when it
        // lands. A linked Pi gets the same bound so protocol requests (prompt context,
        // draft staging) answer within one wake rather than one poll interval.
        if pr.active_fetch.is_some()
            || pr.active_probe_epoch.is_some()
            || app.pr_listing_fetch_active()
            || review_diff_inflight > 0
            || collab.link_up()
        {
            timeout = timeout.min(Duration::from_millis(100));
        }
        if let Some(started) = pr.wait_started {
            timeout = timeout.min(PR_LOADING_DELAY.saturating_sub(started.elapsed()));
        }
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if app.config_error().is_some() {
                        if k.code == KeyCode::Char('q') {
                            app.should_quit = true;
                        }
                        continue;
                    }
                    let config_gate = reconcile_plugin_config(
                        app,
                        cfg,
                        &mut config_epoch,
                        &recovery_tx,
                        &mut recovery_inflight,
                        &mut pr,
                    );
                    if !config_gate.ready() {
                        continue;
                    }
                    if let Err(e) = handle_key(app, k, area) {
                        app.status = format!("error: {e}");
                    }
                    if let Some(path) = app.take_project_switch() {
                        switch_project(app, cfg, &mut pr, &mut config_epoch, &path);
                    }
                    validate_before_draw = true;
                    logln!(
                        "key {:?}{} -> mode={:?} focus={:?} scope={:?} file={}/{} diff_cursor={} scroll={} comments={}",
                        k.code,
                        if k.modifiers.is_empty() {
                            String::new()
                        } else {
                            format!(" {:?}", k.modifiers)
                        },
                        app.mode,
                        app.focus,
                        app.scope,
                        app.file_cursor,
                        app.entries.len(),
                        app.diff_cursor,
                        app.diff_scroll,
                        app.store.len()
                    );
                }
                Event::Mouse(m) => {
                    let config_gate = reconcile_plugin_config(
                        app,
                        cfg,
                        &mut config_epoch,
                        &recovery_tx,
                        &mut recovery_inflight,
                        &mut pr,
                    );
                    if !config_gate.ready() {
                        continue;
                    }
                    // Reuse this frame's `area` and `heights` (computed above for the scroll
                    // settle) so a drag-select doesn't re-measure the whole diff per motion.
                    if let Err(e) = handle_mouse(app, m, area, &heights) {
                        app.status = format!("error: {e}");
                    }
                    validate_before_draw = true;
                    logln!(
                        "mouse {:?} col={} row={} -> focus={:?} file={} diff_cursor={} scroll={} anchor={:?}",
                        m.kind,
                        m.column,
                        m.row,
                        app.focus,
                        app.file_cursor,
                        app.diff_cursor,
                        app.diff_scroll,
                        app.select_anchor
                    );
                }
                // Bracketed paste: insert at the caret while composing, ignored otherwise.
                Event::Paste(text) => {
                    let config_gate = reconcile_plugin_config(
                        app,
                        cfg,
                        &mut config_epoch,
                        &recovery_tx,
                        &mut recovery_inflight,
                        &mut pr,
                    );
                    if !config_gate.ready() {
                        continue;
                    }
                    app.input_paste(&text);
                    validate_before_draw = true;
                    logln!("paste {} chars -> composing={}", text.len(), app.composing());
                }
                _ => {}
            }
        }
        if last_poll.elapsed() >= poll {
            let config_gate = reconcile_plugin_config(
                app,
                cfg,
                &mut config_epoch,
                &recovery_tx,
                &mut recovery_inflight,
                &mut pr,
            );
            if !config_gate.ready() {
                last_poll = Instant::now();
                continue;
            }
            pr.probe_pending = true;
            // Advance the last-turn baseline before reloading, so a turn promoted this poll
            // is visible to this poll's changed-files build. When the agent just went idle, its
            // turn may have pushed or run `gh pr merge`; refetch the PR if the tab is showing it
            // (entering the tab refetches on its own otherwise) (specs/forge-host.md).
            let turn_changed = app.track_turn();
            if turn_changed && app.tab == crate::app::Tab::Pr {
                app.pr_pending = true;
            }
            // A failed refresh must never crash the UI or drop a comment.
            if (!config_gate.file_reloaded() || turn_changed)
                && let Err(e) = app.reload()
            {
                app.status = format!("refresh failed: {e}");
            }
            validate_before_draw = true;
            logln!(
                "poll files={} composing={} diff_cursor={} scroll={}",
                app.entries.len(),
                app.composing(),
                app.diff_cursor,
                app.diff_scroll
            );
            last_poll = Instant::now();
        }
    }
    Ok(())
}

/// What one successful Deep Review launch produced.
#[derive(Debug)]
struct DeepLaunch {
    key: String,
    created: bool,
    /// The target was this pane's own local worktree (local comments hand off too).
    local: bool,
}

/// Whether a host action mutated state the Deep Review store must persist.
fn collab_action_persists(action: &crate::collab::HostAction) -> bool {
    use crate::collab::HostAction;
    matches!(
        action,
        HostAction::StageDraft(_)
            | HostAction::ReviseDraft(_)
            | HostAction::TrayChanged
            | HostAction::FollowChanged(_)
            | HostAction::TurnSettled
    )
}

/// Orchestrate one `Shift+D` on a worker: materialize the target, write the handoff
/// snapshot, and build (or focus) the Herdr workspace. Ownership moves only on success —
/// the handoff document exists either way, but the origin keeps drafting until the launch
/// reports back, and a failed setup leaves everything with this pane.
fn launch_deep_review(
    app: &App,
    collab: &crate::collab::CollabHost,
    tx: &mpsc::Sender<Result<DeepLaunch, String>>,
    request: crate::app::DeepRequest,
) {
    let handoff = serde_json::json!({
        "v": 1,
        "target": request.key,
        "remote": request.remote.as_ref().map(|(target, number)| serde_json::json!({
            "provider": match target.provider {
                crate::forge::Provider::Github => "github",
                crate::forge::Provider::Gitlab => "gitlab",
            },
            "host": target.host,
            "owner": target.owner,
            "name": target.name,
            "number": number,
        })),
        "app": app.collab_export_app_state(),
        "session": collab.export_session_state(),
    });
    let repo = app.repo.clone();
    let pi_model = app.plugin_config().and_then(|c| c.deep_pi_model().map(str::to_owned));
    let tx = tx.clone();
    thread::spawn(move || {
        let _ = tx.send(deep_launch_worker(&repo, &request, handoff, pi_model.as_deref()));
    });
}

/// The blocking half of [`launch_deep_review`].
fn deep_launch_worker(
    repo: &std::path::Path,
    request: &crate::app::DeepRequest,
    mut handoff: serde_json::Value,
    pi_model: Option<&str>,
) -> Result<DeepLaunch, String> {
    use crate::collab::{materialize, store::SessionStore, topology};
    // Probe the launch prerequisites before anything is written: a failed launch must not
    // seed the session store, or retrying it would merge the same handoff drafts into the
    // seeded document and duplicate every one.
    let mut api = topology::SocketApi::from_env()
        .ok_or_else(|| "not inside a herdr session (HERDR_SOCKET_PATH unset)".to_string())?;
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let state = materialize::state_dir();
    let (worktree, head, branch, created_worktree) = match &request.remote {
        Some((target, number)) => {
            let m = materialize::materialize(repo, &state, target.provider, *number, &request.key)?;
            (m.worktree, Some(m.head), Some(m.branch), m.created)
        }
        None => (repo.to_path_buf(), None, None, false),
    };
    handoff["worktree"] = serde_json::json!(worktree.to_string_lossy());
    handoff["head_sha"] = serde_json::json!(head);
    handoff["branch"] = serde_json::json!(branch);
    handoff["created_worktree"] = serde_json::json!(created_worktree);

    let session_store = SessionStore::for_target(&state, &request.key);
    // One locked read-modify-write, so a live deep workspace persisting its state cannot
    // interleave with this merge and lose either side. A seeded document always carries
    // its target; one without is the empty shell, i.e. a first open.
    let mut pi_session = String::new();
    session_store.update(|doc| {
        // The Pi conversation belongs to the session document, not to the target: the id
        // is minted once per session and carried in the store, so deleting the store (End
        // Deep Review) really ends the conversation — a later session on the same target
        // must never resurrect it, which a key-derived id silently did.
        pi_session = doc["pi_session"]
            .as_str()
            .map_or_else(|| mint_pi_session_id(&request.key), str::to_owned);
        if doc["target"].is_string() {
            // A resume keeps the richer state the deep workspace persisted, but drafts
            // the origin composed since must MOVE, not vanish: append them under fresh ids.
            merge_handoff_drafts(doc, &handoff);
        } else {
            // First open: the handoff snapshot seeds the session.
            *doc = std::mem::take(&mut handoff);
        }
        doc["pi_session"] = serde_json::json!(&pi_session);
    })?;

    let spec = topology::WorkspaceSpec {
        label: topology::workspace_label(&request.key),
        worktree: worktree.to_string_lossy().into_owned(),
        reviewr_argv: vec![
            exe.to_string_lossy().into_owned(),
            "--deep".to_string(),
            request.key.clone(),
            worktree.to_string_lossy().into_owned(),
        ],
        pi_argv: {
            let mut argv = vec!["pi".to_string(), "--session-id".to_string(), pi_session.clone()];
            // Pin the configured model so the deep Pi never falls back to a provider the
            // reviewer did not choose (config `deep_pi_model`).
            if let Some(model) = pi_model {
                argv.push("--model".to_string());
                argv.push(model.to_string());
            }
            argv
        },
        env: vec![
            ("REVIEWR_COLLAB_TARGET".to_string(), request.key.clone()),
            // A key-derived socket: a local-target deep pane shares its worktree with the
            // origin sidebar, whose worktree-derived socket is already taken.
            (
                "REVIEWR_COLLAB_SOCKET".to_string(),
                crate::collab::transport::socket_path_for_key(&request.key),
            ),
        ],
    };
    let workspace = topology::ensure_workspace(&mut api, &spec)?;
    Ok(DeepLaunch {
        key: request.key.clone(),
        created: workspace.created,
        local: request.remote.is_none(),
    })
}

/// Append a handoff's drafts, comments, and ownership refs into an existing session
/// document. Draft ids and comment indexes are remapped above what the document already
/// holds, so nothing collides and nothing is lost.
fn merge_handoff_drafts(doc: &mut serde_json::Value, handoff: &serde_json::Value) {
    use serde_json::{Value, json};
    let mut drafts = doc["app"]["drafts"].as_array().cloned().unwrap_or_default();
    let mut next_id = drafts.iter().filter_map(|d| d["local_id"].as_u64()).max().unwrap_or(0);
    let mut id_map: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for draft in handoff["app"]["drafts"].as_array().into_iter().flatten() {
        let mut draft = draft.clone();
        if let Some(old) = draft["local_id"].as_u64() {
            next_id += 1;
            id_map.insert(old, next_id);
            draft["local_id"] = json!(next_id);
        }
        drafts.push(draft);
    }
    let base = doc["app"]["comments"].as_array().map_or(0, Vec::len) as u64;
    let mut comments = doc["app"]["comments"].as_array().cloned().unwrap_or_default();
    comments.extend(handoff["app"]["comments"].as_array().into_iter().flatten().cloned());
    let mut refs = doc["app"]["refs"].as_array().cloned().unwrap_or_default();
    for entry in handoff["app"]["refs"].as_array().into_iter().flatten() {
        let mut entry = entry.clone();
        let at = entry["at"].as_u64();
        if entry["kind"].as_str() == Some("local") {
            if let Some(at) = at {
                entry["at"] = json!(at + base);
            }
        } else if let Some(new_id) = at.and_then(|old| id_map.get(&old)) {
            entry["at"] = json!(new_id);
        }
        refs.push(entry);
    }
    doc["app"]["drafts"] = Value::Array(drafts);
    doc["app"]["comments"] = Value::Array(comments);
    doc["app"]["refs"] = Value::Array(refs);
}

/// Mint the Pi session id for one new Deep Review session. Unique (time, pid, and an
/// in-process counter), not key-derived: resume reads the id back from the session store,
/// and the store's deletion is what orphans the conversation for good.
fn mint_pi_session_id(key: &str) -> String {
    use crate::collab::materialize::key_hash;
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let salt = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let a = key_hash(&format!("{key}|{nanos}|{}|{salt}", std::process::id()));
    let b = key_hash(&format!("{a}|pi"));
    format!("{}-{}-{}-{}-{}", &a[0..8], &a[8..12], &a[12..16], &b[0..4], &b[4..16])
}

/// Watch the active review snapshot for a moved remote head; applying it stays explicit.
fn deep_head_watch(app: &mut App) {
    let Some(deep) = &mut app.deep else { return };
    let (Some((_, number)), Some(known)) = (&deep.remote, &deep.head) else { return };
    let Some(snapshot) = (match &app.pr {
        crate::forge::PrView::Pr(s) if s.number == *number => Some(s),
        _ => None,
    }) else {
        return;
    };
    let live = &snapshot.diff_refs.head_sha;
    if !live.is_empty() && live != known && deep.head_moved.as_deref() != Some(live) {
        deep.head_moved = Some(live.clone());
        app.status = "review head moved on the forge — press U to update".to_string();
    }
}

/// Run the explicit update: fast-forward or rebase the private review branch; conflicts
/// abort whole and report.
fn run_deep_update(app: &mut App) {
    use crate::collab::materialize::{self, UpdateOutcome};
    let Some(deep) = &mut app.deep else { return };
    let Some((target, number)) = deep.remote.clone() else { return };
    app.status = "updating review head…".to_string();
    match materialize::update(&app.repo, target.provider, number) {
        Ok(UpdateOutcome::UpToDate) => app.status = "review head already current".to_string(),
        Ok(UpdateOutcome::FastForwarded(sha) | UpdateOutcome::Rebased(sha)) => {
            if let Some(deep) = &mut app.deep {
                deep.head = Some(sha.clone());
                deep.head_moved = None;
            }
            // Re-anchor the `✦` baseline to the new forge head: its fresh commits are
            // the review's, not the agent's, while the agent's carried-over work still
            // differs from that head and keeps its marks.
            if app.collab_baseline.is_some()
                && let Some(deep) = &app.deep
            {
                let hash = materialize::key_hash(&deep.key);
                if crate::git::pin_deep_baseline(&app.repo, &hash, &sha).is_ok() {
                    let _ = deep.store.update(|doc| doc["pi_baseline"] = serde_json::json!(&sha));
                    app.collab_baseline = Some(sha.clone());
                }
            }
            app.status = "review head updated".to_string();
            if let Err(e) = app.reload() {
                app.status = format!("refresh failed: {e}");
            }
        }
        Ok(UpdateOutcome::Conflict(error)) => {
            app.status = format!("update conflicts — resolve manually: {error}");
        }
        Err(error) => app.status = format!("update failed: {error}"),
    }
}

/// End Deep Review: delete collaboration state and any worktree materialization created;
/// never touch a pre-existing local worktree or any branch that predates the session.
fn end_deep_review(app: &mut App, deep: &crate::app::DeepMode) {
    use crate::collab::{materialize, topology};
    deep.store.delete();
    crate::git::drop_deep_baseline(&app.repo, &materialize::key_hash(&deep.key));
    if deep.created_worktree
        && let (Some(branch), Some((_, _))) = (&deep.branch, &deep.remote)
    {
        // `app.repo` IS the materialized worktree here; remove it via its own git dir.
        let worktree = app.repo.clone();
        let _ = materialize::remove(&worktree, &worktree, branch);
    }
    if let (Some(mut api), Ok(workspace)) =
        (topology::SocketApi::from_env(), std::env::var("HERDR_WORKSPACE_ID"))
    {
        let _ = topology::close_workspace(&mut api, &workspace);
    }
    app.should_quit = true;
}

/// Persist the Deep Review session document after a meaningful mutation.
fn persist_deep_state(app: &App, collab: &crate::collab::CollabHost) {
    let Some(deep) = &app.deep else { return };
    let saved = deep.store.update(|doc| {
        doc["target"] = serde_json::json!(deep.key);
        doc["app"] = app.collab_export_app_state();
        doc["session"] = collab.export_session_state();
        if let Some(pi) = collab.pi_session() {
            doc["pi_session_live"] = serde_json::json!(pi);
        }
    });
    if let Err(error) = saved {
        logln!("deep state save failed: {error}");
    }
}

/// Apply one frame's collaboration actions: snapshots and staging read/mutate app state
/// here, on the loop thread, so a prompt's context is one coherent viewer-state read.
fn apply_collab_actions(
    app: &mut App,
    collab: &mut crate::collab::CollabHost,
    actions: Vec<crate::collab::HostAction>,
) {
    use crate::collab::HostAction;
    for action in actions {
        match action {
            HostAction::SnapshotContext { request } => {
                let snapshot = app.collab_snapshot(collab.tray_json());
                collab.send_context(request, &snapshot);
            }
            HostAction::FocusPi => {
                if let Ok(pane) = crate::herdr::resolve_agent_pane() {
                    let _ = crate::herdr::focus(&pane);
                }
            }
            HostAction::StageDraft(draft) => {
                let outcome = app.collab_stage_draft(&draft);
                let reason = outcome.err();
                collab.draft_staged(&draft.draft, reason.is_none(), reason);
            }
            HostAction::ReviseDraft(draft) => {
                let outcome = app.collab_revise_draft(&draft);
                let reason = outcome.err();
                collab.draft_staged(&draft.draft, reason.is_none(), reason);
            }
            HostAction::TrayChanged => {
                app.collab_tray = collab.tray().iter().map(|e| e.alias.clone()).collect();
            }
            HostAction::TurnStarted => {
                // The Deep Review Pi's turns drive last-turn tracking through the protocol,
                // replacing the herdr agent-status poll this pane cannot see.
                let _ = app.apply_agent_status(Some("working"));
            }
            HostAction::TurnSettled => {
                let _ = app.apply_agent_status(Some("idle"));
                if let Err(e) = app.reload() {
                    app.status = format!("refresh failed: {e}");
                }
            }
            HostAction::Navigate(location) => {
                // Land against the current diff first — most steps resolve without paying
                // a reload. A miss on an edit may just mean the diff lags the agent's
                // change: refresh once and settle for the nearest line on the retry.
                let edit = location.kind == crate::collab::protocol::ActivityKind::Edit;
                let outcome = app.collab_navigate_to(&location.path, location.line, !edit);
                if edit && outcome != crate::app::NavOutcome::Landed {
                    let _ = app.reload();
                    app.collab_navigate_to(&location.path, location.line, true);
                }
            }
            HostAction::FollowChanged(on) => {
                app.status = if on { "following pi".to_string() } else { "follow off".to_string() };
            }
        }
    }
}

/// Re-point the whole session at the picked project: a fresh [`App`] on its repo — the same
/// reset the standalone switcher's close-and-reopen produced — keeping the pane's
/// look-and-feel toggles. The config-epoch bump plus `config_changed` restart the PR
/// machinery, so no in-flight probe or fetch from the old repo can paint. Only this pane
/// moves: no other pane is focused, written to, or closed.
fn switch_project(
    app: &mut App,
    cfg: &Config,
    pr: &mut PrCoordinator,
    config_epoch: &mut u64,
    path: &std::path::Path,
) {
    let repo = git::toplevel(path).unwrap_or_else(|| path.to_path_buf());
    logln!("switch project -> {}", repo.display());
    let mut fresh = App::new(repo, Scope::Uncommitted, None);
    if let Some(config) = app.plugin_config() {
        fresh.set_plugin_config(config.clone());
    }
    fresh.set_cli_theme(cfg.theme.clone());
    fresh.wrap = app.wrap;
    fresh.list_pct = app.list_pct;
    let name = fresh
        .repo
        .file_name()
        .map_or_else(|| fresh.repo.display().to_string(), |n| n.to_string_lossy().into_owned());
    fresh.status = match fresh.reload() {
        Ok(()) => format!("switched to {name}"),
        Err(error) => format!("load failed: {error}"),
    };
    *app = fresh;
    *config_epoch = config_epoch.wrapping_add(1);
    pr.config_changed(app.tab == crate::app::Tab::Pr);
}

fn reconcile_plugin_config(
    app: &mut App,
    cfg: &Config,
    config_epoch: &mut u64,
    recovery_tx: &mpsc::Sender<(u64, PluginConfig, App)>,
    recovery_inflight: &mut bool,
    pr: &mut PrCoordinator,
) -> ConfigGate {
    let previous = app.plugin_config().cloned();
    if !observe_plugin_config(app, cfg, config_epoch, recovery_tx, recovery_inflight) {
        pr.stop();
        return ConfigGate::Blocked;
    }
    let current = app.plugin_config().expect("ready after successful observation");
    let Some(previous) = previous.filter(|previous| previous != current) else {
        return ConfigGate::Unchanged;
    };

    let bases_changed = previous.base_branches() != current.base_branches();
    let file_changed = bases_changed || previous.theme() != current.theme();
    let pr_changed = bases_changed || previous.github_host() != current.github_host();
    if pr_changed {
        pr.config_changed(app.tab == crate::app::Tab::Pr);
    }
    if file_changed {
        // `base_branches` participates in every Branch-scope derivation, and a theme change
        // invalidates highlighted diffs. Rebuild before another input or frame can mix states;
        // `reload` preserves the frozen diff while composing.
        if let Err(error) = app.reload() {
            app.status = format!("config refresh failed: {error}");
        }
    }
    ConfigGate::Changed { file_reloaded: file_changed, pr_changed }
}

/// Observe one complete config snapshot. Invalid state blocks work. Recovery loads a fresh app on
/// a tagged worker, then the event loop revalidates its target and carries authored review state
/// before swapping it in.
fn observe_plugin_config(
    app: &mut App,
    cfg: &Config,
    epoch: &mut u64,
    recovery_tx: &mpsc::Sender<(u64, PluginConfig, App)>,
    recovery_inflight: &mut bool,
) -> bool {
    apply_plugin_config_observation(
        app,
        cfg,
        epoch,
        recovery_tx,
        recovery_inflight,
        config::plugin_config(),
    )
}

fn apply_plugin_config_observation(
    app: &mut App,
    cfg: &Config,
    epoch: &mut u64,
    recovery_tx: &mpsc::Sender<(u64, PluginConfig, App)>,
    recovery_inflight: &mut bool,
    observed: Result<PluginConfig, config::PluginConfigError>,
) -> bool {
    match observed {
        Ok(next) => {
            let recovering = app.plugin_config().is_none();
            let changed = app.plugin_config().is_some_and(|current| current != &next);
            if recovering {
                if !*recovery_inflight {
                    *epoch = epoch.wrapping_add(1);
                    *recovery_inflight = true;
                    let (tx, cfg, target, recovery_epoch) =
                        (recovery_tx.clone(), cfg.clone(), next, *epoch);
                    thread::spawn(move || {
                        let mut recovered = ready_app(&cfg, target.clone());
                        if let Err(error) = recovered.reload() {
                            recovered.status = format!("load failed: {error}");
                        }
                        let _ = tx.send((recovery_epoch, target, recovered));
                    });
                }
                return false;
            } else if changed {
                let current = app.plugin_config().expect("ready config");
                if current.base_branches() != next.base_branches()
                    || current.github_host() != next.github_host()
                {
                    *epoch = epoch.wrapping_add(1);
                }
                app.set_plugin_config(next);
            }
            true
        }
        Err(error) => {
            let message = error.to_string();
            if app.plugin_config().is_some() || app.config_error() != Some(message.as_str()) {
                *epoch = epoch.wrapping_add(1);
            }
            app.set_config_error(message);
            false
        }
    }
}

/// Diff scroll steps: a full page for `PageUp`/`PageDown`, half for `ctrl+u`/`ctrl+d`.
const PAGE: isize = 15;
const HALF_PAGE: isize = 8;

fn handle_key(app: &mut App, key: KeyEvent, area: Rect) -> Result<()> {
    use KeyCode::{
        Backspace, Char, Delete, Down, End, Enter, Esc, Home, Left, PageDown, PageUp, Right, Tab,
        Up,
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Any key but the confirming `X` disarms a pending End Deep Review.
    if key.code != KeyCode::Char('X') {
        app.disarm_end_deep();
    }
    // ⌘←/⌘→ step the edit history. Command chords arrive only under the kitty keyboard
    // protocol, so alt+←/→ mirror them everywhere legacy encoding drops the command key —
    // and where ^i, the would-be forward twin of ^o, is indistinguishable from Tab.
    // (Composing keeps alt+arrows for word jumps.)
    if key.modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::ALT) && !app.composing() {
        match key.code {
            KeyCode::Left => {
                app.collab_history_step(-1);
                return Ok(());
            }
            KeyCode::Right => {
                app.collab_history_step(1);
                return Ok(());
            }
            _ => {}
        }
    }

    // A keypress ends any in-progress divider drag, so opening a modal mid-drag (which makes
    // the mouse handler ignore the releasing Up) can't strand `resizing` true.
    app.resizing = false;

    if app.composing() {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let alt_or_shift = key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        let word = alt || ctrl; // word-jump on Alt/Ctrl + arrow (terminal-dependent)
        // The wrapped width of the box, for vertical (wrapped-row) caret movement.
        let cw = ui::composer_content_width(ui::diff_inner_width(area, app.list_pct));
        match key.code {
            Esc => app.cancel_comment(),
            // Alt/Shift+Enter (and Ctrl+J) insert a newline; plain Enter submits.
            Enter if alt_or_shift => app.input_push('\n'),
            Enter => app.submit_comment(),
            Char('j') if ctrl => app.input_push('\n'),
            Char('w') if ctrl => app.input_delete_word(),
            Char('a') if ctrl => app.caret_home(),
            Char('e') if ctrl => app.caret_end(),
            Char('u') if ctrl => app.input_kill_to_start(),
            Char('k') if ctrl => app.input_kill_to_end(),
            // Word-jump: `Alt+b`/`Alt+f` (readline; survives as ESC-prefixed, unlike modified
            // arrows, which many terminals/multiplexers strip) and modified arrows where they
            // are delivered. These precede the plain-character insert below.
            Char('b') if alt => app.caret_word_left(),
            Char('f') if alt => app.caret_word_right(),
            Left if word => app.caret_word_left(),
            Right if word => app.caret_word_right(),
            Left => app.caret_left(),
            Right => app.caret_right(),
            Up => app.caret = ui::caret_vertical(&app.input, app.caret, cw, false),
            Down => app.caret = ui::caret_vertical(&app.input, app.caret, cw, true),
            Home => app.caret_home(),
            End => app.caret_end(),
            Delete => app.input_delete_forward(),
            Backspace => app.input_backspace(),
            Char(c) if !ctrl => app.input_push(c),
            _ => {}
        }
        return Ok(());
    }

    // The project switcher owns the keys while open. Printable characters type into its
    // filter, so this precedes every plain-letter binding; `ctrl-n`/`ctrl-p` move like fzf.
    if app.switcher.is_some() {
        match key.code {
            Esc => app.close_switcher(),
            Enter => app.switcher_select(),
            Down => app.switcher_move(1),
            Up => app.switcher_move(-1),
            Char('n') if ctrl => app.switcher_move(1),
            Char('p') if ctrl => app.switcher_move(-1),
            Backspace => app.switcher_backspace(),
            Char(c) if !ctrl => app.switcher_input(c),
            _ => {}
        }
        return Ok(());
    }

    // The PR tab: navigate/read, draft replies, and explicitly sync the active review group.
    if app.tab == crate::app::Tab::Pr {
        // The picker overlay owns the keys while open.
        if app.pr_picker.is_some() {
            match key.code {
                Esc => app.close_pr_picker(),
                Enter => app.pr_picker_select(),
                Down => app.pr_picker_move(1),
                Up => app.pr_picker_move(-1),
                Char('n') if ctrl => app.pr_picker_move(1),
                Char('p') if ctrl => app.pr_picker_move(-1),
                Backspace => app.pr_picker_backspace(),
                Char('D') => app.start_deep_review_from_picker(),
                Char(c) if !ctrl => app.pr_picker_input(c),
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            Char('p') if ctrl => app.open_switcher(),
            Char('q') => app.should_quit = true,
            Char('r') => app.pr_pending = true,
            Char('1') => app.set_tab(crate::app::Tab::Changes)?,
            Char('2') => app.set_tab(crate::app::Tab::AllFiles)?,
            Char('o') => app.pr_open(),
            Char('p') => app.open_pr_picker(),
            Char('c') => app.start_pr_reply(),
            Char('a') => app.collab_attach(),
            Char('A') => app.collab_toggle_tray(),
            Char('D') => app.start_deep_review(),
            Char('s' | 'S') => app.request_remote_sync(),
            Esc => app.pr_unpin()?,
            Char('j') | Down => app.pr_move(1),
            Char('k') | Up => app.pr_move(-1),
            // The navigator is short; the read pane is what overflows, so the page keys scroll it.
            PageDown => app.pr_scroll_read(PAGE),
            PageUp => app.pr_scroll_read(-PAGE),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::List {
        match key.code {
            Esc | Char('l' | 'q') => app.close_list(),
            Char('j') | Down => app.list_move(1),
            Char('k') | Up => app.list_move(-1),
            Char('s') => app.export(&Agent),
            Char('y') => app.export(&Clipboard),
            Char('e') => app.start_edit(),
            Char('d') => app.delete_comment(),
            Char('a') => app.collab_attach(),
            Char('A') => app.collab_toggle_tray(),
            _ => {}
        }
        return Ok(());
    }

    match (key.code, ctrl) {
        // ctrl combos first, so they win over the plain `u`/`d` bindings below. Half-page
        // keys move the focused pane's cursor (the view follows), like `j`/`k`.
        (Char('u'), true) => app.move_cursor(-HALF_PAGE)?,
        (Char('d'), true) => app.move_cursor(HALF_PAGE)?,
        (Char('p'), true) => app.open_switcher(),
        (Char('q'), _) => app.should_quit = true,
        (Char('r'), _) if app.remote_changes_active() && app.tab == crate::app::Tab::Changes => {
            app.refresh_remote_changes();
        }
        (Char('r'), _) => app.reload()?,
        // `1` / `2` / `3` switch tabs (provisional; the keymap is an Open Decision in tui.md).
        (Char('1'), _) => app.set_tab(crate::app::Tab::Changes)?,
        (Char('2'), _) => app.set_tab(crate::app::Tab::AllFiles)?,
        (Char('3'), _) => app.set_tab(crate::app::Tab::Pr)?,
        (Tab, _) => app.toggle_focus(),
        (Char('j') | Down, _) => app.move_cursor(1)?,
        (Char('k') | Up, _) => app.move_cursor(-1)?,
        // Page keys move the focused pane's cursor.
        (PageDown, _) => app.move_cursor(PAGE)?,
        (PageUp, _) => app.move_cursor(-PAGE)?,
        (Char('w'), _) => app.toggle_wrap(),
        // `]` widens the file list, `[` narrows it (widening the diff).
        (Char(']'), _) => app.resize_list(4),
        (Char('['), _) => app.resize_list(-4),
        // `←`/`→` expand/collapse the collapsible under the cursor — a directory in the file
        // list, a fold in the diff (expand-only); otherwise they scroll the diff sideways
        // (`scroll_h` is a no-op while wrapping, so it only acts when h-scroll is meaningful).
        (Right, _) if app.on_folder() => app.expand_dir(),
        (Left, _) if app.on_folder() => app.collapse_dir(),
        (Right, _) if app.on_fold() => {
            let heights = ui::diff_row_heights(app, area);
            app.expand_fold(&heights, ui::diff_viewport_height(area, app.list_pct));
        }
        (Right, _) => app.scroll_h(8),
        (Left, _) => app.scroll_h(-8),
        (Char('u'), false) => app.set_scope(Scope::Uncommitted)?,
        (Char('b'), false) => app.set_scope(Scope::Branch)?,
        (Char('t'), false) => app.set_scope(Scope::LastTurn)?,
        (Char('v'), _) => app.toggle_select(),
        (Char('c'), _) => app.start_comment(),
        // `e`/`d` act on the comment under the diff cursor, so they only fire with the diff
        // focused — otherwise `d` would silently delete a comment under an off-screen cursor.
        // (The comments-list overlay has its own `e`/`d`, which target the highlighted row.)
        (Char('e'), _) if app.focus == Focus::Diff => app.start_edit(),
        (Char('d'), false) if app.focus == Focus::Diff => app.delete_comment(),
        (Char('s' | 'S'), _)
            if app.tab == crate::app::Tab::Changes && app.remote_changes_active() =>
        {
            app.request_remote_sync();
        }
        (Char('s' | 'S'), _) => app.export(&Agent),
        (Char('y' | 'Y'), _) => app.export(&Clipboard),
        (Char('n'), _) => app.jump_comment(1),
        (Char('N'), _) => app.jump_comment(-1),
        (Char('l'), _) => app.open_list(),
        // `a`/`Shift+A` hand the comment under the cursor to the Pi session's tray.
        (Char('a'), false) => app.collab_attach(),
        (Char('A'), _) => app.collab_toggle_tray(),
        // `f` toggles following the agent; ⌘←/⌘→ (or ^o/^i where ⌘ never arrives)
        // step through its edit history.
        (Char('f'), false) => app.collab_follow_toggle(),
        (Char('D'), _) => app.start_deep_review(),
        (Char('X'), _) if app.deep.is_some() => {
            if app.deep.as_ref().is_some_and(|deep| deep.end_armed) {
                app.confirm_end_deep();
            } else {
                app.request_end_deep();
            }
        }
        (Char('U'), _) if app.deep.is_some() => app.request_deep_update(),
        (Char('o'), true) => app.collab_history_step(-1),
        (Char('i'), true) => app.collab_history_step(1),
        // `esc` clears an in-progress line selection (the footer's `esc clear`).
        (Esc, _)
            if app.remote_changes_active()
                && app.tab == crate::app::Tab::Changes
                && app.select_anchor.is_none() =>
        {
            app.pr_unpin()?;
        }
        (Esc, _) => app.clear_selection(),
        _ => {}
    }
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent, area: Rect, heights: &[usize]) -> Result<()> {
    // A modal (the comment composer or the comments-list overlay) captures the screen and is
    // keyboard-driven, so the mouse is inert while one is open — otherwise clicks and the
    // wheel would drive the panes drawn underneath it.
    if app.composing()
        || app.mode == Mode::List
        || app.pr_picker.is_some()
        || app.switcher.is_some()
    {
        return Ok(());
    }
    // The PR tab: click a tab or the open button, click a row to read it, wheel the
    // navigator (right) to move, wheel the read pane (left) to scroll.
    if app.tab == crate::app::Tab::Pr {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(ui::HeaderHit::Tab(tab)) = ui::hit_header(area, app, m.column, m.row) {
                    app.set_tab(tab)?;
                } else if ui::hit_pr_open(area, app, m.column, m.row) {
                    app.pr_open();
                } else if let Some(i) = ui::pr_nav_hit(area, app, m.column, m.row) {
                    app.pr_select(i);
                }
            }
            MouseEventKind::ScrollDown
                if ui::in_files_pane(area, app.list_pct, m.column, m.row) =>
            {
                app.pr_move(3);
            }
            MouseEventKind::ScrollUp if ui::in_files_pane(area, app.list_pct, m.column, m.row) => {
                app.pr_move(-3);
            }
            MouseEventKind::ScrollDown => app.pr_scroll_read(3),
            MouseEventKind::ScrollUp => app.pr_scroll_read(-3),
            _ => {}
        }
        return Ok(());
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // The divider is checked first: a grab there starts a resize, not a selection.
            if ui::hit_divider(area, app.list_pct, m.column, m.row) {
                app.resizing = true;
            } else if let Some(hit) = ui::hit_header(area, app, m.column, m.row) {
                match hit {
                    ui::HeaderHit::Tab(tab) => app.set_tab(tab)?,
                    ui::HeaderHit::Scope => app.set_scope(app.scope.cycle())?,
                    ui::HeaderHit::Send
                        if !(app.tab == crate::app::Tab::Changes
                            && app.remote_changes_active()) =>
                    {
                        app.export(&Agent);
                    }
                    ui::HeaderHit::Send => {}
                }
            } else if let Some(i) = ui::hit_file(
                area,
                app.list_pct,
                m.column,
                m.row,
                app.file_rows.len(),
                app.file_scroll,
            ) {
                app.select_file(i)?;
            } else if let Some(i) =
                ui::hit_diff(area, app.list_pct, m.column, m.row, heights, app.diff_scroll)
            {
                app.focus = Focus::Diff;
                app.diff_cursor = i;
                app.select_anchor = None;
                // A click on a fold marker expands it, keeping the viewport still.
                app.expand_fold(heights, ui::diff_viewport_height(area, app.list_pct));
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.resizing {
                let body = ui::body_rect(area);
                app.drag_divider(body.width, m.column.saturating_sub(body.x));
            } else if let Some(i) =
                ui::hit_diff(area, app.list_pct, m.column, m.row, heights, app.diff_scroll)
            {
                app.drag_select_to(i);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => app.resizing = false,
        // The wheel scrolls the viewport of whichever pane it is over — never the cursor, so
        // a comment is never anchored to a wheeled-past line. Horizontal scroll is
        // keyboard-only (`←`/`→`), since multiplexers don't reliably deliver h-wheel events.
        MouseEventKind::ScrollDown if ui::in_files_pane(area, app.list_pct, m.column, m.row) => {
            app.wheel_files(3);
        }
        MouseEventKind::ScrollUp if ui::in_files_pane(area, app.list_pct, m.column, m.row) => {
            app.wheel_files(-3);
        }
        MouseEventKind::ScrollDown => app.wheel_diff(3),
        MouseEventKind::ScrollUp => app.wheel_diff(-3),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod refresh_tests {
    use super::{
        ActiveFetch, PrCoordinator, PrEffect, PrRefresh, TaggedPr, apply_plugin_config_observation,
    };
    use crate::app::App;
    use crate::config::{Config, plugin_config_in};
    use crate::forge::{PrFetchInput, PrView};
    use crate::git::OriginIdentity;
    use crate::model::Scope;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    fn input(head: &str) -> PrFetchInput {
        PrFetchInput {
            pinned: None,
            origin: OriginIdentity::Missing,
            branch: Some("feature".to_string()),
            head_oid: Some(head.to_string()),
            candidates: vec!["feature".to_string()],
            base: None,
            base_branches: vec!["main".to_string()],
        }
    }

    #[test]
    fn superseded_completion_never_applies_and_schedules_the_new_generation() {
        let a = input("a");
        let mut refresh = PrRefresh::new(true);
        assert!(refresh.observed(a.clone(), 0, true).is_none());
        let (old_generation, old_input) = refresh.take_fetch().unwrap();

        refresh.trigger();
        refresh.completed(
            TaggedPr {
                generation: old_generation,
                config_epoch: 0,
                input: old_input,
                view: PrView::NoPr(vec![]),
            },
            0,
            true,
        );
        assert!(refresh.observed(a, 0, true).is_none());

        let (new_generation, _) = refresh.take_fetch().unwrap();
        assert_ne!(new_generation, old_generation);
    }

    #[test]
    fn changed_input_clears_instead_of_applying_a_completed_old_snapshot() {
        let a = input("a");
        let b = input("b");
        let mut refresh = PrRefresh::new(true);
        refresh.observed(a.clone(), 0, true);
        let (generation, old_input) = refresh.take_fetch().unwrap();
        refresh.completed(
            TaggedPr { generation, config_epoch: 0, input: old_input, view: PrView::NoPr(vec![]) },
            0,
            true,
        );

        assert!(matches!(refresh.observed(b, 0, true), Some(PrEffect::Clear)));
    }

    #[test]
    fn stale_config_epoch_and_off_tab_input_change_do_not_start_or_apply_work() {
        let a = input("a");
        let b = input("b");
        let mut refresh = PrRefresh::new(true);
        refresh.observed(a.clone(), 1, true);
        let (generation, old_input) = refresh.take_fetch().unwrap();
        refresh.completed(
            TaggedPr { generation, config_epoch: 1, input: old_input, view: PrView::NoPr(vec![]) },
            2,
            false,
        );
        assert!(matches!(refresh.observed(b, 2, false), Some(PrEffect::Clear)));
        assert!(refresh.take_fetch().is_none());
    }

    #[test]
    fn matching_completion_applies_only_after_the_verification_probe() {
        let a = input("a");
        let mut refresh = PrRefresh::new(true);
        refresh.observed(a.clone(), 3, true);
        let (generation, fetch_input) = refresh.take_fetch().unwrap();
        refresh.completed(
            TaggedPr {
                generation,
                config_epoch: 3,
                input: fetch_input,
                view: PrView::NoPr(vec!["feature".to_string()]),
            },
            3,
            true,
        );

        assert!(matches!(refresh.observed(a, 3, true), Some(PrEffect::Apply(PrView::NoPr(_)))));
    }

    #[test]
    fn a_failed_verification_probe_keeps_the_completion_for_the_next_probe() {
        let a = input("a");
        let mut refresh = PrRefresh::new(true);
        refresh.observed(a.clone(), 0, true);
        let (generation, fetch_input) = refresh.take_fetch().unwrap();
        refresh.completed(
            TaggedPr {
                generation,
                config_epoch: 0,
                input: fetch_input,
                view: PrView::NoPr(vec![]),
            },
            0,
            true,
        );

        assert!(refresh.take_fetch().is_none());
        assert!(matches!(refresh.observed(a, 0, true), Some(PrEffect::Apply(PrView::NoPr(_)))));
    }

    #[test]
    fn config_change_off_the_pr_tab_does_not_schedule_a_fetch() {
        let mut refresh = PrRefresh::new(false);
        refresh.observed(input("a"), 0, false);
        refresh.config_changed(false);
        assert!(refresh.take_fetch().is_none());
    }

    #[test]
    fn cancelling_a_fetch_retains_real_worker_ownership_until_completion() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut coordinator = PrCoordinator::new(true);
        coordinator.active_fetch = Some(ActiveFetch { tag: (7, 3), cancelled: cancelled.clone() });

        coordinator.config_changed(true);

        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(coordinator.active_fetch_tag(), Some((7, 3)));
    }

    #[test]
    fn shell_only_config_changes_do_not_invalidate_runtime_work() {
        let repo = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        std::fs::write(config_dir.path().join("config.toml"), "auto_open = false\n").unwrap();
        let cfg = Config::parse([repo.path().display().to_string()]);
        let mut app = App::new(repo.path().to_path_buf(), Scope::Uncommitted, None);
        let (tx, _rx) = mpsc::channel();
        let mut epoch = 0;
        let mut recovery_inflight = false;

        assert!(apply_plugin_config_observation(
            &mut app,
            &cfg,
            &mut epoch,
            &tx,
            &mut recovery_inflight,
            plugin_config_in(config_dir.path()),
        ));
        assert_eq!(epoch, 0);
        assert!(!app.plugin_config().unwrap().auto_open());

        std::fs::write(config_dir.path().join("config.toml"), "base_branches = [\"develop\"]\n")
            .unwrap();
        assert!(apply_plugin_config_observation(
            &mut app,
            &cfg,
            &mut epoch,
            &tx,
            &mut recovery_inflight,
            plugin_config_in(config_dir.path()),
        ));
        assert_eq!(epoch, 1);
    }

    #[test]
    fn invalid_then_valid_observation_blocks_and_recovers_through_a_fresh_worker() {
        let repo = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let path = config_dir.path().join("config.toml");
        std::fs::write(&path, "unknown = true\n").unwrap();
        let cfg = Config::parse([repo.path().display().to_string()]);
        let mut app = App::new(repo.path().to_path_buf(), Scope::Uncommitted, None);
        let (tx, rx) = mpsc::channel();
        let mut epoch = 0;
        let mut recovery_inflight = false;

        assert!(!apply_plugin_config_observation(
            &mut app,
            &cfg,
            &mut epoch,
            &tx,
            &mut recovery_inflight,
            plugin_config_in(config_dir.path()),
        ));
        assert!(app.plugin_config().is_none());
        assert!(app.config_error().unwrap().contains("unknown key"));

        std::fs::write(&path, "theme = \"gruvbox\"\n").unwrap();
        assert!(!apply_plugin_config_observation(
            &mut app,
            &cfg,
            &mut epoch,
            &tx,
            &mut recovery_inflight,
            plugin_config_in(config_dir.path()),
        ));
        let (recovery_epoch, target, recovered) =
            rx.recv_timeout(Duration::from_secs(5)).expect("recovery worker");
        assert_eq!(recovery_epoch, epoch);
        assert_eq!(target.theme(), "gruvbox");
        assert_eq!(recovered.plugin_config().unwrap().theme(), "gruvbox");
    }
}
