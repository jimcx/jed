//! Tab bar: one tab per open editor; `●` marks unsaved changes, `×` closes; a file deleted on
//! disk is struck through, with "(deleted)" after its name (VS Code). Tabs wider than the bar
//! scroll: `‹` / `›` at the bar's ends (dimmed when there's nothing more that way) and the wheel
//! move it a tab at a time; it jumps back to the active tab only when the active tab changes.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::buffer::Buffers;
use crate::group::{Group, Tab};
use crate::layout::{TabArrows, TabHit};
use crate::theme::Theme;

/// What the tab bar drew: each tab shown, and the scroll arrows when the tabs overflow.
#[derive(Debug, Default)]
pub struct TabBarHit {
    pub tabs: Vec<TabHit>,
    pub arrows: Option<TabArrows>,
}

/// `active_group`: the focused group's active tab is bright; other groups' active tabs are dimmer.
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    buffers: &Buffers,
    theme: &Theme,
    active_group: bool,
) -> TabBarHit {
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 {
        return TabBarHit::default();
    }

    // Label and width for every tab: " name ● " / " name × ".
    let labels: Vec<(String, bool, bool)> = group
        .tabs
        .iter()
        .map(|t| match t {
            // A diff tab: `name (Working Tree)`.
            Tab::Diff(d) => (d.title(), false, false),
            // A hex tab: its file's name, never dirty.
            Tab::Hex(h) => {
                let mut name = h.name();
                if h.deleted {
                    name.push_str(" (deleted)");
                }
                (name, false, h.deleted)
            }
            // A shell tab: its title, never dirty.
            Tab::Shell(s) => (s.title.clone(), false, false),
            Tab::Text(t) => {
                let buf = buffers.get(t.buffer);
                let deleted = buf.is_some_and(|b| b.deleted);
                let mut name = buf.map(|b| b.name()).unwrap_or_default();
                if deleted {
                    name.push_str(" (deleted)");
                }
                (name, buf.is_some_and(|b| b.is_dirty()), deleted)
            }
        })
        .collect();
    if labels.is_empty() {
        return TabBarHit::default();
    }
    let width = |name: &str| name.width() as u16 + 4;
    let widths: Vec<u32> = labels.iter().map(|(n, ..)| width(n) as u32).collect();
    let total: u32 = widths.iter().sum();

    // Tabs wider than the bar: `‹` / `›` at its ends scroll it, the tabs between them.
    let overflow = total > area.width as u32 && area.width >= 3;
    let (left, right) = if overflow {
        (area.x + 1, area.x + area.width - 1)
    } else {
        (area.x, area.x + area.width)
    };
    let inner = (right - left) as u32;
    let last = labels.len() - 1;
    // The furthest the bar scrolls: the last tabs fill it (the last one whole).
    let mut max_scroll = last;
    let mut used = widths[last];
    while max_scroll > 0 && used + widths[max_scroll - 1] <= inner {
        used += widths[max_scroll - 1];
        max_scroll -= 1;
    }

    // Scroll so the active tab is fully visible, when it or the bar's width changed since the
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
    for (i, (name, dirty, deleted)) in labels.iter().enumerate().skip(group.scroll) {
        if x >= right {
            break;
        }
        let w = width(name).min(right - x);
        let style = match (i == group.active, active_group) {
            (true, true) => theme.get("tab.active"),
            (true, false) => theme.get("tab.unfocusedActive"),
            _ => theme.get("tab.inactive"),
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
    TabBarHit { tabs: hits, arrows }
}
