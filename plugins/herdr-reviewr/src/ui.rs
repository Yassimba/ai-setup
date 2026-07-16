//! Rendering the Changes view: tab bar, file list, diff, comment box, list, status.
//!
//! See `specs/tui.md`. The layout is a header tab bar, a body split into the diff
//! (left) and the file list (right), and a status bar. While composing, the comment
//! box is spliced inline into the diff under the selected line; the comments-list
//! overlay is drawn on top when open. Rendering reads `App` only; all state changes
//! live in `app.rs`.

use std::rc::Rc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, FooterAction, Mode, Tab, Tier};
use crate::diff::{FileDiff, FileState, LineDecoration, Row};
use crate::file_list::{Annotation, RowKind};
use crate::forge;
use crate::model::Comment;
use crate::theme::Palette;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if let Some(error) = app.config_error() {
        frame.render_widget(
            Paragraph::new(error).wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
        return;
    }
    let p = panes(area, app.list_pct);

    if app.tab == Tab::Pr {
        render_pr_header(frame, app, p.tab);
        render_pr_read(frame, app, p.diff);
        render_pr_nav(frame, app, p.files);
    } else {
        render_tab_bar(frame, app, p.tab);
        render_diff_view(frame, app, p.diff);
        render_file_list(frame, app, p.files);
    }
    if app.tab == Tab::Pr && app.composing() {
        let height = composer_height(app, p.diff.width as usize).min(p.diff.height as usize) as u16;
        let area =
            Rect::new(p.diff.x, p.diff.bottom().saturating_sub(height), p.diff.width, height);
        frame.render_widget(Clear, area);
        render_composer(frame, app, area);
    }
    // One footer band on every tab, drawn after the per-tab base so it sits on both layouts;
    // then the comments-list modal on top when it is open.
    render_footer(frame, app, p.status);

    if app.mode == Mode::List {
        render_comments_list(frame, app, area);
    }
    if app.tab == Tab::Pr && app.pr_picker.is_some() {
        render_pr_picker(frame, app, area);
    }
    if app.switcher.is_some() {
        render_project_switcher(frame, app, area);
    }
}

/// The vertical bands: tab bar, body, footer. The comment input is inline in the diff, not a
/// band of its own. The footer action bar is one row — it fits by dropping the least-relevant
/// actions, not by wrapping.
fn vrows(area: Rect) -> Rc<[Rect]> {
    Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).split(area)
}

/// The frame's layout rects: the diff pane, the file pane, and the whole body band. One
/// place computes the vertical bands and the horizontal split, so every geometry helper and
/// the renderer agree by construction (a layout change can't desync hit-testing from paint).
struct Panes {
    tab: Rect,
    diff: Rect,
    files: Rect,
    body: Rect,
    status: Rect,
}

fn panes(area: Rect, list_pct: u16) -> Panes {
    let rows = vrows(area);
    let body = rows[1];
    let split = Layout::horizontal([
        Constraint::Percentage(100 - list_pct),
        Constraint::Percentage(list_pct),
    ])
    .split(body);
    Panes { tab: rows[0], diff: split[0], files: split[1], body, status: rows[2] }
}

/// The whole body band (between the tab bar and status bar), for divider hit-testing.
#[must_use]
pub fn body_rect(area: Rect) -> Rect {
    vrows(area)[1]
}

/// Whether `(col, row)` lands on the draggable divider between the two panes.
#[must_use]
pub fn hit_divider(area: Rect, list_pct: u16, col: u16, row: u16) -> bool {
    let p = panes(area, list_pct);
    let in_body = row >= p.body.y && row < p.body.y + p.body.height;
    // A 3-column grab zone straddling the abutting pane borders.
    in_body && col + 1 >= p.files.x && col <= p.files.x + 1
}

/// The file-row index a click at `(col, row)` lands on, or `None` if outside the list.
/// `file_scroll` is the top visible row, so a click maps to the scrolled-to row.
#[must_use]
pub fn hit_file(
    area: Rect,
    list_pct: u16,
    col: u16,
    row: u16,
    n_files: usize,
    file_scroll: usize,
) -> Option<usize> {
    let inner = inner_rect(panes(area, list_pct).files);
    if !contains(inner, col, row) {
        return None;
    }
    let idx = (row - inner.y) as usize + file_scroll;
    (idx < n_files).then_some(idx)
}

/// The number of file rows visible in the file pane, used to clamp the file-list scroll.
#[must_use]
pub fn file_viewport_height(area: Rect, list_pct: u16) -> usize {
    inner_rect(panes(area, list_pct).files).height as usize
}

/// Whether `(col, row)` falls in the file pane, so the wheel scrolls the list it is over.
#[must_use]
pub fn in_files_pane(area: Rect, list_pct: u16, col: u16, row: u16) -> bool {
    contains(panes(area, list_pct).files, col, row)
}

/// The logical diff-row index a click at `(col, row)` lands on, or `None` if outside the
/// diff pane. `heights` (display rows per logical row) and `diff_scroll` reproduce the
/// painted window, so a click on any display line of a wrapped row maps to that row.
#[must_use]
pub fn hit_diff(
    area: Rect,
    list_pct: u16,
    col: u16,
    row: u16,
    heights: &[usize],
    diff_scroll: usize,
) -> Option<usize> {
    let inner = inner_rect(panes(area, list_pct).diff);
    if !contains(inner, col, row) {
        return None;
    }
    let target = (row - inner.y) as usize;
    let mut acc = 0;
    for (li, h) in heights.iter().enumerate().skip(diff_scroll) {
        acc += h;
        if target < acc {
            return Some(li);
        }
    }
    None
}

/// The number of diff rows visible in the diff pane, used to clamp the scroll.
#[must_use]
pub fn diff_viewport_height(area: Rect, list_pct: u16) -> usize {
    inner_rect(panes(area, list_pct).diff).height as usize
}

/// The display height (rows on screen) of each visible logical diff row, honoring wrap.
#[must_use]
pub fn diff_row_heights(app: &App, area: Rect) -> Vec<usize> {
    let width = inner_rect(panes(area, app.list_pct).diff).width as usize;
    let gutter_w = gutter_for(&app.diff);
    let p = app.palette();
    // A row's display height is its wrapped code lines plus any inline comment cards under
    // it (excluding a card whose comment is being edited), so scroll-clamping and hit-testing
    // match what the renderer paints.
    let cards = app.comment_cards();
    let remote_cards = app.remote_comment_cards();
    let draft_cards = app.remote_draft_cards();
    let editing = editing_comment(app);
    app.visible
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let base = row_height(r, gutter_w, width, app.wrap);
            let card: usize = cards[i]
                .iter()
                .filter(|&&ci| Some(ci) != editing)
                .filter_map(|&ci| app.store.get(ci).map(|c| (ci, c)))
                .map(|(ci, c)| comment_card_lines(c, app.collab_owned_comment(ci), width, p).len())
                .sum();
            let remote = app.pr_snapshot().map_or(0, |snapshot| {
                remote_cards[i]
                    .iter()
                    .filter_map(|&ci| snapshot.comments.get(ci))
                    .map(|comment| remote_comment_card_lines(comment, width, p).len())
                    .sum()
            });
            let drafts: usize = draft_cards[i]
                .iter()
                .filter_map(|&index| app.remote_drafts.get(index))
                .map(|draft| remote_draft_card_lines(draft, width, p).len())
                .sum();
            base + card + remote + drafts
        })
        .collect()
}

/// The store index of the comment currently being edited, whose inline card is hidden in
/// favor of its edit box; `None` when not editing.
fn editing_comment(app: &App) -> Option<usize> {
    match app.mode {
        Mode::Composing { editing } => editing,
        _ => None,
    }
}

/// Rows the inline comment box occupies at the diff pane's `width`: the wrapped body height
/// (so the box grows as text wraps, not only on explicit newlines) plus the two borders.
#[must_use]
pub fn composer_height(app: &App, width: usize) -> usize {
    box_rows(&app.input, composer_content_width(width)).len() + 2
}

/// The text width inside the comment box: the diff pane width minus its two borders.
#[must_use]
pub fn composer_content_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

/// The diff pane's inner content width for the full terminal `area`, so the event loop can
/// reserve the comment box without a `Frame` (mirrors [`diff_viewport_height`]).
#[must_use]
pub fn diff_inner_width(area: Rect, list_pct: u16) -> usize {
    inner_rect(panes(area, list_pct).diff).width as usize
}

/// The comment box's display lines at `content_w`: each input line word-wrapped, with the
/// caret drawn as a block at its mapped (row, column). An empty box shows a placeholder.
fn composer_lines(app: &App, content_w: usize) -> Vec<Line<'static>> {
    let p = app.palette();
    if app.input.is_empty() {
        return vec![Line::from(vec![
            Span::styled(" ", caret_style(p)),
            Span::styled("Leave a comment…", Style::default().fg(p.overlay0)),
        ])];
    }
    let rows = box_rows(&app.input, content_w);
    let (caret_row, caret_col) = caret_rowcol(&rows, app.caret);
    rows.iter()
        .enumerate()
        .map(|(i, (_, text))| {
            if i == caret_row {
                row_with_caret(text, caret_col, p)
            } else {
                Line::from(text.clone())
            }
        })
        .collect()
}

/// The block-cursor style: the character under the caret shown dark-on-peach.
fn caret_style(p: &Palette) -> Style {
    Style::default().fg(p.surface0).bg(p.peach)
}

/// One box row with the caret block over the character at `col` (a trailing block at the end).
fn row_with_caret(text: &str, col: usize, p: &Palette) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let left: String = chars[..col].iter().collect();
    let mut spans = vec![Span::raw(left)];
    if col < chars.len() {
        spans.push(Span::styled(chars[col].to_string(), caret_style(p)));
        spans.push(Span::raw(chars[col + 1..].iter().collect::<String>()));
    } else {
        spans.push(Span::styled(" ".to_string(), caret_style(p)));
    }
    Line::from(spans)
}

/// Wrap one logical line's `chars` to `width` display columns, returning contiguous half-open
/// char ranges (every char is in exactly one row, so a char index maps cleanly to a row). A
/// greedy word wrap that keeps the break space on its row; an over-wide word hard-breaks.
fn box_wrap(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let w = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut col, mut i, mut last_space) = (0usize, start, None);
        while i < chars.len() {
            let cw = UnicodeWidthChar::width(chars[i]).unwrap_or(0);
            if col + cw > w && i > start {
                break;
            }
            col += cw;
            if chars[i] == ' ' {
                last_space = Some(i);
            }
            i += 1;
        }
        // Break after the last space that fits (keeping it on this row), else hard-break.
        let end = if i < chars.len() {
            last_space.filter(|&s| s + 1 > start).map_or(i, |s| s + 1)
        } else {
            i
        };
        rows.push((start, end));
        start = end;
    }
    rows
}

