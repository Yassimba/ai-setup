//! The structured diff model: a file's changes as rows built from its old and new
//! content, syntax-highlighted, ready to paint.
//!
//! See `specs/diff-view.md`. This module is terminal-free — a `Span` carries an RGB
//! color, and `src/ui.rs` maps it to a ratatui color.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use similar::{ChangeTag, TextDiff};

use crate::highlight::Highlighter;
use crate::model::ChangedFile;

/// An 8-bit RGB color.
pub type Rgb = (u8, u8, u8);

/// A source-control decoration beside a whole-file row in `All files`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineDecoration {
    Added,
    Modified,
    Deleted,
}

/// A run of one line's text in a single color.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub text: String,
    pub color: Rgb,
}

/// A rendered diff row. Content rows (`Context`/`Deletion`/`Insertion`) are selectable
/// for comments; a `Fold` is a collapsed run of context lines it owns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Row {
    Context {
        old_no: u32,
        new_no: u32,
        spans: Vec<Span>,
    },
    Deletion {
        old_no: u32,
        spans: Vec<Span>,
        emphasis: Vec<CharRange>,
    },
    Insertion {
        new_no: u32,
        spans: Vec<Span>,
        emphasis: Vec<CharRange>,
    },
    Fold {
        lines: Vec<Row>,
    },
    /// Context omitted by a forge-provided patch. These lines are unavailable, so this row is
    /// deliberately not expandable like a local fold.
    PatchGap {
        lines: usize,
    },
}

/// A `[start, end)` run of char indices within a line, for word-level emphasis.
pub type CharRange = (u32, u32);

impl Row {
    pub fn old_no(&self) -> Option<u32> {
        match self {
            Row::Context { old_no, .. } | Row::Deletion { old_no, .. } => Some(*old_no),
            Row::Insertion { .. } | Row::Fold { .. } | Row::PatchGap { .. } => None,
        }
    }

    pub fn new_no(&self) -> Option<u32> {
        match self {
            Row::Context { new_no, .. } | Row::Insertion { new_no, .. } => Some(*new_no),
            Row::Deletion { .. } | Row::Fold { .. } | Row::PatchGap { .. } => None,
        }
    }

    pub fn spans(&self) -> &[Span] {
        match self {
            Row::Context { spans, .. }
            | Row::Deletion { spans, .. }
            | Row::Insertion { spans, .. } => spans,
            Row::Fold { .. } | Row::PatchGap { .. } => &[],
        }
    }

    /// The char ranges within this line that differ from its paired counterpart; empty
    /// on context, folds, and unpaired change lines.
    pub fn emphasis(&self) -> &[CharRange] {
        match self {
            Row::Deletion { emphasis, .. } | Row::Insertion { emphasis, .. } => emphasis,
            Row::Context { .. } | Row::Fold { .. } | Row::PatchGap { .. } => &[],
        }
    }

    /// The diff marker for this row: `' '`, `'-'`, or `'+'`; `' '` for a fold.
    pub fn marker(&self) -> char {
        match self {
            Row::Deletion { .. } => '-',
            Row::Insertion { .. } => '+',
            Row::Context { .. } | Row::Fold { .. } | Row::PatchGap { .. } => ' ',
        }
    }

    /// Whether this row anchors a comment — every kind but a fold.
    pub fn is_content(&self) -> bool {
        !matches!(self, Row::Fold { .. } | Row::PatchGap { .. })
    }

    /// The hidden line count of a fold, else 0.
    pub fn hidden(&self) -> usize {
        match self {
            Row::Fold { lines } => lines.len(),
            Row::PatchGap { lines } => *lines,
            _ => 0,
        }
    }

    /// A fold's stable identity across rebuilds: the line number of its first hidden
    /// line. `None` for any other row.
    pub fn fold_anchor(&self) -> Option<u32> {
        match self {
            Row::Fold { lines } => lines.first().and_then(|r| r.new_no().or_else(|| r.old_no())),
            Row::PatchGap { .. }
            | Row::Context { .. }
            | Row::Deletion { .. }
            | Row::Insertion { .. } => None,
        }
    }

    /// The line's plain text, joined from its spans.
    pub fn text(&self) -> String {
        self.spans().iter().map(|s| s.text.as_str()).collect()
    }

    /// The line as a marker-prefixed diff line, for the export snippet.
    pub fn marker_text(&self) -> String {
        format!("{}{}", self.marker(), self.text())
    }
}

/// How a range endpoint's line changed — decides GitLab's `line_range` `type` marker
/// (`new` / `old` / null).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointKind {
    Added,
    Removed,
    Context,
}

/// One endpoint of a ranged comment as a unified-diff parser sees it. Every diff line has
/// *both* position counters — an added line still carries the old side's counter (the next
/// unconsumed old line), a removed line the new side's — and GitLab's line codes
/// (`sha1(path)_<old>_<new>`) need the pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeEndpoint {
    pub old_pos: u32,
    pub new_pos: u32,
    pub kind: EndpointKind,
}

