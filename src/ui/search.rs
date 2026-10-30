//! The Search view, in the side panel below its title: the query input with the case, whole
//! word and regex toggles (`Aa ab .*`) and the details button (`…`), the files to include / exclude inputs under it
//! while the details show, an invalid regex's or glob's message under them, a summary ("12 results in 3 files",
//! "Searching…", "No results found", the truncated note), then the results: a row per file
//! (chevron, name, dimmed folder, match count) followed, unless it's collapsed, by its matching
//! lines with the matches highlighted. The hovered row shows a `×` (dismiss) at its right end,
//! and while the replace input shows a replace button (`⇄`: that line's matches, or the file's)
//! before it.
//! The replace toggle (`▸` / `▾`) sits in the margin left of the query; while it's open the
//! replace input is under the query with a Replace All button (`⇄ All`) beside it, and while
//! that's non-empty each drawn line shows its
//! matches struck through, each followed by its replacement (computed for the drawn rows only).
//! Only the rows on screen are looked at.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::find::draw_field;
use super::scrollbar;
use crate::app::{App, Focus};
use crate::buffer::pos::{self, DisplayCol};
use crate::layout::{ScrollBarHit, ScrollTarget, SearchHit, SidePanel};
use crate::scrollbar::Scrollable;
use crate::search::{Matcher, Replacement};
use crate::search_view::{self, Input, Row};
use crate::workspace_search::{FileMatches, LineMatch, MAX_MATCHES, Results};

const TOGGLES: [&str; 4] = ["Aa", "ab", ".*", "Ig"];
/// The Replace All button beside the replace input, and a hovered row's replace button.
const REPLACE_ALL: &str = "⇄ All";
const REPLACE_ROW: &str = "⇄";
/// Cells a result line is indented by.
const LINE_INDENT: u16 = 3;