/// The box's visual rows over the whole `input`: `(start_char_index, text)` per row, wrapping
/// each logical line (split on `\n`) with [`box_wrap`]. A trailing newline yields an empty row.
fn box_rows(input: &str, width: usize) -> Vec<(usize, String)> {
    let chars: Vec<char> = input.chars().collect();
    let mut rows = Vec::new();
    let mut i = 0;
    loop {
        let line_end = chars[i..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| i + p);
        for (a, b) in box_wrap(&chars[i..line_end], width) {
            rows.push((i + a, chars[i + a..i + b].iter().collect::<String>()));
        }
        match chars[line_end..].first() {
            Some('\n') => {
                i = line_end + 1;
                if i == chars.len() {
                    rows.push((i, String::new())); // a trailing newline opens an empty row
                    break;
                }
            }
            _ => break,
        }
    }
    if rows.is_empty() {
        rows.push((0, String::new()));
    }
    rows
}

/// Map a caret char index to its `(row, col)` in the box rows: the last row that starts at or
/// before the caret, with the column clamped to that row's length.
fn caret_rowcol(rows: &[(usize, String)], caret: usize) -> (usize, usize) {
    let row = rows.iter().rposition(|(start, _)| *start <= caret).unwrap_or(0);
    let (start, text) = &rows[row];
    (row, (caret - start).min(text.chars().count()))
}

/// The new caret char index after moving up (`down == false`) or down one wrapped row, keeping
/// the column where the target row allows. For `↑`/`↓` in the comment editor.
#[must_use]
pub fn caret_vertical(input: &str, caret: usize, content_w: usize, down: bool) -> usize {
    let rows = box_rows(input, content_w);
    let (row, col) = caret_rowcol(&rows, caret);
    let target = if down { (row + 1).min(rows.len() - 1) } else { row.saturating_sub(1) };
    let (start, text) = &rows[target];
    start + col.min(text.chars().count())
}

/// Word-wrap a plain string to `width` columns, reusing the diff's [`wrap_segments`] so the
/// break rule (last space, hard-break an over-wide word, width-aware) is identical.
fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let cells: Vec<Cell> = s
        .chars()
        .map(|ch| Cell {
            ch,
            w: UnicodeWidthChar::width(ch).unwrap_or(0),
            fg: Color::Reset,
            emph: false,
        })
        .collect();
    wrap_segments(&cells, width)
        .into_iter()
        .map(|(a, b)| cells[a..b].iter().map(|c| c.ch).collect())
        .collect()
}

/// A clickable region in the header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeaderHit {
    Tab(Tab),
    Scope,
    Send,
}

/// Which header control a click at `(col, row)` lands on, if any.
#[must_use]
pub fn hit_header(area: Rect, app: &App, col: u16, row: u16) -> Option<HeaderHit> {
    if row != area.y {
        return None;
    }
    for (tab, start, end) in tab_spans() {
        if (start as u16..end as u16).contains(&col) {
            return Some(HeaderHit::Tab(tab));
        }
    }
    let scope_start = header_prefix_len() as u16;
    let scope_end = scope_start + scope_chip(app).len() as u16;
    let button_start = send_button_col(app, area.width as usize) as u16;
    if (scope_start..scope_end).contains(&col) {
        (!(app.tab == Tab::Changes && app.remote_changes_active())).then_some(HeaderHit::Scope)
    } else if col >= button_start && col < area.width {
        Some(HeaderHit::Send)
    } else {
        None
    }
}

/// The two tabs and their labels, left to right. All-ASCII labels keep the byte length equal
/// to the display width, so the header column math stays simple.
const TABS: [(Tab, &str); 3] =
    [(Tab::Changes, "1 Changes"), (Tab::AllFiles, "2 All files"), (Tab::Pr, "3 PR")];
const HEADER_LEAD: &str = " ";
const TAB_GAP: &str = "  ";
const HEADER_GAP: &str = "  ";

/// Each tab's `(tab, start_col, end_col)` in the header, the single source the bar paints and
/// the click hit-tests against.
fn tab_spans() -> Vec<(Tab, usize, usize)> {
    let mut col = HEADER_LEAD.len();
    let mut out = Vec::new();
    for (i, (tab, label)) in TABS.iter().enumerate() {
        if i > 0 {
            col += TAB_GAP.len();
        }
        out.push((*tab, col, col + label.len()));
        col += label.len();
    }
    out
}

/// The column where the scope chip starts: past the tab bar and its trailing gap.
fn header_prefix_len() -> usize {
    tab_spans().last().map_or(HEADER_LEAD.len(), |&(_, _, end)| end) + HEADER_GAP.len()
}

fn scope_chip(app: &App) -> String {
    let label = if app.tab == Tab::Changes {
        app.remote_changes_label().unwrap_or_else(|| app.scope.label().to_string())
    } else {
        app.scope.label().to_string()
    };
    format!("[{label}]")
}

fn send_button(app: &App) -> String {
    format!("[ Send ({}) ]", app.store.len())
}

/// The header suffix: the active scope's changed-file count. Shared so the painter and the
/// hit-test place the right-aligned `Send` button at the same column.
fn header_suffix(app: &App) -> String {
    let more = if app.tab == Tab::Changes && app.remote_changes_truncated() { "+" } else { "" };
    format!("  {}{more} changed", app.changed_count())
}

/// The column the `Send` button paints at, matching `render_tab_bar`'s layout: right-aligned
/// when the header fits, packed left right after the suffix when the bar overflows (`pad`
/// collapses to 0). `hit_header` must use this, not a bare right-alignment, or a `Send` click
/// mis-fires (and on a narrow sidebar lands in a tab span) when the header overflows.
fn send_button_col(app: &App, width: usize) -> usize {
    let before = header_prefix_len() + scope_chip(app).len() + header_suffix(app).len();
    before + width.saturating_sub(before + send_button(app).len())
}

/// The header's shared left side, painted by both tab bars: the lead pad, the three tab labels
/// (the active one bright + underlined, the inactive ones at `SUBTEXT0`), and the trailing gap
/// before each header's own suffix. One source so the two headers can't drift.
fn tab_bar_spans(app: &App) -> Vec<Span<'static>> {
    let p = app.palette();
    let bar = Style::default().bg(p.surface0);
    let mut spans = vec![Span::styled(HEADER_LEAD, bar)];
    for (i, (tab, label)) in TABS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(TAB_GAP, bar));
        }
        let style = if *tab == app.tab {
            bar.fg(p.lavender).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            bar.fg(p.subtext0)
        };
        spans.push(Span::styled(*label, style));
    }
    spans.push(Span::styled(HEADER_GAP, bar));
    spans
}

fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let chip = scope_chip(app);
    let suffix = header_suffix(app);
    let button = send_button(app);
    let used = header_prefix_len() + chip.len() + suffix.len() + button.len();
    let pad = (area.width as usize).saturating_sub(used);

    // A quiet surface bar: the active tab in bright lavender, the inactive one dimmed, the
    // clickable scope and Send controls accented so they read as buttons.
    let p = app.palette();
    let bar = Style::default().bg(p.surface0);
    let mut spans = tab_bar_spans(app);
    spans.push(Span::styled(chip, bar.fg(p.yellow).add_modifier(Modifier::BOLD)));
    spans.push(Span::styled(suffix, bar.fg(p.overlay0)));

    let send_fg = if app.store.is_empty() { p.overlay0 } else { p.green };
    spans.push(Span::styled(" ".repeat(pad), bar));
    spans.push(Span::styled(button, bar.fg(send_fg).add_modifier(Modifier::BOLD)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_file_list(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let block = bordered("Files", app.focus == Focus::Files, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.file_rows.is_empty() {
        let msg = match app.tab {
            Tab::AllFiles => "no files",
            Tab::Changes if app.remote_changes_active() => {
                app.remote_changes_notice().unwrap_or("no remote changes")
            }
            Tab::Changes if app.awaiting_turn() => "waiting for the agent's next turn",
            _ => "no changes",
        };
        frame.render_widget(dim_paragraph(msg, p), inner);
        return;
    }

    let width = inner.width as usize;
    // Window the rows to the scrolled-to viewport; `file_scroll` keeps the cursor on screen.
    let items: Vec<ListItem> = app
        .file_rows
        .iter()
        .enumerate()
        .skip(app.file_scroll)
        .take(inner.height as usize)
        .map(|(i, row)| {
            // The selected row fills with the cursor color, dimmed when the list is unfocused.
            let fill = (i == app.file_cursor).then(|| p.cursor_bg(app.focus == Focus::Files));
            let indent = "  ".repeat(row.depth);
            let badges = app.tree_badges(row);
            match &row.kind {
                RowKind::Dir { expanded, .. } => {
                    let arrow = if *expanded { "▾ " } else { "▸ " };
                    let badge_width = usize::from(badges.changed) * " ±".width()
                        + usize::from(badges.commented) * " ◆".width();
                    let fixed = indent.width() + arrow.width() + 1 + badge_width; // trailing `/`
                    let shown = elide_head(&row.name, width.saturating_sub(fixed).max(1));
                    // A git-ignored directory recedes into a dim, unbolded row (file-list.md).
                    let name_style = if row.ignored {
                        Style::default().fg(p.overlay0)
                    } else {
                        Style::default().fg(p.subtext0).add_modifier(Modifier::BOLD)
                    };
                    let mut spans = vec![
                        Span::styled(format!("{indent}{arrow}"), Style::default().fg(p.overlay0)),
                        Span::styled(format!("{shown}/"), name_style),
                    ];
                    if badges.changed {
                        spans.push(Span::styled(" ±", Style::default().fg(p.yellow)));
                    }
                    if badges.commented {
                        spans.push(Span::styled(" ◆", Style::default().fg(p.peach)));
                    }
                    selectable_row(spans, width, fill)
                }
                RowKind::File { annotation, .. } => file_row_item(
                    &indent,
                    annotation.as_ref(),
                    &row.name,
                    width,
                    FileRowState { fill, ignored: row.ignored, commented: badges.commented },
                    p,
                ),
            }
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

/// A file row: `<indent><marker> <name> <comment> <stats>` — the marker colored by kind,
/// the basename bright with its parent directories dimmed, an optional peach feedback diamond,
/// and the `+a −d` stats right-aligned against the pane edge. A name too wide for the row keeps
/// its tail behind a leading `…/`. An unannotated row drops the marker and stats.
#[derive(Clone, Copy)]
struct FileRowState {
    fill: Option<Color>,
    ignored: bool,
    commented: bool,
}

fn file_row_item(
    indent: &str,
    annotation: Option<&Annotation>,
    name: &str,
    width: usize,
    row_state: FileRowState,
    p: &Palette,
) -> ListItem<'static> {
    let FileRowState { fill, ignored, commented } = row_state;
    let marker = annotation.map_or(String::new(), |a| format!("{} ", a.change.marker()));
    let (additions, deletions) = annotation.map_or((0, 0), |a| (a.additions, a.deletions));
    let stats = stats_str(additions, deletions);
    let feedback = if commented { " ◆" } else { "" };
    let gap = if stats.is_empty() { 0 } else { 2 };
    let fixed = indent.width() + marker.width() + feedback.width() + stats.width() + gap;
    let shown = elide_head(name, width.saturating_sub(fixed).max(1));
    // Dim the parent directories of a collapsed-chain name; keep the basename bright.
    let (dim, base) = match shown.rfind('/') {
        Some(s) => (&shown[..=s], &shown[s + 1..]),
        None => ("", shown.as_str()),
    };

    let mut spans = vec![Span::styled(indent.to_string(), text_style(p))];
    if let Some(a) = annotation {
        spans.push(Span::styled(marker, Style::default().fg(kind_color(p, a.change.marker()))));
    }
    if !dim.is_empty() {
        spans.push(Span::styled(dim.to_string(), Style::default().fg(p.overlay0)));
    }
    // A git-ignored file recedes into a dim basename; its change marker and stats keep their
    // color so a kept ignored file still reads as a change (file-list.md).
    let base_style = if ignored { Style::default().fg(p.overlay0) } else { text_style(p) };
    spans.push(Span::styled(base.to_string(), base_style));
    if commented {
        spans.push(Span::styled(feedback.to_string(), Style::default().fg(p.peach)));
    }
    if !stats.is_empty() {
        let used: usize = spans.iter().map(Span::width).sum();
        let pad = width.saturating_sub(used + stats.width());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(stats_spans(additions, deletions, p));
    }
    selectable_row(spans, width, fill)
}

/// The `+a −d` stats text, dropping a side that is zero (`+210`, `−4`, or empty); used to
/// measure the stats column. [`stats_spans`] paints the same text in green/red.
fn stats_str(additions: u32, deletions: u32) -> String {
    match (additions, deletions) {
        (0, 0) => String::new(),
        (a, 0) => format!("+{a}"),
        (0, d) => format!("−{d}"),
        (a, d) => format!("+{a} −{d}"),
    }
}

/// The `+a −d` stats as colored spans: additions in green, deletions in red, matching the
/// diff's add/remove hues. Same glyphs (and width) as [`stats_str`].
fn stats_spans(additions: u32, deletions: u32, p: &Palette) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if additions > 0 {
        spans.push(Span::styled(format!("+{additions}"), Style::default().fg(p.green)));
    }
    if additions > 0 && deletions > 0 {
        spans.push(Span::raw(" "));
    }
    if deletions > 0 {
        spans.push(Span::styled(format!("−{deletions}"), Style::default().fg(p.red)));
    }
    spans
}

/// Shorten `name` to `max` columns by eliding its head behind a leading `…`, preferring to
/// cut at a path separator so a partial directory name never shows.
fn elide_head(name: &str, max: usize) -> String {
    if name.width() <= max {
        return name.to_string();
    }
    let budget = max.saturating_sub(1); // a column for the `…`
    let mut tail = String::new();
    let mut w = 0;
    for ch in name.chars().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        tail.insert(0, ch);
        w += cw;
    }
    if let Some(slash) = tail.find('/') {
        tail = tail[slash..].to_string();
    }
    format!("…{tail}")
}

/// A saved comment as inline display lines: a quiet box titled with the comment's location
/// (in the comment-yellow accent) holding its wrapped text. Spliced read-only under the
/// commented line so a submitted comment stays visible while reviewing.
fn comment_card_lines(
    c: &Comment,
    pi_owned: bool,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    const INDENT: usize = 2;
    let box_w = width.saturating_sub(INDENT).max(10);
    let text_w = box_w.saturating_sub(4).max(1); // inside "│ " … " │"
    let border = Style::default().fg(p.overlay0);
    let title = Style::default().fg(p.peach).add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(p.text);
    let pad = || Span::raw(" ".repeat(INDENT));

    // An agent-staged draft says so: the reviewer must see whose words they are editing.
    let heading = if pi_owned { "pi draft" } else { "comment" };
    let label = truncate_width(&format!(" {heading} · {} ", c.location()), box_w.saturating_sub(3));
    let fill = box_w.saturating_sub(3 + label.width());
    let mut lines = vec![Line::from(vec![
        pad(),
        Span::styled("╭─", border),
        Span::styled(label, title),
        Span::styled(format!("{}╮", "─".repeat(fill)), border),
    ])];

    for logical in c.text.split('\n') {
        for piece in wrap_text(logical, text_w) {
            let gap = " ".repeat(text_w.saturating_sub(piece.width()));
            lines.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                Span::styled(piece, body_style),
                Span::styled(format!("{gap} │"), border),
            ]));
        }
    }

    lines.push(Line::from(vec![
        pad(),
        Span::styled(format!("╰{}╯", "─".repeat(box_w.saturating_sub(2))), border),
    ]));
    lines
}

/// Truncate `s` to `max` display columns, marking a cut with a trailing `…`.
fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn remote_draft_card_lines(
    pending: &crate::app::PendingRemoteDraft,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    const INDENT: usize = 2;
    let box_w = width.saturating_sub(INDENT).max(10);
    let text_w = box_w.saturating_sub(4).max(1);
    let failed = pending.error.as_deref();
    let border = Style::default().fg(if pending.outcome_unknown {
        p.mauve
    } else if failed.is_some() {
        p.red
    } else {
        p.yellow
    });
    let pad = || Span::raw(" ".repeat(INDENT));
    let state = if pending.outcome_unknown {
        "outcome unknown"
    } else if failed.is_some() {
        "failed"
    } else {
        "pending sync"
    };
    // An inline draft names its anchor so a ranged comment is visibly ranged; a reply
    // draft has no diff anchor of its own.
    let label = match &pending.draft.action {
        forge::ReviewDraftAction::Inline(anchor) => {
            format!(" draft · {} · {state} ", anchor.location())
        }
        forge::ReviewDraftAction::Reply { .. } => format!(" draft · {state} "),
    };
    let label = truncate_width(&label, box_w.saturating_sub(3));
    let fill = box_w.saturating_sub(3 + label.width());
    let mut lines = vec![Line::from(vec![
        pad(),
        Span::styled("╭─", border),
        Span::styled(label, border.add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}╮", "─".repeat(fill)), border),
    ])];
    for logical in pending.draft.body.split('\n') {
        for piece in wrap_text(logical, text_w) {
            let gap = " ".repeat(text_w.saturating_sub(piece.width()));
            lines.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                Span::styled(piece, Style::default().fg(p.text)),
                Span::styled(format!("{gap} │"), border),
            ]));
        }
    }
    if let Some(error) = failed {
        for piece in wrap_text(error, text_w) {
            let gap = " ".repeat(text_w.saturating_sub(piece.width()));
            lines.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                Span::styled(piece, Style::default().fg(p.red)),
                Span::styled(format!("{gap} │"), border),
            ]));
        }
    }
    lines.push(Line::from(vec![
        pad(),
        Span::styled(format!("╰{}╯", "─".repeat(box_w.saturating_sub(2))), border),
    ]));
    lines
}

fn remote_comment_card_lines(
    comment: &forge::Comment,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    const INDENT: usize = 2;
    let box_w = width.saturating_sub(INDENT).max(10);
    let text_w = box_w.saturating_sub(4).max(1);
    let border = Style::default().fg(if comment.is_resolved { p.overlay0 } else { p.lavender });
    let title = Style::default().fg(p.mauve).add_modifier(Modifier::BOLD);
    let pad = || Span::raw(" ".repeat(INDENT));
    let state = if comment.is_resolved {
        " · resolved"
    } else if comment.is_outdated {
        " · outdated"
    } else {
        ""
    };
    let label = truncate_width(
        &format!(" @{}{} · {} ", comment.author, state, comment.anchor),
        box_w.saturating_sub(3),
    );
    let fill = box_w.saturating_sub(3 + label.width());
    let mut lines = vec![Line::from(vec![
        pad(),
        Span::styled("╭─", border),
        Span::styled(label, title),
        Span::styled(format!("{}╮", "─".repeat(fill)), border),
    ])];
    for logical in comment.body.split('\n') {
        for piece in wrap_text(logical, text_w) {
            let gap = " ".repeat(text_w.saturating_sub(piece.width()));
            lines.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                Span::styled(piece, Style::default().fg(p.text)),
                Span::styled(format!("{gap} │"), border),
            ]));
        }
    }
    for reply in &comment.replies {
        let heading = format!("↳ @{}", reply.author);
        let gap = " ".repeat(text_w.saturating_sub(heading.width()));
        lines.push(Line::from(vec![
            pad(),
            Span::styled("│ ", border),
            Span::styled(heading, Style::default().fg(p.peach)),
            Span::styled(format!("{gap} │"), border),
        ]));
        for logical in reply.body.split('\n') {
            for piece in wrap_text(logical, text_w) {
                let gap = " ".repeat(text_w.saturating_sub(piece.width()));
                lines.push(Line::from(vec![
                    pad(),
                    Span::styled("│ ", border),
                    Span::styled(piece, Style::default().fg(p.subtext0)),
                    Span::styled(format!("{gap} │"), border),
                ]));
            }
        }
    }
    if let Some((label, color)) = partial_replies_label(comment, p) {
        let gap = " ".repeat(text_w.saturating_sub(label.width()));
        lines.push(Line::from(vec![
            pad(),
            Span::styled("│ ", border),
            Span::styled(label, Style::default().fg(color)),
            Span::styled(format!("{gap} │"), border),
        ]));
    }
    lines.push(Line::from(vec![
        pad(),
        Span::styled(format!("╰{}╯", "─".repeat(box_w.saturating_sub(2))), border),
    ]));
    lines
}

