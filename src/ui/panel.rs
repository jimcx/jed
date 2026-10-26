//! The bottom panel under the editor groups: a top border (dragged to resize), a tab strip
//! ("PROBLEMS" with its count, as VS Code; "LOCATIONS" with its count and title while there's a
//! Locations list; a tab per task run: its name, state and a close button) and the shown tab's
//! content — the Problems list: a row per file (name, dimmed folder, count) followed by its
//! problems (severity icon, the message's first line, `source(code)` and `[Ln x, Col y]`
//! dimmed); the Locations list: the same file rows, each place's line as a preview with the
//! place highlighted (`search.match`; "Ln x, Col y" dimmed until the file is read); or a task's
//! terminal (`ui::task`). Only the rows on screen are looked at.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::{App, Focus};
use crate::grouped;
use crate::layout::{ListHit, PanelTab, ScrollBarHit, ScrollTarget};
use crate::locations::Locations;
use crate::problems::{self, Problem};
use crate::scrollbar::Scrollable;
use crate::tasks::{RunId, Status};

/// Cells a problem row is indented by.
const INDENT: u16 = 3;

/// What the panel drew for the hit map.
#[derive(Default)]
pub struct PanelHits {
    pub tabs: Vec<(Rect, PanelTab)>,
    /// Task tabs' close buttons.
    pub closes: Vec<(Rect, RunId)>,
    /// The shown list (Problems or Locations).
    pub list: Option<ListHit>,
    /// Its scroll bar (the list's last column).
    pub scrollbar: Option<(ScrollTarget, ScrollBarHit)>,
}

/// A task tab's state mark and its theme key: `●` running, `✓` exit 0, `✗ 2` a failing exit
/// code, `✗ killed`, `✗` another signal or a failed start.
pub fn task_mark(status: &Status) -> (String, &'static str) {
    match status {
        Status::Starting | Status::Running => ("●".into(), "task.running"),
        Status::Exited(e) if e.success() => ("✓".into(), "task.succeeded"),
        Status::Exited(e) if e.killed => ("✗ killed".into(), "task.failed"),
        Status::Exited(e) if e.signal.is_none() => (format!("✗ {}", e.code), "task.failed"),
        Status::Exited(_) | Status::Failed(_) => ("✗".into(), "task.failed"),
    }
}

