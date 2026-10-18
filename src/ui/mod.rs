//! ratatui widgets. Colours come from the theme only.

mod activity;
mod banner;
mod completion;
mod dialog;
mod diff;
mod editor;
mod find;
mod hover;
mod palette;
mod panel;
mod popup;
mod quick_diff;
mod scm;
mod scrollbar;
mod search;
mod side;
mod splash;
mod status;
mod tabs;
mod task;
mod tools;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::edit::RenderWhitespace;
use crate::group::Tab;
use crate::layout::{self, GroupHit, HitMap, ScrollTarget, SplitDir};
use crate::scrollbar::Scrollable;
use crate::view::TextScroll;

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
        let side = side::render(f, side, app);
        (hits.explorer_rows, hits.search, hits.scm, hits.tools) =
            (side.explorer_rows, side.search, side.scm, side.tools);
        hits.side_scrollbar = side.scrollbar;
    }

    let (cells, dividers) = app.groups.tree.layout(regions.editor);
    let mut active_editor = None;
    let whitespace = app.render_whitespace();
    for (id, rect) in cells {
        let tab_bar = Rect {
            height: rect.height.min(1),
            ..rect
        };
        let mut editor_area = Rect {
            y: rect.y + tab_bar.height,
            height: rect.height - tab_bar.height,
            ..rect
        };
        let is_active = id == app.groups.active;
        let look = scrollbar::look(app, ScrollTarget::Group(id));
        let editor_bg = app.theme.get("editor");
        let Some(group) = app.groups.get_mut(id) else { continue };
        let tabs = tabs::render(f, tab_bar, group, &app.buffers, &app.theme, is_active);
        // The changed-on-disk banner takes the editor's first row (leaving it at least one); a
        // diff tab of the file shows it too.
        let mut banner = None;
        let file = match group.tabs.get(group.active) {
            Some(Tab::Text(t)) => app.buffers.get(t.buffer),
            Some(Tab::Diff(d)) => app.buffers.iter().find(|(_, b)| b.path == d.path).map(|(_, b)| b),
            None => None,
        };
        if let Some(buf) = file
            && buf.disk_conflict.is_some()
            && editor_area.height >= 2
        {
            let row = Rect {
                height: 1,
                ..editor_area
            };
            banner = Some(banner::render(f, row, &buf.name(), buf.has_edits(), &app.theme));
            editor_area.y += 1;
            editor_area.height -= 1;
        }

        // A text or diff tab keeps the editor's last column for its scroll bar.
        let bar = if group.tabs.is_empty() {
            None
        } else {
            scrollbar::take_column(&mut editor_area)
        };
        let mut text = Rect::default();
        let mut diff_hit = None;
        let mut scroll_hit = None;
        // Only the focused group shows the terminal cursor.
        let focused = is_active && app.ui.focus == Focus::Editor && app.prompt.is_none() && app.palette.is_none();
        if let Some(d) = group.active_diff_mut() {
            diff_hit = Some(diff::render(
                f,
                editor_area,
                d,
                &app.theme,
                app.settings.tab_width,
                focused,
            ));
            if let Some(bar) = bar {
                // The header row's cell, then the rows' track.
                let out = f.buffer_mut();
                out.set_style(bar, editor_bg);
                out.set_style(Rect { height: 1, ..bar }, app.theme.get("diffEditor.header"));
                let track = Rect {
                    y: bar.y + 1,
                    height: bar.height.saturating_sub(1),
                    ..bar
                };
                scroll_hit = Some(scrollbar::render(out, track, d.scroll(), look, editor_bg, &app.theme));
            }
        }
        match group
            .active_tab_mut()
            .and_then(|t| app.buffers.get_mut(t.buffer).map(|b| (t, b)))
        {
            Some((tab, buf)) => {
                // Find highlights in the active group, when the widget's matches are for this text.
                let find = (is_active && app.find.visible && app.find.is_current(tab.buffer, buf.revision()))
                    .then_some(&app.find);
                let overlay = editor::Overlay {
                    focused,
                    find,
                    // A log is read, not edited: no whitespace glyphs.
                    whitespace: if buf.log.is_some() {
                        RenderWhitespace::None
                    } else {
                        whitespace
                    },
                    blame: app.blame.on,
                };
                text = editor::render(f, editor_area, buf, &mut tab.view, &app.theme, &app.settings, overlay);
                if let Some(bar) = bar {
                    let scroll = TextScroll {
                        view: &mut tab.view,
                        rope: &buf.rope,
                    }
                    .scroll();
                    scroll_hit = Some(scrollbar::render(
                        f.buffer_mut(),
                        bar,
                        scroll,
                        look,
                        editor_bg,
                        &app.theme,
                    ));
                }
            }
            None if diff_hit.is_some() => {}
            None => {
                f.buffer_mut().set_style(editor_area, app.theme.get("editor"));
                splash::render(f, editor_area, &app.theme);
            }
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
            banner,
            diff: diff_hit,
            scrollbar: scroll_hit,
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
        let panel = panel::render(f, panel, app);
        (hits.panel_tabs, hits.panel_closes, hits.panel_list) = (panel.tabs, panel.closes, panel.list);
        hits.panel_scrollbar = panel.scrollbar;
    }

    hits.status_problems = status::render(f, regions.status, app);
    hits.find = active_editor.and_then(|area| find::render(f, area, app));
    hits.quick_diff = quick_diff::render(f, app, &hits.groups);
    hits.hover = hover::render(f, app, &hits.groups);
    hits.completion = completion::render(f, app, &hits.groups);
    hits.palette = palette::render(f, area, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}