/// The parser-position pair for the content row numbered `line` on `side`.
///
/// A [`Row`] keeps only the numbers of the side(s) its line exists on, so this re-walks the
/// rows exactly as a unified-diff parser would: a context row advances both counters, a change
/// row advances its own side (the other side's counter is that row's missing position), a fold
/// walks its hidden lines, and a patch gap (context unavailable between forge hunks) advances
/// both counters equally — an inter-hunk region is unchanged, so its two spans match. A side
/// that appears nowhere starts at 0, matching the `-0,0` / `+0,0` hunk of a purely added or
/// deleted file; otherwise at 1.
#[must_use]
pub fn range_endpoint(rows: &[Row], side: crate::model::Side, line: u32) -> Option<RangeEndpoint> {
    fn side_exists(rows: &[Row], pick: fn(&Row) -> Option<u32>) -> bool {
        rows.iter().any(|row| match row {
            Row::Fold { lines } => side_exists(lines, pick),
            row => pick(row).is_some(),
        })
    }
    fn walk(
        rows: &[Row],
        side: crate::model::Side,
        line: u32,
        old_next: &mut u32,
        new_next: &mut u32,
    ) -> Option<RangeEndpoint> {
        use crate::model::Side;
        for row in rows {
            match row {
                Row::Context { old_no, new_no, .. } => {
                    (*old_next, *new_next) = (*old_no, *new_no);
                    let hit = match side {
                        Side::New => *new_no == line,
                        Side::Old => *old_no == line,
                    };
                    if hit {
                        return Some(RangeEndpoint {
                            old_pos: *old_no,
                            new_pos: *new_no,
                            kind: EndpointKind::Context,
                        });
                    }
                    *old_next += 1;
                    *new_next += 1;
                }
                Row::Deletion { old_no, .. } => {
                    *old_next = *old_no;
                    if side == Side::Old && *old_no == line {
                        return Some(RangeEndpoint {
                            old_pos: *old_no,
                            new_pos: *new_next,
                            kind: EndpointKind::Removed,
                        });
                    }
                    *old_next += 1;
                }
                Row::Insertion { new_no, .. } => {
                    *new_next = *new_no;
                    if side == Side::New && *new_no == line {
                        return Some(RangeEndpoint {
                            old_pos: *old_next,
                            new_pos: *new_no,
                            kind: EndpointKind::Added,
                        });
                    }
                    *new_next += 1;
                }
                Row::Fold { lines } => {
                    if let Some(hit) = walk(lines, side, line, old_next, new_next) {
                        return Some(hit);
                    }
                }
                Row::PatchGap { lines } => {
                    *old_next = old_next.saturating_add(*lines as u32);
                    *new_next = new_next.saturating_add(*lines as u32);
                }
            }
        }
        None
    }
    let mut old_next = u32::from(side_exists(rows, Row::old_no));
    let mut new_next = u32::from(side_exists(rows, Row::new_no));
    walk(rows, side, line, &mut old_next, &mut new_next)
}

/// Whether the file renders as rows, or a notice instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileState {
    Normal,
    Binary,
    TooLarge,
    /// The forge omitted this file's patch (commonly a binary or server-side diff limit).
    Unavailable,
}

/// How the pane renders the model: the `Changes` diff, or the `All files` whole-file content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Old-versus-new with change rows and folds (specs/diff-view.md).
    Diff,
    /// The whole current file as `Context` rows, no folds — the File view.
    File,
}

/// One file from a forge's PR/MR files endpoint. The provider supplies identity and stats;
/// `patch` is the unified hunk body when the forge makes it available.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatchFile {
    pub change: ChangedFile,
    pub patch: Option<String>,
    pub too_large: bool,
}

/// A complete remote review diff, capped by the provider endpoint.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct PatchSet {
    pub files: Vec<PatchFile>,
    pub truncated: bool,
}

/// The selected file modeled as rows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileDiff {
    pub path: String,
    /// The old path when this file was renamed, for the `old → new` header; `None` otherwise.
    pub previous_path: Option<String>,
    pub state: FileState,
    pub view: View,
    pub rows: Vec<Row>,
}

/// A file beyond either budget renders as `too_large` rather than stalling the diff —
/// the byte budget also catches a single huge line that the line budget misses.
const MAX_LINES: usize = 50_000;
/// The byte budget. A file larger than this renders as a `too_large` notice.
const MAX_BYTES: usize = 2_000_000;

/// Whether a file of `len` bytes is over the size budget. `set_file_view` checks the on-disk
/// size with this before reading, so an oversize blob never loads (`app.rs`).
#[must_use]
pub fn over_byte_budget(len: usize) -> bool {
    len > MAX_BYTES
}

impl Default for FileDiff {
    fn default() -> Self {
        Self::empty()
    }
}

impl FileDiff {
    /// An empty placeholder, for when no file is selected.
    pub fn empty() -> Self {
        Self {
            path: String::new(),
            previous_path: None,
            state: FileState::Normal,
            view: View::Diff,
            rows: Vec::new(),
        }
    }

    /// Build the model from `old` and `new` content, highlighting with `hl`. `previous_path`
    /// is the rename source, surfaced in the header; `None` for every other change.
    pub fn build(
        path: String,
        previous_path: Option<String>,
        old: &str,
        new: &str,
        hl: &Highlighter,
    ) -> Self {
        let language = language_of(&path);
        let notice = |state| Self {
            path: path.clone(),
            previous_path: previous_path.clone(),
            state,
            view: View::Diff,
            rows: Vec::new(),
        };
        if old.contains('\0') || new.contains('\0') {
            return notice(FileState::Binary);
        }
        if over_byte_budget(old.len() + new.len())
            || old.lines().count() + new.lines().count() > MAX_LINES
        {
            return notice(FileState::TooLarge);
        }

        let lang = language.as_deref();
        let old_spans = hl.highlight(old, lang);
        let new_spans = hl.highlight(new, lang);
        let line = |spans: &[Vec<Span>], i: usize| spans.get(i).cloned().unwrap_or_default();

        let mut rows = Vec::new();
        for change in TextDiff::from_lines(old, new).iter_all_changes() {
            match change.tag() {
                ChangeTag::Equal => {
                    let (oi, ni) = (change.old_index().unwrap(), change.new_index().unwrap());
                    rows.push(Row::Context {
                        old_no: oi as u32 + 1,
                        new_no: ni as u32 + 1,
                        spans: line(&new_spans, ni),
                    });
                }
                ChangeTag::Delete => {
                    let oi = change.old_index().unwrap();
                    rows.push(Row::Deletion {
                        old_no: oi as u32 + 1,
                        spans: line(&old_spans, oi),
                        emphasis: Vec::new(),
                    });
                }
                ChangeTag::Insert => {
                    let ni = change.new_index().unwrap();
                    rows.push(Row::Insertion {
                        new_no: ni as u32 + 1,
                        spans: line(&new_spans, ni),
                        emphasis: Vec::new(),
                    });
                }
            }
        }
        compute_emphasis(&mut rows);
        Self {
            path,
            previous_path,
            state: FileState::Normal,
            view: View::Diff,
            rows: collapse_context(&rows),
        }
    }

