//! ratatui widgets. Colours come from the theme only.

mod activity;
mod dialog;
mod editor;
mod side;
mod status;
mod tabs;

use ratatui::Frame;

use crate::app::{App, Focus};
use crate::layout::{self, HitMap};

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 4 || area.width < 30 {
        return;
    }
    let regions = layout::compute(area, app.ui.side_visible, app.ui.side_width);
    let mut hits = HitMap {
        regions,
        ..HitMap::default()
    };

    hits.icons = activity::render(f, regions.activity, app);
    if let Some(side) = regions.side {
        hits.explorer_rows = side::render(f, side, app);
    }
    hits.tabs = tabs::render(f, regions.tabs, app);

    let focused = app.ui.focus == Focus::Editor && app.prompt.is_none();
    let active = app.group.active;
    match app.group.tabs.get_mut(active) {
        Some(tab) => {
            if let Some(buf) = app.buffers.get(tab.buffer) {
                hits.editor_text = editor::render(
                    f,
                    regions.editor,
                    buf,
                    &mut tab.view,
                    &app.theme,
                    &app.settings,
                    focused,
                );
            }
        }
        None => f.buffer_mut().set_style(regions.editor, app.theme.get("editor")),
    }

    status::render(f, regions.status, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}
