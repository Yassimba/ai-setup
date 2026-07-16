//! End-to-end tests of the review loop: `App` driven against real repos, with a
//! fake export target so consume-on-success is checked without a live agent.

mod common;

use std::cell::RefCell;

use anyhow::{Result, bail};
use common::Repo;
use herdr_reviewr::app::{App, Focus, FooterAction, Mode, NavOutcome};
use herdr_reviewr::export::ExportTarget;
use herdr_reviewr::model::{Scope, Side};

/// An export target that records what it was handed and can be made to fail.
struct FakeTarget {
    ok: bool,
    captured: RefCell<Vec<String>>,
}

impl FakeTarget {
    fn ok() -> Self {
        Self { ok: true, captured: RefCell::new(Vec::new()) }
    }
    fn failing() -> Self {
        Self { ok: false, captured: RefCell::new(Vec::new()) }
    }
    fn last(&self) -> String {
        self.captured.borrow().last().cloned().unwrap_or_default()
    }
}

impl ExportTarget for FakeTarget {
    fn label(&self) -> &'static str {
        "fake"
    }
    fn export(&self, text: &str) -> Result<()> {
        self.captured.borrow_mut().push(text.to_string());
        if self.ok { Ok(()) } else { bail!("fake export failure") }
    }
}

/// A repo whose single tracked file `a.rs` has an edit and an appended line.
fn edited_repo() -> Repo {
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\ngamma\ndelta\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\ngamma\ndelta\nepsilon\n");
    r
}

fn app_on(r: &Repo) -> App {
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app
}

/// Settle the diff scroll with one display row per logical row (no wrap), for tests that
/// drive short-line diffs — reveal the cursor, then bound the offset, as the loop does.
fn clamp(app: &mut App, viewport: usize) {
    let heights = vec![1usize; app.visible.len()];
    app.reveal_diff_cursor(&heights, viewport);
    app.bound_diff_scroll(&heights, viewport);
}

#[test]
fn the_file_list_decouples_viewport_scroll_from_selection() {
    let r = Repo::init();
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "one\n");
    }
    r.commit_all("init");
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "two\n");
    }
    let mut app = app_on(&r);
    assert_eq!(app.file_rows.len(), 20);
    let viewport = 6;

    // The first file is selected and its diff is open.
    assert_eq!(app.file_cursor, 0);
    let opened = app.diff_path.clone();
    assert!(opened.is_some());

    // Wheel-scrolling moves the viewport only: the selection and the open diff stay put,
    // so browsing the list never reloads a diff (the performance fix). It may leave the
    // cursor off screen — it is not yanked back.
    app.reveal_files = false; // clear the flag the initial reload set (no event loop here)
    app.wheel_files(5);
    app.bound_file_scroll(viewport);
    assert_eq!(app.file_scroll, 5);
    assert_eq!(app.file_cursor, 0);
    assert_eq!(app.diff_path, opened);
    assert!(app.file_cursor < app.file_scroll);
    assert!(!app.reveal_files, "the wheel does not request a reveal");

    // Moving the selection reveals it and opens that one file.
    app.move_cursor(1).unwrap();
    app.reveal_file_cursor(viewport);
    assert_eq!(app.file_cursor, 1);
    assert!(app.file_cursor >= app.file_scroll && app.file_cursor < app.file_scroll + viewport);
    assert_ne!(app.diff_path, opened);

    // Keyboard nav to the bottom keeps the cursor visible (reveal on each move).
    for _ in 0..18 {
        app.move_cursor(1).unwrap();
    }
    app.reveal_file_cursor(viewport);
    assert_eq!(app.file_cursor, 19);
    assert!(app.file_cursor < app.file_scroll + viewport);
    assert_eq!(app.file_scroll, 20 - viewport);

    // An over-scroll is bounded so the window never shows a blank tail.
    app.wheel_files(100);
    app.bound_file_scroll(viewport);
    assert_eq!(app.file_scroll, 20 - viewport);
}

/// A repo whose single file has `n` lines, all changed, so the diff has many visible rows.
fn long_diff_app(n: usize) -> App {
    use std::fmt::Write as _;
    let r = Repo::init();
    let (mut old, mut new) = (String::new(), String::new());
    for i in 0..n {
        let _ = writeln!(old, "line {i}");
        let _ = writeln!(new, "LINE {i}");
    }
    r.write("a.rs", &old);
    r.commit_all("init");
    r.write("a.rs", &new);
    let mut app = app_on(&r);
    app.reload().unwrap();
    app
}

#[test]
fn bound_diff_scroll_keeps_a_wrapped_bottom_reachable() {
    // 30 logical rows, each 3 display lines tall, in a 20-display-row viewport. A row-count
    // cap would stop the scroll at 30-20=10, hiding the last ~13 rows; the height-aware cap
    // must reach the offset that shows the last row.
    let mut app = long_diff_app(5);
    let heights = vec![3usize; 30];
    app.diff_scroll = 999; // wheel over-scroll
    app.bound_diff_scroll(&heights, 20);
    assert!(
        app.diff_scroll > 10,
        "height-aware bound passes the row-count cap: {}",
        app.diff_scroll
    );
    assert!(app.diff_scroll <= 29);
}

#[test]
fn the_wheel_scrolls_the_diff_without_moving_its_cursor() {
    let mut app = long_diff_app(40);
    app.focus = Focus::Diff;
    app.diff_cursor = 3;
    app.reveal_diff = false;
    app.wheel_diff(10);
    let h = vec![1usize; app.visible.len()];
    app.bound_diff_scroll(&h, 8);
    assert_eq!(app.diff_cursor, 3, "the wheel leaves the comment cursor put");
    assert!(app.diff_scroll > 0, "the wheel moved the viewport");
    assert!(!app.reveal_diff, "the wheel does not request a reveal");
}

#[test]
fn a_boundary_move_reveals_the_cursor_after_wheeling() {
    // The B1 regression: a navigation that clamps to the same index must still reveal.
    let r = Repo::init();
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "one\n");
    }
    r.commit_all("init");
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "two\n");
    }
    let mut app = app_on(&r);
    let vp = 6;
    app.wheel_files(10);
    app.bound_file_scroll(vp);
    assert!(app.file_cursor < app.file_scroll, "cursor (row 0) is wheeled off-screen above");
    app.reveal_files = false;
    app.move_cursor(-1).unwrap(); // `k` at row 0 — index stays 0
    assert_eq!(app.file_cursor, 0);
    assert!(app.reveal_files, "a clamp-to-same-index move still requests a reveal");
    app.reveal_file_cursor(vp);
    assert_eq!(app.file_scroll, 0, "the cursor is pulled back into view");
}

#[test]
fn toggling_a_directory_requests_a_reveal() {
    let r = Repo::init();
    r.write("src/a.rs", "x\n");
    r.write("src/b.rs", "y\n");
    r.commit_all("init");
    r.write("src/a.rs", "x2\n");
    r.write("src/b.rs", "y2\n");
    let mut app = app_on(&r);
    app.focus = Focus::Files;
    let dir = app.file_rows.iter().position(|row| row.dir_path() == Some("src")).unwrap();
    app.file_cursor = dir;
    app.reveal_files = false;
    app.collapse_dir();
    assert!(app.reveal_files, "collapsing a directory requests a reveal (even at the same index)");
}

#[test]
fn page_keys_move_the_cursor_in_both_panes() {
    let mut app = long_diff_app(40);
    // File pane: page moves the selection (not just the viewport).
    app.focus = Focus::Files;
    app.file_cursor = 0;
    app.reveal_files = false;
    app.move_cursor(5).unwrap();
    assert_eq!(app.file_cursor, 5usize.min(app.file_rows.len() - 1));
    assert!(app.reveal_files);
    // Diff pane: page moves the cursor.
    app.focus = Focus::Diff;
    app.diff_cursor = 0;
    app.reveal_diff = false;
    app.move_cursor(5).unwrap();
    assert_eq!(app.diff_cursor, 5);
    assert!(app.reveal_diff);
}

#[test]
fn horizontal_scroll_is_inert_while_wrapping() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.wrap = true;
    app.scroll_h(8);
    assert_eq!(app.h_scroll, 0, "h-scroll does nothing while wrap is on, so it can't accumulate");
    app.wrap = false;
    app.scroll_h(8);
    assert_eq!(app.h_scroll, 8, "h-scroll moves once wrap is off");
}

#[test]
fn a_poll_preserves_the_wheel_scroll_in_both_panes() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let (mut old, mut new) = (String::new(), String::new());
    for i in 0..60 {
        let _ = writeln!(old, "line {i}");
        let _ = writeln!(new, "LINE {i}");
    }
    r.write("big.rs", &old);
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "one\n");
    }
    r.commit_all("init");
    r.write("big.rs", &new);
    for i in 0..20 {
        r.write(&format!("f{i:02}.txt"), "two\n");
    }
    let mut app = app_on(&r);

    // Open the long file and wheel its diff down; the cursor stays at the top.
    app.select_file(file_row(&app, "big.rs")).unwrap();
    app.focus = Focus::Diff;
    app.wheel_diff(20);
    let h = vec![1usize; app.visible.len()];
    app.bound_diff_scroll(&h, 10);
    let diff_scroll = app.diff_scroll;
    assert!(diff_scroll > 0);
    // Wheel the file list down too.
    app.wheel_files(8);
    app.bound_file_scroll(6);
    let file_scroll = app.file_scroll;
    assert!(file_scroll > 0);

    // A poll reloads the same unchanged content. It must request no reveal, so the next
    // frame leaves both wheel scrolls where they are (the regression snapped them to the top).
    app.reveal_diff = false;
    app.reveal_files = false;
    app.reload().unwrap();
    assert!(!app.reveal_diff, "a poll does not reveal the diff cursor");
    assert!(!app.reveal_files, "a poll does not reveal the file cursor");
    let h = vec![1usize; app.visible.len()];
    app.bound_diff_scroll(&h, 10);
    app.bound_file_scroll(6);
    assert_eq!(app.diff_scroll, diff_scroll, "the diff wheel scroll survives the poll");
    assert_eq!(app.file_scroll, file_scroll, "the file-list wheel scroll survives the poll");
}

/// The index of the first diff row with the given marker (`'+'`, `'-'`, or `' '`).
fn row_with(app: &App, marker: char) -> usize {
    app.diff.rows.iter().position(|r| r.marker() == marker).expect("a row with that marker")
}

/// The visible file-list row index for `path`.
fn file_row(app: &App, path: &str) -> usize {
    app.file_rows
        .iter()
        .position(|r| r.file_index().is_some_and(|i| app.entries[i].path == path))
        .expect("a file row for the path")
}

#[test]
fn editing_a_comment_surfaces_its_file_from_a_collapsed_directory() {
    let r = Repo::init();
    r.write("src/foo.rs", "a\nb\nc\n");
    r.write("src/bar.rs", "x\n");
    r.write("root.rs", "1\n");
    r.commit_all("init");
    r.write("src/foo.rs", "a\nB\nc\n");
    r.write("src/bar.rs", "y\n");
    r.write("root.rs", "2\n");
    let mut app = app_on(&r);

    // Open src/foo.rs and comment on its changed line.
    app.select_file(file_row(&app, "src/foo.rs")).unwrap();
    comment_on(&mut app, '+', "note on foo");
    let commented_line = app.store.get(0).unwrap().start;

    // Switch the open diff to root.rs, then collapse `src` so foo's row is hidden.
    app.select_file(file_row(&app, "root.rs")).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("root.rs"));
    app.file_cursor = app.file_rows.iter().position(|r| r.dir_path() == Some("src")).unwrap();
    app.collapse_dir();
    assert!(
        !app.file_rows
            .iter()
            .any(|r| r.file_index().is_some_and(|i| app.entries[i].path == "src/foo.rs")),
        "foo's row is hidden under the collapsed src/"
    );

    // Edit the comment from the list: the diff must switch to foo and land on its line,
    // even though foo has no visible row (the A2 bug opened the box over root.rs).
    app.open_list();
    app.start_edit();
    assert_eq!(app.diff_path.as_deref(), Some("src/foo.rs"), "edit surfaced the comment's file");
    let row = app.visible.get(app.diff_cursor).expect("cursor on a row");
    assert_eq!(row.new_no(), Some(commented_line), "cursor landed on the commented line");
    assert!(matches!(app.mode, Mode::Composing { editing: Some(_) }));
}

