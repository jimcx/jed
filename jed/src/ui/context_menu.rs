//! The context menu (`app.context_menu`): a borderless popup on `menu.background` (a shade lighter
//! than the editor) opened at its anchor and kept inside the screen (`context_menu::place`). One
//! row per item with a cell of padding each side: the label, and the key bound to its command
//! right-aligned and dim (looked up from the keymap here, never stored); the selected row filled
//! (`menu.selectionBackground`), a disabled item in `menu.disabledForeground`, a separator a `─`
//! line (`menu.separator`). A menu taller than the screen is cut.

use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::context_menu::{MenuItem, place};
use crate::layout::ContextMenuHit;

/// Blank cells between the longest label and its hint.
const GAP: usize = 2;

pub fn render(f: &mut Frame, app: &App) -> Option<ContextMenuHit> {
    let menu = app.context_menu.as_ref()?;
    let hints: Vec<String> = menu
        .items
        .iter()
        .map(|i| match i {
            MenuItem::Item { command, .. } => app.keymap.binding_for(*command).map(|k| k.label()).unwrap_or_default(),
            MenuItem::Separator => String::new(),
        })
        .collect();
    let widest = |text: &dyn Fn(usize) -> usize| (0..menu.items.len()).map(text).max().unwrap_or(0);
    let labels = widest(&|i| match &menu.items[i] {
        MenuItem::Item { label, .. } => label.width(),
        MenuItem::Separator => 0,
    });
    let hint = widest(&|i| hints[i].width());
    let width = 2 + labels + if hint > 0 { GAP + hint } else { 0 };
    let screen = f.area();
    let rect = place(menu.anchor, width as u16, menu.items.len() as u16, screen);
    if rect.width < 3 || rect.height == 0 {
        return None;
    }
    let theme = &app.theme;
    let base = theme.get("menu.background").patch(theme.get("menu.foreground"));
    let selected = theme
        .get("menu.selectionBackground")
        .patch(theme.get("menu.selectionForeground"));
    let dim = base.patch(theme.get("menu.disabledForeground"));
    let line = base.patch(theme.get("menu.separator"));
    let out = f.buffer_mut();
    let shown = (rect.height as usize).min(menu.items.len());
    for (i, item) in menu.items.iter().take(shown).enumerate() {
        let y = rect.y + i as u16;
        let inner = rect.width as usize - 2;
        match item {
            MenuItem::Separator => {
                out.set_stringn(rect.x, y, " ".repeat(rect.width as usize), rect.width as usize, base);
                out.set_stringn(rect.x + 1, y, "─".repeat(inner), inner, line);
            }
            MenuItem::Item { label, enabled, .. } => {
                let (row, hint_style) = if i == menu.selected && *enabled {
                    (selected, selected)
                } else if *enabled {
                    (base, dim)
                } else {
                    (dim, dim)
                };
                out.set_stringn(rect.x, y, " ".repeat(rect.width as usize), rect.width as usize, row);
                out.set_stringn(rect.x + 1, y, label, inner, row);
                let h = &hints[i];
                let hw = h.width();
                if hw > 0 && hw + 1 < inner {
                    out.set_stringn(rect.x + rect.width - 1 - hw as u16, y, h, hw, hint_style);
                }
            }
        }
    }
    Some(ContextMenuHit { rect, shown })
}
