//! Screen regions, the tree of editor-group splits, and mapping mouse positions to them.

use ratatui::layout::{Position, Rect};

pub const ACTIVITY_WIDTH: u16 = 3;
pub const MIN_SIDE_WIDTH: u16 = 12;
/// Room always left for the editor when resizing the side panel.
pub const MIN_EDITOR_WIDTH: u16 = 20;
/// Smallest editor group a split or a divider drag may produce (tab bar included).
pub const MIN_GROUP_WIDTH: u16 = 12;
pub const MIN_GROUP_HEIGHT: u16 = 4;
/// The bottom panel's smallest height: its top border, its tab strip and two rows.
pub const MIN_PANEL_HEIGHT: u16 = 4;
/// Room always left above the bottom panel for the editor (a tab bar and a few lines).
pub const MIN_EDITOR_HEIGHT: u16 = 5;

pub type GroupId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidePanel {
    Explorer,
    Search,
    SourceControl,
    /// Every tool jed runs (language servers, task runs, jed itself) with its state.
    Tools,
}

impl SidePanel {
    pub const ALL: [SidePanel; 4] = [
        SidePanel::Explorer,
        SidePanel::Search,
        SidePanel::SourceControl,
        SidePanel::Tools,
    ];

    pub fn title(self) -> &'static str {
        match self {
            SidePanel::Explorer => "EXPLORER",
            SidePanel::Search => "SEARCH",
            SidePanel::SourceControl => "SOURCE CONTROL",
            SidePanel::Tools => "TOOLS",
        }
    }
}

/// The bottom panel's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Problems,
    /// Places go to definition found (9g: references); its tab shows while there's a list.
    Locations,
    /// A task run's terminal (one tab per run, until closed).
    Task(crate::tasks::RunId),
}

impl PanelTab {
    /// The tabs always there (Locations only while there's a list), before the task tabs.
    pub const ALL: [PanelTab; 2] = [PanelTab::Problems, PanelTab::Locations];

    /// A fixed tab's title; a task tab shows its run's name and state instead.
    pub fn title(self) -> &'static str {
        match self {
            PanelTab::Problems => "PROBLEMS",
            PanelTab::Locations => "LOCATIONS",
            PanelTab::Task(_) => "TASK",
        }
    }
}