fn render_diff_view(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let title = match (&app.diff_path, &app.diff.previous_path) {
        (Some(new), Some(old)) => format!("{old} → {new}"),
        (Some(new), None) => new.clone(),
        (None, _) => match app.tab {
            Tab::AllFiles => "File",
            _ => "Diff",
        }
        .to_string(),
    };
    let block = bordered(&title, app.focus == Focus::Diff, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.visible.is_empty() {
        // `All files` is a content browser, not a diff, so its empty/notice copy avoids diff
        // vocabulary and never shows the last-turn "waiting" state.
        let msg = match app.tab {
            Tab::AllFiles => match app.diff.state {
                FileState::Binary => "binary — no line comments",
                FileState::TooLarge => "file too large",
                FileState::Unavailable => "content unavailable",
                FileState::Normal if app.diff_path.is_some() => "empty file",
                FileState::Normal => "select a file to read",
            },
            Tab::Changes if app.awaiting_turn() => "waiting for the agent's next turn",
            _ => match app.diff.state {
                FileState::Binary => "binary — no line comments",
                FileState::TooLarge => "file too large to diff",
                FileState::Unavailable => "remote patch unavailable — open it on the forge",
                FileState::Normal => "no diff",
            },
        };
        frame.render_widget(dim_paragraph(msg, p), inner);
        return;
    }

    let height = inner.height as usize;
    if height == 0 {
        return;
    }
    let width = inner.width as usize;
    let gutter_w = gutter_for(&app.diff);
    let layout = RowLayout {
        gutter_w,
        width,
        h_scroll: app.h_scroll,
        wrap: app.wrap,
        focused: app.focus == Focus::Diff,
        pal: p,
    };
    let commented = app.commented_lines();
    let cards = app.comment_cards();
    let remote_cards = app.remote_comment_cards();
    let draft_cards = app.remote_draft_cards();
    let editing = editing_comment(app);
    let (lo, hi) = app.selection_range();
    let selecting = app.focus == Focus::Diff && app.select_anchor.is_some();

    // One logical row → its 1+ wrapped display lines, then any saved-comment cards anchored
    // to it. The cursor/selection apply to the code line's display rows, not the cards. The
    // card of a comment being edited is hidden — its edit box stands in for it.
    let row_lines = |i: usize| -> Vec<Line> {
        let state = RowState {
            commented: commented.contains(&i),
            cursor: app.focus == Focus::Diff && i == app.diff_cursor,
            selected: selecting && i >= lo && i <= hi,
            decoration: app.line_decoration(&app.visible[i]),
            pi: app.pi_line_decoration(&app.visible[i]),
        };
        let mut lines = render_row(&app.visible[i], layout, state);
        for &ci in &cards[i] {
            if Some(ci) != editing
                && let Some(c) = app.store.get(ci)
            {
                lines.extend(comment_card_lines(c, app.collab_owned_comment(ci), width, p));
            }
        }
        if let Some(snapshot) = app.pr_snapshot() {
            for &ci in &remote_cards[i] {
                if let Some(comment) = snapshot.comments.get(ci) {
                    lines.extend(remote_comment_card_lines(comment, width, p));
                }
            }
        }
        for &index in &draft_cards[i] {
            if let Some(draft) = app.remote_drafts.get(index) {
                lines.extend(remote_draft_card_lines(draft, width, p));
            }
        }
        lines
    };
    // Display lines for the logical rows in `range`, in order.
    let display = |range: std::ops::Range<usize>| -> Vec<Line> {
        range.flat_map(&row_lines).collect::<Vec<_>>()
    };

    let rows = app.visible.len();
    if !app.composing() {
        // Fill the pane from `diff_scroll`'s first display line; clamp keeps the cursor in.
        let mut out = display(app.diff_scroll..rows);
        out.truncate(height);
        frame.render_widget(Paragraph::new(out), inner);
        return;
    }

    // Composing: splice the input box under the last selected line, in display rows.
    // Cap the box at height-1 so a comment taller than the viewport can't hide its anchor.
    let box_h = composer_height(app, width).min(height.saturating_sub(1)).max(1);
    let diff_budget = height - box_h;
    let anchor = hi.clamp(app.diff_scroll, rows.saturating_sub(1));
    let above = display(app.diff_scroll..anchor + 1);
    // Keep the anchor's last display line just above the box when `above` overflows.
    let above: Vec<Line> =
        if above.len() > diff_budget { above[above.len() - diff_budget..].to_vec() } else { above };
    let remaining = diff_budget - above.len();
    let mut below = display(anchor + 1..rows);
    below.truncate(remaining);

    let slots = Layout::vertical([
        Constraint::Length(above.len() as u16),
        Constraint::Length(box_h as u16),
        Constraint::Length(below.len() as u16),
    ])
    .split(inner);
    if !above.is_empty() {
        frame.render_widget(Paragraph::new(above), slots[0]);
    }
    render_composer(frame, app, slots[1]);
    if !below.is_empty() {
        frame.render_widget(Paragraph::new(below), slots[2]);
    }
}

/// The line-number column width for a diff of `rows` lines.
fn gutter_width(rows: usize) -> usize {
    rows.to_string().len().max(3)
}

/// The gutter width for a whole `FileDiff`, sized to its largest line number so it does not
/// resize when a fold toggles (folds hide lines but keep the numbering). One definition,
/// shared by `diff_row_heights` (measuring) and `render_diff_view` (painting), so the
/// measured and painted geometry can never disagree.
fn gutter_for(diff: &FileDiff) -> usize {
    let total_lines: usize =
        diff.rows.iter().map(|r| if r.is_content() { 1 } else { r.hidden() }).sum();
    gutter_width(total_lines)
}

/// The gutter prefix width: the change bar plus the right-aligned line number and a space.
fn gutter_prefix_width(gutter_w: usize) -> usize {
    1 + gutter_w + 1
}

/// How many display rows a row needs: 1 for a fold or with wrap off, else the number of
/// word-wrapped segments its (tab-expanded) content fills. Shares [`wrap_segments`] with
/// the renderer so per-row geometry stays aligned with what gets painted.
fn row_height(row: &Row, gutter_w: usize, width: usize, wrap: bool) -> usize {
    if !wrap || matches!(row, Row::Fold { .. } | Row::PatchGap { .. }) {
        return 1;
    }
    let code_width = width.saturating_sub(gutter_prefix_width(gutter_w)).max(1);
    wrap_segments(&code_cells(row, false), code_width).len()
}

/// The diff-pane layout: constant for a frame.
#[derive(Clone, Copy)]
struct RowLayout<'a> {
    gutter_w: usize,
    width: usize,
    h_scroll: usize,
    wrap: bool,
    /// Whether the diff pane is focused — dims the cursor row when it is not.
    focused: bool,
    /// The active palette for the change bars, row tints, and fills.
    pal: &'a Palette,
}

/// A row's per-row highlight state.
#[derive(Clone, Copy)]
struct RowState {
    commented: bool,
    cursor: bool,
    selected: bool,
    decoration: Option<LineDecoration>,
    /// Set when the deep-session agent changed this line since the session baseline: the
    /// change bar cell renders as a `✦` badge in the collab accent, so Pi's edits stand
    /// apart from the review's own changes.
    pi: Option<LineDecoration>,
}

/// A diff row as one or more full-width display lines: a left change bar, the line
/// number, then syntax-colored code tinted red/green. With wrap on, a long line breaks
/// into `code_width`-wide rows; a continuation row carries a blank gutter so numbers
/// stay aligned. With wrap off, the line is one row scrolled by `h_scroll`.
fn render_row(row: &Row, layout: RowLayout<'_>, state: RowState) -> Vec<Line<'static>> {
    let RowLayout { gutter_w, width, h_scroll, wrap, focused, pal } = layout;
    let RowState { commented, cursor, selected, decoration, pi } = state;
    if matches!(row, Row::Fold { .. } | Row::PatchGap { .. }) {
        let label = match row {
            Row::Fold { .. } if cursor => {
                format!("  ⋯  {} unmodified lines — → expand", row.hidden())
            }
            Row::Fold { .. } => format!("  ⋯  {} unmodified lines", row.hidden()),
            Row::PatchGap { .. } => format!("  ⋯  {} lines omitted by remote patch", row.hidden()),
            _ => unreachable!(),
        };
        let mut line = Line::from(Span::styled(label, Style::default().fg(pal.subtext0)));
        if let Some(pad) = width.checked_sub(line.width()).filter(|p| *p > 0) {
            line.push_span(Span::raw(" ".repeat(pad)));
        }
        let bg = if cursor { pal.cursor_bg(focused) } else { pal.surface0 };
        return vec![line.style(Style::default().bg(bg).add_modifier(Modifier::BOLD))];
    }
    let num = row.new_no().or_else(|| row.old_no()).map_or(String::new(), |n| n.to_string());
    // A commented line's number takes the peach comment accent; others sit a step brighter
    // than the dim chrome so they stay legible while read.
    let num_color = if commented { pal.peach } else { pal.overlay1 };
    // The `✦` badge takes the bar cell: whose edit a line is outranks what kind of
    // change it is, and the row's red/green fill still carries the change class.
    let (bar, bar_color) = if pi.is_some() {
        ("✦", pal.mauve)
    } else {
        match (row.marker(), decoration) {
            ('-', _) => ("▌", pal.red),
            ('+', _) | (_, Some(LineDecoration::Added)) => ("▌", pal.green),
            (_, Some(LineDecoration::Modified)) => ("▌", pal.lavender),
            (_, Some(LineDecoration::Deleted)) => ("▸", pal.red),
            _ => (" ", pal.overlay0),
        }
    };
    let row_bg = if cursor {
        Some(pal.cursor_bg(focused))
    } else if selected {
        Some(pal.surface1)
    } else {
        match row.marker() {
            '-' => Some(pal.del_bg),
            '+' => Some(pal.ins_bg),
            _ => None,
        }
    };

    // Word emphasis brightens the changed words, unless the row's fill is a cursor or
    // selection bg, which wins for readability.
    let emph_on = !cursor && !selected;
    let emph_bg = match row.marker() {
        '-' => pal.emph_del_bg,
        '+' => pal.emph_ins_bg,
        _ => pal.ins_bg,
    };
    let cells = code_cells(row, emph_on);

    let prefix_w = gutter_prefix_width(gutter_w);
    let code_width = width.saturating_sub(prefix_w).max(1);
    // Without wrap the line is one chunk scrolled by `h_scroll`; with wrap, word-wrapped
    // segments, the first numbered and the rest blank-gutter.
    let chunks: Vec<&[Cell]> = if wrap {
        wrap_segments(&cells, code_width).into_iter().map(|(s, e)| &cells[s..e]).collect()
    } else {
        vec![cells.get(skip_columns(&cells, h_scroll)..).unwrap_or(&[])]
    };

    chunks
        .into_iter()
        .enumerate()
        .map(|(k, chunk)| {
            let gutter = if k == 0 {
                vec![
                    Span::styled(bar, Style::default().fg(bar_color)),
                    Span::styled(format!("{num:>gutter_w$} "), Style::default().fg(num_color)),
                ]
            } else {
                // A continuation row keeps the change bar but blanks the number column.
                vec![
                    Span::styled(bar, Style::default().fg(bar_color)),
                    Span::raw(" ".repeat(prefix_w - 1)),
                ]
            };
            let mut spans = gutter;
            spans.extend(cells_to_spans(chunk, emph_bg));
            let mut line = Line::from(spans);
            if let Some(pad) = width.checked_sub(line.width()).filter(|p| *p > 0) {
                line.push_span(Span::raw(" ".repeat(pad)));
            }
            match row_bg {
                Some(bg) => line.style(Style::default().bg(bg)),
                None => line,
            }
        })
        .collect()
}

fn rgb(c: crate::diff::Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Tabs expand to this many columns.
const TAB: usize = 4;

/// Greedy word wrap over display cells into half-open ranges, one per display row.
///
/// Breaks at the last space that fits within `width`, falling back to a hard break when a
/// single word is wider than the column. Leading spaces on a continuation are dropped so a
/// break landing just before a space doesn't leave an almost-empty row. An empty line still
/// yields one (empty) range so it occupies a row. The renderer and [`row_height`] share this
/// so what's measured matches what's painted.
fn wrap_segments(cells: &[Cell], width: usize) -> Vec<(usize, usize)> {
    if cells.is_empty() {
        return vec![(0, 0)];
    }
    let mut segs = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        // Take as many cells as fit within `width` columns, always at least one (so a glyph
        // wider than the column still gets its own row rather than stalling).
        let mut col = 0;
        let mut limit = start;
        while limit < cells.len() {
            let cw = cells[limit].w;
            if col + cw > width && limit > start {
                break;
            }
            col += cw;
            limit += 1;
        }
        if limit == cells.len() {
            segs.push((start, cells.len()));
            break;
        }
        // More cells follow; prefer breaking just after the last space that fits.
        let brk = (start..limit).rev().find(|&i| cells[i].ch == ' ').map(|i| i + 1);
        let end = brk.filter(|&e| e > start).unwrap_or(limit);
        segs.push((start, end));
        start = end;
        while start < cells.len() && cells[start].ch == ' ' {
            start += 1;
        }
    }
    segs
}

/// The first cell index lying at or past `cols` display columns — the no-wrap horizontal
/// scroll offset, snapping past a wide glyph that straddles the boundary rather than
/// splitting it.
fn skip_columns(cells: &[Cell], cols: usize) -> usize {
    let mut col = 0;
    let mut i = 0;
    while i < cells.len() && col < cols {
        col += cells[i].w;
        i += 1;
    }
    i
}

/// One display cell of a code line: a glyph, its terminal width in columns (1 for most
/// text, 2 for wide CJK/emoji, 0 for a combining mark), its syntax color, and whether it
/// falls in a word-emphasis range.
struct Cell {
    ch: char,
    w: usize,
    fg: Color,
    emph: bool,
}

/// Expand a row's spans into display cells: tabs become spaces to the next tab stop, and
/// each char carries its column width, color, and (when `emph_on`) its word-emphasis flag.
/// Width comes from `unicode-width` so wide glyphs measure as the two columns they paint.
fn code_cells(row: &Row, emph_on: bool) -> Vec<Cell> {
    let emphasis = if emph_on { row.emphasis() } else { &[] };
    let in_emph = |i: u32| emphasis.iter().any(|&(a, b)| i >= a && i < b);
    let mut cells = Vec::new();
    let mut idx = 0u32;
    let mut col = 0usize; // display column, so tab stops land right after wide glyphs too
    for s in row.spans() {
        let fg = rgb(s.color);
        for ch in s.text.chars() {
            let emph = in_emph(idx);
            if ch == '\t' {
                for _ in 0..(TAB - col % TAB) {
                    cells.push(Cell { ch: ' ', w: 1, fg, emph });
                    col += 1;
                }
            } else {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                cells.push(Cell { ch, w, fg, emph });
                col += w;
            }
            idx += 1;
        }
    }
    cells
}

/// Build spans from display cells, merging runs of equal color/emphasis; an emphasized
/// run takes `emph_bg` as its background.
fn cells_to_spans(cells: &[Cell], emph_bg: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<(Color, bool)> = None;
    for c in cells {
        let key = (c.fg, c.emph);
        if cur != Some(key) {
            if let Some((fg, emph)) = cur {
                spans.push(cell_span(std::mem::take(&mut buf), fg, emph, emph_bg));
            }
            cur = Some(key);
        }
        buf.push(c.ch);
    }
    if let Some((fg, emph)) = cur {
        spans.push(cell_span(buf, fg, emph, emph_bg));
    }
    spans
}

fn cell_span(text: String, fg: Color, emph: bool, emph_bg: Color) -> Span<'static> {
    let style = Style::default().fg(fg);
    Span::styled(text, if emph { style.bg(emph_bg) } else { style })
}

