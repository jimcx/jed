//! Tab bar: one tab per open editor; `●` marks unsaved changes, `×` closes; a file deleted on
//! disk is struck through, with "(deleted)" after its name (VS Code). Tabs wider than the bar
//! scroll: `‹` / `›` at the bar's ends (dimmed when there's nothing more that way) and the wheel
//! move it a tab at a time; it jumps back to the active tab only when the active tab changes.
//! While a tab is dragged, `drop` marks the gap it would land in (`tab.dropMarker`). With two or
//! more groups, each bar's top-right corner shows the group's number (`tab.groupNumber`), which
//! takes the bar's last cells: the `›` arrow then sits one cell left of it. A shell tab's last
//! glyph (`●` running / `○` idle) is drawn in its state colour.

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

/// One tab's label: its text, and how it is marked.
struct Label {
    name: String,
    dirty: bool,
    deleted: bool,
    /// A shell tab's state glyph (the name's last character) and the theme key for its colour.
    mark: Option<&'static str>,
}

/// `active_group`: the focused group's active tab is bright; other groups' active tabs are dimmer.
/// `drop`: the gap a dragged tab would land in, drawn as one marker cell.
/// `number`: the group's 1-based place in reading order, or `None` with a single group.
#[allow(clippy::too_many_arguments)]
pub fn render(
    f: &mut Frame,
    area: Rect,
    group: &mut Group,
    buffers: &Buffers,
    theme: &Theme,
    active_group: bool,
    drop: Option<usize>,
    number: Option<u8>,
) -> TabBarHit {
    let out = f.buffer_mut();
    out.set_style(area, theme.get("tabBar"));
    if area.width == 0 {
        return TabBarHit::default();
    }

    // The number takes the bar's last cells; the tabs, arrows and drop marker stay in `bar`,
    // left of it. Clicking those cells is a click on the bar (`Hit::TabBar`), which focuses it.
    let label = number.map(|n| n.to_string());
    let label_w = label.as_ref().map_or(0, |l| l.width() as u16).min(area.width);
    let bar = Rect {
        width: area.width - label_w,
        ..area
    };
    if let Some(label) = &label {
        out.set_stringn(
            area.x + bar.width,
            area.y,
            label,
            label_w as usize,
            theme.get("tab.groupNumber"),
        );
    }
    if bar.width == 0 {
        return TabBarHit::default();
    }

    // Label and width for every tab: " name ● " / " name × ".
    let labels: Vec<Label> = group
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
        .collect();
    if labels.is_empty() {
        return TabBarHit::default();
    }
    let width = |name: &str| name.width() as u16 + 4;
    let widths: Vec<u32> = labels.iter().map(|l| width(&l.name) as u32).collect();
    let total: u32 = widths.iter().sum();

    // Tabs wider than the bar: `‹` / `›` at its ends scroll it, the tabs between them.
    let overflow = total > bar.width as u32 && bar.width >= 3;
    let (left, right) = if overflow {
        (bar.x + 1, bar.x + bar.width - 1)
    } else {
        (bar.x, bar.x + bar.width)
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
    if group.revealed != Some((active, bar.width)) {
        group.revealed = Some((active, bar.width));
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
        out.set_string(bar.x, area.y, "‹", arrow(group.scroll > 0));
        out.set_string(right, area.y, "›", arrow(group.scroll < max_scroll));
        arrows = Some(TabArrows {
            back: bar.x,
            forward: right,
        });
    }

    let mut hits = Vec::new();
    let mut x = left;
    for (i, label) in labels.iter().enumerate().skip(group.scroll) {
        if x >= right {
            break;
        }
        let Label {
            name,
            dirty,
            deleted,
            mark,
        } = label;
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
        if let Some(key) = mark
            && let Some(glyph) = name.chars().last()
        {
            // The state glyph is the label's last cell, in its own colour (the tab's otherwise).
            let glyph_x = x + name.width() as u16;
            if glyph_x < x + w && glyph_x < right {
                out.set_stringn(glyph_x, area.y, glyph.to_string(), 1, style.patch(theme.get(key)));
            }
        }
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
                height: 1,
            },
            theme.get("tab.dropMarker"),
        );
    }
    TabBarHit { tabs: hits, arrows }
}