/// A task tab's terminal in the panel `panel` (border row included): below the border and the
/// tab strip, one column of padding on the left. The PTY gets its size.
pub fn task_area(panel: Rect) -> Rect {
    Rect {
        x: panel.x + 1,
        y: panel.y + 2,
        width: panel.width.saturating_sub(1),
        height: panel.height.saturating_sub(2),
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Regions {
    pub activity: Rect,
    /// Includes the border column on its right edge.
    pub side: Option<Rect>,
    /// All editor groups, with their tab bars.
    pub editor: Rect,
    /// The bottom panel under the editor groups (its top row is the border), when shown.
    pub panel: Option<Rect>,
    pub status: Rect,
}

/// The screen's regions. `panel_height` (`None`: hidden) is clamped so the editor keeps
/// `MIN_EDITOR_HEIGHT` rows; the panel gets at least `MIN_PANEL_HEIGHT` when that fits.
pub fn compute(area: Rect, side_visible: bool, side_width: u16, panel_height: Option<u16>) -> Regions {
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
    let mut editor = Rect {
        x,
        y: area.y,
        width: (area.x + area.width).saturating_sub(x),
        height: body_h,
    };
    let panel = panel_height.and_then(|h| {
        let max = body_h.saturating_sub(MIN_EDITOR_HEIGHT);
        let h = h.min(max).max(MIN_PANEL_HEIGHT.min(max));
        (h > 0).then(|| {
            editor.height -= h;
            Rect {
                y: editor.y + editor.height,
                height: h,
                ..editor
            }
        })
    });
    Regions {
        activity,
        side,
        editor,
        panel,
        status,
    }
}

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
    /// The keyboard cheat sheet, when open; it takes every mouse event.
    pub cheat_sheet: Option<CheatSheetHit>,
    /// The keyboard shortcuts view, when open; it takes every mouse event.
    pub keys: Option<KeysHit>,
    /// The find widget, when shown; it takes clicks on it.
    pub find: Option<FindHit>,
    /// The Search view, when shown.
    pub search: Option<SearchHit>,
    /// The Source Control view, when shown.
    pub scm: Option<ScmHit>,
    /// The Tools view, when shown.
    pub tools: Option<ToolsHit>,
    /// The Explorer view's "Git History" section, when shown.
    pub history: Option<HistoryHit>,
    /// Its list's scroll bar (its last column), when drawn.
    pub history_scrollbar: Option<ScrollBarHit>,
    /// The bottom panel's tab labels.
    pub panel_tabs: Vec<(Rect, PanelTab)>,
    /// The task tabs' close buttons (`×`).
    pub panel_closes: Vec<(Rect, crate::tasks::RunId)>,
    /// The bottom panel's list (Problems or Locations, whichever tab is shown), when shown.
    pub panel_list: Option<ListHit>,
    /// The side bar's list's scroll bar (the explorer, the Search view's results, the Source
    /// Control list, the Tools view), when drawn.
    pub side_scrollbar: Option<(ScrollTarget, ScrollBarHit)>,
    /// The bottom panel's list's scroll bar (Problems, Locations), when drawn.
    pub panel_scrollbar: Option<(ScrollTarget, ScrollBarHit)>,
    /// The status bar's problem counts (a click toggles the Problems panel).
    pub status_problems: Option<Rect>,
    /// The status bar's `Spaces: N` / `Tab Size: N` (a click opens the indentation picker).
    pub status_indentation: Option<Rect>,
    /// The status bar's language name (a click opens Change Language Mode).
    pub status_language: Option<Rect>,
    /// The hover popup, when shown: on top of everything under it.
    pub hover: Option<Rect>,
    /// The completion list, when shown: on top of everything (the hover too).
    pub completion: Option<CompletionHit>,
    /// The code action list, when shown: on top of everything (the completion list too).
    pub code_actions: Option<CompletionHit>,
    /// The gutter's peek, when shown.
    pub quick_diff: Option<QuickDiffHit>,
}

/// The gutter's peek as drawn: its box and its buttons.
#[derive(Debug, Clone, Copy)]
pub struct QuickDiffHit {
    pub rect: Rect,
    pub stage: Rect,
    pub close: Rect,
}

/// The completion list (or the code action list) as drawn: its box, the rows' area and the
/// list row at its top (row `rows.y + i` shows the list's row `top + i`).
#[derive(Debug, Clone, Copy)]
pub struct CompletionHit {
    pub rect: Rect,
    pub rows: Rect,
    pub top: usize,
}

/// A grouped list (`grouped`) as drawn: its rect and the row each line shows (`rows[i]` is at
/// `list.y + i`), by key so a click lands on what was drawn.
#[derive(Debug, Clone, Default)]
pub struct ListHit {
    pub list: Rect,
    pub rows: Vec<crate::grouped::RowKey>,
}

/// The Search view: its option toggles (case, whole word, regex), and its results list with the row each
/// line on screen shows (`rows[i]` is at `list.y + i`), by key so a click lands on what was drawn
/// even when results streamed in since.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Case, whole word, regex.
    pub toggles: [Rect; 3],
    /// The `…` button (show / hide the files to include / exclude).
    pub details: Rect,
    /// The query, files to include and files to exclude inputs (the last two empty while hidden).
    pub inputs: [Rect; 3],
    /// The `▸` / `▾` replace toggle left of the query.
    pub replace_toggle: Rect,
    /// The replace input (empty while hidden).
    pub replace: Rect,
    pub list: Rect,
    pub rows: Vec<crate::search_view::RowKey>,
    /// The `×` dismiss button drawn (on the hovered row), by row.
    pub dismiss: Vec<(Rect, crate::search_view::RowKey)>,
    /// The replace button drawn beside it while the replace input shows, by row.
    pub replace_row: Vec<(Rect, crate::search_view::RowKey)>,
    /// The Replace All button beside the replace input (empty while hidden).
    pub replace_all: Rect,
    /// File rows' chevrons (collapse / expand), by file.
    pub chevrons: Vec<(Rect, std::path::PathBuf)>,
}

