//! ratatui widgets. Colours come from the theme only.

mod activity;
mod banner;
mod cheatsheet;
mod code_action;
mod completion;
mod dialog;
mod diff;
mod editor;
mod find;
mod hex;
mod hover;
mod keys;
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
        hits.history = side.history;
        hits.history_scrollbar = side.history_scrollbar;
    }

    let (cells, dividers) = app.groups.tree.layout(regions.editor);
    fit_commit_tabs(app, &cells);
    let mut active_editor = None;
    let whitespace = app.render_whitespace();
    // The lightbulb: the active group's cursor line, when code actions exist there.
    let bulb = app.bulb_line();
    app.code_actions.bulb_hit = None;
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
        let drop = app.tab_drop_in(id);
        let Some(group) = app.groups.get_mut(id) else { continue };
        let tabs = tabs::render(f, tab_bar, group, &app.buffers, &app.theme, is_active, drop);

        // The changed-on-disk banner takes the editor's first row (leaving it at least one); a
        // diff tab of the file shows it too.
        let mut banner = None;
        let file = match group.tabs.get(group.active) {
            Some(Tab::Text(t)) => app.buffers.get(t.buffer),
            Some(Tab::Diff(d)) => app.buffers.iter().find(|(_, b)| b.path == d.path).map(|(_, b)| b),
            Some(Tab::Hex(_)) | Some(Tab::Shell(_)) | None => None,
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

        // A text, diff or hex tab keeps the editor's last column for its scroll bar (a shell
        // uses the whole area).
        let mut bar = if group.tabs.is_empty() || group.active_shell().is_some() {
            None
        } else {
            scrollbar::take_column(&mut editor_area)
        };
        // A text or diff tab whose widest line drawn so far is wider than it (or that is
        // scrolled sideways) keeps the last row for the horizontal bar, from the last frame's
        // sizes; below, a frame that finds that changed asks for another.
        let reserve = bar.is_some()
            && (group
                .active_tab_mut()
                .is_some_and(|t| t.view.scroll_width.wants_bar(t.view.width, t.view.left_col))
                || group
                    .active_diff_mut()
                    .is_some_and(|d| d.scroll_width.wants_bar(d.text_width, d.left_col)));
        let hrow = if reserve {
            scrollbar::take_row(&mut editor_area)
        } else {
            None
        };
        if hrow.is_some()
            && let Some(b) = &mut bar
        {
            b.height -= 1;
        }
        // The sideways scroll's state as drawn, and the cells its bar may take.
        let mut hscroll = None;
        let mut text = Rect::default();
        let mut diff_hit = None;
        let mut scroll_hit = None;
        let mut merge_buttons = Vec::new();
        // Only the focused group shows the terminal cursor.
        let focused = is_active
            && app.ui.focus == Focus::Editor
            && app.prompt.is_none()
            && app.palette.is_none()
            && app.cheat_sheet.is_none()
            && app.keys.is_none();
        if let Some(d) = group.active_diff_mut() {
            diff_hit = Some(diff::render(
                f,
                editor_area,
                d,
                &app.theme,
                app.settings.tab_width,
                focused,
            ));
            // As drawn: `diff::render` sets `text_width` and measures the rows it drew.
            hscroll = Some((d.scroll_width, d.text_width, d.left_col, None));
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
        let mut hex_drawn = false;
        let shell_drawn = group.active_shell().is_some();
        if let Some(h) = group.active_hex_mut() {
            hex_drawn = true;
            // Find highlights in the active group, when the widget's query is for this tab.
            let find = (is_active && app.find.visible && app.find.hex.tab() == Some(h.id))
                .then(|| app.find.hex.query())
                .flatten();
            let overlay = hex::Overlay { focused, find };
            text = hex::render(f, editor_area, h, &app.theme, app.settings.hex_bytes_per_line, overlay);
            if let Some(bar) = bar {
                scroll_hit = Some(scrollbar::render(
                    f.buffer_mut(),
                    bar,
                    h.scroll(),
                    look,
                    editor_bg,
                    &app.theme,
                ));
            }
        }
        // A shell tab: its PTY resized to the whole area each frame, the screen drawn there.
        if let Some(shell) = group.active_shell() {
            shell.run.resize(editor_area.height, editor_area.width);
            text = editor_area;
            task::render_shell(f, editor_area, shell, &app.theme, focused);
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
                    // A log or a commit is read, not edited: no whitespace glyphs (an untitled buffer is edited).
                    whitespace: if !buf.has_file() && !buf.is_untitled() {
                        RenderWhitespace::None
                    } else {
                        whitespace
                    },
                    blame: app.blame.on,
                };
                let (drawn, buttons) =
                    editor::render(f, editor_area, buf, &mut tab.view, &app.theme, &app.settings, overlay);
                text = drawn;
                merge_buttons = buttons;
                if let Some((g, line)) = bulb
                    && g == id
                    && let Some(row) = line.checked_sub(tab.view.top_line())
                    && row < text.height as usize
                    && text.x >= editor_area.x + 2
                {
                    // Over the gutter's first two cells (a space and the widest numbers' first digit).
                    let at = Rect {
                        x: editor_area.x,
                        y: text.y + row as u16,
                        width: 2,
                        height: 1,
                    };
                    let style = app
                        .theme
                        .get("gutter")
                        .patch(app.theme.get("editorLightBulb.foreground"));
                    f.buffer_mut()
                        .set_stringn(at.x, at.y, crate::code_action::BULB, 2, style);
                    app.code_actions.bulb_hit = Some((id, at));
                }
                hscroll = Some((
                    tab.view.scroll_width,
                    tab.view.width,
                    tab.view.left_col,
                    (text.width > 0).then_some(text),
                ));
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
            None if diff_hit.is_some() || hex_drawn || shell_drawn => {}
            None => {
                f.buffer_mut().set_style(editor_area, app.theme.get("editor"));
                splash::render(f, editor_area, &app.theme);
            }
        }
        // A diff's `text_width` and a text tab's measured lines are known only now: the bar's row
        // appearing or going asks for another frame.
        let mut hscroll_hit = None;
        let wanted = hscroll.is_some_and(|(w, width, left, _)| w.wants_bar(width, left));
        // (Against the decision, not the row: an editor too short for it would ask every frame.)
        if wanted != reserve {
            app.needs_redraw = true;
        }
        if let (Some(row), Some((w, width, left, span))) = (hrow, hscroll) {
            let out = f.buffer_mut();
            // The corner where the bars meet (and a text tab's gutter) stays blank.
            let all = Rect {
                width: row.width + u16::from(bar.is_some()),
                ..row
            };
            out.set_style(all, editor_bg);
            let track = span.map_or(row, |t| Rect {
                x: t.x,
                width: t.width,
                ..row
            });
            let hlook = scrollbar::look(app, ScrollTarget::Columns(id));
            let scroll = crate::scrollbar::Scroll::columns(&w, width, left);
            hscroll_hit = Some(scrollbar::render_row(out, track, scroll, hlook, editor_bg, &app.theme));
        }
        if is_active {
            active_editor = Some(editor_area);
        }
        hits.groups.push(GroupHit {
            id,
            tab_bar,
            editor: editor_area,
            text,
            tabs: tabs.tabs,
            tab_arrows: tabs.arrows,
            banner,
            merge_buttons,
            diff: diff_hit,
            scrollbar: scroll_hit,
            hscrollbar: hscroll_hit,
        });
    }
    // The zone a dragged tab would snap to, over what was drawn: the target editor, or the half
    // of it the drop's edge band would split off. `set_style` replaces the cells' own style, so
    // the key sets both colours.
    let shade = app.theme.get("editor.dropBackground");
    let shades: Vec<Rect> = hits
        .groups
        .iter()
        .filter_map(|g| app.tab_drop_shade(g.id, g.editor))
        .collect();
    if !shades.is_empty() {
        let out = f.buffer_mut();
        for rect in shades {
            out.set_style(rect, shade);
        }
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

    let status = status::render(f, regions.status, app);
    (hits.status_problems, hits.status_indentation) = (status.problems, status.indentation);
    hits.status_language = status.language;
    hits.find = active_editor.and_then(|area| find::render(f, area, app));
    hits.quick_diff = quick_diff::render(f, app, &hits.groups);
    hits.hover = hover::render(f, app, &hits.groups);
    hits.completion = completion::render(f, app, &hits.groups);
    hits.code_actions = code_action::render(f, app, &hits.groups);
    hits.palette = palette::render(f, area, app);
    hits.cheat_sheet = cheatsheet::render(f, regions.editor, app);
    hits.keys = keys::render(f, regions.editor, app);
    if let Some(prompt) = &app.prompt {
        dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}

/// Commit tabs' text is `git show` at the width drawn: each one about to be drawn re-renders
/// first if its group's text width changed (the group's area as `render` divides it).
fn fit_commit_tabs(app: &mut App, cells: &[(layout::GroupId, Rect)]) {
    let widths: Vec<_> = cells
        .iter()
        .filter_map(|&(id, rect)| {
            let tab = app.groups.get(id)?.active_tab()?;
            let buf = app.buffers.get(tab.buffer).filter(|b| b.commit.is_some())?;
            let mut area = Rect {
                height: rect.height.saturating_sub(1),
                ..rect
            };
            scrollbar::take_column(&mut area);
            Some((id, editor::text_width(area.width, &buf.rope)))
        })
        .collect();
    if !widths.is_empty() {
        app.fit_commit_tabs(&widths);
    }
}
