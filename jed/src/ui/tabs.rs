//! Tab bar: one tab per open editor; `●` marks unsaved changes, `×` closes; a file deleted on
//! disk is struck through, with "(deleted)" after its name (VS Code). Tabs wider than the bar
//! scroll: `‹` / `›` at the bar's ends (dimmed when there's nothing more that way) and the wheel
//! move it a tab at a time, a touch swipe by columns (`Group::shift`, the first tab drawn cut at
//! the left); it jumps back to the active tab only when the active tab changes.
//! While a tab is dragged, `drop` marks the gap it would land in (`tab.dropMarker`). With two or
//! more groups, each bar's top-right corner shows the group's number (`tab.groupNumber`), which
//! takes the bar's last cells: the `›` arrow then sits one cell left of it. A shell tab's last
//! glyph (`●` running / `○` idle) is drawn in its state colour.
//!
//! `TabMetrics` (`App::tab_metrics`) sets the bar's rows and padding: the touch layout is three
//! rows with `  name  ×  `, the label, `×`, arrows and group number on the middle row (the other
//! rows filled with the tab's background), three-cell close and arrow targets, a `+` (new tab) at
//! the bar's right end, the drop marker across all rows and a lifted tab in `tab.lifted`.

use jtui::tabs::TabMetrics;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::buffer::Buffers;
use crate::group::{Group, Tab};
use crate::layout::{TabArrows, TabHit};
use crate::theme::Theme;

