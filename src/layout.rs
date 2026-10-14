//! Screen regions, the tree of editor-group splits, and mapping mouse positions to them.

use ratatui::layout::{Position, Rect};

pub const ACTIVITY_WIDTH: u16 = 3;
pub const MIN_SIDE_WIDTH: u16 = 12;
/// Room always left for the editor when resizing the side panel.
pub const MIN_EDITOR_WIDTH: u16 = 20;
/// Smallest editor group a split or a divider drag may produce (tab bar included).
pub const MIN_GROUP_WIDTH: u16 = 12;
pub const MIN_GROUP_HEIGHT: u16 = 4;

pub type GroupId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidePanel {
    Explorer,
    Search,
    SourceControl,
}

impl SidePanel {
    pub const ALL: [SidePanel; 3] = [SidePanel::Explorer, SidePanel::Search, SidePanel::SourceControl];

    pub fn title(self) -> &'static str {
        match self {
            SidePanel::Explorer => "EXPLORER",
            SidePanel::Search => "SEARCH",
            SidePanel::SourceControl => "SOURCE CONTROL",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Regions {
    pub activity: Rect,
    /// Includes the border column on its right edge.
    pub side: Option<Rect>,
    /// All editor groups, with their tab bars.
    pub editor: Rect,
    pub status: Rect,
}

pub fn compute(area: Rect, side_visible: bool, side_width: u16) -> Regions {
    let body_h = area.height.saturating_sub(1);
    let status = Rect {
        y: area.y + body_h,
        height: 1,
        ..area
    };
    let activity = Rect {
        width: ACTIVITY_WIDTH.min(area.width),
        height: body_h,
        ..area
    };
    let mut x = activity.x + activity.width;
    let side = side_visible.then(|| {
        let max = area
            .width
            .saturating_sub(ACTIVITY_WIDTH + MIN_EDITOR_WIDTH)
            .max(MIN_SIDE_WIDTH);
        let w = side_width
            .clamp(MIN_SIDE_WIDTH, max)
            .min(area.width.saturating_sub(x - area.x));
        let r = Rect {
            x,
            y: area.y,
            width: w,
            height: body_h,
        };
        x += w;
        r
    });
    let editor = Rect {
        x,
        y: area.y,
        width: (area.x + area.width).saturating_sub(x),
        height: body_h,
    };
    Regions {
        activity,
        side,
        editor,
        status,
    }
}

/// Which way a split lays out its two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Side by side, with a vertical divider.
    Right,
    /// Stacked, with a horizontal divider.
    Down,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(GroupId),
    Split {
        dir: SplitDir,
        /// Share of the space (minus the divider) given to `a`.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    fn first_leaf(&self) -> GroupId {
        match self {
            Node::Leaf(g) => *g,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }
}

/// A divider between the two halves of a split, as drawn in the last layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    pub dir: SplitDir,
    /// The whole split's area, for turning a mouse position into a ratio.
    pub parent: Rect,
    /// Path from the root to the split: false = first half, true = second.
    path: Vec<bool>,
}

impl Divider {
    /// The ratio that puts this divider at the mouse position.
    pub fn ratio_at(&self, x: u16, y: u16) -> f32 {
        let (pos, start, len) = match self.dir {
            SplitDir::Right => (x, self.parent.x, self.parent.width),
            SplitDir::Down => (y, self.parent.y, self.parent.height),
        };
        let avail = len.saturating_sub(1).max(1) as f32;
        (pos.saturating_sub(start) as f32 / avail).clamp(0.0, 1.0)
    }
}

/// The layout of editor groups: a binary tree of splits with groups at the leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitTree {
    root: Node,
}

impl SplitTree {
    pub fn new(group: GroupId) -> Self {
        Self {
            root: Node::Leaf(group),
        }
    }

