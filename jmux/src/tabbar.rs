//! A group's tab bar: ` title × ` per shell (padded by `TabMetrics`: the touch layout is three rows
//! tall with `  title  ×  ` and wider targets), the title ending in its state glyph (`○` at the
//! prompt, `●` while a command runs, in `tab.shellError` after a failure). Tabs wider than the bar
//! scroll: `‹` / `›` at its ends (dimmed when there's nothing more that way), the wheel and the
//! arrows move it a tab at a time, and it jumps back to the active tab only when that (or the
//! bar's width) changes.

use jtui::tabs::TabMetrics;
use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::group::Group;
use crate::hit::{TabArrows, TabHit};

/// Draws the bar into `area` (`metrics.rows` tall: the title and `×` on its middle row, the other
/// rows filled with each tab's background); returns where its tabs and arrows landed. `drop` is
/// the gap a dragged tab would land in, marked with one cell across the bar's rows.
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    theme: &Theme,
    metrics: &TabMetrics,
    focused: bool,
    drop: Option<usize>,
) -> (Vec<TabHit>, Option<TabArrows>) {
    // A group squeezed out of a tiny window can lie outside the frame.
    let area = area.intersection(f.area());
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 || area.height == 0 || group.tabs.is_empty() {
        return (Vec::new(), None);
    }
    let width = |title: &str| metrics.tab_width(title.width() as u16);
    let text_y = area.y + metrics.text_row(area.height);
    let widths: Vec<u32> = group.tabs.iter().map(|t| width(&t.title) as u32).collect();
    let total: u32 = widths.iter().sum();

    let overflow = total > area.width as u32 && area.width > 2 * metrics.arrow;
    let (left, right) = if overflow {
        (area.x + metrics.arrow, area.x + area.width - metrics.arrow)
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
        let (back, forward) = (area.x..left, right..area.x + area.width);
        let (back_style, forward_style) = (arrow(group.scroll > 0), arrow(group.scroll < max_scroll));
        out.set_style(Rect::new(back.start, area.y, metrics.arrow, area.height), back_style);
        out.set_style(
            Rect::new(forward.start, area.y, metrics.arrow, area.height),
            forward_style,
        );
        out.set_string(back.start + metrics.arrow_reach(), text_y, "‹", back_style);
        out.set_string(forward.start + metrics.arrow_reach(), text_y, "›", forward_style);
        arrows = Some(TabArrows { back, forward });
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
        out.set_style(Rect::new(x, area.y, w, area.height), style);
        out.set_stringn(
            x + metrics.pad,
            text_y,
            &tab.title,
            w.saturating_sub(metrics.pad) as usize,
            style,
        );
        // The state glyph is the title's last cell, in its own colour.
        let glyph_x = x + metrics.pad - 1 + tab.title.width() as u16;
        if glyph_x < x + w && glyph_x < right {
            let key = if !tab.idle {
                "tab.shellRunning"
            } else if tab.error {
                "tab.shellError"
            } else {
                "tab.shellIdle"
            };
            let glyph = tab.title.chars().last().unwrap_or(' ');
            out.set_stringn(glyph_x, text_y, glyph.to_string(), 1, style.patch(theme.get(key)));
        }
        let close_x = x + w.saturating_sub(metrics.pad + 1);
        if close_x > x + metrics.pad && close_x < right {
            out.set_stringn(close_x, text_y, "×", 1, style);
        }
        hits.push(TabHit {
            index: i,
            x0: x,
            x1: x + w,
            close_x0: close_x.saturating_sub(metrics.close_reach()).max(x),
            close_x1: (close_x + metrics.close_reach() + 1).min(x + w).max(close_x + 1),
        });
        x += w;
    }
    if let Some(index) = drop {
        // The gap before tab `index`, or after the last tab drawn; on a scrolled bar a gap can
        // name a tab that isn't drawn, which is the one just after the last drawn.
        let gap_x = match hits.iter().find(|t| t.index == index).map(|t| t.x0) {
            Some(x) => x,
            None => match hits.last() {
                Some(t) if index > t.index => t.x1,
                _ => left,
            },
        };
        let gap_x = gap_x.clamp(left, right.saturating_sub(1));
        out.set_style(Rect::new(gap_x, area.y, 1, area.height), theme.get("tab.dropMarker"));
    }
    (hits, arrows)
}
