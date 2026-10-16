//! The splash screen: what an editor group with no tabs shows. The "Jed" logo (a large J, then a
//! smaller "ed" by its top) with "Jim's Editor" under the "ed", centred.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

const LOGO: [&str; 7] = [
    "██████████                 ███",
    "     █████  ▄██▀▀██▄  ▄██▀▀███",
    "     █████  ███▀▀▀▀▀  ███  ███",
    "     █████  ▀██▄▄▄█▀  ▀██▄▄███",
    "     █████",
    "██▄  ████▀",
    " ▀█████▀",
];

/// Where the "ed" starts in the logo's rows, and how wide it is: the subtitle is centred under it.
const ED_COL: u16 = 12;
const ED_WIDTH: u16 = 18;
/// The logo row the subtitle goes on (a blank row below the "ed").
const SUBTITLE_ROW: u16 = 5;

const SUBTITLE: &str = "Jim's Editor";

/// Draws the splash in `area` (the editor area, already filled with `editor`): the logo with the
/// subtitle under its "ed" when it fits, else just the subtitle, else nothing.
pub fn render(f: &mut Frame, area: Rect, theme: &Theme) {
    let base = theme.get("editor");
    let logo_width = LOGO.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let logo_height = LOGO.len() as u16;
    let sub_width = SUBTITLE.width() as u16;
    let out = f.buffer_mut();
    let subtitle = base.patch(theme.get("splash.subtitle"));
    if area.width >= logo_width + 2 && area.height >= logo_height + 2 {
        let x = area.x + (area.width - logo_width) / 2;
        let top = area.y + (area.height - logo_height) / 2;
        let style = base.patch(theme.get("splash.logo"));
        for (i, line) in LOGO.iter().enumerate() {
            out.set_string(x, top + i as u16, line, style);
        }
        out.set_string(
            x + ED_COL + (ED_WIDTH - sub_width) / 2,
            top + SUBTITLE_ROW,
            SUBTITLE,
            subtitle,
        );
    } else if area.width >= sub_width && area.height > 0 {
        let x = area.x + (area.width - sub_width) / 2;
        out.set_string(x, area.y + area.height / 2, SUBTITLE, subtitle);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn draw(w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, f.area(), &Theme::dark_plus(true))).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn logo_centred_with_the_subtitle_under_the_ed() {
        let rows = draw(40, 13);
        // 7 rows in 13: three above; 5 columns left; the subtitle 3 in from the "ed".
        assert_eq!(rows[3], format!("     {}", LOGO[0]));
        let row = &rows[3 + SUBTITLE_ROW as usize];
        let at = row.find(SUBTITLE).expect("subtitle drawn");
        let col = row[..at].chars().count();
        assert_eq!(col, 5 + 12 + 3);
    }

    #[test]
    fn subtitle_only_when_the_logo_does_not_fit() {
        let rows = draw(20, 3);
        assert_eq!(rows[1].trim(), SUBTITLE);
        assert!(draw(10, 3).iter().all(|r| r.is_empty()));
    }
}
