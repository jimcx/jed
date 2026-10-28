//! A diff tab (`diff_view::DiffTab`): a header row (the sides' titles, the number of changes),
//! then the aligned rows — two columns (old | new) when the tab is at least
//! `diff_view::SIDE_BY_SIDE_MIN` wide, else one (a change's `-` lines above its `+` lines). Each
//! side has its own line numbers; removed lines on `diffEditor.removedLineBackground`, inserted
//! ones on `insertedLineBackground`, the changed part of a changed pair (common ends trimmed)
//! on `removedTextBackground` / `insertedTextBackground`, the other side of a lone line filled
//! with `diffEditor.diagonalFill`. Each side's syntax colours (`Lines::spans`, from the load) are
//! token foregrounds under those backgrounds (VS Code), walked with one index per side over the
//! visible rows; a side without spans is plain. Each change's first row ends
//! with a ` Stage ` / ` Unstage ` button when the view stages, and an unstaged view's with a red
//! ` Revert ` on the index (left) side. The header ends with ` Open File ` (the file at the
//! cursor's line) left of the number of changes. Only the visible rows are drawn.

use std::ops::Range;

use ratatui::Frame;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::buffer::pos::{self, DisplayCol};
use crate::diff_view::{self, DiffTab, Kind, Lines, Row};
use crate::git::stage::Side;
use crate::layout::DiffHit;
use crate::syntax::Span;
use crate::theme::Theme;

/// The Revert button's label.
const REVERT: &str = " Revert ";
/// A compare tab's buttons moving a change to the right / left side.
const TO_RIGHT: &str = " → ";
const TO_LEFT: &str = " ← ";
/// The header's button opening the file (`git.toggleChanges`).
const OPEN_FILE: &str = " Open File ";

/// The colours, looked up once per frame.
struct Styles {
    base: Style,
    selection: Style,
    current: Style,
    gutter: Style,
    gutter_active: Style,
    removed: Style,
    inserted: Style,
    removed_text: Style,
    inserted_text: Style,
    fill: Style,
    control: Style,
    /// Per `syntax::KEYS` index: the token's foreground (and modifiers), never a background.
    tokens: Vec<Style>,
}

/// One side's syntax colours over the visible rows: its lines are visited in increasing order,
/// so one index walks its spans (a binary search for the first line only).
struct Walk<'a> {
    lines: &'a Lines,
    at: Option<usize>,
}

impl<'a> Walk<'a> {
    fn new(lines: &'a Lines) -> Walk<'a> {
        Walk { lines, at: None }
    }

    /// Line `line`'s text, where it starts in the side's text and the spans from the first one
    /// ending after that start.
    fn line(&mut self, line: usize) -> Line<'a> {
        let (lines, start) = (self.lines, self.lines.start(line));
        let spans = lines.spans();
        let mut i = self.at.unwrap_or_else(|| lines.first_span(line));
        while i < spans.len() && spans[i].end.0 <= start {
            i += 1;
        }
        self.at = Some(i);
        Line {
            text: lines.line(line),
            start,
            spans: &spans[i..],
        }
    }
}

/// A line to draw: its text, its start in its side's text and the spans from there on.
struct Line<'a> {
    text: &'a str,
    start: usize,
    spans: &'a [Span],
}

