//! The completion list (`app.completion.popup`): over its group's editor, below the word being
//! completed (above when there's no room; `ui::popup`), the labels lined up under the word. Each
//! row: the kind's icon in its colour (`completion::icon`), the label with the chars the word
//! matched in `editorSuggestWidget.highlightForeground`; the selected row in
//! `.selectedBackground` with the item's detail right-aligned (`editorSuggestWidget.detail`;
//! VS Code shows it in a side panel). Up to `completion::MAX_ROWS` rows, a longer list scrolls
//! (keys, wheel) with "…" in the border where rows are cut.

use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::popup::{self, Side};
use crate::app::App;
use crate::completion::{self, MAX_ROWS, MAX_WIDTH};
use crate::layout::{CompletionHit, GroupHit};

/// One line, control chars as spaces.
fn clean(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Draws the list if it has rows and its word is on screen; returns where.
pub fn render(f: &mut Frame, app: &mut App, groups: &[GroupHit]) -> Option<CompletionHit> {
    let state = &mut app.completion;
    let popup = state.popup.as_mut().filter(|p| !p.shown.is_empty())?;
    let hit = groups.iter().find(|g| g.id == popup.anchor.group)?;
    let tab = app.groups.get(popup.anchor.group)?.active_tab()?;
    if tab.buffer != popup.anchor.buffer {
        return None;
    }
    let buf = app.buffers.get(popup.anchor.buffer)?;
    let rope = &buf.rope;
    let head = tab.view.selections.primary().head;
    let (ax, ay) = popup::cell_of(rope, &tab.view, hit.text, popup.anchor.start, app.settings.tab_width)?;
    let area = hit.editor;

    let label_w = popup
        .shown
        .iter()
        .map(|&i| popup.items[i].label.width())
        .max()
        .unwrap_or(0);
    let detail_w = popup
        .shown
        .iter()
        .filter_map(|&i| popup.items[i].detail.as_deref())
        .map(|d| clean(d).width())
        .max()
        .unwrap_or(0)
        .min(30);
    let mut content = 2 + label_w.min(MAX_WIDTH);
    if detail_w > 0 {
        content += 2 + detail_w;
    }
    let content = content.min(MAX_WIDTH).min(area.width.saturating_sub(2) as usize);
    if content < 4 {
        return None;
    }
    let rows = popup.shown.len().min(MAX_ROWS) as u16;
    // The labels start under the word: border and icon column to its left.
    let x = ax.saturating_sub(3).max(area.x);
    let rect = popup::place(area, (x, ay), content as u16 + 2, rows + 2, Side::Below)?;

    let theme = &app.theme;
    let base = theme
        .get("editorSuggestWidget.background")
        .patch(theme.get("editorSuggestWidget.foreground"));
    let border = base.patch(theme.get("editorSuggestWidget.border"));
    let selected = base.patch(theme.get("editorSuggestWidget.selectedBackground"));
    let highlight = theme.get("editorSuggestWidget.highlightForeground");
    let detail_style = theme.get("editorSuggestWidget.detail");
    let out = f.buffer_mut();
    let inner = popup::frame(out, rect, base, border);
    let shown = inner.height as usize;
    popup.rows = shown;
    // Keys keep the selection in view (`Popup::move_by`); the wheel may scroll it out.
    popup.top = popup.top.min(popup.shown.len().saturating_sub(shown));
    let mut hits = Vec::new();
    for r in 0..shown {
        let Some(&index) = popup.shown.get(popup.top + r) else {
            break;
        };
        let item = &popup.items[index];
        let y = inner.y + r as u16;
        let row = if popup.top + r == popup.selected {
            selected
        } else {
            base
        };
        for x in inner.x..inner.x + inner.width {
            out[(x, y)].set_symbol(" ").set_style(row);
        }
        let (icon, key) = completion::icon(item.kind);
        out.set_stringn(inner.x, y, icon.to_string(), 1, row.patch(theme.get(key)));
        let query = popup.item_query(rope, head, item).unwrap_or_default();
        state.filter.score(&query, &item.label, Some(&mut hits));
        let end = inner.x + inner.width;
        let mut x = inner.x + 2;
        let mut chars = 0u32;
        let mut matched = hits.iter().peekable();
        let label = clean(&item.label);
        let label_fits = label.width() <= (end - x) as usize;
        for g in label.graphemes(true) {
            let w = g.width() as u16;
            let room = if label_fits { end } else { end.saturating_sub(1) };
            if x + w > room {
                out.set_stringn(x, y, "…", 1, row);
                x += 1;
                break;
            }
            while matched.peek().is_some_and(|&&h| h < chars) {
                matched.next();
            }
            let style = if matched.peek().is_some_and(|&&h| h == chars) {
                row.patch(highlight)
            } else {
                row
            };
            if w > 0 {
                out.set_stringn(x, y, g, w as usize, style);
            }
            x += w;
            chars += g.chars().count() as u32;
        }
        if popup.top + r == popup.selected
            && let Some(d) = item.detail.as_deref()
        {
            let d = clean(d);
            let room = end.saturating_sub(x + 2) as usize;
            if room >= 4 {
                let style = row.patch(detail_style);
                let (text, w) = if d.width() <= room {
                    (d.clone(), d.width())
                } else {
                    let mut t = String::new();
                    let mut w = 0;
                    for g in d.graphemes(true) {
                        if w + g.width() + 1 > room {
                            break;
                        }
                        w += g.width();
                        t.push_str(g);
                    }
                    t.push('…');
                    (t, w + 1)
                };
                out.set_stringn(end - w as u16, y, &text, w, style);
            }
        }
    }
    let more_at = rect.x + rect.width.saturating_sub(3);
    if popup.top > 0 {
        out.set_stringn(more_at, rect.y, "…", 1, border);
    }
    if popup.top + shown < popup.shown.len() {
        out.set_stringn(more_at, rect.y + rect.height - 1, "…", 1, border);
    }
    Some(CompletionHit {
        rect,
        rows: inner,
        top: popup.top,
    })
}