    /// Build a read-only diff from a forge patch. Only returned hunks are rendered; context
    /// between hunks becomes a non-expandable [`Row::PatchGap`].
    pub fn from_patch(file: &PatchFile, hl: &Highlighter) -> Self {
        let unavailable = |state| Self {
            path: file.change.path.clone(),
            previous_path: file.change.previous_path.clone(),
            state,
            view: View::Diff,
            rows: Vec::new(),
        };
        if file.too_large {
            return unavailable(FileState::TooLarge);
        }
        let Some(patch) = file.patch.as_deref() else {
            return unavailable(FileState::Unavailable);
        };
        let Ok(rows) = patch_rows(patch, &file.change.path, hl) else {
            return unavailable(FileState::Unavailable);
        };
        Self {
            path: file.change.path.clone(),
            previous_path: file.change.previous_path.clone(),
            state: FileState::Normal,
            view: View::Diff,
            rows,
        }
    }

    /// Build the File view: the whole current `content` as `Context` rows, syntax-highlighted,
    /// with no folds, change rows, or emphasis. Powers the `All files` tab (specs/diff-view.md).
    /// Degrades to a `binary` or `too_large` notice on the same budgets as [`build`](Self::build).
    fn build_file(path: String, content: &str, hl: &Highlighter) -> Self {
        let notice = |state| Self {
            path: path.clone(),
            previous_path: None,
            state,
            view: View::File,
            rows: Vec::new(),
        };
        if content.contains('\0') {
            return notice(FileState::Binary);
        }
        if over_byte_budget(content.len()) || content.lines().count() > MAX_LINES {
            return notice(FileState::TooLarge);
        }
        let spans = hl.highlight(content, language_of(&path).as_deref());
        let rows = content
            .lines()
            .enumerate()
            .map(|(i, _)| {
                let no = i as u32 + 1;
                Row::Context {
                    old_no: no,
                    new_no: no,
                    spans: spans.get(i).cloned().unwrap_or_default(),
                }
            })
            .collect();
        Self { path, previous_path: None, state: FileState::Normal, view: View::File, rows }
    }

    /// The File-view `too_large` notice, for an over-budget file the caller declines to read.
    /// `set_file_view` checks the on-disk size and builds this rather than reading the bytes.
    pub fn too_large_notice(path: String) -> Self {
        Self {
            path,
            previous_path: None,
            state: FileState::TooLarge,
            view: View::File,
            rows: Vec::new(),
        }
    }
}

/// Parse the hunk body returned by forge file endpoints. File identity/status comes from JSON,
/// so this parser deliberately ignores `diff --git` metadata and only accepts hunk headers and
/// their content rows.
fn patch_rows(hunks: &str, path: &str, hl: &Highlighter) -> Result<Vec<Row>, ()> {
    if over_byte_budget(hunks.len()) || hunks.lines().count() > MAX_LINES {
        return Err(());
    }
    let language = language_of(path);
    let highlight =
        |text: &str| hl.highlight(text, language.as_deref()).into_iter().next().unwrap_or_default();
    let mut rows = Vec::new();
    let (mut old_no, mut new_no) = (1_u32, 1_u32);
    let mut in_hunk = false;
    for line in hunks.lines() {
        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            let old_gap = old_start.saturating_sub(old_no);
            let new_gap = new_start.saturating_sub(new_no);
            let gap = old_gap.max(new_gap) as usize;
            if gap > 0 {
                rows.push(Row::PatchGap { lines: gap });
            }
            old_no = old_start;
            new_no = new_start;
            in_hunk = true;
            continue;
        }
        if !in_hunk || line == "\\ No newline at end of file" {
            continue;
        }
        let Some((marker, text)) = line.split_at_checked(1) else { continue };
        match marker {
            " " => {
                rows.push(Row::Context { old_no, new_no, spans: highlight(text) });
                old_no = old_no.saturating_add(1);
                new_no = new_no.saturating_add(1);
            }
            "-" => {
                rows.push(Row::Deletion { old_no, spans: highlight(text), emphasis: Vec::new() });
                old_no = old_no.saturating_add(1);
            }
            "+" => {
                rows.push(Row::Insertion { new_no, spans: highlight(text), emphasis: Vec::new() });
                new_no = new_no.saturating_add(1);
            }
            _ => {}
        }
    }
    if !in_hunk {
        return Err(());
    }
    compute_emphasis(&mut rows);
    Ok(rows)
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    Some((parse_range_start(old)?, parse_range_start(new)?))
}

fn parse_range_start(range: &str) -> Option<u32> {
    range.split(',').next()?.parse().ok()
}