/// Draws the diff tab in `area`; `focused` puts the terminal cursor on the current row. Writes
/// back the layout and the rows that fit; returns the rows' rect and the buttons for the mouse.
pub fn render(f: &mut Frame, area: Rect, d: &mut DiffTab, theme: &Theme, tab_width: usize, focused: bool) -> DiffHit {
    let st = Styles {
        base: theme.get("editor"),
        current: theme.get("editor").patch(theme.get("editor.lineHighlight")),
        gutter: theme.get("gutter"),
        gutter_active: theme.get("gutter").patch(theme.get("gutter.active")),
        removed: theme.get("editor").patch(theme.get("diffEditor.removedLineBackground")),
        inserted: theme
            .get("editor")
            .patch(theme.get("diffEditor.insertedLineBackground")),
        removed_text: theme.get("diffEditor.removedTextBackground"),
        inserted_text: theme.get("diffEditor.insertedTextBackground"),
        fill: theme.get("editor").patch(theme.get("diffEditor.diagonalFill")),
        control: theme.get("editor.whitespace.control"),
        selection: theme.get("editor.selection"),
        tokens: crate::syntax::KEYS
            .iter()
            .map(|k| Style {
                bg: None,
                ..theme.get(k)
            })
            .collect(),
    };
    let out = f.buffer_mut();
    out.set_style(area, st.base);
    let mut hit = DiffHit::default();
    if area.height == 0 || area.width < 4 {
        return hit;
    }
    let inline = area.width < diff_view::SIDE_BY_SIDE_MIN;
    d.set_layout(inline);
    d.tab_width = tab_width;
    d.sync_caret();
    let caret = d.caret;
    let body = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    d.scroll_to_cursor(body.height as usize);
    hit.rows = body;

    // The header: the sides' titles over their columns, the number of changes at the right.
    let header = theme.get("diffEditor.header");
    out.set_style(Rect { height: 1, ..area }, header);
    let changes = d.doc.as_ref().map(|doc| doc.hunks.len());
    // The last load's failure (the old diff stays shown), else the number of changes.
    let summary = match (changes, &d.error) {
        (Some(_), Some(e)) => format!(" ⚠ {e} "),
        (Some(1), None) => " 1 change ".to_string(),
        (Some(n), None) => format!(" {n} changes "),
        (None, _) => String::new(),
    };
    let (left_title, right_title) = d.titles();
    let half = (area.width - 1) / 2;
    if inline {
        let title = format!(" {left_title} ↔ {right_title}");
        out.set_stringn(area.x, area.y, &title, area.width as usize, header);
    } else {
        out.set_stringn(area.x, area.y, format!(" {left_title}"), half as usize, header);
        out.set_stringn(
            area.x + half + 1,
            area.y,
            format!(" {right_title}"),
            half as usize,
            header,
        );
    }
    let sw = summary.width() as u16;
    if sw + 20 < area.width {
        out.set_stringn(area.x + area.width - sw, area.y, &summary, sw as usize, header);
    }
    // Open File left of the summary (VS Code's diff editor toolbar), over the right title's end.
    let ow = OPEN_FILE.width() as u16;
    let right_end = area.x + area.width - if sw + 20 < area.width { sw } else { 0 };
    if d.compare.is_none() && right_end >= area.x + ow + 20 {
        let rect = Rect::new(right_end - ow, area.y, ow, 1);
        out.set_stringn(
            rect.x,
            rect.y,
            OPEN_FILE,
            ow as usize,
            header.patch(theme.get("quickDiff.action")),
        );
        hit.open_file = Some(rect);
    }

    let message = |out: &mut Cells, text: &str| {
        if body.height > 0 {
            out.set_stringn(
                body.x + 1,
                body.y,
                text,
                body.width as usize - 1,
                theme.get("sideBar.dim"),
            );
        }
    };
    let Some(doc) = d.doc.clone() else {
        message(out, d.error.as_deref().unwrap_or("Loading…"));
        return hit;
    };
    if doc.binary {
        message(out, "Binary file: the diff isn't shown");
        return hit;
    }
    let aligned = doc.aligned(inline);
    if aligned.rows.is_empty() {
        message(out, "Both sides are empty");
        return hit;
    }
    let top = d.top;
    let shown = &aligned.rows[top..(top + body.height as usize).min(aligned.rows.len())];
    let digits = doc.left.len().max(doc.right.len()).to_string().len().max(3);
    let button = match d.pair.staging() {
        Some(Side::Unstaged) => Some(" Stage "),
        Some(Side::Staged) => Some(" Unstage "),
        None => None,
    };
    // Revert: only working tree changes against the index.
    let revertible = d.pair == diff_view::Pair::UNSTAGED;
    let compare = d.compare.is_some();
    let starts = &aligned.starts;
    let mut next_start = starts.partition_point(|&r| r < top);
    let mut cursor_cell = None;
    let (mut left_walk, mut right_walk) = (Walk::new(&doc.left), Walk::new(&doc.right));
    // The text's columns (as `draw_inline` / `draw_half` lay them out; side by side the narrower
    // side) and the widest line drawn, for sideways scrolling.
    d.text_width = if inline {
        body.width.saturating_sub(2 * digits as u16 + 4)
    } else {
        let room = |w: u16| w.saturating_sub(digits as u16 + 2);
        room(half).min(room(body.width - half - 1))
    } as usize;
    hit.text_x = if inline {
        [body.x + 2 * digits as u16 + 4; 2]
    } else {
        [body.x + digits as u16 + 2, body.x + half + 1 + digits as u16 + 2]
    };
    hit.divider = (!inline).then_some(body.x + half);
    let mut widest = 0;
    for (i, row) in shown.iter().enumerate() {
        let index = top + i;
        let y = body.y + i as u16;
        let current = index == d.cursor;
        let at_caret;
        let parts = (row.kind() == Kind::Changed)
            .then(|| diff_view::changed_parts(doc.left.line(row.left.unwrap()), doc.right.line(row.right.unwrap())));
        // The row's width and the widths of the lines under the Stage and the Revert buttons.
        let (_, w, under) = if inline {
            let line = match row.kind() {
                Kind::Removed => left_walk.line(row.left.unwrap_or_default()),
                _ => right_walk.line(row.right.unwrap_or_default()),
            };
            let (right, l) = (row.kind() != Kind::Removed, line_no(row));
            let sel = caret.on_line(right, l, line.text.len());
            let (cell, w) = draw_inline(
                out, body, y, row, line, digits, d.left_col, tab_width, &st, current, sel,
            );
            at_caret = (right == caret.right).then_some(cell);
            (cell, w, (w, w))
        } else {
            let left = Rect::new(body.x, y, half, 1);
            let right = Rect::new(body.x + half + 1, y, body.width - half - 1, 1);
            let side = |rect, line, removed: bool, part, sel| {
                let ctx = Half {
                    rect,
                    sel,
                    line,
                    digits,
                    left_col: d.left_col,
                    tab_width,
                    current,
                };
                (ctx, removed, part)
            };
            let left_line = row.left.map(|l| (l, left_walk.line(l)));
            let right_line = row.right.map(|l| (l, right_walk.line(l)));
            let on = |right: bool, l: Option<usize>| {
                let lines = if right { &doc.right } else { &doc.left };
                l.and_then(|l| caret.on_line(right, l, lines.line(l).len()))
            };
            let (l, lr, lp) = side(
                left,
                left_line,
                true,
                parts.as_ref().map(|p| p.0.clone()),
                on(false, row.left),
            );
            let (r, rr, rp) = side(right, right_line, false, parts.map(|p| p.1), on(true, row.right));
            let at_left = draw_half(out, &l, row, lr, lp, &st);
            let at_right = draw_half(out, &r, row, rr, rp, &st);
            out.set_stringn(body.x + half, y, "│", 1, theme.get("editorGroup.border"));
            let cell = if row.right.is_some() { at_right.0 } else { at_left.0 };
            at_caret = Some(if caret.right { at_right.0 } else { at_left.0 });
            (cell, at_left.1.max(at_right.1), (at_right.1, at_left.1))
        };
        widest = widest.max(w);
        if current {
            // The caret: its column in its side's text, when that is in view.
            let text = if caret.right {
                doc.right.line(caret.head.0)
            } else {
                doc.left.line(caret.head.0)
            };
            let col = pos::col_before_in(text, caret.head.1, tab_width).0;
            cursor_cell = match at_caret {
                Some((x, y)) if col >= d.left_col && col - d.left_col < d.text_width.max(1) => {
                    Some((x + (col - d.left_col) as u16, y))
                }
                _ => None,
            };
        }
        // The change's buttons on its first row: Stage / Unstage at the row's right end; Revert
        // (an unstaged view) at the left column's right end, or left of Stage inline.
        if let Some(label) = button
            && starts.get(next_start) == Some(&index)
        {
            next_start += 1;
            let k = row.hunk.unwrap_or_default();
            let w = label.width() as u16;
            let rw = REVERT.width() as u16;
            if body.width > w + 24 {
                let rect = Rect::new(body.x + body.width - w, y, w, 1);
                out.set_stringn(rect.x, y, label, w as usize, theme.get("quickDiff.action"));
                hit.buttons.push((rect, k));
                // The button covers the line's last columns (the right side's): scrolling
                // reaches past them.
                widest = widest.max(under.0 + w as usize);
            }
            let revert = match (revertible, inline) {
                (false, _) => None,
                (true, false) if half > rw + 12 => Some(Rect::new(body.x + half - rw, y, rw, 1)),
                (true, true) if body.width > w + rw + 25 => Some(Rect::new(body.x + body.width - w - rw - 1, y, rw, 1)),
                _ => None,
            };
            if let Some(rect) = revert {
                out.set_stringn(rect.x, y, REVERT, rw as usize, theme.get("diffEditor.revertAction"));
                hit.reverts.push((rect, k));
                // Side by side over the left side's end; inline left of Stage, over both.
                let over = if inline { w + rw + 1 } else { rw };
                widest = widest.max(under.1 + over as usize);
            }
        } else if compare && starts.get(next_start) == Some(&index) {
            next_start += 1;
            let k = row.hunk.unwrap_or_default();
            let aw = TO_RIGHT.width() as u16;
            let key = theme.get("diffEditor.revertAction");
            // `←` then `→`: side by side at the left column's right end, next to the divider (the
            // right column's line number stays visible); inline at the row's right end.
            let spots = if inline {
                (body.width > 2 * aw + 25).then(|| {
                    let x = body.x + body.width - aw;
                    (x, x - aw - 1)
                })
            } else {
                (half > 2 * aw + 12 && body.width - half - 1 > 12).then(|| {
                    let x = body.x + half - aw;
                    (x, x - aw)
                })
            };
            if let Some((right_x, left_x)) = spots {
                for (x, label, to_right) in [(right_x, TO_RIGHT, true), (left_x, TO_LEFT, false)] {
                    let rect = Rect::new(x, y, aw, 1);
                    out.set_stringn(x, y, label, aw as usize, key);
                    hit.moves.push((rect, k, to_right));
                }
                // Over the end of the line under them: inline both lines', else the left side's.
                let over = if inline { 2 * aw + 1 } else { 2 * aw };
                widest = widest.max(under.1 + over as usize);
            }
        }
    }
    d.scroll_width.measure(widest);
    if focused && let Some((x, y)) = cursor_cell {
        f.set_cursor_position(Position { x, y });
    }
    hit
}

