//! ratatui widgets. Colours come from the theme only.

mod activity;
mod banner;
mod cheatsheet;
mod code_action;
mod completion;
mod context_menu;
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
mod semantic;
mod side;
mod splash;
mod status;
mod tab_tip;
mod tabs;
mod task;
mod tools;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::diff_view::DiffTab;
use crate::edit::{RenderWhitespace, WordWrap};
use crate::git::HunkKind;
use crate::group::Tab;
use crate::layout::{self, GroupHit, HitMap, ScrollTarget, SplitDir};
use crate::scrollbar::Scrollable;
use crate::view::{TextScroll, View};

/// The screen row (0-based from the editor's first text row) `view` draws `line`'s `sub` row on,
/// if it is on screen: the wrapped rows of the lines from the view's top down to `line`, less the
/// rows of the top line scrolled past. Only the visible lines are laid out, so a line below the
/// view costs at most the screen's height.
fn row_screen_row(
    view: &mut View,
    rope: &ropey::Rope,
    revision: u64,
    line: usize,
    sub: usize,
    tab_width: usize,
) -> Option<usize> {
    let (width, height, top, top_row) = (view.width, view.height, view.top_line(), view.top_row());
    if line < top {
        return None;
    }
    // `row` is the full rows of the lines from `top` down to `line`, so `(line, sub)` is at
    // screen row `row + sub - top_row` (on screen when `top_row <= row + sub < height + top_row`);
    // stop as soon as a line's first row reaches the bottom edge, so only the visible lines are
    // laid out.
    let mut row = 0;
    let mut at = top;
    while at < line && row < height + top_row {
        row += view.wrap.layout(rope, revision, at, width, tab_width).len();
        at += 1;
    }
    if at != line {
        return None;
    }
    let screen = row + sub;
    (screen >= top_row && screen < height + top_row).then(|| screen - top_row)
}

