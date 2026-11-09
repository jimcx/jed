//! A hex tab (`hex::HexTab`): its rows exactly as `hex::row` formats them (`hexdump -C`), only
//! the visible ones, fitted to the area first (`HexTab::fit`: bytes per row, the scroll). The
//! offsets in `hexEditor.offset`, the `|`s in `hexEditor.border`, a byte that isn't printable
//! ASCII (its hex digits and its `.`) in `hexEditor.nonPrintable`; the selection as
//! `editor.selection` in both columns (the gap between two selected bytes' hex too); the cursor's
//! byte as the terminal cursor in its column (in the focused group) and `editor.cursor.secondary`
//! in the other (in both when unfocused). Rows wider than the area are cut. A byte whose page
//! isn't loaded yet (a paged file: `HexDoc::window` queues the read) is `··` / `·` in
//! `hexEditor.pending`, one that can't be read `??` / `?`. Find's matches (the
//! widget's query, `hex::find_in` over the visible bytes that are loaded) in both columns, under the
//! selection as `editor.findMatchHighlight`, the selected one over it as `editor.findMatch`, as
//! the editor layers them; a match over a row break shows on both rows.
//!
//! The data inspector (`HexTab::inspect`, `Overlay::inspector`) is a panel of label / value rows
//! (`hexEditor.inspector.*`) right of the rows when the area fits 8-byte rows beside it, else
//! under them (when there's height for it; otherwise it isn't drawn); the rows get the width
//! (height) left over. Its header shows the byte order, a click target (`HexHit::toggle`); the
//! marked rows are `hexEditor.inspector.active` and their bytes `hexEditor.inspected` in both
//! columns.

use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::hex::{self, ByteOrder, Column, HexTab, Inspected};
use crate::layout::HexHit;
use crate::theme::Theme;

/// How a tab is drawn besides its bytes.
pub struct Overlay<'a> {
    /// Only the focused group shows the terminal cursor.
    pub focused: bool,
    /// The find widget's query, when it's shown over this tab.
    pub find: Option<&'a hex::Query>,
    /// The data inspector, when shown: the byte order it reads in.
    pub inspector: Option<ByteOrder>,
    /// How the hex column groups its bytes.
    pub grouping: hex::Grouping,
}

/// The inspector's width beside the rows (its separator column included).
const PANEL_WIDTH: u16 = 36;
/// Its height under the rows: the header and one row per type.
const PANEL_HEIGHT: u16 = 15;
/// Rows of bytes kept when the panel goes under them.
const MIN_BYTE_ROWS: u16 = 3;
/// Columns of a row's label.
const LABEL_WIDTH: u16 = 12;

/// Where the inspector sits relative to the byte rows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement {
    Beside,
    Below,
}

/// Splits `area` into the byte rows' rect and the inspector's.
fn split(area: Rect, len: usize, max: usize, grouping: hex::Grouping) -> (Rect, Option<(Rect, Placement)>) {
    let want = max.clamp(1, 8);
    if area.width > PANEL_WIDTH
        && hex::Layout::fit_grouped((area.width - PANEL_WIDTH) as usize, want, len as u64, grouping).bytes_per_row()
            >= want
    {
        let rows = Rect {
            width: area.width - PANEL_WIDTH,
            ..area
        };
        let panel = Rect {
            x: area.x + rows.width,
            width: PANEL_WIDTH,
            ..area
        };
        return (rows, Some((panel, Placement::Beside)));
    }
    if area.height >= PANEL_HEIGHT + MIN_BYTE_ROWS {
        let rows = Rect {
            height: area.height - PANEL_HEIGHT,
            ..area
        };
        let panel = Rect {
            y: area.y + rows.height,
            height: PANEL_HEIGHT,
            ..area
        };
        return (rows, Some((panel, Placement::Below)));
    }
    (area, None)
}

