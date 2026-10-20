//! Side panel: explorer tree, the Search view (`ui::search`), the Source Control view
//! (`ui::scm`) or the Tools view (`ui::tools`). The Explorer's body is split vertically: the
//! tree above, then the draggable divider and header of the "Git History" section, and its
//! commit rows (`ui::side`'s `render_history`) — each with its own scroll bar.

use ratatui::Frame;
use ratatui::buffer::Buffer as Cells;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::scrollbar;
use crate::app::{App, Focus};
use crate::git::log::Commit;
use crate::history;
use crate::layout::{HistoryHit, ScmHit, ScrollBarHit, ScrollTarget, SearchHit, SidePanel, ToolsHit};
use crate::scrollbar::{Scroll, Scrollable};
use crate::theme::Theme;

/// The summary keeps at least this many cells before the author is dropped from the suffix.
const MIN_SUMMARY: usize = 8;

/// What the side panel drew, for hit-testing.
#[derive(Default)]
pub struct SideHits {
    /// The area holding explorer rows.
    pub explorer_rows: Rect,
    pub search: Option<SearchHit>,
    pub scm: Option<ScmHit>,
    pub tools: Option<ToolsHit>,
    /// The Explorer's "Git History" section, when shown.
    pub history: Option<HistoryHit>,
    /// The shown list's scroll bar (its last column).
    pub scrollbar: Option<(ScrollTarget, ScrollBarHit)>,
    /// The history list's scroll bar (its last column).
    pub history_scrollbar: Option<ScrollBarHit>,
}

/// Draws the panel.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> SideHits {
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let out = f.buffer_mut();
    out.set_style(area, base);

    // Border on the right edge; dragging it resizes the panel.
    let border = base.patch(theme.get("sideBar.border"));
    let bx = area.x + area.width - 1;
    for y in area.y..area.y + area.height {
        out.set_stringn(bx, y, "│", 1, border);
    }
    let inner_w = area.width.saturating_sub(1);
    let title = base.patch(theme.get("sideBar.title"));
    out.set_stringn(
        area.x,
        area.y,
        format!("  {}", app.ui.side_panel.title()),
        inner_w as usize,
        title,
    );

    let body = Rect {
        x: area.x,
        y: area.y + 2,
        width: inner_w,
        height: area.height.saturating_sub(2),
    };
    let view = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..body
    };
    match app.ui.side_panel {
        SidePanel::Search => {
            let (search, bar) = super::search::render(f, view, app);
            return SideHits {
                search,
                scrollbar: bar.map(|b| (ScrollTarget::Side(SidePanel::Search), b)),
                ..SideHits::default()
            };
        }
        SidePanel::SourceControl => {
            let (scm, bar) = super::scm::render(f, view, app);
            return SideHits {
                scm,
                scrollbar: bar.map(|b| (ScrollTarget::Side(SidePanel::SourceControl), b)),
                ..SideHits::default()
            };
        }
        SidePanel::Tools => {
            let (tools, bar) = super::tools::render(f, view, app);
            return SideHits {
                tools,
                scrollbar: bar.map(|b| (ScrollTarget::Side(SidePanel::Tools), b)),
                ..SideHits::default()
            };
        }
        SidePanel::Explorer => {}
    }

    let header = base.patch(theme.get("sideBar.header"));
    let name = app.workspace.name().to_uppercase();
    out.set_stringn(area.x, area.y + 1, format!(" ▾ {name}"), inner_w as usize, header);

    // The body splits into the tree and the "Git History" section (which hides when there's no
    // room for both).
    let (mut tree, section) = match app.git_history.split(body.height) {
        Some((tree_h, section_h)) => (
            Rect { height: tree_h, ..body },
            Some(Rect {
                y: body.y + tree_h,
                height: section_h,
                ..body
            }),
        ),
        None => (body, None),
    };
    // The rows keep their last column for the scroll bar.
    let bar = scrollbar::take_column(&mut tree);
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::Explorer));
    let explorer = &mut app.explorer;
    explorer.set_height(tree.height.max(1) as usize);
    let count = explorer.row_count();
    explorer.scroll = explorer.scroll.min(count.saturating_sub(explorer.height));
    let scroll = Scroll::rows(count, explorer.height, explorer.scroll);
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme));
    let rows = explorer.rows();
    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };

    for (i, row) in rows.iter().skip(explorer.scroll).take(tree.height as usize).enumerate() {
        let y = tree.y + i as u16;
        let e = row.entry;
        let is_selected = explorer.selected.as_ref() == Some(&e.path);
        let style = if is_selected { base.patch(selected_style) } else { base };
        if is_selected {
            out.set_style(
                Rect {
                    x: tree.x,
                    y,
                    width: tree.width,
                    height: 1,
                },
                style,
            );
        }
        let marker = match (e.is_dir, e.expanded) {
            (true, true) => "▾ ",
            (true, false) => "▸ ",
            (false, _) => "  ",
        };
        // Git colouring (VS Code): the name in its status's colour, a changed file's letter at
        // the right edge.
        let decoration = app.git.status.as_ref().and_then(|s| s.decoration(&e.path));
        let style = match decoration {
            Some(d) => style.patch(theme.get(d.theme_key())),
            None => style,
        };
        let mut room = tree.width as usize;
        if let Some(d) = decoration
            && !e.is_dir
            && tree.width > 6
        {
            room -= 3;
            out.set_stringn(tree.x + tree.width - 2, y, d.letter().to_string(), 1, style);
        }
        let text = format!("{}{marker}{}", "  ".repeat(row.depth + 1), e.name);
        out.set_stringn(tree.x, y, text, room, style);
    }
    let (history, history_scrollbar) = match section {
        Some(section) => render_history(f, section, app),
        None => (None, None),
    };
    SideHits {
        explorer_rows: tree,
        scrollbar: bar.map(|b| (ScrollTarget::Side(SidePanel::Explorer), b)),
        history,
        history_scrollbar,
        ..SideHits::default()
    }
}

