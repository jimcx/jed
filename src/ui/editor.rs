//! The text area: gutter (line numbers, quick diff's markers), text (syntax colours), matched
//! brackets, diagnostics' squiggles, selections, find matches, cursors, rendered whitespace,
//! merge conflicts (block backgrounds, marker labels, the action row) and the cursor line's
//! inline blame.

use std::borrow::Cow;
use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

use crate::blame;
use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::conflict::Choice;
use crate::diagnostics::{self, Severity};
use crate::edit::{CursorStyle, RenderWhitespace, SemanticHighlighting, Settings};
use crate::find::FindWidget;
use crate::layout::{MergeButton, MergeButtonHit};
use crate::search::Match;
use crate::theme::Theme;
use crate::view::View;

use super::semantic;

/// What else the editor shows besides the buffer.
pub struct Overlay<'a> {
    /// Shows the terminal cursor (the focused group, nothing else open).
    pub focused: bool,
    /// The find widget, when it's open on this view and its matches are for this text.
    pub find: Option<&'a FindWidget>,
    /// Which spaces and tabs are drawn as `·` / `→` (`App::render_whitespace`).
    pub whitespace: RenderWhitespace,
    /// Inline blame is on (`app.blame.on`): the primary cursor line's blame after its text.
    pub blame: bool,
    /// Long lines wrap at the text width (`App::word_wrap`), drawn on screen rows.
    pub wrapping: bool,
}

/// Spaces and tabs only, as VS Code's renderer (other Unicode spaces aren't whitespace here).
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The action row after a conflict's `<<<<<<<` line: its buttons' labels and what they do
/// (VS Code's Accept Current / Incoming / Both, then Compare).
const MERGE_ACTIONS: [(&str, MergeButton); 4] = [
    (" Accept Current ", MergeButton::Accept(Choice::Current)),
    (" Accept Incoming ", MergeButton::Accept(Choice::Incoming)),
    (" Accept Both ", MergeButton::Accept(Choice::Both)),
    (" Compare ", MergeButton::Compare),
];

/// Where a line's leading whitespace ends and its trailing whitespace starts (byte offsets into
/// the line; a blank line is all leading and all trailing). Only for the modes that need them.
fn whitespace_bounds(mode: RenderWhitespace, line: &str) -> (usize, usize) {
    match mode {
        RenderWhitespace::Boundary => (
            line.len() - line.trim_start_matches(is_blank).len(),
            line.trim_end_matches(is_blank).len(),
        ),
        RenderWhitespace::Trailing => (0, line.trim_end_matches(is_blank).len()),
        _ => (0, line.len()),
    }
}

/// Whether the grapheme `g` at byte `off` of `line` is whitespace drawn as `·` / `→` in `mode`
/// (VS Code's `renderWhitespace`): `bounds` from `whitespace_bounds`, `selected` whether it's in
/// a selection. `boundary` leaves out a single space between two non-whitespace chars.
fn shows_whitespace(
    mode: RenderWhitespace,
    line: &str,
    off: usize,
    g: &str,
    (lead_end, trail_start): (usize, usize),
    selected: bool,
) -> bool {
    let tab = g == "\t";
    if !tab && g != " " {
        return false;
    }
    match mode {
        RenderWhitespace::None => false,
        RenderWhitespace::All => true,
        RenderWhitespace::Selection => selected,
        RenderWhitespace::Trailing => off >= trail_start,
        RenderWhitespace::Boundary => {
            // Space and tab are ASCII, so the neighbouring bytes tell.
            let blank_at = |i: Option<usize>| {
                i.and_then(|i| line.as_bytes().get(i))
                    .is_some_and(|b| matches!(b, b' ' | b'\t'))
            };
            tab || off < lead_end || off >= trail_start || blank_at(off.checked_sub(1)) || blank_at(Some(off + 1))
        }
    }
}

/// The gutter's width for `rope`: its line numbers (at least 3 digits) and a space each side.
fn gutter_width(rope: &ropey::Rope) -> u16 {
    (pos::line_count(rope).to_string().len().max(3) + 2) as u16
}

