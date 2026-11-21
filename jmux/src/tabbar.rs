//! A group's tab bar: ` title × ` per shell (padded by `TabMetrics`: the touch layout is three rows
//! tall with `  title  ×  ` and wider targets), the title ending in its state glyph (`○` at the
//! prompt, `●` while a command runs, in `tab.shellError` after a failure). Tabs wider than the bar
//! scroll: `‹` / `›` at its ends (dimmed when there's nothing more that way), the wheel and the
//! arrows move it a tab at a time, a touch swipe by columns (`Group::shift`, the first tab drawn
//! cut at the left), and it jumps back to the active tab only when that (or the bar's width)
//! changes. The touch layout ends the bar in a `+` that opens a tab in its group.

use jtui::tabs::TabMetrics;
use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::group::Group;
use crate::hit::{BarHit, TabArrows, TabHit};

/// What a drag marks on a bar.
#[derive(Debug, Default, Clone, Copy)]
pub struct Marks {
    /// The gap a dragged tab would land in, marked with one cell across the bar's rows.
    pub drop: Option<usize>,
    /// The tab a finger has picked up (touch layout), drawn as `tab.lifted`.
    pub lifted: Option<usize>,
}

/// The furthest a bar `width` cells wide (without its `+`) scrolls, given its tabs' `widths`: the
/// last tabs fill it (the last one whole). Shared by `render` and the touch swipe.
pub fn max_scroll(widths: &[u32], width: u16, metrics: TabMetrics) -> usize {
    let total: u32 = widths.iter().sum();
    let overflow = total > u32::from(width) && width > 2 * metrics.arrow;
    let inner = u32::from(width) - if overflow { 2 * u32::from(metrics.arrow) } else { 0 };
    let mut max = widths.len().saturating_sub(1);
    let mut used = widths.get(max).copied().unwrap_or(0);
    while max > 0 && used + widths[max - 1] <= inner {
        used += widths[max - 1];
        max -= 1;
    }
    max
}

/// The furthest a bar `width` cells wide (without its `+`) scrolls in columns: the last tab's
/// right edge at the bar's. What a touch swipe stops at.
pub fn max_offset(widths: &[u32], width: u16, metrics: TabMetrics) -> u32 {
    let total: u32 = widths.iter().sum();
    if total <= u32::from(width) || width <= 2 * metrics.arrow {
        return 0;
    }
    total - (u32::from(width) - 2 * u32::from(metrics.arrow))
}

/// Draws `text` starting at column `x` (left of `lo` when the tab is cut there) on row `y`, only
/// the characters wholly within `lo..hi`.
fn put_clipped(out: &mut Buffer, x: i32, y: u16, text: &str, lo: u16, hi: u16, style: Style) {
    let mut at = x;
    for (b, ch) in text.char_indices() {
        let w = ch.width().unwrap_or(0) as i32;
        if w > 0 && at >= i32::from(lo) {
            if at < i32::from(hi) {
                out.set_stringn(at as u16, y, &text[b..], (i32::from(hi) - at) as usize, style);
            }
            return;
        }
        at += w;
    }
}