/// Expand the fold under the cursor with synthetic geometry (these tests don't render).
fn expand_fold(app: &mut App) {
    let heights = vec![1usize; app.visible.len()];
    app.expand_fold(&heights, 80);
}

/// Place the diff cursor on the first row with `marker` and write a comment there.
fn comment_on(app: &mut App, marker: char, text: &str) {
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(app, marker);
    app.start_comment();
    for ch in text.chars() {
        app.input_push(ch);
    }
    app.submit_comment();
}

/// An app sitting in the comment composer on the first changed line, caret at 0.
fn composing_app() -> App {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    app
}

fn typed(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.input_push(ch);
    }
}

#[test]
fn the_editor_inserts_and_deletes_at_the_caret() {
    let mut app = composing_app();
    typed(&mut app, "ac");
    assert_eq!((app.input.as_str(), app.caret), ("ac", 2));
    app.caret_left();
    app.input_push('b'); // insert mid-text, not at the end
    assert_eq!((app.input.as_str(), app.caret), ("abc", 2));
    app.input_backspace(); // deletes the char before the caret ('b')
    assert_eq!((app.input.as_str(), app.caret), ("ac", 1));
    app.input_delete_forward(); // deletes the char at the caret ('c')
    assert_eq!((app.input.as_str(), app.caret), ("a", 1));
}

#[test]
fn the_editor_moves_by_char_word_and_line() {
    let mut app = composing_app();
    typed(&mut app, "hello world");
    app.caret_home();
    assert_eq!(app.caret, 0);
    app.caret_end();
    assert_eq!(app.caret, 11);
    app.caret_word_left();
    assert_eq!(app.caret, 6, "to the start of 'world'");
    app.caret_word_left();
    assert_eq!(app.caret, 0, "to the start of 'hello'");
    app.caret_word_right();
    assert_eq!(app.caret, 5, "to the end of 'hello'");
}

#[test]
fn the_editor_kills_to_line_bounds_and_pastes_multiline() {
    let mut app = composing_app();
    typed(&mut app, "alpha beta");
    app.caret_home();
    app.caret_word_right(); // caret after "alpha"
    app.input_kill_to_end();
    assert_eq!(app.input, "alpha");
    app.input_kill_to_start();
    assert_eq!((app.input.as_str(), app.caret), ("", 0));
    // A multi-line paste lands as one unit with normalized newlines.
    app.input_paste("x\r\ny");
    assert_eq!((app.input.as_str(), app.caret), ("x\ny", 3));
}

#[test]
fn a_paste_outside_the_editor_is_ignored() {
    let r = edited_repo();
    let mut app = app_on(&r); // Normal mode, not composing
    app.input_paste("ignored");
    assert!(app.input.is_empty(), "paste does nothing outside the comment editor");
}

/// The primary (first) footer action for the current context.
fn primary(app: &App) -> FooterAction {
    app.footer_actions().first().expect("a footer action").0
}

#[test]
fn the_footer_offers_the_action_for_what_the_cursor_is_on() {
    let mut app = composing_app(); // diff focus, on a changed line, composer open
    app.cancel_comment(); // back to Normal, still on the changed line
    assert_eq!(primary(&app), FooterAction::Comment, "a diff line offers comment");

    app.toggle_select();
    assert_eq!(primary(&app), FooterAction::Comment, "a live selection still leads with comment");
    assert!(
        app.footer_actions().iter().any(|&(a, _)| a == FooterAction::ClearSelection),
        "and offers to clear the selection"
    );
    app.toggle_select();

    comment_on(&mut app, '+', "note");
    // The cursor now sits on the line it just commented.
    assert_eq!(primary(&app), FooterAction::EditComment, "a commented line offers edit");
    assert!(
        app.footer_actions().iter().any(|&(a, _)| a == FooterAction::Send),
        "a written comment surfaces send wherever the cursor is"
    );
}

#[test]
fn esc_clears_a_live_selection() {
    let mut app = composing_app();
    app.cancel_comment();
    app.toggle_select();
    assert!(app.select_anchor.is_some(), "v starts a selection");
    app.clear_selection();
    assert!(app.select_anchor.is_none(), "esc clears the selection");
}

#[test]
fn the_footer_offers_scope_everywhere_on_a_file_tab() {
    let mut app = composing_app();
    app.cancel_comment(); // diff focus, on a content line
    let has_scope = |a: &App| a.footer_actions().iter().any(|&(x, _)| x == FooterAction::Scope);
    assert!(has_scope(&app), "scope shows while reviewing a diff line");
    app.focus = Focus::Files;
    assert!(has_scope(&app), "scope shows in the file list too");
}

#[test]
fn the_pr_footer_offers_open_for_any_resolved_pr() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{Merge, PrSnapshot, PrState, PrView, Provider, Sync};

    let r = edited_repo();
    let mut app = app_on(&r);
    app.set_tab(Tab::Pr).unwrap();
    // No resolved PR (still loading): nothing to open.
    assert!(
        !app.footer_actions().iter().any(|&(a, _)| a == FooterAction::OpenPr),
        "no resolved PR → no open action"
    );

    // A resolved PR with zero comments still offers `o open` — `o` opens the PR URL, not a comment.
    app.pr = PrView::Pr(Box::new(PrSnapshot {
        provider: Provider::Github,
        number: 7,
        title: "t".into(),
        url: "u".into(),
        state: PrState::Open,
        is_draft: false,
        head_ref: "feature".into(),
        head_is_fork: false,
        base_ref: "main".into(),
        diff_refs: herdr_reviewr::forge::DiffRefs::default(),
        merge: Merge::Clean,
        sync: Sync::InSync,
        checks: vec![],
        comments: vec![],
        truncated: false,
        threads_partial: None,
    }));
    assert!(app.pr_selected_comment().is_none(), "zero comments → nothing selected");
    assert_eq!(
        app.footer_actions().first().map(|&(a, _)| a),
        Some(FooterAction::OpenPr),
        "a resolved PR offers open even with no comments"
    );
}

#[test]
fn the_footer_offers_send_only_once_a_comment_exists() {
    let mut app = composing_app();
    app.cancel_comment();
    assert!(
        !app.footer_actions().iter().any(|&(a, _)| a == FooterAction::Send),
        "no comments yet → no send action"
    );
    comment_on(&mut app, '+', "note");
    assert!(
        app.footer_actions().iter().any(|&(a, _)| a == FooterAction::Send),
        "a comment written → send appears"
    );
}

/// A repo whose `big.rs` has 40 lines with one change in the middle, so the head and
/// tail unchanged runs fold.
fn folded_repo() -> Repo {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut old = String::new();
    for i in 0..40 {
        writeln!(old, "line {i}").unwrap();
    }
    r.write("big.rs", &old);
    r.commit_all("init");
    r.write("big.rs", &old.replace("line 20", "LINE 20"));
    r
}

#[test]
fn a_fold_expands_permanently_and_keeps_the_cursor_in_range() {
    let r = folded_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    let folded = app.visible.len();
    assert!(app.visible.iter().any(|row| row.hidden() > 0), "opens folded");

    // Land on the leading fold and expand it (the `→` action) — the visible count grows.
    app.diff_cursor = app.visible.iter().position(|row| row.hidden() > 0).unwrap();
    assert!(app.on_fold(), "`→` expands here");
    expand_fold(&mut app);
    let expanded = app.visible.len();
    assert!(expanded > folded, "expanding reveals the hidden lines");
    assert!(app.diff_cursor < app.visible.len(), "cursor stays in range");
    assert!(!app.on_fold(), "the fold is gone, so `→` now scrolls instead");

    // Expansion is permanent — pressing again on a revealed content line does nothing.
    expand_fold(&mut app);
    assert_eq!(app.visible.len(), expanded, "no collapse-back");
}

#[test]
fn a_selection_cannot_cross_a_fold() {
    let r = folded_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;

    // Anchor just above the trailing fold, then try to select well past it.
    let tail = app.visible.iter().rposition(|row| row.hidden() > 0).unwrap();
    app.diff_cursor = tail - 1;
    app.toggle_select();
    app.move_cursor(10).unwrap();
    assert_eq!(app.diff_cursor, tail - 1, "the cursor stops shy of the trailing fold");
    let (lo, hi) = app.selection_range();
    assert!((lo..=hi).all(|i| app.visible[i].is_content()), "no fold row is in the selection");

    // The same upward, across the leading fold.
    let head = app.visible.iter().position(|row| row.hidden() > 0).unwrap();
    app.select_anchor = None;
    app.diff_cursor = head + 1;
    app.toggle_select();
    app.move_cursor(-10).unwrap();
    assert_eq!(app.diff_cursor, head + 1, "the cursor stops just after the leading fold");
}

#[test]
fn paging_the_diff_cannot_cross_a_fold() {
    let r = folded_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    let tail = app.visible.iter().rposition(|row| row.hidden() > 0).unwrap();
    app.diff_cursor = tail - 1;
    app.toggle_select();
    app.move_cursor(50).unwrap(); // a big page that would jump well past the trailing fold
    assert_eq!(app.diff_cursor, tail - 1, "page stops shy of the fold while selecting");
}

#[test]
fn expanding_a_fold_does_not_bleed_into_another_file() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut body = String::new();
    for i in 0..40 {
        let _ = writeln!(body, "line {i}");
    }
    r.write("a.rs", &body);
    r.write("b.rs", &body);
    r.commit_all("init");
    r.write("a.rs", &body.replace("line 20", "A20"));
    r.write("b.rs", &body.replace("line 20", "B20"));
    let mut app = app_on(&r); // a.rs opens first (sorted)

    // Expand a.rs's leading fold (its anchor is line 1, same as b.rs's leading fold).
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|row| row.hidden() > 0).unwrap();
    expand_fold(&mut app);
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"));

    // Switching to b.rs must not carry a.rs's expansion across (shared line-number key).
    app.focus = Focus::Files;
    app.move_cursor(1).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("b.rs"));
    assert!(app.visible[0].hidden() > 0, "b.rs's leading fold stays collapsed");
}

#[test]
fn expanding_a_fold_keeps_the_viewport_still() {
    let r = folded_repo();

    // A fold in the top half grows upward: scroll advances so the lines below hold position.
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    let head = app.visible.iter().position(|row| row.hidden() > 0).unwrap();
    let shift = app.visible[head].hidden() - 1;
    app.diff_cursor = head;
    app.diff_scroll = 0;
    let heights = vec![1usize; app.visible.len()];
    app.expand_fold(&heights, 20);
    assert_eq!(app.diff_scroll, shift, "top-half fold grows upward");

    // A fold in the bottom half grows downward: scroll holds so the lines above stay put.
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    let tail = app.visible.iter().rposition(|row| row.hidden() > 0).unwrap();
    app.diff_cursor = tail;
    app.diff_scroll = 0;
    let heights = vec![1usize; app.visible.len()];
    app.expand_fold(&heights, tail + 2); // the fold sits in the bottom half of the viewport
    assert_eq!(app.diff_scroll, 0, "bottom-half fold grows downward");
}

#[test]
fn a_comment_through_a_fold_anchors_to_gits_line_and_survives_a_poll() {
    let r = folded_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;

    // Comment on the changed line (new-side 21) while the rest is folded.
    app.diff_cursor = app.visible.iter().position(|row| row.text().contains("LINE 20")).unwrap();
    app.start_comment();
    for ch in "here".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    let c = app.store.iter().next().unwrap();
    assert_eq!((c.side, c.start), (Side::New, 21));

    // A fold expand plus a poll keeps the comment.
    app.diff_cursor = app.visible.iter().position(|row| row.hidden() > 0).unwrap();
    expand_fold(&mut app);
    app.reload().unwrap();
    assert_eq!(app.store.len(), 1, "the comment survives a fold expand and a poll");
    assert!(app.commented_lines().iter().any(|&i| app.visible[i].text().contains("LINE 20")));
}

#[test]
fn comment_anchors_to_gits_real_line_numbers() {
    // `edited_repo`: a.rs has beta→BETA on line 2 and epsilon appended as new line 5.
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;

    app.diff_cursor = app.diff.rows.iter().position(|r| r.text().contains("epsilon")).unwrap();
    app.start_comment();
    for ch in "appended".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    app.diff_cursor =
        app.diff.rows.iter().position(|r| r.marker() == '-' && r.text().contains("beta")).unwrap();
    app.start_comment();
    for ch in "removed".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    let appended = app.store.iter().find(|c| c.text == "appended").unwrap();
    assert_eq!((appended.side, appended.start, appended.end), (Side::New, 5, 5));
    let removed = app.store.iter().find(|c| c.text == "removed").unwrap();
    assert_eq!((removed.side, removed.start, removed.end), (Side::Old, 2, 2));
}

