//! Drawing the server's `App` into a frame: the commands side bar (`ui_side`) when it shows, each
//! group's tab bar and terminal, the dividers between groups, above a one-line status
//! bar (or the close question). Writes the `HitMap`.

use jtui::borders::Segment;
use jtui::scrollbar::Scrollable;
use jtui::scrollbar_draw::{self, Look, ScrollBarHit};
use jtui::split::SplitDir;
use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Drag};
use crate::hit::{GroupHit, HitMap};
use crate::side::SideBar;
use crate::tab_drag::{self, TabDrop};
use crate::tabbar;
use crate::ui_side;

/// Rows the status bar takes.
pub const STATUS_ROWS: u16 = 1;
/// Rows a group's tab bar takes.
pub const TAB_ROWS: u16 = 1;

/// The area of the groups in a `rows` × `cols` screen: all but the status bar and the side bar
/// (with its border column) when it shows.
pub fn body(rows: u16, cols: u16, side: &SideBar) -> Rect {
    let taken = side.reserved(cols).min(cols);
    Rect::new(taken, 0, cols - taken, rows.saturating_sub(STATUS_ROWS))
}

/// The side bar's area (without its border column), none while it is hidden.
pub fn side_area(rows: u16, cols: u16, side: &SideBar) -> Option<Rect> {
    side.visible
        .then(|| Rect::new(0, 0, side.width_in(cols), rows.saturating_sub(STATUS_ROWS)))
}

/// A group's terminal: its area below the tab bar.
pub fn term_area(group: Rect) -> Rect {
    let bar = TAB_ROWS.min(group.height);
    Rect::new(group.x, group.y + bar, group.width, group.height - bar)
}

/// A group's terminal split into the shell's cells and the one-column track of its scroll bar
/// (none when the area is narrower than two columns, as jed's `take_column`).
pub fn shell_area(term: Rect) -> (Rect, Option<Rect>) {
    let mut text = term;
    let track = scrollbar_draw::take_column(&mut text);
    (text, track)
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let (cells, dividers) = app.groups.layout(body(area.height, area.width, &app.side));
    let mut hit = HitMap::default();
    let target = match &app.drag {
        Some(Drag::Tab(t)) => t.target,
        _ => None,
    };
    let scrolling = match &app.drag {
        Some(Drag::ScrollBar { group, .. }) => Some(*group),
        _ => None,
    };
    for (id, rect) in cells {
        let term = term_area(rect);
        let bar = Rect::new(rect.x, rect.y, rect.width, rect.height - term.height);
        let focused = id == app.groups.active;
        let Some(group) = app.groups.get_mut(id) else {
            continue;
        };
        let drop = match target {
            Some(TabDrop::TabBar { group, index }) if group == id => Some(index),
            _ => None,
        };
        let (tabs, arrows) = tabbar::render(f, bar, group, &app.theme, focused, drop);
        let (text, track) = shell_area(term);
        let mut scroll = ScrollBarHit::default();
        if let Some(shell) = group.active_tab() {
            // The run follows the area every frame (a no-op while the size is the same); hidden
            // tabs keep their last size.
            shell.run.resize(text.height, text.width);
            let search = app.search.as_ref().filter(|s| s.run == shell.run.id);
            jterm::draw::render_shell(
                f,
                text,
                shell,
                &app.theme,
                focused && app.confirm.is_none() && !app.side.focused,
                search,
            );
            if let Some(track) = track {
                let base = app.theme.get("editor").patch(app.theme.get("terminal.background"));
                let look = if scrolling == Some(id) {
                    Look::Active
                } else {
                    Look::Normal
                };
                let bar = shell.run.scrollable().scroll();
                scroll = scrollbar_draw::render(f.buffer_mut(), track, bar, look, base, &app.theme);
            }
        }
        if let Some(TabDrop::Terminal { group, side }) = target
            && group == id
        {
            // The zone a drop would take, over what the shell drew.
            let zone = tab_drag::shade_rect(term, side).intersection(f.area());
            f.buffer_mut().set_style(zone, app.theme.get("terminal.dropBackground"));
        }
        hit.groups.push(GroupHit {
            id,
            bar,
            term,
            text,
            scroll,
            tabs,
            arrows,
        });
    }
    // The dividers' lines, joined where they cross, and the side bar's border.
    let mut segments: Vec<Segment> = dividers
        .iter()
        .map(|d| Segment::new(d.rect, d.dir, None, "editorGroup.border"))
        .collect();
    if let Some(side) = side_area(area.height, area.width, &app.side) {
        let edge = Rect::new(side.x + side.width, side.y, 1, side.height);
        segments.push(Segment::new(edge, SplitDir::Right, Some("sideBar"), "sideBar.border"));
        hit.side = Some(ui_side::render(f, app, side, edge));
    }
    app.borders.apply(f, segments, &app.theme);
    hit.dividers = dividers;
    app.hit = hit;
    if area.height > 0 {
        let bar = Rect::new(0, area.height - STATUS_ROWS, area.width, STATUS_ROWS);
        if let Some(confirm) = &app.confirm {
            let text = format!(" Close terminal? '{}' is running  [y] close  [n] cancel", confirm.name);
            let style = app.theme.get("statusBar.prompt");
            f.buffer_mut().set_style(bar, style);
            f.buffer_mut()
                .set_stringn(bar.x, bar.y, &text, bar.width as usize, style);
        } else if let Some(notice) = &app.notice {
            let style = app.theme.get("statusBar.notice");
            f.buffer_mut().set_style(bar, style);
            f.buffer_mut()
                .set_stringn(bar.x, bar.y, format!(" {notice}"), bar.width as usize, style);
        } else {
            let title = app.groups.active_group().active_tab().map_or("", |s| s.title.as_str());
            status_bar(f, bar, &app.name, app.counts(), title, &app.detach_hint(), &app.theme);
        }
    }
}

