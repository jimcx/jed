//! The split tree: a binary tree of splits with groups at the leaves, laid out into areas and dividers.

use ratatui::layout::{Position, Rect};

/// A leaf's id: the app's own id for what the leaf shows.
pub type GroupId = u64;

/// Smallest group a split or a divider drag may produce (a tab bar included).
pub const MIN_GROUP_WIDTH: u16 = 12;
pub const MIN_GROUP_HEIGHT: u16 = 4;

/// Which way a split lays out its two halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDir {
    /// Side by side, with a vertical divider.
    Right,
    /// Stacked, with a horizontal divider.
    Down,
}

/// An editor's edge, for a tab dropped there: the group splits on that side and the new group
/// takes the space there. A drop on an edge of `Right` or `Down` puts the new group second in
/// the tree; `Left` / `Up` put it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropSide {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Set by the app after `layout`: whether it belongs to the app's secondary tree rather than the main one.
    pub terminal: bool,
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

/// The layout of editor groups: a binary tree of splits with groups at the leaves. Serialized
/// as is (group ids at the leaves) in the session file (`session`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
        self.split_with(target, dir, new, false)
    }

    /// Splits `target` in two; with `new_first` the new group takes the first half (left of or
    /// above `target`). False if `target` isn't in the tree.
    pub fn split_with(&mut self, target: GroupId, dir: SplitDir, new: GroupId, new_first: bool) -> bool {
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
        let (a, b) = if new_first { (new, target) } else { (target, new) };
        *leaf = Node::Split {
            dir,
            ratio: 0.5,
            a: Box::new(Node::Leaf(a)),
            b: Box::new(Node::Leaf(b)),
        };
        true
    }

    /// The first (outermost, leftmost) side-by-side split: the first group of its left half and
    /// every group of its right half, in reading order. `None` when no split is `Right`.
    pub fn first_right_split(&self) -> Option<(GroupId, Vec<GroupId>)> {
        fn leaves(n: &Node, out: &mut Vec<GroupId>) {
            match n {
                Node::Leaf(g) => out.push(*g),
                Node::Split { a, b, .. } => {
                    leaves(a, out);
                    leaves(b, out);
                }
            }
        }
        fn find(n: &Node) -> Option<(GroupId, Vec<GroupId>)> {
            let Node::Split { dir, a, b, .. } = n else { return None };
            if *dir == SplitDir::Right {
                let mut right = Vec::new();
                leaves(b, &mut right);
                return Some((a.first_leaf(), right));
            }
            find(a).or_else(|| find(b))
        }
        find(&self.root)
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
                        terminal: false,
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

    /// Repairs a tree read from a file: ratios outside 0..=1 (or NaN) become 0.5. The caller
    /// checks the leaves (`groups`) against its groups.
    pub fn sanitize(&mut self) {
        fn walk(n: &mut Node) {
            if let Node::Split { ratio, a, b, .. } = n {
                if !(0.0..=1.0).contains(ratio) {
                    *ratio = 0.5;
                }
                walk(a);
                walk(b);
            }
        }
        walk(&mut self.root);
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
                x: area.x + first + len.min(1),
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
                y: area.y + first + len.min(1),
                height: second,
                ..area
            },
        ),
    }
}

/// The gap among a bar's drawn tabs, given as (index, x0, x1), at `x`: the tabs whose midpoint is
/// left of the pointer. The index is in the group's own terms, so a scrolled bar's first drawn
/// tab's index is counted from.
pub fn gap_at(tabs: impl IntoIterator<Item = (usize, u16, u16)>, x: u16) -> usize {
    let mut first = None;
    let mut left = 0;
    for (index, x0, x1) in tabs {
        first.get_or_insert(index);
        left += usize::from(x0 + (x1 - x0) / 2 < x);
    }
    first.unwrap_or(0) + left
}

