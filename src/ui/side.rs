//! Side panel: explorer tree, the Search view (`ui::search`) or the Source Control view
//! (`ui::scm`).

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::layout::{ScmHit, SearchHit, SidePanel};

/// What the side panel drew, for hit-testing.
#[derive(Default)]
pub struct SideHits {
    /// The area holding explorer rows.
    pub explorer_rows: Rect,
    pub search: Option<SearchHit>,
    pub scm: Option<ScmHit>,
}

/// Draws the panel.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> SideHits {
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
    let view = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..body
    };
    match app.ui.side_panel {
        SidePanel::Search => {
            return SideHits {
                search: super::search::render(f, view, app),
                ..SideHits::default()
            };
        }
        SidePanel::SourceControl => {
            return SideHits {
                scm: super::scm::render(f, view, app),
                ..SideHits::default()
            };
        }
        SidePanel::Explorer => {}
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
        // Git colouring (VS Code): the name in its status's colour, a changed file's letter at
        // the right edge.
        let decoration = app.git.status.as_ref().and_then(|s| s.decoration(&e.path));
        let style = match decoration {
            Some(d) => style.patch(theme.get(d.theme_key())),
            None => style,
        };
        let mut room = body.width as usize;
        if let Some(d) = decoration
            && !e.is_dir
            && body.width > 6
        {
            room -= 3;
            out.set_stringn(body.x + body.width - 2, y, d.letter().to_string(), 1, style);
        }
        let text = format!("{}{marker}{}", "  ".repeat(row.depth + 1), e.name);
        out.set_stringn(body.x, y, text, room, style);
    }
    SideHits {
        explorer_rows: body,
        ..SideHits::default()
    }
}
