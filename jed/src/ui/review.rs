//! The Code Review panel, in the side panel below its title: the merge request's summary (`!42
//! title` with `‹1/3›` when the branch has several, `head → base`, files and viewed counts and
//! state), then `▾ FILES` and one row per changed file: the viewed `✓`, git's status letter (in
//! `review.*` colours), the path and `+added -deleted` at the right. Under them, sized by
//! `side_sections::split`, the CONVERSATIONS and PENDING sections (a divider row, a collapsing
//! header; PENDING's submit buttons). Without a merge request, the
//! reason in words (`Empty::text`); a failed lookup's error in `review.error` on top of what was
//! shown before. Only the rows on screen are drawn.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::scrollbar;
use crate::app::{App, Focus};
use crate::command::Command;
use crate::forge::{ApprovalState, MrState};
use crate::layout::{ReviewHit, ScrollBarHit, ScrollTarget, SidePanel};
use crate::review::{Content, Pane};
use crate::review_anchor::Placement;
use crate::side_sections::{split, split_collapsed};

/// Draws the panel in `area` (the side panel below its title, without its border), the file
/// list's scroll bar in its last column. Sets the list's height and keeps its selection on its file.
pub fn render(f: &mut Frame, area: Rect, app: &mut App) -> (Option<ReviewHit>, Option<ScrollBarHit>) {
    if area.width < 8 || area.height < 1 {
        return (None, None);
    }
    let look = scrollbar::look(app, ScrollTarget::Side(SidePanel::Review));
    let forge_name = app.review.review().map(|r| r.kind.display_name());
    let pending_on = app.pending_review_id().and(forge_name);
    let drafts_on_forge = app.settings.review_drafts_on_forge;
    let theme = &app.theme;
    let base = theme.get("sideBar");
    // Foreground only, for text over a highlighted row (the row's background stays).
    let dim_fg = theme.get("sideBar.dim");
    let error_fg = theme.get("review.error");
    let dim = base.patch(dim_fg);
    let header = base.patch(theme.get("sideBar.header"));
    let error = base.patch(theme.get("review.error"));
    let focused = app.ui.focus == Focus::SidePanel && app.prompt.is_none() && app.palette.is_none();
    let inactive = base.patch(theme.get("list.inactive"));
    let selected_style = if focused { theme.get("list.active") } else { inactive };
    let out = f.buffer_mut();
    let width = area.width as usize;
    let mut y = area.y;
    let end = area.y + area.height;
    let state = &mut app.review;
    // Lines of text, wrapped, until the area's end.
    let text = |out: &mut ratatui::buffer::Buffer, y: &mut u16, limit: u16, s: &str, style| {
        for line in wrap(s, width.saturating_sub(2)) {
            if *y >= limit {
                return;
            }
            out.set_stringn(area.x + 1, *y, &line, width - 1, style);
            *y += 1;
        }
    };
    if let Some(e) = &state.error {
        text(out, &mut y, end, e, error);
    }
    let mut hit = ReviewHit {
        list: Rect::default(),
        rows: Vec::new(),
        summary: Rect::default(),
        ..ReviewHit::default()
    };
    let Content::Loaded(review) = &state.content else {
        let message = match &state.content {
            Content::Empty(e) => e.text(),
            _ if state.loading => "Looking for a merge request…".into(),
            _ if state.error.is_some() => String::new(),
            _ => "No merge request looked up yet.".into(),
        };
        text(out, &mut y, end, &message, dim);
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
    let shown = truncate(&title, width - 2);
    if !mark.is_empty() && shown.ends_with(&mark) {
        let w = mark.chars().count() as u16;
        hit.switcher = Rect::new(area.x + 1 + (shown.chars().count() as u16 - w), y, w, 1);
    }
    text(out, &mut y, end, &shown, header);
    if state.view.summary {
        out.set_style(hit.summary, base.patch(selected_style));
    }
    let branches = format!("{} → {}", review.mr.head.name, review.mr.base.name);
    text(out, &mut y, end, &truncate(&branches, width - 2), dim);
    let viewed = state.viewed_count();
    let review = state.review().expect("loaded above");
    let unresolved = review.unresolved();
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
    if unresolved > 0 {
        counts.push_str(&format!(" · {unresolved} unresolved"));
    }
    counts.push_str(&format!(" · {mr_state}"));
    text(out, &mut y, end, &truncate(&counts, width - 2), dim);
    for (state, mark, what, key) in [
        (ApprovalState::Approved, '✓', "approved", "review.approved"),
        (
            ApprovalState::ChangesRequested,
            '✗',
            "changes requested",
            "review.changesRequested",
        ),
    ] {
        let names: Vec<&str> = review
            .approvals
            .iter()
            .filter(|a| a.state == state)
            .map(|a| a.author.as_str())
            .collect();
        if !names.is_empty() {
            let line = format!("{mark} {what} by {}", names.join(", "));
            let style = base.patch(theme.get(key));
            text(out, &mut y, end, &truncate(&line, width - 2), style);
        }
    }
    // Submit Review needs local HEAD to be the merge request's head.
    if let Some(banner) = review.head_banner(app.git.status.as_ref().and_then(|s| s.head_id.as_deref())) {
        text(out, &mut y, end, banner, error);
    }
    // FILES, then CONVERSATIONS and PENDING (each a divider, a header and a body) share the rest.
    if y >= end {
        return (Some(hit), None);
    }
    let sections = state.sections();
    let drafts = match &state.content {
        Content::Loaded(r) => state.drafts.rows(r),
        _ => Vec::new(),
    };
    state.view.conv.height = 0;
    state.view.pending.height = 0;
    let region = Rect {
        y,
        height: end - y,
        ..area
    };
    hit.body = region;
    let Content::Loaded(review) = &state.content else {
        unreachable!()
    };
    let wishes = sections.iter().map(|s| s.1).collect::<Vec<_>>();
    let files_collapsed = state.view.files_collapsed;
    let (files_h, heights) = if files_collapsed {
        split_collapsed(region.height, &wishes)
    } else {
        split(region.height, &wishes)
    };
    let height_of = |pane: Pane| {
        sections
            .iter()
            .zip(&heights)
            .find(|(s, _)| s.0 == pane)
            .map_or(0, |(_, h)| *h)
    };
    let conv_h = height_of(Pane::Conversations);
    let pending_h = height_of(Pane::Pending);
    let files_end = y + files_h;
    hit.files_header = Rect { y, height: 1, ..area };
    let header_selected = (files_collapsed || state.view.files_header)
        && !state.view.summary
        && !state.view.conv.focused
        && !state.view.pending.focused;
    let files_label = format!(" {} FILES", if files_collapsed { '▸' } else { '▾' });
    let files_head = if header_selected {
        base.patch(selected_style)
    } else {
        header
    };
    out.set_style(hit.files_header, files_head);
    out.set_stringn(area.x, y, &files_label, width, files_head);
    y += 1;
    if let Some(e) = &review.files_error {
        text(out, &mut y, files_end, e, error);
    }
    let mut bar = None;
    if y < files_end && !files_collapsed {
        let mut list = Rect {
            y,
            height: files_end - y,
            ..area
        };
        let track = scrollbar::take_column(&mut list);
        let files = &review.files;
        state.view.set_height(files, list.height as usize);
        state.view.settle(files);
        state.view.clamp(files.len());
        let top = state.view.scroll(files.len());
        let selected = state.view.selected_row(files);
        let scroll = state.view.scrollable(files.len());
        bar = track.map(|track| {
            let scroll = crate::scrollbar::Scrollable::scroll(&scroll);
            scrollbar::render(out, track, scroll, look, base, theme)
        });
        hit.list = list;
        let files_style = if state.view.conv.focused || state.view.files_header {
            inactive
        } else {
            selected_style
        };
        for (i, file) in files.iter().enumerate().skip(top).take(list.height as usize) {
            let row_y = list.y + (i - top) as u16;
            let style = if selected == Some(i) {
                let s = base.patch(files_style);
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
    }

    // CONVERSATIONS: a divider row (a border, left blank), the header, two-line rows.
    state.view.conv.height = 0;
    if conv_h > 0 {
        let divider_y = files_end;
        hit.dividers.push((
            Rect {
                y: divider_y,
                height: 1,
                ..area
            },
            Pane::Conversations,
        ));
        hit.conv_header = Rect {
            y: divider_y + 1,
            height: 1,
            ..area
        };
        let len = review.conv_len();
        let conv = &mut state.view.conv;
        let label = format!(" {} CONVERSATIONS ({len})", if conv.collapsed { '▸' } else { '▾' });
        out.set_stringn(area.x, divider_y + 1, &label, width, header);
        let mut by = divider_y + 2;
        let body_end = divider_y + conv_h;
        if by < body_end
            && let Some(e) = &review.comments_error
        {
            out.set_stringn(area.x + 1, by, truncate(e, width - 2), width - 1, error);
            by += 1;
        }
        let rows = (body_end.saturating_sub(by) / 2) as usize;
        conv.height = rows;
        let top = conv.top(len);
        let selected = conv.selected(len);
        hit.conv_list = Rect {
            y: by,
            height: rows as u16 * 2,
            ..area
        };
        hit.conv_top = top;
        hit.conv_len = len;
        for i in top..(top + rows).min(len) {
            let y1 = by + ((i - top) * 2) as u16;
            let mut row_style = base;
            if selected == Some(i) {
                row_style = base.patch(if conv.focused { selected_style } else { inactive });
                out.set_style(
                    Rect {
                        y: y1,
                        height: 2,
                        ..area
                    },
                    row_style,
                );
            }
            let (glyph, glyph_key, label, tag, first, faded) = match review.threads.get(i) {
                Some(t) => {
                    let c = t.thread.comments.first();
                    let (glyph, key) = if t.thread.resolved {
                        ("✓", "review.resolved")
                    } else {
                        ("●", "review.unresolved")
                    };
                    let a = &t.thread.anchor;
                    // A placed thread says "newer commit" before the forge's own outdated flag.
                    let (tag, faded): (String, bool) = match t.place {
                        Placement::Placed { .. } if t.behind => ("newer commit".into(), true),
                        Placement::Placed { .. } => (
                            t.outdated_on(review.kind)
                                .map_or_else(String::new, |f| format!("outdated on {f}")),
                            false,
                        ),
                        Placement::Outdated => ("outdated".into(), true),
                        Placement::Missing => ("no commit".into(), true),
                    };
                    (
                        glyph,
                        key,
                        format!("{}:{}  {}", a.path, a.start_line, c.map_or("", |c| c.author.as_str())),
                        tag,
                        c.map_or("", |c| c.body.as_str()),
                        faded,
                    )
                }
                None => {
                    let c = &review.conversation[i - review.threads.len()];
                    (
                        "○",
                        "review.resolved",
                        format!("{}  (merge request)", c.author),
                        String::new(),
                        c.body.as_str(),
                        false,
                    )
                }
            };
            let fade = if faded { dim_fg } else { Style::new() };
            out.set_stringn(area.x + 1, y1, glyph, 1, row_style.patch(theme.get(glyph_key)));
            let tag_w = if tag.is_empty() { 0 } else { tag.width() + 1 };
            let room = width.saturating_sub(4 + tag_w);
            out.set_stringn(area.x + 3, y1, truncate(&label, room), room, row_style.patch(fade));
            if !tag.is_empty() {
                let x = area.x + (width - tag.width() - 1) as u16;
                out.set_stringn(x, y1, &tag, tag.width(), row_style.patch(dim_fg));
            }
            let text_room = width.saturating_sub(4);
            let line = first.lines().next().unwrap_or("").trim();
            out.set_stringn(
                area.x + 3,
                y1 + 1,
                truncate(line, text_room),
                text_room,
                row_style.patch(dim_fg),
            );
        }
    }
    // PENDING: a divider row, the header, the submit buttons, one-line rows.
    if pending_h > 0 {
        let divider_y = files_end + conv_h;
        hit.dividers.push((
            Rect {
                y: divider_y,
                height: 1,
                ..area
            },
            Pane::Pending,
        ));
        let len = drafts.len();
        let p = &mut state.view.pending;
        hit.pending_header = Rect {
            y: divider_y + 1,
            height: 1,
            ..area
        };
        let arrow = if p.collapsed { '▸' } else { '▾' };
        let label = match pending_on {
            Some(name) => format!(" {arrow} PENDING ({len}, on {name})"),
            None => format!(" {arrow} PENDING ({len})"),
        };
        out.set_stringn(area.x, divider_y + 1, &label, width, header);
        let body_end = divider_y + pending_h;
        let mut by = divider_y + 2;
        // The buttons flow onto as many rows as they need (each row taken from the list's).
        if by < body_end && !p.collapsed {
            let buttons = [
                (" Submit ", Command::ReviewSubmitComment, "review.composer.button"),
                (" Approve ", Command::ReviewSubmitApprove, "review.thread.button"),
                (
                    " Request Changes ",
                    Command::ReviewSubmitRequestChanges,
                    "review.thread.button",
                ),
            ];
            let right = area.x + area.width - 1;
            let mut x = area.x + 1;
            for (label, command, key) in buttons {
                let w = label.width() as u16;
                if x + w > right && x > area.x + 1 {
                    by += 1;
                    x = area.x + 1;
                }
                if by >= body_end || x + w > right {
                    break;
                }
                out.set_stringn(x, by, label, w as usize, theme.get(key));
                hit.pending_buttons.push((Rect::new(x, by, w, 1), command));
                x += w + 1;
            }
            by = (by + 1).min(body_end);
        }
        let rows = body_end.saturating_sub(by) as usize;
        p.height = rows;
        let top = p.top(len);
        let selected = p.selected(len);
        hit.pending_list = Rect {
            y: by,
            height: rows as u16,
            ..area
        };
        hit.pending_top = top;
        hit.pending_len = len;
        for (k, row) in drafts.iter().enumerate().skip(top).take(rows) {
            let ry = by + (k - top) as u16;
            let mut row_style = base;
            if selected == Some(k) {
                row_style = base.patch(if p.focused { selected_style } else { inactive });
                out.set_style(
                    Rect {
                        y: ry,
                        height: 1,
                        ..area
                    },
                    row_style,
                );
            }
            out.set_stringn(area.x + 1, ry, "●", 1, row_style.patch(theme.get("review.unresolved")));
            let place = row.draft.place();
            let room = width.saturating_sub(4);
            let place = truncate(&place, room);
            out.set_stringn(area.x + 3, ry, &place, room, row_style);
            let used = place.width() + 2;
            if used < room {
                let failed = state.drafts.errors.get(&row.draft.id);
                let (text, style) = match row.held {
                    _ if failed.is_some() => (
                        format!("failed: {}", failed.map_or("", |e| e.as_str())),
                        row_style.patch(error_fg),
                    ),
                    Some(why) => (
                        format!("held back: {why}"),
                        row_style.patch(theme.get("review.pending.held")),
                    ),
                    None => (
                        format!(
                            "{}{}{}",
                            row.draft.tags(),
                            where_tag(
                                row.draft,
                                forge_name,
                                drafts_on_forge,
                                state.drafts.local_why.get(&row.draft.id)
                            ),
                            row.draft.body.lines().next().unwrap_or("").trim()
                        ),
                        row_style.patch(dim_fg),
                    ),
                };
                out.set_stringn(
                    area.x + 3 + used as u16,
                    ry,
                    truncate(&text, room - used),
                    room - used,
                    style,
                );
            }
        }
    }
    (Some(hit), bar)
}

/// Where a draft is kept, before its text: `on GitHub · ` once the forge has it, `local only: why · `
/// when it was refused (`local only · ` for any other draft that isn't on it), nothing with
/// the setting off (Claude's drafts carry their `[claude]` tag before it).
fn where_tag(d: &crate::review_drafts::Draft, forge: Option<&str>, on: bool, why: Option<&String>) -> String {
    if let (Some(_), Some(name)) = (&d.forge, forge) {
        format!("on {name} · ")
    } else if let Some(why) = why {
        format!("local only: {why} · ")
    } else if on {
        "local only · ".into()
    } else {
        String::new()
    }
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

#[cfg(test)]
mod where_tests {
    use super::where_tag;
    use crate::review_drafts::{Draft, ForgeDraft};

    #[test]
    fn a_draft_says_where_it_is_kept() {
        let mut d = Draft::new("p".into(), true, 1, 1, "c".into(), vec![], "b".into());
        assert_eq!(where_tag(&d, Some("GitHub"), false, None), "");
        assert_eq!(where_tag(&d, Some("GitHub"), true, None), "local only · ");
        let why = "no".to_string();
        assert_eq!(where_tag(&d, Some("GitHub"), true, Some(&why)), "local only: no · ");
        d.forge = Some(ForgeDraft {
            review_id: "1".into(),
            comment_id: "2".into(),
        });
        assert_eq!(where_tag(&d, Some("GitHub"), true, None), "on GitHub · ");
        d.forge = None;
        d.by_claude = true;
        assert_eq!(where_tag(&d, Some("GitHub"), true, None), "local only · ");
    }
}