#[test]
fn comments_on_added_and_removed_lines_capture_the_snippet() {
    let r = edited_repo();
    let mut app = app_on(&r);
    assert_eq!(app.entries.len(), 1);

    comment_on(&mut app, '+', "this addition needs a test");
    comment_on(&mut app, '-', "why was this dropped?");
    assert_eq!(app.store.len(), 2);

    let removed = app
        .store
        .iter()
        .find(|c| c.location().ends_with("(removed)"))
        .expect("a removed-side comment");
    assert!(removed.lines.starts_with('-'), "snippet keeps the diff marker: {:?}", removed.lines);

    let added = app
        .store
        .iter()
        .find(|c| !c.location().ends_with("(removed)"))
        .expect("a new-side comment");
    assert!(added.lines.starts_with('+'));
}

#[test]
fn a_saved_comment_survives_a_refresh() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "keep me");
    assert_eq!(app.store.len(), 1);

    r.write("b.rs", "another change\n"); // the world moves on
    app.reload().unwrap();

    assert_eq!(app.store.len(), 1, "refresh must not drop a saved comment");
    assert_eq!(app.store.iter().next().unwrap().text, "keep me");
    assert!(app.entries.iter().any(|f| f.path == "b.rs"), "file list still refreshed");
}

#[test]
fn file_tree_badges_roll_comments_up_and_mark_changed_all_files_folders() {
    use herdr_reviewr::app::Tab;

    let r = Repo::init();
    r.write("src/a.rs", "one\n");
    r.write("src/b.rs", "one\n");
    r.write("src2/a.rs", "one\n");
    r.write("src2/b.rs", "one\n");
    r.commit_all("init");
    r.write("src/a.rs", "two\n");
    r.write("src/b.rs", "two\n");
    r.write("src2/a.rs", "two\n");
    r.write("src2/b.rs", "two\n");
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "check this");

    let file = app
        .file_rows
        .iter()
        .find(|row| {
            row.file_index()
                .and_then(|index| app.entries.get(index))
                .is_some_and(|entry| entry.path == "src/a.rs")
        })
        .expect("commented file row");
    assert!(app.tree_badges(file).commented, "the file carries its comment indicator");
    let directory =
        app.file_rows.iter().find(|row| row.dir_path() == Some("src")).expect("src directory row");
    assert!(app.tree_badges(directory).commented, "feedback rolls up to the parent folder");
    assert!(!app.tree_badges(directory).changed, "Changes folders need no redundant marker");
    let sibling = app
        .file_rows
        .iter()
        .find(|row| row.dir_path() == Some("src2"))
        .expect("src2 directory row");
    assert!(!app.tree_badges(sibling).commented, "path prefixes do not leak into siblings");

    app.set_tab(Tab::AllFiles).unwrap();
    let directory = app
        .file_rows
        .iter()
        .find(|row| row.dir_path() == Some("src"))
        .expect("collapsed All-files directory row");
    let badges = app.tree_badges(directory);
    assert!(badges.commented, "collapsed folders retain descendant feedback");
    assert!(badges.changed, "All-files folders expose descendant scope changes");
}

#[test]
fn a_refresh_while_composing_freezes_input_and_diff() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    for ch in "half-written thought".chars() {
        app.input_push(ch);
    }
    let frozen_diff = app.diff.clone();

    r.write("a.rs", "alpha\nBETA\ngamma\ndelta\nepsilon\nzeta\n"); // diff shifts under us
    r.write("c.rs", "c\n");
    app.reload().unwrap();

    assert!(app.composing(), "still composing");
    assert_eq!(app.input, "half-written thought", "input untouched");
    assert_eq!(app.diff, frozen_diff, "the open diff is frozen while composing");
    assert!(app.entries.iter().any(|f| f.path == "c.rs"), "file list still refreshes");
}

#[test]
fn a_failed_export_keeps_comments_and_success_consumes_them() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "one");
    comment_on(&mut app, '-', "two");
    assert_eq!(app.store.len(), 2);

    app.export(&FakeTarget::failing());
    assert_eq!(app.store.len(), 2, "a failed export leaves every comment in place");

    let target = FakeTarget::ok();
    app.export(&target);
    assert!(app.store.is_empty(), "a successful export consumes the comments");

    // The sent text is the real export block format, end to end through App::export.
    let sent = target.last();
    assert!(sent.contains("one") && sent.contains("two"), "both comment texts present: {sent:?}");
    assert!(sent.lines().next().is_some_and(|l| l.starts_with("a.rs:")), "leads with a location");
    assert!(sent.contains("\n\n"), "blocks separated by a blank line: {sent:?}");
    assert!(
        sent.lines().any(|l| l.starts_with('+') || l.starts_with('-')),
        "each block carries its diff snippet: {sent:?}"
    );
}

#[test]
fn send_consumes_the_whole_set() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "first");
    comment_on(&mut app, '-', "second");

    app.export(&FakeTarget::ok());
    assert!(app.store.is_empty(), "send takes every comment, not just one");
}

#[test]
fn a_comment_of_only_blank_lines_is_cancelled() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    app.input_push(' ');
    app.input_push('\n');
    app.input_push('\n');
    app.submit_comment();

    assert!(app.store.is_empty(), "a whitespace-only comment is not saved");
    assert!(!app.composing(), "compose mode exits");
}

#[test]
fn the_composer_reserve_keeps_the_anchored_line_visible() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut original = String::new();
    for i in 0..60 {
        writeln!(original, "line {i}").unwrap();
    }
    r.write("big.rs", &original);
    r.commit_all("init");
    r.write("big.rs", &original.replace("line", "LINE"));

    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = 30;
    app.start_comment();
    for ch in "one\ntwo\nthree".chars() {
        app.input_push(ch);
    }

    // Mirror the event loop: reserve the box's rows, then clamp. The anchored line must
    // stay within the narrowed viewport so it renders above the box.
    let viewport = 12;
    let effective = viewport - herdr_reviewr::ui::composer_height(&app, 80);
    clamp(&mut app, effective);
    assert!(
        (app.diff_scroll..app.diff_scroll + effective).contains(&app.diff_cursor),
        "anchored line {} stays in the reserved viewport [{}, {})",
        app.diff_cursor,
        app.diff_scroll,
        app.diff_scroll + effective
    );
}

#[test]
fn a_comment_can_be_written_across_multiple_lines() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    for ch in "first line".chars() {
        app.input_push(ch);
    }
    app.input_push('\n'); // Alt/Shift+Enter inserts a newline
    for ch in "second line".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    let c = app.store.iter().next().unwrap();
    assert_eq!(c.text, "first line\nsecond line", "the body keeps its line break");

    let target = FakeTarget::ok();
    app.export(&target);
    let sent = target.last();
    assert!(sent.contains("first line\nsecond line"), "export preserves the break: {sent:?}");
    assert!(!sent.contains("\n\n\n"), "no blank-line run that could split a block");
}

#[test]
fn the_cursor_stays_on_a_folder_across_a_poll_and_toggle() {
    let r = Repo::init();
    r.write("src/a.rs", "x\n");
    r.write("src/b.rs", "y\n");
    r.write("root.rs", "z\n");
    r.commit_all("init");
    r.write("src/a.rs", "x2\n");
    r.write("src/b.rs", "y2\n"); // two changed files keep `src/` an expandable directory
    r.write("root.rs", "z2\n");
    let mut app = app_on(&r);
    app.focus = Focus::Files;

    // Land the cursor on the `src` directory row; the open diff is some file.
    let dir_row = app.file_rows.iter().position(|r| r.dir_path() == Some("src")).unwrap();
    app.file_cursor = dir_row;
    let open = app.diff_path.clone();
    assert!(open.is_some(), "a file diff is open");

    // A poll must not yank the cursor onto the open file, nor blank the diff.
    app.reload().unwrap();
    assert_eq!(app.file_cursor, dir_row, "cursor stays on the folder across a poll");
    assert_eq!(app.diff_path, open, "the open diff is unchanged");

    // Collapsing then a poll keeps the cursor on the (now collapsed) folder.
    app.collapse_dir();
    app.reload().unwrap();
    let dir_row = app.file_rows.iter().position(|r| r.dir_path() == Some("src")).unwrap();
    assert_eq!(app.file_cursor, dir_row, "cursor stays on the folder after collapse + poll");
    assert_eq!(app.diff_path, open, "the open diff is still unchanged");
}

#[test]
fn arrows_collapse_and_expand_a_folder() {
    let r = Repo::init();
    r.write("src/a.rs", "x\n");
    r.write("src/b.rs", "y\n");
    r.commit_all("init");
    r.write("src/a.rs", "x2\n");
    r.write("src/b.rs", "y2\n");
    let mut app = app_on(&r);
    app.focus = Focus::Files;

    let dir_row = app.file_rows.iter().position(|r| r.dir_path() == Some("src")).unwrap();
    app.file_cursor = dir_row;
    assert!(app.on_folder(), "the cursor is on the folder");
    let expanded = app.file_rows.len();

    app.collapse_dir(); // ←
    assert!(app.file_rows.len() < expanded, "collapsing hides the children");
    assert!(app.on_folder(), "the cursor stays on the folder row");

    app.expand_dir(); // →
    assert_eq!(app.file_rows.len(), expanded, "expanding shows them again");
}

#[test]
fn the_pane_divider_resizes_and_clamps() {
    let r = edited_repo();
    let mut app = app_on(&r);
    let start = app.list_pct;
    app.resize_list(4);
    assert_eq!(app.list_pct, start + 4, "[ / ] step the divider");
    // Clamps: never collapses either pane however far it is pushed.
    for _ in 0..50 {
        app.resize_list(4);
    }
    assert!(app.list_pct <= 60, "the file list never swallows the diff");
    for _ in 0..50 {
        app.resize_list(-4);
    }
    assert!(app.list_pct >= 15, "the diff never swallows the file list");

    // A drag sets the width from the divider's body column.
    app.drag_divider(100, 70); // list spans columns 70..100 → 30%
    assert_eq!(app.list_pct, 30);
}

#[test]
fn ctrl_w_deletes_the_previous_word_in_a_comment() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    for ch in "needs a closer look".chars() {
        app.input_push(ch);
    }
    app.input_delete_word(); // drops "look"
    assert_eq!(app.input, "needs a closer ");
    app.input_delete_word(); // drops the space then "closer"
    assert_eq!(app.input, "needs a ");
}

#[test]
fn the_comment_box_grows_as_a_long_line_wraps() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    // A single long line with no explicit newline must still report more than one row.
    let width = 30; // narrow diff pane
    let one_word = herdr_reviewr::ui::composer_height(&app, width);
    for ch in "the quick brown fox jumps over the lazy dog again and again".chars() {
        app.input_push(ch);
    }
    let wrapped = herdr_reviewr::ui::composer_height(&app, width);
    assert!(wrapped > one_word, "box grew from {one_word} to {wrapped} rows as text wrapped");
}

#[test]
fn a_comment_can_be_edited_then_deleted() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "original");
    let snippet_before = app.store.get(0).unwrap().lines.clone();

    app.open_list();
    app.start_edit();
    app.input.clear();
    for ch in "rewritten".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    assert_eq!(app.store.get(0).unwrap().text, "rewritten");
    assert_eq!(app.store.get(0).unwrap().lines, snippet_before, "edit changes only the text");

    app.open_list();
    app.delete_comment();
    assert!(app.store.is_empty());
}

#[test]
fn deleting_the_last_comment_closes_the_list_overlay() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "only one");
    app.open_list();
    assert_eq!(app.mode, Mode::List);
    app.delete_comment();
    assert!(app.store.is_empty());
    assert_eq!(app.mode, Mode::Normal, "an emptied overlay closes instead of stranding the user");
}

#[test]
fn finishing_an_edit_returns_to_its_origin() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "first");
    comment_on(&mut app, ' ', "second");

    // Edit from the comments-list overlay → returns to the list.
    app.open_list();
    app.start_edit();
    app.input_push('!');
    app.submit_comment();
    assert_eq!(app.mode, Mode::List, "a list-initiated edit returns to the list");

    // Edit from the diff → returns to Normal.
    app.close_list();
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_edit();
    app.submit_comment();
    assert_eq!(app.mode, Mode::Normal, "a diff-initiated edit returns to Normal");
}