/// Draws the panel in `area` (its top row is the border); returns its tab labels, the task
/// tabs' close buttons and the shown list's hits.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> PanelHits {
    let theme = &app.theme;
    let base = theme.get("panel");
    let dim = base.patch(theme.get("panel.dim"));
    let out = f.buffer_mut();
    out.set_style(area, base);
    let border = base.patch(theme.get("panel.border"));
    for x in area.x..area.x + area.width {
        out.set_stringn(x, area.y, "─", 1, border);
    }
    let mut hits = PanelHits::default();
    if area.height < 2 {
        return hits;
    }

    let files = problems::files(&app.lsp.diagnostics, &app.buffers);
    let total = problems::total(&files);

    // The tab strip: " PROBLEMS  12 ", then the task tabs: "build ● ×".
    let tabs_y = area.y + 1;
    let mut x = area.x + 1;
    let end = area.x + area.width;
    let title_style = |tab: PanelTab| {
        let key = if tab == app.ui.panel_tab {
            "panelTitle.active"
        } else {
            "panelTitle.inactive"
        };
        base.patch(theme.get(key))
    };
    for tab in PanelTab::ALL {
        let badge_count = match tab {
            PanelTab::Problems => total,
            PanelTab::Locations => match &app.locations {
                Some(l) => l.total(),
                None => continue,
            },
            PanelTab::Task(_) => continue,
        };
        let start = x;
        x = out
            .set_stringn(x, tabs_y, tab.title(), end.saturating_sub(x) as usize, title_style(tab))
            .0;
        if badge_count > 0 && x + 1 < end {
            let badge = format!(" {badge_count} ");
            x = out
                .set_stringn(
                    x + 1,
                    tabs_y,
                    &badge,
                    end.saturating_sub(x + 1) as usize,
                    base.patch(theme.get("panel.badge")),
                )
                .0;
        }
        hits.tabs.push((
            Rect {
                x: start,
                y: tabs_y,
                width: x - start,
                height: 1,
            },
            tab,
        ));
        x += 2;
    }
    for run in app.tasks.runs() {
        if x >= end {
            break;
        }
        let tab = PanelTab::Task(run.id);
        let start = x;
        x = out
            .set_stringn(x, tabs_y, &run.name, end.saturating_sub(x) as usize, title_style(tab))
            .0;
        let (mark, key) = task_mark(&run.status());
        if x + 1 < end {
            x = out
                .set_stringn(
                    x + 1,
                    tabs_y,
                    &mark,
                    end.saturating_sub(x + 1) as usize,
                    base.patch(theme.get(key)),
                )
                .0;
        }
        hits.tabs.push((
            Rect {
                x: start,
                y: tabs_y,
                width: x - start,
                height: 1,
            },
            tab,
        ));
        if x + 1 < end {
            out.set_stringn(x + 1, tabs_y, "×", 1, dim);
            hits.closes.push((Rect::new(x + 1, tabs_y, 1, 1), run.id));
            x += 2;
        }
        x += 2;
    }
    if app.ui.panel_tab == PanelTab::Locations
        && let Some(l) = &app.locations
        && x + 2 < end
    {
        out.set_stringn(x + 1, tabs_y, &l.title, (end - x - 2) as usize, dim);
    }

    let list = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: area.height.saturating_sub(2),
    };
    if list.height == 0 {
        return hits;
    }
    match app.ui.panel_tab {
        PanelTab::Task(id) => {
            drop(files);
            let info = Rect::new(x, tabs_y, end.saturating_sub(x), 1);
            hits.scrollbar =
                super::task::render(f, area, info, app, id).map(|b| (ScrollTarget::Panel(PanelTab::Task(id)), b));
            return hits;
        }
        PanelTab::Locations => {
            drop(files);
            let look = scrollbar::look(app, ScrollTarget::Panel(PanelTab::Locations));
            let (list, bar) = locations(out, list, app, look);
            hits.list = Some(list);
            hits.scrollbar = bar.map(|b| (ScrollTarget::Panel(PanelTab::Locations), b));
            return hits;
        }
        PanelTab::Problems => {}
    }
    if files.is_empty() {
        out.set_stringn(
            list.x + 2,
            list.y,
            "No problems have been detected in the workspace.",
            list.width.saturating_sub(3) as usize,
            dim,
        );
        hits.list = Some(ListHit { list, rows: Vec::new() });
        return hits;
    }

    let look = scrollbar::look(app, ScrollTarget::Panel(PanelTab::Problems));
    let mut list = list;
    let bar = scrollbar::take_column(&mut list);
    let view = &mut app.problems.list;
    view.set_height(&files, list.height as usize);
    view.clamp(&files);
    let top = view.scroll(&files);
    let selected_row = view.selected_row(&files);
    let scroll = view.scrollable(&files).scroll();
    if let Some(track) = bar {
        let bar = scrollbar::render(out, track, scroll, look, base, theme, &[]);
        hits.scrollbar = Some((ScrollTarget::Panel(PanelTab::Problems), bar));
    }
    let focused = app.ui.focus == Focus::Panel && app.prompt.is_none() && app.palette.is_none();
    let selection = if focused {
        theme.get("list.active")
    } else {
        theme.get("list.inactive")
    };
    let plain = Styles {
        base,
        dim,
        name: base.patch(theme.get("panel.fileName")),
        badge: base.patch(theme.get("panel.badge")),
        icons: crate::diagnostics::Severity::ALL.map(|s| base.patch(theme.get(s.icon_key()))),
    };
    let bg = Style {
        bg: selection.bg,
        ..Style::default()
    };
    let on_selection = Styles {
        base: base.patch(selection),
        dim: dim.patch(bg),
        name: plain.name.patch(bg),
        badge: plain.badge,
        icons: plain.icons.map(|s| s.patch(bg)),
    };
    let root = app.workspace.root.as_path();
    let tab_width = app.settings.tab_width;

    let shown = grouped::rows(&files, top, list.height as usize);
    // The problems on screen, fetched once per file (a closed file with several servers merges
    // its sets per fetch).
    let mut fetched: Option<(usize, usize, Vec<Problem>)> = None;
    for (r, &(i, offset)) in shown.iter().enumerate() {
        let y = list.y + r as u16;
        let s = if selected_row == Some(top + r) {
            out.set_style(Rect { y, height: 1, ..list }, on_selection.base);
            &on_selection
        } else {
            &plain
        };
        let file = &files[i];
        if offset == 0 {
            draw_file(out, list.x, y, list.width, file.path, file.count(), root, s);
            continue;
        }
        let k = offset - 1;
        let stale = fetched
            .as_ref()
            .is_none_or(|(fi, from, v)| *fi != i || k < *from || k >= from + v.len());
        if stale {
            let n = shown[r..].iter().take_while(|(j, _)| *j == i).count();
            fetched = Some((i, k, file.problems(k, n)));
        }
        let Some((_, from, v)) = &fetched else { continue };
        if let Some(p) = v.get(k - from) {
            let (line, col) = p.place.line_col(file.rope(), tab_width);
            draw_problem(
                out,
                list.x + INDENT,
                y,
                list.width.saturating_sub(INDENT + 1),
                p,
                (line, col),
                s,
            );
        }
    }
    let rows = shown
        .iter()
        .map(|&(i, offset)| (files[i].path.to_path_buf(), offset))
        .collect();
    hits.list = Some(ListHit { list, rows });
    hits
}

