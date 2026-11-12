//! Named colour keys: a `Theme` maps a key to a `Style`; widgets never hard-code colours, they look
//! one up. The app builds its theme from a list of entries; `widget_entries` lists the keys the
//! shared widgets (scroll bars, borders, the tab bar, text fields, menus, terminals) read, so both
//! apps start from the same look for them.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    styles: HashMap<&'static str, Style>,
    /// Terminal panes' colours (`terminal_cell`).
    terminal: TerminalColors,
}

/// The 16 ANSI colours (`terminal.ansi*`), the default foreground and background.
struct TerminalColors {
    ansi: [Color; 16],
    fg: Color,
    bg: Color,
    truecolor: bool,
}

/// The `terminal.ansi*` keys, in ANSI order (0–7, then the bright 8–15).
pub const ANSI_KEYS: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

/// Ends what `Theme::ansi` started.
pub const ANSI_RESET: &str = "\x1b[0m";

impl Theme {
    /// A theme of `entries` (a later entry replaces an earlier one with the same key).
    /// `truecolor` says whether the terminal shows 24-bit colour (the entries were built for
    /// it, see `rgb`): it picks how `terminal_cell` maps RGB cells.
    pub fn new(entries: impl IntoIterator<Item = (&'static str, Style)>, truecolor: bool) -> Self {
        let styles: HashMap<&'static str, Style> = entries.into_iter().collect();
        let fg = |key: &str| styles.get(key).and_then(|s| s.fg).unwrap_or(Color::Reset);
        let terminal = TerminalColors {
            ansi: ANSI_KEYS.map(fg),
            fg: fg("terminal.foreground"),
            bg: styles
                .get("terminal.background")
                .and_then(|s| s.bg)
                .unwrap_or(Color::Reset),
            truecolor,
        };
        Self { styles, terminal }
    }

    /// Whether the theme was built for a 24-bit colour terminal.
    pub fn truecolor(&self) -> bool {
        self.terminal.truecolor
    }

    pub fn get(&self, key: &str) -> Style {
        self.styles.get(key).copied().unwrap_or_else(|| {
            tracing::warn!(key, "missing theme key");
            Style::default()
        })
    }

    /// `key`'s style, `None` (and no warning) when the theme doesn't define it.
    pub fn find(&self, key: &str) -> Option<Style> {
        self.styles.get(key).copied()
    }

    /// `style` dimmed for code the server says is compiled out: its foreground (the editor's when
    /// it has none) blended halfway into the editor background, so the syntax colours stay
    /// recognisable. Without an RGB colour to blend (one of the terminal's 16) it takes
    /// `semantic.inactive`'s foreground.
    pub fn dim(&self, style: Style) -> Style {
        let editor = self.get("editor");
        let blended = style.fg.or(editor.fg).zip(editor.bg).and_then(|(fg, bg)| {
            let (f, b) = (rgb_of(fg)?, rgb_of(bg)?);
            let mix = |f: u8, b: u8| ((f as u16 + b as u16) / 2) as u8;
            let hex = (mix(f.0, b.0) as u32) << 16 | (mix(f.1, b.1) as u32) << 8 | mix(f.2, b.2) as u32;
            Some(rgb(hex, matches!(fg, Color::Rgb(..))))
        });
        match blended {
            Some(fg) => style.fg(fg),
            None => style.patch(self.get("semantic.inactive")),
        }
    }

    /// The ANSI escape that starts `key`'s style on a terminal (truecolor, or 256 colours as the
    /// theme was built), for text printed outside the UI (a command-line sample); end it
    /// with `ANSI_RESET`.
    pub fn ansi(&self, key: &str) -> String {
        let style = self.get(key);
        let mut codes = Vec::new();
        for (modifier, code) in [
            (Modifier::BOLD, "1"),
            (Modifier::ITALIC, "3"),
            (Modifier::UNDERLINED, "4"),
        ] {
            if style.add_modifier.contains(modifier) {
                codes.push(code.to_string());
            }
        }
        match style.fg {
            Some(Color::Rgb(r, g, b)) => codes.push(format!("38;2;{r};{g};{b}")),
            Some(Color::Indexed(i)) => codes.push(format!("38;5;{i}")),
            _ => {}
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }

    /// Maps a terminal pane's cell as the app draws it from the `vt100` screen (`Reset` for
    /// the defaults, `Indexed` for palette colours, `Rgb`) to the theme: the defaults to
    /// `terminal.foreground` / `.background`, ANSI 0–15 to `terminal.ansi*` (0–7 drawn bright
    /// when bold, VS Code's `drawBoldTextInBrightColors`), 16–255 kept, RGB to the nearest
    /// 256-colour index without truecolor.
    pub fn terminal_cell(&self, cell: &mut ratatui::buffer::Cell) {
        let t = &self.terminal;
        let bold = cell.modifier.contains(Modifier::BOLD);
        let map = |c: Color, default: Color, fg: bool| match c {
            Color::Reset => default,
            Color::Indexed(i) if i < 8 && fg && bold => t.ansi[i as usize + 8],
            Color::Indexed(i) if i < 16 => t.ansi[i as usize],
            Color::Rgb(r, g, b) if !t.truecolor => Color::Indexed(to_ansi256(r, g, b)),
            c => c,
        };
        cell.fg = map(cell.fg, t.fg, true);
        cell.bg = map(cell.bg, t.bg, false);
    }
}

/// The Dark+ entries for the keys the shared widgets read: the scroll bar thumb, border lines, the
/// tab bar, text fields, context menus and terminal panes (default colours and the ANSI palette).
/// Every app's theme starts from these (`Theme::new(widget_entries(t).into_iter().chain(its own), t)`)
/// and must not repeat a key. `truecolor` picks RGB colours or the nearest of the 256.
pub fn widget_entries(truecolor: bool) -> Vec<(&'static str, Style)> {
    let c = |hex: u32| rgb(hex, truecolor);
    let fg_bg = |fg: u32, bg: u32| Style::new().fg(c(fg)).bg(c(bg));
    vec![
        // The editors' scroll bar thumb (its track is the editor's background): Dark+'s
        // translucent sliders blended over the editor — #79797966, hover #646464B3, while
        // dragged #BFBFBF66.
        ("scrollbarSlider.background", Style::new().bg(c(0x424242))),
        ("scrollbarSlider.hoverBackground", Style::new().bg(c(0x4F4F4F))),
        ("scrollbarSlider.activeBackground", Style::new().bg(c(0x5E5E5E))),
        ("dialog.input", fg_bg(0xCCCCCC, 0x3C3C3C)),
        ("sideBar.border", Style::new().fg(c(0x3C3C3C))),
        ("tabBar", Style::new().bg(c(0x252526))),
        ("tab.active", fg_bg(0xFFFFFF, 0x1E1E1E)),
        ("tab.inactive", fg_bg(0x969696, 0x2D2D2D)),
        ("tab.unfocusedActive", fg_bg(0x8F8F8F, 0x1E1E1E)),
        // The tab bar's `‹` / `›` while its tabs overflow it (Dark+'s icon foreground, and
        // dimmed when there's nothing more that way).
        ("tab.scrollArrow", fg_bg(0xC5C5C5, 0x252526)),
        ("tab.scrollArrow.disabled", fg_bg(0x5A5A5A, 0x252526)),
        // A group's number in its tab bar's top-right corner (two or more groups).
        ("tab.groupNumber", fg_bg(0x6E6E6E, 0x252526)),
        // A shell tab's state glyph: `○` at the prompt, `●` while a command runs, and a red
        // `○` when the last command failed.
        ("tab.shellRunning", Style::new().fg(c(0x3794FF))),
        ("tab.shellIdle", Style::new().fg(c(0x858585))),
        ("tab.shellError", Style::new().fg(c(0xF48771))),
        // Where a dragged tab would land on a tab bar (VS Code's `tab.dropBorder`).
        ("tab.dropMarker", fg_bg(0xFFFFFF, 0x007ACC)),
        ("editorGroup.border", fg_bg(0x444444, 0x1E1E1E)),
        ("tab.deleted", Style::new().add_modifier(Modifier::CROSSED_OUT)),
        ("panel.border", Style::new().fg(c(0x4B4B4B))),
        // Terminal panes (Dark+'s integrated terminal: its foreground, the panel's background,
        // the ANSI palette), and their tabs' state marks.
        ("terminal.foreground", Style::new().fg(c(0xCCCCCC))),
        ("terminal.background", Style::new().bg(c(0x1E1E1E))),
        ("terminal.ansiBlack", Style::new().fg(c(0x000000))),
        ("terminal.ansiRed", Style::new().fg(c(0xCD3131))),
        ("terminal.ansiGreen", Style::new().fg(c(0x0DBC79))),
        ("terminal.ansiYellow", Style::new().fg(c(0xE5E510))),
        ("terminal.ansiBlue", Style::new().fg(c(0x2472C8))),
        ("terminal.ansiMagenta", Style::new().fg(c(0xBC3FBC))),
        ("terminal.ansiCyan", Style::new().fg(c(0x11A8CD))),
        ("terminal.ansiWhite", Style::new().fg(c(0xE5E5E5))),
        ("terminal.ansiBrightBlack", Style::new().fg(c(0x666666))),
        ("terminal.ansiBrightRed", Style::new().fg(c(0xF14C4C))),
        ("terminal.ansiBrightGreen", Style::new().fg(c(0x23D18B))),
        ("terminal.ansiBrightYellow", Style::new().fg(c(0xF5F543))),
        ("terminal.ansiBrightBlue", Style::new().fg(c(0x3B8EEA))),
        ("terminal.ansiBrightMagenta", Style::new().fg(c(0xD670D6))),
        ("terminal.ansiBrightCyan", Style::new().fg(c(0x29B8DB))),
        ("terminal.ansiBrightWhite", Style::new().fg(c(0xE5E5E5))),
        // The context menu: a shade lighter than the editor, a filled selected row.
        ("menu.background", Style::new().bg(c(0x252526))),
        ("menu.foreground", Style::new().fg(c(0xCCCCCC))),
        ("menu.selectionBackground", Style::new().bg(c(0x04395E))),
        ("menu.selectionForeground", Style::new().fg(c(0xFFFFFF))),
        ("menu.disabledForeground", Style::new().fg(c(0x6E6E6E))),
        ("menu.separator", Style::new().fg(c(0x454545))),
    ]
}

/// An underline in `color`, the text's own colour kept.
pub fn squiggle(color: Color) -> Style {
    Style::new().underline_color(color).add_modifier(Modifier::UNDERLINED)
}

/// Whether the terminal advertises 24-bit colour.
pub fn terminal_has_truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
}

/// A colour's red, green and blue: exact for `Rgb`, xterm's for the 256-colour cube and grey
/// ramp; `None` for the terminal's 16 palette colours and the default.
fn rgb_of(color: Color) -> Option<(u8, u8, u8)> {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i @ 16..=231) => {
            let i = (i - 16) as usize;
            Some((LEVELS[i / 36], LEVELS[i / 6 % 6], LEVELS[i % 6]))
        }
        Color::Indexed(i @ 232..=255) => {
            let v = 8 + 10 * (i - 232);
            Some((v, v, v))
        }
        _ => None,
    }
}