/// Fill word-level `emphasis` on the related deletion/insertion lines of each change block
/// (a run of deletions immediately followed by a run of insertions). Rather than pairing by
/// position — which mis-pairs unrelated lines when a block rewrites several lines at once —
/// each deletion greedily searches forward for its *homolog*: the first not-yet-claimed
/// insertion similar enough to be the same line edited (see [`pair_homologs`], after
/// git-delta's `infer_edits`). Lines with no homolog stay unemphasized, carrying only their
/// red/green; emphasis then points at a real edit instead of flooding a wholesale rewrite.
/// Map the current file's line numbers to editor-style source-control gutter decorations.
/// Deleted-only blocks attach to the next surviving line, or the final line at EOF.
#[must_use]
pub fn line_decorations(old: &str, new: &str) -> HashMap<u32, LineDecoration> {
    let mut out = HashMap::new();
    let mut deleted = false;
    let mut inserted = false;
    let mut last_new_line = 1_u32;
    let flush_deleted =
        |out: &mut HashMap<u32, LineDecoration>, line: u32, deleted: bool, inserted: bool| {
            if deleted && !inserted {
                out.insert(line.max(1), LineDecoration::Deleted);
            }
        };
    for change in TextDiff::from_lines(old, new).iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let line = change.new_index().unwrap_or_default() as u32 + 1;
                flush_deleted(&mut out, line, deleted, inserted);
                deleted = false;
                inserted = false;
                last_new_line = line;
            }
            ChangeTag::Delete => deleted = true,
            ChangeTag::Insert => {
                let line = change.new_index().unwrap_or_default() as u32 + 1;
                out.insert(
                    line,
                    if deleted { LineDecoration::Modified } else { LineDecoration::Added },
                );
                inserted = true;
                last_new_line = line;
            }
        }
    }
    flush_deleted(&mut out, last_new_line, deleted, inserted);
    out
}

fn compute_emphasis(rows: &mut [Row]) {
    let mut i = 0;
    while i < rows.len() {
        let del_start = i;
        while i < rows.len() && matches!(rows[i], Row::Deletion { .. }) {
            i += 1;
        }
        let ins_start = i;
        while i < rows.len() && matches!(rows[i], Row::Insertion { .. }) {
            i += 1;
        }
        pair_homologs(rows, del_start..ins_start, ins_start..i);
        // No change block started here; step over the context/fold row.
        if del_start == i {
            i += 1;
        }
    }
}

/// Pair each deletion in `dels` with its homolog insertion in `inss` and set both lines'
/// emphasis. Greedy forward scan: deletion `d` takes the first insertion at or after the
/// last-claimed one whose similarity clears [`MIN_SIMILARITY`]; insertions skipped along the
/// way are abandoned (they were inserts, not edits of `d`). A deletion with no qualifying
/// insertion is left unpaired. Mirrors git-delta's homolog inference.
fn pair_homologs(rows: &mut [Row], dels: std::ops::Range<usize>, inss: std::ops::Range<usize>) {
    let mut next_ins = inss.start;
    for d in dels {
        let old = rows[d].text();
        let mut p = next_ins;
        while p < inss.end {
            let new = rows[p].text();
            let (ratio, old_e, new_e) = word_emphasis(&old, &new);
            if ratio >= MIN_SIMILARITY {
                if let Row::Deletion { emphasis, .. } = &mut rows[d] {
                    *emphasis = old_e;
                }
                if let Row::Insertion { emphasis, .. } = &mut rows[p] {
                    *emphasis = new_e;
                }
                next_ins = p + 1;
                break;
            }
            p += 1;
        }
    }
}

/// Two lines below this similarity are taken to be different lines, not one line edited, so
/// they are never paired for inline emphasis (see [`pair_homologs`]). git-delta's equivalent
/// `max_line_distance` defaults to 0.6 *distance* — the complementary metric — but we sit
/// stricter because over-highlighting a rewrite is worse than missing a marginal edit: a pair
/// that only shares a syntactic skeleton (a reformat, or two different `let`s) scatters
/// unhelpful fragments. Empirically marginal pairs land near ~0.6–0.65 and genuine edits near
/// ~0.71–0.78, so the bar sits in the gap.
const MIN_SIMILARITY: f32 = 0.7;

/// The word-level similarity of `(old, new)` and the char ranges that changed: the words
/// present only in `old` (deletion emphasis) and only in `new` (insertion emphasis). The
/// caller gates on the ratio (see [`pair_homologs`]); the ranges are meaningful only once it
/// clears the bar. Adjacent changed words separated only by whitespace coalesce into one
/// block (see [`coalesce_ws_gaps`]), so a changed phrase reads as a single span, not fragments.
fn word_emphasis(old: &str, new: &str) -> (f32, Vec<CharRange>, Vec<CharRange>) {
    let diff = TextDiff::from_words(old, new);
    let (mut old_ranges, mut new_ranges) = (Vec::new(), Vec::new());
    let (mut old_pos, mut new_pos) = (0u32, 0u32);
    for change in diff.iter_all_changes() {
        let len = change.value().chars().count() as u32;
        match change.tag() {
            ChangeTag::Equal => {
                old_pos += len;
                new_pos += len;
            }
            ChangeTag::Delete => {
                push_range(&mut old_ranges, old_pos, len);
                old_pos += len;
            }
            ChangeTag::Insert => {
                push_range(&mut new_ranges, new_pos, len);
                new_pos += len;
            }
        }
    }
    let old_e = trim_range_edges(coalesce_ws_gaps(old_ranges, old), old);
    let new_e = trim_range_edges(coalesce_ws_gaps(new_ranges, new), new);
    (diff.ratio(), old_e, new_e)
}

/// Shrink each emphasis range off its leading and trailing whitespace, dropping any range
/// that is all whitespace. So a highlight hugs the changed tokens — never bare indentation
/// (a reformat that only deepened the indent) and never the space before an added trailing
/// comment. Interior whitespace from [`coalesce_ws_gaps`] survives, since it is not an edge.
fn trim_range_edges(ranges: Vec<CharRange>, text: &str) -> Vec<CharRange> {
    let chars: Vec<char> = text.chars().collect();
    ranges
        .into_iter()
        .filter_map(|(mut a, mut b)| {
            while a < b && chars[a as usize].is_whitespace() {
                a += 1;
            }
            while b > a && chars[b as usize - 1].is_whitespace() {
                b -= 1;
            }
            (a < b).then_some((a, b))
        })
        .collect()
}

