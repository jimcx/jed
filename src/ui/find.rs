//! The find / replace widget, at the top right of the active editor as in VS Code:
//! the find input, the option toggles (case, whole word, regex) and "3 of 41"; the replace input
//! below it when shown; an invalid regex's message on a last row. Over a hex tab: a fourth toggle,
//! `0x` (hex pairs or ASCII text), whole word and regex drawn disabled, no replace row, the byte
//! search's count ("…" while it runs) and an invalid hex query's message as the error.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Clear;
use unicode_width::UnicodeWidthChar;

use crate::app::{App, Focus};
use crate::field::TextField;
use crate::find::Field;
use crate::layout::FindHit;

/// Widest the widget gets.
const MAX_WIDTH: u16 = 64;
/// Room for the count ("12345 of 12345").
const COUNT_WIDTH: u16 = 14;
/// On a hex tab ("99999 of 100000+").
const HEX_COUNT_WIDTH: u16 = 17;
const TOGGLES: [&str; 4] = ["Aa", "ab", ".*", "0x"];

/// Draws the widget over `area` (the active group's editor) if it's open; returns where it is.
pub fn render(f: &mut Frame, area: Rect, app: &App) -> Option<FindHit> {
    let find = &app.find;
    let hex = app.active_hex();
    let sel = app.group().active_tab().map(|t| *t.view.selections.primary());
    if (hex.is_none() && sel.is_none()) || !find.visible || area.width < 24 || area.height < 3 {
        return None;
    }
    let theme = &app.theme;
    let base = theme.get("dialog");
    let input = base.patch(theme.get("dialog.input"));
    let dim = base.patch(theme.get("palette.detail"));
    let selected = input.patch(theme.get("editor.selection"));
    let error = match hex {
        Some(_) => find.hex.error().map(|e| e.to_string()),
        None => find.error().map(|e| e.to_string()),
    };
    // A hex tab has no replace.
    let replace_visible = find.replace_visible && hex.is_none();

    let rows = 1 + u16::from(replace_visible) + u16::from(error.is_some());
    let width = area.width.min(MAX_WIDTH);
    let rect = Rect {
        x: area.x + area.width - width,
        y: area.y,
        width,
        height: rows,
    };
    f.render_widget(Clear, rect);
    f.buffer_mut().set_style(rect, base);

    // " [field] Aa ab .* count ", with 0x after .* on a hex tab.
    let shown = if hex.is_some() { 4 } else { 3 };
    let count_width = if hex.is_some() { HEX_COUNT_WIDTH } else { COUNT_WIDTH };
    let toggles_w = 3 * shown as u16;
    let field_w = width.saturating_sub(1 + toggles_w + 1 + count_width + 1).max(4);
    let field_x = rect.x + 1;
    let find_rect = Rect {
        x: field_x,
        y: rect.y,
        width: field_w,
        height: 1,
    };
    let out = f.buffer_mut();
    let mut cursor = draw_field(out, find_rect, &find.query, "Find", input, selected, dim);

    let options = app.find_options;
    let on = [options.case_sensitive, options.whole_word, options.regex, options.hex];
    let mut toggles = [Rect::default(); 4];
    for (i, label) in TOGGLES.iter().enumerate().take(shown) {
        let x = field_x + field_w + 1 + 3 * i as u16;
        // Whole word and regex don't apply to bytes.
        let style = if hex.is_some() && (i == 1 || i == 2) {
            base.patch(theme.get("find.option.disabled"))
        } else if on[i] {
            base.patch(theme.get("find.option.active"))
        } else {
            base.patch(theme.get("find.option"))
        };
        out.set_stringn(x, rect.y, label, 2, style);
        toggles[i] = Rect {
            x,
            y: rect.y,
            width: 2,
            height: 1,
        };
    }
    let count = match (hex, sel) {
        (Some(h), _) => find.hex.count_label(h.selection()),
        (None, Some(sel)) => find.count_label((sel.start(), sel.end())),
        (None, None) => String::new(),
    };
    let count_x = field_x + field_w + toggles_w + 1;
    out.set_stringn(count_x, rect.y, &count, count_width as usize, dim);

    let mut replace = None;
    if replace_visible {
        let r = Rect {
            y: rect.y + 1,
            ..find_rect
        };
        let replace_cursor = draw_field(out, r, &find.replace, "Replace", input, selected, dim);
        if find.field == Field::Replace {
            cursor = replace_cursor;
        }
        replace = Some(Rect { y: r.y, ..rect });
    }
    if let Some(e) = &error {
        let y = rect.y + rows - 1;
        let style = base.patch(theme.get("dialog.error"));
        out.set_stringn(field_x, y, e, (width - 2) as usize, style);
    }

    if app.ui.focus == Focus::Find && app.prompt.is_none() && app.palette.is_none() {
        f.set_cursor_position(cursor);
    }
    Some(FindHit {
        area: rect,
        replace: replace.map(|r| Rect { height: 1, ..r }),
        toggles,
    })
}

/// One input, scrolled sideways to keep the cursor in view (all of it from column 0 when it
/// fits; a placeholder when empty), its selected text in `selected`; returns where its cursor is.
pub(super) fn draw_field(
    out: &mut ratatui::buffer::Buffer,
    r: Rect,
    field: &TextField,
    placeholder: &str,
    style: Style,
    selected: Style,
    dim: Style,
) -> Position {
    out.set_style(r, style);
    if field.text.is_empty() {
        out.set_stringn(r.x, r.y, placeholder, r.width as usize, style.patch(dim));
        return Position { x: r.x, y: r.y };
    }
    let width = r.width as usize;
    let cursor_col: usize = field.text[..field.cursor].chars().map(|c| c.width().unwrap_or(0)).sum();
    let scroll = (cursor_col + 1).saturating_sub(width);
    let sel = field.selection().unwrap_or_default();
    let mut col = 0;
    for (i, c) in field.text.char_indices() {
        let w = c.width().unwrap_or(0);
        if col >= scroll && col + w - scroll <= width {
            let s = if sel.contains(&i) { selected } else { style };
            out.set_stringn(r.x + (col - scroll) as u16, r.y, c.to_string(), w, s);
        }
        col += w;
    }
    Position {
        x: r.x + (cursor_col - scroll).min(width.saturating_sub(1)) as u16,
        y: r.y,
    }
}
