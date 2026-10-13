//! The text area: gutter, text, selections, cursors.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::edit::Settings;
use crate::selection::Selection;
use crate::theme::Theme;
use crate::view::View;

/// Draws the editor and returns the text area (excluding the gutter), for mouse hit-testing.
pub fn render(f: &mut Frame, area: Rect, buf: &Buffer, view: &mut View, theme: &Theme, s: &Settings) -> Rect {
    let rope = &buf.rope;
    let tab = s.tab_width;
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
    if view.reveal_cursor {
        view.scroll_to_cursor(rope, tab);
        view.reveal_cursor = false;
    }

    let base = theme.get("editor");
    let current_line_style = base.patch(theme.get("editor.lineHighlight"));
    let selection = theme.get("editor.selection");
    let secondary_cursor = theme.get("editor.cursor.secondary");
    let control = theme.get("editor.whitespace.control");
    let gutter = theme.get("gutter");
    let gutter_active = theme.get("gutter.active");

    let primary = view.selections.primary().head;
    let cursor_line = pos::line_of(rope, primary);
    let sels: Vec<Selection> = view.selections.iter().copied().collect();
    let secondary: HashSet<ByteIdx> = sels.iter().map(|s| s.head).filter(|h| *h != primary).collect();
    // Selections are sorted and bytes are visited in order, so one index walks them.
    let mut si = 0;
    let mut in_selection = |b: ByteIdx| {
        while si < sels.len() && sels[si].end() <= b {
            si += 1;
        }
        si < sels.len() && sels[si].start() <= b
    };

    let out = f.buffer_mut();
    out.set_style(area, base);
    let right = text.x + text.width;
    for row in 0..area.height {
        let line = view.top_line + row as usize;
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

        let start = pos::line_start(rope, line);
        let content = pos::line_text(rope, line);
        let mut col = 0usize;
        for (off, g) in content.grapheme_indices(true) {
            let b = ByteIdx(start.0 + off);
            let (shown, w) = pos::display_grapheme(g, DisplayCol(col), tab);
            let selected = in_selection(b);
            if col + w <= view.left_col {
                col += w;
                continue;
            }
            if col >= view.left_col + view.width {
                break;
            }
            let mut style = row_style;
            if selected {
                style = style.patch(selection);
            }
            if g.starts_with(|c: char| c.is_control()) && g != "\t" {
                style = style.patch(control);
            }
            if secondary.contains(&b) {
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
        if (over_eol || secondary.contains(&end)) && col >= view.left_col && col < view.left_col + view.width {
            let mut style = row_style;
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
    if cursor_line >= view.top_line && cursor_line < view.top_line + view.height {
        let col = pos::display_col(rope, primary, tab).0;
        if col >= view.left_col && col < view.left_col + view.width {
            let x = text.x + (col - view.left_col) as u16;
            let y = text.y + (cursor_line - view.top_line) as u16;
            f.set_cursor_position(Position { x, y });
        }
    }
    text
}
