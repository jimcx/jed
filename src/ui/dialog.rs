//! Modal prompts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Prompt};

pub fn render(f: &mut Frame, area: Rect, app: &App, prompt: &Prompt) {
    let style = app.theme.get("dialog");
    let key = style.patch(app.theme.get("dialog.key"));
    let (question, choices): (String, &[(&str, &str)]) = match prompt {
        Prompt::SaveBeforeQuit => {
            let name = app.buffer.path.file_name().unwrap_or_default().to_string_lossy();
            (
                format!("Save changes to {name} before quitting?"),
                &[("S", "ave"), ("D", "on't Save"), ("C", "ancel")],
            )
        }
    };

    let mut buttons = Vec::new();
    for (k, rest) in choices {
        buttons.push(Span::styled(format!("[{k}]"), key));
        buttons.push(Span::styled(format!("{rest}   "), style));
    }
    let width = (question.len() as u16 + 6).clamp(40, area.width);
    let height = 6.min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(style.patch(app.theme.get("dialog.border")))
        .style(style);
    let body = Paragraph::new(vec![
        Line::raw(""),
        Line::styled(format!(" {question}"), style),
        Line::raw(""),
        Line::from(buttons).centered(),
    ])
    .block(block);
    f.render_widget(Clear, rect);
    f.render_widget(body, rect);
}