/// The inline comment input box, drawn at `area` (under the selection in the diff).
fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let loc = app.pending_location().unwrap_or_else(|| "comment".to_string());
    let editing = matches!(app.mode, Mode::Composing { editing: Some(_) });
    let title = if editing { format!("edit · {loc}") } else { format!("comment · {loc}") };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.peach))
        .title(title);
    let content_w = composer_content_width(area.width as usize);
    let body = Paragraph::new(composer_lines(app, content_w)).block(block);
    frame.render_widget(body, area);
}

/// The key glyph and label for a footer action; an empty label renders the glyph alone. The
/// `TogglePane` and `Send` labels depend on `app` (the destination pane, the comment count).
fn action_key_label(app: &App, action: FooterAction) -> (String, String) {
    use FooterAction as A;
    let (k, l): (&str, &str) = match action {
        A::Comment => ("c", "comment"),
        A::Reply => ("c", "reply"),
        A::Select => ("v", "select"),
        A::ClearSelection => ("esc", "clear"),
        A::EditComment => ("e", "edit"),
        A::DeleteComment => ("d", "delete"),
        A::JumpComment => ("n/N", "jump"),
        A::ExpandFold => ("→", "expand fold"),
        A::ExpandDir => ("→", "expand"),
        A::CollapseDir => ("←", "collapse"),
        A::TogglePane => {
            return ("⇥".into(), if app.focus == Focus::Files { "diff" } else { "files" }.into());
        }
        A::Scope => ("u/b/t", "scope"),
        A::Projects => ("^p", "projects"),
        A::SwitchProject => ("enter", "switch"),
        A::Send
            if app.mode != Mode::List
                && (app.tab == Tab::Pr
                    || (app.tab == Tab::Changes && app.remote_changes_active())) =>
        {
            return ("s".into(), format!("sync {}", app.syncable_remote_draft_count()));
        }
        A::Send => return ("s".into(), format!("send {}", app.store.len())),
        A::List => ("l", "list"),
        A::Copy => ("y", "copy"),
        A::Save => ("enter", "save"),
        A::Newline => ("⇧⏎", "newline"),
        A::Cancel => ("esc", "cancel"),

        A::AttachPi => ("a", "ask pi"),
        A::TrayToggle => ("A", "tray"),
        A::DeepReview => ("D", "deep review"),
        A::EndDeep => ("X", "end review"),
        A::UpdateHead => ("U", "update head"),
        A::OpenPr => ("o", "open ↗"),
        A::PickPr => ("p", "pick"),
        A::PinPr => ("enter", "pin"),
        A::UnpinPr => ("esc", "unpin"),
        A::CloseList | A::ClosePicker => ("esc", "close"),
        A::Refresh => ("r", "refresh"),
        A::Tabs => ("1·2·3", ""),
        A::Quit => ("q", ""),
    };
    (k.into(), l.into())
}

/// A tier's `(key, label)` styles: the primary bright and bold, normal actions readable, the
/// orientation cluster dim so the eye lands on what to do, not on the always-there anchors.
fn tier_styles(tier: Tier, p: &Palette) -> (Style, Style) {
    match tier {
        Tier::Primary => (Style::default().fg(p.peach).add_modifier(Modifier::BOLD), text_style(p)),
        Tier::Normal => (Style::default().fg(p.lavender), Style::default().fg(p.subtext0)),
        Tier::Orientation => (Style::default().fg(p.overlay0), Style::default().fg(p.overlay0)),
    }
}

/// Render a run of actions as ` · `-separated `key label` spans, styled per tier.
fn action_spans(app: &App, acts: &[(FooterAction, Tier)]) -> Vec<Span<'static>> {
    let p = app.palette();
    let mut spans = Vec::new();
    for (i, &(action, tier)) in acts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(p.overlay0)));
        }
        let (key, label) = action_key_label(app, action);
        let (key_style, label_style) = tier_styles(tier, p);
        spans.push(Span::styled(key, key_style));
        if !label.is_empty() {
            spans.push(Span::styled(format!(" {label}"), label_style));
        }
    }
    spans
}

/// The footer action bar: the context's actions (primary highlighted) packed left, the dim
/// orientation cluster packed right, fitting one line — orientation dropped first, then trailing
/// `Normal` actions, with a trailing `…` marking anything clipped (`specs/tui.md`).
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let w = area.width as usize;
    let all = app.footer_actions();
    let (mut left_acts, orient_acts): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|&(_, t)| t != Tier::Orientation);

    // The read-only PR tab leads with the PR's state summary; the transient status sits among
    // the actions and never displaces them. The state line is capped so a long one never crowds
    // the primary action (and the `…`) off the line — leaving room for the actions plus the marker.
    let actions_w: usize = action_spans(app, &left_acts).iter().map(Span::width).sum();
    let pr_info = (app.tab == Tab::Pr).then(|| app.pr_snapshot()).flatten().map(|s| {
        let budget = w.saturating_sub(actions_w + 4).max(8);
        let text = truncate_width(&format!("{}   ", pr_state_line(s)), budget);
        Span::styled(text, Style::default().fg(p.subtext0))
    });
    let status = (!app.status.is_empty())
        .then(|| Span::styled(format!("  · {} ", app.status), Style::default().fg(p.peach)));
    // The collaboration segment: tray aliases plus the Pi link state, present whenever a
    // collaboration host runs so a downed link is visible rather than silently absent.
    let collab = app.collab_link.map(|up| {
        let chips = if app.collab_tray.is_empty() {
            String::new()
        } else {
            app.collab_tray.join(" ") + " "
        };
        if up {
            let mut label = format!("  {chips}✦ pi");
            // Both follow states are named: `following` says the pane will move with Pi,
            // while follow-off keeps the reviewer aware of Pi's position without moving them.
            match app.collab_follow {
                Some(true) => {
                    label.push_str(" following");
                    // The manual-navigation grace is visible while it holds follow back,
                    // counting down so the pause never reads as follow being broken.
                    if let Some(ms) = app.collab_grace_ms {
                        use std::fmt::Write as _;
                        let _ = write!(label, " ⏸ {}.{}s", ms / 1000, ms % 1000 / 100);
                    }
                }
                Some(false) => {
                    label.push_str(" ⊘follow");
                    if let Some(at) = &app.collab_pi_location {
                        use std::fmt::Write as _;
                        let _ = write!(label, " @ {at}");
                    }
                }
                None => {}
            }
            if let Some((at, len)) = app.collab_history {
                use std::fmt::Write as _;
                let _ = write!(label, " ⟲ {at}/{len}");
            }
            Span::styled(label, Style::default().fg(p.mauve))
        } else {
            Span::styled(format!("  {chips}✧ pi offline"), Style::default().fg(p.overlay0))
        }
    });

    let build_left = |acts: &[(FooterAction, Tier)]| -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        if let Some(info) = &pr_info {
            spans.push(info.clone());
        }
        spans.extend(action_spans(app, acts));
        if let Some(st) = &status {
            spans.push(st.clone());
        }
        if let Some(chip) = &collab {
            spans.push(chip.clone());
        }
        spans
    };
    let orient: Vec<Span> = if orient_acts.is_empty() {
        Vec::new()
    } else {
        let mut spans = vec![Span::styled("│ ", Style::default().fg(p.overlay0))];
        spans.extend(action_spans(app, &orient_acts));
        spans
    };
    let orient_w: usize = orient.iter().map(Span::width).sum();

    let mut left = build_left(&left_acts);
    let line_width = |s: &[Span]| -> usize { s.iter().map(Span::width).sum() };
    let fits_with_orient = !orient.is_empty() && line_width(&left) + 1 + orient_w <= w;

    let spans = if fits_with_orient {
        // Leave one trailing cell so the last hint (`q`) doesn't butt against the edge.
        let pad = w.saturating_sub(line_width(&left) + orient_w + 1);
        left.push(Span::raw(" ".repeat(pad)));
        left.extend(orient);
        left
    } else {
        // Orientation is dropped; trim trailing `Normal` actions until the line fits, leaving
        // room for the `…` that marks the drop. The primary action is never trimmed.
        let dropped_orient = !orient.is_empty();
        let mut popped = false;
        while line_width(&left) + 2 > w
            && left_acts.len() > 1
            && left_acts.last().is_some_and(|&(_, t)| t == Tier::Normal)
        {
            left_acts.pop();
            popped = true;
            left = build_left(&left_acts);
        }
        // `…` whenever anything was clipped: the orientation cluster, a trimmed action, or a
        // primary still too wide to fit.
        if dropped_orient || popped || line_width(&left) + 2 > w {
            left.push(Span::styled(" …", Style::default().fg(p.overlay0)));
        }
        left
    };

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.surface0)),
        area,
    );
}

