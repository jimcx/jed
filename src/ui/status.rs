//! Status bar: trust mode, file, problem counts, message (else the cursor line's diagnostic),
//! cursor position, file format, language mode.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Level};
use crate::buffer::pos;
use crate::diagnostics::Severity;
use crate::workspace::trust::Trust;

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let style = app.theme.get("statusBar");
    let out = f.buffer_mut();
    out.set_style(area, style);
    let width = area.width as usize;
    let mut x = area.x;

    if app.trust == Trust::Restricted {
        let (nx, _) = out.set_stringn(x, area.y, " Restricted ", width, app.theme.get("statusBar.restricted"));
        x = nx;
    }

    // Problems in the workspace (VS Code's `⊗ 1 ⚠ 2`), once a language server runs or any
    // diagnostics were published.
    let counts = app.lsp.diagnostics.counts();
    let problems = (!app.lsp.servers().is_empty() || counts != Default::default()).then(|| {
        format!(
            "{} {}  {} {}  ",
            Severity::Error.icon(),
            counts.of(Severity::Error),
            Severity::Warning.icon(),
            counts.of(Severity::Warning)
        )
    });
    let problems = problems.unwrap_or_default();
    // The message to show: a notification, else the diagnostic on the cursor line.
    let mut message = app.message.as_ref().map(|msg| {
        let msg_style = match msg.level {
            Level::Info => style,
            Level::Warning => app.theme.get("statusBar.warning"),
            Level::Error => app.theme.get("statusBar.error"),
        };
        (msg.text.clone(), msg_style)
    });

    let mut right_x = area.x + area.width;
    if let (Some(buf), Some(tab)) = (app.active_buffer(), app.group().active_tab()) {
        if message.is_none() {
            let head = tab.view.selections.primary().head;
            let line = pos::line_of(&buf.rope, head);
            let (start, end, _) = pos::line_bounds(&buf.rope, line);
            message = buf.diagnostics.for_line(start, end, head).map(|m| {
                let text = m.diag.message.split_whitespace().collect::<Vec<_>>().join(" ");
                (format!("{} {text}", m.diag.severity.icon()), style)
            });
        }
        let dirty = if buf.is_dirty() { " ●" } else { "" };
        let flag = if buf.deleted {
            " [deleted]"
        } else if buf.read_only.is_some() {
            " [read-only]"
        } else {
            ""
        };
        let left = format!(" {}{dirty}{flag}  {problems}", buf.name());

        // As VS Code: the position for one selection, the count for several.
        let count = tab.view.selections.len();
        let position = if count > 1 {
            format!("{count} selections")
        } else {
            let head = tab.view.selections.primary().head;
            let line = pos::line_of(&buf.rope, head) + 1;
            let col = pos::display_col(&buf.rope, head, app.settings.tab_width).0 + 1;
            format!("Ln {line}, Col {col}")
        };
        let settings = app.settings.for_language(buf.syntax.language());
        let indent = if settings.insert_spaces {
            format!("Spaces: {}", app.settings.tab_width)
        } else {
            format!("Tab Size: {}", app.settings.tab_width)
        };
        let encoding = if buf.bom { "UTF-8 with BOM" } else { "UTF-8" };
        let right = format!(
            "{position}   {indent}   {encoding}   {}   {} ",
            buf.eol.label(),
            buf.syntax.language().name()
        );

        let right_w = right.width().min(width);
        right_x = area.x + (width - right_w) as u16;
        out.set_stringn(right_x, area.y, &right, right_w, style);
        let room = right_x.saturating_sub(x) as usize;
        x = out.set_stringn(x, area.y, &left, room, style).0;
    } else if !problems.is_empty() {
        x = out.set_stringn(x, area.y, format!(" {problems}"), width, style).0;
    }

    if let Some((text, msg_style)) = message {
        let room = right_x.saturating_sub(x + 1) as usize;
        out.set_stringn(x, area.y, format!(" {text} "), room, msg_style);
    }
}
