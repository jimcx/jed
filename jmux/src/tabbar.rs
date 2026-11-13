//! A group's tab bar: ` title × ` per shell, the title ending in its state glyph (`○` at the
//! prompt, `●` while a command runs, in `tab.shellError` after a failure). Tabs wider than the bar
//! scroll: `‹` / `›` at its ends (dimmed when there's nothing more that way), the wheel and the
//! arrows move it a tab at a time, and it jumps back to the active tab only when that (or the
//! bar's width) changes.

use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::group::Group;
use crate::hit::{TabArrows, TabHit};

/// Cells a tab takes: the title and ` ` before, ` × ` after.
fn width(title: &str) -> u16 {
    title.width() as u16 + 4
}

/// Draws the bar into `area` (one row); returns where its tabs and arrows landed.
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    theme: &Theme,
    focused: bool,
) -> (Vec<TabHit>, Option<TabArrows>) {
    // A group squeezed out of a tiny window can lie outside the frame.
    let area = area.intersection(f.area());
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 || area.height == 0 || group.tabs.is_empty() {
        return (Vec::new(), None);
    }
    let widths: Vec<u32> = group.tabs.iter().map(|t| width(&t.title) as u32).collect();
    let total: u32 = widths.iter().sum();

    let overflow = total > area.width as u32 && area.width >= 3;
    let (left, right) = if overflow {
        (area.x + 1, area.x + area.width - 1)
    } else {
        (area.x, area.x + area.width)
    };
    let inner = (right - left) as u32;
    let last = widths.len() - 1;
    // The furthest the bar scrolls: the last tabs fill it (the last one whole).
    let mut max_scroll = last;
    let mut used = widths[last];
    while max_scroll > 0 && used + widths[max_scroll - 1] <= inner {
        used += widths[max_scroll - 1];
        max_scroll -= 1;
    }
    // Scroll so the active tab is fully visible when it or the bar's width changed since the
    // last reveal; otherwise the bar stays where the wheel or an arrow left it.
    let active = group.active.min(last);
    if group.revealed != Some((active, area.width)) {
        group.revealed = Some((active, area.width));
        group.scroll = group.scroll.min(active);
        let mut used: u32 = widths[group.scroll..=active].iter().sum();
        while group.scroll < active && used > inner {
            used -= widths[group.scroll];
            group.scroll += 1;
        }
    }
    group.scroll = group.scroll.min(max_scroll);

    let mut arrows = None;
    if overflow {
        let arrow = |enabled: bool| {
            theme.get(if enabled {
                "tab.scrollArrow"
            } else {
                "tab.scrollArrow.disabled"
            })
        };
        out.set_string(area.x, area.y, "‹", arrow(group.scroll > 0));
        out.set_string(right, area.y, "›", arrow(group.scroll < max_scroll));
        arrows = Some(TabArrows {
            back: area.x,
            forward: right,
        });
    }

    let mut hits = Vec::new();
    let mut x = left;
    for (i, tab) in group.tabs.iter().enumerate().skip(group.scroll) {
        if x >= right {
            break;
        }
        let w = width(&tab.title).min(right - x);
        let style = match (i == group.active, focused) {
            (true, true) => theme.get("tab.active"),
            (true, false) => theme.get("tab.unfocusedActive"),
            _ => theme.get("tab.inactive"),
        };
        out.set_style(Rect::new(x, area.y, w, 1), style);
        out.set_stringn(x + 1, area.y, &tab.title, w.saturating_sub(1) as usize, style);
        // The state glyph is the title's last cell, in its own colour.
        let glyph_x = x + tab.title.width() as u16;
        if glyph_x < x + w && glyph_x < right {
            let key = if !tab.idle {
                "tab.shellRunning"
            } else if tab.error {
                "tab.shellError"
            } else {
                "tab.shellIdle"
            };
            let glyph = tab.title.chars().last().unwrap_or(' ');
            out.set_stringn(glyph_x, area.y, glyph.to_string(), 1, style.patch(theme.get(key)));
        }
        let close_x = x + w.saturating_sub(2);
        if close_x > x + 1 && close_x < right {
            out.set_stringn(close_x, area.y, "×", 1, style);
        }
        hits.push(TabHit {
            index: i,
            x0: x,
            x1: x + w,
            close_x,
        });
        x += w;
    }
    (hits, arrows)
}