fn render_comments_list(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let popup = centered(area, 80, 60);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.mauve))
        .title(format!("Comments ({})", app.store.len()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let width = inner.width as usize;
    // Windowed on the cursor like the navigator's scroll, so a list taller than the
    // popup keeps the selected row on screen.
    let viewport = inner.height as usize;
    let scroll = app.list_cursor.saturating_sub(viewport.saturating_sub(1));
    let items: Vec<ListItem> = app
        .store
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport)
        .map(|(i, c)| {
            let loc = Span::styled(
                c.location(),
                Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
            );
            let mut spans = vec![loc, Span::styled(format!("  {}", c.text), text_style(p))];
            // A comment whose anchor may have moved (file left the changeset, or a content
            // comment's file was deleted) is flagged but kept.
            if app.is_stale(c) {
                spans.push(Span::styled("  (stale)", Style::default().fg(p.red)));
            }
            // The list overlay is the active modal, so its row reads at full brightness.
            selectable_row(spans, width, (i == app.list_cursor).then_some(p.surface2))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

/// The default body text color.
fn text_style(p: &Palette) -> Style {
    Style::default().fg(p.text)
}

/// A list row, highlighted with the shared selection fill (`surface2` + bold, full
/// width) when `selected` — the same treatment the diff cursor uses, so every cursor
/// in the UI reads the same. The fill is applied per span (with a trailing pad) so it
/// spans the full width under the `List` widget, matching the diff's `Paragraph` rows.
fn selectable_row(
    mut spans: Vec<Span<'static>>,
    width: usize,
    fill: Option<Color>,
) -> ListItem<'static> {
    if let Some(bg) = fill {
        let used: usize = spans.iter().map(Span::width).sum();
        if width > used {
            spans.push(Span::raw(" ".repeat(width - used)));
        }
        for s in &mut spans {
            s.style = s.style.bg(bg).add_modifier(Modifier::BOLD);
        }
    }
    ListItem::new(Line::from(spans))
}

/// The PR/MR picker overlay (`p` on the PR tab): the target's open PRs/MRs, then the newest
/// merged and closed ones, each section newest first; `enter` pins the highlighted row
/// (specs/forge-host.md "Picker"). Closed rows carry their fate: `merged · ✓ passed`,
/// `merged · ✗ failed`, `closed`.
fn render_pr_picker(frame: &mut Frame, app: &App, area: Rect) {
    use crate::app::PrPicker;
    let p = app.palette();
    let popup = centered(area, 80, 60);
    frame.render_widget(Clear, popup);
    let (unit, number_prefix) = match app.pr_context.as_ref().map(|(t, _)| t.provider) {
        Some(forge::Provider::Gitlab) => ("MRs", "!"),
        _ => ("PRs", "#"),
    };
    let count = match &app.pr_picker {
        Some(PrPicker::Loaded { listing, filtered, .. }) => format!(
            " ({}/{} · {} open · {} done)",
            filtered.len(),
            listing.open.len() + listing.done.len(),
            listing.open.len(),
            listing.done.len()
        ),
        _ => String::new(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.mauve))
        .title(format!("{unit}{count}"));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height == 0 {
        return;
    }

    let query_row = Rect { height: 1, ..inner };
    let query = Line::from(vec![
        Span::styled("> ", Style::default().fg(p.mauve).add_modifier(Modifier::BOLD)),
        Span::styled(app.pr_picker_query.clone(), text_style(p)),
        Span::styled("█", Style::default().fg(p.overlay0)),
    ]);
    frame.render_widget(Paragraph::new(query), query_row);
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let width = list.width as usize;
    match &app.pr_picker {
        Some(PrPicker::Loading) => {
            frame.render_widget(Paragraph::new("loading…").style(text_style(p)), list);
        }
        Some(PrPicker::Failed(message)) => {
            frame.render_widget(
                Paragraph::new(message.as_str()).style(Style::default().fg(p.red)),
                list,
            );
        }
        Some(PrPicker::Loaded { listing, .. })
            if listing.open.is_empty() && listing.done.is_empty() =>
        {
            frame.render_widget(
                Paragraph::new("no items — esc to close").style(text_style(p)),
                list,
            );
        }
        Some(PrPicker::Loaded { filtered, .. }) if filtered.is_empty() => {
            frame.render_widget(Paragraph::new("no match").style(text_style(p)), list);
        }
        Some(PrPicker::Loaded { listing, filtered, cursor }) => {
            // Display only fuzzy matches, retaining section headers when that section has a hit.
            // The cursor indexes `filtered`, not the source listing.
            enum Row<'a> {
                Header(&'static str),
                Item(usize, &'a forge::PrListItem),
            }
            let mut rows: Vec<Row> = Vec::new();
            let mut previous_section = None;
            for (result_index, &source_index) in filtered.iter().enumerate() {
                let (section, item) = if source_index < listing.open.len() {
                    (0, &listing.open[source_index])
                } else {
                    (1, &listing.done[source_index - listing.open.len()])
                };
                if previous_section != Some(section) {
                    rows.push(Row::Header(if section == 0 { "Open" } else { "Merged & closed" }));
                    previous_section = Some(section);
                }
                rows.push(Row::Item(result_index, item));
            }
            let cursor_row = rows
                .iter()
                .position(|row| matches!(row, Row::Item(i, _) if i == cursor))
                .unwrap_or(0);
            let visible = list.height as usize;
            let top = cursor_row.saturating_sub(visible.saturating_sub(1));
            let items: Vec<ListItem> = rows
                .iter()
                .skip(top)
                .take(visible)
                .map(|row| match row {
                    Row::Header(label) => ListItem::new(Line::from(Span::styled(
                        format!("— {label} —"),
                        Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
                    ))),
                    Row::Item(i, item) => {
                        let number = Span::styled(
                            format!("{number_prefix}{}", item.number),
                            Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
                        );
                        let mut spans = vec![number];
                        if item.is_draft {
                            spans.push(Span::styled("  draft", Style::default().fg(p.overlay0)));
                        }
                        spans.push(Span::styled(format!("  {}", item.title), text_style(p)));
                        spans.push(Span::styled(
                            format!("  {} @{}", item.head_ref, item.author),
                            Style::default().fg(p.overlay0),
                        ));
                        if let Some(status) = picker_fate(item, p) {
                            spans.push(Span::raw("  "));
                            spans.extend(status);
                        }
                        selectable_row(spans, width, (*i == *cursor).then_some(p.surface2))
                    }
                })
                .collect();
            frame.render_widget(List::new(items), list);
        }
        None => {}
    }
}

/// The project-switcher overlay (`ctrl-p` on any tab): a typed filter over the discovered
/// projects, best match first; `enter` re-points the sidebar at the highlighted one
/// (`specs/tui.md#project-switcher`). The first inner row is the filter; the list scrolls
/// to keep the cursor visible, like the PR picker.
fn render_project_switcher(frame: &mut Frame, app: &App, area: Rect) {
    let Some(switcher) = &app.switcher else {
        return;
    };
    let p = app.palette();
    let popup = centered(area, 60, 60);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.mauve))
        .title(format!("Projects ({}/{})", switcher.filtered.len(), switcher.projects.len()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height == 0 {
        return;
    }

    let query_row = Rect { height: 1, ..inner };
    let query = Line::from(vec![
        Span::styled("> ", Style::default().fg(p.mauve).add_modifier(Modifier::BOLD)),
        Span::styled(switcher.query.clone(), text_style(p)),
        Span::styled("█", Style::default().fg(p.overlay0)),
    ]);
    frame.render_widget(Paragraph::new(query), query_row);

    let list =
        Rect { y: inner.y.saturating_add(1), height: inner.height.saturating_sub(1), ..inner };
    if switcher.projects.is_empty() {
        let hint = "no projects found — set switcher_roots in config.toml";
        frame.render_widget(Paragraph::new(hint).style(Style::default().fg(p.red)), list);
        return;
    }
    if switcher.filtered.is_empty() {
        frame.render_widget(Paragraph::new("no match").style(text_style(p)), list);
        return;
    }

    let width = list.width as usize;
    let visible = list.height as usize;
    let top = switcher.cursor.saturating_sub(visible.saturating_sub(1));
    let items: Vec<ListItem> = switcher
        .filtered
        .iter()
        .enumerate()
        .skip(top)
        .take(visible)
        .map(|(row, &i)| {
            let project = &switcher.projects[i];
            let spans = vec![
                Span::styled(project.name.clone(), text_style(p)),
                Span::styled(
                    format!("  {}", project.path.display()),
                    Style::default().fg(p.overlay0),
                ),
            ];
            selectable_row(spans, width, (row == switcher.cursor).then_some(p.surface2))
        })
        .collect();
    frame.render_widget(List::new(items), list);
}

/// The trailing fate of a non-open row — `merged · ✓ passed` / `merged · ✗ failed` /
/// `closed` — plus the CI verdict and the comment counts: exact `open · resolved` thread
/// counts where the forge reports them (GitLab), the plain total elsewhere. `None` when a
/// row has nothing to say, so clean rows stay clean.
fn picker_fate(item: &forge::PrListItem, p: &Palette) -> Option<Vec<Span<'static>>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    match item.state {
        forge::PrState::Merged => spans.push(Span::styled("merged", Style::default().fg(p.mauve))),
        forge::PrState::Closed => spans.push(Span::styled("closed", Style::default().fg(p.red))),
        forge::PrState::Open => {}
    }
    if let Some(ci) = item.ci {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(p.overlay0)));
        }
        let (mark, label, color) = match ci {
            forge::CheckStatus::Success => ("✓", "passed", p.green),
            forge::CheckStatus::Failure => ("✗", "failed", p.red),
            forge::CheckStatus::Running => ("●", "running", p.yellow),
            forge::CheckStatus::Pending => ("○", "pending", p.overlay0),
            forge::CheckStatus::Skipped => ("⊘", "skipped", p.overlay0),
        };
        spans.push(Span::styled(format!("{mark} {label}"), Style::default().fg(color)));
    }
    let dot = |spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(p.overlay0)));
        }
    };
    match (item.threads_open, item.threads_resolved) {
        (Some(open), Some(resolved)) if open + resolved > 0 => {
            dot(&mut spans);
            if open > 0 {
                spans.push(Span::styled(format!("{open} open"), Style::default().fg(p.yellow)));
                if resolved > 0 {
                    spans.push(Span::styled(" · ", Style::default().fg(p.overlay0)));
                }
            }
            if resolved > 0 {
                spans.push(Span::styled(
                    format!("{resolved} resolved"),
                    Style::default().fg(p.overlay0),
                ));
            }
        }
        _ if item.comments > 0 => {
            dot(&mut spans);
            spans.push(Span::styled(
                format!("{} comments", item.comments),
                Style::default().fg(p.overlay0),
            ));
        }
        _ => {}
    }
    (!spans.is_empty()).then_some(spans)
}

