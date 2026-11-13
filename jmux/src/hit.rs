//! What the last frame drew where, for the mouse: written by `ui::render`, read by `App::mouse`.

use jtui::split::{Divider, GroupId};
use ratatui::layout::Rect;

/// A tab's x range in its bar and the x of its `×`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabHit {
    pub index: usize,
    pub x0: u16,
    pub x1: u16,
    pub close_x: u16,
}

/// The x of a bar's `‹` (scrolls towards the first tab) and `›` (towards the last).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabArrows {
    pub back: u16,
    pub forward: u16,
}

/// One group's bar and terminal area.
#[derive(Debug, Clone, Default)]
pub struct GroupHit {
    pub id: GroupId,
    pub bar: Rect,
    pub term: Rect,
    pub tabs: Vec<TabHit>,
    pub arrows: Option<TabArrows>,
}

#[derive(Debug, Clone, Default)]
pub struct HitMap {
    pub groups: Vec<GroupHit>,
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
    /// A group's bar beside its tabs.
    Bar {
        group: GroupId,
    },
    Terminal {
        group: GroupId,
    },
    /// A divider: an index into `HitMap::dividers`.
    Divider(usize),
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

impl HitMap {
    pub fn at(&self, x: u16, y: u16) -> Option<Hit> {
        if let Some(i) = self.dividers.iter().position(|d| contains(d.rect, x, y)) {
            return Some(Hit::Divider(i));
        }
        for g in &self.groups {
            let group = g.id;
            if contains(g.term, x, y) {
                return Some(Hit::Terminal { group });
            }
            if !contains(g.bar, x, y) {
                continue;
            }
            if let Some(a) = g.arrows {
                if x == a.back {
                    return Some(Hit::Arrow { group, forward: false });
                }
                if x == a.forward {
                    return Some(Hit::Arrow { group, forward: true });
                }
            }
            return Some(match g.tabs.iter().find(|t| x >= t.x0 && x < t.x1) {
                Some(t) if x == t.close_x => Hit::Close { group, index: t.index },
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
                tabs: vec![
                    TabHit {
                        index: 0,
                        x0: 1,
                        x1: 9,
                        close_x: 7,
                    },
                    TabHit {
                        index: 1,
                        x0: 9,
                        x1: 17,
                        close_x: 15,
                    },
                ],
                arrows: Some(TabArrows { back: 0, forward: 29 }),
            }],
            dividers: Vec::new(),
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
        let map = HitMap { groups, dividers };
        assert_eq!(map.at(d.x, 0), Some(Hit::Divider(0)));
        assert_eq!(map.at(d.x, 3), Some(Hit::Divider(0)));
        assert_eq!(map.at(d.x - 1, 3), Some(Hit::Terminal { group: 1 }));
        assert_eq!(map.at(d.x + 1, 3), Some(Hit::Terminal { group: 2 }));
    }
}
