//! Render tests: drive `ui::render` through ratatui's `TestBackend` and assert on
//! the painted buffer, so the layout and component wiring are checked for real.

mod common;

use common::Repo;
use herdr_reviewr::app::{App, Focus};
use herdr_reviewr::model::{Comment, Scope, Side};
use herdr_reviewr::ui::{self, HeaderHit};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn dump(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    dump(terminal.backend().buffer())
}

/// Render and return the buffer, for cell-style assertions.
fn render_buffer(app: &App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// Render at a specific width (height fixed), for footer fit-to-width assertions.
fn render_at(app: &App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    dump(terminal.backend().buffer())
}

/// Catppuccin surface2 — the shared selection/cursor fill.
const SELECTION_BG: ratatui::style::Color = ratatui::style::Color::Rgb(0x58, 0x5b, 0x70);
/// Catppuccin peach — the comment-editor caret block.
const PEACH: ratatui::style::Color = ratatui::style::Color::Rgb(0xfa, 0xb3, 0x87);

/// Open the comment composer on the first changed line of `edited_app`.
fn composing(app: &mut App) {
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|r| r.marker() == '+').unwrap();
    app.start_comment();
}

#[test]
fn invalid_config_replaces_the_entire_sidebar_with_its_error() {
    let mut app = edited_app();
    app.set_config_error(
        "config /tmp/reviewr/config.toml: invalid value for `theme`; expected a built-in theme name"
            .to_string(),
    );

    let out = render(&app);

    assert!(out.contains("config /tmp/reviewr/config.toml"));
    assert!(out.contains("expected a built-in theme name"));
    assert!(!out.contains("Changes"), "normal sidebar chrome must be hidden");
}

#[test]
fn the_empty_comment_box_shows_a_placeholder() {
    let mut app = edited_app();
    composing(&mut app);
    assert!(render(&app).contains("Leave a comment…"), "an empty box shows the placeholder");
}

#[test]
fn the_caret_block_sits_on_the_character_at_the_caret() {
    let mut app = edited_app();
    composing(&mut app);
    app.input_push('a');
    app.input_push('b');
    app.caret_left(); // caret between 'a' and 'b' → block over 'b'
    let buf = render_buffer(&app);
    let mut found = false;
    for y in 0..40 {
        for x in 0..140 {
            if buf.cell((x, y)).is_some_and(|c| c.bg == PEACH && c.symbol() == "b") {
                found = true;
            }
        }
    }
    assert!(found, "the caret block highlights the character at the caret");
}

#[test]
fn caret_vertical_moves_between_wrapped_rows() {
    // "abcdef" hard-wraps at width 3 to "abc"/"def"; caret 4 (def col 1) up → 1; 1 down → 4.
    assert_eq!(ui::caret_vertical("abcdef", 4, 3, false), 1);
    assert_eq!(ui::caret_vertical("abcdef", 1, 3, true), 4);
}

#[test]
fn the_fold_hint_names_the_arrow_key() {
    use std::fmt::Write as _;
    let r = Repo::init();
    let mut body = String::new();
    for i in 0..30 {
        let _ = writeln!(body, "line {i}");
    }
    r.write("f.rs", &body);
    r.commit_all("init");
    r.write("f.rs", &body.replace("line 15", "LINE 15")); // one change, long runs fold
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|row| row.hidden() > 0).expect("a fold row");

    let out = render(&app);
    assert!(out.contains("→ expand"), "the fold hint names the `→` key");
    assert!(!out.contains("⏎ expand"), "no stale enter hint remains");
}