/// Which edge of `rect` the point (`x`, `y`) is near enough to snap to, or `None` for its
/// centre (or outside it). Each edge's band is a quarter of that dimension, and the nearest
/// edge within its band wins (ties in the order Left, Right, Up, Down). An edge whose dimension
/// can't hold a split of two minimum groups is no edge at all.
pub fn snap_side(rect: Rect, x: u16, y: u16) -> Option<DropSide> {
    if !rect.contains(Position { x, y }) {
        return None;
    }
    let horizontal = rect.width > 2 * MIN_GROUP_WIDTH;
    let vertical = rect.height > 2 * MIN_GROUP_HEIGHT;
    let (band_w, band_h) = (rect.width / 4, rect.height / 4);
    let (right, bottom) = (rect.x + rect.width - 1, rect.y + rect.height - 1);
    // In tie order; `min` keeps the first of equal distances.
    let candidates = [
        (DropSide::Left, x - rect.x, horizontal, band_w),
        (DropSide::Right, right - x, horizontal, band_w),
        (DropSide::Up, y - rect.y, vertical, band_h),
        (DropSide::Down, bottom - y, vertical, band_h),
    ];
    candidates
        .into_iter()
        .filter(|&(_, distance, fits, band)| fits && distance <= band)
        .min_by_key(|&(_, distance, ..)| distance)
        .map(|(side, ..)| side)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session file stores the tree as is: a rename of a field, variant or direction breaks old sessions.
    #[test]
    fn serialized_form_is_the_session_file_format() {
        let mut t = SplitTree::new(0);
        assert!(t.split(0, SplitDir::Right, 1));
        assert!(t.split(1, SplitDir::Down, 2));
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"root":{"split":{"dir":"right","ratio":0.5,"a":{"leaf":0},"b":{"split":{"dir":"down","ratio":0.5,"a":{"leaf":1},"b":{"leaf":2}}}}}}"#
        );
        assert_eq!(serde_json::from_str::<SplitTree>(&json).unwrap(), t);
    }

    #[test]
    fn first_right_split_names_the_left_group_and_the_right_half() {
        let mut t = SplitTree::new(0);
        assert_eq!(t.first_right_split(), None);
        assert!(t.split(0, SplitDir::Down, 1));
        assert_eq!(t.first_right_split(), None);
        assert!(t.split(0, SplitDir::Right, 2));
        assert!(t.split(2, SplitDir::Down, 3));
        assert_eq!(t.first_right_split(), Some((0, vec![2, 3])));
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
    fn split_with_puts_the_new_group_first_for_left_and_up() {
        let mut t = SplitTree::new(0);
        assert!(t.split_with(0, SplitDir::Right, 1, true));
        assert_eq!(t.groups(), vec![1, 0], "the new group is on the left");
        assert!(t.split_with(0, SplitDir::Down, 2, true));
        assert_eq!(t.groups(), vec![1, 2, 0], "and above the target");
        let cells = t.layout(Rect::new(0, 0, 81, 21)).0;
        assert_eq!(
            cells,
            vec![
                (1, Rect::new(0, 0, 40, 21)),
                (2, Rect::new(41, 0, 40, 10)),
                (0, Rect::new(41, 11, 40, 10)),
            ]
        );
        assert!(!t.split_with(9, SplitDir::Right, 3, true), "no such target");
        assert_eq!(t.groups(), vec![1, 2, 0], "unchanged");
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
    fn splitting_an_empty_area_stays_inside_it() {
        for dir in [SplitDir::Right, SplitDir::Down] {
            let area = Rect::new(3, 4, 0, 0);
            let (a, d, b) = split_rect(area, dir, 0.5);
            for r in [a, d, b] {
                assert!(r.x <= area.x && r.y <= area.y, "{dir:?}: {r:?}");
            }
        }
    }

    #[test]
    fn the_gap_is_the_tabs_whose_midpoint_is_left_of_the_pointer() {
        let tabs = [(0, 0, 10), (1, 10, 20), (2, 20, 30)];
        assert_eq!(gap_at(tabs, 0), 0);
        assert_eq!(gap_at(tabs, 4), 0, "left of the first midpoint");
        assert_eq!(gap_at(tabs, 5), 0, "on the midpoint is not past it");
        assert_eq!(gap_at(tabs, 6), 1);
        assert_eq!(gap_at(tabs, 15), 1);
        assert_eq!(gap_at(tabs, 16), 2);
        assert_eq!(gap_at(tabs, 25), 2);
        assert_eq!(gap_at(tabs, 26), 3, "past the last midpoint: the end");
        assert_eq!(gap_at(tabs, 60), 3, "past the bar is still the end");
    }

    #[test]
    fn a_scrolled_bar_counts_from_its_first_drawn_tab() {
        let tabs = [(4, 0, 10), (5, 10, 20)];
        assert_eq!(gap_at(tabs, 0), 4);
        assert_eq!(gap_at(tabs, 6), 5);
        assert_eq!(gap_at(tabs, 26), 6);
        assert_eq!(gap_at([], 8), 0, "an empty bar: the only gap");
    }

    /// A 40×20 editor: each band is the nearer quarter (10 columns, 5 rows).
    #[test]
    fn the_nearer_quarter_is_a_side_and_the_middle_is_the_centre() {
        let r = Rect::new(0, 0, 40, 20);
        assert_eq!(snap_side(r, 0, 10), Some(DropSide::Left));
        assert_eq!(snap_side(r, 10, 10), Some(DropSide::Left), "exactly a quarter in");
        assert_eq!(snap_side(r, 11, 10), None, "past the band: the centre");
        assert_eq!(snap_side(r, 29, 10), Some(DropSide::Right));
        assert_eq!(snap_side(r, 39, 10), Some(DropSide::Right), "the last column");
        assert_eq!(snap_side(r, 28, 10), None);
        assert_eq!(snap_side(r, 20, 0), Some(DropSide::Up));
        assert_eq!(snap_side(r, 20, 5), Some(DropSide::Up), "exactly a quarter down");
        assert_eq!(snap_side(r, 20, 6), None);
        assert_eq!(snap_side(r, 20, 14), Some(DropSide::Down));
        assert_eq!(snap_side(r, 20, 19), Some(DropSide::Down), "the last row");
        assert_eq!(snap_side(r, 20, 13), None);
        assert_eq!(snap_side(r, 20, 10), None, "the centre");
        assert_eq!(snap_side(r, 40, 10), None, "outside");
        assert_eq!(snap_side(r, 20, 20), None);
    }

    #[test]
    fn the_nearest_edge_wins_with_left_right_up_down_ties() {
        let r = Rect::new(0, 0, 40, 20);
        assert_eq!(snap_side(r, 2, 2), Some(DropSide::Left), "left and up tie: left first");
        assert_eq!(
            snap_side(r, 37, 2),
            Some(DropSide::Right),
            "right and up tie: right first"
        );
        assert_eq!(
            snap_side(r, 2, 17),
            Some(DropSide::Left),
            "left and down tie: left first"
        );
        assert_eq!(
            snap_side(r, 37, 17),
            Some(DropSide::Right),
            "right and down tie: right first"
        );
        assert_eq!(snap_side(r, 20, 2), Some(DropSide::Up), "the top half");
        assert_eq!(
            snap_side(r, 5, 4),
            Some(DropSide::Up),
            "the nearer edge wins, not the first"
        );
        assert_eq!(snap_side(r, 4, 5), Some(DropSide::Left));
    }

    #[test]
    fn snap_side_works_on_a_rect_away_from_the_origin() {
        let r = Rect::new(40, 7, 40, 20);
        assert_eq!(snap_side(r, 40, 17), Some(DropSide::Left));
        assert_eq!(snap_side(r, 50, 17), Some(DropSide::Left), "exactly a quarter in");
        assert_eq!(snap_side(r, 51, 17), None);
        assert_eq!(snap_side(r, 79, 17), Some(DropSide::Right), "the last column");
        assert_eq!(snap_side(r, 60, 7), Some(DropSide::Up));
        assert_eq!(snap_side(r, 60, 26), Some(DropSide::Down), "the last row");
        assert_eq!(snap_side(r, 39, 17), None, "outside on the left");
        assert_eq!(snap_side(r, 80, 17), None, "outside on the right");
        assert_eq!(snap_side(r, 60, 6), None, "outside above");
        assert_eq!(snap_side(r, 60, 27), None, "outside below");
    }

    #[test]
    fn an_edge_that_cannot_hold_a_split_is_no_edge() {
        // 25 is the narrowest width that can hold two minimum groups (12 + a divider + 12).
        let narrow = Rect::new(0, 0, 24, 20);
        assert_eq!(snap_side(narrow, 0, 10), None, "too narrow to split sideways");
        let just = Rect::new(0, 0, 25, 20);
        assert_eq!(snap_side(just, 0, 10), Some(DropSide::Left), "25 fits a split");
        assert_eq!(snap_side(just, 24, 10), Some(DropSide::Right));

        // 9 is the shortest height that can hold two minimum groups (4 + a divider + 4).
        let short = Rect::new(0, 0, 40, 8);
        assert_eq!(snap_side(short, 20, 0), None, "too short to split lengthways");
        let just = Rect::new(0, 0, 40, 9);
        assert_eq!(snap_side(just, 20, 0), Some(DropSide::Up), "9 fits a split");
        assert_eq!(snap_side(just, 20, 8), Some(DropSide::Down));
    }
}