/// Draws the panel in `panel`; returns what the mouse can hit.
fn render_panel(
    out: &mut Cells,
    panel: Rect,
    placement: Placement,
    order: ByteOrder,
    inspected: Option<&Inspected>,
    theme: &Theme,
) -> HexHit {
    let base = theme.get("editor");
    let header = base.patch(theme.get("hexEditor.inspector.header"));
    let label = base.patch(theme.get("hexEditor.inspector.label"));
    let value = base.patch(theme.get("hexEditor.inspector.value"));
    let active = theme.get("hexEditor.inspector.active");
    let mut c = panel;
    if placement == Placement::Beside {
        let border = base.patch(theme.get("hexEditor.border"));
        for y in panel.y..panel.y + panel.height {
            out.set_stringn(panel.x, y, "│", 1, border);
        }
        c.x += 1;
        c.width -= 1;
    }
    let mut hit = HexHit {
        offset: inspected.map_or(0, |i| i.offset),
        ..HexHit::default()
    };
    out.set_stringn(c.x, c.y, " Data Inspector", c.width as usize, header);
    let tag = format!(" {} ", order.label());
    let tag_w = tag.chars().count() as u16;
    if c.width >= tag_w {
        let at = Rect::new(c.x + c.width - tag_w, c.y, tag_w, 1);
        out.set_stringn(at.x, at.y, &tag, tag_w as usize, header);
        hit.toggle = at;
    }
    let Some(inspected) = inspected else { return hit };
    for (i, row) in inspected.rows.iter().enumerate() {
        let y = c.y + 1 + i as u16;
        if y >= c.y + c.height {
            break;
        }
        let r = Rect::new(c.x, y, c.width, 1);
        let marked = inspected.marked.contains(&i);
        if marked {
            out.set_style(r, active);
        }
        let (l, v) = if marked { (active, active) } else { (label, value) };
        let lw = LABEL_WIDTH.min(c.width);
        out.set_stringn(c.x + 1, y, row.label, lw.saturating_sub(1) as usize, l);
        if c.width > lw {
            out.set_stringn(c.x + lw, y, &row.value, (c.width - lw) as usize, v);
        }
        hit.rows.push((r, row.kind));
        // `→` at the row's right end follows an integer of a pointer's width as an offset.
        if row.kind.is_integer()
            && matches!(row.width, 4 | 8)
            && row.value != hex::BLANK
            && row.value != hex::PENDING
            && c.width > lw + 2
        {
            let at = Rect::new(c.x + c.width - 2, y, 2, 1);
            out.set_stringn(at.x, at.y, "→", 1, if marked { active } else { label });
            hit.follow.push((at, row.kind));
        }
    }
    hit
}

