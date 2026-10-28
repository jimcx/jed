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

pub fn render(f: &mut Frame, area: Rect, app: &App, prompt: &Prompt) -> Option<Rect> {
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
        Prompt::SaveBeforeStage { buffer, file, .. } => {
            let what = if *file { "it" } else { "the change" };
            lines.push(Line::raw(format!(" Save {} before staging {what}?", name_of(*buffer))));
            lines.push(Line::raw(" Changes are staged from the file on disk."));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("S", "ave and Stage"), ("C", "ancel")]));
        }
        Prompt::SaveBeforeStageAll { buffers } => {
            let q = match buffers.as_slice() {
                [one] => format!(" Save {} before staging all changes?", name_of(*one)),
                many => format!(" Save {} files before staging all changes?", many.len()),
            };
            lines.push(Line::raw(q));
            for id in buffers.iter().take(8).filter(|_| buffers.len() > 1) {
                lines.push(Line::raw(format!("   {}", name_of(*id))));
            }
            lines.push(Line::raw(" Changes are staged from the files on disk."));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("S", "ave and Stage"), ("C", "ancel")]));
        }
        Prompt::RestoreBackups => {
            let offered = &app.backups.offered;
            let q = match offered.as_slice() {
                [one] => format!(
                    " Unsaved changes to {} were backed up",
                    one.label().unwrap_or_else(|| rel(&one.path))
                ),
                many => format!(" Unsaved changes to {} files were backed up", many.len()),
            };
            lines.push(Line::raw(q));
            lines.push(Line::raw(" when jed last stopped without saving them:"));
            let now = crate::history::now_ms();
            for b in offered.iter().take(MAX_TRUST_LINES) {
                let ago = crate::history::ago(b.time, now);
                lines.push(Line::from(vec![
                    Span::styled(format!("   {}", b.label().unwrap_or_else(|| rel(&b.path))), key),
                    Span::styled(format!("  {ago}"), style),
                ]));
            }
            if offered.len() > MAX_TRUST_LINES {
                lines.push(Line::raw(format!("   …and {} more", offered.len() - MAX_TRUST_LINES)));
            }
            lines.push(Line::raw(""));
            lines.push(Line::raw(" Restore puts them back in their tabs, unsaved."));
            lines.push(Line::raw(" Later keeps them for the next time jed opens this folder."));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("R", "estore"), ("D", "iscard"), ("L", "ater")]));
        }
        Prompt::ConfirmReplace {
            occurrences,
            files,
            with,
        } => {
            lines.push(Line::raw(format!(
                " Replace {occurrences} occurrences across {files} files?"
            )));
            let with = if with.is_empty() {
                "nothing".to_string()
            } else {
                format!("'{with}'")
            };
            lines.push(Line::raw(format!(" With {with}; open files are left unsaved.")));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("R", "eplace"), ("C", "ancel")]));
        }
        Prompt::ConfirmOverwrite { path, .. } => {
            lines.push(Line::raw(format!(" {} already exists. Overwrite it?", rel(path))));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("O", "verwrite"), ("C", "ancel")]));
        }
        Prompt::ConfirmStageConflict { path, .. } => {
            lines.push(Line::raw(format!(
                " {} still has conflict markers. Stage it as resolved?",
                rel(path)
            )));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("S", "tage anyway"), ("C", "ancel")]));
        }
        Prompt::ConfirmDelete { path, is_dir } => {
            let what = if *is_dir { "folder" } else { "file" };
            lines.push(Line::raw(format!(" Move {what} {} to the Trash?", rel(path))));
            lines.push(Line::raw(""));
            lines.push(buttons(&[("T", "rash"), ("C", "ancel")]));
        }
        Prompt::InstallShellIntegration { snippet, init, line } => {
            let home = crate::paths::home();
            let short = |p: &std::path::Path| match p.strip_prefix(&home) {
                Ok(r) => format!("~/{}", r.display()),
                Err(_) => p.display().to_string(),
            };
            lines.push(Line::raw(" The tab bar follows each command the shell runs:"));
            lines.push(Line::raw(" add this line to the shell's init file."));
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!("   {line}"), key));
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!(" The snippet it sources: {}", short(snippet))));
            lines.push(Line::raw(format!(" Init file: {}", short(init))));
            lines.push(Line::raw(""));
            lines.push(Line::raw(" A new shell tab picks it up; jed never runs the shell."));
            lines.push(Line::raw(""));
            let append = format!("ppend to {}", short(init));
            lines.push(buttons(&[("C", "opy"), ("A", &append)]));
        }
        Prompt::Input(input) => {
            let title = match &input.kind {
                InputKind::NewFile { dir } => format!(" New file in {}/", rel(dir)),
                InputKind::NewFolder { dir } => format!(" New folder in {}/", rel(dir)),
                InputKind::Rename { from } => format!(" Rename {}", rel(from)),
                InputKind::SaveAs { buffer } => format!(" Save {} as", name_of(*buffer)),
                InputKind::RenameSymbol { old, .. } => format!(" Rename '{old}' to"),
            };
            lines.push(Line::raw(title));
            lines.push(Line::raw(""));
            let field = style.patch(app.theme.get("dialog.input"));
            // Selected text (a rename's prefilled name) is replaced by typing.
            let selected = field.patch(app.theme.get("editor.selection"));
            let text = &input.field.text;
            let sel = input.field.selection().unwrap_or(0..0);
            lines.push(Line::from(vec![
                Span::styled(" ", field),
                Span::styled(text[..sel.start].to_string(), field),
                Span::styled(text[sel.clone()].to_string(), selected),
                Span::styled(text[sel.end..].to_string(), field),
                Span::styled(" ", field),
            ]));
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
    let mut input_rect = None;
    if let (Some((row, col)), Prompt::Input(_)) = (cursor, prompt) {
        input_rect = Some(Rect {
            x: rect.x + 1,
            y: rect.y + 1 + row as u16,
            width: width.saturating_sub(2),
            height: 1,
        });
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
    input_rect
}
