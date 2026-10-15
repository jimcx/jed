//! The Search view, in the side panel below its title: the query input with the case and regex
//! toggles (`Aa .*`), an invalid regex's message under it, a summary ("12 results in 3 files",
//! "Searching…", "No results found", the truncated note), then the results: a row per file
//! (name, dimmed folder, match count) followed by its matching lines with the matches
//! highlighted. Only the rows on screen are looked at.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::find::draw_field;
use crate::app::{App, Focus};
use crate::buffer::pos::{self, DisplayCol};
use crate::layout::SearchHit;
use crate::search_view::{self, Row};
use crate::workspace_search::{FileMatches, LineMatch, MAX_MATCHES, Results};

const TOGGLES: [&str; 2] = ["Aa", ".*"];
/// Cells a result line is indented by.
const LINE_INDENT: u16 = 3;

/// Draws the view in `area` (the side panel below its title, without its border).
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> Option<SearchHit> {
    if area.width < 10 || area.height < 2 {
        return None;
    }
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let input = base.patch(theme.get("dialog.input"));
    let dim = base.patch(theme.get("sideBar.dim"));
    let selected = input.patch(theme.get("editor.selection"));
    let view = &mut app.search_view;
    let out = f.buffer_mut();
    let bottom = area.y + area.height;

    // " [query] Aa .* "
    let field_w = area.width.saturating_sub(1 + 1 + 3 * TOGGLES.len() as u16).max(4);
    let field = Rect {
        x: area.x + 1,
        y: area.y,
        width: field_w,
        height: 1,
    };
    let cursor = draw_field(out, field, &view.query, "Search", input, selected, dim);
    let on = [view.options.case_sensitive, view.options.regex];
    let mut toggles = [Rect::default(); 2];
    for (i, label) in TOGGLES.iter().enumerate() {
        let x = field.x + field_w + 1 + 3 * i as u16;
        let key = if on[i] { "find.option.active" } else { "find.option" };
        out.set_stringn(x, area.y, label, 2, base.patch(theme.get(key)));
        toggles[i] = Rect {
            x,
            y: area.y,
            width: 2,
            height: 1,
        };
    }
    let mut y = area.y + 1;
    let text_w = area.width.saturating_sub(2) as usize;
    let line = |out: &mut ratatui::buffer::Buffer, y: &mut u16, text: &str, style: Style| {
        if *y < bottom {
            out.set_stringn(area.x + 1, *y, text, text_w, style);
            *y += 1;
        }
    };
    if let Some(e) = &view.error {
        line(out, &mut y, &e.to_string(), base.patch(theme.get("search.error")));
    }
    let results = app.search.as_ref().filter(|_| view.error.is_none());
    let pending = view.deadline().is_some();
    match results {
        Some(r) if !r.query.pattern.is_empty() || pending => {
            line(out, &mut y, &summary(r, pending), dim);
            if r.truncated {
                let note = format!("Results truncated at {} matches", thousands(MAX_MATCHES));
                line(out, &mut y, &note, base.patch(theme.get("search.warning")));
            }
        }
        None if pending => line(out, &mut y, "Searching…", dim),
        _ => {}
    }

    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    if focused && !view.list_focused {
        f.set_cursor_position(cursor);
    }
    let mut hit = SearchHit {
        toggles,
        list: Rect::default(),
        rows: Vec::new(),
    };
    let Some(results) = results else { return Some(hit) };
    let list = Rect {
        x: area.x,
        y: (y + 1).min(bottom),
        width: area.width,
        height: bottom.saturating_sub(y + 1),
    };
    view.height = list.height.max(1) as usize;
    // Clamp (the list may have shrunk), keeping the same row on top.
    let top = view.scroll(results);
    view.set_scroll(results, top);
    let top = view.scroll(results);
    let selected_row = view.selected_row(results);
    let out = f.buffer_mut();
    let styles = Styles {
        base,
        dim,
        name: base.patch(theme.get("search.fileName")),
        count: base.patch(theme.get("search.count")),
        matched: base.patch(theme.get("search.match")),
    };
    // The selected row as the explorer draws its own: brighter while the list has the keys.
    let selection = if focused && view.list_focused {
        theme.get("list.active")
    } else {
        theme.get("list.inactive")
    };
    let on_selection = Styles {
        base: base.patch(selection),
        dim: dim.patch(bg_of(selection)),
        name: styles.name.patch(bg_of(selection)),
        count: styles.count,
        matched: styles.matched,
    };
    let shown = search_view::rows(results, top, list.height as usize);
    for (i, row) in shown.iter().enumerate() {
        let y = list.y + i as u16;
        let s = if selected_row == Some(top + i) {
            out.set_style(Rect { y, height: 1, ..list }, on_selection.base);
            &on_selection
        } else {
            &styles
        };
        match row.row() {
            Row::File(file) => draw_file(out, list.x, y, list.width, file, s),
            Row::Line(m) => draw_line(
                out,
                list.x + LINE_INDENT,
                y,
                list.width.saturating_sub(LINE_INDENT + 1),
                m,
                s,
            ),
        }
    }
    hit.list = list;
    hit.rows = shown.iter().map(|r| r.key()).collect();
    Some(hit)
}