/// The screen row of `line`'s first row (`row_screen_row`).
fn line_screen_row(view: &mut View, rope: &ropey::Rope, revision: u64, line: usize, tab_width: usize) -> Option<usize> {
    row_screen_row(view, rope, revision, line, 0, tab_width)
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 4 || area.width < 30 {
        return;
    }
    let panel_height = app.ui.panel_visible.then_some(app.ui.panel_height);
    let terminal_height = app.ui.terminal_visible.then_some(app.ui.terminal_height);
    let regions = layout::compute(
        area,
        app.ui.side_visible,
        app.ui.side_width,
        panel_height,
        terminal_height,
    );
    let terminal_strip = (!app.ui.terminal_visible)
        .then(|| {
            layout::compute(
                area,
                app.ui.side_visible,
                app.ui.side_width,
                panel_height,
                Some(app.ui.terminal_height),
            )
            .terminal
        })
        .flatten();
    let mut hits = HitMap {
        regions,
        terminal_strip,
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

    let (mut cells, mut dividers) = app.groups.tree.layout(regions.editor);
    fit_commit_tabs(app, &cells);
    // The terminal area's groups are drawn by the same loop, after the editor's.
    if let Some(t) = regions.terminal {
        // Its top row is the border, drawn here; the groups get the rest.
        let border = app.theme.get("editor").patch(app.theme.get("panel.border"));
        let out = f.buffer_mut();
        for x in t.x..t.x + t.width {
            out.set_stringn(x, t.y, "─", 1, border);
        }
        let rest = Rect {
            y: t.y + 1,
            height: t.height - 1,
            ..t
        };
        let (c, d) = app.terminals.tree.layout(rest);
        cells.extend(c);
        dividers.extend(d.into_iter().map(|mut d| {
            d.terminal = true;
            d
        }));
    }
    // Each group's number in reading order, for the corner of its tab bar (two or more groups).
    let group_ids = (app.groups.len() >= 2).then(|| app.groups.ids());
    let terminal_ids = (app.terminals.len() >= 2).then(|| app.terminals.ids());
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
        let terminal = crate::group::is_terminal_group(id);
        let is_active = id
            == if terminal {
                app.terminals.active
            } else {
                app.groups.active
            };
        let look = scrollbar::look(app, ScrollTarget::Group(id));
        let editor_bg = app.theme.get("editor");
        let drop = app.tab_drop_in(id);
        let number = (if terminal { &terminal_ids } else { &group_ids })
            .as_ref()
            .and_then(|ids| ids.iter().position(|&g| g == id))
            .map(|i| (i as u8).saturating_add(1));
        // Long lines wrap at the editor's width (the effective `word_wrap` setting).
        let wrapping = app.word_wrap() == WordWrap::On;
        let Some(group) = (if terminal {
            app.terminals.get_mut(id)
        } else {
            app.groups.get_mut(id)
        }) else {
            continue;
        };
        let tabs = tabs::render(f, tab_bar, group, &app.buffers, &app.theme, is_active, drop, number);

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

        // A text, diff, hex or shell tab keeps the editor's last column for its scroll bar.
        let mut bar = if group.tabs.is_empty() {
            None
        } else {
            scrollbar::take_column(&mut editor_area)
        };
        // A text or diff tab keeps a second column, one cell left of the bar, for its changed
        // lines' marks (always, so nothing reflows when they appear or go); a hex tab has none,
        // and single file mode has no git status to mark.
        let mut marks_col = if !app.mode.is_single() && (group.active_tab().is_some() || group.active_diff().is_some())
        {
            scrollbar::take_column(&mut editor_area)
        } else {
            None
        };
        // A text or diff tab whose widest line drawn so far is wider than it (or that is
        // scrolled sideways) keeps the last row for the horizontal bar, from the last frame's
        // sizes; below, a frame that finds that changed asks for another.
        let reserve = bar.is_some()
            && (group
                .active_tab_mut()
                .is_some_and(|t| !wrapping && t.view.scroll_width.wants_bar(t.view.width, t.view.left_col))
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
        if hrow.is_some()
            && let Some(m) = &mut marks_col
        {
            m.height -= 1;
        }
        // The sideways scroll's state as drawn, and the cells its bar may take.
        let mut hscroll = None;
        let mut text = Rect::default();
        let mut diff_hit = None;
        let mut scroll_hit = None;
        let mut marks_hit = None;
        let mut merge_buttons = Vec::new();
        // Only the focused group shows the terminal cursor.
        let focused = is_active
            && app.ui.focus == if terminal { Focus::Terminal } else { Focus::Editor }
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
                let scroll = d.scroll();
                // The header row's cell, then the rows' track.
                let out = f.buffer_mut();
                out.set_style(bar, editor_bg);
                out.set_style(Rect { height: 1, ..bar }, app.theme.get("diffEditor.header"));
                let track = Rect {
                    y: bar.y + 1,
                    height: bar.height.saturating_sub(1),
                    ..bar
                };
                if let Some(m) = marks_col {
                    // The header's cell, then the hunks' rows on the bar's track.
                    out.set_style(m, editor_bg);
                    out.set_style(Rect { height: 1, ..m }, app.theme.get("diffEditor.header"));
                    let mtrack = Rect {
                        y: m.y + 1,
                        height: m.height.saturating_sub(1),
                        ..m
                    };
                    let marks = diff_marks(d);
                    scrollbar::render_marks(out, mtrack, scroll, editor_bg, &app.theme, &marks);
                    marks_hit = Some(mtrack);
                }
                scroll_hit = Some(scrollbar::render(out, track, scroll, look, editor_bg, &app.theme));
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
        // A shell tab: its PTY resized to the whole area each frame, the screen drawn there,
        // and its scroll bar in the last column.
        if let Some(shell) = group.active_shell() {
            shell.run.resize(editor_area.height, editor_area.width);
            text = editor_area;
            task::render_shell(f, editor_area, shell, &app.theme, focused);
            if let Some(bar) = bar {
                let base = app.theme.get("editor").patch(app.theme.get("terminal.background"));
                scroll_hit = Some(scrollbar::render(
                    f.buffer_mut(),
                    bar,
                    shell.run.scrollable().scroll(),
                    look,
                    base,
                    &app.theme,
                ));
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
                    // A log or a commit is read, not edited: no whitespace glyphs (an untitled buffer is edited).
                    whitespace: if !buf.has_file() && !buf.is_untitled() {
                        RenderWhitespace::None
                    } else {
                        whitespace
                    },
                    blame: app.blame.on,
                    wrapping,
                };
                let (drawn, buttons) =
                    editor::render(f, editor_area, buf, &mut tab.view, &app.theme, &app.settings, overlay);
                text = drawn;
                merge_buttons = buttons;
                if let Some((g, line)) = bulb
                    && g == id
                    && text.x >= editor_area.x + 2
                    && let Some(row) = if wrapping {
                        let tab_width = app.settings.for_buffer(buf).tab_width;
                        line_screen_row(&mut tab.view, &buf.rope, buf.revision(), line, tab_width)
                    } else {
                        line.checked_sub(tab.view.top_line())
                    }
                    && row < text.height as usize
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
                hscroll = (!wrapping).then(|| {
                    (
                        tab.view.scroll_width,
                        tab.view.width,
                        tab.view.left_col,
                        (text.width > 0).then_some(text),
                    )
                });
                if let Some(bar) = bar {
                    let scroll = TextScroll {
                        view: &mut tab.view,
                        rope: &buf.rope,
                    }
                    .scroll();
                    // Every change's lines, staged or not (the gutter shows the unstaged ones);
                    // while wrapping, the bar's marks are rows.
                    let (revision, width, tab_width) = (tab.view.revision, tab.view.width, tab.view.tab_width);
                    let marks: Vec<scrollbar::Mark> = buf
                        .quick_diff
                        .marks()
                        .iter()
                        .filter_map(|m| {
                            let lines = buf.quick_diff.lines_of(&buf.rope, m)?;
                            let lines = if wrapping {
                                let start =
                                    tab.view
                                        .wrap
                                        .row_of_line(&buf.rope, revision, lines.start, width, tab_width);
                                let end = tab
                                    .view
                                    .wrap
                                    .row_of_line(&buf.rope, revision, lines.end, width, tab_width);
                                start..end
                            } else {
                                lines
                            };
                            Some(scrollbar::Mark {
                                lines,
                                key: m.theme_key(),
                            })
                        })
                        .collect();
                    if let Some(m) = marks_col {
                        let out = f.buffer_mut();
                        scrollbar::render_marks(out, m, scroll, editor_bg, &app.theme, &marks);
                        marks_hit = Some(m);
                    }
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
                width: row.width + u16::from(bar.is_some()) + u16::from(marks_col.is_some()),
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
        if is_active && !terminal {
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
            marks: marks_hit,
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
        .chain(app.terminal_strip_shade())
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
    tab_tip::render(f, app, &hits.groups);
    hits.completion = completion::render(f, app, &hits.groups);
    hits.code_actions = code_action::render(f, app, &hits.groups);
    hits.context_menu = context_menu::render(f, app);
    hits.palette = palette::render(f, area, app);
    hits.cheat_sheet = cheatsheet::render(f, regions.editor, app);
    hits.keys = keys::render(f, regions.editor, app);
    if let Some(prompt) = &app.prompt {
        hits.prompt_input = dialog::render(f, area, app, prompt);
    }
    app.hits = hits;
}

/// A diff tab's hunks as the bar's marks: each hunk's rows in the layout drawn, in the gutter
/// colour of its kind (`Hunk::kind`; no staged split: the tab is one side).
fn diff_marks(d: &DiffTab) -> Vec<scrollbar::Mark> {
    let starts = &d.aligned().starts;
    d.hunks()
        .iter()
        .enumerate()
        .filter_map(|(k, h)| {
            let start = *starts.get(k)?;
            let rows = if d.inline {
                h.old.len() + h.new.len()
            } else {
                h.old.len().max(h.new.len())
            };
            let key = match h.kind() {
                HunkKind::Added => "editorGutter.addedBackground",
                HunkKind::Deleted => "editorGutter.deletedBackground",
                HunkKind::Modified => "editorGutter.modifiedBackground",
            };
            Some(scrollbar::Mark {
                lines: start..start + rows,
                key,
            })
        })
        .collect()
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
            // The scroll bar's column and the marks column beside it.
            scrollbar::take_column(&mut area);
            scrollbar::take_column(&mut area);
            Some((id, editor::text_width(area.width, &buf.rope)))
        })
        .collect();
    if !widths.is_empty() {
        app.fit_commit_tabs(&widths);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line's first screen row: the wrapped rows of the lines from the view's top down to it,
    /// less the top line's rows scrolled past; none for a line above the view or below a screen
    /// too short to reach it.
    #[test]
    fn line_screen_row_follows_the_wrapped_top() {
        let rope = ropey::Rope::from_str("aaaa bbbb cccc\ncc\ndddd eeee\n");
        // Line 0 has 3 rows at width 5, line 1 one, line 2 two.
        let mut view = View::new();
        (view.width, view.height) = (5, 5);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 0, 4), Some(0));
        assert_eq!(line_screen_row(&mut view, &rope, 0, 1, 4), Some(3));
        assert_eq!(line_screen_row(&mut view, &rope, 0, 2, 4), Some(4));

        // The top line two rows down: its first row is off screen.
        view.set_top(&rope, 0, 2);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 0, 4), None);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 1, 4), Some(1));
        assert_eq!(line_screen_row(&mut view, &rope, 0, 2, 4), Some(2));

        // A line above the view, and ones the screen is too short to reach.
        view.set_top(&rope, 1, 0);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 0, 4), None);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 2, 4), Some(1));
        view.height = 2;
        assert_eq!(line_screen_row(&mut view, &rope, 0, 2, 4), Some(1));
        assert_eq!(line_screen_row(&mut view, &rope, 0, 3, 4), None);
    }

    /// With the top line scrolled down, a line whose first row is in the screen's last rows is
    /// still reported (the count is compared after subtracting `top_row`, not before), and the
    /// walk keeps going past a line whose full row count alone passes the bottom.
    #[test]
    fn line_screen_row_counts_scrolled_rows_for_lines_near_the_bottom() {
        // Line 0 is six rows at width 5, lines 1..3 one each.
        let rope = ropey::Rope::from_str("aaaa bbbb cccc dddd eeee ffff\nxx\nyy\nzz\n");
        let mut view = View::new();
        (view.width, view.height) = (5, 5);
        // The top line three rows down: its rows 3..6 fill the screen's first three rows, so
        // line 1's first row is row 3 and line 2's is row 4 — both on screen, even though their
        // full row counts (6 and 7) are at or past the height.
        view.set_top(&rope, 0, 3);
        assert_eq!(line_screen_row(&mut view, &rope, 0, 1, 4), Some(3));
        assert_eq!(line_screen_row(&mut view, &rope, 0, 2, 4), Some(4));
        // Line 3 would be row 5, past the screen's last row (4).
        assert_eq!(line_screen_row(&mut view, &rope, 0, 3, 4), None);
    }
}
