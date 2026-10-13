//! Tab bar: one tab per open editor; `●` marks unsaved changes, `×` closes.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::layout::TabHit;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Vec<TabHit> {
    let theme = &app.theme;
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 {
        return Vec::new();
    }

    // Label and width for every tab: " name ● " / " name × ".
    let labels: Vec<(String, bool, bool)> = app
        .group
        .tabs
        .iter()
        .map(|t| {
            let buf = app.buffers.get(t.buffer);
            let name = buf.map(|b| b.name()).unwrap_or_default();
            (name, buf.is_some_and(|b| b.is_dirty()), buf.is_some_and(|b| b.deleted))
        })
        .collect();
    if labels.is_empty() {
        return Vec::new();
    }
    let width = |name: &str| name.width() as u16 + 4;

    // Scroll so the active tab is fully visible.
    let group = &mut app.group;
    group.scroll = group.scroll.min(group.active);
    loop {
        let used: u16 = labels[group.scroll..=group.active.min(labels.len() - 1)]
            .iter()
            .map(|(n, ..)| width(n))
            .sum();
        if used <= area.width || group.scroll >= group.active {
            break;
        }
        group.scroll += 1;
    }

    let mut hits = Vec::new();
    let mut x = area.x;
    let right = area.x + area.width;
    for (i, (name, dirty, deleted)) in labels.iter().enumerate().skip(group.scroll) {
        if x >= right {
            break;
        }
        let w = width(name).min(right - x);
        let style = if i == group.active {
            theme.get("tab.active")
        } else {
            theme.get("tab.inactive")
        };
        out.set_style(
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            style,
        );
        let name_style = if *deleted {
            style.patch(theme.get("tab.deleted"))
        } else {
            style
        };
        out.set_stringn(x + 1, area.y, name, w.saturating_sub(1) as usize, name_style);
        let close_x = x + w.saturating_sub(2);
        if close_x > x + 1 && close_x < right {
            out.set_stringn(close_x, area.y, if *dirty { "●" } else { "×" }, 1, style);
        }
        hits.push(TabHit {
            index: i,
            x0: x,
            x1: x + w,
            close_x,
        });
        x += w;
    }
    hits
}