/// One side of a side-by-side row.
struct Half<'a> {
    rect: Rect,
    /// The line's number and what to draw of it; `None`: filler.
    line: Option<(usize, Line<'a>)>,
    digits: usize,
    left_col: usize,
    tab_width: usize,
    current: bool,
    /// The selected bytes of the line, and whether its line break is selected.
    sel: Option<(Range<usize>, bool)>,
}

/// Draws one side of a row (its number, then its text; filler when it has no line). Returns
/// where its text starts and the line's width (0 for filler).
fn draw_half(
    out: &mut Cells,
    h: &Half,
    row: &Row,
    removed: bool,
    part: Option<Range<usize>>,
    st: &Styles,
) -> ((u16, u16), usize) {
    let Rect { x, y, width, .. } = h.rect;
    let text_x = x + (h.digits as u16 + 2).min(width);
    let Some((line, text)) = &h.line else {
        out.set_stringn(x, y, "╱".repeat(width as usize), width as usize, st.fill);
        return ((text_x, y), 0);
    };
    let bg = match row.kind() {
        Kind::Same if h.current => st.current,
        Kind::Same => st.base,
        _ if removed => st.removed,
        _ => st.inserted,
    };
    out.set_stringn(x, y, " ".repeat(width as usize), width as usize, bg);
    let number = if h.current { st.gutter_active } else { st.gutter };
    let number = number.bg(bg.bg.unwrap_or_default());
    out.set_stringn(x, y, format!("{:>w$} ", line + 1, w = h.digits), width as usize, number);
    let highlight = part.map(|p| (p, if removed { st.removed_text } else { st.inserted_text }));
    let room = width.saturating_sub(text_x - x);
    let w = draw_text(
        out,
        (text_x, y, room),
        text,
        h.left_col,
        h.tab_width,
        bg,
        highlight,
        h.sel.clone(),
        st,
    );
    ((text_x, y), w)
}