/// Merge consecutive emphasis ranges whose in-between text is all whitespace, swallowing
/// that whitespace into the highlight. A run of changed words then shows as one block
/// rather than separate words with bare gaps; leading and trailing indentation, and gaps
/// holding any non-space character, are left out because they never sit between two changes.
fn coalesce_ws_gaps(ranges: Vec<CharRange>, text: &str) -> Vec<CharRange> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<CharRange> = Vec::new();
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last)
                if chars[last.1 as usize..start as usize].iter().all(|c| c.is_whitespace()) =>
            {
                last.1 = end;
            }
            _ => out.push((start, end)),
        }
    }
    out
}

/// Append `[pos, pos+len)`, merging into the previous range when they touch.
fn push_range(ranges: &mut Vec<CharRange>, pos: u32, len: u32) {
    if len == 0 {
        return;
    }
    match ranges.last_mut() {
        Some(last) if last.1 == pos => last.1 = pos + len,
        _ => ranges.push((pos, pos + len)),
    }
}

/// Context lines kept adjacent to each change; longer unchanged runs collapse to a fold.
const FOLD_MARGIN: usize = 3;

/// Replace each run of unchanged `Context` rows that exceeds the margin with a single
/// `Fold` owning the hidden rows, keeping `FOLD_MARGIN` lines next to every change and
/// at the file head and tail.
fn collapse_context(rows: &[Row]) -> Vec<Row> {
    let n = rows.len();
    let mut keep = vec![false; n];
    for (i, row) in rows.iter().enumerate() {
        if matches!(row, Row::Context { .. }) {
            continue;
        }
        let lo = i.saturating_sub(FOLD_MARGIN);
        let hi = (i + FOLD_MARGIN).min(n - 1);
        keep[lo..=hi].iter_mut().for_each(|k| *k = true);
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if keep[i] {
            out.push(rows[i].clone());
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !keep[i] {
            i += 1;
        }
        // A single hidden line is shown as-is — a `⋯ 1 line` fold would save nothing.
        if i - start > 1 {
            out.push(Row::Fold { lines: rows[start..i].to_vec() });
        } else {
            out.extend(rows[start..i].iter().cloned());
        }
    }
    out
}

/// The extension used to pick a syntax, e.g. `rs` for `src/app.rs`; `None` when the
/// file name has no extension.
fn language_of(path: &str) -> Option<String> {
    Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_string)
}

/// Caches built `FileDiff`s by content, so an unchanged poll skips diffing and
/// highlighting.
#[derive(Default, Debug)]
pub struct DiffCache {
    entries: HashMap<String, (u64, FileDiff)>,
}

/// Cap the cache so a long session browsing many files cannot grow it without bound;
/// at the cap it is cleared (only the open file is ever rebuilt).
const CACHE_CAP: usize = 256;

impl DiffCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached diff when `old`/`new`/`previous_path` are unchanged for `path`,
    /// else build, cache, and return it. `previous_path` (the rename source) is part of the
    /// key, so the same path appearing as a rename in one scope and plain in another never
    /// returns a stale header; the cache is also cleared on a scope switch (`set_scope`).
    pub fn get(
        &mut self,
        path: String,
        previous_path: Option<String>,
        old: &str,
        new: &str,
        hl: &Highlighter,
    ) -> FileDiff {
        let key = content_hash(previous_path.as_deref(), old, new);
        self.get_or_build(path.clone(), key, || FileDiff::build(path, previous_path, old, new, hl))
    }

    /// Return the cached File view when `content` is unchanged for `path`, else build it.
    /// File-view entries are namespaced under a `file:` key so a path's File view and Diff
    /// view coexist in the cache instead of evicting each other on a tab switch.
    pub fn get_file(&mut self, path: String, content: &str, hl: &Highlighter) -> FileDiff {
        let key = content_hash(None, content, content);
        self.get_or_build(format!("file:{path}"), key, || FileDiff::build_file(path, content, hl))
    }

    /// Shared cache body: return the entry under `cache_key` when its stored hash still equals
    /// `content_key`, else `build` it, evict-on-cap, and insert. Both [`get`](Self::get) and
    /// [`get_file`](Self::get_file) differ only in the key and the build call.
    fn get_or_build(
        &mut self,
        cache_key: String,
        content_key: u64,
        build: impl FnOnce() -> FileDiff,
    ) -> FileDiff {
        if let Some((cached, diff)) = self.entries.get(&cache_key)
            && *cached == content_key
        {
            return diff.clone();
        }
        let diff = build();
        if self.entries.len() >= CACHE_CAP {
            self.entries.clear();
        }
        self.entries.insert(cache_key, (content_key, diff.clone()));
        diff
    }
}