/// Draws `tab` in `area` (`max`: the `hex_bytes_per_line` setting); returns the rows' rect
/// (`GroupHit::text`, for the mouse).
pub fn render(
    f: &mut Frame,
    full: Rect,
    tab: &mut HexTab,
    theme: &Theme,
    max: usize,
    overlay: Overlay,
) -> (Rect, Option<HexHit>) {
    let focused = overlay.focused;
    let (area, panel) = match overlay.inspector {
        Some(_) => split(full, tab.len(), max, overlay.grouping),
        None => (full, None),
    };
    tab.fit(area.width as usize, area.height as usize, max, overlay.grouping);
    let base = theme.get("editor");
    let offset_style = base.patch(theme.get("hexEditor.offset"));
    let border = base.patch(theme.get("hexEditor.border"));
    let dim = theme.get("hexEditor.nonPrintable");
    let selection = theme.get("editor.selection");
    let other = theme.get("editor.cursor.secondary");
    let find_highlight = theme.get("editor.findMatchHighlight");
    let find_match = theme.get("editor.findMatch");
    let pending = theme.get("hexEditor.pending");
    let out = f.buffer_mut();
    out.set_style(full, base);
    let inspected = overlay
        .inspector
        .and_then(|o| tab.inspect(o))
        .filter(|_| panel.is_some());
    let outlined: Vec<Range<usize>> = inspected.iter().flat_map(Inspected::outlines).collect();
    let inspected_style = theme.get("hexEditor.inspected");

    let layout = tab.layout;
    let n = layout.bytes_per_row();
    let len = tab.len();
    let selected = tab.selection().unwrap_or(0..0);
    let visible = tab.top..(tab.top + area.height as usize * n).min(len);
    // A match may start up to its length - 1 bytes above the rows and end as far below.
    let reach = overlay.find.map_or(0, |q| q.len().saturating_sub(1));
    let window = tab
        .doc
        .window(visible.start.saturating_sub(reach) as u64..(visible.end + reach) as u64);
    let found = Found::new(&window, overlay.find, visible, &selected);
    let mut cursor_cell = None;
    let paint = |out: &mut Cells, y: u16, cols: Range<usize>, style: Style| {
        let end = cols.end.min(area.width as usize);
        if cols.start < end {
            let rect = Rect::new(area.x + cols.start as u16, y, (end - cols.start) as u16, 1);
            out.set_style(rect, style);
        }
    };
    let rows = layout.rows(len as u64);
    for screen in 0..area.height {
        let row = tab.top_row() + screen as usize;
        if row >= rows {
            break;
        }
        let y = area.y + screen;
        let start = layout.row_offset(row, len as u64) as usize;
        out.set_stringn(
            area.x,
            y,
            hex::row_in(&window, &layout, start as u64, len as u64),
            area.width as usize,
            base,
        );
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
            match window.get(at as u64) {
                Some(hex::Cell::Byte(b)) if hex::is_printable(b) => {}
                Some(hex::Cell::Byte(_)) => {
                    paint(out, y, hex_cells.clone(), dim);
                    paint(out, y, ascii_cell.clone(), dim);
                }
                _ => {
                    paint(out, y, hex_cells.clone(), pending);
                    paint(out, y, ascii_cell.clone(), pending);
                }
            }
            // Find matches, the selection, the selected match: each run of bytes painted through
            // the gaps between their hex cells, so a run is one block.
            let layers: [(&dyn Fn(usize) -> bool, Style); 4] = [
                (&|b| found.covers(b), find_highlight),
                (&|b| selected.contains(&b), selection),
                (&|b| found.current.contains(&b), find_match),
                (&|b| outlined.iter().any(|r| r.contains(&b)), inspected_style),
            ];
            for (has, style) in layers {
                if has(at) {
                    // (A reversed group's cells run right to left: no gap to bridge.)
                    let end = if i + 1 < count && has(at + 1) && !layout.grouping().reversed() {
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
    let hit = panel.zip(overlay.inspector).map(|((rect, placement), order)| {
        render_panel(f.buffer_mut(), rect, placement, order, inspected.as_ref(), theme)
    });
    (area, hit)
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
    /// Searches only `visible` (± the query's length − 1, `hex::find_in`, in each run of loaded
    /// bytes of `window`: a match touching a page that isn't loaded yet isn't one); matches have one
    /// length, so the cover counts come from a difference array in O(bytes + matches).
    fn new(window: &hex::Window, query: Option<&hex::Query>, visible: Range<usize>, selected: &Range<usize>) -> Self {
        let mut found = Found {
            cover: Vec::new(),
            current: 0..0,
            visible: visible.clone(),
        };
        let Some(q) = query else { return found };
        let mut matches = Vec::new();
        let mut cells = window.cells.iter().enumerate().peekable();
        while let Some((i, c)) = cells.next() {
            if !matches!(c, hex::Cell::Byte(_)) {
                continue;
            }
            let mut run: Vec<u8> = vec![];
            let base = window.start as usize + i;
            let mut next = Some((i, c));
            while let Some((_, hex::Cell::Byte(b))) = next {
                run.push(*b);
                next = cells.next_if(|(_, c)| matches!(c, hex::Cell::Byte(_)));
            }
            let lo = visible.start.saturating_sub(base).min(run.len());
            let hi = visible.end.saturating_sub(base).min(run.len());
            matches.extend(
                hex::find_in(&run, q, lo..hi)
                    .into_iter()
                    .map(|m| m.start + base..m.end + base),
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::{Mode, PAGE, Query, RESIDENT_MAX, read_pages};

    #[test]
    fn a_match_over_a_loaded_and_a_pending_page_is_highlighted_once_both_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let mut data = vec![0u8; RESIDENT_MAX as usize + 1000];
        let at = PAGE as usize - 3;
        data[at..at + 6].copy_from_slice(b"NEEDLE");
        std::fs::write(&path, &data).unwrap();
        let doc = hex::read(&path).unwrap().unwrap();
        assert!(doc.resident_bytes().is_none());
        let q = Query::parse("NEEDLE", Mode::Ascii, true).unwrap();
        let range = PAGE as usize - 16..PAGE as usize + 16;
        let found = |doc: &hex::HexDoc| {
            let w = doc.window(range.start as u64..range.end as u64);
            Found::new(&w, Some(&q), range.clone(), &(0..0))
        };
        let covered = |f: &Found| range.clone().filter(|&i| f.covers(i)).count();
        assert_eq!(covered(&found(&doc)), 0, "both pending");
        // Only the page before the boundary: the match touches a pending page, so it isn't drawn.
        assert!(doc.finish_reads(doc.version(), read_pages(doc.source(), [0])));
        assert_eq!(covered(&found(&doc)), 0, "one of two pages");
        assert!(doc.finish_reads(doc.version(), read_pages(doc.source(), [1])));
        let f = found(&doc);
        assert_eq!(covered(&f), 6);
        assert!(f.covers(at) && f.covers(at + 5) && !f.covers(at + 6));
    }
}