/// Draws a row of the one-column layout: both line numbers, `-` / `+`, the text. Returns where
/// its text starts and the line's width.
#[allow(clippy::too_many_arguments)]
fn draw_inline(
    out: &mut Cells,
    body: Rect,
    y: u16,
    row: &Row,
    line: Line,
    digits: usize,
    left_col: usize,
    tab_width: usize,
    st: &Styles,
    current: bool,
    sel: Option<(Range<usize>, bool)>,
) -> ((u16, u16), usize) {
    let (bg, sign) = match row.kind() {
        Kind::Removed => (st.removed, "-"),
        Kind::Added | Kind::Changed => (st.inserted, "+"),
        Kind::Same if current => (st.current, " "),
        Kind::Same => (st.base, " "),
    };
    let width = body.width as usize;
    out.set_stringn(body.x, y, " ".repeat(width), width, bg);
    let number = if current { st.gutter_active } else { st.gutter };
    let number = number.bg(bg.bg.unwrap_or_default());
    let n = |l: Option<usize>| l.map_or(String::new(), |l| (l + 1).to_string());
    let gutter = format!("{:>w$} {:>w$} ", n(row.left), n(row.right), w = digits);
    let (gx, _) = out.set_stringn(body.x, y, &gutter, width, number);
    let (sx, _) = out.set_stringn(
        gx,
        y,
        format!("{sign} "),
        width.saturating_sub((gx - body.x) as usize),
        bg,
    );
    let room = body.width.saturating_sub(sx - body.x);
    let w = draw_text(out, (sx, y, room), &line, left_col, tab_width, bg, None, sel, st);
    ((sx, y), w)
}

