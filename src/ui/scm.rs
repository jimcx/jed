//! The Source Control view, in the side panel below its title: the branch (`Status::label`),
//! then VS Code's sections — "Staged Changes" (only with files) and "Changes" — each a header
//! row (`▾` / `▸`, title, the count in a badge at the right) and its files: the name in its
//! change's `gitDecoration.*` colour (struck through when deleted), the folder dimmed, the
//! change's letter at the right edge. The hovered and the selected row show their button left
//! of the letter / badge: `+` stage, `−` unstage (a header's: the whole section). No
//! repository, still reading, or nothing changed: a line of text instead. Only the rows on
//! screen are drawn.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::{App, Focus};
use crate::git::Change;
use crate::layout::{ScmHit, ScrollBarHit, ScrollTarget, SidePanel};
use crate::scm::{Row, Section};
use crate::scrollbar::Scrollable;

/// Draws the view in `area` (the side panel below its title, without its border), and the
/// list's scroll bar in its last column.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> (Option<ScmHit>, Option<ScrollBarHit>) {
    if area.width < 8 || area.height < 2 {
        return (None, None);
    }
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::SourceControl));
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let dim = base.patch(theme.get("sideBar.dim"));
    let header = base.patch(theme.get("sideBar.header"));
    let out = f.buffer_mut();
    let width = area.width as usize;

    let Some(status) = app.git.status.as_ref() else {
        let text = if app.git.running() {
            "Reading the git status…"
        } else {
            "The folder currently open doesn't have a git repository."
        };
        wrap(out, area, text, dim);
        return (None, None);
    };
    out.set_stringn(area.x, area.y, format!(" ⎇ {}", status.label()), width, header);
    let list = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    if status.entries.is_empty() {
        wrap(out, list, "No changes in the working tree.", dim);
        let hit = ScmHit {
            list,
            ..ScmHit::default()
        };
        return (Some(hit), None);
    }
    let mut list = list;
    let bar = scrollbar::take_column(&mut list);

    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };
    let view = &mut app.scm;
    view.height = list.height as usize;
    let rows = view.rows(status);
    view.clamp(&rows);
    let top = view.scroll(&rows);
    let selected = view.selected_row(&rows);
    let scroll = view.scrollable(&rows).scroll();
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme));
    let view = &app.scm;
    let mut hit = ScmHit {
        list,
        ..ScmHit::default()
    };
    for (i, row) in rows.iter().enumerate().skip(top).take(list.height as usize) {
        let y = list.y + (i - top) as u16;
        let is_selected = selected == Some(i);
        let style = if is_selected { base.patch(selected_style) } else { base };
        if is_selected {
            out.set_style(Rect { y, height: 1, ..list }, style);
        }
        let key = row.key();
        let buttons = is_selected || view.hover.as_ref() == Some(&key);
        let unstage = row.section() == Section::Staged;
        let button = if unstage { "−" } else { "+" };
        let action = style.patch(theme.get("scm.action"));
        // The right edge: a header's count badge, a file's letter; the button left of it.
        let (right, label) = match row {
            Row::Header {
                section,
                count,
                collapsed,
            } => {
                let badge = format!(" {count} ");
                let bx = list.x + list.width.saturating_sub(badge.width() as u16 + 1);
                out.set_stringn(bx, y, &badge, badge.width(), style.patch(theme.get("scm.count")));
                let marker = if *collapsed { "▸" } else { "▾" };
                let text = format!(" {marker} {}", section.title());
                (bx, (text, style.patch(theme.get("sideBar.header")), None))
            }
            Row::File { entry, change, .. } => {
                let d = change.decoration();
                let colour = style.patch(theme.get(d.theme_key()));
                let lx = list.x + list.width.saturating_sub(2);
                out.set_stringn(lx, y, d.letter().to_string(), 1, colour);
                let name = entry
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let dir = entry.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                let name_style = if *change == Change::Deleted {
                    colour.patch(theme.get("scm.deleted"))
                } else {
                    colour
                };
                (lx, (format!("    {name}"), name_style, Some(dir)))
            }
        };
        let show_button = buttons && !matches!(row, Row::Header { count: 0, .. });
        let mut room = right.saturating_sub(list.x + 1) as usize;
        if show_button && right >= list.x + 3 {
            let r = Rect {
                x: right - 2,
                y,
                width: 1,
                height: 1,
            };
            out.set_stringn(r.x, y, button, 1, action);
            hit.buttons.push((r, key.clone(), unstage));
            room = room.saturating_sub(2);
        }
        let (text, text_style, dir) = label;
        let (end, _) = out.set_stringn(list.x, y, &text, room, text_style);
        if let Some(dir) = dir.filter(|d| !d.is_empty()) {
            let used = (end - list.x) as usize;
            if used + 2 < room {
                let dir_style = style.patch(theme.get("sideBar.dim"));
                out.set_stringn(end + 1, y, &dir, room - used - 1, dir_style);
            }
        }
        hit.rows.push(key);
    }
    (Some(hit), bar)
}

/// `text` word-wrapped from the second column of `area`'s rows.
fn wrap(out: &mut Buffer, area: Rect, text: &str, style: Style) {
    let width = area.width.saturating_sub(2) as usize;
    let mut line = String::new();
    let mut y = area.y;
    let bottom = area.y + area.height;
    for word in text.split(' ') {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            if y < bottom {
                out.set_stringn(area.x + 1, y, &line, width, style);
            }
            y += 1;
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if y < bottom && !line.is_empty() {
        out.set_stringn(area.x + 1, y, &line, width, style);
    }
}
