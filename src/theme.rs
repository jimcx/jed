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
            ("editor.whitespace.control", Style::new().fg(c(0xF48771))),
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
            ("tab.deleted", Style::new().add_modifier(Modifier::CROSSED_OUT)),
            ("statusBar.restricted", fg_bg(0x1E1E1E, 0xCCA700)),
        ]);
        Self { styles }
    }
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

    #[test]
    fn ansi256_mapping() {
        assert_eq!(to_ansi256(0, 0, 0), 16);
        assert_eq!(to_ansi256(255, 255, 255), 231);
        assert_eq!(to_ansi256(0x1E, 0x1E, 0x1E), 234);
        assert_eq!(to_ansi256(0, 0x7A, 0xCC), 32);
    }
}
