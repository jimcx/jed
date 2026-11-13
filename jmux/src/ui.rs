//! Drawing the server's `App` into a frame: each group's tab bar and terminal, the dividers between
//! groups, above a one-line status
//! bar (or the close question). Writes the `HitMap`.

use jtui::borders::Segment;
use jtui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;
use crate::hit::{GroupHit, HitMap};
use crate::tabbar;

/// Rows the status bar takes.
pub const STATUS_ROWS: u16 = 1;
/// Rows a group's tab bar takes.
pub const TAB_ROWS: u16 = 1;

/// The area of the groups in a `rows` × `cols` screen: all but the status bar.
pub fn body(rows: u16, cols: u16) -> Rect {
    Rect::new(0, 0, cols, rows.saturating_sub(STATUS_ROWS))
}

/// A group's terminal: its area below the tab bar.
pub fn term_area(group: Rect) -> Rect {
    let bar = TAB_ROWS.min(group.height);
    Rect::new(group.x, group.y + bar, group.width, group.height - bar)
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let (cells, dividers) = app.groups.layout(body(area.height, area.width));
    let mut hit = HitMap::default();
    for (id, rect) in cells {
        let term = term_area(rect);
        let bar = Rect::new(rect.x, rect.y, rect.width, rect.height - term.height);
        let focused = id == app.groups.active;
        let Some(group) = app.groups.get_mut(id) else {
            continue;
        };
        let (tabs, arrows) = tabbar::render(f, bar, group, &app.theme, focused);
        if let Some(shell) = group.active_tab() {
            // The run follows the area every frame (a no-op while the size is the same); hidden
            // tabs keep their last size.
            shell.run.resize(term.height, term.width);
            jterm::draw::render_shell(f, term, shell, &app.theme, focused && app.confirm.is_none(), None);
        }
        hit.groups.push(GroupHit {
            id,
            bar,
            term,
            tabs,
            arrows,
        });
    }
    // The dividers' lines, joined where they cross.
    let segments = dividers
        .iter()
        .map(|d| Segment::new(d.rect, d.dir, None, "editorGroup.border"))
        .collect();
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
        } else {
            let title = app.groups.active_group().active_tab().map_or("", |s| s.title.as_str());
            status_bar(f, bar, &app.name, title, &app.detach_hint(), &app.theme);
        }
    }
}

/// ` <session>  <title>` on the left, the detach hint on the right.
fn status_bar(f: &mut Frame, bar: Rect, name: &str, title: &str, hint: &str, theme: &Theme) {
    let (style, detail) = (theme.get("statusBar"), theme.get("statusBar.detail"));
    let buf = f.buffer_mut();
    buf.set_style(bar, style);
    let width = bar.width as usize;
    let hint = format!("{hint} ");
    let left = format!(" {name}");
    let (x, _) = buf.set_stringn(bar.x, bar.y, &left, width, style);
    let right_x = bar.x + bar.width.saturating_sub(hint.len() as u16);
    if right_x > x + 2 {
        buf.set_stringn(right_x, bar.y, &hint, width, detail);
        let room = (right_x - x).saturating_sub(3) as usize;
        buf.set_stringn(x + 2, bar.y, title, room, detail);
    }
}