#[test]
fn pi_edits_since_the_session_baseline_carry_a_gutter_badge() {
    let r = Repo::init();
    r.write("hello.rs", "alpha\nbeta\ngamma\n");
    r.commit_all("init");
    // The reviewer's own WIP predates the deep session, so it lands inside the baseline
    // snapshot and must never be attributed to the agent.
    r.write("hello.rs", "ALPHA\nbeta\ngamma\n");
    let baseline = herdr_reviewr::git::snapshot_worktree(r.path()).unwrap();
    // The agent edits after the session started.
    r.write("hello.rs", "ALPHA\nbeta\nGAMMA\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.collab_baseline = Some(baseline);
    app.reload().unwrap();

    let badged: Vec<String> =
        render(&app).lines().filter(|line| line.contains('✦')).map(str::to_string).collect();
    assert!(
        badged.iter().any(|line| line.contains("GAMMA")),
        "the agent's edit carries the ✦ badge: {badged:?}"
    );
    assert!(
        !badged.iter().any(|line| line.contains("ALPHA")),
        "the reviewer's pre-session WIP is not the agent's: {badged:?}"
    );
}

#[test]
fn a_file_the_agent_deleted_badges_its_deletion_rows() {
    let r = Repo::init();
    r.write("gone.rs", "alpha\nbeta\n");
    r.write("kept.rs", "one\n");
    r.commit_all("init");
    // The reviewer deleted one file before the session — that removal is the review's.
    std::fs::remove_file(r.path().join("kept.rs")).unwrap();
    let baseline = herdr_reviewr::git::snapshot_worktree(r.path()).unwrap();
    // The agent deletes the other file after the session started: with no surviving
    // worktree line to carry a mark, its deletion rows badge whole.
    std::fs::remove_file(r.path().join("gone.rs")).unwrap();
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.collab_baseline = Some(baseline);
    app.reload().unwrap();

    // `gone.rs` sorts first and opens on reload.
    let badged: Vec<String> =
        render(&app).lines().filter(|line| line.contains('✦')).map(str::to_string).collect();
    assert!(
        badged.iter().any(|line| line.contains("alpha")),
        "the agent's whole-file deletion carries the ✦ badge: {badged:?}"
    );

    app.move_cursor(1).unwrap(); // the files pane has focus on start — step to `kept.rs`
    let out = render(&app);
    assert!(!out.contains('✦'), "the reviewer's pre-session deletion is not the agent's:\n{out}");
}

fn edited_app() -> App {
    let r = Repo::init();
    r.write("hello.rs", "alpha\nbeta\n");
    r.commit_all("init");
    r.write("hello.rs", "alpha\nBETA\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    // The repo is only needed through reload(); rendering reads cached state, so
    // `r` can drop here and clean up its tempdir.
    app
}

#[test]
fn the_file_list_renders_as_a_directory_tree() {
    let r = Repo::init();
    r.write("src/app.rs", "x\n");
    r.write("src/ui.rs", "y\n");
    r.write("Cargo.toml", "[package]\n");
    r.commit_all("init");
    r.write("src/app.rs", "x2\n");
    r.write("src/ui.rs", "y2\n");
    r.write("Cargo.toml", "[package]\nname='z'\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    // Scan only the file-list pane (the right third) so the diff header — which does show
    // the open file's full path — doesn't confuse the assertions.
    let files_pane: String = render(&app)
        .lines()
        .map(|l| l.chars().skip(l.chars().count() * 70 / 100).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(files_pane.contains("src/"), "the directory groups its files: {files_pane:?}");
    assert!(files_pane.contains("app.rs") && files_pane.contains("ui.rs"), "files by basename");
    assert!(!files_pane.contains("src/app.rs"), "a grouped file is not shown by full path");
    assert!(files_pane.contains("Cargo.toml"), "the top-level file shows too");
}

#[test]
fn all_files_folders_render_change_and_comment_badges() {
    let r = Repo::init();
    r.write("src/app.rs", "one\n");
    r.write("src/ui.rs", "one\n");
    r.commit_all("init");
    r.write("src/app.rs", "two\n");
    r.write("src/ui.rs", "two\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|row| row.marker() == '+').unwrap();
    app.start_comment();
    for ch in "review this".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    app.set_tab(herdr_reviewr::app::Tab::AllFiles).unwrap();

    let out = render(&app);
    // The diff header may also carry a `src/` path, so find the folder row by its badge.
    let folder = out
        .lines()
        .find(|line| line.contains("src/") && line.contains('±'))
        .expect("src folder row");
    assert!(folder.contains('±'), "changed descendants mark the All-files folder: {folder}");
    assert!(folder.contains('◆'), "commented descendants mark the folder: {folder}");

    let narrow = render_at(&app, 50);
    let folder = narrow
        .lines()
        .find(|line| line.contains("src/") && line.contains('±'))
        .expect("narrow src row");
    assert!(
        folder.contains('±') && folder.contains('◆'),
        "name elision preserves both folder badges at narrow widths: {folder}"
    );
}

#[test]
fn a_saved_comment_renders_inline_as_a_card() {
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|row| row.marker() == '+').unwrap();
    app.start_comment();
    for ch in "memoize this".chars() {
        app.input_push(ch);
    }
    app.submit_comment(); // box closes, comment saved

    let out = render(&app);
    assert!(out.contains("memoize this"), "the saved comment stays visible inline: {out:?}");
    assert!(out.contains("comment ·"), "the inline card is titled with the location");
}

#[test]
fn a_renamed_file_shows_old_arrow_new_in_the_header() {
    let r = Repo::init();
    r.write("old_name.rs", "stable contents that survive the move\nplus a second line\n");
    r.commit_all("init");
    r.git(&["mv", "old_name.rs", "new_name.rs"]);
    r.write("new_name.rs", "stable contents that survive the move\nplus an edited line\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    let out = render(&app);
    assert!(out.contains("old_name.rs → new_name.rs"), "header shows the rename: {out:?}");
}

#[test]
fn tabs_expand_to_spaces_in_the_diff() {
    let r = Repo::init();
    r.write("t.rs", "x\n");
    r.commit_all("init");
    r.write("t.rs", "x\n\tindented\n"); // a tab-indented added line
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    let out = render(&app);
    let line = out.lines().find(|l| l.contains("indented")).expect("the added line renders");
    // The literal tab is gone; the word is preceded by spaces (4-col tab stop).
    assert!(!line.contains('\t'), "no literal tab in the rendered line");
    assert!(line.contains("    indented") || line.contains("   indented"), "tab became spaces");
}

#[test]
fn a_long_line_wraps_across_display_rows() {
    let long: String = std::iter::repeat_n("abcd", 60).collect(); // 240 cols, wider than the pane
    let r = Repo::init();
    r.write("w.rs", "x\n");
    r.commit_all("init");
    r.write("w.rs", &format!("x\n{long}\n"));
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap(); // wrap defaults on

    // The whole long line is visible (no truncation): every chunk renders.
    let shown: String = render(&app).chars().filter(|c| *c == 'a').collect();
    assert!(shown.len() >= 60, "all of the wrapped line is shown, not truncated");
    // The logical row reports a display height > 1 (it wraps).
    let heights = ui::diff_row_heights(&app, AREA);
    let wrapped = app.visible.iter().position(|r| r.text().starts_with("abcd")).unwrap();
    assert!(heights[wrapped] > 1, "the long line spans multiple display rows");
}

#[test]
fn wrapping_breaks_at_word_boundaries() {
    // Words sized so the line must wrap, but no word is wider than the pane: every break
    // should land on a space, so no word is split across two display rows.
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima \
                 mike november oscar papa quebec romeo sierra tango";
    let r = Repo::init();
    r.write("w.rs", "x\n");
    r.commit_all("init");
    r.write("w.rs", &format!("x\n{words}\n"));
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap(); // wrap defaults on

    let heights = ui::diff_row_heights(&app, AREA);
    let wrapped = app.visible.iter().position(|r| r.text().starts_with("alpha")).unwrap();
    assert!(heights[wrapped] > 1, "the line wraps across rows");

    // Every word survives intact on some rendered line (none straddles a wrap break).
    let out = render(&app);
    for word in words.split(' ') {
        assert!(out.lines().any(|l| l.contains(word)), "word {word:?} is not split across rows");
    }
}

#[test]
fn wide_glyphs_wrap_by_column_width_not_char_count() {
    // 50 wide CJK glyphs span 100 columns; 50 ASCII chars span 50. Width-aware wrapping
    // must give the CJK line more display rows — a char-counting wrap would tie them.
    let cjk: String = std::iter::repeat_n('あ', 50).collect();
    let ascii: String = std::iter::repeat_n('a', 50).collect();
    let r = Repo::init();
    r.write("w.rs", "x\n");
    r.commit_all("init");
    r.write("w.rs", &format!("x\n{ascii}\n{cjk}\n"));
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap(); // wrap defaults on

    let heights = ui::diff_row_heights(&app, AREA);
    let ascii_h = heights[app.visible.iter().position(|r| r.text().starts_with('a')).unwrap()];
    let cjk_h = heights[app.visible.iter().position(|r| r.text().starts_with('あ')).unwrap()];
    assert!(cjk_h > ascii_h, "wide glyphs wrap by columns: cjk {cjk_h} > ascii {ascii_h}");
}

#[test]
fn horizontal_scroll_shifts_the_diff_left() {
    let r = Repo::init();
    r.write("w.rs", "x\n");
    r.commit_all("init");
    r.write("w.rs", "x\nAAAABBBBCCCCDDDD_marker\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.wrap = false; // horizontal scroll applies only with wrap off
    app.reload().unwrap();
    assert!(render(&app).contains("AAAABBBB"), "the line head shows before scrolling");

    app.scroll_h(8); // drop the first 8 code columns
    let out = render(&app);
    assert!(!out.contains("AAAABBBB"), "the scrolled-off head is gone");
    assert!(out.contains("CCCCDDDD_marker"), "the later columns are now visible");
}

#[test]
fn a_changed_word_gets_the_emphasis_background() {
    const EMPH_INS_BG: ratatui::style::Color = ratatui::style::Color::Rgb(0x30, 0x55, 0x3f);
    let r = Repo::init();
    r.write("e.rs", "let x = foo(a);\n");
    r.commit_all("init");
    r.write("e.rs", "let x = bar(a, b);\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Files; // no diff cursor, so the emphasis bg shows
    let buf = render_buffer(&app);

    // Somewhere in the diff pane a cell carries the brighter insertion-emphasis bg,
    // and it sits under a changed character (a `b` from `bar`), not the shared prefix.
    let mut found = false;
    for y in 0..40 {
        for x in 0..95 {
            if let Some(c) = buf.cell((x, y))
                && c.bg == EMPH_INS_BG
                && c.symbol() == "b"
            {
                found = true;
            }
        }
    }
    assert!(found, "a changed word carries the emphasis background");
}

#[test]
fn the_selected_file_row_fills_with_the_shared_selection_color() {
    let app = edited_app(); // one file, file_cursor = 0, Files focused
    let buf = render_buffer(&app);
    // Files pane: right 32% of 140 cols; its border is at y=1, first content row at y=2.
    let files_x0 = 140 - 140 * 32 / 100 + 1;
    let selected =
        (files_x0..139).filter(|&x| buf.cell((x, 2)).is_some_and(|c| c.bg == SELECTION_BG)).count();
    assert!(selected > 10, "the selected file row fills wide with surface2: {selected} cells");
}

#[test]
fn shows_tab_bar_file_list_and_diff() {
    let app = edited_app();
    let out = render(&app);
    assert!(out.contains("Changes"), "tab bar names the view");
    assert!(out.contains("uncommitted"), "current scope shown");
    assert!(out.contains("hello.rs"), "file appears in the list");
    assert!(out.contains("BETA"), "diff content is rendered");
    assert!(out.contains("changed"), "the header shows the changed count");
}

/// The last non-blank rendered row — the footer band.
fn footer_line(out: &str) -> String {
    out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().to_string()
}

/// Focus the diff on its first changed line.
fn on_changed_line(app: &mut App) {
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|r| r.marker() == '+').unwrap();
}

#[test]
fn the_footer_shows_the_action_for_the_context() {
    let mut app = edited_app();
    on_changed_line(&mut app);
    let footer = footer_line(&render(&app));
    assert!(footer.contains("c comment"), "a diff line offers comment:\n{footer}");
    assert!(footer.contains("v select"), "and selecting a range:\n{footer}");
    assert!(!footer.contains("changed"), "the changed count is not in the footer:\n{footer}");
}

#[test]
fn the_footer_drops_to_fit_and_marks_the_clip() {
    let mut app = edited_app();
    on_changed_line(&mut app); // diff focus, content line → c comment · v select · …
    // Wide: every action fits, nothing is clipped.
    let wide = footer_line(&render_at(&app, 120));
    assert!(
        wide.contains("c comment") && wide.contains("v select") && !wide.contains('…'),
        "wide footer shows all actions, no clip marker:\n{wide}"
    );
    // Narrow: the primary survives, the least-relevant actions are trimmed, and `…` marks it.
    let narrow = footer_line(&render_at(&app, 18));
    assert!(narrow.contains("c comment"), "the primary action is never dropped:\n{narrow}");
    assert!(narrow.contains('…'), "the clip is marked with …:\n{narrow}");
    assert!(!narrow.contains("v select"), "the least-relevant action is trimmed:\n{narrow}");
}

#[test]
fn the_pr_footer_keeps_the_open_action_when_the_state_line_is_long() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{
        Check, CheckStatus, Merge, PrSnapshot, PrState, PrView, Provider, Sync,
    };
    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::Pr).unwrap();
    app.pr = PrView::Pr(Box::new(PrSnapshot {
        provider: Provider::Github,
        number: 226,
        title: "t".into(),
        url: "u".into(),
        state: PrState::Open,
        is_draft: false,
        head_ref: "feature".into(),
        head_is_fork: false,
        base_ref: "main".into(),
        diff_refs: herdr_reviewr::forge::DiffRefs::default(),
        merge: Merge::Conflicting, // a long state line: conflicts · behind · failing · +more
        sync: Sync::Behind(3),
        checks: vec![Check { name: "ci".into(), status: CheckStatus::Failure }],
        comments: vec![],
        truncated: true,
        threads_partial: None,
    }));
    // At narrow width the state line is capped so the primary `o open ↗` is never crowded off.
    let footer = footer_line(&render_at(&app, 60));
    assert!(footer.contains("o open"), "the open action survives a long state line:\n{footer}");
}

#[test]
fn reply_chains_render_explicit_partial_states_and_a_complete_one_shows_no_marker() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{
        Comment, CommentKind, Merge, PartialReason, PrSnapshot, PrState, PrView, Provider,
        RemoteReply, RepliesState, Sync,
    };
    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::Pr).unwrap();
    let finding = |state: RepliesState| Comment {
        kind: CommentKind::Finding,
        author: "rev".into(),
        author_is_bot: false,
        anchor: "x.rs:1".into(),
        body: "finding body".into(),
        snippet: None,
        created_at: "2026-07-01T00:00:00Z".into(),
        is_resolved: false,
        is_outdated: false,
        reply_count: 3,
        replies: vec![RemoteReply {
            id: "r1".into(),
            author: "alice".into(),
            body: "first reply".into(),
            created_at: "2026-07-01T01:00:00Z".into(),
        }],
        replies_state: state,
        diff_anchor: None,
        remote_id: None,
    };
    let snap = |comment: Comment, threads_partial: Option<PartialReason>| {
        PrView::Pr(Box::new(PrSnapshot {
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
            comments: vec![comment],
            truncated: false,
            threads_partial,
        }))
    };

    // A complete chain shows its replies and no marker — completeness is stated, not implied,
    // even while reply_count (which counts system notes on GitLab) disagrees.
    app.pr = snap(finding(RepliesState::Complete), None);
    let page = render(&app);
    assert!(page.contains("first reply"), "replies render inline:\n{page}");
    assert!(!page.contains("more replies"), "a complete chain has no marker:\n{page}");

    // A capped walk names the missing count and the reload key.
    app.pr =
        snap(finding(RepliesState::Partial { missing: 2, reason: PartialReason::Capped }), None);
    let page = render(&app);
    assert!(
        page.contains("+2 more replies not loaded — r to reload"),
        "a capped chain is explicit:\n{page}"
    );

    // A failed walk reads as a failure, not as a smaller conversation.
    app.pr = snap(
        finding(RepliesState::Partial {
            missing: 2,
            reason: PartialReason::PageFailed("HTTP 502".into()),
        }),
        None,
    );
    let page = render(&app);
    assert!(
        page.contains("⚠ 2 more replies failed to load — r to retry"),
        "a failed chain is explicit:\n{page}"
    );

    // An incomplete thread list marks the whole surface in the state line.
    app.pr =
        snap(finding(RepliesState::Complete), Some(PartialReason::PageFailed("HTTP 502".into())));
    let page = render_at(&app, 140);
    assert!(
        page.contains("⚠ some threads failed to load — r to retry"),
        "a failed thread walk is explicit:\n{page}"
    );
    app.pr = snap(finding(RepliesState::Complete), Some(PartialReason::Capped));
    let page = render_at(&app, 140);
    assert!(
        page.contains("thread list capped — more on GitHub ↗"),
        "a capped thread walk points at the forge:\n{page}"
    );
}

#[test]
fn the_footer_shows_tray_chips_and_the_pi_link_state() {
    let mut app = edited_app();
    app.collab_link = Some(true);
    app.collab_tray = vec!["C1".into(), "C2".into()];
    let footer = footer_line(&render(&app));
    assert!(footer.contains("C1 C2 ✦ pi"), "chips and a live link render:\n{footer}");

    // Follow-on says so, so the reviewer knows the pane will move with Pi.
    app.collab_follow = Some(true);
    let footer = footer_line(&render(&app));
    assert!(footer.contains("✦ pi following"), "an active follow is named:\n{footer}");

    // The manual-navigation grace shows as a countdown, so the pause is visibly temporary.
    app.collab_grace_ms = Some(1_500);
    let footer = footer_line(&render(&app));
    assert!(footer.contains("following ⏸ 1.5s"), "the grace counts down:\n{footer}");
    app.collab_grace_ms = Some(400);
    let footer = footer_line(&render(&app));
    assert!(footer.contains("following ⏸ 0.4s"), "the remainder shrinks:\n{footer}");
    app.collab_grace_ms = None;
    let footer = footer_line(&render(&app));
    assert!(!footer.contains('⏸'), "no grace, no timer:\n{footer}");

    // Follow-off keeps showing Pi's position; history browsing shows its cursor.
    app.collab_follow = Some(false);
    app.collab_pi_location = Some("src/a.rs:12".into());
    app.collab_history = Some((2, 5));
    let footer = footer_line(&render(&app));
    assert!(
        footer.contains("⊘follow @ src/a.rs:12") && footer.contains("⟲ 2/5"),
        "aware without being moved:\n{footer}"
    );

    app.collab_link = Some(false);
    app.collab_follow = None;
    app.collab_pi_location = None;
    app.collab_history = None;
    app.collab_tray.clear();
    let footer = footer_line(&render(&app));
    assert!(footer.contains("✧ pi offline"), "a downed link is visible, not absent:\n{footer}");

    app.collab_link = None;
    let footer = footer_line(&render(&app));
    assert!(!footer.contains("pi"), "no collaboration host, no segment:\n{footer}");
}

#[test]
fn an_agent_staged_finding_renders_as_a_pi_draft_card() {
    use herdr_reviewr::collab::protocol::{DraftAnchor, StagedDraft};
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.collab_stage_draft(&StagedDraft {
        draft: "d1".into(),
        body: "consider naming".into(),
        anchor: Some(DraftAnchor { path: "a.rs".into(), line: 2, start_line: None }),
        reply_to: None,
    })
    .unwrap();

    // A worktree-anchored finding is a content comment: it lives on `All files`, whose
    // coordinates are the file's own lines rather than diff rows.
    app.set_tab(herdr_reviewr::app::Tab::AllFiles).unwrap();
    let out = render(&app);
    assert!(out.contains("pi draft · a.rs:2"), "the card names its agent author:\n{out}");
    assert!(out.contains("consider naming"), "the body renders:\n{out}");
}

#[test]
fn pr_header_names_the_resolved_branch_and_marks_a_fork() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::{Merge, PrSnapshot, PrState, PrView, Provider, Sync};
    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::Pr).unwrap();
    let snap = |fork: bool| {
        PrView::Pr(Box::new(PrSnapshot {
            provider: Provider::Github,
            number: 226,
            title: "t".into(),
            url: "u".into(),
            state: PrState::Open,
            is_draft: false,
            head_ref: "persiyanov/feature".into(),
            head_is_fork: fork,
            base_ref: "main".into(),
            diff_refs: herdr_reviewr::forge::DiffRefs::default(),
            merge: Merge::Clean,
            sync: Sync::InSync,
            checks: vec![],
            comments: vec![],
            truncated: false,
            threads_partial: None,
        }))
    };
    // The header shows the branch that resolved — it can differ from the local branch —
    // and marks a fork head, so a same-named fork PR is visible (specs/forge-host.md).
    app.pr = snap(false);
    let header = render(&app).lines().next().unwrap().to_string();
    assert!(header.contains("persiyanov/feature"), "resolved branch in the header:\n{header}");
    assert!(!header.contains('⑂'), "no fork marker on a same-repo head:\n{header}");
    app.pr = snap(true);
    let header = render(&app).lines().next().unwrap().to_string();
    assert!(header.contains("⑂ persiyanov/feature"), "fork head is marked:\n{header}");
    // Narrow bars drop the branch first; the chip's number stays.
    app.pr = snap(false);
    let narrow = render_at(&app, 44).lines().next().unwrap().to_string();
    assert!(!narrow.contains("persiyanov/feature"), "branch drops when narrow:\n{narrow}");
    assert!(narrow.contains("#226"), "the chip survives a narrow bar:\n{narrow}");
}

