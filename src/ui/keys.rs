//! The keyboard shortcuts view (`crate::keys`): full screen over the editor area, a search box
//! on its first row, then every row's key, title, context and source, its selection highlighted
//! and the scroll bar on its right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::App;
use crate::layout::{KeysHit, ScrollTarget};
use crate::scrollbar::Scrollable;

/// Columns between a row's key and its title.
const KEY_GAP: usize = 2;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Option<KeysHit> {
    let (query, record) = {
        let view = app.keys.as_ref()?;
        (view.query.clone(), view.record)
    };
    if area.width < 12 || area.height < 4 {
        return None;
    }
    let look = scrollbar::look(app, ScrollTarget::Keys);
    let theme = &app.theme;
    let base = theme.get("dialog");
    let dim = base.patch(theme.get("palette.detail"));
    let input = base.patch(theme.get("dialog.input"));
    let placeholder = input.patch(theme.get("palette.detail"));
    let selected_style = base.patch(theme.get("list.active"));

    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(base.patch(theme.get("dialog.border")))
        .title(" Keyboard Shortcuts ")
        .style(base);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let out = f.buffer_mut();
    // The search box, then a blank row.
    let search = Rect { height: 1, ..inner };
    out.set_style(search, input);
    let prompt = "Search keybindings: ";
    let px = inner.x + 1;
    out.set_stringn(px, inner.y, prompt, inner.width.saturating_sub(2) as usize, placeholder);
    let qx = px + prompt.width() as u16;
    if qx < inner.x + inner.width {
        let room = (inner.x + inner.width - qx) as usize;
        if record {
            out.set_stringn(qx, inner.y, "Press key combination…", room, placeholder);
        } else if query.is_empty() {
            out.set_stringn(qx, inner.y, "type to filter by name or key", room, placeholder);
        } else {
            out.set_stringn(qx, inner.y, &query, room, input);
        }
    }

    let list = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let view = app.keys.as_mut()?;
    view.height = list.height as usize;
    view.set_top(view.top);
    let top = view.top;
    let selected = view.selected;
    let scroll = view.scroll();

    let mut rows = list;
    let track = scrollbar::take_column(&mut rows);
    let x = rows.x + 1;
    let right = rows.x + rows.width;
    // Columns from every row, so they don't jump as the search narrows the list. The title
    // keeps at least `TITLE_MIN` columns; the context, then the source, are dropped when the
    // width can't hold them too.
    let source_w = view.all.iter().map(|r| r.source.width()).max().unwrap_or(0);
    let context_w = view.all.iter().map(|r| r.context.width()).max().unwrap_or(0);
    let key_w = view
        .all
        .iter()
        .map(|r| r.key.width())
        .max()
        .unwrap_or(0)
        .min(rows.width as usize / 3);
    const TITLE_MIN: usize = 20;
    let budget = rows.width.saturating_sub(2) as usize;
    let after_key = key_w + KEY_GAP + TITLE_MIN;
    let source_w = if budget >= after_key + 1 + source_w {
        source_w
    } else {
        0
    };
    let source_room = source_w + if source_w > 0 { 1 } else { 0 };
    let context_w = if budget >= after_key + 1 + context_w + source_room {
        context_w
    } else {
        0
    };
    let source_gap = if source_w > 0 { 1 + source_w as u16 } else { 0 };
    let context_gap = if context_w > 0 { 1 + context_w as u16 } else { 0 };
    let source_x = right - source_gap;
    let context_x = source_x - context_gap;
    let title_x = x + key_w as u16 + KEY_GAP as u16;
    let title_w = context_x.saturating_sub(title_x + 1) as usize;

    for (dy, row) in view.rows.iter().skip(top).take(rows.height as usize).enumerate() {
        let y = rows.y + dy as u16;
        let row_base = if top + dy == selected { selected_style } else { base };
        let line = Rect { y, height: 1, ..rows };
        out.set_style(line, row_base);
        out.set_stringn(x, y, &row.key, key_w, row_base.patch(theme.get("dialog.key")));
        out.set_stringn(title_x, y, row.command.title(), title_w, row_base);
        if context_x > title_x {
            out.set_stringn(
                context_x,
                y,
                &row.context,
                context_w,
                row_base.patch(theme.get("palette.detail")),
            );
        }
        out.set_stringn(
            source_x,
            y,
            &row.source,
            source_w,
            row_base.patch(theme.get("palette.detail")),
        );
    }
    if view.rows.is_empty() && rows.height > 0 {
        out.set_stringn(
            x,
            rows.y,
            "No matching keybindings",
            right.saturating_sub(x) as usize,
            dim,
        );
    }
    let scrollbar = track.map(|t| scrollbar::render(out, t, scroll, look, base, theme));
    Some(KeysHit { rect: area, scrollbar })
}
