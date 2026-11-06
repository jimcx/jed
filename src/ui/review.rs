//! The Code Review panel, in the side panel below its title: the merge request's summary (`!42
//! title` with `‹1/3›` when the branch has several, `head → base`, files and viewed counts and
//! state), then `▾ FILES` and one row per changed file: the viewed `✓`, git's status letter (in
//! `review.*` colours), the path and `+added -deleted` at the right. Without a merge request, the
//! reason in words (`Empty::text`); a failed lookup's error in `review.error` on top of what was
//! shown before. Only the rows on screen are drawn.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::{App, Focus};
use crate::forge::MrState;
use crate::layout::{ReviewHit, ScrollBarHit, ScrollTarget, SidePanel};
use crate::review::Content;

/// Draws the panel in `area` (the side panel below its title, without its border), the file
/// list's scroll bar in its last column. Sets the list's height and keeps its selection on its file.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> (Option<ReviewHit>, Option<ScrollBarHit>) {
    if area.width < 8 || area.height < 1 {
        return (None, None);
    }
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::Review));
    let theme = &app.theme;
    let base = theme.get("sideBar");
    let dim = base.patch(theme.get("sideBar.dim"));
    let header = base.patch(theme.get("sideBar.header"));
    let error = base.patch(theme.get("review.error"));
    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let selected_style = if focused {
        theme.get("list.active")
    } else {
        base.patch(theme.get("list.inactive"))
    };
    let out = f.buffer_mut();
    let width = area.width as usize;
    let mut y = area.y;
    let end = area.y + area.height;
    let state = &mut app.review;
    // Lines of text, wrapped, until the area's end.
    let text = |out: &mut ratatui::buffer::Buffer, y: &mut u16, s: &str, style| {
        for line in wrap(s, width.saturating_sub(2)) {
            if *y >= end {
                return;
            }
            out.set_stringn(area.x + 1, *y, &line, width - 1, style);
            *y += 1;
        }
    };
    if let Some(e) = &state.error {
        text(out, &mut y, e, error);
    }
    let mut hit = ReviewHit {
        list: Rect::default(),
        rows: Vec::new(),
        summary: Rect::default(),
    };
    let Content::Loaded(review) = &state.content else {
        let message = match &state.content {
            Content::Empty(e) => e.text(),
            _ if state.loading => "Looking for a merge request…".into(),
            _ if state.error.is_some() => String::new(),
            _ => "No merge request looked up yet.".into(),
        };
        text(out, &mut y, &message, dim);
        return (Some(hit), None);
    };

    // The summary.
    let (n, of) = review.position();
    let mark = if of > 1 {
        format!("  ‹{n}/{of}›")
    } else {
        String::new()
    };
    let draft = if review.mr.draft { "Draft: " } else { "" };
    let title = format!("!{} {draft}{}{mark}", review.mr.number, review.mr.title);
    hit.summary = Rect { y, height: 1, ..area };
    text(out, &mut y, &truncate(&title, width - 2), header);
    let branches = format!("{} → {}", review.mr.head.name, review.mr.base.name);
    text(out, &mut y, &truncate(&branches, width - 2), dim);
    let viewed = state.viewed_count();
    let review = state.review().expect("loaded above");
    let files = review.files.len();
    let mr_state = match review.mr.state {
        MrState::Open => "open",
        MrState::Closed => "closed",
        MrState::Merged => "merged",
    };
    let mut counts = format!("{files} file{}", if files == 1 { "" } else { "s" });
    if viewed > 0 {
        counts.push_str(&format!(" · {viewed} viewed"));
    }
    counts.push_str(&format!(" · {mr_state}"));
    text(out, &mut y, &truncate(&counts, width - 2), dim);
    if y < end {
        out.set_stringn(area.x, y, " ▾ FILES", width, header);
        y += 1;
    }
    if let Some(e) = &review.files_error {
        text(out, &mut y, e, error);
    }
    if y >= end {
        return (Some(hit), None);
    }

    // The file list.
    let mut list = Rect {
        y,
        height: end - y,
        ..area
    };
    let bar = scrollbar::take_column(&mut list);
    let state = &mut app.review;
    let Content::Loaded(review) = &state.content else {
        unreachable!()
    };
    let files = &review.files;
    state.view.set_height(files, list.height as usize);
    state.view.settle(files);
    state.view.clamp(files.len());
    let top = state.view.scroll(files.len());
    let selected = state.view.selected_row(files);
    let scroll = state.view.scrollable(files.len());
    let bar = bar.map(|track| {
        let scroll = crate::scrollbar::Scrollable::scroll(&scroll);
        scrollbar::render(out, track, scroll, look, base, theme)
    });
    hit.list = list;
    for (i, file) in files.iter().enumerate().skip(top).take(list.height as usize) {
        let row_y = list.y + (i - top) as u16;
        let style = if selected == Some(i) {
            let s = base.patch(selected_style);
            out.set_style(
                Rect {
                    y: row_y,
                    height: 1,
                    ..list
                },
                s,
            );
            s
        } else {
            base
        };
        let is_viewed = state.viewed.as_ref().is_some_and(|v| {
            let key = crate::review::Viewed::key(&review.repo, review.mr.number, &file.path);
            v.is_viewed(&key, &file.hash)
        });
        if is_viewed {
            out.set_stringn(list.x + 1, row_y, "✓", 1, style.patch(theme.get("review.viewed")));
        }
        let key = match file.status {
            'A' => "review.added",
            'D' => "review.deleted",
            'R' | 'C' => "review.renamed",
            _ => "review.modified",
        };
        out.set_stringn(
            list.x + 3,
            row_y,
            file.status.encode_utf8(&mut [0; 4]),
            1,
            style.patch(theme.get(key)),
        );
        // The counts at the right edge.
        let mut right = list.x + list.width;
        let mut put = |s: String, key: &str, right: &mut u16| {
            let w = s.width() as u16;
            if *right >= list.x + 5 + w {
                *right -= w;
                out.set_stringn(*right, row_y, &s, w as usize, style.patch(theme.get(key)));
                *right = right.saturating_sub(1);
            }
        };
        if file.deletions > 0 {
            put(format!("-{}", file.deletions), "review.deleted", &mut right);
        }
        if file.additions > 0 {
            put(format!("+{}", file.additions), "review.added", &mut right);
        }
        let room = right.saturating_sub(list.x + 5) as usize;
        out.set_stringn(list.x + 5, row_y, truncate(&file.path, room), room, style);
        hit.rows.push(file.path.clone());
    }
    (Some(hit), bar)
}

/// `s` cut to `width` columns, ending in `…` when it was.
fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `s` broken into lines of at most `width` columns at spaces (a longer word is cut).
fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        while line.width() > width {
            let mut used = 0;
            let cut = line
                .chars()
                .take_while(|c| {
                    used += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
                    used <= width
                })
                .collect::<String>();
            line = line[cut.len()..].to_string();
            lines.push(cut);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn truncate_ends_in_an_ellipsis() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
