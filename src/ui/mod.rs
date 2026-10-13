//! ratatui widgets. Colours come from the theme only.

mod activity;
mod dialog;
mod editor;
mod side;
mod status;
mod tabs;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::layout::{self, GroupHit, HitMap, SplitDir};

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

    let (cells, dividers) = app.groups.tree.layout(regions.editor);
    for (id, rect) in cells {
        let tab_bar = Rect {
            height: rect.height.min(1),
            ..rect
        };
        let editor_area = Rect {
            y: rect.y + tab_bar.height,
            height: rect.height - tab_bar.height,
            ..rect
        };
        let is_active = id == app.groups.active;
        let Some(group) = app.groups.get_mut(id) else { continue };
        let tabs = tabs::render(f, tab_bar, group, &app.buffers, &app.theme, is_active);

        let mut text = Rect::default();
        let active = group.active;
        match group
            .tabs
            .get_mut(active)
            .and_then(|t| app.buffers.get(t.buffer).map(|b| (t, b)))
        {
            Some((tab, buf)) => {
                // Only the focused group shows the terminal cursor.
                let focused = is_active && app.ui.focus == Focus::Editor && app.prompt.is_none();
                text = editor::render(f, editor_area, buf, &mut tab.view, &app.theme, &app.settings, focused);
            }
            None => f.buffer_mut().set_style(editor_area, app.theme.get("editor")),
        }
        hits.groups.push(GroupHit {
            id,
            tab_bar,
            editor: editor_area,
            text,
            tabs,
        });
    }
    let border = app.theme.get("editorGroup.border");
    for d in &dividers {
        let (glyph, cells) = match d.dir {
            SplitDir::Right => ("│", d.rect.height),
            SplitDir::Down => ("─", d.rect.width),
        };
        let out = f.buffer_mut();
        for i in 0..cells {
            let (x, y) = match d.dir {
                SplitDir::Right => (d.rect.x, d.rect.y + i),
                SplitDir::Down => (d.rect.x + i, d.rect.y),
            };
            out.set_stringn(x, y, glyph, 1, border);
        }
    }
    hits.dividers = dividers;

    status::render(f, regions.status, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}
