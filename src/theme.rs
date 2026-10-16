//! Named colour keys. Widgets never hard-code colours; they look up a key here.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};

pub struct Theme {
    styles: HashMap<&'static str, Style>,
}

impl Theme {
    pub fn get(&self, key: &str) -> Style {
        self.styles.get(key).copied().unwrap_or_else(|| {
            tracing::warn!(key, "missing theme key");
            Style::default()
        })
    }

    /// The built-in dark theme, modelled on VS Code Dark+.
    pub fn dark_plus(truecolor: bool) -> Self {
        let c = |hex: u32| rgb(hex, truecolor);
        let fg_bg = |fg: u32, bg: u32| Style::new().fg(c(fg)).bg(c(bg));
        let styles = HashMap::from([
            ("editor", fg_bg(0xD4D4D4, 0x1E1E1E)),
            ("editor.lineHighlight", Style::new().bg(c(0x2A2D2E))),
            ("editor.selection", Style::new().bg(c(0x264F78))),
            ("editor.cursor.secondary", Style::new().add_modifier(Modifier::REVERSED)),
            // Dark Modern's current match; Dark+'s other matches (#EA5C0055) blended over the editor.
            ("editor.findMatch", Style::new().bg(c(0x9E6A03))),
            ("editor.findMatchHighlight", Style::new().bg(c(0x613214))),
            // Dark+'s `editorBracketMatch` is a faint green fill with a grey border; a terminal cell
            // has no border, so a stronger fill.
            ("editor.bracketMatch", Style::new().bg(c(0x2F4F3A))),
            ("editor.whitespace.control", Style::new().fg(c(0xF48771))),
            // Diagnostics' squiggles (Dark+'s colours): a terminal has no undercurl through
            // ratatui, so an underline in the colour (SGR 58; terminals without it underline in
            // the text's colour). Dark+'s hint (#EEEEEEB3) blended over the editor.
            ("editorError.foreground", squiggle(c(0xF14C4C))),
            ("editorWarning.foreground", squiggle(c(0xCCA700))),
            ("editorInfo.foreground", squiggle(c(0x3794FF))),
            ("editorHint.foreground", squiggle(c(0xB0B0B0))),
            ("gutter", fg_bg(0x858585, 0x1E1E1E)),
            ("gutter.active", Style::new().fg(c(0xC6C6C6))),
            ("statusBar", fg_bg(0xFFFFFF, 0x007ACC)),
            ("statusBar.warning", fg_bg(0x1E1E1E, 0xCCA700)),
            ("statusBar.error", fg_bg(0xFFFFFF, 0xC72E0F)),
            ("dialog", fg_bg(0xCCCCCC, 0x252526)),
            ("dialog.border", Style::new().fg(c(0x454545))),
            ("dialog.key", Style::new().fg(c(0x3794FF)).add_modifier(Modifier::BOLD)),
            ("dialog.input", fg_bg(0xCCCCCC, 0x3C3C3C)),
            ("dialog.error", Style::new().fg(c(0xF48771))),
            ("activityBar", fg_bg(0x858585, 0x333333)),
            ("activityBar.active", Style::new().fg(c(0xFFFFFF))),
            ("sideBar", fg_bg(0xCCCCCC, 0x252526)),
            ("sideBar.title", Style::new().fg(c(0xBBBBBB))),
            (
                "sideBar.header",
                Style::new().fg(c(0xCCCCCC)).add_modifier(Modifier::BOLD),
            ),
            ("sideBar.border", Style::new().fg(c(0x3C3C3C))),
            ("sideBar.dim", Style::new().fg(c(0x858585))),
            ("list.active", fg_bg(0xFFFFFF, 0x04395E)),
            ("list.inactive", Style::new().bg(c(0x37373D))),
            ("tabBar", Style::new().bg(c(0x252526))),
            ("tab.active", fg_bg(0xFFFFFF, 0x1E1E1E)),
            ("tab.inactive", fg_bg(0x969696, 0x2D2D2D)),
            ("tab.unfocusedActive", fg_bg(0x8F8F8F, 0x1E1E1E)),
            ("editorGroup.border", fg_bg(0x444444, 0x1E1E1E)),
            ("tab.deleted", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            ("statusBar.restricted", fg_bg(0x1E1E1E, 0xCCA700)),
            (
                "palette.highlight",
                Style::new().fg(c(0x2AAAFF)).add_modifier(Modifier::BOLD),
            ),
            ("palette.detail", Style::new().fg(c(0x8C8C8C))),
            ("find.option", Style::new().fg(c(0x8C8C8C))),
            ("find.option.active", fg_bg(0xFFFFFF, 0x1F6FB0)),
            // Search view: Dark+'s find highlight (#EA5C0055) blended over the side bar.
            ("search.match", Style::new().bg(c(0x663719))),
            (
                "search.fileName",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::BOLD),
            ),
            ("search.count", fg_bg(0xFFFFFF, 0x4D4D4D)),
            ("search.error", Style::new().fg(c(0xF48771))),
            ("search.warning", Style::new().fg(c(0xCCA700))),
            // The bottom panel (Dark+: panel background as the editor's, a #80808059 border).
            ("panel", fg_bg(0xCCCCCC, 0x1E1E1E)),
            ("panel.border", Style::new().fg(c(0x4B4B4B))),
            ("panel.dim", Style::new().fg(c(0x8C8C8C))),
            (
                "panelTitle.active",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::UNDERLINED),
            ),
            ("panelTitle.inactive", Style::new().fg(c(0x969696))),
            ("panel.badge", fg_bg(0xFFFFFF, 0x4D4D4D)),
            (
                "panel.fileName",
                Style::new().fg(c(0xE7E7E7)).add_modifier(Modifier::BOLD),
            ),
            ("problemsErrorIcon.foreground", Style::new().fg(c(0xF14C4C))),
            ("problemsWarningIcon.foreground", Style::new().fg(c(0xCCA700))),
            ("problemsInfoIcon.foreground", Style::new().fg(c(0x3794FF))),
            ("problemsHintIcon.foreground", Style::new().fg(c(0xB0B0B0))),
            // The hover popup (Dark+), its headings bold, code (inline and blocks) in Dark+'s
            // `textPreformat.foreground`.
            ("editorHoverWidget.background", Style::new().bg(c(0x252526))),
            ("editorHoverWidget.foreground", Style::new().fg(c(0xCCCCCC))),
            ("editorHoverWidget.border", Style::new().fg(c(0x454545))),
            ("editorHoverWidget.heading", Style::new().add_modifier(Modifier::BOLD)),
            ("textPreformat.foreground", Style::new().fg(c(0xD7BA7D))),
            // Syntax (the capture keys in `syntax::KEYS`): VS Code Dark+'s token colours.
            ("comment", Style::new().fg(c(0x6A9955))),
            ("constant", Style::new().fg(c(0x4FC1FF))),
            ("constant.builtin", Style::new().fg(c(0x569CD6))),
            ("constructor", Style::new().fg(c(0x4EC9B0))),
            ("escape", Style::new().fg(c(0xD7BA7D))),
            ("function", Style::new().fg(c(0xDCDCAA))),
            ("keyword", Style::new().fg(c(0x569CD6))),
            ("keyword.control", Style::new().fg(c(0xC586C0))),
            ("label", Style::new().fg(c(0xC8C8C8))),
            // Markdown (Dark+'s `markup.heading`, `markup.inline.raw`, `markup.underline`, the
            // list and quote punctuation).
            (
                "markup.heading",
                Style::new().fg(c(0x569CD6)).add_modifier(Modifier::BOLD),
            ),
            (
                "markup.link",
                Style::new().fg(c(0xD4D4D4)).add_modifier(Modifier::UNDERLINED),
            ),
            ("markup.list", Style::new().fg(c(0x6796E6))),
            ("markup.quote", Style::new().fg(c(0x6A9955))),
            ("markup.raw", Style::new().fg(c(0xCE9178))),
            ("number", Style::new().fg(c(0xB5CEA8))),
            ("operator", Style::new().fg(c(0xD4D4D4))),
            ("property", Style::new().fg(c(0x9CDCFE))),
            ("punctuation", Style::new().fg(c(0xD4D4D4))),
            ("punctuation.special", Style::new().fg(c(0x569CD6))),
            // XML (Dark+'s `punctuation.definition.tag`, `entity.name.tag`, attribute names); YAML
            // keys are tags too.
            ("punctuation.tag", Style::new().fg(c(0x808080))),
            ("string", Style::new().fg(c(0xCE9178))),
            ("tag", Style::new().fg(c(0x569CD6))),
            ("tag.attribute", Style::new().fg(c(0x9CDCFE))),
            ("type", Style::new().fg(c(0x4EC9B0))),
            ("type.builtin", Style::new().fg(c(0x569CD6))),
            ("variable", Style::new().fg(c(0x9CDCFE))),
            ("variable.builtin", Style::new().fg(c(0x569CD6))),
            ("variable.parameter", Style::new().fg(c(0x9CDCFE))),
        ]);
        Self { styles }
    }
}

