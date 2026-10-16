//! The gutter's peek (`app.quick_diff.peek`): below the change it's about (above it when there's
//! no room below), over its group's editor, as wide as the text area (up to `MAX_WIDTH`). A title row (what changed, the
//! ` Stage Change ` button and ` × `), then the index's removed lines (`-`, in
//! `diffEditor.removedLineBackground`) and the buffer's lines that replaced them (`+`, in
//! `diffEditor.insertedLineBackground`), at most `MAX_ROWS`, "…" in the bottom border when cut.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::popup::{self, Side};
use crate::app::App;
use crate::buffer::pos;
use crate::layout::{GroupHit, QuickDiffHit};

const MAX_WIDTH: u16 = 100;
const MAX_ROWS: usize = 12;
const STAGE: &str = " Stage Change ";
const CLOSE: &str = " × ";

/// Draws the peek if there is one and its change is on screen; returns its rect and buttons.
pub fn render(f: &mut Frame, app: &App, groups: &[GroupHit]) -> Option<QuickDiffHit> {
    let peek = app.quick_diff.peek.as_ref()?;
    let hit = groups.iter().find(|g| g.id == peek.group)?;
    let tab = app.groups.get(peek.group)?.active_tab()?;
    if tab.buffer != peek.buffer {
        return None;
    }
    let buf = app.buffers.get(peek.buffer)?;
    let rope = &buf.rope;
    let last = peek.lines.end.saturating_sub(1).min(pos::line_count(rope) - 1);
    let (_, y) = popup::cell_of(
        rope,
        &tab.view,
        hit.text,
        pos::line_start(rope, last),
        app.settings.tab_width,
    )?;

    // `-` lines from the index, `+` lines from the buffer (not for a deletion).
    let mark = &peek.mark;
    let mut rows: Vec<(bool, String)> = mark.removed.iter().map(|l| (false, l.clone())).collect();
    if mark.removed_count > mark.removed.len() {
        rows.push((false, format!("… {} more", mark.removed_count - mark.removed.len())));
    }
    if mark.kind != crate::git::HunkKind::Deleted {
        for line in peek.lines.clone() {
            if line >= pos::line_count(rope) || rows.len() > MAX_ROWS {
                break;
            }
            rows.push((true, pos::line_text(rope, line).to_string()));
        }
    }
    let added = if mark.kind == crate::git::HunkKind::Deleted {
        0
    } else {
        peek.lines.len()
    };
    let title = match (mark.removed_count, added) {
        (0, n) => format!(" {n} line{} added", plural(n)),
        (n, 0) => format!(" {n} line{} deleted", plural(n)),
        (r, a) => format!(" {r} line{} changed to {a}", plural(r)),
    };

    let area = hit.editor;
    let width = hit.text.width.min(MAX_WIDTH);
    let height = (rows.len().min(MAX_ROWS) + 3) as u16;
    // Below the change's last line if it fits, else above its first (on screen or not).
    let below = (area.y + area.height).saturating_sub(y + 1);
    let rect = if below >= height {
        popup::place(area, (hit.text.x, y), width, height, Side::Below)?
    } else {
        let top = tab.view.top_line();
        let first = peek.lines.start.max(top) - top;
        let y_first = hit.text.y + (first as u16).min(y - hit.text.y);
        popup::place(area, (hit.text.x, y_first), width, height, Side::Above)?
    };

    let theme = &app.theme;
    let base = theme
        .get("editorHoverWidget.background")
        .patch(theme.get("editorHoverWidget.foreground"));
    let border = base.patch(theme.get("editorHoverWidget.border"));
    let removed = base.patch(theme.get("diffEditor.removedLineBackground"));
    let inserted = base.patch(theme.get("diffEditor.insertedLineBackground"));
    let action = theme.get("quickDiff.action");
    let out = f.buffer_mut();
    let inner = popup::frame(out, rect, base, border);
    let w = inner.width as usize;

    // The title row: the summary, then the buttons at the right.
    let close = Rect {
        x: inner.x + inner.width.saturating_sub(CLOSE.width() as u16),
        y: inner.y,
        width: (CLOSE.width() as u16).min(inner.width),
        height: 1,
    };
    let stage = Rect {
        x: close.x.saturating_sub(STAGE.width() as u16 + 1).max(inner.x),
        y: inner.y,
        width: (STAGE.width() as u16).min(close.x.saturating_sub(inner.x)),
        height: 1,
    };
    let room = stage.x.saturating_sub(inner.x) as usize;
    out.set_stringn(inner.x, inner.y, &title, room, base.patch(theme.get("quickDiff.title")));
    out.set_stringn(stage.x, stage.y, STAGE, stage.width as usize, action);
    out.set_stringn(close.x, close.y, CLOSE, close.width as usize, base);

    let shown = (inner.height as usize).saturating_sub(1);
    for (i, (plus, text)) in rows.iter().take(shown).enumerate() {
        let y = inner.y + 1 + i as u16;
        let style: Style = if *plus { inserted } else { removed };
        out.set_stringn(inner.x, y, " ".repeat(w), w, style);
        let line = format!("{} {}", if *plus { '+' } else { '-' }, printable(text));
        out.set_stringn(inner.x + 1, y, line, w.saturating_sub(1), style);
    }
    if rows.len() > shown {
        out.set_stringn(
            rect.x + rect.width.saturating_sub(3),
            rect.y + rect.height - 1,
            "…",
            1,
            border,
        );
    }
    Some(QuickDiffHit { rect, stage, close })
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Tabs as four spaces, other control characters as spaces (a preview, not the editor's text).
fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\t' => "    ".chars().collect::<Vec<_>>(),
            c if c.is_control() => vec![' '],
            c => vec![c],
        })
        .collect()
}
