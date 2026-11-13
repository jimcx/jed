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
            for key in jterm::draw::KEYS.iter().chain(&["statusBar", "statusBar.detail"]) {
                assert!(theme.find(key).is_some(), "missing {key}");
            }
        }
    }
}