/// The text width `render` gives `rope` in an editor area `width` columns wide (the gutter
/// excluded): what a commit tab is rendered for before it's drawn.
pub fn text_width(width: u16, rope: &ropey::Rope) -> usize {
    width.saturating_sub(gutter_width(rope)).max(1) as usize
}

/// One screen row the editor draws: a whole line, or one of its wrapped rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisualRow {
    /// The 0-based line.
    line: usize,
    /// The row within the line (0 when not wrapping, 0..`rows` when it is).
    sub: usize,
    /// How many screen rows the line has at the view's width (1 when not wrapping).
    rows: usize,
    /// The row's first byte, within the line's content (0 when not wrapping).
    start: usize,
    /// The byte past the row's last grapheme, within the line's content (0 when not wrapping).
    end: usize,
    /// The display column the row's text starts at: the indentation on continuation rows.
    col: usize,
}

/// The screen rows `view` shows, top first: while wrapping, a line's rows from `top_row`, then
/// the next line's from its first row. Only the rows laid out here are drawn (and the overlays'
/// byte range covers only them); the rows' break points come from `view.wrap`.
fn visual_rows(
    rope: &ropey::Rope,
    view: &mut View,
    revision: u64,
    wrapping: bool,
    total: usize,
    tab: usize,
) -> Vec<VisualRow> {
    let mut out = Vec::with_capacity(view.height);
    let mut line = view.top_line();
    let mut sub = if wrapping { view.top_row() } else { 0 };
    while out.len() < view.height && line < total {
        let (rows, start, end, col) = if wrapping {
            let layout = view.wrap.layout(rope, revision, line, view.width, tab);
            let sub = sub.min(layout.len() - 1);
            let row = layout.rows()[sub];
            (layout.len(), row.start.0, layout.row_end(sub).0, row.col.0)
        } else {
            (1, 0, 0, 0)
        };
        let current = sub.min(rows - 1);
        out.push(VisualRow {
            line,
            sub: current,
            rows,
            start,
            end,
            col,
        });
        sub = current + 1;
        if sub >= rows {
            line += 1;
            sub = 0;
        }
    }
    out
}

