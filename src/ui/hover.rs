//! The hover popup (`app.hover.popup`): over its group's editor, anchored at `popup.anchor` (the
//! start of the server's range or the word; above that row when it fits, else below;
//! `ui::popup`), as wide as its widest line up to `hover::MAX_WIDTH` and the editor, as tall as
//! its wrapped rows up to `hover::MAX_HEIGHT` and the room there. A border (`editorHoverWidget.border`) with one column of padding inside;
//! text in `editorHoverWidget.foreground` on `.background`, headings bold, code (blocks and inline)
//! in `textPreformat.foreground`, rules as a line in the border's colour; a diagnostic's icon in
//! its severity's `problems*Icon.foreground`, its `source(code)` in `editorHoverWidget.source`.
//! A longer one shows "…" in its bottom (more below) / top (more above) border and scrolls with
//! the wheel.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::popup::{self, Side};
use crate::app::App;
use crate::hover::{self, Kind};
use crate::layout::GroupHit;

/// Draws the popup if there is one and its anchor is on screen; returns its rect.
pub fn render(f: &mut Frame, app: &mut App, groups: &[GroupHit]) -> Option<Rect> {
    let popup = app.hover.popup.as_mut()?;
    let hit = groups.iter().find(|g| g.id == popup.group)?;
    let tab = app.groups.get(popup.group)?.active_tab()?;
    if tab.buffer != popup.buffer {
        return None;
    }
    let buf = app.buffers.get(popup.buffer)?;
    let anchor = popup::cell_of(&buf.rope, &tab.view, hit.text, popup.anchor, app.settings.tab_width)?;
    let area = hit.editor;
    // Border and a column of padding on each side.
    let width = hover::width(&popup.lines)
        .min(hover::MAX_WIDTH)
        .min(area.width.saturating_sub(4) as usize);
    if width == 0 {
        return None;
    }
    let rows = hover::wrap(&popup.lines, width);
    let height = rows.len().min(hover::MAX_HEIGHT) as u16 + 2;
    let rect = popup::place(area, anchor, width as u16 + 4, height, Side::Above)?;

    let theme = &app.theme;
    let base = theme
        .get("editorHoverWidget.background")
        .patch(theme.get("editorHoverWidget.foreground"));
    let border = base.patch(theme.get("editorHoverWidget.border"));
    let code = base.patch(theme.get("textPreformat.foreground"));
    let heading = base.patch(theme.get("editorHoverWidget.heading"));
    let source = base.patch(theme.get("editorHoverWidget.source"));
    let out = f.buffer_mut();
    let inner = popup::frame(out, rect, base, border);
    let shown = inner.height as usize;
    popup.rows = rows.len();
    popup.shown = shown;
    popup.top = popup.top.min(rows.len().saturating_sub(shown));
    let x0 = inner.x + 1;
    for (r, row) in rows.iter().skip(popup.top).take(shown).enumerate() {
        let y = inner.y + r as u16;
        let line = &popup.lines[row.line];
        if line.kind == Kind::Rule {
            for x in inner.x..inner.x + inner.width {
                out.set_stringn(x, y, "─", 1, border);
            }
            continue;
        }
        let mut x = x0;
        let end = x0 + width as u16;
        let mut spans = line.code.iter().peekable();
        for (off, g) in line.text[row.range.clone()].grapheme_indices(true) {
            let at = row.range.start + off;
            let w = g.width();
            if x + w as u16 > end {
                break;
            }
            while spans.peek().is_some_and(|s| s.end <= at) {
                spans.next();
            }
            let style = match line.kind {
                Kind::Code => code,
                Kind::Diagnostic(severity) if at == 0 => base.patch(theme.get(severity.icon_key())),
                Kind::Source => source,
                _ if spans.peek().is_some_and(|s| s.start <= at) => code,
                Kind::Heading => heading,
                _ => base,
            };
            if w > 0 {
                out.set_stringn(x, y, g, w, style);
            }
            x += w as u16;
        }
    }
    let more_at = rect.x + rect.width.saturating_sub(3);
    if popup.top > 0 {
        out.set_stringn(more_at, rect.y, "…", 1, border);
    }
    if popup.top + shown < rows.len() {
        out.set_stringn(more_at, rect.y + rect.height - 1, "…", 1, border);
    }
    Some(rect)
}
