//! ratatui widgets. Colours come from the theme only.

mod dialog;
mod editor;
mod status;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::App;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 2 || area.width < 16 {
        return;
    }
    let [editor_area, status_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    app.text_area = editor::render(f, editor_area, &app.buffer, &mut app.view, &app.theme, &app.settings);
    status::render(f, status_area, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
}