fn content_hash(previous_path: Option<&str>, old: &str, new: &str) -> u64 {
    let mut h = DefaultHasher::new();
    previous_path.hash(&mut h);
    old.hash(&mut h);
    new.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::{
        DiffCache, FileDiff, FileState, LineDecoration, PatchFile, RangeEndpoint, Row, View,
        language_of, line_decorations,
    };
    use crate::highlight::Highlighter;
    use crate::model::{ChangeKind, ChangedFile};
    use crate::theme;

    /// The default theme's syntax pairing (bundled Catppuccin Mocha), for highlighter setup.
    fn mocha() -> crate::theme::SyntaxChoice {
        theme::resolve(Some("catppuccin")).syntax
    }

    #[test]
    fn whole_file_decorations_mark_added_modified_and_deleted_blocks() {
        let marks = line_decorations("one\ntwo\nthree\n", "one\nchanged\nthree\nadded\n");
        assert_eq!(marks.get(&2), Some(&LineDecoration::Modified));
        assert_eq!(marks.get(&4), Some(&LineDecoration::Added));

        let deleted = line_decorations("one\ntwo\nthree\n", "one\nthree\n");
        assert_eq!(deleted.get(&2), Some(&LineDecoration::Deleted));
    }

    #[test]
    fn file_view_is_all_context_with_no_folds() {
        use std::fmt::Write as _;
        let hl = Highlighter::new(mocha());
        let mut content = String::new();
        for i in 0..40 {
            writeln!(content, "line {i}").unwrap();
        }
        let d = FileDiff::build_file("a.rs".into(), &content, &hl);
        assert_eq!(d.view, View::File);
        assert_eq!(d.state, FileState::Normal);
        assert_eq!(d.rows.len(), 40);
        assert!(d.rows.iter().all(|r| matches!(r, Row::Context { .. })), "every row is context");
        // Even a long unchanged run never folds in the File view.
        assert!(!d.rows.iter().any(|r| matches!(r, Row::Fold { .. })));
        assert_eq!(d.rows[0].new_no(), Some(1));
        assert_eq!(d.rows[39].new_no(), Some(40));
    }

    #[test]
    fn file_view_degrades_on_binary() {
        let hl = Highlighter::new(mocha());
        let d = FileDiff::build_file("blob.bin".into(), "a\0b", &hl);
        assert_eq!(d.state, FileState::Binary);
        assert_eq!(d.view, View::File);
        assert!(d.rows.is_empty());
    }

    #[test]
    fn forge_patch_preserves_line_numbers_and_non_expandable_hunk_gaps() {
        let file = PatchFile {
            change: ChangedFile {
                path: "src/a.rs".into(),
                kind: ChangeKind::Modified,
                additions: 2,
                deletions: 2,
                previous_path: None,
            },
            patch: Some(
                "@@ -2,2 +2,2 @@\n same\n-old\n+new\n@@ -10,1 +10,1 @@\n-tail\n+tip\n".into(),
            ),
            too_large: false,
        };
        let diff = FileDiff::from_patch(&file, &Highlighter::new(mocha()));
        assert_eq!(diff.state, FileState::Normal);
        assert!(matches!(diff.rows[0], Row::PatchGap { lines: 1 }));
        assert!(matches!(diff.rows[1], Row::Context { old_no: 2, new_no: 2, .. }));
        assert!(matches!(diff.rows[2], Row::Deletion { old_no: 3, .. }));
        assert!(matches!(diff.rows[3], Row::Insertion { new_no: 3, .. }));
        let gap = diff.rows.iter().find(|row| matches!(row, Row::PatchGap { lines: 6 })).unwrap();
        assert_eq!(gap.fold_anchor(), None, "remote omissions cannot be expanded");
    }

    #[test]
    fn range_endpoints_reconstruct_both_parser_counters() {
        use super::{EndpointKind, range_endpoint};
        use crate::model::Side;
        let file = PatchFile {
            change: ChangedFile {
                path: "src/a.rs".into(),
                kind: ChangeKind::Modified,
                additions: 2,
                deletions: 2,
                previous_path: None,
            },
            patch: Some(
                "@@ -2,3 +2,4 @@\n same\n-old\n+new\n+extra\n more\n@@ -10,1 +11,1 @@\n-tail\n"
                    .into(),
            ),
            too_large: false,
        };
        let diff = FileDiff::from_patch(&file, &Highlighter::new(mocha()));
        // A context line carries its own two numbers.
        assert_eq!(
            range_endpoint(&diff.rows, Side::New, 2),
            Some(RangeEndpoint { old_pos: 2, new_pos: 2, kind: EndpointKind::Context })
        );
        // An added line keeps the old side's running counter (the next unconsumed old line).
        assert_eq!(
            range_endpoint(&diff.rows, Side::New, 3),
            Some(RangeEndpoint { old_pos: 4, new_pos: 3, kind: EndpointKind::Added })
        );
        assert_eq!(
            range_endpoint(&diff.rows, Side::New, 4),
            Some(RangeEndpoint { old_pos: 4, new_pos: 4, kind: EndpointKind::Added })
        );
        // A removed line keeps the new side's counter; the second hunk's counters resync
        // across the patch gap even though its old/new starts differ.
        assert_eq!(
            range_endpoint(&diff.rows, Side::Old, 10),
            Some(RangeEndpoint { old_pos: 10, new_pos: 11, kind: EndpointKind::Removed })
        );
        assert_eq!(range_endpoint(&diff.rows, Side::New, 99), None);
    }

    #[test]
    fn range_endpoints_pin_an_absent_side_at_zero() {
        use super::{EndpointKind, range_endpoint};
        use crate::model::Side;
        let added = PatchFile {
            change: ChangedFile {
                path: "fresh.rs".into(),
                kind: ChangeKind::Added,
                additions: 2,
                deletions: 0,
                previous_path: None,
            },
            patch: Some("@@ -0,0 +1,2 @@\n+one\n+two\n".into()),
            too_large: false,
        };
        let diff = FileDiff::from_patch(&added, &Highlighter::new(mocha()));
        // A purely added file diffs against nothing: GitLab's parser holds the old counter
        // at the `-0,0` hunk start for every added line.
        assert_eq!(
            range_endpoint(&diff.rows, Side::New, 2),
            Some(RangeEndpoint { old_pos: 0, new_pos: 2, kind: EndpointKind::Added })
        );

        let deleted = PatchFile {
            change: ChangedFile {
                path: "gone.rs".into(),
                kind: ChangeKind::Deleted,
                additions: 0,
                deletions: 2,
                previous_path: None,
            },
            patch: Some("@@ -1,2 +0,0 @@\n-one\n-two\n".into()),
            too_large: false,
        };
        let diff = FileDiff::from_patch(&deleted, &Highlighter::new(mocha()));
        assert_eq!(
            range_endpoint(&diff.rows, Side::Old, 1),
            Some(RangeEndpoint { old_pos: 1, new_pos: 0, kind: EndpointKind::Removed })
        );
    }

    #[test]
    fn range_endpoints_walk_fold_interiors() {
        use super::{EndpointKind, range_endpoint};
        use crate::model::Side;
        use std::fmt::Write as _;
        // A local diff collapses long context runs into folds; the walk must keep both
        // counters advancing through the hidden interior.
        let mut old = String::new();
        for i in 1..=30 {
            writeln!(old, "line {i}").unwrap();
        }
        let new = old.replace("line 28", "LINE 28");
        let d = build(&old, &new);
        assert!(d.rows.iter().any(|r| matches!(r, Row::Fold { .. })), "long context folds");
        // The rewritten line pairs a deletion (old 28) with an insertion (new 28); the
        // deletion consumes old 28 first, so the insertion's old counter is already 29.
        assert_eq!(
            range_endpoint(&d.rows, Side::New, 28),
            Some(RangeEndpoint { old_pos: 29, new_pos: 28, kind: EndpointKind::Added })
        );
        // A line hidden inside the fold still resolves.
        assert_eq!(
            range_endpoint(&d.rows, Side::New, 10),
            Some(RangeEndpoint { old_pos: 10, new_pos: 10, kind: EndpointKind::Context })
        );
    }

    #[test]
    fn missing_or_too_large_forge_patch_has_an_explicit_notice_state() {
        let change = ChangedFile {
            path: "asset.bin".into(),
            kind: ChangeKind::Modified,
            additions: 0,
            deletions: 0,
            previous_path: None,
        };
        let hl = Highlighter::new(mocha());
        let unavailable = FileDiff::from_patch(
            &PatchFile { change: change.clone(), patch: None, too_large: false },
            &hl,
        );
        assert_eq!(unavailable.state, FileState::Unavailable);
        let large = FileDiff::from_patch(
            &PatchFile { change, patch: Some("@@ -1 +1 @@\n-a\n+b".into()), too_large: true },
            &hl,
        );
        assert_eq!(large.state, FileState::TooLarge);
    }

    fn build(old: &str, new: &str) -> FileDiff {
        let hl = Highlighter::new(mocha());
        FileDiff::build("a.rs".into(), None, old, new, &hl)
    }

    #[test]
    fn cache_keys_on_previous_path_so_a_rename_and_a_plain_edit_differ() {
        let hl = Highlighter::new(mocha());
        let mut cache = DiffCache::new();
        // Same path + same content, but one is a rename (carries a previous_path) and one is
        // not. The cache must not return the rename's build for the plain edit.
        let renamed = cache.get("f.rs".into(), Some("old.rs".into()), "x\n", "y\n", &hl);
        let plain = cache.get("f.rs".into(), None, "x\n", "y\n", &hl);
        assert_eq!(renamed.previous_path.as_deref(), Some("old.rs"));
        assert_eq!(plain.previous_path, None);
    }

    #[test]
    fn rows_carry_sides_numbers_and_markers() {
        let d = build("alpha\nbeta\ngamma\n", "alpha\nBETA\ngamma\n");
        assert_eq!(d.state, FileState::Normal);
        let del = d.rows.iter().find(|r| matches!(r, Row::Deletion { .. })).unwrap();
        let ins = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        assert_eq!(del.old_no(), Some(2));
        assert_eq!(del.new_no(), None);
        assert_eq!(ins.new_no(), Some(2));
        assert_eq!(del.marker_text(), "-beta");
        assert_eq!(ins.marker_text(), "+BETA");
        // The whole file is shown — context rows surround the change.
        assert!(d.rows.iter().filter(|r| matches!(r, Row::Context { .. })).count() >= 2);
    }

    #[test]
    fn long_unchanged_runs_collapse_to_a_fold() {
        use std::fmt::Write as _;
        let mut old = String::new();
        for i in 0..40 {
            writeln!(old, "line {i}").unwrap();
        }
        let new = old.replace("line 20", "LINE 20");
        let d = build(&old, &new);
        // The middle is one change with 3 context lines each side; the long head and tail
        // unchanged runs each collapse to a fold.
        let folds = d.rows.iter().filter(|r| matches!(r, Row::Fold { .. })).count();
        assert_eq!(folds, 2, "leading and trailing runs fold");
        let change = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        assert_eq!(change.new_no(), Some(21)); // line 20 is 1-based line 21
    }

    #[test]
    fn word_emphasis_marks_only_the_changed_words() {
        let d = build("let x = foo(a);\n", "let x = bar(a, b);\n");
        let del = d.rows.iter().find(|r| matches!(r, Row::Deletion { .. })).unwrap();
        let ins = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        // Both lines share the `let x = ` and `(a` prefix; `foo`→`bar` and the `, b` are
        // the only emphasized spans, never the whole line.
        assert!(!del.emphasis().is_empty() && !ins.emphasis().is_empty());
        let covers = |row: &Row, needle: &str| {
            let text = row.text();
            row.emphasis().iter().any(|&(a, b)| {
                let seg: String = text.chars().skip(a as usize).take((b - a) as usize).collect();
                seg.contains(needle)
            })
        };
        assert!(covers(del, "foo"), "deletion emphasizes the removed word");
        assert!(covers(ins, "bar"), "insertion emphasizes the new word");
        // `let x = ` is shared, so it is never emphasized.
        assert!(!covers(del, "let"));
    }

    #[test]
    fn adjacent_changed_words_coalesce_across_whitespace() {
        // `Hi You` → `Hello There`: two changed words split by a space. The emphasis is a
        // single block spanning the space, not two fragments — the Word-Alt look.
        let d = build("greet Hi You here\n", "greet Hello There here\n");
        let del = d.rows.iter().find(|r| matches!(r, Row::Deletion { .. })).unwrap();
        let ins = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        let seg = |row: &Row, &(a, b): &(u32, u32)| -> String {
            row.text().chars().skip(a as usize).take((b - a) as usize).collect()
        };
        assert_eq!(del.emphasis().len(), 1, "the removed phrase is one block");
        assert_eq!(seg(del, &del.emphasis()[0]), "Hi You");
        assert_eq!(ins.emphasis().len(), 1, "the new phrase is one block");
        assert_eq!(seg(ins, &ins.emphasis()[0]), "Hello There");
    }

    #[test]
    fn emphasis_pairs_a_deletion_with_its_homolog_not_its_position() {
        // One line edited, with a new line inserted above it. Positional pairing would pair
        // the deletion with the inserted comment (dissimilar → nothing); homolog search skips
        // the comment and pairs with the real edit, so `compute`→`computeSum` lights up and
        // the unrelated inserted line stays plain.
        let d = build("let total = compute();\n", "// added\nlet total = computeSum();\n");
        let seg = |row: &Row| -> String {
            let (a, b) = row.emphasis()[0];
            row.text().chars().skip(a as usize).take((b - a) as usize).collect()
        };
        let del = d.rows.iter().find(|r| matches!(r, Row::Deletion { .. })).unwrap();
        let comment = d.rows.iter().find(|r| r.text() == "// added").unwrap();
        let edited = d.rows.iter().find(|r| r.text() == "let total = computeSum();").unwrap();
        assert_eq!(seg(del), "compute();", "the deletion emphasizes its real edit");
        assert_eq!(seg(edited), "computeSum();", "its homolog insertion is the one emphasized");
        assert!(comment.emphasis().is_empty(), "the unrelated inserted line stays plain");
    }

    #[test]
    fn emphasis_hugs_the_tokens_not_surrounding_whitespace() {
        // Adding a trailing comment: the highlight is `// note`, not ` // note` — leading
        // whitespace is trimmed off the range so it never paints bare spaces.
        let d = build("    let x = 1;\n", "    let x = 1; // note\n");
        let ins = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        assert_eq!(ins.emphasis().len(), 1);
        let (a, b) = ins.emphasis()[0];
        let seg: String = ins.text().chars().skip(a as usize).take((b - a) as usize).collect();
        assert_eq!(seg, "// note", "emphasis hugs the comment, no leading space");
    }

    #[test]
    fn a_reformat_or_unrelated_pair_is_not_emphasized() {
        // A one-liner reformatted to multi-line, and two different statements sharing a
        // `let … ;` skeleton, both fall below the similarity bar — they scatter unhelpful
        // fragments otherwise. Each keeps only its line-level red/green.
        let reformat = build(
            "    rows.push(Row::Deletion { old_no: oi + 1, spans: s });\n",
            "    rows.push(Row::Deletion {\n        old_no: oi + 1,\n        spans: s,\n    });\n",
        );
        let unrelated = build(
            "    let start = scroll.min(len.sub(height));\n",
            "    let target = (row - inner.y) as usize;\n",
        );
        for d in [reformat, unrelated] {
            assert!(
                d.rows.iter().all(|r| r.emphasis().is_empty()),
                "no inline emphasis on a sub-threshold pair"
            );
        }
    }

    #[test]
    fn a_wholesale_line_rewrite_gets_no_word_emphasis() {
        // Two unrelated lines that merely share `///` and punctuation must not light up:
        // the line-level red/green already says they changed, and full-line emphasis on a
        // dissimilar pair is noise. The similarity gate suppresses it.
        let d = build(
            "/// Keep diff_scroll so the cursor stays within the viewport\n",
            "/// Scroll the diff horizontally by delta columns\n",
        );
        let del = d.rows.iter().find(|r| matches!(r, Row::Deletion { .. })).unwrap();
        let ins = d.rows.iter().find(|r| matches!(r, Row::Insertion { .. })).unwrap();
        assert!(del.emphasis().is_empty(), "dissimilar deletion is not emphasized");
        assert!(ins.emphasis().is_empty(), "dissimilar insertion is not emphasized");
    }

    #[test]
    fn an_unpaired_change_line_has_no_emphasis() {
        // One deletion, two insertions: line 0 pairs; the extra insertion stays plain.
        let d = build("alpha\n", "ALPHA\nbeta\n");
        let extra = d
            .rows
            .iter()
            .find(|r| matches!(r, Row::Insertion { .. }) && r.text() == "beta")
            .unwrap();
        assert!(extra.emphasis().is_empty(), "the unpaired insertion is not emphasized");
    }

    #[test]
    fn binary_content_is_flagged_not_rowed() {
        let d = build("ok\n", "bin\0ary\n");
        assert_eq!(d.state, FileState::Binary);
        assert!(d.rows.is_empty());
    }

    #[test]
    fn language_comes_from_the_extension() {
        assert_eq!(language_of("src/app.rs").as_deref(), Some("rs"));
        assert_eq!(language_of("Makefile"), None);
        assert_eq!(language_of("a/b.tar.gz").as_deref(), Some("gz"));
    }

    #[test]
    fn cache_reuses_an_unchanged_build() {
        let hl = Highlighter::new(mocha());
        let mut cache = DiffCache::new();
        let d1 = cache.get("a.rs".into(), None, "x\n", "y\n", &hl);
        let d2 = cache.get("a.rs".into(), None, "x\n", "y\n", &hl);
        assert_eq!(d1, d2);
    }
}