#[test]
fn editing_from_the_list_navigates_to_the_comments_file() {
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.write("b.rs", "one\ntwo\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\n");
    r.write("b.rs", "one\nTWO\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    // Comment on b.rs, then move the view to a.rs.
    let bi = app.entries.iter().position(|f| f.path == "b.rs").unwrap();
    app.select_file(bi).unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    for ch in "fix this".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    let ai = app.entries.iter().position(|f| f.path == "a.rs").unwrap();
    app.select_file(ai).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"));

    // Editing the comment from the list pulls the view back to its file and lands the
    // cursor on a real diff line there (so the inline box opens over the comment).
    app.open_list();
    app.start_edit();
    assert!(app.composing());
    assert_eq!(app.diff_path.as_deref(), Some("b.rs"), "edit switched to the comment's file");
    let dl = &app.diff.rows[app.diff_cursor];
    assert!(dl.new_no().is_some() || dl.old_no().is_some(), "cursor sits on a real diff line");
}

#[test]
fn a_comment_on_a_reverted_file_is_flagged_stale() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "note");

    r.write("a.rs", "alpha\nbeta\ngamma\ndelta\n"); // back to committed state
    app.reload().unwrap();

    assert!(app.entries.iter().all(|f| f.path != "a.rs"), "file left the changeset");
    assert_eq!(app.store.len(), 1, "the comment still exists");
    let c = app.store.get(0).unwrap();
    assert!(app.is_stale(c), "a diff comment whose file left the changeset is stale");
}

#[test]
fn switching_scope_swaps_the_changeset() {
    let r = Repo::init();
    r.write("base.rs", "b\n");
    r.commit_all("base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("committed.rs", "c\n");
    r.commit_all("feature work");
    r.write("dirty.rs", "d\n"); // uncommitted, untracked

    let mut app = App::new(r.path_buf(), Scope::Uncommitted, Some("main".to_string()));
    app.reload().unwrap();
    assert!(app.entries.iter().any(|f| f.path == "dirty.rs"));
    assert!(app.entries.iter().all(|f| f.path != "committed.rs"), "uncommitted omits commits");

    // Branch is a superset of uncommitted: it adds the committed work, keeps the dirty file.
    app.set_scope(Scope::Branch).unwrap();
    assert!(app.entries.iter().any(|f| f.path == "committed.rs"), "branch adds committed work");
    assert!(app.entries.iter().any(|f| f.path == "dirty.rs"), "branch keeps the working tree");
}

#[test]
fn a_multi_line_range_comment_spans_lines_and_keeps_the_whole_snippet() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;

    // Anchor on the first changed line, then extend the selection down two rows.
    let first = row_with(&app, '-');
    app.diff_cursor = first;
    app.toggle_select();
    app.move_cursor(1).unwrap();
    app.move_cursor(1).unwrap();
    let (lo, hi) = app.selection_range();
    assert!(hi > lo, "selection spans more than one line");

    app.start_comment();
    for ch in "this whole hunk is suspicious".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    assert_eq!(app.store.len(), 1);
    let c = app.store.iter().next().unwrap();
    assert!(c.end > c.start, "comment covers a line range: {}..{}", c.start, c.end);
    let snippet: Vec<&str> = c.lines.lines().collect();
    assert!(snippet.len() >= 2, "snippet keeps every selected line: {:?}", c.lines);
    assert!(
        snippet.iter().all(|l| l.starts_with(['+', '-', ' '])),
        "every snippet line keeps its diff marker: {:?}",
        c.lines
    );
}

#[test]
fn scope_cannot_change_while_composing() {
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    app.input_push('x');

    app.set_scope(Scope::Branch).unwrap();
    assert_eq!(app.scope, Scope::Uncommitted, "scope is frozen mid-comment");
    assert!(app.composing(), "still composing");
    assert_eq!(app.input, "x", "input untouched");
}

#[test]
fn tab_cannot_change_while_composing() {
    use herdr_reviewr::app::Tab;
    let r = edited_repo();
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    app.input_push('x');

    // A tab switch mid-comment must be a no-op, so the panes never swap out from under the
    // open composer (the compose-freeze invariant), matching set_scope.
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.tab, Tab::Changes, "the tab is frozen mid-comment");
    assert!(app.composing(), "still composing");
    assert_eq!(app.input, "x", "input untouched");
}

#[test]
fn the_app_reads_branch_scoped_diffs_not_working_tree() {
    let r = Repo::init();
    r.write("shared.rs", "base\n");
    r.commit_all("base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("on_branch.rs", "committed on the feature branch\n");
    r.commit_all("feature work");

    let mut app = App::new(r.path_buf(), Scope::Branch, Some("main".to_string()));
    app.reload().unwrap();

    let idx =
        app.entries.iter().position(|f| f.path == "on_branch.rs").expect("branch file listed");
    app.select_file(idx).unwrap();

    // The diff the App loaded for branch scope is base...HEAD, so it shows the
    // committed branch content — which the uncommitted (working-tree) scope cannot.
    let on_branch = app
        .diff
        .rows
        .iter()
        .any(|r| r.marker() == '+' && r.text().contains("committed on the feature branch"));
    assert!(on_branch, "branch diff carries the committed line");
}

#[test]
fn the_diff_scroll_is_sticky_and_only_follows_the_cursor_off_screen() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut original = String::new();
    for i in 0..60 {
        writeln!(original, "line {i}").unwrap();
    }
    r.write("big.rs", &original);
    r.commit_all("init");
    let edited = original.replace("line", "LINE");
    r.write("big.rs", &edited);

    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    let height = 10;

    clamp(&mut app, height);
    assert_eq!(app.diff_scroll, 0);

    // Cursor moves but stays in view — the window does not scroll.
    app.diff_cursor = 5;
    clamp(&mut app, height);
    assert_eq!(app.diff_scroll, 0, "no scroll while the cursor is visible");

    // Cursor leaves the bottom — scroll just enough to reveal it, no recentering.
    app.diff_cursor = 12;
    clamp(&mut app, height);
    assert_eq!(app.diff_scroll, 12 + 1 - height);

    // Cursor jumps back above the window — scroll follows up to it.
    app.diff_cursor = 1;
    clamp(&mut app, height);
    assert_eq!(app.diff_scroll, 1);

    // A viewport taller than the whole diff never scrolls.
    app.diff_cursor = 0;
    let tall = app.visible.len() + 50;
    clamp(&mut app, tall);
    assert_eq!(app.diff_scroll, 0, "no scroll when the diff fits the viewport");
}

#[test]
fn a_refresh_keeps_the_diff_scroll_position() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut original = String::new();
    for i in 0..60 {
        writeln!(original, "line {i}").unwrap();
    }
    r.write("big.rs", &original);
    r.commit_all("init");
    r.write("big.rs", &original.replace("line", "LINE"));

    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = 25;
    clamp(&mut app, 10);
    let (cursor, scroll) = (app.diff_cursor, app.diff_scroll);
    assert!(scroll > 0, "we scrolled down into the diff");

    // A poll refresh of the same, still-changed file must not snap back to the top.
    app.reload().unwrap();
    assert_eq!(app.diff_cursor, cursor, "refresh keeps the cursor line");
    assert_eq!(app.diff_scroll, scroll, "refresh keeps the scroll position");
}

#[test]
fn the_diff_title_stays_on_the_composed_file_through_a_refresh() {
    let r = edited_repo(); // a.rs is the only changed file
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"));

    app.start_comment();
    app.input_push('x');

    // While composing, a.rs leaves the changeset and another file appears.
    r.write("a.rs", "alpha\nbeta\ngamma\ndelta\n");
    r.write("z.rs", "new\n");
    app.reload().unwrap();

    // The frozen diff — and its title — stay on the file being commented, even though
    // the file cursor now points elsewhere. (No title/body mismatch.)
    assert!(app.composing());
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"), "diff title frozen on composed file");
    assert_ne!(app.current_entry().map(|f| f.path.as_str()), Some("a.rs"));
}

#[test]
fn a_comment_submitted_after_its_file_left_the_changeset_anchors_to_that_file() {
    let r = edited_repo(); // a.rs is the only changed file
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+');
    app.start_comment();
    for ch in "note for a.rs".chars() {
        app.input_push(ch);
    }

    // a.rs leaves the changeset and another file appears, drifting the file cursor.
    r.write("a.rs", "alpha\nbeta\ngamma\ndelta\n");
    r.write("z.rs", "new\n");
    app.reload().unwrap();
    assert_ne!(app.current_entry().map(|f| f.path.as_str()), Some("a.rs"));

    app.submit_comment();
    let c = app.store.iter().next().unwrap();
    assert_eq!(c.file, "a.rs", "comment anchors to its diff's file, not the drifted cursor");
}

#[test]
fn deleting_the_last_listed_comment_clamps_the_list_cursor() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "one");
    comment_on(&mut app, '-', "two");

    app.open_list();
    app.list_move(1); // cursor on the last comment (index 1)
    assert_eq!(app.list_cursor, 1);

    app.delete_comment(); // removes index 1
    assert_eq!(app.store.len(), 1);
    assert_eq!(app.list_cursor, 0, "list cursor clamps back into range");
}

#[test]
fn a_non_repo_path_yields_an_empty_state_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path().to_path_buf(), Scope::Uncommitted, None);
    assert!(app.reload().is_ok(), "a non-repo reload is graceful, not an error");
    assert!(app.entries.is_empty());
    assert!(app.diff.rows.is_empty());
}

#[test]
fn jump_moves_the_cursor_onto_a_commented_line() {
    let r = edited_repo();
    let mut app = app_on(&r);
    comment_on(&mut app, '+', "note");

    app.focus = Focus::Diff;
    app.diff_cursor = 0;
    app.jump_comment(1);
    assert!(app.commented_lines().contains(&app.diff_cursor), "cursor landed on a comment");
}

// --- last-turn scope -----------------------------------------------------------