// --- PR tab (specs/forge-host.md, specs/tui.md) --------------------------------

/// The header for the read-only PR tab: the tab names, then a right-anchored, clickable
/// `status #number ↗` chip (status colored by lifecycle, the `↗` sharing the number's colour),
/// with the PR title right-aligned to its left. Merge/sync/checks live in the footer.
fn render_pr_header(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let bar = Style::default().bg(p.surface0);
    let mut spans = tab_bar_spans(app);
    // A picked PR/MR is pinned to the tab — say so where the eye checks which PR it's on.
    if let Some(pin) = &app.pr_pin {
        spans
            .push(Span::styled(format!("  ⚲ pinned {}", pin.number), Style::default().fg(p.mauve)));
    }
    let lead_tabs: usize = spans.iter().map(Span::width).sum();
    let w = area.width as usize;

    // A resolved PR shows its identity chip; with no PR the header carries nothing — the read
    // pane is the single home for the empty/degraded message, not repeated across all regions.
    if let forge::PrView::Pr(s) = &app.pr {
        let number = format!("{}{}", s.provider.number_prefix(), s.number);
        let (status, color) = pr_status_chip(p, s);
        let chip_w = pr_chip_width(s);
        // The resolved head branch, dim left of the chip — the name that resolved, which can
        // differ from the worktree's local branch; `⑂` marks a fork head so a same-named
        // fork PR is visible (specs/forge-host.md). Dropped first when the bar is narrow.
        let head = match (s.head_ref.is_empty(), s.head_is_fork) {
            (true, _) => String::new(),
            (false, true) => format!("⑂ {}", s.head_ref),
            (false, false) => s.head_ref.clone(),
        };
        let head_w = if head.is_empty() { 0 } else { head.width() + 2 };
        // Keep the branch only while the title still gets a readable minimum beside it.
        let head_w =
            if w.saturating_sub(lead_tabs + chip_w + 2 + head_w) >= 8 { head_w } else { 0 };
        // The title fills the gap left of the branch + chip, right-aligned (a leading pad).
        let name =
            truncate_width(&s.title, w.saturating_sub(lead_tabs + chip_w + 2 + head_w).max(4));
        let pad = w.saturating_sub(lead_tabs + name.width() + head_w + 2 + chip_w);
        spans.push(Span::styled(" ".repeat(pad), bar));
        spans.push(Span::styled(name, bar.fg(p.subtext0)));
        if head_w > 0 {
            spans.push(Span::styled("  ", bar));
            spans.push(Span::styled(head, bar.fg(p.overlay0)));
        }
        spans.push(Span::styled("  ", bar));
        spans.push(Span::styled(status, bar.fg(color).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(" ", bar));
        spans.push(Span::styled(number, bar.fg(p.yellow).add_modifier(Modifier::BOLD)));
        // The arrow shares the PR number's colour, reading as part of the clickable chip.
        spans.push(Span::styled(" ↗", bar.fg(p.yellow)));
    }

    // Fill the rest of the bar (the Pr arm already reaches the right edge).
    let used: usize = spans.iter().map(Span::width).sum();
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), bar));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The status chip word for a PR's lifecycle; its accent comes from [`pr_status_chip`].
fn pr_status_word(s: &forge::PrSnapshot) -> &'static str {
    match s.state {
        forge::PrState::Merged => "merged",
        forge::PrState::Closed => "closed",
        forge::PrState::Open if s.is_draft => "draft",
        forge::PrState::Open => "open",
    }
}

/// The status chip word and its theme accent, by lifecycle.
fn pr_status_chip(p: &Palette, s: &forge::PrSnapshot) -> (&'static str, Color) {
    let color = match s.state {
        forge::PrState::Merged => p.mauve,
        forge::PrState::Closed => p.red,
        forge::PrState::Open if s.is_draft => p.yellow,
        forge::PrState::Open => p.green,
    };
    (pr_status_word(s), color)
}

/// The display width of the header's `status #number ↗` chip — shared by the painter and the
/// click hit-test so they agree on its right-anchored column range.
fn pr_chip_width(s: &forge::PrSnapshot) -> usize {
    pr_status_word(s).width()
        + " ".width()
        + format!("{}{}", s.provider.number_prefix(), s.number).width()
        + " ↗".width()
}

/// The PR's merge, sync, and checks status for the footer, joined by `·`. Merge and sync show
/// only for an open PR — they are meaningless once it is merged or closed.
fn pr_state_line(s: &forge::PrSnapshot) -> String {
    let mut parts: Vec<String> = Vec::new();
    if s.state == forge::PrState::Open {
        match s.merge {
            forge::Merge::Conflicting => parts.push(format!("⚠ conflicts with {}", s.base_ref)),
            forge::Merge::Blocked => parts.push("blocked".into()),
            forge::Merge::Clean => {}
        }
        match s.sync {
            forge::Sync::Unpushed(n) => parts.push(format!("⇡ {n} unpushed")),
            forge::Sync::Behind(n) => parts.push(format!("⇣ {n} behind")),
            forge::Sync::Unknown => parts.push("? sync unknown".to_string()),
            forge::Sync::InSync => {}
        }
    }
    parts.push(checks_summary(s));
    parts.push(format!("{} comments", s.comments.len()));
    // An incomplete thread list is louder than the generic prefix marker: it names whether
    // the walk failed or ran out of budget, so a partial list never reads as complete.
    match &s.threads_partial {
        Some(forge::PartialReason::PageFailed(_)) => {
            parts.push("⚠ some threads failed to load — r to retry".into());
        }
        Some(forge::PartialReason::Capped) => {
            parts.push(format!("thread list capped — more on {} ↗", s.provider.forge_name()));
        }
        None => {
            // A capped surface means the lists are a prefix; point at the forge for the rest
            // rather than showing the partial counts as if complete.
            if s.truncated {
                parts.push(format!("+more on {} ↗", s.provider.forge_name()));
            }
        }
    }
    parts.join(" · ")
}

/// The explicit partial-reply marker for one thread card, or `None` for a complete chain.
/// Wording distinguishes a failed walk (retryable) from a budget cap so a prefix can never
/// masquerade as the whole conversation.
fn partial_replies_label(
    comment: &forge::Comment,
    p: &crate::theme::Palette,
) -> Option<(String, ratatui::style::Color)> {
    match &comment.replies_state {
        forge::RepliesState::Complete => None,
        forge::RepliesState::Partial { missing, reason } => Some(match reason {
            forge::PartialReason::PageFailed(_) => {
                (format!("⚠ {missing} more replies failed to load — r to retry"), p.red)
            }
            forge::PartialReason::Capped => {
                (format!("+{missing} more replies not loaded — r to reload"), p.overlay1)
            }
        }),
    }
}

/// A one-token checks summary for the footer (`✓ checks` / `✗ N failing` / `● running`).
fn checks_summary(s: &forge::PrSnapshot) -> String {
    match s.checks_rollup() {
        None => "no checks".into(),
        Some(forge::CheckStatus::Failure) => format!("✗ {} failing", s.failing_checks()),
        Some(forge::CheckStatus::Running) => "● checks running".into(),
        Some(_) => "✓ checks".into(),
    }
}

