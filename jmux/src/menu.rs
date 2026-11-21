//! The touch layout's tab menu (`☰` on the status bar): its state and geometry. It lists the
//! focused group's tabs, a separator, then the actions of `actions()`, each item a text row with an
//! empty row of padding above, between and below, so a finger has three rows to hit. `app/menu.rs`
//! drives it, `ui_menu` draws it.

use ratatui::layout::Rect;

use crate::command::Command;

/// The widest the menu gets; a narrower screen gets all of its width.
pub const MAX_WIDTH: u16 = 40;

/// The actions listed after the tabs: a label and the command it runs. Adding an action is one line.
pub fn actions() -> Vec<(&'static str, Command)> {
    vec![("New Tab", Command::NewTerminal)]
}

/// What an item does when picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Focus the focused group's tab `index`.
    Tab(usize),
    Action(Command),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub target: Target,
    /// The focused group's active tab.
    pub active: bool,
}

/// What a row of the menu's content is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Padding that belongs to the item (a tap there picks it).
    Pad(usize),
    Text(usize),
    Separator,
}

/// The menu's rows top to bottom: padding, each tab and padding, the separator, then the same for
/// the actions. `tabs` is how many items are tabs, `total` how many there are.
pub fn rows(tabs: usize, total: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for i in 0..total {
        if i == tabs && tabs > 0 {
            rows.push(Row::Separator);
        }
        // The padding above an item belongs to it; the last of a section also owns the padding below.
        rows.push(Row::Pad(i));
        rows.push(Row::Text(i));
        if i + 1 == tabs || i + 1 == total {
            rows.push(Row::Pad(i));
        }
    }
    rows
}

/// The menu's state while it is open.
#[derive(Debug, Clone, Default)]
pub struct Menu {
    /// The item a key has moved to; none until one does.
    pub highlight: Option<usize>,
    /// Rows scrolled off the top.
    pub scroll: u16,
    /// The furthest `scroll` goes and the rows shown, as the last frame drew them.
    pub max_scroll: u16,
    pub view_rows: u16,
}

impl Menu {
    /// Scrolls by `rows` (positive: later rows come in), within the content.
    pub fn scroll_by(&mut self, rows: i32) {
        let at = i32::from(self.scroll) + rows;
        self.scroll = at.clamp(0, i32::from(self.max_scroll)) as u16;
    }

    /// Scrolls just enough that item `i`'s three rows show.
    pub fn reveal(&mut self, rows: &[Row], i: usize) {
        let Some(text) = rows.iter().position(|r| *r == Row::Text(i)) else {
            return;
        };
        let top = text.saturating_sub(1) as u16;
        let bottom = (text + 2).min(rows.len()) as u16;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + self.view_rows {
            self.scroll = bottom.saturating_sub(self.view_rows);
        }
        self.scroll = self.scroll.min(self.max_scroll);
    }
}

/// The menu's area in a `rows` × `cols` screen: its bottom edge on the row above the status bar,
/// all the width of a narrow screen, at most `MAX_WIDTH` otherwise, `content` rows tall or as many
/// as fit above the status bar.
pub fn area(rows: u16, cols: u16, content: usize) -> Rect {
    let free = rows.saturating_sub(crate::ui::STATUS_ROWS);
    let height = (content.min(usize::from(free))) as u16;
    Rect::new(0, free - height, cols.min(MAX_WIDTH), height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_pad_each_item_and_separate_the_actions() {
        use Row::*;
        assert_eq!(
            rows(2, 3),
            vec![
                Pad(0),
                Text(0),
                Pad(1),
                Text(1),
                Pad(1),
                Separator,
                Pad(2),
                Text(2),
                Pad(2)
            ]
        );
        assert_eq!(rows(0, 1), vec![Pad(0), Text(0), Pad(0)]);
    }

    #[test]
    fn the_area_sits_on_the_status_bar_and_stays_on_screen() {
        assert_eq!(area(30, 80, 10), Rect::new(0, 19, 40, 10));
        assert_eq!(area(30, 30, 100), Rect::new(0, 0, 30, 29));
    }

    #[test]
    fn reveal_scrolls_the_item_into_view() {
        let r = rows(5, 6);
        let mut m = Menu {
            view_rows: 6,
            max_scroll: (r.len() - 6) as u16,
            ..Menu::default()
        };
        m.reveal(&r, 4);
        let text = r.iter().position(|x| *x == Row::Text(4)).unwrap() as u16;
        assert!(m.scroll < text && text + 2 <= m.scroll + 6, "{}", m.scroll);
        m.reveal(&r, 0);
        assert_eq!(m.scroll, 0);
    }
}