#[test]
fn last_turn_is_empty_until_a_turn_is_observed() {
    let r = Repo::init();
    r.write("a.rs", "a\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    app.reload().unwrap();
    assert!(app.awaiting_turn(), "no baseline captured yet");
    assert!(app.entries.is_empty(), "the scope is empty before a turn");
}

#[test]
fn last_turn_shows_a_change_producing_turn() {
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    app.apply_agent_status(Some("idle"));
    app.apply_agent_status(Some("working")); // turn start: candidate = "one"
    r.write("a.rs", "one\ntwo\n");
    app.apply_agent_status(Some("working")); // first change promotes the baseline
    app.reload().unwrap();
    assert!(!app.awaiting_turn(), "the baseline is now set");
    assert!(app.entries.iter().any(|f| f.path == "a.rs"), "the turn's edit shows");
}

#[test]
fn a_question_only_turn_keeps_the_previous_turns_diff() {
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    // Turn A edits a file.
    app.apply_agent_status(Some("idle"));
    app.apply_agent_status(Some("working"));
    r.write("a.rs", "one\ntwo\n");
    app.apply_agent_status(Some("working"));
    // Turn B is a question — no file change.
    app.apply_agent_status(Some("idle"));
    app.apply_agent_status(Some("working"));
    app.apply_agent_status(Some("idle"));
    app.reload().unwrap();
    assert!(
        app.entries.iter().any(|f| f.path == "a.rs"),
        "A's diff persists across a question-only turn"
    );
}

#[test]
fn a_permission_pause_stays_one_turn() {
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    app.apply_agent_status(Some("idle"));
    app.apply_agent_status(Some("working")); // turn start: candidate = "one"
    r.write("a.rs", "one\nbefore\n"); // edit before the prompt
    app.apply_agent_status(Some("blocked")); // permission prompt promotes baseline = "one"
    app.apply_agent_status(Some("working")); // resume — must NOT re-baseline
    r.write("a.rs", "one\nbefore\nafter\n"); // edit after the prompt
    app.apply_agent_status(Some("working"));
    app.reload().unwrap();
    let a = app.entries.iter().find(|f| f.path == "a.rs").expect("a.rs changed");
    let annotation = a.annotation.as_ref().expect("a changed file is annotated");
    assert_eq!(annotation.additions, 2, "both the pre- and post-prompt edits belong to one turn");
}

#[test]
fn the_baseline_survives_a_restart() {
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    {
        let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
        app.apply_agent_status(Some("idle"));
        app.apply_agent_status(Some("working"));
        r.write("a.rs", "one\ntwo\n");
        app.apply_agent_status(Some("working")); // promotes and persists the ref
    }
    // A fresh App — a sidebar restart — resumes the persisted baseline.
    let mut restarted = App::new(r.path_buf(), Scope::LastTurn, None);
    restarted.reload().unwrap();
    assert!(!restarted.awaiting_turn(), "baseline resumed from the private ref");
    assert!(restarted.entries.iter().any(|f| f.path == "a.rs"), "the turn's edit still shows");
}

#[test]
fn no_agent_status_pauses_tracking() {
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    app.apply_agent_status(None); // no herdr / no resolvable agent
    r.write("a.rs", "one\ntwo\n");
    app.apply_agent_status(None);
    app.reload().unwrap();
    assert!(app.awaiting_turn(), "without a status signal the baseline never forms");
}

/// The visible-row index of the file at `path`, or `None` when it is hidden/absent.
fn file_row_of(app: &App, path: &str) -> Option<usize> {
    app.file_rows
        .iter()
        .position(|row| row.file_index().is_some_and(|i| app.entries[i].path == path))
}

#[test]
fn all_files_tab_browses_the_whole_worktree_and_renders_content() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::diff::{LineDecoration, View};
    let r = Repo::init();
    r.write("src/app.rs", "fn main() {}\n");
    r.write("src/ui.rs", "fn render() {}\n");
    r.write("README.md", "# hi\n");
    r.commit_all("init");
    r.write("README.md", "# changed\n"); // change a top-level file (no dir to reveal)
    let mut app = app_on(&r);

    // Changes lists only the changed file and opens its diff.
    assert_eq!(app.tab, Tab::Changes);
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.diff_path.as_deref(), Some("README.md"));

    // All files lists the whole worktree and opens its first file (README, the top-level one),
    // so src/ stays collapsed by default.
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.tab, Tab::AllFiles);
    assert!(app.entries.iter().any(|e| e.path == "src/ui.rs"), "an unchanged file is listed");
    assert_eq!(app.diff_path.as_deref(), Some("README.md"), "All files opens its first file");
    assert!(
        app.diff.rows.iter().any(|row| app.line_decoration(row) == Some(LineDecoration::Modified)),
        "the changed line carries an editor-style gutter decoration"
    );
    assert!(app.file_rows.iter().any(|row| row.dir_path() == Some("src")), "src/ is a dir row");
    assert!(file_row_of(&app, "src/ui.rs").is_none(), "a collapsed dir hides its children");

    // Expanding src/ (a click on the directory) then opening a file shows its full content.
    let src_row = app.file_rows.iter().position(|row| row.dir_path() == Some("src")).unwrap();
    app.select_file(src_row).unwrap();
    let ui_row = file_row_of(&app, "src/ui.rs").expect("src/ui.rs visible once src/ is expanded");
    app.select_file(ui_row).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("src/ui.rs"));
    assert_eq!(app.diff.view, View::File);
    assert!(app.diff.rows.iter().any(|row| row.text().contains("fn render")));
}

#[test]
fn file_tabs_project_one_logical_location_and_mark_unrepresentable_files() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::diff::View;
    let r = Repo::init();
    r.write("src/app.rs", "fn main() {}\n");
    r.write("README.md", "# hi\n");
    r.commit_all("init");
    r.write("src/app.rs", "fn main() { run() }\n");
    let mut app = app_on(&r);
    let changes_open = app.diff_path.clone();
    assert_eq!(changes_open.as_deref(), Some("src/app.rs"));

    // `2` — the same logical location, projected into the file view.
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("src/app.rs"), "one location, two projections");
    assert_eq!(app.diff.view, View::File);

    // Open README.md on All files, then `1`: Changes cannot represent an unchanged file —
    // it keeps its own open diff and says so instead of pretending.
    let readme_row = file_row_of(&app, "README.md").expect("README.md at the top level");
    app.select_file(readme_row).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("README.md"));
    app.set_tab(Tab::Changes).unwrap();
    assert_eq!(app.diff_path, changes_open, "the unrepresentable file does not retarget");
    assert_eq!(app.diff.view, View::Diff);
    assert!(
        app.status.contains("no changes in this scope"),
        "the unavailable location is explicit: {}",
        app.status
    );

    // Forward again: the logical location (the Changes diff) projects into All files.
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("src/app.rs"));
    assert_eq!(app.diff.view, View::File);
}

#[test]
fn changed_count_and_staleness_stay_scope_based_on_all_files() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::model::Comment;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.write("b.rs", "two\n");
    r.commit_all("init");
    r.write("a.rs", "ONE\n"); // exactly one changed file
    let mut app = app_on(&r);
    assert_eq!(app.changed_count(), 1, "Changes counts the one changed file");

    // A diff comment on b.rs, which is in the worktree but not in the changeset.
    let comment = Comment {
        file: "b.rs".into(),
        side: Side::New,
        start: 1,
        end: 1,
        lines: " two".into(),
        text: "?".into(),
        diff_anchored: true,
    };
    app.store.add(comment.clone());

    app.set_tab(Tab::AllFiles).unwrap();
    assert!(app.entries.len() >= 2, "All files lists the whole worktree");
    assert_eq!(app.changed_count(), 1, "the count is the changeset, not the worktree total");
    assert!(
        app.is_stale(&comment),
        "a diff comment keys on the changeset even while All files lists b.rs"
    );
}

/// The annotation on the `All files` row for `path`: `Some(Some(_))` annotated, `Some(None)`
/// listed-but-unchanged, `None` not visible.
#[allow(clippy::option_option)] // outer = row found, inner = its annotation
fn annotation_of(app: &App, path: &str) -> Option<Option<herdr_reviewr::file_list::Annotation>> {
    use herdr_reviewr::file_list::RowKind;
    app.file_rows.iter().find_map(|row| match &row.kind {
        RowKind::File { index, annotation } if app.entries[*index].path == path => {
            Some(annotation.clone())
        }
        _ => None,
    })
}

#[test]
fn all_files_annotates_changed_files_only() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::model::ChangeKind;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.write("b.rs", "two\n");
    r.commit_all("init");
    r.write("a.rs", "ONE\n"); // a.rs changed, b.rs unchanged
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    assert!(
        matches!(annotation_of(&app, "a.rs"), Some(Some(a)) if a.change == ChangeKind::Modified),
        "a changed file carries its marker"
    );
    assert_eq!(
        annotation_of(&app, "b.rs"),
        Some(None),
        "an unchanged file is listed without a marker"
    );
}

#[test]
fn switching_scope_on_all_files_remarks_in_place() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.write("b.rs", "two\n");
    r.commit_all("init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("b.rs", "TWO\n");
    r.commit_all("committed change to b"); // committed on the branch
    r.write("a.rs", "ONE\n"); // one uncommitted change
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    app.focus = Focus::Files;
    app.move_cursor(1).unwrap();
    let cursor = app.file_cursor;
    assert_eq!(app.changed_count(), 1, "uncommitted marks only the dirty file");
    assert!(
        matches!(annotation_of(&app, "a.rs"), Some(Some(_))),
        "a.rs is marked under uncommitted"
    );
    assert_eq!(annotation_of(&app, "b.rs"), Some(None), "b.rs is unmarked under uncommitted");

    // Branch is a superset: it adds the committed b.rs and keeps a.rs — re-marked in place.
    app.set_scope(Scope::Branch).unwrap();
    assert_eq!(app.file_cursor, cursor, "the cursor holds across a scope re-mark");
    assert_eq!(app.changed_count(), 2, "branch marks both the committed and the dirty file");
    assert!(matches!(annotation_of(&app, "a.rs"), Some(Some(_))), "a.rs stays marked");
    assert!(matches!(annotation_of(&app, "b.rs"), Some(Some(_))), "b.rs is now marked");
}

#[test]
fn all_files_lazily_loads_an_expanded_ignored_directory() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("src/app.rs", "fn main() {}\n");
    r.commit_all("init");
    r.write(".gitignore", "target/\n");
    r.write("target/build.o", "x\n");
    r.write("target/sub/y.o", "y\n");
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    app.focus = Focus::Files;

    // target/ is a collapsed, ignored placeholder; its contents are not loaded yet.
    assert!(app.entries.iter().any(|e| e.path == "target" && e.is_dir && e.ignored));
    assert!(!app.entries.iter().any(|e| e.path.starts_with("target/")), "children not loaded yet");

    // Expand it → immediate children load (one level only), still ignored/dimmed.
    let row = |a: &App| a.file_rows.iter().position(|r| r.dir_path() == Some("target")).unwrap();
    app.file_cursor = row(&app);
    app.expand_dir();
    assert!(
        app.entries.iter().any(|e| e.path == "target/build.o" && e.ignored),
        "file child loads"
    );
    assert!(
        app.entries.iter().any(|e| e.path == "target/sub" && e.is_dir),
        "subdir placeholder loads"
    );
    assert!(!app.entries.iter().any(|e| e.path == "target/sub/y.o"), "deeper level stays lazy");

    // Collapse → children drop back out of the entry set.
    app.file_cursor = row(&app);
    app.collapse_dir();
    assert!(
        !app.entries.iter().any(|e| e.path.starts_with("target/")),
        "collapsing unloads children"
    );
}

#[test]
fn content_comment_is_stale_only_when_its_file_is_deleted() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    let row = file_row_of(&app, "a.rs").expect("a.rs at the top level");
    app.select_file(row).unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = 0;
    app.start_comment();
    for ch in "note".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    let c = app.store.get(0).expect("a comment was made").clone();
    assert!(!c.diff_anchored, "a File-view comment is content-anchored");

    app.reload().unwrap();
    assert!(!app.is_stale(&c), "a content comment on an existing, unchanged file is not stale");
    r.remove("a.rs");
    app.reload().unwrap();
    assert!(app.is_stale(&c), "it becomes stale only once its file is deleted");
}

#[test]
fn the_tabs_keep_independent_selections() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init"); // a clean worktree — no changes
    let mut app = app_on(&r);
    assert_eq!(app.changed_count(), 0);
    assert!(app.diff_path.is_none(), "Changes opens nothing with an empty changeset");

    app.set_tab(Tab::AllFiles).unwrap();
    let row = file_row_of(&app, "a.rs").unwrap();
    app.select_file(row).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"), "viewing a.rs in All files");

    // Back to Changes: nothing carries over, so its own (empty) state stands.
    app.set_tab(Tab::Changes).unwrap();
    assert!(app.diff_path.is_none(), "the All files selection does not carry into Changes");
}

#[test]
fn a_file_view_comment_exports_as_path_line_with_a_context_snippet() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\ngamma\n");
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    let row = file_row_of(&app, "a.rs").expect("a.rs listed");
    app.select_file(row).unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = 1; // the second line, "beta"
    app.start_comment();
    for ch in "why".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    let target = FakeTarget::ok();
    app.export(&target);
    let out = target.last();
    assert!(out.contains("a.rs:2"), "header is path:line:\n{out}");
    assert!(!out.contains("(removed)"), "a content comment never carries (removed):\n{out}");
    assert!(out.contains(" beta"), "the snippet is the space-prefixed content line:\n{out}");
}

#[test]
fn an_oversize_file_in_all_files_degrades_to_a_notice() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::diff::{FileState, View};
    let r = Repo::init();
    r.write("small.rs", "fn main() {}\n");
    r.write("big.bin", &"x\n".repeat(1_100_000)); // ~2.2 MB, over the 2 MB budget
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    let row = file_row_of(&app, "big.bin").expect("big.bin listed");
    app.select_file(row).unwrap();
    assert_eq!(app.diff.state, FileState::TooLarge, "an over-budget file is not read whole");
    assert_eq!(app.diff.view, View::File);
    assert!(app.visible.is_empty());
}

#[test]
fn switching_to_an_empty_file_view_focuses_the_tree() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "alpha\n");
    r.commit_all("init");
    r.remove("a.rs"); // deleted: still tracked (in ls-files) but empty on disk
    let mut app = app_on(&r);
    app.focus = Focus::Diff; // reader is in the diff pane on the deletion
    app.set_tab(Tab::AllFiles).unwrap();
    assert!(app.visible.is_empty(), "the deleted file's content view is empty");
    assert_eq!(app.focus, Focus::Files, "an empty left pane focuses the tree, not traps the keys");
}

