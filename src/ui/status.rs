//! Status bar: trust mode, git branch, file, problem counts, language server progress, message
//! (else the cursor line's diagnostic), cursor position, file format, language mode.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Level};
use crate::buffer::pos;
use crate::diagnostics::Severity;
use crate::workspace::trust::Trust;

/// Draws the status bar; returns where the problem counts are (a click toggles the Problems
/// list).
pub fn render(f: &mut Frame, area: Rect, app: &App) -> Option<Rect> {
    let style = app.theme.get("statusBar");
    let out = f.buffer_mut();
    out.set_style(area, style);
    let width = area.width as usize;
    let mut x = area.x;

    if app.trust == Trust::Restricted {
        let (nx, _) = out.set_stringn(x, area.y, " Restricted ", width, app.theme.get("statusBar.restricted"));
        x = nx;
    }
    // The git branch (or detached commit), `*` / `+` / `!` for unstaged / staged / conflicted
    // changes, as VS Code.
    if let Some(git) = &app.git.status {
        let room = width.saturating_sub((x - area.x) as usize);
        x = out
            .set_stringn(x, area.y, format!(" ⎇ {} ", git.label()), room, style)
            .0;
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
    // The counts' cells: from `x`, `problems` (without its trailing gap) cut at `end`.
    let counts_at = |x: u16, end: u16| {
        let w = (problems.trim_end().width() as u16).min(end.saturating_sub(x));
        (w > 0).then_some(Rect {
            x,
            y: area.y,
            width: w,
            height: 1,
        })
    };
    let mut counts = None;
    if let Some(h) = app.active_hex() {
        let flag = if h.deleted { " [deleted]" } else { " [read-only]" };
        let name = format!(" {}{flag}  ", h.name());
        let left = format!("{name}{problems}");
        let right = format!("{} ", hex_status(h).join("   "));
        let right_w = right.width().min(width);
        right_x = area.x + (width - right_w) as u16;
        out.set_stringn(right_x, area.y, &right, right_w, style);
        let room = right_x.saturating_sub(x) as usize;
        let start = x;
        x = out.set_stringn(x, area.y, &left, room, style).0;
        if !problems.is_empty() {
            counts = counts_at(start + name.width() as u16, x);
        }
    } else if let (Some(buf), Some(tab)) = (app.active_buffer(), app.group().active_tab()) {
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
        let name = format!(" {}{dirty}{flag}  ", buf.name());
        let left = format!("{name}{problems}");

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
        let start = x;
        x = out.set_stringn(x, area.y, &left, room, style).0;
        if !problems.is_empty() {
            counts = counts_at(start + name.width() as u16, x);
        }
    } else if !problems.is_empty() {
        let start = x;
        x = out.set_stringn(x, area.y, format!(" {problems}"), width, style).0;
        counts = counts_at(start + 1, x);
    }

    // A language server at work (`$/progress`): `rust-analyzer: Indexing 40% 3/25 (core)`.
    if let Some(progress) = app.lsp.progress() {
        let room = right_x.saturating_sub(x + 1) as usize;
        x = out.set_stringn(x, area.y, format!(" {progress} "), room, style).0;
    }

    if let Some((text, msg_style)) = message {
        let room = right_x.saturating_sub(x + 1) as usize;
        out.set_stringn(x, area.y, format!(" {text} "), room, msg_style);
    }
    counts
}

/// A hex tab's part of the status bar: the cursor's offset in hex and decimal and its byte's
/// value (none in an empty file), the selection's length, the file's size.
fn hex_status(h: &crate::hex::HexTab) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(&b) = h.doc.bytes().get(h.cursor) {
        let c = h.cursor;
        parts.push(format!("Offset 0x{c:x} ({c})"));
        parts.push(format!("0x{b:02x} {b}"));
    }
    if let Some(sel) = h.selection() {
        let n = sel.len();
        parts.push(format!("{n} byte{} selected", if n == 1 { "" } else { "s" }));
    }
    parts.push(format!("{} bytes", h.len()));
    parts
}
