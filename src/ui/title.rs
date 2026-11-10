//! The title bar (`[window] title_bar`): `jed <root>` (or the window title override) on the left,
//! the `[[window.buttons]]` at the far right.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::App;

/// Room kept for the title when the buttons would crowd it out.
const MIN_TITLE: u16 = 12;

/// Draws the bar in `area` (one row); returns each button drawn with its index in
/// `[window] buttons`. Buttons that don't fit (counting from the right-hand end) are left out.
pub fn render(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, usize)> {
    let (bar, button) = if app.window_focused {
        ("titleBar", "titleBar.button")
    } else {
        ("titleBar.inactive", "titleBar.inactive.button")
    };
    let style = app.theme.get(bar);
    let button_style = app.theme.get(button);
    let out = f.buffer_mut();
    out.set_style(area, style);

    let buttons = app.workspace.window().buttons.unwrap_or_default();
    let end = area.x + area.width;
    let mut x = end;
    let mut hits = Vec::new();
    // From the right, so the first button is the leftmost; one blank cell between buttons.
    for (i, button) in buttons.iter().enumerate().rev() {
        let text = format!(
            " {} ",
            button.text().chars().filter(|c| !c.is_control()).collect::<String>()
        );
        let w = text.width() as u16;
        if x.saturating_sub(area.x) < w + 1 + MIN_TITLE {
            break;
        }
        x -= w;
        out.set_stringn(x, area.y, &text, w as usize, button_style);
        hits.push((Rect::new(x, area.y, w, 1), i));
        x -= 1;
    }
    // Experiment: the leading "jed" is drawn as "Jᵉᵈ".
    let text = app.identity.title_text();
    let text = match text.strip_prefix("jed") {
        Some(rest) => format!("J\u{1d49}\u{1d48}{rest}"),
        None => text,
    };
    let title = format!(" {text}");
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    out.set_stringn(area.x, area.y, &title, x.saturating_sub(area.x) as usize, style);
    hits
}
