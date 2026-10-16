//! ratatui widgets. Colours come from the theme only.

mod activity;
mod dialog;
mod editor;
mod find;
mod hover;
mod palette;
mod panel;
mod popup;
mod search;
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
    let panel_height = app.ui.panel_visible.then_some(app.ui.panel_height);
    let regions = layout::compute(area, app.ui.side_visible, app.ui.side_width, panel_height);
    let mut hits = HitMap {
        regions,
        ..HitMap::default()
    };

    hits.icons = activity::render(f, regions.activity, app);
    if let Some(side) = regions.side {
        (hits.explorer_rows, hits.search) = side::render(f, side, app);
    }

    let (cells, dividers) = app.groups.tree.layout(regions.editor);
    let mut active_editor = None;
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
            .and_then(|t| app.buffers.get_mut(t.buffer).map(|b| (t, b)))
        {
            Some((tab, buf)) => {
                // Only the focused group shows the terminal cursor.
                let focused =
                    is_active && app.ui.focus == Focus::Editor && app.prompt.is_none() && app.palette.is_none();
                // Find highlights in the active group, when the widget's matches are for this text.
                let find = (is_active && app.find.visible && app.find.is_current(tab.buffer, buf.revision()))
                    .then_some(&app.find);
                let overlay = editor::Overlay { focused, find };
                text = editor::render(f, editor_area, buf, &mut tab.view, &app.theme, &app.settings, overlay);
            }
            None => f.buffer_mut().set_style(editor_area, app.theme.get("editor")),
        }
        if is_active {
            active_editor = Some(editor_area);
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
    if let Some(panel) = regions.panel {
        (hits.panel_tabs, hits.panel_list) = panel::render(f, panel, app);
    }

    hits.status_problems = status::render(f, regions.status, app);
    hits.find = active_editor.and_then(|area| find::render(f, area, app));
    hits.hover = hover::render(f, app, &hits.groups);
    hits.palette = palette::render(f, area, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}
