//! A hex tab (`hex::HexTab`): its rows exactly as `hex::row` formats them (`hexdump -C`), only
//! the visible ones, fitted to the area first (`HexTab::fit`: bytes per row, the scroll). The
//! offsets in `hexEditor.offset`, the `|`s in `hexEditor.border`, a byte that isn't printable
//! ASCII (its hex digits and its `.`) in `hexEditor.nonPrintable`; the selection as
//! `editor.selection` in both columns (the gap between two selected bytes' hex too); the cursor's
//! byte as the terminal cursor in its column (in the focused group) and `editor.cursor.secondary`
//! in the other (in both when unfocused). Rows wider than the area are cut. Find's matches (the
//! widget's query, `hex::find_in` over the visible bytes only) in both columns, under the
//! selection as `editor.findMatchHighlight`, the selected one over it as `editor.findMatch`, as
//! the editor layers them; a match over a row break shows on both rows.

use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::hex::{self, Column, HexTab};
use crate::theme::Theme;

/// How a tab is drawn besides its bytes.
pub struct Overlay<'a> {
    /// Only the focused group shows the terminal cursor.
    pub focused: bool,
    /// The find widget's query, when it's shown over this tab.
    pub find: Option<&'a hex::Query>,
}

/// Draws `tab` in `area` (`max`: the `hex_bytes_per_line` setting); returns the rows' rect
/// (`GroupHit::text`, for the mouse).
pub fn render(f: &mut Frame, area: Rect, tab: &mut HexTab, theme: &Theme, max: usize, overlay: Overlay) -> Rect {
    let focused = overlay.focused;
    tab.fit(area.width as usize, area.height as usize, max);
    let base = theme.get("editor");
    let offset_style = base.patch(theme.get("hexEditor.offset"));
    let border = base.patch(theme.get("hexEditor.border"));
    let dim = theme.get("hexEditor.nonPrintable");
    let selection = theme.get("editor.selection");
    let other = theme.get("editor.cursor.secondary");
    let find_highlight = theme.get("editor.findMatchHighlight");
    let find_match = theme.get("editor.findMatch");
    let out = f.buffer_mut();
    out.set_style(area, base);

    let layout = tab.layout;
    let n = layout.bytes_per_row();
    let len = tab.len();
    let bytes = tab.doc.bytes();
    let selected = tab.selection().unwrap_or(0..0);
    let found = Found::new(
        bytes,
        overlay.find,
        tab.top..(tab.top + area.height as usize * n).min(len),
        &selected,
    );
    let mut cursor_cell = None;
    let paint = |out: &mut Cells, y: u16, cols: Range<usize>, style: Style| {
        let end = cols.end.min(area.width as usize);
        if cols.start < end {
            let rect = Rect::new(area.x + cols.start as u16, y, (end - cols.start) as u16, 1);
            out.set_style(rect, style);
        }
    };
    let rows = layout.rows(len);
    for screen in 0..area.height {
        let row = tab.top_row() + screen as usize;
        if row >= rows {
            break;
        }
        let y = area.y + screen;
        let start = layout.row_offset(row, len);
        out.set_stringn(area.x, y, hex::row(&tab.doc, &layout, start), area.width as usize, base);
        paint(out, y, layout.offset_cols(), offset_style);
        if start >= len {
            // The end offset's row.
            continue;
        }
        let count = n.min(len - start);
        paint(out, y, layout.bar_col()..layout.bar_col() + 1, border);
        let close = layout.ascii_col(count);
        paint(out, y, close..close + 1, border);
        for i in 0..count {
            let at = start + i;
            let hex_cells = layout.hex_col(i)..layout.hex_col(i) + 2;
            let ascii_cell = layout.ascii_col(i)..layout.ascii_col(i) + 1;
            if !hex::is_printable(bytes[at]) {
                paint(out, y, hex_cells.clone(), dim);
                paint(out, y, ascii_cell.clone(), dim);
            }
            // Find matches, the selection, the selected match: each run of bytes painted through
            // the gaps between their hex cells, so a run is one block.
            let layers: [(&dyn Fn(usize) -> bool, Style); 3] = [
                (&|b| found.covers(b), find_highlight),
                (&|b| selected.contains(&b), selection),
                (&|b| found.current.contains(&b), find_match),
            ];
            for (has, style) in layers {
                if has(at) {
                    let end = if i + 1 < count && has(at + 1) {
                        layout.hex_col(i + 1)
                    } else {
                        hex_cells.end
                    };
                    paint(out, y, hex_cells.start..end, style);
                    paint(out, y, ascii_cell.clone(), style);
                }
            }
            if at == tab.cursor {
                let (mine, theirs) = match tab.column {
                    Column::Hex => (hex_cells.clone(), ascii_cell.clone()),
                    Column::Ascii => (ascii_cell.clone(), hex_cells.clone()),
                };
                paint(out, y, theirs, other);
                if focused {
                    cursor_cell = (mine.start < area.width as usize).then(|| (area.x + mine.start as u16, y));
                } else {
                    paint(out, y, mine, other);
                }
            }
        }
    }
    if let Some((x, y)) = cursor_cell {
        f.set_cursor_position(Position { x, y });
    }
    area
}

/// Find's matches over the visible bytes: how many cover each one, and the selected match.
struct Found {
    visible: Range<usize>,
    /// Per visible byte, the number of matches covering it.
    cover: Vec<u32>,
    /// The selection when it is exactly a match (else empty).
    current: Range<usize>,
}

impl Found {
    /// Searches only `visible` (± the query's length − 1, `hex::find_in`); matches have one
    /// length, so the cover counts come from a difference array in O(bytes + matches).
    fn new(bytes: &[u8], query: Option<&hex::Query>, visible: Range<usize>, selected: &Range<usize>) -> Self {
        let mut found = Found {
            cover: Vec::new(),
            current: 0..0,
            visible: visible.clone(),
        };
        let Some(q) = query else { return found };
        let matches = hex::find_in(bytes, q, visible.clone());
        if matches.is_empty() {
            return found;
        }
        let mut diff = vec![0i64; visible.len() + 1];
        for m in &matches {
            let s = m.start.max(visible.start) - visible.start;
            let e = m.end.min(visible.end) - visible.start;
            diff[s] += 1;
            diff[e] -= 1;
        }
        let mut sum = 0;
        found.cover = diff[..visible.len()]
            .iter()
            .map(|d| {
                sum += d;
                sum as u32
            })
            .collect();
        if matches.binary_search_by_key(&selected.start, |m| m.start).is_ok() && selected.len() == q.len() {
            found.current = selected.clone();
        }
        found
    }

    fn covers(&self, at: usize) -> bool {
        at >= self.visible.start && self.cover.get(at - self.visible.start).is_some_and(|&c| c > 0)
    }
}
