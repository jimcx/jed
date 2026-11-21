//! Draws the tab menu (`menu`, driven by `app/menu.rs`) over the groups, its bottom edge on the row
//! above the status bar, and writes its `HitMap` part.

use ratatui::Frame;
use ratatui::widgets::Clear;

use crate::app::App;
use crate::hit::MenuHit;
use crate::menu::{self, Row, Target};

pub fn render(f: &mut Frame, app: &mut App) {
    if app.menu.is_none() {
        return;
    }
    let items = app.menu_items();
    let tabs = items.iter().filter(|i| matches!(i.target, Target::Tab(_))).count();
    let content = menu::rows(tabs, items.len());
    let screen = f.area();
    let area = menu::area(screen.height, screen.width, content.len());
    if area.width == 0 || area.height == 0 {
        return;
    }
    let max_scroll = (content.len() as u16).saturating_sub(area.height);
    let Some(state) = &mut app.menu else {
        return;
    };
    (state.view_rows, state.max_scroll) = (area.height, max_scroll);
    state.scroll = state.scroll.min(max_scroll);
    let (scroll, highlight) = (usize::from(state.scroll), state.highlight);

    let theme = &app.theme;
    let base = theme.get("menu");
    // Blank what was drawn under the menu (the shells' text), not just restyle it.
    f.render_widget(Clear, area);
    let out = f.buffer_mut();
    out.set_style(area, base);
    let shown = &content[scroll..scroll + usize::from(area.height)];
    let group = app.groups.active_group();
    for (r, row) in shown.iter().enumerate() {
        let y = area.y + r as u16;
        match *row {
            Row::Text(i) => {
                let item = &items[i];
                let style = if highlight == Some(i) {
                    base.patch(theme.get("menu.highlight"))
                } else if item.active {
                    base.patch(theme.get("menu.active"))
                } else {
                    base
                };
                out.set_style(ratatui::layout::Rect::new(area.x, y, area.width, 1), style);
                out.set_stringn(area.x, y, format!("  {}  ", item.label), usize::from(area.width), style);
                // A tab's state glyph, the title's last cell, in its own colour (as the tab bar).
                if let Target::Tab(t) = item.target
                    && let Some(tab) = group.tabs.get(t)
                {
                    let glyph_x = area.x + 1 + item.label.chars().count() as u16;
                    if glyph_x < area.x + area.width {
                        let key = if !tab.idle {
                            "tab.shellRunning"
                        } else if tab.error {
                            "tab.shellError"
                        } else {
                            "tab.shellIdle"
                        };
                        let glyph = item.label.chars().last().unwrap_or(' ');
                        out.set_stringn(glyph_x, y, glyph.to_string(), 1, style.patch(theme.get(key)));
                    }
                }
            }
            Row::Separator => {
                let style = base.patch(theme.get("menu.separator"));
                out.set_stringn(
                    area.x,
                    y,
                    "─".repeat(usize::from(area.width)),
                    usize::from(area.width),
                    style,
                );
            }
            Row::Pad(_) => {}
        }
    }
    app.hit.menu = Some(MenuHit {
        area,
        rows: shown.to_vec(),
    });
}
