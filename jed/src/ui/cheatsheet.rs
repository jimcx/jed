//! The keyboard cheat sheet overlay (`crate::cheatsheet`): a box centred over the editor area,
//! its sections flowed down as many columns as fit, the one-column scroll bar on its right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Clear};

use super::scrollbar;
use crate::app::App;
use crate::cheatsheet::{self, COLUMN_GAP, KEY_GAP, Line};
use crate::layout::{CheatSheetHit, ScrollTarget};
use crate::scrollbar::Scrollable;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Option<CheatSheetHit> {
    let context = app.cheat_sheet.as_ref()?.context;
    if area.width < 12 || area.height < 4 {
        return None;
    }
    let look = scrollbar::look(app, ScrollTarget::CheatSheet);
    let theme = &app.theme;
    let base = theme.get("dialog");
    let key_style = base.patch(theme.get("dialog.key"));
    let heading = base.patch(theme.get("cheatSheet.heading"));

    let sections = cheatsheet::sections(&app.keymap, context);
    // Borders, a space either side and the scroll bar's column.
    let room = (area.width - 5) as usize;
    let cols = cheatsheet::layout(&sections, room);
    let content_w = (cols.iter().map(|c| c.width).sum::<usize>() + cols.len().saturating_sub(1) * COLUMN_GAP).min(room);
    let rows = cols.iter().map(|c| c.lines.len()).max().unwrap_or(0);
    let width = content_w as u16 + 5;
    let height = (rows as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(base.patch(theme.get("dialog.border")))
        .title(" Keyboard Cheat Sheet ")
        .style(base);
    let mut inner = block.inner(rect);
    f.render_widget(block, rect);
    let track = scrollbar::take_column(&mut inner);

    let sheet = app.cheat_sheet.as_mut()?;
    sheet.height = inner.height as usize;
    sheet.rows = rows;
    sheet.set_top(sheet.top);
    let top = sheet.top;
    let scroll = sheet.scroll();

    let out = f.buffer_mut();
    let right = inner.x + inner.width.saturating_sub(1);
    let mut x = inner.x + 1;
    for col in &cols {
        if x >= right {
            break;
        }
        let room = |at: u16| right.saturating_sub(at) as usize;
        for (dy, line) in col.lines.iter().skip(top).take(inner.height as usize).enumerate() {
            let y = inner.y + dy as u16;
            match line {
                Line::Header(title) => {
                    out.set_stringn(x, y, title, room(x), heading);
                }
                Line::Row { row, key_width } => {
                    out.set_stringn(x, y, &row.key, room(x), key_style);
                    let tx = x + (key_width + KEY_GAP) as u16;
                    if tx < right {
                        out.set_stringn(tx, y, &row.title, room(tx), base);
                    }
                }
                Line::Blank => {}
            }
        }
        x = x.saturating_add((col.width + COLUMN_GAP) as u16);
    }
    let scrollbar = track.map(|t| scrollbar::render(out, t, scroll, look, base, theme));
    Some(CheatSheetHit { rect, scrollbar })
}