/// Draws the editor and returns the text area (excluding the gutter), for mouse hit-testing,
/// and the merge conflicts' action-row buttons it drew.
pub fn render(
    f: &mut Frame,
    area: Rect,
    buf: &mut Buffer,
    view: &mut View,
    theme: &Theme,
    s: &Settings,
    overlay: Overlay,
) -> (Rect, Vec<MergeButtonHit>) {
    let focused = overlay.focused;
    let s = &s.for_buffer(buf);
    let tab = s.tab_width;
    let rope = &buf.rope;
    let total = pos::line_count(rope);
    let digits = total.to_string().len().max(3);
    let gutter_w = gutter_width(rope);
    let text = Rect {
        x: area.x + gutter_w,
        width: area.width.saturating_sub(gutter_w),
        ..area
    };

    let height = text.height.max(1) as usize;
    let width = text.width.max(1) as usize;
    let wrapping = overlay.wrapping;
    let revision = buf.revision();
    // A split resize rewraps the lines: the cursor may have moved off screen, so reveal it again.
    if wrapping && view.width != width {
        view.reveal_cursor = true;
    }
    // The wrap inputs scrolling reads (the view's width / height are written the same way).
    view.height = height;
    view.width = width;
    view.wrapping = wrapping;
    view.revision = revision;
    view.tab_width = tab;
    // Re-anchor the viewport (the buffer may have changed since the last frame); wrapping keeps
    // the sub-row it was scrolled to, clamped to the line's rows at this width.
    let top_row = view.top_row();
    view.set_top_line(rope, view.top_line());
    if wrapping {
        let rows = view.wrap.layout(rope, revision, view.top_line(), width, tab).len();
        view.set_top(rope, view.top_line(), top_row.min(rows - 1));
        // The scroll bar reads the total rows without laying the text out itself.
        view.wrap.total_rows(rope, revision, width, tab);
    }
    if let Some((start, end)) = view.reveal_range.take() {
        view.reveal_in_center(rope, start, end, tab);
        view.reveal_cursor = false;
    } else if let Some(p) = view.reveal_pos.take() {
        view.scroll_to(rope, p, tab);
        view.reveal_cursor = false;
    } else if view.reveal_cursor {
        view.scroll_to_cursor(rope, tab);
        view.reveal_cursor = false;
    }
    // The rows to draw, top first: a wrapped line takes several. Their byte range is what the
    // byte-range overlays below look at, so nothing off screen is walked.
    let rows = visual_rows(rope, view, revision, wrapping, total, tab);
    // Sideways scrolling is off while wrapping: the rows already fit the text width.
    let left = if wrapping { 0 } else { view.left_col };

    let base = theme.get("editor");
    let current_line_style = base.patch(theme.get("editor.lineHighlight"));
    let selection = theme.get("editor.selection");
    // Secondary cursors as the terminal draws the primary: a block reversed, a bar or an
    // underline as an underlined cell (the char stays readable).
    let secondary_cursor = theme.get(match s.cursor_style {
        CursorStyle::Block => "editor.cursor.secondary",
        CursorStyle::Line | CursorStyle::Underline => "editor.cursor.secondaryUnderline",
    });
    let find_match = theme.get("editor.findMatch");
    let find_highlight = theme.get("editor.findMatchHighlight");
    let control = theme.get("editor.whitespace.control");
    let whitespace = overlay.whitespace;
    let whitespace_fg = theme.get("editorWhitespace.foreground");
    let gutter = theme.get("gutter");
    let gutter_active = theme.get("gutter.active");

    let primary = view.selections.primary().head;
    let cursor_line = pos::line_of(rope, primary);
    // Inline blame: the cursor line's label (one binary search), unless the primary selection
    // spans lines.
    let blame = overlay
        .blame
        .then(|| {
            let sel = view.selections.primary();
            if pos::line_of(rope, sel.start()) != pos::line_of(rope, sel.end()) {
                return None;
            }
            let line = buf.blame.line(rope, cursor_line)?;
            Some(blame::label(line, crate::history::now_ms()))
        })
        .flatten();
    let blame_style = theme.get("git.blame.editorDecorationForeground");
    let last_line = (view.top_line() + view.height).min(total - 1);
    // Only the data on screen: there may be tens of thousands of selections and matches. While
    // wrapping, a line can extend past the view, so the range is the first drawn row's start to
    // the last drawn row's end.
    let (visible_start, visible_end) = if wrapping {
        let start = rows.first().map_or_else(
            || pos::line_start(rope, view.top_line()),
            |r| ByteIdx(pos::line_start(rope, r.line).0 + r.start),
        );
        let end = rows
            .last()
            .map_or(start, |r| ByteIdx(pos::line_start(rope, r.line).0 + r.end));
        (start, end.max(start))
    } else {
        (pos::line_start(rope, view.top_line()), pos::line_end(rope, last_line))
    };
    let sels = view.selections.overlapping(visible_start, visible_end);
    // Matched brackets at the cursors on screen (cached in the view: recomputed only when the
    // text or those cursors change).
    let bracket_marks: Vec<ByteIdx> = if s.match_brackets {
        let heads: Vec<ByteIdx> = sels.iter().map(|s| s.head).collect();
        view.brackets.get(rope, &buf.syntax, buf.revision(), &heads).to_vec()
    } else {
        Vec::new()
    };
    let bracket_match = theme.get("editor.bracketMatch");
    let mut bi = 0;
    let mut is_bracket = |b: ByteIdx| {
        while bi < bracket_marks.len() && bracket_marks[bi] < b {
            bi += 1;
        }
        bracket_marks.get(bi) == Some(&b)
    };

    // Syntax colours: the token's foreground, under selection / find backgrounds. From the
    // buffer's highlight cache (no query unless the tree changed or the view left the cached
    // lines); a commit's are git's (`CommitDoc::spans`).
    let lines = view.top_line()..last_line + 1;
    let spans: Vec<(ByteIdx, ByteIdx, Style)> = match &buf.commit {
        Some(c) => c
            .spans(rope, lines)
            .into_iter()
            .map(|(start, end, key)| (start, end, theme.get(key)))
            .collect(),
        None => {
            let token_styles: Vec<_> = crate::syntax::KEYS.iter().map(|k| theme.get(k)).collect();
            buf.syntax
                .spans(rope, lines)
                .iter()
                .map(|s| (s.start, s.end, token_styles[s.highlight.index()]))
                .collect()
        }
    };
    let mut ki = 0;
    let mut token_at = |b: ByteIdx| {
        while ki < spans.len() && spans[ki].1 <= b {
            ki += 1;
        }
        spans.get(ki).filter(|s| s.0 <= b).map(|s| s.2)
    };

    let secondary: HashSet<ByteIdx> = sels.iter().map(|s| s.head).filter(|h| *h != primary).collect();
    // Selections are sorted and bytes are visited in order, so one index walks them.
    let mut si = 0;
    let mut in_selection = |b: ByteIdx| {
        while si < sels.len() && sels[si].end() <= b {
            si += 1;
        }
        si < sels.len() && sels[si].start() <= b
    };
    // Find matches on screen (from the widget's cache), walked the same way. The one that is
    // the primary selection is the current match.
    let view_range = visible_start..visible_end;
    let matches: &[Match] = overlay.find.map_or(&[], |f| f.matches_in(view_range.clone()));
    let mut mi = 0;
    let mut in_match = |b: ByteIdx| {
        while mi < matches.len() && matches[mi].end <= b {
            mi += 1;
        }
        let m = matches.get(mi).filter(|m| m.start <= b)?;
        let sel = view.selections.primary();
        Some(m.start == sel.start() && m.end == sel.end())
    };

    // Diagnostics on screen: the marks overlapping the viewport (binary search: there may be
    // thousands), flattened into runs (the most severe wins) and walked with one index. An empty
    // range is drawn under the char there, the one before at a line end, or the cell past an
    // empty line's end.
    let squiggles = Severity::ALL.map(|s| theme.get(s.theme_key()));
    let mut peek = pos::Peek::new(rope);
    let marks = buf.diagnostics.overlapping(view_range.start, view_range.end);
    let runs = diagnostics::runs(marks, |m| diagnostics::extent(rope, &mut peek, m));
    let mut ri = 0;
    let mut squiggle_at = |b: ByteIdx| {
        while ri < runs.len() && runs[ri].end <= b {
            ri += 1;
        }
        runs.get(ri)
            .filter(|r| r.start <= b)
            .map(|r| squiggles[r.severity as usize])
    };

    // The language server's semantic tokens on screen (binary search, then one index; a token with
    // no theme style leaves tree-sitter's colour) and the regions it marked inactive (clangd's
    // compiled-out code), whose foreground is dimmed over everything. Off: neither is read.
    let semantic_on = s.semantic_highlighting == SemanticHighlighting::On;
    let (sem_tokens, sem_inactive) = if semantic_on {
        (
            buf.semantic.overlapping(view_range.start, view_range.end),
            buf.semantic.overlapping_inactive(view_range.start, view_range.end),
        )
    } else {
        (&[][..], &[][..])
    };
    let sem_styles = (!sem_tokens.is_empty()).then(|| semantic::Styles::new(theme));
    let mut sti = 0;
    let mut semantic_at = |b: ByteIdx| {
        while sti < sem_tokens.len() && sem_tokens[sti].end <= b {
            sti += 1;
        }
        let t = sem_tokens.get(sti).filter(|t| t.start <= b)?;
        sem_styles.as_ref()?.get(t.kind, t.mods)
    };
    let mut ii = 0;
    let mut inactive_at = |b: ByteIdx| {
        while ii < sem_inactive.len() && sem_inactive[ii].end <= b {
            ii += 1;
        }
        sem_inactive.get(ii).is_some_and(|r| r.start <= b)
    };

    // Quick diff's markers on screen (by line, increasing), in the gutter's last column.
    let markers = buf.quick_diff.visible(rope, view.top_line(), last_line);
    let mut qi = 0;
    let marker_x = (gutter_w > 1 && area.width > gutter_w).then_some(area.x + gutter_w - 1);

    // Merge conflicts on screen: each block line's background (the markers' lines too) and the
    // labels after the `<<<<<<<`, `|||||||` and `>>>>>>>` lines, the first with the action row.
    // Both walk the visible conflicts in order (one index each), never the buffer per byte.
    let conflicts = buf.conflicts.scan(rope, revision);
    let (current_header, current_content) = (
        theme.get("merge.currentHeaderBackground"),
        theme.get("merge.currentContentBackground"),
    );
    let (incoming_header, incoming_content) = (
        theme.get("merge.incomingHeaderBackground"),
        theme.get("merge.incomingContentBackground"),
    );
    let (common_header, common_content) = (
        theme.get("merge.commonHeaderBackground"),
        theme.get("merge.commonContentBackground"),
    );
    let mut ci = 0;
    let mut merge_at = |b: ByteIdx| -> Option<Style> {
        while ci < conflicts.len() && conflicts[ci].end <= b {
            ci += 1;
        }
        let c = conflicts.get(ci).filter(|c| c.start <= b)?;
        Some(if b < c.current.start {
            current_header
        } else if b < c.current.end {
            current_content
        } else if c.base.as_ref().is_some_and(|base| b < base.start) {
            common_header
        } else if b < c.base.as_ref().map_or(c.current.end, |base| base.end) {
            common_content
        } else if b < c.incoming.start {
            incoming_header
        } else if b < c.incoming.end {
            incoming_content
        } else {
            incoming_header
        })
    };
    /// A marker line's label and, on the `<<<<<<<` line, the conflict whose action row it gets.
    struct Marker {
        line: usize,
        label: &'static str,
        conflict: Option<ByteIdx>,
    }
    let view_end = pos::line_end(rope, last_line);
    let mut marker_rows: Vec<Marker> = Vec::new();
    for conflict in conflicts
        .iter()
        .filter(|c| c.start <= view_end && c.end > view_range.start)
    {
        marker_rows.push(Marker {
            line: pos::line_of(rope, conflict.start),
            label: "(Current Change)",
            conflict: Some(conflict.start),
        });
        if conflict.base.is_some() {
            marker_rows.push(Marker {
                line: pos::line_of(rope, conflict.current.end),
                label: "(Common Ancestor)",
                conflict: None,
            });
        }
        marker_rows.push(Marker {
            line: pos::line_of(rope, conflict.incoming.end),
            label: "(Incoming Change)",
            conflict: None,
        });
    }
    let mut mki = 0;
    let mut merge_buttons: Vec<MergeButtonHit> = Vec::new();
    let merge_button_style = theme.get("quickDiff.action");

    let out = f.buffer_mut();
    out.set_style(area, base);
    let right = text.x + text.width;
    // The widest line drawn, for sideways scrolling (`View::scroll_width`). Wrapping never
    // scrolls sideways, so wrapped rows aren't measured.
    let mut widest = 0;
    // The primary cursor's (row within its line, display column from that row's start).
    let cursor = focused.then(|| {
        if wrapping {
            let layout = view.wrap.layout(rope, revision, cursor_line, view.width, tab);
            let start = pos::line_start(rope, cursor_line);
            let (sub, col) = layout.row_of(ByteIdx(primary.0 - start.0));
            (sub, layout.rows()[sub].col.0 + col.0)
        } else {
            (0, pos::display_col(rope, primary, tab).0)
        }
    });
    let mut cursor_cell = None;
    for (offset, visual) in rows.iter().enumerate() {
        let line = visual.line;
        let sub = visual.sub;
        let last = sub + 1 == visual.rows;
        let y = area.y + offset as u16;
        let start = pos::line_start(rope, line);
        let row_style = if line == cursor_line { current_line_style } else { base };
        let row_style = match merge_at(start) {
            Some(merge) => row_style.patch(merge),
            None => row_style,
        };
        out.set_style(Rect { y, height: 1, ..area }, row_style);

        // The line number only on a line's first row; a continuation row's gutter stays blank.
        if sub == 0 {
            let num_style = if line == cursor_line {
                gutter.patch(gutter_active)
            } else {
                gutter
            };
            let num = format!(" {:>digits$} ", line + 1);
            out.set_stringn(
                area.x,
                y,
                num,
                gutter_w as usize,
                num_style.bg(row_style.bg.unwrap_or_default()),
            );
            while qi < markers.len() && markers[qi].0 < line {
                qi += 1;
            }
            if let Some(x) = marker_x
                && let Some(&(_, marker)) = markers.get(qi).filter(|(l, _)| *l == line)
            {
                let style = theme.get(marker.theme_key()).bg(row_style.bg.unwrap_or_default());
                out.set_stringn(x, y, marker.glyph(), 1, style);
            }
        }

        // The row's text: the whole line when not wrapping, its slice when wrapping (`from..to`
        // within the line, whose display column starts at `visual.col`).
        let layout = wrapping.then(|| view.wrap.layout(rope, revision, line, view.width, tab));
        let content: Cow<'_, str> = match &layout {
            Some(layout) => Cow::Borrowed(layout.text()),
            None => Cow::Owned(pos::line_text(rope, line)),
        };
        let bounds = whitespace_bounds(whitespace, &content);
        let from = visual.start;
        let to = if wrapping { visual.end } else { content.len() };
        let mut col = visual.col;
        // Where the line ran past the right edge: the rest is only measured (`widest`).
        let mut cut = None;
        for (off, g) in content[from..to].grapheme_indices(true) {
            let off = from + off;
            let b = ByteIdx(start.0 + off);
            let (mut shown, w) = pos::display_grapheme(g, DisplayCol(col), tab);
            let selected = in_selection(b);
            if col + w <= left {
                col += w;
                continue;
            }
            if !wrapping && col >= left + view.width {
                cut = Some(off);
                break;
            }
            let mut style = row_style;
            if let Some(token) = token_at(b) {
                style = style.patch(token);
            }
            if let Some(sem) = semantic_at(b) {
                style = style.patch(sem);
            }
            if is_bracket(b) {
                style = style.patch(bracket_match);
            }
            if let Some(squiggle) = squiggle_at(b) {
                style = style.patch(squiggle);
            }
            // Rendered whitespace: the glyph in the tab's first cell (columns unchanged), its
            // colour over the token's, under backgrounds; not at a drawn cursor (a secondary's
            // reversed cell would take the dim colour as its background; the terminal cursor
            // marks the primary only when focused).
            let secondary_here = secondary.contains(&b);
            let cursor_here = secondary_here || (focused && b == primary);
            if whitespace != RenderWhitespace::None && shows_whitespace(whitespace, &content, off, g, bounds, selected)
            {
                shown = if g == "\t" {
                    format!("→{}", " ".repeat(w - 1)).into()
                } else {
                    "·".into()
                };
                if !cursor_here {
                    style = style.patch(whitespace_fg);
                }
            }
            let found = in_match(b);
            if found.is_some() {
                style = style.patch(find_highlight);
            }
            if selected {
                style = style.patch(selection);
            }
            if found == Some(true) {
                style = style.patch(find_match);
            }
            if inactive_at(b) {
                style = theme.dim(style);
            }
            if g.starts_with(|c: char| c.is_control()) && g != "\t" {
                style = style.patch(control);
            }
            if secondary_here {
                style = style.patch(secondary_cursor);
            }
            let x = text.x + (col.max(left) - left) as u16;
            if col < left {
                // A wide char cut by the left edge: show the visible part as blanks.
                let visible = col + w - left;
                out.set_stringn(x, y, " ".repeat(visible), visible, style);
            } else {
                out.set_stringn(x, y, &shown, (right - x) as usize, style);
            }
            col += w;
        }
        if !wrapping {
            widest = widest.max(match cut {
                Some(off) => pos::col_after(&content[off..], DisplayCol(col), tab).0,
                None => col,
            });
        }
        if let Some((csub, ccol)) = cursor
            && line == cursor_line
            && sub == csub
            && ccol >= left
            && ccol < left + view.width
        {
            cursor_cell = Some((text.x + (ccol - left) as u16, y));
        }

        // The line's end lives on its last row: a selection running over the line break, a
        // secondary cursor there, a squiggle past an empty line, the merge labels and the
        // action row, then inline blame.
        if last {
            // A selection over the line break paints one cell past the line end.
            let end = ByteIdx(start.0 + content.len());
            let over_eol = line + 1 < total && in_selection(end);
            // A squiggle past the end only on an empty line (an empty range there).
            let squiggle = squiggle_at(end).filter(|_| content.is_empty());
            if (over_eol || secondary.contains(&end) || squiggle.is_some()) && col >= left && col < left + view.width {
                let mut style = row_style;
                if let Some(squiggle) = squiggle {
                    style = style.patch(squiggle);
                }
                if over_eol {
                    style = style.patch(selection);
                }
                if secondary.contains(&end) {
                    style = style.patch(secondary_cursor);
                }
                out.set_stringn(text.x + (col - left) as u16, y, " ", 1, style);
            }

            // Merge conflict markers: the label after the line's text and, on the `<<<<<<<`
            // line, its action row. Buttons are dropped from the right (Compare first) when they
            // don't fit; each drawn one is recorded for hit-testing.
            while mki < marker_rows.len() && marker_rows[mki].line < line {
                mki += 1;
            }
            // A marker line's label and action row take precedence over inline blame, which would
            // otherwise overwrite them (Next Conflict leaves the cursor exactly here).
            let marker_here = marker_rows.get(mki).is_some_and(|m| m.line == line);
            if let Some(marker) = marker_rows.get(mki).filter(|m| m.line == line) {
                let edge = left + view.width;
                let mut x = col + blame::GAP;
                if x >= left && x < edge {
                    let avail = edge.saturating_sub(x);
                    if let Some(shown) = blame::fit(marker.label, avail) {
                        let at = text.x + (x - left) as u16;
                        out.set_stringn(at, y, &shown, (right - at) as usize, row_style.patch(blame_style));
                        x += shown.chars().count() + blame::GAP;
                    }
                }
                if let Some(conflict) = marker.conflict
                    && x >= left
                {
                    // The buttons' widths and the one-cell gaps between them.
                    let width = |n: usize| {
                        MERGE_ACTIONS[..n]
                            .iter()
                            .map(|(label, _)| label.chars().count() + 1)
                            .sum::<usize>()
                            .saturating_sub(1)
                    };
                    let mut count = MERGE_ACTIONS.len();
                    while count > 0 && x + width(count) > edge {
                        count -= 1;
                    }
                    let style = row_style.patch(merge_button_style);
                    for (i, (label, button)) in MERGE_ACTIONS[..count].iter().enumerate() {
                        let at = text.x + (x - left) as u16;
                        out.set_stringn(at, y, label, (right - at) as usize, style);
                        merge_buttons.push((
                            Rect {
                                x: at,
                                y,
                                width: label.chars().count() as u16,
                                height: 1,
                            },
                            *button,
                            conflict,
                        ));
                        x += label.chars().count();
                        if i + 1 < count {
                            x += 1;
                        }
                    }
                }
            }

            // Inline blame: `GAP` cells after the line's end (`col`: its whole width, unless the
            // line reached the right edge), cut with "…" at the edge, nothing when too little
            // room is left or the label's start is scrolled off to the left.
            if line == cursor_line
                && !marker_here
                && let Some(label) = &blame
                && col < left + view.width
            {
                let start = col + blame::GAP;
                if start >= left
                    && let Some(shown) = blame::fit(label, (left + view.width).saturating_sub(start))
                {
                    let x = text.x + (start - left) as u16;
                    out.set_stringn(x, y, &shown, (right - x) as usize, row_style.patch(blame_style));
                }
            }
        }
    }

    // Rulers: a thin line in each visible ruler column, only in cells that show no text (behind
    // the text); a cell with a modifier (a secondary cursor) keeps its look.
    let rulers = s.rulers.as_deref().unwrap_or_default();
    if !rulers.is_empty() {
        let style = theme.get("editorRuler.foreground");
        for &col in rulers.iter().filter(|&&c| c >= left && c < left + view.width) {
            let x = text.x + (col - left) as u16;
            for y in text.y..text.y + text.height {
                let cell = &mut out[(x, y)];
                if cell.symbol() == " " && cell.modifier.is_empty() {
                    cell.set_char('\u{2502}');
                    cell.fg = style.fg.unwrap_or_default();
                }
            }
        }
    }

    if !wrapping {
        view.scroll_width.measure(widest);
    }

    // The terminal's own cursor goes on the primary selection, on the screen row it lands on.
    if let Some((x, y)) = cursor_cell {
        f.set_cursor_position(Position { x, y });
    }
    (text, merge_buttons)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `line` with each whitespace char `mode` shows replaced by `·` / `→`; `selected` is a byte
    /// range in a selection.
    fn shown(mode: RenderWhitespace, line: &str, selected: std::ops::Range<usize>) -> String {
        let bounds = whitespace_bounds(mode, line);
        line.grapheme_indices(true)
            .map(
                |(off, g)| match shows_whitespace(mode, line, off, g, bounds, selected.contains(&off)) {
                    true if g == "\t" => "→",
                    true => "·",
                    false => g,
                },
            )
            .collect()
    }

    /// The rows a wrapped line and the next lines take; the first row only when `top_row` says
    /// so; whole lines when not wrapping.
    #[test]
    fn visual_rows_walk_wrapped_lines_and_their_top_row() {
        let rope = ropey::Rope::from_str("  abcd efgh ijkl mnop qrst\nnext\n\nlast\n");
        let total = pos::line_count(&rope);
        let mut view = View::new();
        (view.width, view.height) = (25, 4);
        let rows = visual_rows(&rope, &mut view, 0, true, total, 4);
        let got: Vec<(usize, usize, usize, usize, usize)> =
            rows.iter().map(|r| (r.line, r.sub, r.start, r.end, r.col)).collect();
        assert_eq!(
            got,
            vec![(0, 0, 0, 22, 0), (0, 1, 22, 26, 2), (1, 0, 0, 4, 0), (2, 0, 0, 0, 0)],
            "the long line's two rows (indented), then the next lines"
        );

        // Starting on a continuation row skips the line's first one.
        view.set_top(&rope, 0, 1);
        let rows = visual_rows(&rope, &mut view, 0, true, total, 4);
        assert_eq!(
            rows[0],
            VisualRow {
                line: 0,
                sub: 1,
                rows: 2,
                start: 22,
                end: 26,
                col: 2
            }
        );
        assert_eq!((rows[1].line, rows[1].sub), (1, 0));

        // Not wrapping: one row a line from `top_line`, the sub-row ignored.
        let rows = visual_rows(&rope, &mut view, 0, false, total, 4);
        let got: Vec<(usize, usize)> = rows.iter().map(|r| (r.line, r.sub)).collect();
        assert_eq!(got, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
    }

    #[test]
    fn each_mode_picks_its_whitespace() {
        use RenderWhitespace::*;
        let line = "  a b  c\td  ";
        assert_eq!(shown(None, line, 0..99), line);
        assert_eq!(shown(All, line, 0..0), "··a·b··c→d··");
        assert_eq!(
            shown(Boundary, line, 0..0),
            "··a b··c→d··",
            "single spaces between words stay"
        );
        assert_eq!(shown(Trailing, line, 0..99), "  a b  c\td··");
        assert_eq!(shown(Selection, line, 1..5), " ·a·b  c\td  ");
        assert_eq!(shown(Selection, line, 0..0), line);
        // A blank line is all trailing; a space next to a tab is a boundary.
        assert_eq!(shown(Trailing, " \t ", 0..0), "·→·");
        assert_eq!(shown(Boundary, "a \tb c", 0..0), "a·→b c");
        assert_eq!(shown(Boundary, "a b", 0..0), "a b");
        // A single leading or trailing space is a boundary.
        assert_eq!(shown(Boundary, " a b ", 0..0), "·a b·");
        assert_eq!(shown(Boundary, "\ta b", 0..0), "→a b");
        // Only U+0020 and tab: a no-break space is left alone.
        assert_eq!(shown(All, "a\u{a0}b ", 0..0), "a\u{a0}b·");
    }
}
