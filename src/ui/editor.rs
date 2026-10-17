//! The text area: gutter (line numbers, quick diff's markers), text (syntax colours), matched
//! brackets, diagnostics' squiggles, selections, find matches, cursors, rendered whitespace.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::diagnostics::{self, Severity};
use crate::edit::{RenderWhitespace, Settings};
use crate::find::FindWidget;
use crate::search::Match;
use crate::theme::Theme;
use crate::view::View;

/// What else the editor shows besides the buffer.
pub struct Overlay<'a> {
    /// Shows the terminal cursor (the focused group, nothing else open).
    pub focused: bool,
    /// The find widget, when it's open on this view and its matches are for this text.
    pub find: Option<&'a FindWidget>,
    /// Which spaces and tabs are drawn as `·` / `→` (`App::render_whitespace`).
    pub whitespace: RenderWhitespace,
}

/// Spaces and tabs only, as VS Code's renderer (other Unicode spaces aren't whitespace here).
fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

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

/// Draws the editor and returns the text area (excluding the gutter), for mouse hit-testing.
pub fn render(
    f: &mut Frame,
    area: Rect,
    buf: &mut Buffer,
    view: &mut View,
    theme: &Theme,
    s: &Settings,
    overlay: Overlay,
) -> Rect {
    let focused = overlay.focused;
    let tab = s.tab_width;
    let rope = &buf.rope;
    let total = pos::line_count(rope);
    let digits = total.to_string().len().max(3);
    let gutter_w = (digits + 2) as u16;
    let text = Rect {
        x: area.x + gutter_w,
        width: area.width.saturating_sub(gutter_w),
        ..area
    };

    view.height = text.height.max(1) as usize;
    view.width = text.width.max(1) as usize;
    // Re-anchor the viewport (the buffer may have changed since the last frame).
    view.set_top_line(rope, view.top_line());
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

    let base = theme.get("editor");
    let current_line_style = base.patch(theme.get("editor.lineHighlight"));
    let selection = theme.get("editor.selection");
    let secondary_cursor = theme.get("editor.cursor.secondary");
    let find_match = theme.get("editor.findMatch");
    let find_highlight = theme.get("editor.findMatchHighlight");
    let control = theme.get("editor.whitespace.control");
    let whitespace = overlay.whitespace;
    let whitespace_fg = theme.get("editorWhitespace.foreground");
    let gutter = theme.get("gutter");
    let gutter_active = theme.get("gutter.active");

    let primary = view.selections.primary().head;
    let cursor_line = pos::line_of(rope, primary);
    let last_line = (view.top_line() + view.height).min(total - 1);
    // Only the selections on screen: there may be tens of thousands.
    let sels = view
        .selections
        .overlapping(pos::line_start(rope, view.top_line()), pos::line_end(rope, last_line));
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
    // buffer's highlight cache (no query unless the tree changed or the view left the cached lines).
    let token_styles: Vec<_> = crate::syntax::KEYS.iter().map(|k| theme.get(k)).collect();
    let spans = buf.syntax.spans(rope, view.top_line()..last_line + 1);
    let mut ki = 0;
    let mut token_at = |b: ByteIdx| {
        while ki < spans.len() && spans[ki].end <= b {
            ki += 1;
        }
        spans
            .get(ki)
            .filter(|s| s.start <= b)
            .map(|s| token_styles[s.highlight.index()])
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
    let view_range = pos::line_start(rope, view.top_line())..pos::line_end(rope, last_line);
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
    let runs = diagnostics::runs(marks, |m| {
        if m.start < m.end {
            return (m.start, m.end);
        }
        let line_break = |c: Option<char>| matches!(c, None | Some('\n' | '\r'));
        match (line_break(peek.after(m.start)), line_break(peek.before(m.start))) {
            (false, _) => (m.start, pos::next_grapheme(rope, m.start)),
            (true, false) => (pos::prev_grapheme(rope, m.start), m.start),
            (true, true) => (m.start, ByteIdx(m.start.0 + 1)),
        }
    });
    let mut ri = 0;
    let mut squiggle_at = |b: ByteIdx| {
        while ri < runs.len() && runs[ri].end <= b {
            ri += 1;
        }
        runs.get(ri)
            .filter(|r| r.start <= b)
            .map(|r| squiggles[r.severity as usize])
    };

    // Quick diff's markers on screen (by line, increasing), in the gutter's last column.
    let markers = buf.quick_diff.visible(rope, view.top_line(), last_line);
    let mut qi = 0;
    let marker_x = (gutter_w > 1 && area.width > gutter_w).then_some(area.x + gutter_w - 1);

    let out = f.buffer_mut();
    out.set_style(area, base);
    let right = text.x + text.width;
    for row in 0..area.height {
        let line = view.top_line() + row as usize;
        if line >= total {
            break;
        }
        let y = area.y + row;
        let row_style = if line == cursor_line { current_line_style } else { base };
        out.set_style(Rect { y, height: 1, ..area }, row_style);

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

        let start = pos::line_start(rope, line);
        let content = pos::line_text(rope, line);
        let bounds = whitespace_bounds(whitespace, &content);
        let mut col = 0usize;
        for (off, g) in content.grapheme_indices(true) {
            let b = ByteIdx(start.0 + off);
            let (mut shown, w) = pos::display_grapheme(g, DisplayCol(col), tab);
            let selected = in_selection(b);
            if col + w <= view.left_col {
                col += w;
                continue;
            }
            if col >= view.left_col + view.width {
                break;
            }
            let mut style = row_style;
            if let Some(token) = token_at(b) {
                style = style.patch(token);
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
            if g.starts_with(|c: char| c.is_control()) && g != "\t" {
                style = style.patch(control);
            }
            if secondary_here {
                style = style.patch(secondary_cursor);
            }
            let x = text.x + (col.max(view.left_col) - view.left_col) as u16;
            if col < view.left_col {
                // A wide char cut by the left edge: show the visible part as blanks.
                let visible = col + w - view.left_col;
                out.set_stringn(x, y, " ".repeat(visible), visible, style);
            } else {
                out.set_stringn(x, y, &shown, (right - x) as usize, style);
            }
            col += w;
        }

        // End of line: selection running over the line break, or a secondary cursor here.
        let end = ByteIdx(start.0 + content.len());
        let over_eol = line + 1 < total && in_selection(end);
        // A squiggle past the end only on an empty line (an empty range there).
        let squiggle = squiggle_at(end).filter(|_| content.is_empty());
        if (over_eol || secondary.contains(&end) || squiggle.is_some())
            && col >= view.left_col
            && col < view.left_col + view.width
        {
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
            out.set_stringn(text.x + (col - view.left_col) as u16, y, " ", 1, style);
        }
    }

    // The terminal's own cursor goes on the primary selection.
    if focused && cursor_line >= view.top_line() && cursor_line < view.top_line() + view.height {
        let col = pos::display_col(rope, primary, tab).0;
        if col >= view.left_col && col < view.left_col + view.width {
            let x = text.x + (col - view.left_col) as u16;
            let y = text.y + (cursor_line - view.top_line()) as u16;
            f.set_cursor_position(Position { x, y });
        }
    }
    text
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