/// The "Git History" section: the divider row (in the border colour), the collapsible header,
/// then the commit rows — summary, author, relative date and short id as space allows — or
/// "Reading history…", an error, or "No history".
fn render_history(f: &mut Frame, section: Rect, app: &mut App) -> (Option<HistoryHit>, Option<ScrollBarHit>) {
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let out = f.buffer_mut();
    let divider = Rect {
        y: section.y,
        height: 1,
        ..section
    };
    let rule = "─".repeat(divider.width as usize);
    out.set_stringn(
        divider.x,
        divider.y,
        &rule,
        divider.width as usize,
        base.patch(theme.get("sideBar.border")),
    );
    let header = Rect {
        y: section.y + 1,
        height: 1,
        ..section
    };
    let marker = if app.git_history.collapsed() { "▸" } else { "▾" };
    out.set_stringn(
        header.x,
        header.y,
        format!(" {marker} GIT HISTORY"),
        header.width as usize,
        base.patch(theme.get("sideBar.header")),
    );
    let mut hit = HistoryHit {
        divider,
        header,
        ..HistoryHit::default()
    };
    if app.git_history.collapsed() || section.height <= 2 {
        return (Some(hit), None);
    }

    let mut list = Rect {
        y: section.y + 2,
        height: section.height - 2,
        ..section
    };
    let bar = scrollbar::take_column(&mut list);
    if list.height == 0 || list.width == 0 {
        return (Some(hit), None);
    }
    let look = scrollbar::look(app, ScrollTarget::History);
    let history = &mut app.git_history;
    history.set_height(list.height as usize);
    history.clamp();
    let top = history.scroll();
    let len = history.rows().len();
    let scroll = history.scrollable().scroll();
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme));
    let selected = history.selected_row();
    let focused =
        app.ui.focus == Focus::SidePanel && history.focused() && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };

    if len == 0 {
        let dim = base.patch(theme.get("sideBar.dim"));
        let text = if history.loading {
            "Reading history…".to_string()
        } else if let Some(error) = &history.error {
            error.clone()
        } else {
            "No history".to_string()
        };
        out.set_stringn(list.x + 1, list.y, &text, list.width.saturating_sub(1) as usize, dim);
    } else {
        let now = history::now_ms();
        for i in top..(top + list.height as usize).min(len) {
            let y = list.y + (i - top) as u16;
            let style = if selected == Some(i) {
                base.patch(selected_style)
            } else {
                base
            };
            if selected == Some(i) {
                out.set_style(Rect { y, height: 1, ..list }, style);
            }
            draw_commit(out, list, y, &history.commits[i], style, theme, now);
        }
    }
    hit.list = list;
    hit.top = top;
    (Some(hit), bar)
}

/// One commit row: ` summary` on the left, then (as space allows) `author, ago  short` at the
/// right — the author dropped first, then the date, then the whole suffix; the summary is cut
/// with "…" before any of that.
fn draw_commit(out: &mut Cells, list: Rect, y: u16, commit: &Commit, style: Style, theme: &Theme, now: u64) {
    let width = list.width as usize;
    if width == 0 {
        return;
    }
    let ago = history::ago((commit.time.max(0) as u64).saturating_mul(1000), now);
    let author = commit.author.as_str();
    let short = commit.short.as_str();
    let full = format!("{author}, {ago}  {short}");
    let no_author = format!("{ago}  {short}");
    let hash_only = short.to_string();
    let fits = |s: &str| width >= s.width() + MIN_SUMMARY + 2;
    let (suffix, kind) = if fits(&full) {
        (full, 0)
    } else if fits(&no_author) {
        (no_author, 1)
    } else if width >= hash_only.width() + 4 {
        (hash_only, 2)
    } else {
        (String::new(), 3)
    };
    let room = width.saturating_sub(suffix.width());
    let summary = format!(" {}", cut(&commit.summary, room.saturating_sub(1)));
    out.set_stringn(list.x, y, &summary, room, style);
    if kind == 3 {
        return;
    }
    let mut x = list.x + list.width - suffix.width() as u16;
    let mut put = |text: &str, style: Style, x: &mut u16| {
        let (end, _) = out.set_stringn(*x, y, text, text.width(), style);
        *x = end;
    };
    match kind {
        0 => {
            put(author, style.patch(theme.get("gitHistory.author")), &mut x);
            put(", ", style, &mut x);
            put(&ago, style.patch(theme.get("gitHistory.date")), &mut x);
            put("  ", style, &mut x);
            put(short, style.patch(theme.get("gitHistory.hash")), &mut x);
        }
        1 => {
            put(&ago, style.patch(theme.get("gitHistory.date")), &mut x);
            put("  ", style, &mut x);
            put(short, style.patch(theme.get("gitHistory.hash")), &mut x);
        }
        _ => put(short, style.patch(theme.get("gitHistory.hash")), &mut x),
    }
}

/// `text` cut to `width` cells, with "…" when it doesn't fit.
fn cut(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}