pub fn rgb(hex: u32, truecolor: bool) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(to_ansi256(r, g, b))
    }
}

/// Nearest xterm-256 colour: the 6×6×6 cube or the grey ramp, whichever is closer.
pub fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| (0..6).min_by_key(|&i| (LEVELS[i] as i32 - v as i32).abs()).unwrap();
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);

    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gi_ramp = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let grey = 8 + gi_ramp * 10;

    let dist = |(a, b2, c2): (u8, u8, u8)| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a, r) + d(b2, g) + d(c2, b)
    };
    if dist((grey, grey, grey)) < dist(cube) {
        232 + gi_ramp
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

impl crate::style::Styles for Theme {
    fn get(&self, key: &str) -> Style {
        Theme::get(self, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widget entries plus the keys `dim` reads (the editor's colours, the inactive fallback).
    fn test_theme(truecolor: bool) -> Theme {
        let c = |hex: u32| rgb(hex, truecolor);
        let extra = [
            ("editor", Style::new().fg(c(0xD4D4D4)).bg(c(0x1E1E1E))),
            ("semantic.inactive", Style::new().fg(c(0x5A5A5A))),
            ("keyword", Style::new().fg(c(0x569CD6))),
        ];
        Theme::new(widget_entries(truecolor).into_iter().chain(extra), truecolor)
    }

    /// `widget_entries` has no repeated key and defines every key `Theme::new` reads.
    #[test]
    fn widget_entries_are_unique_and_cover_the_terminal_keys() {
        let entries = widget_entries(true);
        let mut keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "a key is listed twice");
        for k in ANSI_KEYS.iter().chain(&["terminal.foreground", "terminal.background"]) {
            assert!(keys.contains(k), "{k} missing");
        }
        assert_eq!(widget_entries(false).len(), total);
        assert!(test_theme(false).find("scrollbarSlider.background").is_some());
        assert!(test_theme(true).truecolor() && !test_theme(false).truecolor());
    }

    /// A later entry replaces an earlier one with the same key.
    #[test]
    fn later_entries_win() {
        let first = Style::new().fg(Color::Rgb(1, 2, 3));
        let second = Style::new().fg(Color::Rgb(4, 5, 6));
        let theme = Theme::new([("k", first), ("other", first), ("k", second)], true);
        assert_eq!(theme.find("k"), Some(second));
        assert_eq!(theme.find("other"), Some(first));
        assert_eq!(theme.find("missing"), None);
    }

    /// `dim` blends the foreground halfway into the editor background (keeping its hue), in
    /// truecolor and 256 colours; a palette colour falls back to `semantic.inactive`.
    #[test]
    fn dim_blends_the_foreground_into_the_background() {
        let theme = test_theme(true);
        // Keyword blue 0x569CD6 over 0x1E1E1E.
        let dim = theme.dim(Style::new().fg(Color::Rgb(0x56, 0x9C, 0xD6)));
        assert_eq!(dim.fg, Some(Color::Rgb(0x3A, 0x5D, 0x7A)));
        // No foreground: the editor's own text colour (0xD4D4D4), dimmed.
        assert_eq!(theme.dim(Style::new()).fg, Some(Color::Rgb(0x79, 0x79, 0x79)));
        let low = test_theme(false);
        let blue = low.get("keyword").fg.unwrap();
        let dimmed = low.dim(Style::new().fg(blue)).fg.unwrap();
        assert!(matches!(dimmed, Color::Indexed(_)) && dimmed != blue);
        assert_eq!(
            theme.dim(Style::new().fg(Color::Red)).fg,
            theme.get("semantic.inactive").fg
        );
    }

    /// `ansi` writes the modifiers then the foreground in the theme's colour depth; nothing for a style without any.
    #[test]
    fn ansi_escapes_follow_the_style() {
        let styled = Style::new()
            .fg(Color::Rgb(1, 2, 3))
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let indexed = Style::new().fg(Color::Indexed(42)).add_modifier(Modifier::ITALIC);
        let theme = Theme::new([("a", styled), ("b", indexed), ("c", Style::new())], true);
        assert_eq!(theme.ansi("a"), "\x1b[1;4;38;2;1;2;3m");
        assert_eq!(theme.ansi("b"), "\x1b[3;38;5;42m");
        assert_eq!(theme.ansi("c"), "");
    }

    #[test]
    fn terminal_cells_take_the_theme_colours() {
        use ratatui::buffer::Cell;
        for truecolor in [true, false] {
            let theme = test_theme(truecolor);
            let mapped = |fg: Color, bg: Color, bold: bool| {
                let mut cell = Cell::default();
                cell.fg = fg;
                cell.bg = bg;
                if bold {
                    cell.modifier = Modifier::BOLD;
                }
                theme.terminal_cell(&mut cell);
                (cell.fg, cell.bg)
            };
            let key_fg = |k: &str| theme.get(k).fg.unwrap();
            assert_eq!(
                mapped(Color::Reset, Color::Reset, false),
                (
                    key_fg("terminal.foreground"),
                    theme.get("terminal.background").bg.unwrap()
                )
            );
            let (fg, bg) = mapped(Color::Indexed(1), Color::Indexed(4), false);
            assert_eq!((fg, bg), (key_fg("terminal.ansiRed"), key_fg("terminal.ansiBlue")));
            assert_eq!(
                mapped(Color::Indexed(1), Color::Reset, true).0,
                key_fg("terminal.ansiBrightRed")
            );
            assert_eq!(
                mapped(Color::Indexed(9), Color::Reset, false).0,
                key_fg("terminal.ansiBrightRed")
            );
            assert_eq!(mapped(Color::Indexed(208), Color::Reset, false).0, Color::Indexed(208));
            let rgb = mapped(Color::Rgb(0, 0x7A, 0xCC), Color::Reset, false).0;
            if truecolor {
                assert_eq!(rgb, Color::Rgb(0, 0x7A, 0xCC));
                assert_eq!(key_fg("terminal.ansiRed"), Color::Rgb(0xCD, 0x31, 0x31));
            } else {
                assert_eq!(rgb, Color::Indexed(32));
                assert!(matches!(key_fg("terminal.ansiRed"), Color::Indexed(_)));
            }
        }
    }

    #[test]
    fn ansi256_mapping() {
        assert_eq!(to_ansi256(0, 0, 0), 16);
        assert_eq!(to_ansi256(255, 255, 255), 231);
        assert_eq!(to_ansi256(0x1E, 0x1E, 0x1E), 234);
        assert_eq!(to_ansi256(0, 0x7A, 0xCC), 32);
    }
}
