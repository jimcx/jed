//! What the last frame drew where, for the mouse: written by `ui::render`, read by `App::mouse`.

use jtui::scrollbar_draw::ScrollBarHit;
use jtui::split::{Divider, GroupId};
use ratatui::layout::Rect;
use std::ops::Range;

/// A tab's x range in its bar and the x range of its `×`'s target (half-open, inside the tab's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabHit {
    pub index: usize,
    pub x0: u16,
    pub x1: u16,
    pub close_x0: u16,
    pub close_x1: u16,
}

/// The x ranges (half-open) of a bar's `‹` (scrolls towards the first tab) and `›` (towards the
/// last).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabArrows {
    pub back: Range<u16>,
    pub forward: Range<u16>,
}

/// Where `tabbar::render` put a bar's tabs and its arrows (when it overflows).
#[derive(Debug, Clone, Default)]
pub struct BarHit {
    pub tabs: Vec<TabHit>,
    pub arrows: Option<TabArrows>,
}

/// One group's bar and terminal area.
#[derive(Debug, Clone, Default)]
pub struct GroupHit {
    pub id: GroupId,
    pub bar: Rect,
    /// Everything below the bar: the shell's `text` and the scroll bar's column.
    pub term: Rect,
    /// Where the shell's cells are (`term` less the scroll bar's column).
    pub text: Rect,
    /// The scroll bar: its track (empty when the area is too narrow) and thumb.
    pub scroll: ScrollBarHit,
    pub tabs: Vec<TabHit>,
    pub arrows: Option<TabArrows>,
}

/// The tab menu as drawn (touch layout, or opened by `menu.toggle`).
#[derive(Debug, Clone, Default)]
pub struct MenuHit {
    pub area: Rect,
    /// What each drawn row (top to bottom, from `area.y`) is, as `menu::rows` gave it.
    pub rows: Vec<crate::menu::Row>,
}

/// The side bar as drawn.
#[derive(Debug, Clone, Default)]
pub struct SideHit {
    /// The whole bar (its border column is `border`).
    pub area: Rect,
    /// The column of the line between the bar and the groups, which a drag moves.
    pub border: Rect,
    pub filter: Rect,
    /// The filter's sideways scroll as drawn (to map a click to a character).
    pub filter_scroll: usize,
    /// The rows' area: row `i` of it is the visible row `first + i`.
    pub list: Rect,
    pub first: usize,
    /// How many rows are drawn.
    pub shown: usize,
    pub scroll: ScrollBarHit,
}

#[derive(Debug, Clone, Default)]
pub struct HitMap {
    pub side: Option<SideHit>,
    pub groups: Vec<GroupHit>,
    /// The status bar's `☰` button (touch layout).
    pub menu_button: Option<Rect>,
    /// The open tab menu.
    pub menu: Option<MenuHit>,
    /// The dividers between groups, as `Groups::layout` gave them.
    pub dividers: Vec<Divider>,
}

/// What a screen cell is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tab {
        group: GroupId,
        index: usize,
    },
    Close {
        group: GroupId,
        index: usize,
    },
    Arrow {
        group: GroupId,
        forward: bool,
    },
    /// The status bar's `☰` button.
    Menu,
    /// An item of the open tab menu: an index into its items.
    MenuItem(usize),
    /// A cell of the open menu that is no item (the separator).
    MenuDead,
    /// Outside the open menu (made by the app, not `HitMap::at`).
    MenuOutside,
    /// A group's bar beside its tabs.
    Bar {
        group: GroupId,
    },
    Terminal {
        group: GroupId,
    },
    /// A terminal's scroll bar column.
    ScrollBar {
        group: GroupId,
    },
    /// A divider: an index into `HitMap::dividers`.
    Divider(usize),
    /// The side bar's border line.
    SideBorder,
    SideFilter,
    /// A drawn row: the index among the visible rows.
    SideRow(usize),
    SideScrollBar,
    /// The rest of the bar.
    Side,
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

impl HitMap {
    pub fn at(&self, x: u16, y: u16) -> Option<Hit> {
        if let Some(m) = &self.menu
            && contains(m.area, x, y)
        {
            return Some(match m.rows.get(usize::from(y - m.area.y)) {
                Some(crate::menu::Row::Pad(i) | crate::menu::Row::Text(i)) => Hit::MenuItem(*i),
                _ => Hit::MenuDead,
            });
        }
        if self.menu_button.is_some_and(|r| contains(r, x, y)) {
            return Some(Hit::Menu);
        }
        if let Some(s) = &self.side {
            if contains(s.border, x, y) {
                return Some(Hit::SideBorder);
            }
            if contains(s.filter, x, y) {
                return Some(Hit::SideFilter);
            }
            if contains(s.scroll.track, x, y) {
                return Some(Hit::SideScrollBar);
            }
            if contains(s.list, x, y) {
                let i = usize::from(y - s.list.y);
                return Some(if i < s.shown {
                    Hit::SideRow(s.first + i)
                } else {
                    Hit::Side
                });
            }
            if contains(s.area, x, y) {
                return Some(Hit::Side);
            }
        }
        if let Some(i) = self.dividers.iter().position(|d| contains(d.rect, x, y)) {
            return Some(Hit::Divider(i));
        }
        for g in &self.groups {
            let group = g.id;
            if contains(g.scroll.track, x, y) {
                return Some(Hit::ScrollBar { group });
            }
            if contains(g.term, x, y) {
                return Some(Hit::Terminal { group });
            }
            if !contains(g.bar, x, y) {
                continue;
            }
            if let Some(a) = &g.arrows {
                if a.back.contains(&x) {
                    return Some(Hit::Arrow { group, forward: false });
                }
                if a.forward.contains(&x) {
                    return Some(Hit::Arrow { group, forward: true });
                }
            }
            return Some(match g.tabs.iter().find(|t| x >= t.x0 && x < t.x1) {
                Some(t) if (t.close_x0..t.close_x1).contains(&x) => Hit::Close { group, index: t.index },
                Some(t) => Hit::Tab { group, index: t.index },
                None => Hit::Bar { group },
            });
        }
        None
    }