/// Draws the view in `area` (the side panel below its title, without its border), and the
/// results' scroll bar in the list's last column (the query and toggles keep the full width).
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> (Option<SearchHit>, Option<ScrollBarHit>) {
    if area.width < 10 || area.height < 2 {
        return (None, None);
    }
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::Search));
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let input = base.patch(theme.get("dialog.input"));
    let dim = base.patch(theme.get("sideBar.dim"));
    let selected = input.patch(theme.get("editor.selection"));
    let view = &mut app.search_view;
    let out = f.buffer_mut();
    let bottom = area.y + area.height;

    // " [query] Aa .* … "
    let field_w = area.width.saturating_sub(1 + 3 * TOGGLES.len() as u16 + 2 + 1).max(4);
    let field = Rect {
        x: area.x + 1,
        y: area.y,
        width: field_w,
        height: 1,
    };
    let mut inputs = [field, Rect::default(), Rect::default()];
    let first = draw_field(out, field, &view.query, "Search", input, selected, dim);
    let mut cursors = [first.cursor; 4];
    let mut input_scrolls = [first.scroll, 0, 0];
    let mut replace_scroll = 0;
    // VS Code's replace toggle in the margin left of the query.
    let replace_toggle = Rect {
        x: area.x,
        y: area.y,
        width: 1,
        height: 1,
    };
    let chevron = if view.replacing { "▾" } else { "▸" };
    out.set_stringn(area.x, area.y, chevron, 1, base.patch(theme.get("search.chevron")));
    let on = [
        view.options.case_sensitive,
        view.options.whole_word,
        view.options.regex,
        view.include_ignored,
    ];
    let mut toggles = [Rect::default(); 4];
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
    // `…`: lit while the details show, or while hidden ones still filter.
    let details = Rect {
        x: field.x + field_w + 1 + 3 * TOGGLES.len() as u16,
        y: area.y,
        width: 1,
        height: 1,
    };
    let key = if view.details || view.hidden_filter() {
        "find.option.active"
    } else {
        "find.option"
    };
    out.set_stringn(details.x, details.y, "…", 1, base.patch(theme.get(key)));
    let mut y = area.y + 1;
    let mut replace = Rect::default();
    let mut replace_all = Rect::default();
    if view.replacing && y < bottom {
        // As wide as the query; Replace All where the query's toggles are.
        replace = Rect { y, ..field };
        let drawn = draw_field(out, replace, &view.replace, "Replace", input, selected, dim);
        cursors[3] = drawn.cursor;
        replace_scroll = drawn.scroll;
        let x = field.x + field_w + 1;
        let w = (area.x + area.width)
            .saturating_sub(x + 1)
            .min(REPLACE_ALL.width() as u16);
        replace_all = Rect {
            x,
            y,
            width: w,
            height: 1,
        };
        out.set_stringn(x, y, REPLACE_ALL, w as usize, base.patch(theme.get("search.action")));
        y += 1;
    }
    if view.details {
        let fields = [(&view.include, "files to include"), (&view.exclude, "files to exclude")];
        for (i, (text, placeholder)) in fields.into_iter().enumerate() {
            if y >= bottom {
                break;
            }
            let r = Rect {
                x: area.x + 1,
                y,
                width: area.width.saturating_sub(2),
                height: 1,
            };
            let drawn = draw_field(out, r, text, placeholder, input, selected, dim);
            cursors[i + 1] = drawn.cursor;
            input_scrolls[i + 1] = drawn.scroll;
            inputs[i + 1] = r;
            y += 1;
        }
    }
    let cursor = match view.input {
        Input::Query => cursors[0],
        Input::Include => cursors[1],
        Input::Exclude => cursors[2],
        Input::Replace => cursors[3],
    };
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
        details,
        inputs,
        input_scrolls,
        replace_toggle,
        replace,
        replace_scroll,
        list: Rect::default(),
        rows: Vec::new(),
        dismiss: Vec::new(),
        chevrons: Vec::new(),
        replace_row: Vec::new(),
        replace_all,
    };
    let Some(results) = results else {
        return (Some(hit), None);
    };
    let mut list = Rect {
        x: area.x,
        y: (y + 1).min(bottom),
        width: area.width,
        height: bottom.saturating_sub(y + 1),
    };
    let bar = scrollbar::take_column(&mut list);
    view.list.set_height(&results.files, list.height.max(1) as usize);
    // Clamp (the list may have shrunk), keeping the same row on top.
    let top = view.scroll(results);
    view.set_scroll(results, top);
    let top = view.scroll(results);
    let selected_row = view.selected_row(results);
    let out = f.buffer_mut();
    let scroll = view.list.scrollable(&results.files).scroll();
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme));
    let styles = Styles {
        base,
        dim,
        name: base.patch(theme.get("search.fileName")),
        count: base.patch(theme.get("search.count")),
        matched: base.patch(theme.get("search.match")),
        action: base.patch(theme.get("search.action")),
        chevron: base.patch(theme.get("search.chevron")),
    };
    let with = view.replacement(results);
    let preview = with
        .as_ref()
        .zip(results.matcher.as_ref())
        .map(|(with, matcher)| ReplacePreview {
            matcher,
            with,
            removed: base.patch(theme.get("search.replace.removed")),
            inserted: base.patch(theme.get("search.replace.inserted")),
        });
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
        action: styles.action.patch(bg_of(selection)),
        chevron: styles.chevron.patch(bg_of(selection)),
    };
    let shown = search_view::rows(results, top, list.height as usize);
    for (i, row) in shown.iter().enumerate() {
        let y = list.y + i as u16;
        let is_selected = selected_row == Some(top + i);
        let s = if is_selected {
            out.set_style(Rect { y, height: 1, ..list }, on_selection.base);
            &on_selection
        } else {
            &styles
        };
        let key = row.key();
        // The `×` in the row's second-last cell (the last stays blank, as the count's edge).
        let dismiss = (list.width > 12 && view.hover.as_ref() == Some(&key)).then(|| Rect {
            x: list.x + list.width - 2,
            y,
            width: 1,
            height: 1,
        });
        // While the replace input shows, a replace button two cells left of it.
        let replace_row = dismiss.filter(|_| view.replacing).map(|r| Rect { x: r.x - 2, ..r });
        let right = match (dismiss, replace_row) {
            (_, Some(_)) => 4,
            (Some(_), None) => 2,
            _ => 0,
        };
        match row.row() {
            Row::File(file) => {
                out.set_stringn(list.x + 1, y, if file.collapsed { "▸" } else { "▾" }, 1, s.chevron);
                let chevron = Rect {
                    x: list.x,
                    y,
                    width: 2,
                    height: 1,
                };
                hit.chevrons.push((chevron, file.path.clone()));
                draw_file(out, list.x + 2, y, list.width.saturating_sub(2 + right), file, s);
            }
            Row::Line(m) => draw_line(
                out,
                list.x + LINE_INDENT,
                y,
                list.width.saturating_sub(LINE_INDENT + 1 + right),
                m,
                (s.base, s.dim, s.matched),
                preview.as_ref(),
            ),
        }
        if let Some(r) = replace_row {
            out.set_stringn(r.x, r.y, REPLACE_ROW, 1, s.action);
            hit.replace_row.push((r, key.clone()));
        }
        if let Some(r) = dismiss {
            out.set_stringn(r.x, r.y, "×", 1, s.action);
            hit.dismiss.push((r, key));
        }
    }
    hit.list = list;
    hit.rows = shown.iter().map(|r| r.key()).collect();
    (Some(hit), bar)
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
    action: Style,
    chevron: Style,
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

