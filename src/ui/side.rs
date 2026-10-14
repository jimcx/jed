//! Side panel: explorer tree, or a placeholder for panels not built yet.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::layout::SidePanel;

/// Draws the panel and returns the area holding explorer rows.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Rect {
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let out = f.buffer_mut();
    out.set_style(area, base);

    // Border on the right edge; dragging it resizes the panel.
    let border = base.patch(theme.get("sideBar.border"));
    let bx = area.x + area.width - 1;
    for y in area.y..area.y + area.height {
        out.set_stringn(bx, y, "│", 1, border);
    }
    let inner_w = area.width.saturating_sub(1);
    let title = base.patch(theme.get("sideBar.title"));
    out.set_stringn(
        area.x,
        area.y,
        format!("  {}", app.ui.side_panel.title()),
        inner_w as usize,
        title,
    );

    let body = Rect {
        x: area.x,
        y: area.y + 2,
        width: inner_w,
        height: area.height.saturating_sub(2),
    };
    if app.ui.side_panel != SidePanel::Explorer {
        let dim = base.patch(theme.get("sideBar.dim"));
        let when = match app.ui.side_panel {
            SidePanel::Search => "milestone 12",
            _ => "milestone 8",
        };
        out.set_stringn(
            body.x + 2,
            body.y,
            format!("Coming in {when}"),
            inner_w.saturating_sub(2) as usize,
            dim,
        );
        return Rect::default();
    }

    let header = base.patch(theme.get("sideBar.header"));
    let name = app.workspace.name().to_uppercase();
    out.set_stringn(area.x, area.y + 1, format!(" ▾ {name}"), inner_w as usize, header);

    let explorer = &mut app.explorer;
    explorer.height = body.height.max(1) as usize;
    let max_scroll = explorer.rows().len().saturating_sub(explorer.height);
    explorer.scroll = explorer.scroll.min(max_scroll);
    let rows = explorer.rows();
    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };

    for (i, row) in rows.iter().skip(explorer.scroll).take(body.height as usize).enumerate() {
        let y = body.y + i as u16;
        let e = row.entry;
        let is_selected = explorer.selected.as_ref() == Some(&e.path);
        let style = if is_selected { base.patch(selected_style) } else { base };
        if is_selected {
            out.set_style(
                Rect {
                    x: body.x,
                    y,
                    width: body.width,
                    height: 1,
                },
                style,
            );
        }
        let marker = match (e.is_dir, e.expanded) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            (false, _) => "  ",
        };
        let text = format!("{}{marker}{}", "  ".repeat(row.depth + 1), e.name);
        out.set_stringn(body.x, y, text, body.width as usize, style);
    }
    body
}
