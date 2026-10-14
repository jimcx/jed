//! Modal prompts.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, InputKind, Prompt};
use crate::workspace::trust::Trust;

/// Lines of commands shown in the trust dialog before "and N more".
const MAX_TRUST_LINES: usize = 12;

pub fn render(f: &mut Frame, area: Rect, app: &App, prompt: &Prompt) {
    let style = app.theme.get("dialog");
    let key = style.patch(app.theme.get("dialog.key"));
    let buttons = |choices: &[(&str, &str)]| {
        let mut spans = Vec::new();
        for (k, rest) in choices {
            spans.push(Span::styled(format!("[{k}]"), key));
            spans.push(Span::styled(format!("{rest}   "), style));
        }
        Line::from(spans).centered()
    };
    let name_of = |id| app.buffers.get(id).map(|b| b.name()).unwrap_or_default();
    // Paths relative to the workspace, which itself shows as its name.
    let rel = |p: &std::path::Path| match p.strip_prefix(&app.workspace.root) {
        Ok(r) if r.as_os_str().is_empty() => app.workspace.name(),
        Ok(r) => format!("{}/{}", app.workspace.name(), r.display()),
        Err(_) => p.display().to_string(),
    };

    let mut lines = vec![Line::raw("")];
    let mut cursor = None;
    match prompt {
        Prompt::SaveBeforeClose { buffer, .. } => {
            lines.push(Line::raw(format!(" Save changes to {}?", name_of(*buffer))));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("S", "ave"), ("D", "on't Save"), ("C", "ancel")]));
        }
        Prompt::SaveAllBeforeQuit { buffers } => {
            let q = match buffers.as_slice() {
                [one] => format!(" Save changes to {} before quitting?", name_of(*one)),
                many => format!(" Save changes to {} files before quitting?", many.len()),
            };
            lines.push(Line::raw(q));
            for id in buffers.iter().take(8).filter(|_| buffers.len() > 1) {
                lines.push(Line::raw(format!("   {}", name_of(*id))));
            }
            lines.push(Line::raw(""));
            let save = if buffers.len() > 1 { "ave All" } else { "ave" };
            lines.push(buttons(&[("S", save), ("D", "on't Save"), ("C", "ancel")]));
        }
        Prompt::Trust { commands } => {
            lines.push(Line::raw(format!(
                " {} in this folder can run these commands:",
                crate::workspace::FILE_NAME
            )));
            lines.push(Line::raw(""));
            for c in commands.iter().take(MAX_TRUST_LINES) {
                lines.push(Line::styled(format!("   {c}"), key));
            }
            if commands.len() > MAX_TRUST_LINES {
                lines.push(Line::raw(format!("   …and {} more", commands.len() - MAX_TRUST_LINES)));
            }
            lines.push(Line::raw(""));
            lines.push(Line::raw(" Trust them only if you trust the authors of this folder."));
            lines.push(Line::raw(" In Restricted mode they won't run; editing still works."));
            // Reopened from "Manage Workspace Trust": say what was decided before.
            match app.trust {
                Trust::Trusted => lines.push(Line::raw(" Currently: trusted.")),
                Trust::Restricted => lines.push(Line::raw(" Currently: Restricted mode.")),
                Trust::NotNeeded | Trust::Undecided => {}
            }
            lines.push(Line::raw(""));
            lines.push(buttons(&[("T", "rust"), ("R", "estricted mode")]));
        }
        Prompt::ConfirmDelete { path, is_dir } => {
            let what = if *is_dir { "folder" } else { "file" };
            lines.push(Line::raw(format!(" Move {what} {} to the Trash?", rel(path))));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("T", "rash"), ("C", "ancel")]));
        }
        Prompt::Input(input) => {
            let title = match &input.kind {
                InputKind::NewFile { dir } => format!(" New file in {}/", rel(dir)),
                InputKind::NewFolder { dir } => format!(" New folder in {}/", rel(dir)),
                InputKind::Rename { from } => format!(" Rename {}", rel(from)),
            };
            lines.push(Line::raw(title));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!(" {} ", input.field.text),
                style.patch(app.theme.get("dialog.input")),
            ));
            cursor = Some((lines.len() - 1, 1 + input.field.text[..input.field.cursor].width()));
            lines.push(match input.error {
                Some(e) => Line::styled(format!(" {e}"), style.patch(app.theme.get("dialog.error"))),
                None => Line::raw(""),
            });
            lines.push(buttons(&[("Enter", " confirm"), ("Esc", " cancel")]));
        }
    }

    let content_w = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (content_w + 4).clamp(44, area.width);
    let height = (lines.len() as u16 + 3).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    if let (Some((row, col)), Prompt::Input(_)) = (cursor, prompt) {
        // Stretch the input field across the dialog.
        let field = &mut lines[row];
        let pad = (width as usize).saturating_sub(4 + field.width());
        field.spans.push(Span::styled(
            " ".repeat(pad),
            style.patch(app.theme.get("dialog.input")),
        ));
        let x = (rect.x + 1 + col as u16).min(rect.x + width - 2);
        f.set_cursor_position(Position {
            x,
            y: rect.y + 1 + row as u16,
        });
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style.patch(app.theme.get("dialog.border")))
        .style(style);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).style(Style::default()).block(block), rect);
}