    pub fn group(&self, id: GroupId) -> Option<&GroupHit> {
        self.groups.iter().find(|g| g.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_resolve_to_tabs_close_buttons_arrows_and_terminals() {
        let map = HitMap {
            groups: vec![GroupHit {
                id: 7,
                bar: Rect::new(0, 0, 30, 1),
                term: Rect::new(0, 1, 30, 5),
                text: Rect::new(0, 1, 29, 5),
                scroll: ScrollBarHit {
                    track: Rect::new(29, 1, 1, 5),
                    thumb: None,
                },
                tabs: vec![
                    TabHit {
                        index: 0,
                        x0: 1,
                        x1: 9,
                        close_x0: 7,
                        close_x1: 8,
                    },
                    TabHit {
                        index: 1,
                        x0: 9,
                        x1: 17,
                        close_x0: 15,
                        close_x1: 16,
                    },
                ],
                arrows: Some(TabArrows {
                    back: 0..1,
                    forward: 29..30,
                }),
            }],
            dividers: Vec::new(),
            side: None,
            ..HitMap::default()
        };
        assert_eq!(map.at(3, 0), Some(Hit::Tab { group: 7, index: 0 }));
        assert_eq!(map.at(15, 0), Some(Hit::Close { group: 7, index: 1 }));
        assert_eq!(
            map.at(0, 0),
            Some(Hit::Arrow {
                group: 7,
                forward: false
            })
        );
        assert_eq!(
            map.at(29, 0),
            Some(Hit::Arrow {
                group: 7,
                forward: true
            })
        );
        assert_eq!(map.at(20, 0), Some(Hit::Bar { group: 7 }));
        assert_eq!(map.at(4, 3), Some(Hit::Terminal { group: 7 }));
        assert_eq!(map.at(29, 3), Some(Hit::ScrollBar { group: 7 }));
        assert_eq!(map.at(4, 6), None);
    }

    #[test]
    fn a_divider_wins_over_the_groups_beside_it() {
        let mut tree = jtui::split::SplitTree::new(1);
        tree.split(1, jtui::split::SplitDir::Right, 2);
        let (cells, dividers) = tree.layout(Rect::new(0, 0, 21, 6));
        let d = dividers[0].rect;
        let groups = cells
            .iter()
            .map(|(id, r)| GroupHit {
                id: *id,
                bar: Rect { height: 1, ..*r },
                term: Rect::new(r.x, r.y + 1, r.width, r.height - 1),
                ..GroupHit::default()
            })
            .collect();
        let map = HitMap {
            groups,
            dividers,
            side: None,
            ..HitMap::default()
        };
        assert_eq!(map.at(d.x, 0), Some(Hit::Divider(0)));
        assert_eq!(map.at(d.x, 3), Some(Hit::Divider(0)));
        assert_eq!(map.at(d.x - 1, 3), Some(Hit::Terminal { group: 1 }));
        assert_eq!(map.at(d.x + 1, 3), Some(Hit::Terminal { group: 2 }));
    }

    #[test]
    fn the_side_bar_resolves_its_parts_before_the_groups() {
        let map = HitMap {
            side: Some(SideHit {
                area: Rect::new(0, 0, 10, 8),
                border: Rect::new(10, 0, 1, 8),
                filter: Rect::new(1, 0, 8, 1),
                filter_scroll: 0,
                list: Rect::new(0, 1, 9, 7),
                first: 4,
                shown: 2,
                scroll: ScrollBarHit {
                    track: Rect::new(9, 1, 1, 7),
                    thumb: None,
                },
            }),
            ..HitMap::default()
        };
        assert_eq!(map.at(10, 3), Some(Hit::SideBorder));
        assert_eq!(map.at(3, 0), Some(Hit::SideFilter));
        assert_eq!(map.at(3, 1), Some(Hit::SideRow(4)));
        assert_eq!(map.at(3, 2), Some(Hit::SideRow(5)));
        assert_eq!(map.at(3, 3), Some(Hit::Side));
        assert_eq!(map.at(9, 3), Some(Hit::SideScrollBar));
        assert_eq!(map.at(0, 0), Some(Hit::Side));
        assert_eq!(map.at(11, 3), None);
    }
}
