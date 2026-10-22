//! The Tools view, in the side panel below its title: one row per tool (`tools_view::rows`) —
//! a language server's state mark (`○` starting, `●` running, `✗` failed, `–` exited, in its
//! `tools.*` colour), name, then dimmed its languages and state (its progress while it has
//! some) and its root when not the workspace's; a task run's mark (`task_mark`: `●` / `✓` /
//! `✗`), name and dimmed status; "jed" — and, at the right edge, the count of error lines in the
//! row's log in a `tools.errorBadge`. Only the rows on screen are drawn.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::{App, Focus};
use crate::layout::{ScrollBarHit, ScrollTarget, SidePanel, ToolsHit};
use crate::scrollbar::Scrollable;
use crate::tools_view::{self, Row, ServerState};

/// Draws the view in `area` (the side panel below its title, without its border), and the
/// list's scroll bar in its last column. Sets the list's height, keeps its selection on its
/// key (`settle`) and notes the error counts drawn.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> (Option<ToolsHit>, Option<ScrollBarHit>) {
    if area.width < 8 || area.height < 1 {
        return (None, None);
    }
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::Tools));
    let errors = app.logs.error_counts();
    let rows = tools_view::rows(&app.lsp, app.tasks.runs(), &app.grammars, &errors, &app.workspace.root);
    app.tools_view.errors_drawn = errors;

    let theme = &app.theme;
    let base = theme.get("sideBar");
    let out = f.buffer_mut();
    let mut list = area;
    let bar = scrollbar::take_column(&mut list);
    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };
    let view = &mut app.tools_view;
    view.set_height(&rows, list.height as usize);
    view.settle(&rows);
    view.clamp(&rows);
    let top = view.scroll(&rows);
    let selected = view.selected_row(&rows);
    let scroll = view.scrollable(rows.len()).scroll();
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme));
    let mut hit = ToolsHit { list, rows: Vec::new() };
    for (i, row) in rows.iter().enumerate().skip(top).take(list.height as usize) {
        let y = list.y + (i - top) as u16;
        let style = if selected == Some(i) {
            let s = base.patch(selected_style);
            out.set_style(Rect { y, height: 1, ..list }, s);
            s
        } else {
            base
        };
        let dim = style.patch(theme.get("sideBar.dim"));
        let (mark, mark_key, name, detail) = match row {
            Row::Server(s) => {
                let state = match (&s.progress, s.state) {
                    (Some(p), ServerState::Running) => p.clone(),
                    _ => s.state.label().to_string(),
                };
                let mut detail = if s.languages.is_empty() {
                    state
                } else {
                    format!("{} · {state}", s.languages)
                };
                if let Some(root) = &s.root {
                    detail.push_str(&format!(" · {}", root.display()));
                }
                (
                    s.state.glyph().to_string(),
                    s.state.theme_key(),
                    s.name.as_str(),
                    detail,
                )
            }
            Row::Task { name, status, .. } => {
                let (mark, key) = super::panel::task_mark(status);
                let glyph = mark.chars().next().map(String::from).unwrap_or_default();
                (
                    glyph,
                    key,
                    name.as_str(),
                    format!("task · {}", tools_view::task_label(status)),
                )
            }
            Row::Grammar { id, state, .. } => (
                state.glyph().to_string(),
                state.theme_key(),
                id.as_str(),
                format!("syntax plugin · {}", state.label()),
            ),
            Row::Jed { .. } => ("≡".to_string(), "sideBar.dim", "jed", "jed's own log".to_string()),
        };
        // The error badge at the right edge.
        let mut right = list.x + list.width;
        if row.errors() > 0 {
            let badge = format!(" {} ", row.errors());
            let bx = right.saturating_sub(badge.width() as u16 + 1);
            if bx > list.x + 4 {
                out.set_stringn(bx, y, &badge, badge.width(), style.patch(theme.get("tools.errorBadge")));
                right = bx;
            }
        }
        let room = right.saturating_sub(list.x + 1) as usize;
        out.set_stringn(list.x + 1, y, &mark, room, style.patch(theme.get(mark_key)));
        let room = room.saturating_sub(2);
        let (end, _) = out.set_stringn(list.x + 3, y, name, room, style);
        let used = (end - (list.x + 3)) as usize;
        if used + 2 < room {
            out.set_stringn(end + 2, y, &detail, room - used - 2, dim);
        }
        hit.rows.push(row.key());
    }
    (Some(hit), bar)
}