/// What the tab bar drew: each tab shown, and the scroll arrows when the tabs overflow.
#[derive(Debug, Default)]
pub struct TabBarHit {
    pub tabs: Vec<TabHit>,
    pub arrows: Option<TabArrows>,
    /// The `+` target's x range (touch layout).
    pub new_tab: Option<std::ops::Range<u16>>,
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

/// A tab's width in a bar for `metrics`.
pub fn tab_width(name: &str, metrics: &TabMetrics) -> u16 {
    metrics.tab_width(name.width() as u16)
}

/// One tab's label: its text, and how it is marked.
struct Label {
    name: String,
    dirty: bool,
    deleted: bool,
    /// A shell tab's state glyph (the name's last character) and the theme key for its colour.
    mark: Option<&'static str>,
}

/// Label of every tab of `group`.
fn labels(group: &Group, buffers: &Buffers) -> Vec<Label> {
    group
        .tabs
        .iter()
        .map(|t| match t {
            // A diff tab: `name (Working Tree)`.
            Tab::Diff(d) => Label {
                name: d.title(),
                dirty: false,
                deleted: false,
                mark: None,
            },
            // A hex tab: its file's name, never dirty.
            Tab::Hex(h) => {
                let mut name = h.name();
                if h.deleted {
                    name.push_str(" (deleted)");
                }
                Label {
                    name,
                    dirty: false,
                    deleted: h.deleted,
                    mark: None,
                }
            }
            // A Markdown preview: `Preview name`, never dirty.
            Tab::Markdown(m) => Label {
                name: m.title(),
                dirty: false,
                deleted: false,
                mark: None,
            },
            // A shell tab: the command that runs (or the shell) and its state glyph, never dirty.
            Tab::Shell(s) => Label {
                name: s.title.clone(),
                dirty: false,
                deleted: false,
                mark: Some(if !s.idle {
                    "tab.shellRunning"
                } else if s.error {
                    "tab.shellError"
                } else {
                    "tab.shellIdle"
                }),
            },
            Tab::Text(t) => {
                let buf = buffers.get(t.buffer);
                let deleted = buf.is_some_and(|b| b.deleted);
                let mut name = buf.map(|b| b.name()).unwrap_or_default();
                if deleted {
                    name.push_str(" (deleted)");
                }
                Label {
                    name,
                    dirty: buf.is_some_and(|b| b.is_dirty()),
                    deleted,
                    mark: None,
                }
            }
        })
        .collect()
}

/// Every tab's width in a bar for `metrics` (what `render` lays out and the touch swipe scrolls by).
pub fn widths(group: &Group, buffers: &Buffers, metrics: &TabMetrics) -> Vec<u32> {
    labels(group, buffers)
        .iter()
        .map(|l| u32::from(tab_width(&l.name, metrics)))
        .collect()
}

/// The keys that focus the terminal area's first three groups (`Command::FocusTerminal`), shown in their bars.
pub const TERMINAL_KEYS: [char; 3] = ['t', 'y', 'u'];

/// `active_group`: the focused group's active tab is bright; other groups' active tabs are dimmer.
/// `drop`: the gap a dragged tab would land in, drawn as one marker cell.
/// `number`: the group's 1-based place in reading order (a terminal area's group: the letter of its
/// Cmd key, `TERMINAL_KEYS`), or `None` with a single group.
/// `lifted`: the tab a finger has picked up (touch layout).
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    buffers: &Buffers,
    theme: &Theme,
    active_group: bool,
    drop: Option<usize>,
    number: Option<String>,
    metrics: &TabMetrics,
    lifted: Option<usize>,
) -> TabBarHit {
    // A group squeezed out of a tiny window can lie outside the frame.
    let area = area.intersection(f.area());
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 || area.height == 0 {
        return TabBarHit::default();
    }

    // The number takes the bar's last cells; the tabs, arrows and drop marker stay in `bar`,
    // left of it. Clicking those cells is a click on the bar (`Hit::TabBar`), which focuses it.
    let label = number;
    let label_w = label.as_ref().map_or(0, |l| l.width() as u16).min(area.width);
    let bar = Rect {
        width: area.width - label_w,
        ..area
    };
    let text_y = area.y + metrics.text_row(area.height);
    if let Some(label) = &label {
        out.set_stringn(
            area.x + bar.width,
            text_y,
            label,
            label_w as usize,
            theme.get("tab.groupNumber"),
        );
    }
    if bar.width == 0 {
        return TabBarHit::default();
    }
    // The touch layout's `+` takes the bar's right end; the tabs and arrows share what's left.
    let mut bar = bar;
    let mut new_tab = None;
    if metrics.new_tab > 0 && bar.width > metrics.new_tab {
        bar.width -= metrics.new_tab;
        let x = bar.x + bar.width;
        let style = theme.get("tab.scrollArrow");
        out.set_style(Rect::new(x, bar.y, metrics.new_tab, bar.height), style);
        out.set_string(x + metrics.new_tab / 2, text_y, "+", style);
        new_tab = Some(x..x + metrics.new_tab);
    }

    // Label and width for every tab: " name ● " / " name × ".
    let labels = labels(group, buffers);
    if labels.is_empty() {
        return TabBarHit {
            new_tab,
            ..TabBarHit::default()
        };
    }
    let width = |name: &str| tab_width(name, metrics);
    let widths: Vec<u32> = labels.iter().map(|l| width(&l.name) as u32).collect();
    let total: u32 = widths.iter().sum();

    // Tabs wider than the bar: `‹` / `›` at its ends scroll it, the tabs between them.
    let overflow = total > bar.width as u32 && bar.width > 2 * metrics.arrow;
    let (left, right) = if overflow {
        (bar.x + metrics.arrow, bar.x + bar.width - metrics.arrow)
    } else {
        (bar.x, bar.x + bar.width)
    };
    let inner = (right - left) as u32;
    let last = labels.len() - 1;
    let max_scroll = max_scroll(&widths, bar.width, *metrics);

    // Scroll so the active tab is fully visible, when it or the bar's width changed since the
    // last reveal; otherwise the bar stays where the wheel or an arrow left it.
    let active = group.active.min(last);
    if group.revealed != Some((active, bar.width)) {
        group.revealed = Some((active, bar.width));
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
        let (back_style, forward_style) = (
            arrow(group.scroll > 0 || group.shift > 0),
            arrow(group.scroll < max_scroll),
        );
        let (back, forward) = (bar.x..left, right..bar.x + bar.width);
        if metrics.arrow > 1 {
            out.set_style(Rect::new(back.start, bar.y, metrics.arrow, bar.height), back_style);
            out.set_style(
                Rect::new(forward.start, bar.y, metrics.arrow, bar.height),
                forward_style,
            );
        }
        out.set_string(back.start + metrics.arrow_reach(), text_y, "‹", back_style);
        out.set_string(forward.start + metrics.arrow_reach(), text_y, "›", forward_style);
        arrows = Some(TabArrows { back, forward });
    }

    let mut hits = Vec::new();
    // Where the next tab starts: left of `left` for a first tab cut by a swipe.
    let mut vx = i32::from(left) - i32::from(group.shift);
    for (i, label) in labels.iter().enumerate().skip(group.scroll) {
        if vx >= i32::from(right) {
            break;
        }
        let Label {
            name,
            dirty,
            deleted,
            mark,
        } = label;
        // Cut at the right edge as before (its `×` moves in); at the left only drawn from `left`.
        let w = i32::from(width(name)).min(i32::from(right) - vx);
        let (x0, x1) = (vx.max(i32::from(left)) as u16, (vx + w) as u16);
        let style = match (i == group.active, active_group) {
            _ if lifted == Some(i) => theme.get("tab.lifted"),
            (true, true) => theme.get("tab.active"),
            (true, false) => theme.get("tab.unfocusedActive"),
            _ => theme.get("tab.inactive"),
        };
        out.set_style(
            Rect {
                x: x0,
                y: area.y,
                width: x1 - x0,
                height: area.height,
            },
            style,
        );
        let name_style = if *deleted {
            style.patch(theme.get("tab.deleted"))
        } else {
            style
        };
        let pad = i32::from(metrics.pad);
        put_clipped(out, vx + pad, text_y, name, x0, x1, name_style);
        if let Some(key) = mark
            && let Some(glyph) = name.chars().last()
        {
            // The state glyph is the label's last cell, in its own colour (the tab's otherwise).
            let glyph_x = vx + pad - 1 + name.width() as i32;
            if glyph_x >= i32::from(x0) && glyph_x < i32::from(x1) {
                out.set_stringn(
                    glyph_x as u16,
                    text_y,
                    glyph.to_string(),
                    1,
                    style.patch(theme.get(key)),
                );
            }
        }
        let close_x = vx + (w - pad - 1).max(0);
        if close_x > vx + pad && close_x >= i32::from(x0) && close_x < i32::from(right) {
            out.set_stringn(close_x as u16, text_y, if *dirty { "●" } else { "×" }, 1, style);
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
    if let Some(index) = drop {
        // The gap before tab `index`, or after the last tab drawn at the bar's end; kept inside
        // the drawn area (between the arrows, while they are there). On a scrolled bar a gap
        // can name a tab that isn't drawn: it is the one just after the last drawn tab.
        let x = match hits.iter().find(|t| t.index == index).map(|t| t.x0) {
            Some(x) => x,
            None => match hits.last() {
                Some(t) if index > t.index => t.x1,
                _ => left,
            },
        };
        let x = x.clamp(left, right.saturating_sub(1));
        out.set_style(
            Rect {
                x,
                y: area.y,
                width: 1,
                height: area.height,
            },
            theme.get("tab.dropMarker"),
        );
    }
    TabBarHit {
        tabs: hits,
        arrows,
        new_tab,
    }
}