/// ` <session>  3 terminals · 1 running  <title>` on the left, the detach hint on the right. What
/// doesn't fit goes: the title first, then the counts.
fn status_bar(f: &mut Frame, bar: Rect, name: &str, counts: (usize, usize), title: &str, hint: &str, theme: &Theme) {
    let (style, detail) = (theme.get("statusBar"), theme.get("statusBar.detail"));
    let buf = f.buffer_mut();
    buf.set_style(bar, style);
    let width = bar.width as usize;
    let hint = format!("{hint} ");
    let (terminals, running) = counts;
    let mut summary = format!("{terminals} terminal{}", if terminals == 1 { "" } else { "s" });
    if running > 0 {
        summary.push_str(&format!(" · {running} running"));
    }
    let left = format!(" {name}");
    let (x, _) = buf.set_stringn(bar.x, bar.y, &left, width, style);
    let right_x = bar.x + bar.width.saturating_sub(hint.len() as u16);
    if right_x > x + 2 {
        buf.set_stringn(right_x, bar.y, &hint, width, detail);
        let room = (right_x - x).saturating_sub(3) as usize;
        let (x, _) = buf.set_stringn(x + 2, bar.y, &summary, room, detail);
        let room = right_x.saturating_sub(x).saturating_sub(3) as usize;
        if room > 0 && summary.chars().count() <= (right_x - bar.x) as usize {
            buf.set_stringn(x + 2, bar.y, title, room, detail);
        }
    }
}

#[cfg(test)]
mod status_bar_tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn bar(width: u16, counts: (usize, usize), title: &str) -> String {
        let theme = crate::theme::dark_plus(true);
        let mut t = Terminal::new(TestBackend::new(width, 1)).unwrap();
        t.draw(|f| status_bar(f, f.area(), "work", counts, title, "Ctrl+Shift+D detach", &theme))
            .unwrap();
        t.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn counts_pluralise_and_running_shows_only_when_positive() {
        let wide = bar(100, (1, 0), "vim");
        assert!(
            wide.contains("work") && wide.contains("1 terminal ") && wide.contains("vim"),
            "{wide}"
        );
        assert!(
            !wide.contains("running") && wide.ends_with("Ctrl+Shift+D detach "),
            "{wide}"
        );
        let wide = bar(100, (3, 2), "");
        assert!(wide.contains("3 terminals · 2 running"), "{wide}");
    }

    #[test]
    fn a_narrow_bar_truncates_the_title_before_the_counts() {
        let roomy = bar(50, (3, 0), "a-very-long-title-here");
        assert!(
            roomy.contains("3 terminals  a-very") && !roomy.contains("title-here"),
            "{roomy}"
        );
        let narrow = bar(45, (3, 0), "a-very-long-title-here");
        assert!(narrow.contains("3 terminals") && !narrow.contains("a-very"), "{narrow}");
        assert!(narrow.contains("detach"), "{narrow}");
    }
}