/// A result line's replace preview: each match struck through (`removed`), then what replaces it
/// (`inserted`).
pub(super) struct ReplacePreview<'a> {
    pub matcher: &'a Matcher,
    pub with: &'a Replacement,
    pub removed: Style,
    pub inserted: Style,
}

/// One matching line in `width` cells, its matches highlighted (or, with `replace`, struck
/// through and followed by their replacements); "…" where text is cut. Also the Locations list's
/// previews (`ui::panel`).
pub(super) fn draw_line(
    out: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    m: &LineMatch,
    (base, dim, matched): (Style, Style, Style),
    replace: Option<&ReplacePreview>,
) {
    let width = width as usize;
    if width < 2 {
        return;
    }
    let p = search_view::preview(m, width);
    let mut col = 0;
    if p.cut {
        out.set_stringn(x, y, "…", 1, dim);
        col = 1;
    }
    let replacements = replace.map(|r| search_view::replacements(m, &p, r.matcher, r.with));
    // The text in styled pieces: plain, a match, its replacement, ….
    let mut pieces: Vec<(&str, Style)> = Vec::new();
    let mut at = 0;
    for (k, r) in p.ranges.iter().enumerate() {
        if r.start < at {
            continue;
        }
        pieces.push((&p.text[at..r.start], base));
        match (&replacements, replace) {
            (Some(reps), Some(look)) => {
                pieces.push((&p.text[r.start..r.end], look.removed));
                pieces.push((&reps[k], look.inserted));
            }
            _ => pieces.push((&p.text[r.start..r.end], matched)),
        }
        at = r.end;
    }
    pieces.push((&p.text[at..], base));
    let graphemes: Vec<(&str, Style)> = pieces
        .iter()
        .flat_map(|&(text, style)| text.graphemes(true).map(move |g| (g, style)))
        .collect();
    for (k, &(g, style)) in graphemes.iter().enumerate() {
        let (shown, w) = pos::display_grapheme(g, DisplayCol(0), 1);
        let last = k + 1 == graphemes.len();
        // Keep a cell for "…" unless this is the last grapheme.
        if col + w > width || (!last && col + w == width) {
            out.set_stringn(x + col as u16, y, "…", 1, dim);
            break;
        }
        out.set_stringn(x + col as u16, y, &shown, w, style);
        col += w;
    }
}