/// Just the background of `style` (so text keeps its own colour on a selected row).
fn bg_of(style: Style) -> Style {
    Style {
        bg: style.bg,
        ..Style::default()
    }
}

struct Styles {
    base: Style,
    dim: Style,
    name: Style,
    count: Style,
    matched: Style,
}

/// "12 results in 3 files", "No results found", with "Searching…" while running or about to.
fn summary(r: &Results, pending: bool) -> String {
    let plural = |n: usize, word: &str| format!("{} {word}{}", thousands(n), if n == 1 { "" } else { "s" });
    let counts = format!("{} in {}", plural(r.total, "result"), plural(r.files.len(), "file"));
    match (r.done && !pending, r.total) {
        (true, 0) => "No results found".into(),
        (true, _) => counts,
        (false, 0) => "Searching…".into(),
        (false, _) => format!("Searching… {counts}"),
    }
}

/// 1234567 → "1,234,567".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// " name  folder        12 ": the count right-aligned; the folder gives way first.
fn draw_file(out: &mut ratatui::buffer::Buffer, x: u16, y: u16, width: u16, file: &FileMatches, s: &Styles) {
    let name = file
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let dir = file.path.parent().map_or_else(String::new, |d| d.display().to_string());
    let count = format!(" {} ", thousands(file.count()));
    let count_w = count.width() as u16;
    let room = width.saturating_sub(count_w + 2) as usize;
    let (nx, _) = out.set_stringn(x + 1, y, &name, room, s.name);
    let used = (nx - x - 1) as usize;
    if !dir.is_empty() && used + 2 < room {
        out.set_stringn(nx + 1, y, &dir, room - used - 1, s.dim);
    }
    if width > count_w + 1 {
        out.set_stringn(x + width - count_w - 1, y, &count, count_w as usize, s.count);
    }
}

/// One matching line in `width` cells, its matches highlighted; "…" where text is cut.
fn draw_line(out: &mut ratatui::buffer::Buffer, x: u16, y: u16, width: u16, m: &LineMatch, s: &Styles) {
    let width = width as usize;
    if width < 2 {
        return;
    }
    let p = search_view::preview(m, width);
    let mut col = 0;
    if p.cut {
        out.set_stringn(x, y, "…", 1, s.dim);
        col = 1;
    }
    let mut ranges = p.ranges.iter().peekable();
    let graphemes: Vec<(usize, &str)> = p.text.grapheme_indices(true).collect();
    for (k, &(off, g)) in graphemes.iter().enumerate() {
        let (shown, w) = pos::display_grapheme(g, DisplayCol(0), 1);
        let last = k + 1 == graphemes.len();
        // Keep a cell for "…" unless this is the last grapheme.
        if col + w > width || (!last && col + w == width) {
            out.set_stringn(x + col as u16, y, "…", 1, s.dim);
            break;
        }
        while ranges.peek().is_some_and(|r| r.end <= off) {
            ranges.next();
        }
        let hit = ranges.peek().is_some_and(|r| r.start <= off && off < r.end);
        let style = if hit { s.matched } else { s.base };
        out.set_stringn(x + col as u16, y, &shown, w, style);
        col += w;
    }
}