/// The Source Control view's list as drawn: its rect, the row each line shows (`rows[i]` is
/// at `list.y + i`, by key) and each stage / unstage button (its row, `true` = unstage).
#[derive(Debug, Clone, Default)]
pub struct ScmHit {
    pub list: Rect,
    pub rows: Vec<crate::scm::RowKey>,
    pub buttons: Vec<(Rect, crate::scm::RowKey, bool)>,
}

/// The Tools view's list as drawn: its rect and the row each line shows (`rows[i]` is at
/// `list.y + i`, by key). Its clicks arrive as `Hit::SidePanel`.
#[derive(Debug, Clone, Default)]
pub struct ToolsHit {
    pub list: Rect,
    pub rows: Vec<crate::tools_view::RowKey>,
}

/// The Explorer view's "Git History" section as drawn: the draggable divider, the header and
/// the commit rows (`top`: the list row drawn first).
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryHit {
    pub divider: Rect,
    pub header: Rect,
    /// The commit rows (`Rect::default()` while collapsed); its last column is the scroll bar.
    pub list: Rect,
    pub top: usize,
}

/// The find widget: its box, the replace row (if shown) and the option toggles (case, whole
/// word, regex, and hex on a hex tab).
#[derive(Debug, Clone, Copy)]
pub struct FindHit {
    pub area: Rect,
    pub replace: Option<Rect>,
    /// Aa, ab, .*, then 0x (an empty rect unless the widget is over a hex tab).
    pub toggles: [Rect; 4],
}

/// The palette box, and its list area: row `i` on screen is row `scroll + i`.
#[derive(Debug, Clone, Copy)]
pub struct PaletteHit {
    pub area: Rect,
    pub rows: Rect,
}

/// A merge conflict's action-row button: what a click on it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeButton {
    /// Accept that side of the conflict at `ByteIdx` (its `<<<<<<<`).
    Accept(crate::conflict::Choice),
    /// A diff of current against incoming (a no-op until the Compare command lands).
    Compare,
}

/// One action-row button as drawn: its rect, what it does, and its conflict's `start`.
pub type MergeButtonHit = (Rect, MergeButton, crate::buffer::pos::ByteIdx);

/// One editor group as drawn: tab bar, editor, and the text area inside it.
#[derive(Debug, Clone, Default)]
pub struct GroupHit {
    pub id: GroupId,
    pub tab_bar: Rect,
    /// Below the tab bar (and the banner, if shown), the scroll bar's column excluded.
    pub editor: Rect,
    /// Text area (gutter excluded).
    pub text: Rect,
    pub tabs: Vec<TabHit>,
    /// The tab bar's scroll arrows, while its tabs overflow it.
    pub tab_arrows: Option<TabArrows>,
    /// The changed-on-disk banner above the editor, when shown.
    pub banner: Option<BannerHit>,
    /// The merge conflicts' action-row buttons drawn in the editor.
    pub merge_buttons: Vec<MergeButtonHit>,
    /// The diff view, when the group shows a diff tab.
    pub diff: Option<DiffHit>,
    /// The vertical scroll bar at the editor's right edge (a text or diff tab).
    pub scrollbar: Option<ScrollBarHit>,
    /// The horizontal scroll bar on the editor's last row, while the view's lines are wider
    /// than it (its track is one row; `ScrollTarget::Columns`).
    pub hscrollbar: Option<ScrollBarHit>,
}

