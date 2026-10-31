//! Status bar: trust mode, git branch, file, problem counts, language server progress, message
//! (else the cursor line's diagnostic), cursor position, file format, language mode.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Level};
use crate::buffer::pos;
use crate::diagnostics::Severity;
use crate::workspace::trust::Trust;

/// What the status bar drew that a click acts on.
#[derive(Debug, Default)]
pub struct StatusHit {
    /// The problem counts (a click toggles the Problems list).
    pub problems: Option<Rect>,
    /// A text tab's `Spaces: N` / `Tab Size: N` (a click opens the indentation picker).
    pub indentation: Option<Rect>,
    /// The language name (a click opens Change Language Mode).
    pub language: Option<Rect>,
}

/// The bar's style: `[window] status_bar`'s colour, or for `auto` one picked from a stable hash
/// (FNV-1a) of the workspace root relative to `$HOME` (the absolute path outside it).
fn status_style(app: &App) -> ratatui::style::Style {
    use crate::workspace::config::StatusBarColor;
    let configured = app.workspace.config.window.status_bar;
    if configured != StatusBarColor::Auto {
        return app.theme.get(configured.theme_key());
    }
    let root = &app.workspace.root;
    let rel = std::env::var_os("HOME")
        .and_then(|h| root.strip_prefix(h).ok())
        .unwrap_or(root.as_path());
    let hash = rel
        .to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    app.theme.status_bar_auto(hash)
}

/// Draws the status bar; returns where its clickable items are.
pub fn render(f: &mut Frame, area: Rect, app: &App) -> StatusHit {
    let style = status_style(app);
    let out = f.buffer_mut();
    out.set_style(area, style);
    let width = area.width as usize;
    let mut x = area.x;

    // Single file mode runs nothing to trust: no badge.
    if app.trust == Trust::Restricted && !app.mode.is_single() {
        let (nx, _) = out.set_stringn(x, area.y, " Restricted ", width, app.theme.get("statusBar.restricted"));
        x = nx;
    }
    // The git branch (or detached commit), `*` / `+` / `!` for unstaged / staged / conflicted
    // changes, as VS Code.
    if app.mode.is_single() {
        let room = width.saturating_sub((x - area.x) as usize);
        x = out
            .set_stringn(x, area.y, app.editor_hint.unwrap_or(" Single file "), room, style)
            .0;
    } else if let Some(git) = &app.git.status {
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
    let mut indentation = None;
    let mut language = None;
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
        // Merge conflicts in the file (the editor scanned it earlier in this frame): VS Code's
        // "2 conflicts" beside the name.
        let conflicts = buf
            .conflicts
            .count()
            .filter(|&n| n > 0)
            .map(|n| format!("! {n} conflict{}  ", if n == 1 { "" } else { "s" }))
            .unwrap_or_default();
        let dirty = if buf.is_dirty() { " ●" } else { "" };
        let flag = if buf.deleted {
            " [deleted]"
        } else if buf.read_only.is_some() {
            " [read-only]"
        } else {
            ""
        };
        let name = format!(" {}{dirty}{flag}  ", buf.name());
        let left = format!("{name}{conflicts}{problems}");

        // As VS Code: the position for one selection, the count for several.
        let count = tab.view.selections.len();
        let position = if count > 1 {
            format!("{count} selections")
        } else {
            let head = tab.view.selections.primary().head;
            let line = pos::line_of(&buf.rope, head) + 1;
            let col = pos::display_col(&buf.rope, head, app.settings.for_buffer(buf).tab_width).0 + 1;
            format!("Ln {line}, Col {col}")
        };
        let settings = app.settings.for_buffer(buf);
        let indent = if settings.insert_spaces {
            format!("Spaces: {}", settings.indent_size())
        } else {
            format!("Tab Size: {}", settings.tab_width)
        };
        let encoding = if buf.bom { "UTF-8 with BOM" } else { "UTF-8" };
        // Indentation, encoding, line ending, language: all shown, but in editor mode the hint
        // wins: the position stays, then each of the language, the indentation, the line ending
        // and the encoding while it fits beside the hint.
        let items = [indent.as_str(), encoding, buf.eol.label(), buf.syntax.language().name()];
        let mut shown = [true; 4];
        if app.editor_hint.is_some() {
            let budget = width.saturating_sub((x - area.x) as usize);
            let mut used = position.width() + 1;
            shown = [false; 4];
            for i in [3, 0, 2, 1] {
                if used + 3 + items[i].width() <= budget {
                    used += 3 + items[i].width();
                    shown[i] = true;
                }
            }
        }
        let mut right = position.clone();
        for (item, _) in items.iter().zip(shown).filter(|(_, on)| *on) {
            right.push_str("   ");
            right.push_str(item);
        }
        right.push(' ');

        let right_w = right.width().min(width);
        right_x = area.x + (width - right_w) as u16;
        out.set_stringn(right_x, area.y, &right, right_w, style);
        // The indent label's cells, as far as they're drawn (a narrow bar cuts `right`'s end).
        let end = area.x + area.width;
        let indent_x = (right_x as usize + position.width() + 3).min(end as usize) as u16;
        let indent_w = if shown[0] {
            (indent.width() as u16).min(end - indent_x)
        } else {
            0
        };
        indentation = (indent_w > 0).then_some(Rect::new(indent_x, area.y, indent_w, 1));
        if shown[3] {
            let before: usize = items
                .iter()
                .zip(shown)
                .take(3)
                .filter(|(_, on)| *on)
                .map(|(i, _)| 3 + i.width())
                .sum();
            let x0 = (right_x as usize + position.width() + before + 3).min(end as usize) as u16;
            let w = (items[3].width() as u16).min(end - x0);
            language = (w > 0).then_some(Rect::new(x0, area.y, w, 1));
        }
        // Editor mode: the file's name whole or not at all, never a stub of it after the hint.
        let room = match right_x.saturating_sub(x) as usize {
            room if app.editor_hint.is_some() && room < name.trim_end().width() => 0,
            room => room,
        };
        let start = x;
        x = out.set_stringn(x, area.y, &left, room, style).0;
        if !problems.is_empty() {
            counts = counts_at(start + (name.width() + conflicts.width()) as u16, x);
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
    StatusHit {
        problems: counts,
        indentation,
        language,
    }
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