/// Draws the bar into `area` (`metrics.rows` tall: the title and `×` on its middle row, the other
/// rows filled with each tab's background); returns where its tabs, arrows and `+` landed.
/// `marks` is what a drag shows on it.
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    theme: &Theme,
    metrics: &TabMetrics,
    focused: bool,
    marks: Marks,
) -> BarHit {
    // A group squeezed out of a tiny window can lie outside the frame.
    let mut area = area.intersection(f.area());
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 || area.height == 0 {
        return BarHit::default();
    }
    let text_y = area.y + metrics.text_row(area.height);
    // The `+` takes the bar's right end; the tabs and arrows share what's left.
    let mut new_tab = None;
    if metrics.new_tab > 0 && area.width > metrics.new_tab {
        area.width -= metrics.new_tab;
        let x = area.x + area.width;
        let style = theme.get("tab.scrollArrow");
        out.set_style(Rect::new(x, area.y, metrics.new_tab, area.height), style);
        out.set_string(x + metrics.new_tab / 2, text_y, "+", style);
        new_tab = Some(x..x + metrics.new_tab);
    }
    if group.tabs.is_empty() {
        return BarHit {
            new_tab,
            ..BarHit::default()
        };
    }
    let width = |title: &str| metrics.tab_width(title.width() as u16);
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
    let max_scroll = max_scroll(&widths, area.width, *metrics);
    // Scroll so the active tab is fully visible when it or the bar's width changed since the
    // last reveal; otherwise the bar stays where the wheel or an arrow left it.
    let active = group.active.min(last);
    if group.revealed != Some((active, area.width)) {
        group.revealed = Some((active, area.width));
        if group.scroll >= active {
            (group.scroll, group.shift) = (active, 0);
        }
        let mut used: u32 = widths[group.scroll..=active].iter().sum::<u32>() - u32::from(group.shift);
        while group.scroll < active && used > inner {
            used -= widths[group.scroll] - u32::from(group.shift);
            group.scroll += 1;
            group.shift = 0;
        }
    }
    if group.scroll >= max_scroll {
        (group.scroll, group.shift) = (max_scroll, 0);
    }
    group.shift = group.shift.min(widths[group.scroll].saturating_sub(1) as u16);

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
        let (back_style, forward_style) = (
            arrow(group.scroll > 0 || group.shift > 0),
            arrow(group.scroll < max_scroll),
        );
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
    // Where the next tab starts: left of `left` for a first tab cut by a swipe.
    let mut vx = i32::from(left) - i32::from(group.shift);
    for (i, tab) in group.tabs.iter().enumerate().skip(group.scroll) {
        if vx >= i32::from(right) {
            break;
        }
        // Cut at the right edge as before (its `×` moves in); at the left only drawn from `left`.
        let w = i32::from(width(&tab.title)).min(i32::from(right) - vx);
        let (x0, x1) = (vx.max(i32::from(left)) as u16, (vx + w) as u16);
        let style = match (i == group.active, focused) {
            _ if marks.lifted == Some(i) => theme.get("tab.lifted"),
            (true, true) => theme.get("tab.active"),
            (true, false) => theme.get("tab.unfocusedActive"),
            _ => theme.get("tab.inactive"),
        };
        let pad = i32::from(metrics.pad);
        out.set_style(Rect::new(x0, area.y, x1 - x0, area.height), style);
        put_clipped(out, vx + pad, text_y, &tab.title, x0, x1, style);
        // The state glyph is the title's last cell, in its own colour.
        let glyph_x = vx + pad - 1 + tab.title.width() as i32;
        if glyph_x >= i32::from(x0) && glyph_x < i32::from(x1) {
            let key = if !tab.idle {
                "tab.shellRunning"
            } else if tab.error {
                "tab.shellError"
            } else {
                "tab.shellIdle"
            };
            let glyph = tab.title.chars().last().unwrap_or(' ');
            out.set_stringn(
                glyph_x as u16,
                text_y,
                glyph.to_string(),
                1,
                style.patch(theme.get(key)),
            );
        }
        let close_x = vx + (w - pad - 1).max(0);
        let close_shown = close_x > vx + pad && close_x >= i32::from(x0) && close_x < i32::from(right);
        if close_shown {
            out.set_stringn(close_x as u16, text_y, "×", 1, style);
        }
        // A `×` cut off at the left has no target; the press there is on the tab.
        let (close_x0, close_x1) = if close_x >= i32::from(x0) {
            let reach = i32::from(metrics.close_reach());
            (
                (close_x - reach).max(i32::from(x0)) as u16,
                (close_x + reach + 1).min(i32::from(x1)).max(close_x + 1) as u16,
            )
        } else {
            (x0, x0)
        };
        hits.push(TabHit {
            index: i,
            x0,
            x1,
            close_x0,
            close_x1,
        });
        vx += w;
    }
    if let Some(index) = marks.drop {
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
    BarHit {
        tabs: hits,
        arrows,
        new_tab,
    }
}