/// What a scroll bar scrolls: a group's active tab, the side bar's list (by the view showing
/// it), the bottom panel's list (by its tab). `App::with_scrollable` maps it to the list's
/// `scrollbar::Scrollable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    Group(GroupId),
    /// A group's active text or diff tab sideways (the horizontal bar).
    Columns(GroupId),
    Side(SidePanel),
    Panel(PanelTab),
    /// The Explorer view's "Git History" commit list.
    History,
    /// The keyboard cheat sheet overlay.
    CheatSheet,
    /// The keyboard shortcuts view overlay.
    Keys,
}

/// The keyboard cheat sheet overlay: its box and its scroll bar.
#[derive(Debug, Clone, Copy)]
pub struct CheatSheetHit {
    pub rect: Rect,
    pub scrollbar: Option<ScrollBarHit>,
}

/// The keyboard shortcuts view: its full-screen rect and its scroll bar.
#[derive(Debug, Clone, Copy)]
pub struct KeysHit {
    pub rect: Rect,
    pub scrollbar: Option<ScrollBarHit>,
}

/// A scroll bar as drawn: its track (one column, or one row for `ScrollTarget::Columns`) and the thumb on it (`None`: nothing to
/// scroll), in cells from the track's start.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollBarHit {
    pub track: Rect,
    pub thumb: Option<crate::scrollbar::Thumb>,
}

/// A diff tab: the rows' rect (below its header), each change's Stage / Unstage button (with
/// the change's hunk index) and the header's Open File button.
#[derive(Debug, Clone, Default)]
pub struct DiffHit {
    pub rows: Rect,
    /// Stage / Unstage, by hunk.
    pub buttons: Vec<(Rect, usize)>,
    /// Revert (an unstaged diff only), by hunk.
    pub reverts: Vec<(Rect, usize)>,
    /// The header's Open File button (the file at the cursor's line), when it fits.
    pub open_file: Option<Rect>,
}

/// The changed-on-disk banner: its row and each button.
#[derive(Debug, Clone, Default)]
pub struct BannerHit {
    pub row: Rect,
    pub actions: Vec<(Rect, crate::disk::BannerAction)>,
}

/// A tab's x range and the x of its close/dirty marker.
#[derive(Debug, Clone, Copy)]
pub struct TabHit {
    pub index: usize,
    pub x0: u16,
    pub x1: u16,
    pub close_x: u16,
}

/// The x of a tab bar's `‹` (scrolls towards the first tab) and `›` (towards the last).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabArrows {
    pub back: u16,
    pub forward: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Icon(SidePanel),
    SideBorder,
    /// Explorer row on screen (add the scroll offset for the entry).
    ExplorerRow(u16),
    /// The Explorer view's "Git History" divider (dragging it resizes the section).
    HistoryDivider,
    /// Its header row (a click toggles the collapse).
    HistoryHeader,
    /// A commit row on screen (add `HistoryHit::top` for the list row).
    HistoryRow(u16),
    SidePanel,
    /// Index into `HitMap::dividers`.
    Divider(usize),
    Tab(GroupId, usize),
    TabClose(GroupId, usize),
    TabBar(GroupId),
    /// A tab bar's scroll arrow: `‹` (false) or `›` (true).
    TabArrow(GroupId, bool),
    /// The changed-on-disk banner over a group's editor, on a button or not.
    Banner(GroupId, Option<crate::disk::BannerAction>),
    Editor(GroupId),
    /// A scroll bar: a group's (`GroupHit::scrollbar`), the side bar's or the panel's list's
    /// (`HitMap::side_scrollbar` / `panel_scrollbar`).
    ScrollBar(ScrollTarget),
    /// The bottom panel's top border (dragging it resizes the panel).
    PanelBorder,
    PanelTab(PanelTab),
    /// A task tab's close button.
    PanelTabClose(crate::tasks::RunId),
    /// Anywhere else in the bottom panel (its list: `HitMap::problems`).
    Panel,
    Status,
    /// The status bar's problem counts.
    StatusProblems,
    /// The status bar's indentation item.
    StatusIndentation,
    StatusLanguage,
    /// The hover popup (over an editor).
    Hover,
    /// The completion list (over an editor).
    Completion,
    /// The code action list (over an editor).
    CodeActions,
    /// The gutter's peek (over an editor).
    QuickDiff,
    Nothing,
}