#[test]
fn a_diff_comment_does_not_render_in_the_file_view() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::diff::View;
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\ngamma\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\ngamma\n"); // a.rs changed
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = row_with(&app, '+'); // the +BETA insertion
    app.start_comment();
    app.input_push('x');
    app.submit_comment();
    assert!(app.store.get(0).unwrap().diff_anchored, "made in the Changes diff");
    assert!(!app.commented_lines().is_empty(), "renders in its own diff view");

    // In All files, open a.rs as content: the diff-anchored comment must not bleed in.
    app.set_tab(Tab::AllFiles).unwrap();
    let row = file_row_of(&app, "a.rs").expect("a.rs listed");
    app.select_file(row).unwrap();
    assert_eq!(app.diff.view, View::File);
    assert!(
        app.commented_lines().is_empty(),
        "a diff-anchored comment does not render in the File view"
    );
}

#[test]
fn editing_a_comment_on_all_files_opens_the_file_view() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::diff::View;
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.write("b.rs", "one\ntwo\n");
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    // A content comment on a.rs.
    let arow = file_row_of(&app, "a.rs").expect("a.rs listed");
    app.select_file(arow).unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = 0;
    app.start_comment();
    app.input_push('x');
    app.submit_comment();
    // Open b.rs, so the comment's file is not the one shown.
    let brow = file_row_of(&app, "b.rs").expect("b.rs listed");
    app.select_file(brow).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("b.rs"));

    // Edit the comment from the list: it must bring a.rs back as a File view, not a diff.
    app.open_list();
    app.start_edit();
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"));
    assert_eq!(app.diff.view, View::File, "editing on All files opens the File view, not a diff");
    assert!(app.composing());
}

#[test]
fn changing_scope_on_all_files_snaps_the_changes_diff_to_the_top() {
    use std::fmt::Write as _;

    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    let mut body = String::new();
    for i in 0..40 {
        writeln!(body, "line {i}").unwrap();
    }
    r.write("a.rs", &body);
    r.commit_all("base");
    r.git(&["checkout", "-b", "feature"]);
    r.write("a.rs", &body.replace("line 5", "LINE 5"));
    r.commit_all("feature edit"); // a.rs differs from base → changed in branch scope
    r.write("a.rs", &body.replace("line 5", "LINE 5").replace("line 30", "LINE 30")); // uncommitted

    let mut app = app_on(&r); // Uncommitted scope; a.rs open in Changes
    app.focus = Focus::Diff;
    app.diff_cursor = 2;
    app.diff_scroll = 1;

    // Change scope while on All files, then return to Changes.
    app.set_tab(Tab::AllFiles).unwrap();
    app.set_scope(Scope::Branch).unwrap();
    app.set_tab(Tab::Changes).unwrap();

    assert!(app.entries.iter().any(|e| e.path == "a.rs"), "a.rs is in the branch changeset");
    assert_eq!(app.diff_scroll, 0, "an explicit scope switch snaps the Changes diff to the top");
    assert_eq!(app.diff_cursor, 0);
}

/// A PR-tab detour must not corrupt the two-tab diff stash: each file tab restores its own
/// open file when returned to, even after passing through the read-only `PR` tab.
#[test]
fn the_pr_tab_detour_preserves_each_file_tab_state() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.write("b.rs", "two\n");
    r.commit_all("init");
    r.write("a.rs", "ONE\n"); // a.rs is the only changed file
    let mut app = app_on(&r);

    assert_eq!(app.tab, Tab::Changes);
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"));

    // All files can open b.rs, which Changes can never show (b.rs is unchanged).
    app.set_tab(Tab::AllFiles).unwrap();
    app.select_file(file_row(&app, "b.rs")).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("b.rs"));

    // Detour through the PR tab; the file tabs stay frozen.
    app.set_tab(Tab::Pr).unwrap();
    assert_eq!(app.tab, Tab::Pr);

    // Returning to All files restores b.rs (active file tab unchanged → no swap).
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("b.rs"), "All files restored after the PR detour");

    // Returning to Changes swaps its state back — a.rs, never All files' b.rs.
    app.set_tab(Tab::Changes).unwrap();
    assert_eq!(app.tab, Tab::Changes);
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"), "Changes restored without bleeding b.rs");
}

/// The PR navigator cursor walks comments only (checks are a status display), the read pane
/// tracks the selected comment, and `pr_move` clamps at both ends.
#[test]
fn pr_navigator_walks_comments_only_and_clamps() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{
        Check, CheckStatus, Comment, CommentKind, Merge, PrSnapshot, PrState, PrView, Provider,
        RepliesState, Sync,
    };

    let finding = |author: &str| Comment {
        kind: CommentKind::Finding,
        author: author.into(),
        author_is_bot: true,
        anchor: "a.rs:1".into(),
        body: "b".into(),
        snippet: None,
        created_at: "2026-06-27T10:00:00Z".into(),
        is_resolved: false,
        is_outdated: false,
        reply_count: 0,
        replies: Vec::new(),
        replies_state: RepliesState::Complete,
        diff_anchor: None,
        remote_id: None,
    };
    let snap = PrSnapshot {
        provider: Provider::Github,
        number: 1,
        title: "t".into(),
        url: "u".into(),
        state: PrState::Open,
        is_draft: false,
        head_ref: "feature".into(),
        head_is_fork: false,
        base_ref: "main".into(),
        diff_refs: herdr_reviewr::forge::DiffRefs::default(),
        merge: Merge::Clean,
        sync: Sync::InSync,
        checks: vec![
            Check { name: "build".into(), status: CheckStatus::Success },
            Check { name: "test".into(), status: CheckStatus::Failure },
        ],
        comments: vec![finding("first"), finding("second")],
        truncated: false,
        threads_partial: None,
    };

    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::Pr).unwrap();
    app.pr = PrView::Pr(Box::new(snap));

    // The cursor starts on the first comment — checks are skipped entirely.
    assert_eq!(app.pr_row_count(), 2, "two comments; the two checks are not cursor stops");
    assert_eq!(app.pr_selected_comment().map(|c| c.author.as_str()), Some("first"));
    app.pr_move(1);
    assert_eq!(app.pr_selected_comment().map(|c| c.author.as_str()), Some("second"));
    app.pr_move(5);
    assert_eq!(
        app.pr_selected_comment().map(|c| c.author.as_str()),
        Some("second"),
        "clamps at the last comment"
    );
    app.pr_move(-10);
    assert_eq!(
        app.pr_selected_comment().map(|c| c.author.as_str()),
        Some("first"),
        "clamps at the first comment"
    );
}

#[test]
fn apply_pr_follows_the_selected_comment_across_a_refresh() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{
        Comment, CommentKind, Merge, PrSnapshot, PrState, PrView, Provider, RepliesState, Sync,
    };

    let comment = |author: &str, created: &str| Comment {
        kind: CommentKind::Comment,
        author: author.into(),
        author_is_bot: false,
        anchor: "comment".into(),
        body: "b".into(),
        snippet: None,
        created_at: created.into(),
        is_resolved: false,
        is_outdated: false,
        reply_count: 0,
        replies: Vec::new(),
        replies_state: RepliesState::Complete,
        diff_anchor: None,
        remote_id: None,
    };
    let snap = |comments: Vec<Comment>| {
        PrView::Pr(Box::new(PrSnapshot {
            provider: Provider::Github,
            number: 1,
            title: "t".into(),
            url: "u".into(),
            state: PrState::Open,
            is_draft: false,
            head_ref: "feature".into(),
            head_is_fork: false,
            base_ref: "main".into(),
            diff_refs: herdr_reviewr::forge::DiffRefs::default(),
            merge: Merge::Clean,
            sync: Sync::InSync,
            checks: Vec::new(),
            comments,
            truncated: false,
            threads_partial: None,
        }))
    };

    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = app_on(&r);
    app.set_tab(Tab::Pr).unwrap();

    // Newest-first [ann@10:00, bob@09:00]; the cursor lands on the newest, then move to bob.
    app.apply_pr(snap(vec![
        comment("ann", "2026-06-27T10:00:00Z"),
        comment("bob", "2026-06-27T09:00:00Z"),
    ]));
    assert_eq!(app.pr_selected_comment().map(|c| c.author.as_str()), Some("ann"));
    app.pr_move(1);
    assert_eq!(app.pr_selected_comment().map(|c| c.author.as_str()), Some("bob"));

    // A refresh prepends a newer comment: the cursor follows bob to its new index, not index 1.
    app.apply_pr(snap(vec![
        comment("cara", "2026-06-27T11:00:00Z"),
        comment("ann", "2026-06-27T10:00:00Z"),
        comment("bob", "2026-06-27T09:00:00Z"),
    ]));
    assert_eq!(
        app.pr_selected_comment().map(|c| c.author.as_str()),
        Some("bob"),
        "the cursor follows the same comment by identity, not its old index"
    );

    // A refresh where bob is gone clamps the now-dangling cursor back into range.
    app.apply_pr(snap(vec![
        comment("cara", "2026-06-27T11:00:00Z"),
        comment("ann", "2026-06-27T10:00:00Z"),
    ]));
    assert_eq!(
        app.pr_selected_comment().map(|c| c.author.as_str()),
        Some("ann"),
        "a vanished selection clamps to the last row"
    );
}

#[test]
fn same_input_failure_preserves_any_visible_pr_snapshot_and_remedy() {
    use herdr_reviewr::forge::PrView;

    let repo = Repo::init();
    let mut app = app_on(&repo);
    app.apply_pr(PrView::NoPr(vec!["feature".to_string()]));

    app.apply_pr(PrView::NotAuthed { tool: "gh", host: "github.example.com".to_string() });

    assert_eq!(app.pr, PrView::NoPr(vec!["feature".to_string()]));
    assert_eq!(
        app.pr_notice(),
        Some("not signed in — run `gh auth login --hostname github.example.com`, then press r")
    );
}

#[test]
fn theme_selection_swaps_the_palette_and_falls_back() {
    use herdr_reviewr::theme;
    let repo = Repo::init();
    let mut app = App::new(repo.path_buf(), Scope::Uncommitted, None);

    // The default theme is catppuccin (Mocha).
    assert_eq!(*app.palette(), theme::resolve(Some("catppuccin")).palette);

    // A --theme override (highest precedence) swaps the whole palette.
    app.set_cli_theme(Some("catppuccin-latte".to_string()));
    assert_eq!(*app.palette(), theme::resolve(Some("catppuccin-latte")).palette);

    // An unknown name falls back to the default — never a half-applied palette.
    app.set_cli_theme(Some("nope".to_string()));
    assert_eq!(*app.palette(), theme::resolve(Some("catppuccin")).palette);
}