/// The Locations list in `list`: file rows as the Problems list's, then each place's line; its
/// scroll bar in the last column.
fn locations(
    out: &mut ratatui::buffer::Buffer,
    list: Rect,
    app: &mut App,
    look: scrollbar::Look,
) -> (ListHit, Option<ScrollBarHit>) {
    let theme = &app.theme;
    let base = theme.get("panel");
    let dim = base.patch(theme.get("panel.dim"));
    let Some(l) = app.locations.as_mut() else {
        out.set_stringn(
            list.x + 2,
            list.y,
            "No locations.",
            list.width.saturating_sub(3) as usize,
            dim,
        );
        return (ListHit { list, rows: Vec::new() }, None);
    };
    let mut list = list;
    let bar = scrollbar::take_column(&mut list);
    let Locations { files, list: view, .. } = l;
    view.set_height(files, list.height as usize);
    view.clamp(files);
    let top = view.scroll(files);
    let selected_row = view.selected_row(files);
    let scroll = view.scrollable(files).scroll();
    let bar = bar.map(|track| scrollbar::render(out, track, scroll, look, base, theme, &[]));
    let focused = app.ui.focus == Focus::Panel && app.prompt.is_none() && app.palette.is_none();
    let selection = if focused {
        theme.get("list.active")
    } else {
        theme.get("list.inactive")
    };
    let bg = Style {
        bg: selection.bg,
        ..Style::default()
    };
    let plain = Styles {
        base,
        dim,
        name: base.patch(theme.get("panel.fileName")),
        badge: base.patch(theme.get("panel.badge")),
        icons: [base; 4],
    };
    let on_selection = Styles {
        base: base.patch(selection),
        dim: dim.patch(bg),
        name: plain.name.patch(bg),
        badge: plain.badge,
        icons: [base; 4],
    };
    let matched = theme.get("search.match");
    let root = app.workspace.root.as_path();
    let shown = grouped::rows(files, top, list.height as usize);
    for (r, &(i, offset)) in shown.iter().enumerate() {
        let y = list.y + r as u16;
        let s = if selected_row == Some(top + r) {
            out.set_style(Rect { y, height: 1, ..list }, on_selection.base);
            &on_selection
        } else {
            &plain
        };
        let file = &files[i];
        if offset == 0 {
            draw_file(out, list.x, y, list.width, &file.path, file.items.len(), root, s);
            continue;
        }
        let item = &file.items[offset - 1];
        let (x, width) = (list.x + INDENT, list.width.saturating_sub(INDENT + 1));
        match &item.preview {
            Some(m) => super::search::draw_line(out, x, y, width, m, (s.base, s.dim, s.base.patch(matched)), None),
            None => {
                let at = format!("Ln {}, Col {}", item.start.line + 1, item.start.character + 1);
                out.set_stringn(x, y, &at, width as usize, s.dim);
            }
        }
    }
    let rows = shown
        .iter()
        .map(|&(i, offset)| (files[i].path.clone(), offset))
        .collect();
    (ListHit { list, rows }, bar)
}