#[test]
fn pr_empty_states_name_candidates_detachment_and_the_ambiguity_count() {
    use herdr_reviewr::app::Tab;
    use herdr_reviewr::forge::PrView;
    let r = Repo::init();
    r.write("x.rs", "y\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::Pr).unwrap();
    // The empty state names what was queried, so resolution is inspectable, never silent.
    let names = ["alpha", "beta", "gamma", "delta", "epsilon"];
    app.pr = PrView::NoPr(names.iter().map(|s| (*s).to_string()).collect());
    let out = render(&app);
    assert!(
        out.contains("no PR for alpha, beta, gamma +2 more yet"),
        "candidates are named, capped at three:\n{out}"
    );
    app.pr = PrView::NoPr(Vec::new());
    let out = render(&app);
    assert!(out.contains("detached HEAD"), "detached wording for no candidates:\n{out}");
    app.pr = PrView::Ambiguous(3);
    let out = render(&app);
    assert!(out.contains("3 open PRs"), "the ambiguity count shows:\n{out}");
}

#[test]
fn the_footer_keeps_its_actions_alongside_a_status() {
    let mut app = edited_app();
    on_changed_line(&mut app);
    app.status = "comment added".to_string();
    let footer = footer_line(&render(&app));
    // A status sits among the actions, never replacing them.
    assert!(footer.contains("comment added"), "the status shows:\n{footer}");
    assert!(
        footer.contains("c comment"),
        "the primary action persists alongside a status:\n{footer}"
    );
}

#[test]
fn empty_repo_shows_empty_states() {
    let r = Repo::init();
    r.write("seed.rs", "x\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    let out = render(&app);
    assert!(out.contains("no changes"), "empty file list state");
}

#[test]
fn composing_renders_the_inline_multiline_box() {
    let mut app = edited_app();
    app.focus = Focus::Diff;
    app.diff_cursor = app.diff.rows.iter().position(|r| r.marker() == '+').unwrap();
    app.start_comment();
    for ch in "line one".chars() {
        app.input_push(ch);
    }
    app.input_push('\n');
    for ch in "line two".chars() {
        app.input_push(ch);
    }

    let out = render(&app);
    assert!(out.contains("comment ·"), "box titled with the location");
    assert!(out.contains("line one"), "first input line shown");
    assert!(out.contains("line two"), "second input line shown — the box is multi-line");
}

#[test]
fn the_box_grows_with_multiline_input_and_keeps_the_anchor_visible() {
    let r = Repo::init();
    r.write("mid.rs", "a\nb\nc\nd\ne\n");
    r.commit_all("init");
    r.write("mid.rs", "a\nB\nc\nd\ne\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor =
        app.diff.rows.iter().position(|r| r.marker() == '+' && r.text().contains('B')).unwrap();
    app.start_comment();
    for ch in "one\ntwo\nthree".chars() {
        app.input_push(ch);
    }

    let out = render(&app);
    assert!(out.contains("one") && out.contains("two") && out.contains("three"), "all box lines");
    let lines: Vec<&str> = out.lines().collect();
    // The inserted line is the only one carrying an uppercase `B` (no `+` glyph now).
    let anchor = lines.iter().position(|l| l.contains('B')).expect("anchor line visible");
    let box_row = lines.iter().position(|l| l.contains("comment ·")).expect("box");
    assert!(anchor < box_row, "the commented line stays above the box as it grows");
}

#[test]
fn the_box_is_inserted_under_the_selected_line() {
    let r = Repo::init();
    r.write("mid.rs", "alpha\nbeta\ngamma\n");
    r.commit_all("init");
    r.write("mid.rs", "alpha\nBETA\ngamma\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = app.diff.rows.iter().position(|r| r.text().contains("BETA")).unwrap();
    app.start_comment();
    for ch in "note".chars() {
        app.input_push(ch);
    }

    let out = render(&app);
    let lines: Vec<&str> = out.lines().collect();
    let box_row = lines.iter().position(|l| l.contains("comment ·")).expect("box rendered");
    let below_row = lines.iter().position(|l| l.contains("gamma")).expect("context below shown");
    assert!(below_row > box_row, "the diff line below the selection is pushed under the box");
}

const AREA: Rect = Rect { x: 0, y: 0, width: 140, height: 40 };

#[test]
fn pr_picker_renders_its_fuzzy_query_and_only_matching_rows() {
    use herdr_reviewr::app::{PrPicker, Tab};
    use herdr_reviewr::forge::{PrListItem, PrListing, PrState};

    let mut app = edited_app();
    app.set_tab(Tab::Pr).unwrap();
    let item = |number, title: &str| PrListItem {
        number,
        title: title.into(),
        head_ref: "feature/search".into(),
        author: "alice".into(),
        is_draft: false,
        state: PrState::Open,
        ci: None,
        created_at: String::new(),
        comments: 0,
        threads_open: None,
        threads_resolved: None,
    };
    app.pr_picker_query = "needle".into();
    app.pr_picker = Some(PrPicker::Loaded {
        listing: PrListing {
            open: vec![item(1, "other"), item(2, "needle result")],
            done: Vec::new(),
        },
        filtered: vec![1],
        cursor: 0,
    });

    let out = render(&app);
    assert!(out.contains("> needle█"), "the live query is visible: {out}");
    assert!(out.contains("needle result"), "the matching MR remains visible");
    assert!(!out.contains("  other"), "non-matches leave the rendered list");
    assert!(out.contains("1/2"), "the title reports matching and total counts");
}

#[test]
fn remote_review_changes_renders_identity_loading_and_read_only_actions() {
    use herdr_reviewr::app::RemoteChanges;
    use herdr_reviewr::forge::{Provider, ReviewDiffRequest};
    use herdr_reviewr::git::RepoTarget;

    let mut app = edited_app();
    app.remote_changes = RemoteChanges::Loading(ReviewDiffRequest::new(
        RepoTarget {
            provider: Provider::Gitlab,
            host: "gitlab.example.com".into(),
            owner: "group".into(),
            name: "project".into(),
        },
        42,
    ));
    app.reload().unwrap();
    let out = render(&app);
    assert!(out.contains("[MR !42]"));
    assert!(out.contains("loading remote diff"));
    assert!(out.contains("esc unpin"));
    assert!(!out.contains("c comment"));
}

#[test]
fn header_clicks_map_to_scope_and_send() {
    let app = edited_app(); // scope uncommitted, no comments
    // Scan the header row instead of hardcoding columns, so the test survives changes
    // to the label/button text.
    let scope: Vec<u16> = (0..AREA.width)
        .filter(|&c| ui::hit_header(AREA, &app, c, 0) == Some(HeaderHit::Scope))
        .collect();
    let send: Vec<u16> = (0..AREA.width)
        .filter(|&c| ui::hit_header(AREA, &app, c, 0) == Some(HeaderHit::Send))
        .collect();

    assert!(!scope.is_empty(), "scope chip is clickable");
    assert!(!send.is_empty(), "send button is clickable");
    assert!(scope.iter().max() < send.iter().min(), "scope is left of the button, no overlap");
    assert!(*send.iter().max().unwrap() < AREA.width);

    let gap = scope.iter().max().unwrap() + 1;
    assert_eq!(ui::hit_header(AREA, &app, gap, 0), None, "the space between controls is inert");
    assert_eq!(ui::hit_header(AREA, &app, scope[0], 5), None, "only row 0 is the header");
}

#[test]
fn file_and_diff_clicks_map_to_row_indices() {
    let app = edited_app();
    // Right pane: the first file row maps to index 0; clicking past the list misses.
    assert_eq!(ui::hit_file(AREA, app.list_pct, 120, 2, app.file_rows.len(), 0), Some(0));
    assert_eq!(ui::hit_file(AREA, app.list_pct, 120, 9, app.file_rows.len(), 0), None);
    // With the list scrolled down, the top visible row maps to that scrolled-to index.
    assert_eq!(ui::hit_file(AREA, app.list_pct, 120, 2, 50, 7), Some(7));
    assert_eq!(ui::hit_file(AREA, app.list_pct, 120, 3, 50, 7), Some(8));
    // The wheel routes by pointer: a column in the right pane is "in" the file list,
    // one in the left (diff) pane is not.
    assert!(ui::in_files_pane(AREA, app.list_pct, 120, 3));
    assert!(!ui::in_files_pane(AREA, app.list_pct, 10, 3));
    // Left pane: diff rows map top-down to diff-line indices.
    assert!(app.visible.len() > 1);
    let heights = ui::diff_row_heights(&app, AREA);
    assert_eq!(ui::hit_diff(AREA, app.list_pct, 10, 2, &heights, 0), Some(0));
    assert_eq!(ui::hit_diff(AREA, app.list_pct, 10, 3, &heights, 0), Some(1));
    // With a nonzero scroll and wrapped (multi-row) lines, the click must skip the
    // scrolled-off rows and account for each visible row's display height. Rows are
    // 2 tall each; diff_scroll=1 puts row index 1 at the top of the pane (inner.y == 2).
    let tall = [2usize, 2, 2, 2];
    assert_eq!(ui::hit_diff(AREA, app.list_pct, 10, 2, &tall, 1), Some(1)); // top visible row
    assert_eq!(ui::hit_diff(AREA, app.list_pct, 10, 3, &tall, 1), Some(1)); // its second display row
    assert_eq!(ui::hit_diff(AREA, app.list_pct, 10, 4, &tall, 1), Some(2)); // next logical row
}

#[test]
fn a_binary_file_shows_the_no_line_comments_message() {
    let r = Repo::init();
    r.write("logo.bin", "\0\0\0\0seed\0\0");
    r.commit_all("init");
    r.write("logo.bin", "\0\0\0\0changed\0\0\0");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    let idx = app.entries.iter().position(|f| f.path == "logo.bin").expect("binary file listed");
    app.select_file(idx).unwrap();

    let out = render(&app);
    assert!(out.contains("binary — no line comments"), "binary diff message shown:\n{out}");
}

#[test]
fn the_comments_list_flags_a_stale_comment() {
    let r = Repo::init();
    r.write("a.rs", "alpha\nbeta\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nBETA\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.focus = Focus::Diff;
    app.diff_cursor = app.diff.rows.iter().position(|r| r.marker() == '+').unwrap();
    app.start_comment();
    for ch in "look here".chars() {
        app.input_push(ch);
    }
    app.submit_comment();

    // a.rs reverts to its committed state → leaves the changeset → the comment is stale.
    r.write("a.rs", "alpha\nbeta\n");
    app.reload().unwrap();
    app.open_list();

    let out = render(&app);
    assert!(out.contains("(stale)"), "stale comment flagged in the list:\n{out}");
}

#[test]
fn open_list_renders_the_comments_overlay() {
    let mut app = edited_app();
    app.focus = Focus::Diff;
    app.diff_cursor = app.diff.rows.iter().position(|r| r.marker() == '+').unwrap();
    app.start_comment();
    for ch in "overlay note".chars() {
        app.input_push(ch);
    }
    app.submit_comment();
    app.open_list();

    let out = render(&app);
    assert!(out.contains("Comments ("), "overlay titled with a count");
    assert!(out.contains("overlay note"), "comment text listed");
}

#[test]
fn a_long_comments_list_keeps_the_cursor_row_visible() {
    let mut app = edited_app();
    // Anchored to a file outside the diff so no inline card paints the text behind the popup.
    for i in 0..60 {
        app.store.add(Comment {
            file: "elsewhere.rs".into(),
            side: Side::New,
            start: i + 1,
            end: i + 1,
            lines: "+x".into(),
            text: format!("note {i}"),
            diff_anchored: true,
        });
    }
    app.open_list();
    app.list_cursor = 59;

    let out = render(&app);
    assert!(out.contains("note 59"), "the list windows on the cursor:\n{out}");
    assert!(!out.contains("note 0 "), "rows above the window scroll away:\n{out}");
}

#[test]
fn last_turn_without_a_baseline_renders_the_waiting_state() {
    let r = Repo::init();
    r.write("a.rs", "a\n");
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::LastTurn, None);
    app.reload().unwrap();
    let out = render(&app);
    assert!(out.contains("[last turn]"), "the scope chip reads last turn");
    assert!(out.contains("waiting for the agent's next turn"), "the cold-start empty state shows");
}

#[test]
fn all_files_tab_bar_footer_and_count_read_for_the_tab() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("a.rs", "one\n");
    r.commit_all("init");
    r.write("a.rs", "ONE\n"); // one change
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::AllFiles).unwrap();

    let out = render(&app);
    assert!(out.contains("1 Changes"), "tab labels carry their switch digit:\n{out}");
    assert!(out.contains("2 All files"));
    assert!(
        out.contains("1 changed"),
        "the changed count stays in the header on All files:\n{out}"
    );
    let footer = footer_line(&out);
    assert!(footer.contains("scope"), "the footer shows context actions on All files:\n{footer}");
    assert!(
        !footer.contains("changed"),
        "the changed count is not repeated in the footer:\n{footer}"
    );
}

#[test]
fn a_narrow_overflowing_header_does_not_mis_map_a_click_to_send() {
    let r = Repo::init();
    r.write("a.rs", "x\n");
    r.commit_all("init");
    r.write("a.rs", "y\n");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();

    // At a narrow sidebar width the two-tab header overflows and the Send button is off-screen.
    // No on-screen column may map to Send — the old right-aligned hit-zone landed a phantom Send
    // over the chip/tab region, swallowing those clicks as a Send.
    let width: u16 = 34;
    let area = Rect::new(0, 0, width, 40);
    let phantom = (0..width).any(|c| ui::hit_header(area, &app, c, 0) == Some(HeaderHit::Send));
    assert!(!phantom, "no on-screen column mis-maps to Send when the narrow header overflows");

    // At a wide width the Send button is right-aligned and clickable.
    let wide = Rect::new(0, 0, 140, 40);
    let send = (0..140).any(|c| ui::hit_header(wide, &app, c, 0) == Some(HeaderHit::Send));
    assert!(send, "Send is clickable when the header fits");
}

#[test]
fn all_files_empty_pane_reads_select_a_file() {
    use herdr_reviewr::app::Tab;
    let r = Repo::init();
    r.write("src/a.rs", "x\n");
    r.write("src/b.rs", "y\n"); // two children so src/ is a real collapsed dir, not a folded file
    r.commit_all("init");
    let mut app = App::new(r.path_buf(), Scope::Uncommitted, None);
    app.reload().unwrap();
    app.set_tab(Tab::AllFiles).unwrap(); // clean repo: no seed; cursor rests on collapsed src/

    let out = render(&app);
    assert!(out.contains("select a file to read"), "the empty All files left pane copy:\n{out}");
    assert!(!out.contains("no diff"), "no diff vocabulary in the content browser:\n{out}");
}

#[test]
fn renders_a_light_theme_without_panic() {
    let mut app = edited_app();
    app.set_cli_theme(Some("catppuccin-latte".to_string()));
    // Driving the full render path with a derived light palette must not panic, and a Latte
    // color (the focused pane's lavender border) reaches the painted buffer.
    let buf = render_buffer(&app);
    let latte_lavender = herdr_reviewr::theme::resolve(Some("catppuccin-latte")).palette.lavender;
    let painted = (0..40)
        .flat_map(|y| (0..140).map(move |x| (x, y)))
        .any(|(x, y)| buf.cell((x, y)).is_some_and(|c| c.fg == latte_lavender));
    assert!(painted, "the Latte palette reaches the painted buffer");
}
