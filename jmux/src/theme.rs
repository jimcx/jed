//! jmux's colours: Dark+ for the keys the shared widgets and `jterm::draw` read, plus the status bar's.
//! The only file here that names a colour.

use jtui::theme::{Theme, rgb, widget_entries};
use ratatui::style::Style;

/// The theme for a client that shows 24-bit colour (`truecolor`) or the 256 colours.
pub fn dark_plus(truecolor: bool) -> Theme {
    let c = |hex: u32| rgb(hex, truecolor);
    let fg_bg = |fg: u32, bg: u32| Style::new().fg(c(fg)).bg(c(bg));
    // The keys of `jterm::draw::KEYS` that `widget_entries` doesn't have.
    let own = vec![
        ("editor", fg_bg(0xD4D4D4, 0x1E1E1E)),
        ("editor.selection", Style::new().bg(c(0x264F78))),
        ("editor.findMatch", Style::new().bg(c(0x9E6A03))),
        ("editor.findMatchHighlight", Style::new().bg(c(0x613214))),
        ("dialog", fg_bg(0xCCCCCC, 0x252526)),
        ("palette.detail", Style::new().fg(c(0x8C8C8C))),
        ("find.option", Style::new().fg(c(0x8C8C8C))),
        ("find.option.active", fg_bg(0xFFFFFF, 0x1F6FB0)),
        ("statusBar", fg_bg(0xFFFFFF, 0x007ACC)),
        ("statusBar.detail", fg_bg(0xE0E0E0, 0x007ACC)),
        // The zone a dragged tab would take (as jed's `editor.dropBackground`).
        ("terminal.dropBackground", fg_bg(0xFFFFFF, 0x0A4A7A)),
        // The touch layout's `☰` button, shown while the menu is open, and the menu: its fill, the
        // active tab, the item a key moved to, and the line before the actions.
        ("statusBar.menu", fg_bg(0xFFFFFF, 0x005A9E)),
        ("statusBar.menu.active", fg_bg(0x007ACC, 0xFFFFFF)),
        ("menu", fg_bg(0xCCCCCC, 0x252526)),
        ("menu.active", fg_bg(0xFFFFFF, 0x04395E)),
        ("menu.highlight", fg_bg(0xFFFFFF, 0x0E639C)),
        ("menu.separator", Style::new().fg(c(0x5A5A5A))),
        // The close question that replaces the status bar.
        ("statusBar.prompt", fg_bg(0x000000, 0xD7BA7D)),
        // A one-line message (the shell integration is missing) in place of the title.
        ("statusBar.notice", fg_bg(0xFFFFFF, 0x6C4A00)),
        // The commands side bar (jed's side bar keys): its fill, dim text, the selected row, and
        // the glyphs of running / succeeded / failed commands.
        ("sideBar", fg_bg(0xCCCCCC, 0x252526)),
        ("sideBar.dim", Style::new().fg(c(0x858585))),
        ("list.active", fg_bg(0xFFFFFF, 0x04395E)),
        ("list.inactive", Style::new().bg(c(0x37373D))),
        ("commands.running", Style::new().fg(c(0x3794FF))),
        ("commands.ok", Style::new().fg(c(0x0DBC79))),
        ("commands.failed", Style::new().fg(c(0xF48771))),
        // A closed terminal's rows.
        ("commands.closed", Style::new().fg(c(0x5A5A5A))),
    ];
    Theme::new(widget_entries(truecolor).into_iter().chain(own), truecolor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defines_every_key_the_terminal_drawing_reads() {
        for truecolor in [true, false] {
            let theme = dark_plus(truecolor);
            for key in jterm::draw::KEYS.iter().chain(&[
                "statusBar",
                "statusBar.detail",
                "statusBar.prompt",
                "statusBar.notice",
                "terminal.dropBackground",
                "statusBar.menu",
                "statusBar.menu.active",
                "menu",
                "menu.active",
                "menu.highlight",
                "menu.separator",
                "sideBar",
                "sideBar.dim",
                "sideBar.border",
                "list.active",
                "list.inactive",
                "commands.running",
                "commands.ok",
                "commands.failed",
                "commands.closed",
            ]) {
                assert!(theme.find(key).is_some(), "missing {key}");
            }
        }
    }
}