/// The right navigator: the checks list above the newest-first comments list, with the cursor
/// row filled and the view windowed to keep it on screen.
fn render_pr_nav(frame: &mut Frame, app: &App, area: Rect) {
    // The navigator over the PR's checks and comments. Identity lives in the header; the left
    // pane reads the selected comment — so this pane names its contents, not "PR" again.
    let p = app.palette();
    let block = bordered("Checks & comments", true, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(s) = app.pr_snapshot() else {
        // The empty/degraded message lives once, in the read pane; this navigator stays blank.
        return;
    };
    let width = inner.width as usize;
    let dim = Style::default().fg(p.overlay0);
    let now = std::time::SystemTime::now();

    // (row spans, is the navigator cursor on this row). Only comment rows are selectable; the
    // checks section is a status display.
    let mut rows: Vec<(Vec<Span<'static>>, bool)> = Vec::new();
    rows.push((vec![Span::styled(pr_checks_header(s), dim)], false));
    for c in &s.checks {
        let (glyph, color) = check_glyph(p, c.status);
        rows.push((
            vec![
                Span::styled(format!(" {glyph} "), Style::default().fg(color)),
                Span::styled(c.name.clone(), text_style(p)),
            ],
            false,
        ));
    }
    rows.push((Vec::new(), false));
    rows.push((vec![Span::styled(format!("comments · {}", s.comments.len()), dim)], false));
    debug_assert_eq!(
        rows.len(),
        pr_nav_comment_offset(s),
        "pr_nav_hit's offset must mirror the rows painted above the comments"
    );
    for (j, cm) in s.comments.iter().enumerate() {
        rows.push((pr_comment_row(cm, width, now, p), app.pr_cursor == j));
    }

    let viewport = inner.height as usize;
    let selected = rows.iter().position(|(_, sel)| *sel).unwrap_or(0);
    let scroll = selected.saturating_sub(viewport.saturating_sub(1));
    let items: Vec<ListItem> = rows
        .into_iter()
        .skip(scroll)
        .take(viewport)
        .map(|(spans, sel)| selectable_row(spans, width, sel.then(|| p.cursor_bg(true))))
        .collect();
    frame.render_widget(List::new(items), inner);
}

/// The `checks` section header with its rollup (`✗ 1 failing` / `✓ N passed` / `running`).
fn pr_checks_header(s: &forge::PrSnapshot) -> String {
    match s.checks_rollup() {
        None => "checks  none".into(),
        Some(forge::CheckStatus::Failure) => format!("checks  ✗ {} failing", s.failing_checks()),
        Some(forge::CheckStatus::Running) => "checks  running".into(),
        Some(_) => format!("checks  ✓ {} passed", s.checks.len()),
    }
}

/// One comment row: `@author anchor`, then a trailing `resolved`/`outdated` marker or the age.
fn pr_comment_row(
    cm: &forge::Comment,
    width: usize,
    now: std::time::SystemTime,
    p: &Palette,
) -> Vec<Span<'static>> {
    let author_color = if cm.author_is_bot { p.overlay1 } else { p.peach };
    let trailing = if cm.is_resolved {
        "resolved".to_string()
    } else if cm.is_outdated {
        "outdated".to_string()
    } else {
        forge::relative_age(&cm.created_at, now)
    };
    let author = format!("@{} ", cm.author);
    let budget = width.saturating_sub(author.width() + trailing.width() + 3).max(1);
    let anchor = elide_head(&cm.anchor, budget);
    vec![
        Span::styled(author, Style::default().fg(author_color)),
        Span::styled(anchor, text_style(p)),
        Span::styled(format!("  {trailing}"), Style::default().fg(p.overlay0)),
    ]
}

/// The left read pane: the selected comment's hunk (for a finding) then its body, a check's
/// open hint, or the loading/degraded message.
fn render_pr_read(frame: &mut Frame, app: &App, area: Rect) {
    let p = app.palette();
    let selected = app.pr_selected_comment();
    // The unit label follows the snapshot's provider: a GitLab origin reads "MR" here.
    let unit = app.pr_snapshot().map_or("PR", |s| s.provider.unit());
    let title = match selected {
        Some(cm) => format!("@{} · {}", cm.author, cm.anchor),
        None => unit.to_string(),
    };
    let block = bordered(&title, false, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(notice) = app.pr_notice() {
        lines.push(Line::from(Span::styled(notice.to_owned(), Style::default().fg(p.yellow))));
        lines.push(Line::raw(""));
    } else if app.pr_refreshing() {
        lines.push(Line::from(Span::styled("refreshing…", Style::default().fg(p.overlay0))));
        lines.push(Line::raw(""));
    }

    if let Some(cm) = selected {
        if let Some(context) = app.pr_selected_context() {
            for (row, focused) in context {
                let raw = if let Row::PatchGap { lines } = &row {
                    format!("     ⋯ {lines} lines omitted")
                } else {
                    let number = row.new_no().or_else(|| row.old_no()).unwrap_or_default();
                    format!("{number:>4} {}{}", row.marker(), row.text())
                };
                let color = match row.marker() {
                    '+' => p.green,
                    '-' => p.red,
                    _ => p.subtext0,
                };
                let style = if focused {
                    Style::default().fg(color).bg(p.surface1).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color)
                };
                lines.push(Line::from(Span::styled(raw, style)));
            }
            lines.push(Line::raw(""));
        } else if let Some(hunk) = &cm.snippet {
            for raw in hunk.lines() {
                let color = match raw.bytes().next() {
                    Some(b'+') => p.green,
                    Some(b'-') => p.red,
                    _ => p.overlay0,
                };
                lines.push(Line::from(Span::styled(raw.to_string(), Style::default().fg(color))));
            }
            lines.push(Line::raw(""));
        }
        for logical in cm.body.split('\n') {
            for piece in wrap_text(logical, width.max(1)) {
                lines.push(Line::from(Span::styled(piece, text_style(p))));
            }
        }
        for reply in &cm.replies {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                format!("↳ @{}", reply.author),
                Style::default().fg(p.peach).add_modifier(Modifier::BOLD),
            )));
            for logical in reply.body.split('\n') {
                for piece in wrap_text(logical, width.max(1)) {
                    lines.push(Line::from(Span::styled(piece, text_style(p))));
                }
            }
        }
        if let Some((label, color)) = partial_replies_label(cm, p) {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(label, Style::default().fg(color))));
        }
        for pending in app.pr_selected_reply_drafts() {
            lines.push(Line::raw(""));
            let (state, color) = if pending.outcome_unknown {
                ("outcome unknown — verify on forge", p.mauve)
            } else if pending.error.is_some() {
                ("sync failed", p.red)
            } else {
                ("pending sync", p.yellow)
            };
            lines.push(Line::from(Span::styled(
                format!("↳ draft · {state}"),
                Style::default().fg(color),
            )));
            for logical in pending.draft.body.split('\n') {
                for piece in wrap_text(logical, width.max(1)) {
                    lines.push(Line::from(Span::styled(piece, text_style(p))));
                }
            }
        }
    } else {
        lines
            .push(Line::from(Span::styled(pr_empty_msg(&app.pr), Style::default().fg(p.overlay0))));
    }

    // Clamp in `usize` before the `u16` cast — `pr_read_scroll` grows unbounded via the wheel,
    // so casting first could wrap a large value below the clamp.
    let scroll = app.pr_read_scroll.min(lines.len().saturating_sub(1)) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

/// The one-line message for a loading or degraded PR view, each naming what unblocks it.
/// The no-PR state names the candidate branches it queried, so a resolution that surprises
/// is inspectable rather than silent (specs/forge-host.md).
fn pr_empty_msg(view: &forge::PrView) -> String {
    if let Some(message) = view.retry_remedy() {
        return message;
    }
    match view {
        forge::PrView::Loading => "loading…".into(),
        forge::PrView::Pending | forge::PrView::Pr(_) => String::new(),
        forge::PrView::NoPr(candidates) if candidates.is_empty() => {
            "detached HEAD — check out a branch to resolve its PR".into()
        }
        forge::PrView::NoPr(candidates) => {
            format!("no PR for {} yet — push and open one, then press r", name_a_few(candidates))
        }
        forge::PrView::Ambiguous(n) => {
            format!("{n} open PRs/MRs back this branch — open one on the forge")
        }
        forge::PrView::NoCli(_)
        | forge::PrView::NotAuthed { .. }
        | forge::PrView::ApiUnreachable { .. }
        | forge::PrView::Error(_) => {
            unreachable!("retry failures returned above")
        }
        forge::PrView::NeedsSupportedOrigin => "the PR tab needs a GitHub or GitLab origin".into(),
        forge::PrView::UnsupportedHost(host) => {
            format!(
                "unsupported host {host} — authenticate it (`gh`/`glab auth login --hostname \
                 {host}`) or set `github_host`/`gitlab_host`"
            )
        }
        forge::PrView::MalformedOrigin(host) => {
            format!("malformed origin for {host} — expected a repository path")
        }
    }
}

/// Up to three names, then `+N more` — the queried candidates stay one readable line.
fn name_a_few(names: &[String]) -> String {
    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    match names.len() {
        0..=3 => shown,
        n => format!("{shown} +{} more", n - 3),
    }
}

/// Whether a click at `(col, row)` lands on the header's right-anchored `status #number ↗`
/// chip — the whole chip opens the PR.
#[must_use]
pub fn hit_pr_open(area: Rect, app: &App, col: u16, row: u16) -> bool {
    let Some(s) = app.pr_snapshot() else {
        return false;
    };
    if row != area.y {
        return false;
    }
    let chip_w = pr_chip_width(s) as u16;
    // The chip occupies the last `chip_w` columns; `saturating_sub` keeps the bound overflow-free.
    col >= area.width.saturating_sub(chip_w) && col < area.width
}

/// The comment index a click at `(col, row)` lands on, or `None` (a check, header, or blank).
/// Mirrors `render_pr_nav`'s row layout and cursor-windowed scroll; only comments are selectable.
#[must_use]
pub fn pr_nav_hit(area: Rect, app: &App, col: u16, row: u16) -> Option<usize> {
    let inner = inner_rect(panes(area, app.list_pct).files);
    if !contains(inner, col, row) {
        return None;
    }
    let s = app.pr_snapshot()?;
    // The first comment's display row, mirroring `render_pr_nav`'s layout; the view windows on
    // the selected comment exactly as the painter does.
    let first = pr_nav_comment_offset(s);
    let sel_display = first + app.pr_cursor;
    let viewport = inner.height as usize;
    let scroll = sel_display.saturating_sub(viewport.saturating_sub(1));
    let d = (row - inner.y) as usize + scroll;
    (d >= first && d - first < s.comments.len()).then(|| d - first)
}

/// The display row of the first comment in `render_pr_nav`'s navigator — past the checks header,
/// the checks themselves, a blank, and the comments header. The single home for that layout
/// offset, shared with the click hit-test so the painted rows and the hit math can't drift.
fn pr_nav_comment_offset(s: &forge::PrSnapshot) -> usize {
    s.checks.len() + 3
}

/// The status glyph and Catppuccin accent for a check.
fn check_glyph(p: &Palette, status: forge::CheckStatus) -> (&'static str, Color) {
    match status {
        forge::CheckStatus::Success => ("✓", p.green),
        forge::CheckStatus::Failure => ("✗", p.red),
        forge::CheckStatus::Running => ("●", p.yellow),
        forge::CheckStatus::Pending => ("○", p.overlay0),
        forge::CheckStatus::Skipped => ("⊘", p.overlay0),
    }
}

// --- helpers -------------------------------------------------------------------

fn bordered(title: &str, focused: bool, p: &Palette) -> Block<'static> {
    // A focused pane gets a lavender border; an unfocused one recedes to a surface tone.
    let color = if focused { p.lavender } else { p.surface2 };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title.to_string())
}

fn dim_paragraph<'a>(text: &'a str, p: &Palette) -> Paragraph<'a> {
    Paragraph::new(text).style(Style::default().fg(p.overlay0))
}

/// The theme accent for a change marker, matched to the diff's add/remove hues.
fn kind_color(p: &Palette, marker: char) -> Color {
    match marker {
        'A' | '?' => p.green,
        'D' => p.red,
        'R' => p.mauve,
        _ => p.yellow,
    }
}

/// Whether `(col, row)` falls inside `rect`.
fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// The content area inside a one-cell border.
fn inner_rect(outer: Rect) -> Rect {
    Rect {
        x: outer.x.saturating_add(1),
        y: outer.y.saturating_add(1),
        width: outer.width.saturating_sub(2),
        height: outer.height.saturating_sub(2),
    }
}

/// A `Rect` centered in `area` at `pct_x` × `pct_y` percent of its size.
fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}