/// An underline in `color`, the text's own colour kept.
fn squiggle(color: Color) -> Style {
    Style::new().underline_color(color).add_modifier(Modifier::UNDERLINED)
}

/// Whether the terminal advertises 24-bit colour.
pub fn terminal_has_truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
}

fn rgb(hex: u32, truecolor: bool) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(to_ansi256(r, g, b))
    }
}

/// Nearest xterm-256 colour: the 6×6×6 cube or the grey ramp, whichever is closer.
fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every highlight key has a colour, distinct in 256-colour mode for the main token kinds.
    #[test]
    fn syntax_keys_are_in_the_theme() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for key in crate::syntax::KEYS {
                assert!(theme.styles.get(key).and_then(|s| s.fg).is_some(), "{key}");
            }
        }
        let theme = Theme::dark_plus(false);
        let fg = |k: &str| theme.get(k).fg.unwrap();
        assert_eq!(fg("keyword"), Color::Indexed(to_ansi256(0x56, 0x9C, 0xD6)));
        let kinds = [
            "keyword",
            "keyword.control",
            "string",
            "comment",
            "number",
            "type",
            "function",
            "variable",
        ];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert_ne!(fg(a), fg(b), "{a} and {b}");
            }
        }
    }

    #[test]
    fn squiggles_are_coloured_underlines() {
        for truecolor in [true, false] {
            let theme = Theme::dark_plus(truecolor);
            for s in crate::diagnostics::Severity::ALL {
                let style = theme.styles[s.theme_key()];
                assert!(style.add_modifier.contains(Modifier::UNDERLINED), "{s:?}");
                assert!(style.underline_color.is_some() && style.fg.is_none() && style.bg.is_none());
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