/// A line's text from display column `left_col`, `width` cells at most, through
/// `pos::display_grapheme` (tabs, control characters) as the editor draws it: the token colours
/// of its spans over `style`, then `highlight`'s background over a byte range. Returns the line's
/// whole width (the part past the edge only measured).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    out: &mut Cells,
    (x, y, width): (u16, u16, u16),
    line: &Line,
    left_col: usize,
    tab_width: usize,
    style: Style,
    highlight: Option<(Range<usize>, Style)>,
    sel: Option<(Range<usize>, bool)>,
    st: &Styles,
) -> usize {
    let width = width as usize;
    let mut col = 0usize;
    let mut k = 0;
    for (off, g) in line.text.grapheme_indices(true) {
        let (shown, w) = pos::display_grapheme(g, DisplayCol(col), tab_width);
        if col < left_col {
            col += w;
            continue;
        }
        if col + w > left_col + width {
            return pos::col_after(&line.text[off..], DisplayCol(col), tab_width).0;
        }
        let mut s = style;
        let at = line.start + off;
        while k < line.spans.len() && line.spans[k].end.0 <= at {
            k += 1;
        }
        if let Some(span) = line.spans.get(k)
            && span.start.0 <= at
        {
            s = s.patch(st.tokens[span.highlight.index()]);
        }
        if let Some((range, h)) = &highlight
            && range.contains(&off)
        {
            s = s.patch(*h);
        }
        if g.starts_with(|c: char| c.is_control()) && g != "\t" {
            s = s.patch(st.control);
        }
        if let Some((range, _)) = &sel
            && range.contains(&off)
        {
            s = s.patch(st.selection);
        }
        out.set_stringn(x + (col - left_col) as u16, y, &shown, w, s);
        col += w;
    }
    // A selected line break: one cell past the text.
    if let Some((_, true)) = sel
        && col >= left_col
        && col - left_col < width
    {
        out.set_stringn(x + (col - left_col) as u16, y, " ", 1, style.patch(st.selection));
    }
    col
}

/// The line number a row draws inline.
fn line_no(row: &Row) -> usize {
    match row.kind() {
        Kind::Removed => row.left.unwrap_or_default(),
        _ => row.right.unwrap_or_default(),
    }
}
