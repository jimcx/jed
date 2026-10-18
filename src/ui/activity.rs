//! Activity bar: one icon per side panel.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::App;
use crate::layout::SidePanel;

fn icon(panel: SidePanel) -> &'static str {
    match panel {
        SidePanel::Explorer => "▤",
        SidePanel::Search => "⌕",
        SidePanel::SourceControl => "⑂",
        SidePanel::Tools => "≡",
    }
}

/// Returns each icon's row, for hit-testing.
pub fn render(f: &mut Frame, area: Rect, app: &App) -> Vec<(u16, SidePanel)> {
    let base = app.theme.get("activityBar");
    let active = base.patch(app.theme.get("activityBar.active"));
    let out = f.buffer_mut();
    out.set_style(area, base);
    let mut rows = Vec::new();
    for (i, panel) in SidePanel::ALL.into_iter().enumerate() {
        let y = area.y + 1 + 2 * i as u16;
        if y >= area.y + area.height {
            break;
        }
        let is_active = app.ui.side_visible && app.ui.side_panel == panel;
        let style = if is_active { active } else { base };
        if is_active {
            out.set_stringn(area.x, y, "▎", 1, style);
        }
        out.set_stringn(area.x + 1, y, icon(panel), 1, style);
        rows.push((y, panel));
    }
    rows
}
