//! The code action list (`app.code_actions.popup`): over its group's editor, below the anchor's
//! row (above when there's no room; `ui::popup`), from the anchor's column. Each row: `★` (in
//! `editorActionList.preferred`) for the preferred action, then its title; a disabled one in
//! `editorActionList.disabledForeground` with its reason after it; the selected row in
//! `.focusBackground`. Up to `code_action::MAX_ROWS` rows, a longer list scrolls (keys, wheel)
//! with "…" in the border where rows are cut.

use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::popup::{self, Side};
use crate::app::App;
use crate::code_action::{Action, MAX_ROWS, MAX_WIDTH};
use crate::layout::{CompletionHit, GroupHit};

/// A row's text after the mark: the title, and a disabled action's reason.
fn label(a: &Action) -> String {
    let clean = |s: &str| -> String {
        let line = s.lines().next().unwrap_or("");
        line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    };
    match &a.disabled {
        Some(reason) => format!("{}  ({})", clean(&a.title), clean(reason)),
        None => clean(&a.title),
    }
}

/// Draws the list if its anchor is on screen; returns where.
pub fn render(f: &mut Frame, app: &mut App, groups: &[GroupHit]) -> Option<CompletionHit> {
    let popup = app.code_actions.popup.as_mut()?;
    let hit = groups.iter().find(|g| g.id == popup.group)?;
    let tab = app.groups.get_mut(popup.group)?.active_tab_mut()?;
    if tab.buffer != popup.buffer {
        return None;
    }
    let buf = app.buffers.get(popup.buffer)?;
    let (ax, ay) = popup::cell_of(
        &buf.rope,
        &mut tab.view,
        hit.text,
        popup.anchor,
        app.settings.for_buffer(buf).tab_width,
    )?;
    let area = hit.editor;
    let labels: Vec<String> = popup.actions.iter().map(label).collect();
    let widest = labels.iter().map(|l| l.width()).max().unwrap_or(0);
    let content = (2 + widest).min(MAX_WIDTH).min(area.width.saturating_sub(2) as usize);
    if content < 4 {
        return None;
    }
    let rows = popup.actions.len().min(MAX_ROWS) as u16;
    let rect = popup::place(
        area,
        (ax.saturating_sub(1).max(area.x), ay),
        content as u16 + 2,
        rows + 2,
        Side::Below,
    )?;

    let theme = &app.theme;
    let base = theme
        .get("editorActionList.background")
        .patch(theme.get("editorActionList.foreground"));
    let border = base.patch(theme.get("editorActionList.border"));
    let focus = base.patch(theme.get("editorActionList.focusBackground"));
    let disabled = theme.get("editorActionList.disabledForeground");
    let preferred = theme.get("editorActionList.preferred");
    let out = f.buffer_mut();
    let inner = popup::frame(out, rect, base, border);
    let shown = inner.height as usize;
    popup.rows = shown;
    popup.top = popup.top.min(popup.actions.len().saturating_sub(shown));
    for r in 0..shown {
        let index = popup.top + r;
        let Some(action) = popup.actions.get(index) else {
            break;
        };
        let y = inner.y + r as u16;
        let row = if index == popup.selected { focus } else { base };
        for x in inner.x..inner.x + inner.width {
            out[(x, y)].set_symbol(" ").set_style(row);
        }
        if action.preferred {
            out.set_stringn(inner.x, y, "★", 1, row.patch(preferred));
        }
        let style = if action.disabled.is_some() {
            row.patch(disabled)
        } else {
            row
        };
        let end = inner.x + inner.width;
        let mut x = inner.x + 2;
        let text = &labels[index];
        let fits = text.width() <= end.saturating_sub(x) as usize;
        for g in text.graphemes(true) {
            let w = g.width() as u16;
            let room = if fits { end } else { end.saturating_sub(1) };
            if x + w > room {
                out.set_stringn(x, y, "…", 1, style);
                break;
            }
            if w > 0 {
                out.set_stringn(x, y, g, w as usize, style);
            }
            x += w;
        }
    }
    let more_at = rect.x + rect.width.saturating_sub(3);
    if popup.top > 0 {
        out.set_stringn(more_at, rect.y, "…", 1, border);
    }
    if popup.top + shown < popup.actions.len() {
        out.set_stringn(more_at, rect.y + rect.height - 1, "…", 1, border);
    }
    Some(CompletionHit {
        rect,
        rows: inner,
        top: popup.top,
    })
}
