//! Screen regions and mapping mouse positions to them.

use ratatui::layout::{Position, Rect};

pub const ACTIVITY_WIDTH: u16 = 3;
pub const MIN_SIDE_WIDTH: u16 = 12;
/// Room always left for the editor when resizing the side panel.
pub const MIN_EDITOR_WIDTH: u16 = 20;

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
    pub tabs: Rect,
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
    let rest_w = (area.x + area.width).saturating_sub(x);
    let tabs = Rect {
        x,
        y: area.y,
        width: rest_w,
        height: 1.min(body_h),
    };
    let editor = Rect {
        x,
        y: area.y + tabs.height,
        width: rest_w,
        height: body_h.saturating_sub(tabs.height),
    };
    Regions {
        activity,
        side,
        tabs,
        editor,
        status,
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
    /// Tabs: x range and the x of the close/dirty marker.
    pub tabs: Vec<TabHit>,
    /// Editor text area (gutter excluded).
    pub editor_text: Rect,
}

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
    Tab(usize),
    TabClose(usize),
    TabBar,
    Editor,
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
        if r.tabs.contains(p) {
            return match self.tabs.iter().find(|t| x >= t.x0 && x < t.x1) {
                Some(t) if x == t.close_x => Hit::TabClose(t.index),
                Some(t) => Hit::Tab(t.index),
                None => Hit::TabBar,
            };
        }
        if r.editor.contains(p) {
            return Hit::Editor;
        }
        if r.status.contains(p) {
            return Hit::Status;
        }
        Hit::Nothing
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
        assert_eq!(r.tabs, Rect::new(28, 0, 72, 1));
        assert_eq!(r.editor, Rect::new(28, 1, 72, 28));
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
    fn hits() {
        let regions = compute(Rect::new(0, 0, 100, 30), true, 25);
        let map = HitMap {
            regions,
            icons: vec![(1, SidePanel::Explorer)],
            explorer_rows: Rect::new(3, 2, 24, 27),
            tabs: vec![TabHit {
                index: 0,
                x0: 28,
                x1: 40,
                close_x: 38,
            }],
            editor_text: Rect::new(33, 1, 67, 28),
        };
        assert_eq!(map.hit(1, 1), Hit::Icon(SidePanel::Explorer));
        assert_eq!(map.hit(27, 5), Hit::SideBorder);
        assert_eq!(map.hit(5, 4), Hit::ExplorerRow(2));
        assert_eq!(map.hit(5, 0), Hit::SidePanel);
        assert_eq!(map.hit(30, 0), Hit::Tab(0));
        assert_eq!(map.hit(38, 0), Hit::TabClose(0));
        assert_eq!(map.hit(60, 0), Hit::TabBar);
        assert_eq!(map.hit(60, 10), Hit::Editor);
        assert_eq!(map.hit(60, 29), Hit::Status);
    }
}