#[test]
fn pr_picker_pins_and_a_branch_switch_unpins() {
    use herdr_reviewr::app::{PrPicker, Tab};
    use herdr_reviewr::forge::{PrFetchInput, PrListItem, PrListing, PrState, Provider};
    use herdr_reviewr::git::{OriginIdentity, RepoTarget};

    let repo = Repo::init();
    let mut app = app_on(&repo);
    app.set_tab(Tab::Pr).unwrap();

    // Before any probe there is no forge target: the picker opens straight into its
    // explanation instead of a fetch that can never resolve.
    app.open_pr_picker();
    assert!(matches!(app.pr_picker, Some(PrPicker::Failed(_))));
    assert!(app.take_pr_picker_fetch().is_none());
    app.close_pr_picker();

    // A probe input is the authority for the picker's context and the pin's branch.
    let target = RepoTarget {
        provider: Provider::Gitlab,
        host: "gitlab.example.com".into(),
        owner: "group/sub".into(),
        name: "project".into(),
    };
    let input = |branch: &str| PrFetchInput {
        origin: OriginIdentity::Repository(target.clone()),
        branch: Some(branch.to_string()),
        head_oid: Some("abc".into()),
        candidates: vec![branch.to_string()],
        base: None,
        base_branches: vec!["origin/main".into()],
        pinned: None,
    };
    let mut probed = input("feature");
    app.reconcile_pr_pin(&mut probed);
    assert_eq!(probed.pinned, None, "no pin yet");

    // The probe starts one prefetch. Opening before it lands keeps the loading state without
    // scheduling duplicate work.
    let listing_request = app.take_pr_picker_fetch().expect("prefetch request");
    assert_eq!(listing_request.target, target);
    app.open_pr_picker();
    assert_eq!(app.pr_picker, Some(PrPicker::Loading));
    assert!(app.take_pr_picker_fetch().is_none(), "one fetch per target");
    let item = |number: u64, title: &str| PrListItem {
        number,
        title: title.into(),
        head_ref: "feat/x".into(),
        author: "yassin17".into(),
        is_draft: false,
        state: PrState::Open,
        ci: None,
        created_at: "2026-07-01T00:00:00Z".into(),
        comments: 0,
        threads_open: None,
        threads_resolved: None,
    };
    // The cursor walks both sections: open first, then merged & closed.
    let mut done = item(7, "second");
    done.state = PrState::Merged;
    let first_listing_request = listing_request.clone();
    app.pr_picker_loaded(
        listing_request,
        Ok(PrListing { open: vec![item(41, "first")], done: vec![done] }),
    );

    // Fuzzy search accepts title fragments and provider-style numbers, then Enter maps the
    // filtered cursor back to the source listing.
    for ch in "SECOND".chars() {
        app.pr_picker_input(ch);
    }
    assert!(matches!(
        app.pr_picker,
        Some(PrPicker::Loaded { ref filtered, cursor: 0, .. }) if filtered == &[1]
    ));
    for _ in 0.."SECOND".len() {
        app.pr_picker_backspace();
    }
    for ch in "!7".chars() {
        app.pr_picker_input(ch);
    }
    assert!(matches!(
        app.pr_picker,
        Some(PrPicker::Loaded { ref filtered, .. }) if filtered == &[1]
    ));
    app.pr_pending = false;
    app.pr_picker_select();
    assert!(app.pr_picker.is_none());
    assert!(app.pr_pending, "a new pin refetches");
    assert_eq!(app.pr_pin.as_ref().map(|p| p.number), Some(7));
    let review_request = app.take_remote_changes_fetch().expect("selected review diff request");
    assert_eq!(review_request.target, target);
    assert_eq!(review_request.number, 7);
    assert!(app.take_remote_changes_fetch().is_none(), "one remote-diff fetch per trigger");

    // The next same-branch probe carries the pin; a branch switch drops it.
    let mut same = input("feature");
    app.reconcile_pr_pin(&mut same);
    assert_eq!(same.pinned, Some(7));
    let mut switched = input("other-branch");
    app.reconcile_pr_pin(&mut switched);
    assert_eq!(switched.pinned, None);
    assert!(app.pr_pin.is_none(), "a branch switch is a new review seat");
    app.remote_changes_loaded(review_request, Ok(herdr_reviewr::diff::PatchSet::default()))
        .unwrap();
    assert!(matches!(app.remote_changes, herdr_reviewr::app::RemoteChanges::Idle));

    // The target-scoped listing remains cached, so the next open paints immediately.
    assert!(app.pr_picker.is_none());
    app.open_pr_picker();
    assert!(matches!(
        app.pr_picker,
        Some(PrPicker::Loaded { ref listing, .. }) if listing.open[0].number == 41
    ));
    assert!(app.take_pr_picker_fetch().is_none(), "fresh cache suppresses another fetch");
    app.pr_picker_input('f');

    // A→B→A can leave the first A worker alive. Its old generation must not overwrite the
    // newer A request merely because the target happens to match again.
    let other_target = RepoTarget {
        provider: Provider::Gitlab,
        host: "gitlab.example.com".into(),
        owner: "other".into(),
        name: "project".into(),
    };
    let mut other = input("other-branch");
    other.origin = OriginIdentity::Repository(other_target);
    app.reconcile_pr_pin(&mut other);
    assert!(app.pr_picker.is_none(), "a target change closes the old repository's picker");
    assert!(app.pr_picker_query.is_empty());
    let _other_request = app.take_pr_picker_fetch().expect("B prefetch");
    let mut back = input("back-on-a");
    app.reconcile_pr_pin(&mut back);
    let current_a_request = app.take_pr_picker_fetch().expect("new A prefetch");
    app.pr_picker_loaded(
        first_listing_request,
        Ok(PrListing { open: vec![item(99, "obsolete")], done: vec![] }),
    );
    app.open_pr_picker();
    assert_eq!(app.pr_picker, Some(PrPicker::Loading));
    app.pr_picker_loaded(
        current_a_request,
        Ok(PrListing { open: vec![item(55, "current")], done: vec![] }),
    );
    assert!(matches!(
        app.pr_picker,
        Some(PrPicker::Loaded { ref listing, .. }) if listing.open[0].number == 55
    ));
    app.close_pr_picker();

    // esc unpins (no-op here — already unpinned by the switch).
    app.pr_pending = false;
    app.pr_unpin().unwrap();
    assert!(!app.pr_pending);
}

#[test]
fn applying_a_remote_diff_skips_the_local_branch_scan() {
    use herdr_reviewr::app::RemoteChanges;
    use herdr_reviewr::diff::PatchSet;
    use herdr_reviewr::forge::{Provider, ReviewDiffRequest};
    use herdr_reviewr::git::RepoTarget;

    let repo = Repo::init();
    repo.write("tracked.txt", "base\n");
    repo.commit_all("base");
    let mut app = App::new(repo.path_buf(), Scope::Branch, Some("definitely-missing-base".into()));
    let request = ReviewDiffRequest::new(
        RepoTarget {
            provider: Provider::Gitlab,
            host: "gitlab.example.com".into(),
            owner: "group".into(),
            name: "project".into(),
        },
        7,
    );
    app.remote_changes = RemoteChanges::Loading(request.clone());

    // A local branch reload with this explicit base fails. Applying a self-contained remote
    // patch must not touch that path at all.
    app.remote_changes_loaded(request, Ok(PatchSet::default()))
        .expect("remote patch does not scan the local branch");
    assert!(matches!(app.remote_changes, RemoteChanges::Ready { .. }));
}

#[test]
fn selected_remote_review_owns_changes_until_unpinned_without_touching_the_worktree() {
    use herdr_reviewr::app::{PrPin, RemoteChanges, Tab};
    use herdr_reviewr::diff::{PatchFile, PatchSet, Row};
    use herdr_reviewr::forge::{
        Comment as ForgeComment, CommentKind, DiffAnchor, DiffRefs, Merge, PrFetchInput,
        PrSnapshot, PrState, PrView, Provider, RemoteCommentId, RepliesState, ReviewDiffRequest,
        ReviewDraftAction, ReviewSyncOutcome, Sync,
    };
    use herdr_reviewr::git::{OriginIdentity, RepoTarget};
    use herdr_reviewr::model::{ChangeKind, ChangedFile};

    let repo = Repo::init();
    repo.write("local.txt", "base\n");
    repo.commit_all("base");
    repo.write("local.txt", "dirty\n");
    let mut app = app_on(&repo);
    assert_eq!(app.entries[0].path, "local.txt");

    let target = RepoTarget {
        provider: Provider::Gitlab,
        host: "gitlab.example.com".into(),
        owner: "group".into(),
        name: "project".into(),
    };
    let request = ReviewDiffRequest::new(target.clone(), 42);
    let remote = PatchFile {
        change: ChangedFile {
            path: "src/remote.rs".into(),
            kind: ChangeKind::Modified,
            additions: 1,
            deletions: 1,
            previous_path: None,
        },
        patch: Some("@@ -5 +5 @@\n-old\n+new\n".into()),
        too_large: false,
    };
    app.pr_pin = Some(PrPin { branch: Some("local".into()), number: 42 });
    let mut input = PrFetchInput {
        origin: OriginIdentity::Repository(target.clone()),
        branch: Some("local".into()),
        head_oid: Some("head".into()),
        candidates: vec!["local".into()],
        base: None,
        base_branches: vec!["main".into()],
        pinned: None,
    };
    app.reconcile_pr_pin(&mut input);
    app.pr = PrView::Pr(Box::new(PrSnapshot {
        provider: Provider::Gitlab,
        number: 42,
        title: "review".into(),
        url: "https://gitlab.example.com/group/project/-/merge_requests/42".into(),
        state: PrState::Open,
        is_draft: false,
        head_ref: "feature".into(),
        head_is_fork: false,
        base_ref: "main".into(),
        diff_refs: DiffRefs {
            base_sha: "base".into(),
            start_sha: "start".into(),
            head_sha: "head".into(),
        },
        merge: Merge::Clean,
        sync: Sync::Unknown,
        checks: vec![],
        comments: vec![ForgeComment {
            kind: CommentKind::Finding,
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
            replies_state: RepliesState::Complete,
            diff_anchor: Some(DiffAnchor {
                path: "src/remote.rs".into(),
                old_path: None,
                side: herdr_reviewr::model::Side::New,
                line: 5,
                start_line: None,
                endpoints: None,
            }),
            remote_id: Some(RemoteCommentId {
                thread_id: "thread".into(),
                root_comment_id: Some(9),
            }),
        }],
        truncated: false,
        threads_partial: None,
    }));
    app.remote_changes = RemoteChanges::Ready {
        request,
        patch: PatchSet { files: vec![remote.clone()], truncated: false },
    };
    let mut context = herdr_reviewr::forge::PrFetchInput {
        origin: herdr_reviewr::git::OriginIdentity::Repository(target.clone()),
        branch: Some("local".into()),
        head_oid: Some("head".into()),
        candidates: vec!["local".into()],
        base: None,
        base_branches: vec!["main".into()],
        pinned: None,
    };
    app.reconcile_pr_pin(&mut context);
    app.reload().unwrap();

    assert_eq!(app.tab, Tab::Changes);
    assert_eq!(app.entries[0].path, "src/remote.rs");
    assert!(app.tree_badges(&app.file_rows[0]).commented, "remote threads mark their file");
    assert!(app.visible.iter().any(|row| matches!(row, Row::Insertion { new_no: 5, .. })));
    assert!(app.remote_comment_cards().iter().any(|cards| !cards.is_empty()));
    assert!(
        app.commented_lines()
            .iter()
            .any(|&i| matches!(app.visible[i], Row::Insertion { new_no: 5, .. })),
        "a forge thread marks its anchored row"
    );
    assert!(app.pr_selected_context().is_some(), "PR reader reuses remote patch context");
    app.focus = herdr_reviewr::app::Focus::Diff;
    assert_eq!(
        std::fs::read_to_string(repo.path().join("local.txt")).unwrap(),
        "dirty\n",
        "remote review never changes the checkout"
    );
    assert!(
        app.footer_actions()
            .iter()
            .any(|(action, _)| { matches!(action, herdr_reviewr::app::FooterAction::Comment) }),
        "remote review accepts local drafts"
    );

    app.focus = herdr_reviewr::app::Focus::Diff;
    // A mixed-side selection anchors to the new side (the deletion carries no new-side
    // number), so GitLab accepts it exactly like GitHub — here it collapses to the
    // insertion's single line.
    let deletion = app.visible.iter().position(|row| matches!(row, Row::Deletion { .. })).unwrap();
    let insertion =
        app.visible.iter().position(|row| matches!(row, Row::Insertion { .. })).unwrap();
    app.diff_cursor = deletion;
    app.toggle_select();
    app.diff_cursor = insertion;
    app.start_comment();
    assert!(app.composing(), "GitLab accepts a multi-row selection");
    for ch in "please adjust".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    assert_eq!(app.remote_draft_count(), 1);
    let ReviewDraftAction::Inline(anchor) = &app.remote_drafts[0].draft.action else {
        panic!("inline draft expected");
    };
    assert_eq!((anchor.line, anchor.start_line), (5, None));
    assert!(anchor.endpoints.is_some(), "authored anchors carry parser endpoints");

    if let PrView::Pr(snapshot) = &mut app.pr {
        snapshot.number = 41;
    }
    app.request_remote_sync();
    assert!(app.take_remote_sync().is_none(), "stale review refs cannot start a sync");
    assert!(app.status.contains("current commit details"));
    app.set_tab(Tab::Pr).unwrap();
    assert!(
        !app.footer_actions()
            .iter()
            .any(|(action, _)| { matches!(action, herdr_reviewr::app::FooterAction::Reply) }),
        "the old review's comment is not replyable while new metadata is loading"
    );
    app.start_pr_reply();
    assert!(!app.composing(), "a stale comment cannot start a reply on the new review");
    app.set_tab(Tab::Changes).unwrap();
    if let PrView::Pr(snapshot) = &mut app.pr {
        snapshot.number = 42;
    }
    app.request_remote_sync();
    let first_sync = app.take_remote_sync().expect("grouped sync request");
    assert_eq!(first_sync.drafts.len(), 1);
    let local_id = first_sync.drafts[0].local_id;
    app.remote_sync_finished(
        &first_sync,
        &ReviewSyncOutcome {
            succeeded: vec![],
            failed: vec![(local_id, "rejected".into())],
            uncertain: vec![],
        },
    );
    assert_eq!(app.remote_draft_count(), 1, "failed group items remain retryable");
    assert_eq!(app.remote_drafts[0].error.as_deref(), Some("rejected"));

    app.request_remote_sync();
    let retry = app.take_remote_sync().expect("retry group");
    app.remote_sync_finished(
        &retry,
        &ReviewSyncOutcome { succeeded: vec![local_id], failed: vec![], uncertain: vec![] },
    );
    assert_eq!(app.remote_draft_count(), 0, "successful drafts leave the local queue");

    app.set_tab(Tab::Pr).unwrap();
    app.start_pr_reply();
    // A background identity convergence while the editor is open cannot retarget the reply.
    app.remote_changes = RemoteChanges::Idle;
    if let PrView::Pr(snapshot) = &mut app.pr {
        snapshot.number = 43;
    }
    for ch in "reply from reviewr".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    assert!(matches!(
        app.remote_drafts.last().unwrap().draft.action,
        ReviewDraftAction::Reply { .. }
    ));
    assert_eq!(app.remote_drafts.last().unwrap().number, 42);
    if let PrView::Pr(snapshot) = &mut app.pr {
        snapshot.number = 42;
    }
    app.remote_changes = RemoteChanges::Ready {
        request: ReviewDiffRequest::new(target.clone(), 42),
        patch: PatchSet { files: vec![remote.clone()], truncated: false },
    };
    app.request_remote_sync();
    let uncertain = app.take_remote_sync().expect("reply sync request");
    let reply_id = uncertain.drafts[0].local_id;
    app.remote_sync_finished(
        &uncertain,
        &ReviewSyncOutcome {
            succeeded: vec![],
            failed: vec![],
            uncertain: vec![(reply_id, "connection lost after POST".into())],
        },
    );
    assert!(app.remote_drafts.last().unwrap().outcome_unknown);
    app.request_remote_sync();
    assert!(app.take_remote_sync().is_none(), "unknown outcomes never retry blindly");
    app.set_tab(Tab::Changes).unwrap();

    app.pr_unpin().unwrap();
    assert_eq!(app.entries[0].path, "local.txt", "unpin restores local scope immediately");

    // A branch-switch probe can complete after the user has already returned to Changes. It
    // must restore local rows in the same transition, before authoring becomes available.
    app.pr_pin = Some(PrPin { branch: Some("feature".into()), number: 42 });
    app.remote_changes = RemoteChanges::Ready {
        request: ReviewDiffRequest::new(target.clone(), 42),
        patch: PatchSet { files: vec![remote], truncated: false },
    };
    app.reload().unwrap();
    let mut switched = herdr_reviewr::forge::PrFetchInput {
        origin: herdr_reviewr::git::OriginIdentity::Repository(target),
        branch: Some("other".into()),
        head_oid: Some("abc".into()),
        candidates: vec!["other".into()],
        base: None,
        base_branches: vec!["main".into()],
        pinned: None,
    };
    app.reconcile_pr_pin(&mut switched);
    assert!(!app.remote_changes_active());
    assert_eq!(app.entries[0].path, "local.txt", "branch switch cannot leave stale remote rows");
    assert_eq!(std::fs::read_to_string(repo.path().join("local.txt")).unwrap(), "dirty\n");
}

