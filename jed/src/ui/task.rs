//! A task tab's terminal in the bottom panel: the frame (info line, scroll bar) around the screen
//! `jterm::draw` draws (cells, selection, search, cursor).
//!
//! Render-time state written back: every running task's PTY is resized to the tab's area
//! (`layout::task_area`, its last column kept for the scroll bar `render` draws in
//! `layout::task_bar`), and `app.tasks.size` is set to it. A screen taller than the area (an
//! ended run that started bigger) is drawn from the row that keeps its last line in view.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
use crate::layout::{self, PanelTab, ScrollBarHit, ScrollTarget};
use crate::scrollbar::Scrollable;
use crate::tasks::RunId;

use jterm::draw::draw_screen;
pub use jterm::draw::render_shell;

/// Draws run `id` in the panel `panel` (border row included); `info` is what's left of the tab
/// strip's row: the "scrolled back" note, else the command (and its folder when not the root),
/// dimmed. Returns the run's scroll bar (`None` when the area is too small).
pub fn render(f: &mut Frame, panel: Rect, info: Rect, app: &mut App, id: RunId) -> Option<ScrollBarHit> {
    let content = layout::task_area(panel);
    if content.width == 0 || content.height == 0 {
        return None;
    }
    app.tasks.size = (content.height, content.width);
    for run in app.tasks.runs() {
        if run.status().is_running() {
            run.resize(content.height, content.width);
        }
    }
    let run = app.tasks.get(id)?;
    let focused =
        app.ui.focus == Focus::Panel && app.prompt.is_none() && app.palette.is_none() && app.cheat_sheet.is_none();
    let theme = &app.theme;
    let base = theme.get("panel");
    let below_tabs = Rect {
        y: content.y,
        height: content.height,
        ..panel
    };
    f.buffer_mut()
        .set_style(below_tabs, base.patch(theme.get("terminal.background")));

    let search = app.term_search.as_ref().filter(|s| s.run == id);
    let back = draw_screen(f, content, run, theme, focused, search);
    // The scroll bar in the panel's last column (`task_area` kept it), over the terminal's
    // background.
    let look = super::scrollbar::look(app, ScrollTarget::Panel(PanelTab::Task(id)));
    let bar = super::scrollbar::render(
        f.buffer_mut(),
        layout::task_bar(panel),
        run.scrollable().scroll(),
        look,
        base.patch(theme.get("terminal.background")),
        theme,
    );
    let note = if back > 0 {
        format!("{back} lines back (Shift+PageDown, Ctrl+End)")
    } else {
        let command = run.command.replace(['\n', '\r', '\t'], " ");
        match run.cwd.strip_prefix(&app.workspace.root) {
            Ok(rel) if rel.as_os_str().is_empty() => command,
            Ok(rel) => format!("{command}  (in {})", rel.display()),
            Err(_) => format!("{command}  (in {})", run.cwd.display()),
        }
    };
    if info.width > 4 {
        let w = (note.width() as u16).min(info.width - 4);
        let x = info.x + info.width - w - 1;
        f.buffer_mut()
            .set_stringn(x, info.y, &note, w as usize, base.patch(theme.get("panel.dim")));
    }
    Some(bar)
}
