//! The commands side bar's state and the pure parts of its layout (no drawing: `ui_side` draws it,
//! `app/side.rs` drives it). The rows themselves are `history::History`'s; this holds what the bar
//! adds: whether it shows, its width, whether it has the keys, the filter box.

use jtui::field::TextField;
use unicode_width::UnicodeWidthStr;

/// The narrowest the bar is drawn (the border drag stops here).
pub const MIN_WIDTH: u16 = 12;
/// Its width until the border is dragged.
pub const DEFAULT_WIDTH: u16 = 32;
/// The columns the groups keep when the bar is dragged wide.
pub const MIN_GROUPS: u16 = 20;

#[derive(Debug)]
pub struct SideBar {
    pub visible: bool,
    /// The width asked for (the drawn one is `width_in`: it gives way on a narrow screen).
    pub width: u16,
    /// The bar has the keys (`Context::Side`).
    pub focused: bool,
    /// The filter box over the rows (`History::set_filter` follows it).
    pub filter: TextField,
    /// Rows the list showed last frame, for the keys that move by a page (set by `ui_side`).
    pub view_rows: usize,
}

impl SideBar {
    pub fn new() -> SideBar {
        SideBar {
            visible: false,
            width: DEFAULT_WIDTH,
            focused: false,
            filter: TextField::new(String::new()),
            view_rows: 10,
        }
    }

    /// The bar's drawn width on a `cols`-wide screen (without its border column); 0 while hidden.
    pub fn width_in(&self, cols: u16) -> u16 {
        if self.visible { clamp_width(self.width, cols) } else { 0 }
    }

    /// The columns the bar takes off the groups: its width and the border, none while hidden.
    pub fn reserved(&self, cols: u16) -> u16 {
        if self.visible { self.width_in(cols) + 1 } else { 0 }
    }
}

impl Default for SideBar {
    fn default() -> Self {
        SideBar::new()
    }
}

/// `width` limited to `MIN_WIDTH`, and to what leaves the groups `MIN_GROUPS` columns and the border
/// column; on a screen too narrow for both, to the screen less the border.
pub fn clamp_width(width: u16, cols: u16) -> u16 {
    let max = cols.saturating_sub(MIN_GROUPS + 1).max(MIN_WIDTH);
    width.clamp(MIN_WIDTH, max).min(cols.saturating_sub(1))
}

/// What the list says while it has no rows. `missing`: no shell has the integration; `key` is the
/// install command's key label.
pub fn empty_text(missing: bool, key: &str) -> String {
    if missing {
        let press = if key.is_empty() {
            String::new()
        } else {
            format!("press {key} or ")
        };
        format!("No commands yet. Shell integration isn't installed: {press}run jmux install-shell-integration")
    } else {
        "No commands yet".to_string()
    }
}

/// `text` broken into lines of at most `width` columns at spaces (a longer word is cut).
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.width() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let mut head = String::new();
            for c in word.chars() {
                if (head.clone() + &c.to_string()).width() > width {
                    break;
                }
                head.push(c);
            }
            let rest = word[head.len()..].to_string();
            lines.push(head);
            word = rest;
        }
        let need = if line.is_empty() { 0 } else { line.width() + 1 };
        if need + word.width() > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` cut to `width` columns, ending in `…` when it was cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        let next = out.clone() + &c.to_string();
        if next.width() + 1 > width {
            break;
        }
        out = next;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Where a row's parts go in a list `width` columns wide: the glyph at column 1, the command at 3,
/// then the terminal's title and the time at the right edge.
#[derive(Debug, PartialEq, Eq)]
pub struct RowCols {
    /// The command's room.
    pub command: usize,
    /// The title's start column and room, when there is room for it.
    pub title: Option<(usize, usize)>,
    /// The time's start column.
    pub time: usize,
}

/// Lays out a row of a list `width` columns wide with a time `time_w` wide and a title of `title_w`.
/// The command keeps most of the room; the title gets up to a third of what is left, and goes
/// first when the bar is narrow.
pub fn row_cols(width: usize, time_w: usize, title_w: usize) -> RowCols {
    let left = 3;
    let time = width.saturating_sub(time_w + 1).max(left);
    let room = time.saturating_sub(left + 1);
    let title_room = if room >= 18 { (room / 3).min(title_w) } else { 0 };
    if title_room == 0 {
        return RowCols {
            command: room,
            title: None,
            time,
        };
    }
    let command = room - title_room - 1;
    RowCols {
        command,
        title: Some((left + command + 1, title_room)),
        time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_keeps_the_minimum_and_leaves_the_groups_their_room() {
        assert_eq!(clamp_width(5, 100), MIN_WIDTH);
        assert_eq!(clamp_width(32, 100), 32);
        assert_eq!(clamp_width(200, 100), 100 - MIN_GROUPS - 1);
        // Too narrow a screen for both: the bar yields to the border column.
        assert_eq!(clamp_width(32, 20), 12);
        assert_eq!(clamp_width(32, 8), 7);
        let mut side = SideBar::new();
        assert_eq!((side.width_in(100), side.reserved(100)), (0, 0));
        side.visible = true;
        assert_eq!((side.width_in(100), side.reserved(100)), (32, 33));
    }

    #[test]
    fn the_empty_text_says_how_to_install_only_when_it_is_missing() {
        let text = empty_text(true, "Ctrl+Shift+I");
        assert!(
            text.contains("press Ctrl+Shift+I or run jmux install-shell-integration"),
            "{text}"
        );
        assert!(empty_text(true, "").contains("run jmux install-shell-integration"));
        assert_eq!(empty_text(false, "Ctrl+Shift+I"), "No commands yet");
    }

    #[test]
    fn wrap_and_fit_respect_the_width() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert!(wrap("", 5).is_empty());
        assert_eq!(fit("hello", 5), "hello");
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(fit("hello", 0), "");
    }

    #[test]
    fn a_row_gives_the_time_the_edge_and_the_title_only_what_the_command_can_spare() {
        // 30 wide: glyph, command, title, a 4-wide time at the right.
        let cols = row_cols(30, 4, 6);
        assert_eq!(cols.time, 25);
        let (x, w) = cols.title.unwrap();
        assert_eq!(w, 6);
        assert_eq!(x, 3 + cols.command + 1);
        assert!(x + w < cols.time);
        // Narrow: no title, the command takes the room.
        let cols = row_cols(14, 4, 6);
        assert_eq!((cols.title, cols.command, cols.time), (None, 5, 9));
        // A tiny list does not underflow.
        let _ = row_cols(2, 10, 10);
    }
}