    /// Groups in reading order (left to right, top to bottom); Cmd+1/2/3 count in this order.
    pub fn groups(&self) -> Vec<GroupId> {
        fn walk(n: &Node, out: &mut Vec<GroupId>) {
            match n {
                Node::Leaf(g) => out.push(*g),
                Node::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Splits `target` in two; `new` goes right of or below it. False if `target` isn't in the tree.
    pub fn split(&mut self, target: GroupId, dir: SplitDir, new: GroupId) -> bool {
        fn find(n: &mut Node, target: GroupId) -> Option<&mut Node> {
            match n {
                Node::Leaf(g) if *g == target => Some(n),
                Node::Leaf(_) => None,
                Node::Split { a, b, .. } => find(a, target).or_else(|| find(b, target)),
            }
        }
        let Some(leaf) = find(&mut self.root, target) else {
            return false;
        };
        *leaf = Node::Split {
            dir,
            ratio: 0.5,
            a: Box::new(Node::Leaf(target)),
            b: Box::new(Node::Leaf(new)),
        };
        true
    }

    /// Removes a group; its sibling takes the space. Returns the group that should get focus,
    /// or `None` if `group` is the last one (or not in the tree).
    pub fn remove(&mut self, group: GroupId) -> Option<GroupId> {
        fn remove(n: &mut Node, group: GroupId) -> Option<GroupId> {
            let Node::Split { a, b, .. } = n else { return None };
            let keep = match (&**a, &**b) {
                (Node::Leaf(g), _) if *g == group => Some(std::mem::replace(&mut **b, Node::Leaf(0))),
                (_, Node::Leaf(g)) if *g == group => Some(std::mem::replace(&mut **a, Node::Leaf(0))),
                _ => None,
            };
            match keep {
                Some(sibling) => {
                    let focus = sibling.first_leaf();
                    *n = sibling;
                    Some(focus)
                }
                None => remove(a, group).or_else(|| remove(b, group)),
            }
        }
        remove(&mut self.root, group)
    }

    /// Each group's area and each divider, for `area`.
    pub fn layout(&self, area: Rect) -> (Vec<(GroupId, Rect)>, Vec<Divider>) {
        fn walk(n: &Node, area: Rect, path: &mut Vec<bool>, cells: &mut Vec<(GroupId, Rect)>, divs: &mut Vec<Divider>) {
            match n {
                Node::Leaf(g) => cells.push((*g, area)),
                Node::Split { dir, ratio, a, b } => {
                    let (ra, rd, rb) = split_rect(area, *dir, *ratio);
                    divs.push(Divider {
                        rect: rd,
                        dir: *dir,
                        parent: area,
                        path: path.clone(),
                    });
                    path.push(false);
                    walk(a, ra, path, cells, divs);
                    path.pop();
                    path.push(true);
                    walk(b, rb, path, cells, divs);
                    path.pop();
                }
            }
        }
        let (mut cells, mut divs) = (Vec::new(), Vec::new());
        walk(&self.root, area, &mut Vec::new(), &mut cells, &mut divs);
        (cells, divs)
    }

    /// Moves a divider (from `layout`). The layout keeps groups at their minimum size.
    pub fn set_ratio(&mut self, divider: &Divider, ratio: f32) {
        let mut n = &mut self.root;
        for &second in &divider.path {
            let Node::Split { a, b, .. } = n else { return };
            n = if second { b } else { a };
        }
        if let Node::Split { ratio: r, .. } = n {
            *r = ratio.clamp(0.0, 1.0);
        }
    }
}

/// Splits `area` into (first, divider, second). Each half keeps its minimum size when it fits.
fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect, Rect) {
    let (len, min) = match dir {
        SplitDir::Right => (area.width, MIN_GROUP_WIDTH),
        SplitDir::Down => (area.height, MIN_GROUP_HEIGHT),
    };
    let avail = len.saturating_sub(1);
    let min = min.min(avail / 2);
    let first = ((avail as f32 * ratio).round() as u16).clamp(min, avail - min);
    let second = avail - first;
    match dir {
        SplitDir::Right => (
            Rect { width: first, ..area },
            Rect {
                x: area.x + first,
                width: len.min(1),
                ..area
            },
            Rect {
                x: area.x + first + 1,
                width: second,
                ..area
            },
        ),
        SplitDir::Down => (
            Rect { height: first, ..area },
            Rect {
                y: area.y + first,
                height: len.min(1),
                ..area
            },
            Rect {
                y: area.y + first + 1,
                height: second,
                ..area
            },
        ),
    }
}

/// Where each clickable thing was drawn in the last frame.
#[derive(Debug, Clone, Default)]
pub struct HitMap {
    pub regions: Regions,
    /// Activity bar icons: row and panel.
    pub icons: Vec<(u16, SidePanel)>,
    /// Explorer rows area; row `i` on screen is entry `scroll + i`.
    pub explorer_rows: Rect,
    pub groups: Vec<GroupHit>,
    pub dividers: Vec<Divider>,
    /// The palette, when open; it takes every mouse event.
    pub palette: Option<PaletteHit>,
}

/// The palette box, and its list area: row `i` on screen is row `scroll + i`.
#[derive(Debug, Clone, Copy)]
pub struct PaletteHit {
    pub area: Rect,
    pub rows: Rect,
}

/// One editor group as drawn: tab bar, editor, and the text area inside it.
#[derive(Debug, Clone, Default)]
pub struct GroupHit {
    pub id: GroupId,
    pub tab_bar: Rect,
    pub editor: Rect,
    /// Text area (gutter excluded).
    pub text: Rect,
    pub tabs: Vec<TabHit>,
}

/// A tab's x range and the x of its close/dirty marker.
#[derive(Debug, Clone, Copy)]
pub struct TabHit {
    pub index: usize,
    pub x0: u16,
    pub x1: u16,
    pub close_x: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Icon(SidePanel),
    SideBorder,
    /// Explorer row on screen (add the scroll offset for the entry).
    ExplorerRow(u16),
    SidePanel,
    /// Index into `HitMap::dividers`.
    Divider(usize),
    Tab(GroupId, usize),
    TabClose(GroupId, usize),
    TabBar(GroupId),
    Editor(GroupId),
    Status,
    Nothing,
}

impl HitMap {
    pub fn hit(&self, x: u16, y: u16) -> Hit {
        let p = Position { x, y };
        let r = &self.regions;
        if r.activity.contains(p) {
            return self
                .icons
                .iter()
                .find(|(iy, _)| *iy == y)
                .map_or(Hit::Nothing, |(_, panel)| Hit::Icon(*panel));
        }
        if let Some(side) = r.side
            && side.contains(p)
        {
            if x == side.x + side.width - 1 {
                return Hit::SideBorder;
            }
            if self.explorer_rows.contains(p) {
                return Hit::ExplorerRow(y - self.explorer_rows.y);
            }
            return Hit::SidePanel;
        }
        if let Some(i) = self.dividers.iter().position(|d| d.rect.contains(p)) {
            return Hit::Divider(i);
        }
        for g in &self.groups {
            if g.tab_bar.contains(p) {
                return match g.tabs.iter().find(|t| x >= t.x0 && x < t.x1) {
                    Some(t) if x == t.close_x => Hit::TabClose(g.id, t.index),
                    Some(t) => Hit::Tab(g.id, t.index),
                    None => Hit::TabBar(g.id),
                };
            }
            if g.editor.contains(p) {
                return Hit::Editor(g.id);
            }
        }
        if r.status.contains(p) {
            return Hit::Status;
        }
        Hit::Nothing
    }

    pub fn group(&self, id: GroupId) -> Option<&GroupHit> {
        self.groups.iter().find(|g| g.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_tile_the_screen() {
        let r = compute(Rect::new(0, 0, 100, 30), true, 25);
        assert_eq!(r.activity, Rect::new(0, 0, 3, 29));
        assert_eq!(r.side, Some(Rect::new(3, 0, 25, 29)));
        assert_eq!(r.editor, Rect::new(28, 0, 72, 29));
        assert_eq!(r.status, Rect::new(0, 29, 100, 1));

        let r = compute(Rect::new(0, 0, 100, 30), false, 25);
        assert_eq!(r.side, None);
        assert_eq!(r.editor.x, 3);
    }

    #[test]
    fn side_width_is_clamped() {
        let r = compute(Rect::new(0, 0, 60, 10), true, 200);
        assert_eq!(r.side.unwrap().width, 60 - ACTIVITY_WIDTH - MIN_EDITOR_WIDTH);
        let r = compute(Rect::new(0, 0, 60, 10), true, 1);
        assert_eq!(r.side.unwrap().width, MIN_SIDE_WIDTH);
    }

    #[test]
    fn split_and_layout() {
        let mut t = SplitTree::new(0);
        assert!(t.split(0, SplitDir::Right, 1));
        assert!(t.split(1, SplitDir::Down, 2));
        assert!(!t.split(9, SplitDir::Down, 3));
        assert_eq!(t.groups(), vec![0, 1, 2]);

        let (cells, divs) = t.layout(Rect::new(0, 0, 81, 21));
        assert_eq!(
            cells,
            vec![
                (0, Rect::new(0, 0, 40, 21)),
                (1, Rect::new(41, 0, 40, 10)),
                (2, Rect::new(41, 11, 40, 10))
            ]
        );
        assert_eq!(divs[0].rect, Rect::new(40, 0, 1, 21));
        assert_eq!(divs[1].rect, Rect::new(41, 10, 40, 1));
    }

    #[test]
    fn remove_gives_space_to_sibling() {
        let mut t = SplitTree::new(0);
        t.split(0, SplitDir::Right, 1);
        t.split(1, SplitDir::Down, 2);
        assert_eq!(t.remove(1), Some(2));
        assert_eq!(t.groups(), vec![0, 2]);
        assert_eq!(t.remove(0), Some(2));
        assert_eq!(t.groups(), vec![2]);
        assert_eq!(t.remove(2), None, "the last group stays");
        assert_eq!(t.layout(Rect::new(0, 0, 10, 10)).0, vec![(2, Rect::new(0, 0, 10, 10))]);
    }

    #[test]
    fn resize_respects_minimums() {
        let mut t = SplitTree::new(0);
        t.split(0, SplitDir::Right, 1);
        let area = Rect::new(0, 0, 101, 20);
        let div = t.layout(area).1[0].clone();
        t.set_ratio(&div, div.ratio_at(30, 5));
        assert_eq!(t.layout(area).0[0].1.width, 30);
        t.set_ratio(&div, div.ratio_at(1, 5));
        assert_eq!(t.layout(area).0[0].1.width, MIN_GROUP_WIDTH);
        t.set_ratio(&div, div.ratio_at(100, 5));
        assert_eq!(t.layout(area).0[1].1.width, MIN_GROUP_WIDTH);
    }

    #[test]
    fn hits() {
        let regions = compute(Rect::new(0, 0, 100, 30), true, 25);
        let mut t = SplitTree::new(7);
        t.split(7, SplitDir::Right, 8);
        let (cells, dividers) = t.layout(regions.editor);
        let groups = cells
            .iter()
            .map(|(id, r)| GroupHit {
                id: *id,
                tab_bar: Rect { height: 1, ..*r },
                editor: Rect {
                    y: r.y + 1,
                    height: r.height - 1,
                    ..*r
                },
                text: Rect::default(),
                tabs: vec![TabHit {
                    index: 0,
                    x0: r.x,
                    x1: r.x + 10,
                    close_x: r.x + 8,
                }],
            })
            .collect();
        let map = HitMap {
            regions,
            icons: vec![(1, SidePanel::Explorer)],
            explorer_rows: Rect::new(3, 2, 24, 27),
            groups,
            dividers,
            palette: None,
        };
        assert_eq!(map.hit(1, 1), Hit::Icon(SidePanel::Explorer));
        assert_eq!(map.hit(27, 5), Hit::SideBorder);
        assert_eq!(map.hit(5, 4), Hit::ExplorerRow(2));
        assert_eq!(map.hit(5, 0), Hit::SidePanel);
        assert_eq!(map.hit(30, 0), Hit::Tab(7, 0));
        assert_eq!(map.hit(36, 0), Hit::TabClose(7, 0));
        assert_eq!(map.hit(50, 0), Hit::TabBar(7));
        assert_eq!(map.hit(50, 10), Hit::Editor(7));
        assert_eq!(map.hit(64, 10), Hit::Divider(0));
        assert_eq!(map.hit(66, 0), Hit::Tab(8, 0));
        assert_eq!(map.hit(80, 10), Hit::Editor(8));
        assert_eq!(map.hit(60, 29), Hit::Status);
    }
}
