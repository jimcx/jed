//! A commit tab (`commit_view::CommitTab`): `git show --stat --format=medium`'s lines, coloured
//! as git colours them — the commit id in `gitShow.commit`, a stat's `+` graph in `gitShow.add`,
//! its `-` graph (and a binary's old size) in `gitShow.del` — with the file whose history opened
//! the tab's stat line highlighted (`editor.lineHighlight`). The selection is
//! `editor.selection`; no line numbers. Only the visible lines are drawn, cut at the area's
//! width (no horizontal scroll).

use ratatui::Frame;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::commit_view::{self, CommitTab};
use crate::git_show::Kind;
use crate::layout::CommitHit;
use crate::theme::Theme;

pub fn render(f: &mut Frame, area: Rect, tab: &mut CommitTab, theme: &Theme, focused: bool) -> CommitHit {
    let base = theme.get("editor");
    let out = f.buffer_mut();
    out.set_style(area, base);
    let hit = CommitHit { rows: area };
    if area.height == 0 || area.width == 0 {
        tab.scroll_to_cursor(area.height as usize);
        return hit;
    }
    tab.scroll_to_cursor(area.height as usize);
    let top = tab.top;
    // Render at this width first (a commit's first draw), so the visible range is known.
    let count = tab.lines(area.width as usize).len();
    let last = (top + area.height as usize).min(count);
    let own = tab.own_file_row();
    let cursor = tab.cursor;
    let error = tab.error.clone();
    let cursor_cell = (focused && cursor.row >= top && cursor.row < last).then(|| {
        (
            cursor_x(area, tab, cursor.row, cursor.col),
            area.y + (cursor.row - top) as u16,
        )
    });
    let selections: Vec<Option<(usize, usize)>> = (top..last).map(|row| tab.row_selection(row)).collect();
    let line_highlight = base.patch(theme.get("editor.lineHighlight"));
    let selection = theme.get("editor.selection");
    let dim = base.patch(theme.get("sideBar.dim"));
    let lines = tab.lines(area.width as usize);
    if lines.is_empty() {
        // No commit yet (the load's error, or still reading).
        let text = error.as_deref().unwrap_or("Loading…");
        let room = area.width.saturating_sub(1) as usize;
        out.set_stringn(area.x + 1, area.y, text, room, dim);
        return hit;
    }
    let right = area.x + area.width;
    for (i, line) in lines[top..last].iter().enumerate() {
        let row = top + i;
        let y = area.y + i as u16;
        let row_style = if own == Some(row) { line_highlight } else { base };
        out.set_style(Rect { y, height: 1, ..area }, row_style);
        let selected = selections[i];
        let mut x = area.x;
        let mut col = 0usize;
        'segments: for segment in &line.segments {
            let style = row_style.patch(kind_style(segment.kind, theme));
            for c in segment.text.chars() {
                if x >= right {
                    break 'segments;
                }
                let mut s = style;
                if let Some((from, to)) = selected
                    && col >= from
                    && col < to
                {
                    s = s.patch(selection);
                }
                x = draw_char(out, x, y, right, c, s);
                col += 1;
            }
        }
    }
    // The cursor's cell (only in the focused group).
    if let Some((x, y)) = cursor_cell {
        f.set_cursor_position(Position { x, y });
    }
    hit
}

/// The style a segment's kind adds over the row's (none for git's default colour).
fn kind_style(kind: Kind, theme: &Theme) -> Style {
    match kind {
        Kind::Commit => theme.get("gitShow.commit"),
        Kind::StatAdd => theme.get("gitShow.add"),
        Kind::StatDel => theme.get("gitShow.del"),
        _ => Style::default(),
    }
}

/// Draws one character at `x`; returns the next cell (`\t` four spaces, control chars none).
fn draw_char(out: &mut Cells, x: u16, y: u16, right: u16, c: char, style: Style) -> u16 {
    let room = (right - x) as usize;
    if c == '\t' {
        let (end, _) = out.set_stringn(x, y, "    ", room, style);
        return end;
    }
    if c.is_control() {
        return x;
    }
    let mut buf = [0u8; 4];
    let text = c.encode_utf8(&mut buf);
    let (end, _) = out.set_stringn(x, y, text, room, style);
    end
}

/// The cell of the cursor's character in `row`, cut at the area's right edge.
fn cursor_x(area: Rect, tab: &CommitTab, row: usize, col: usize) -> u16 {
    let used: usize = tab
        .line_text(row)
        .chars()
        .take(col)
        .map(commit_view::display_width)
        .sum();
    let x = area
        .x
        .saturating_add(used.min(area.width.saturating_sub(1) as usize) as u16);
    x.min(area.x + area.width.saturating_sub(1))
}
