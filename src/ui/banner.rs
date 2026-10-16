//! The changed-on-disk banner: one row over an editor whose file changed on disk while its buffer
//! had unsaved edits, or into something not reloaded silently (binary, not UTF-8):
//! `Buffer::disk_conflict`. A message, then a button per `disk::BannerAction`.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::disk::BannerAction;
use crate::layout::BannerHit;
use crate::theme::Theme;

/// Draws the banner on `row` for the file `name` (`edits`: the buffer has unsaved edits, else
/// the file wasn't reloaded because it's no longer text); returns the buttons' rects.
pub fn render(f: &mut Frame, row: Rect, name: &str, edits: bool, theme: &Theme) -> BannerHit {
    let out = f.buffer_mut();
    let base = theme.get("banner");
    out.set_style(row, base);
    let mut hit = BannerHit {
        row,
        actions: Vec::new(),
    };
    let right = row.x + row.width;
    let labels: Vec<(String, BannerAction)> = BannerAction::ALL
        .iter()
        .map(|a| (format!(" {} ", a.label()), *a))
        .collect();
    let buttons: u16 = labels.iter().map(|(l, _)| l.width() as u16 + 1).sum();
    // The message gets what the buttons leave ("⚠ " first).
    let message = if edits {
        format!("{name} has changed on disk, and you have unsaved changes.")
    } else {
        format!("{name} has changed on disk and wasn't reloaded.")
    };
    let room = row.width.saturating_sub(buttons + 4) as usize;
    out.set_stringn(row.x + 1, row.y, "⚠", 1, base.patch(theme.get("banner.icon")));
    let (end, _) = out.set_stringn(row.x + 3, row.y, &message, room, base);
    let mut x = end + 1;
    let button = base.patch(theme.get("banner.action"));
    for (label, action) in labels {
        let w = label.width() as u16;
        if x + w > right {
            break;
        }
        out.set_stringn(x, row.y, &label, w as usize, button);
        hit.actions.push((Rect::new(x, row.y, w, 1), action));
        x += w + 1;
    }
    hit
}