struct Styles {
    base: Style,
    dim: Style,
    name: Style,
    badge: Style,
    icons: [Style; 4],
}

/// " name  folder        3 ": the path relative to the root (a file outside it: its whole
/// folder), the count right-aligned; the folder gives way first.
#[allow(clippy::too_many_arguments)]
fn draw_file(
    out: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    path: &std::path::Path,
    count: usize,
    root: &std::path::Path,
    s: &Styles,
) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let name = rel
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let dir = rel.parent().map_or_else(String::new, |d| d.display().to_string());
    let count = format!(" {count} ");
    let count_w = count.width() as u16;
    let room = width.saturating_sub(count_w + 2) as usize;
    let (nx, _) = out.set_stringn(x + 1, y, &name, room, s.name);
    let used = (nx - x - 1) as usize;
    if !dir.is_empty() && used + 2 < room {
        out.set_stringn(nx + 1, y, &dir, room - used - 1, s.dim);
    }
    if width > count_w + 1 {
        out.set_stringn(x + width - count_w - 1, y, &count, count_w as usize, s.badge);
    }
}

/// "⊗ message  source(code) [Ln 3, Col 5]": the message's first line, cut to leave room for the
/// dimmed tail when there's enough; else just the message.
fn draw_problem(
    out: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    p: &Problem,
    (line, col): (usize, usize),
    s: &Styles,
) {
    let width = width as usize;
    if width < 3 {
        return;
    }
    let d = p.diag;
    out.set_stringn(x, y, d.severity.icon(), 1, s.icons[d.severity as usize]);
    let first = d.message.lines().next().unwrap_or("").trim_end();
    let message: String = first.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let message = message.as_str();
    let mut tail = String::new();
    if let Some(source) = problems::source_label(d) {
        tail.push_str(&source);
        tail.push(' ');
    }
    tail.push_str(&format!("[Ln {line}, Col {col}]"));
    let room = width - 2;
    let tail_w = tail.width();
    let (msg_room, show_tail) = if message.width() + 2 + tail_w <= room {
        (message.width(), true)
    } else if room >= tail_w + 2 + 12 {
        (room - tail_w - 2, true)
    } else {
        (room, false)
    };
    let mx = x + 2;
    let (end, _) = out.set_stringn(mx, y, message, msg_room, s.base);
    if message.width() > msg_room && msg_room > 0 {
        // Cut: an ellipsis in its last cell.
        out.set_stringn(mx + msg_room as u16 - 1, y, "…", 1, s.dim);
    }
    if show_tail {
        let tx = end.max(mx + msg_room.min(message.width()) as u16) + 2;
        out.set_stringn(tx, y, &tail, tail_w, s.dim);
    }
}