impl HitMap {
    pub fn hit(&self, x: u16, y: u16) -> Hit {
        let p = Position { x, y };
        let r = &self.regions;
        if self.code_actions.is_some_and(|c| c.rect.contains(p)) {
            return Hit::CodeActions;
        }
        if self.completion.is_some_and(|c| c.rect.contains(p)) {
            return Hit::Completion;
        }
        if self.hover.is_some_and(|h| h.contains(p)) {
            return Hit::Hover;
        }
        if self.quick_diff.is_some_and(|q| q.rect.contains(p)) {
            return Hit::QuickDiff;
        }
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
            if let Some((t, s)) = self.side_scrollbar
                && s.track.contains(p)
            {
                return Hit::ScrollBar(t);
            }
            if let Some(s) = self.history_scrollbar
                && s.track.contains(p)
            {
                return Hit::ScrollBar(ScrollTarget::History);
            }
            if let Some(h) = &self.history {
                if h.divider.contains(p) {
                    return Hit::HistoryDivider;
                }
                if h.header.contains(p) {
                    return Hit::HistoryHeader;
                }
                if h.list.contains(p) {
                    return Hit::HistoryRow(y - h.list.y);
                }
            }
            if self.explorer_rows.contains(p) {
                return Hit::ExplorerRow(y - self.explorer_rows.y);
            }
            return Hit::SidePanel;
        }
        if let Some(panel) = r.panel
            && panel.contains(p)
        {
            if y == panel.y {
                return Hit::PanelBorder;
            }
            if let Some((_, id)) = self.panel_closes.iter().find(|(rect, _)| rect.contains(p)) {
                return Hit::PanelTabClose(*id);
            }
            if let Some((t, s)) = self.panel_scrollbar
                && s.track.contains(p)
            {
                return Hit::ScrollBar(t);
            }
            return match self.panel_tabs.iter().find(|(rect, _)| rect.contains(p)) {
                Some((_, tab)) => Hit::PanelTab(*tab),
                None => Hit::Panel,
            };
        }
        if let Some(i) = self.dividers.iter().position(|d| d.rect.contains(p)) {
            return Hit::Divider(i);
        }
        for g in &self.groups {
            if g.tab_bar.contains(p) {
                if let Some(a) = g.tab_arrows
                    && (x == a.back || x == a.forward)
                {
                    return Hit::TabArrow(g.id, x == a.forward);
                }
                return match g.tabs.iter().find(|t| x >= t.x0 && x < t.x1) {
                    Some(t) if x == t.close_x => Hit::TabClose(g.id, t.index),
                    Some(t) => Hit::Tab(g.id, t.index),
                    None => Hit::TabBar(g.id),
                };
            }
            if let Some(b) = &g.banner
                && b.row.contains(p)
            {
                let action = b.actions.iter().find(|(r, _)| r.contains(p)).map(|(_, a)| *a);
                return Hit::Banner(g.id, action);
            }
            if g.scrollbar.is_some_and(|s| s.track.contains(p)) {
                return Hit::ScrollBar(ScrollTarget::Group(g.id));
            }
            if g.hscrollbar.is_some_and(|s| s.track.contains(p)) {
                return Hit::ScrollBar(ScrollTarget::Columns(g.id));
            }
            if g.editor.contains(p) {
                return Hit::Editor(g.id);
            }
        }
        if r.status.contains(p) {
            if self.status_problems.is_some_and(|c| c.contains(p)) {
                return Hit::StatusProblems;
            }
            if self.status_indentation.is_some_and(|c| c.contains(p)) {
                return Hit::StatusIndentation;
            }
            if self.status_language.is_some_and(|c| c.contains(p)) {
                return Hit::StatusLanguage;
            }
            return Hit::Status;
        }
        Hit::Nothing
    }

    pub fn group(&self, id: GroupId) -> Option<&GroupHit> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// The scroll bar `target` names, as the last frame drew it.
    pub fn scrollbar(&self, target: ScrollTarget) -> Option<ScrollBarHit> {
        let drawn = |bar: Option<(ScrollTarget, ScrollBarHit)>| bar.filter(|(t, _)| *t == target).map(|(_, s)| s);
        match target {
            ScrollTarget::Group(g) => self.group(g)?.scrollbar,
            ScrollTarget::Columns(g) => self.group(g)?.hscrollbar,
            ScrollTarget::Side(_) => drawn(self.side_scrollbar),
            ScrollTarget::Panel(_) => drawn(self.panel_scrollbar),
            ScrollTarget::History => self.history_scrollbar,
            ScrollTarget::CheatSheet => self.cheat_sheet?.scrollbar,
            ScrollTarget::Keys => self.keys?.scrollbar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_tile_the_screen() {
        let r = compute(Rect::new(0, 0, 100, 30), true, 25, None);
        assert_eq!(r.activity, Rect::new(0, 0, 3, 29));
        assert_eq!(r.side, Some(Rect::new(3, 0, 25, 29)));
        assert_eq!(r.editor, Rect::new(28, 0, 72, 29));
        assert_eq!(r.status, Rect::new(0, 29, 100, 1));

        let r = compute(Rect::new(0, 0, 100, 30), false, 25, None);
        assert_eq!(r.side, None);
        assert_eq!(r.editor.x, 3);
    }

    #[test]
    fn side_width_is_clamped() {
        let r = compute(Rect::new(0, 0, 60, 10), true, 200, None);
        assert_eq!(r.side.unwrap().width, 60 - ACTIVITY_WIDTH - MIN_EDITOR_WIDTH);
        let r = compute(Rect::new(0, 0, 60, 10), true, 1, None);
        assert_eq!(r.side.unwrap().width, MIN_SIDE_WIDTH);
    }

    /// The panel sits under the editor groups (not the side panel), clamped both ways.
    #[test]
    fn panel_under_the_editor() {
        let r = compute(Rect::new(0, 0, 100, 30), true, 25, Some(10));
        assert_eq!(r.editor, Rect::new(28, 0, 72, 19));
        assert_eq!(r.panel, Some(Rect::new(28, 19, 72, 10)));
        assert_eq!(r.side, Some(Rect::new(3, 0, 25, 29)), "the side panel keeps its height");
        let r = compute(Rect::new(0, 0, 100, 30), true, 25, Some(100));
        assert_eq!(r.editor.height, MIN_EDITOR_HEIGHT);
        assert_eq!(r.panel.unwrap().height, 29 - MIN_EDITOR_HEIGHT);
        let r = compute(Rect::new(0, 0, 100, 30), true, 25, Some(1));
        assert_eq!(r.panel.unwrap().height, MIN_PANEL_HEIGHT);
        // Too small for both: the editor wins.
        let r = compute(Rect::new(0, 0, 100, 7), true, 25, Some(10));
        assert_eq!((r.editor.height, r.panel.unwrap().height), (5, 1));
        let r = compute(Rect::new(0, 0, 100, 5), true, 25, Some(10));
        assert_eq!((r.editor.height, r.panel), (4, None));
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
    fn hits() {
        let regions = compute(Rect::new(0, 0, 100, 30), true, 25, Some(6));
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
                    width: r.width - 1,
                    ..*r
                },
                text: Rect::default(),
                tab_arrows: (*id == 8).then_some(TabArrows {
                    back: r.x,
                    forward: r.x + r.width - 1,
                }),
                tabs: vec![TabHit {
                    index: 0,
                    x0: r.x,
                    x1: r.x + 10,
                    close_x: r.x + 8,
                }],
                // A banner over the right group's editor, a button at its start.
                banner: (*id == 8).then(|| BannerHit {
                    row: Rect {
                        y: r.y + 1,
                        height: 1,
                        ..*r
                    },
                    actions: vec![(Rect::new(r.x + 2, r.y + 1, 6, 1), crate::disk::BannerAction::Reload)],
                }),
                diff: None,
                merge_buttons: Vec::new(),
                // Its scroll bar at the right edge.
                scrollbar: Some(ScrollBarHit {
                    track: Rect {
                        x: r.x + r.width - 1,
                        y: r.y + 1,
                        width: 1,
                        height: r.height - 1,
                    },
                    thumb: None,
                }),
                hscrollbar: None,
            })
            .collect();
        let map = HitMap {
            regions,
            icons: vec![(1, SidePanel::Explorer)],
            // The explorer's rows, its scroll bar in the column left of the border.
            explorer_rows: Rect::new(3, 2, 23, 20),
            groups,
            dividers,
            palette: None,
            cheat_sheet: None,
            keys: None,
            find: None,
            search: None,
            scm: None,
            tools: None,
            // The history section under the tree: divider, header, then rows.
            history: Some(HistoryHit {
                divider: Rect::new(3, 22, 24, 1),
                header: Rect::new(3, 23, 24, 1),
                list: Rect::new(3, 24, 23, 5),
                top: 0,
            }),
            history_scrollbar: Some(ScrollBarHit {
                track: Rect::new(26, 24, 1, 5),
                thumb: None,
            }),
            panel_tabs: vec![
                (Rect::new(30, 24, 10, 1), PanelTab::Problems),
                (Rect::new(42, 24, 7, 1), PanelTab::Task(crate::tasks::RunId(3))),
            ],
            panel_closes: vec![(Rect::new(50, 24, 1, 1), crate::tasks::RunId(3))],
            panel_list: None,
            side_scrollbar: Some((
                ScrollTarget::Side(SidePanel::Explorer),
                ScrollBarHit {
                    track: Rect::new(26, 2, 1, 20),
                    thumb: None,
                },
            )),
            // The panel's list from row 25, its bar at the right edge.
            panel_scrollbar: Some((
                ScrollTarget::Panel(PanelTab::Problems),
                ScrollBarHit {
                    track: Rect::new(99, 25, 1, 4),
                    thumb: None,
                },
            )),
            status_problems: Some(Rect::new(10, 29, 8, 1)),
            status_indentation: Some(Rect::new(70, 29, 9, 1)),
            status_language: None,
            hover: Some(Rect::new(40, 5, 10, 4)),
            code_actions: None,
            completion: Some(CompletionHit {
                rect: Rect::new(44, 7, 5, 3),
                rows: Rect::new(45, 8, 3, 1),
                top: 0,
            }),
            quick_diff: Some(QuickDiffHit {
                rect: Rect::new(40, 12, 20, 4),
                stage: Rect::new(45, 13, 6, 1),
                close: Rect::new(55, 13, 3, 1),
            }),
        };
        assert_eq!(map.hit(41, 6), Hit::Hover, "the popup is on top");
        assert_eq!(map.hit(41, 13), Hit::QuickDiff, "the gutter's peek over the editor");
        assert_eq!(map.hit(46, 8), Hit::Completion, "the completion list over the hover");
        assert_eq!(map.hit(1, 1), Hit::Icon(SidePanel::Explorer));
        assert_eq!(map.hit(27, 5), Hit::SideBorder);
        assert_eq!(map.hit(5, 4), Hit::ExplorerRow(2));
        assert_eq!(map.hit(5, 0), Hit::SidePanel);
        let side_bar = Hit::ScrollBar(ScrollTarget::Side(SidePanel::Explorer));
        assert_eq!(map.hit(26, 4), side_bar, "the list's last column");
        assert_eq!(map.hit(25, 4), Hit::ExplorerRow(2));
        assert_eq!(map.hit(26, 1), Hit::SidePanel, "above the list");
        assert_eq!(map.hit(99, 26), Hit::ScrollBar(ScrollTarget::Panel(PanelTab::Problems)));
        assert_eq!(map.hit(98, 26), Hit::Panel);
        assert_eq!(map.hit(99, 24), Hit::Panel, "the tab strip runs over it");
        assert_eq!(
            map.scrollbar(ScrollTarget::Side(SidePanel::Explorer)).unwrap().track.x,
            26
        );
        assert!(
            map.scrollbar(ScrollTarget::Side(SidePanel::Search)).is_none(),
            "another view's"
        );
        assert!(map.scrollbar(ScrollTarget::Group(7)).is_some());
        assert_eq!(map.hit(30, 0), Hit::Tab(7, 0));
        assert_eq!(map.hit(36, 0), Hit::TabClose(7, 0));
        let arrows = map.groups.iter().find(|g| g.id == 8).unwrap().tab_arrows.unwrap();
        assert_eq!(map.hit(arrows.back, 0), Hit::TabArrow(8, false));
        assert_eq!(map.hit(arrows.forward, 0), Hit::TabArrow(8, true));
        assert_eq!(map.hit(50, 0), Hit::TabBar(7));
        assert_eq!(map.hit(50, 10), Hit::Editor(7));
        assert_eq!(
            map.hit(63, 10),
            Hit::ScrollBar(ScrollTarget::Group(7)),
            "the editor's last column"
        );
        assert_eq!(map.hit(62, 10), Hit::Editor(7));
        assert_eq!(map.hit(63, 0), Hit::TabBar(7), "the tab bar runs over it");
        assert_eq!(map.hit(64, 10), Hit::Divider(0));
        assert_eq!(map.hit(50, 23), Hit::PanelBorder);
        assert_eq!(map.hit(31, 24), Hit::PanelTab(PanelTab::Problems));
        assert_eq!(map.hit(44, 24), Hit::PanelTab(PanelTab::Task(crate::tasks::RunId(3))));
        assert_eq!(map.hit(50, 24), Hit::PanelTabClose(crate::tasks::RunId(3)));
        assert_eq!(map.hit(50, 26), Hit::Panel);
        assert_eq!(map.hit(12, 29), Hit::StatusProblems);
        assert_eq!(map.hit(70, 29), Hit::StatusIndentation);
        assert_eq!(map.hit(79, 29), Hit::Status);
        assert_eq!(map.hit(5, 20), Hit::ExplorerRow(18), "the tree above the history");
        assert_eq!(map.hit(5, 22), Hit::HistoryDivider);
        assert_eq!(map.hit(5, 23), Hit::HistoryHeader);
        assert_eq!(map.hit(5, 24), Hit::HistoryRow(0));
        assert_eq!(map.hit(5, 26), Hit::HistoryRow(2));
        assert_eq!(
            map.hit(26, 25),
            Hit::ScrollBar(ScrollTarget::History),
            "the history list's last column"
        );
        assert!(map.scrollbar(ScrollTarget::History).is_some());
        assert_eq!(map.hit(26, 4), side_bar, "the tree's own bar");
        assert_eq!(map.hit(66, 0), Hit::Tab(8, 0));
        assert_eq!(map.hit(80, 10), Hit::Editor(8));
        let reload = Some(crate::disk::BannerAction::Reload);
        assert_eq!(map.hit(68, 1), Hit::Banner(8, reload), "the banner over the editor");
        assert_eq!(map.hit(90, 1), Hit::Banner(8, None));
        assert_eq!(map.hit(50, 1), Hit::Editor(7), "no banner there");
        assert_eq!(map.hit(60, 29), Hit::Status);
    }
}