#[test]
fn a_gitlab_range_selection_drafts_a_multi_line_note_anchor() {
    use herdr_reviewr::app::{RemoteChanges, Tab};
    use herdr_reviewr::diff::{EndpointKind, PatchFile, PatchSet, RangeEndpoint, Row};
    use herdr_reviewr::forge::{Provider, ReviewDiffRequest, ReviewDraftAction};
    use herdr_reviewr::git::RepoTarget;
    use herdr_reviewr::model::{ChangeKind, ChangedFile};

    let repo = Repo::init();
    repo.write("local.txt", "base\n");
    repo.commit_all("base");
    let mut app = app_on(&repo);

    let target = RepoTarget {
        provider: Provider::Gitlab,
        host: "gitlab.example.com".into(),
        owner: "group".into(),
        name: "project".into(),
    };
    let remote = PatchFile {
        change: ChangedFile {
            path: "src/remote.rs".into(),
            kind: ChangeKind::Modified,
            additions: 2,
            deletions: 1,
            previous_path: None,
        },
        patch: Some("@@ -5,2 +5,3 @@\n same\n-old\n+new\n+more\n".into()),
        too_large: false,
    };
    app.remote_changes = RemoteChanges::Ready {
        request: ReviewDiffRequest::new(target, 7),
        patch: PatchSet { files: vec![remote], truncated: false },
    };
    app.reload().unwrap();
    assert_eq!(app.tab, Tab::Changes);
    app.focus = herdr_reviewr::app::Focus::Diff;

    let first =
        app.visible.iter().position(|row| matches!(row, Row::Insertion { new_no: 6, .. })).unwrap();
    let second =
        app.visible.iter().position(|row| matches!(row, Row::Insertion { new_no: 7, .. })).unwrap();
    app.diff_cursor = first;
    app.toggle_select();
    app.diff_cursor = second;
    app.start_comment();
    assert!(app.composing(), "a same-side GitLab range opens the composer");
    for ch in "tighten this block".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    assert_eq!(app.remote_draft_count(), 1);
    let ReviewDraftAction::Inline(anchor) = &app.remote_drafts[0].draft.action else {
        panic!("inline draft expected");
    };
    assert_eq!((anchor.start_line, anchor.line), (Some(6), 7));
    assert_eq!(anchor.location(), "src/remote.rs:6-7");
    // Both endpoints carry the unified-diff position pair GitLab's line codes need: the
    // deletion consumed old line 6, so both added lines hold the old counter at 7.
    assert_eq!(
        anchor.endpoints,
        Some((
            RangeEndpoint { old_pos: 7, new_pos: 6, kind: EndpointKind::Added },
            RangeEndpoint { old_pos: 7, new_pos: 7, kind: EndpointKind::Added },
        ))
    );

    // The viewer shows the range too: every covered row is marked, and the card sits
    // under the range's last row.
    let commented = app.commented_lines();
    assert!(commented.contains(&first), "the range's first row is marked");
    assert!(commented.contains(&second), "the range's last row is marked");
    assert!(
        !commented.contains(&(first - 1)),
        "rows outside the range stay unmarked (the deletion above it)"
    );
    let cards = app.remote_draft_cards();
    assert_eq!(cards[second], vec![0], "the draft card splices under the range's last row");
    assert!(cards[first].is_empty(), "no duplicate card on interior rows");
}

#[test]
fn switching_file_tabs_preserves_the_logical_location() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::model::Side;
    let r = Repo::init();
    r.write("a.rs", "one\ntwo\nthree\nfour\n");
    r.commit_all("init");
    r.write("a.rs", "one\ntwo\nTHREE\nfour\n");
    let mut app = app_on(&r);
    app.focus = herdr_reviewr::app::Focus::Diff;
    app.diff_cursor = app
        .visible
        .iter()
        .position(|row| row.new_no() == Some(3))
        .expect("the changed line renders");
    assert_eq!(app.current_location(), Some(("a.rs".to_string(), Side::New, 3)));

    // `2` — the All-files projection lands on the same file and line.
    app.set_tab(Tab::AllFiles).unwrap();
    assert_eq!(app.current_location(), Some(("a.rs".to_string(), Side::New, 3)));

    // Move to an unchanged line, then `1`: Changes shows the same line as diff context.
    app.diff_cursor = app
        .visible
        .iter()
        .position(|row| row.new_no() == Some(2))
        .expect("line 2 in the file view");
    app.set_tab(Tab::Changes).unwrap();
    assert_eq!(
        app.current_location(),
        Some(("a.rs".to_string(), Side::New, 2)),
        "a context line is representable in the diff projection"
    );
}

#[test]
fn a_location_changes_cannot_represent_keeps_the_file_and_says_so() {
    use herdr_reviewr::app::Tab;
    use std::fmt::Write as _;
    let r = Repo::init();
    // Enough unchanged lines that the head of the file folds away from the single
    // trailing change, making early lines unrepresentable in the Changes projection.
    let mut body = String::new();
    for i in 1..=40 {
        let _ = writeln!(body, "line {i}");
    }
    r.write("a.rs", &body);
    r.commit_all("init");
    r.write("a.rs", &body.replace("line 40", "LINE 40"));
    let mut app = app_on(&r);
    app.set_tab(Tab::AllFiles).unwrap();
    app.focus = herdr_reviewr::app::Focus::Diff;
    app.diff_cursor = app
        .visible
        .iter()
        .position(|row| row.new_no() == Some(1))
        .expect("line 1 in the file view");

    app.set_tab(Tab::Changes).unwrap();
    assert_eq!(app.diff_path.as_deref(), Some("a.rs"), "the file stays selected");
    // Line 1 hides inside the leading fold: the cursor lands on the fold that holds it
    // (so `→ expand` reaches it) or the state names the unrepresentable location.
    let on_fold = app.visible.get(app.diff_cursor).is_some_and(|row| row.hidden() > 0);
    assert!(
        on_fold || app.status.contains("outside this change set"),
        "unrepresentable locations are explicit, not silently retargeted: {}",
        app.status
    );
}

#[test]
fn agent_navigation_opens_the_file_and_lands_on_the_line() {
    let r = Repo::init();
    r.write("src/a.rs", "one\ntwo\nthree\n");
    r.write("src/b.rs", "alpha\nbeta\n");
    r.commit_all("init");
    r.write("src/a.rs", "one\ntwo\nTHREE\n");
    r.write("src/b.rs", "alpha\nBETA\n");
    let mut app = app_on(&r);

    let outcome = app.collab_navigate_to("src/b.rs", Some(2), false);
    assert_eq!(outcome, NavOutcome::Landed);
    assert_eq!(app.diff_path.as_deref(), Some("src/b.rs"));
    assert_eq!(
        app.visible.get(app.diff_cursor).and_then(herdr_reviewr::diff::Row::new_no),
        Some(2),
        "the cursor sits on the agent's line"
    );

    // An absolute path resolves against the worktree.
    let absolute = r.path().join("src/a.rs");
    app.collab_navigate_to(absolute.to_str().unwrap(), Some(3), false);
    assert_eq!(app.diff_path.as_deref(), Some("src/a.rs"));

    // A strict attempt reports an unrepresentable line so the caller can refresh first.
    let outcome = app.collab_navigate_to("src/a.rs", Some(99), false);
    assert_eq!(outcome, NavOutcome::LineMissing, "a strict miss is reported, not absorbed");

    // Settling lands on the nearest shown line and names the real location out loud.
    let outcome = app.collab_navigate_to("src/a.rs", Some(99), true);
    assert_eq!(outcome, NavOutcome::Landed);
    assert!(
        app.status.contains("outside this change set"),
        "unrepresentable agent locations are explicit: {}",
        app.status
    );
    assert!(
        app.visible.get(app.diff_cursor).and_then(herdr_reviewr::diff::Row::new_no).is_some(),
        "the settled cursor sits on a real line"
    );

    // A file outside the scope's tree reports the miss; settling says so in the status.
    let outcome = app.collab_navigate_to("src/ghost.rs", Some(1), true);
    assert_eq!(outcome, NavOutcome::FileMissing);
    assert!(app.status.contains("not in this view"), "file misses are audible: {}", app.status);
}

#[test]
fn a_live_edit_under_a_content_comment_marks_it_stale_never_rebinds_it() {
    use herdr_reviewr::collab::protocol::{DraftAnchor, StagedDraft};
    let r = Repo::init();
    r.write("a.rs", "one\ntwo\nthree\n");
    r.commit_all("init");
    r.write("a.rs", "one\ntwo\nTHREE\n");
    let mut app = app_on(&r);
    app.collab_stage_draft(&StagedDraft {
        draft: "d1".into(),
        body: "look here".into(),
        anchor: Some(DraftAnchor { path: "a.rs".into(), line: 2, start_line: None }),
        reply_to: None,
    })
    .unwrap();
    let comment = app.store.get(0).unwrap().clone();
    assert!(!app.is_stale(&comment), "the anchor matches the worktree");

    // The agent rewrites the anchored line: the comment is marked, not silently rebound.
    r.write("a.rs", "one\nTWO CHANGED\nTHREE\n");
    app.reload().unwrap();
    let comment = app.store.get(0).unwrap().clone();
    assert!(app.is_stale(&comment), "a shifted anchor reads stale");

    // The file disappearing is stale too; an untouched anchor stays live.
    r.write("a.rs", "one\ntwo\nTHREE\n");
    assert!(!app.is_stale(&comment), "restoring the lines restores freshness");
}
