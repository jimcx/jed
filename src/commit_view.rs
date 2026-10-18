//! A commit tab: one commit as `git show --stat --format=medium` prints it, read only (no
//! drawing: `ui::commit`; no app state: `app/commit_tabs.rs` opens, loads and moves it).
//!
//! The text is [`git_show::Line`]s rendered for the tab's width by the load
//! ([`git_show::render`]); a width change re-renders them without reading git again (a commit
//! never changes). The cursor is a [`TextPos`] — a line and a character column in the line's
//! text — and a selection is the range between it and an anchor, as in a read-only editor.

use std::sync::Arc;

use crate::clipboard::Copied;
use crate::command::Motion;
use crate::git::show::Show;
use crate::git_show::{self, Line};
use crate::quick_diff::Want;
use crate::scrollbar::{Scroll, Scrollable};

/// The cursor: a line, and a character column in the line's text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub row: usize,
    pub col: usize,
}

impl TextPos {
    pub fn new(row: usize, col: usize) -> TextPos {
        TextPos { row, col }
    }
}

/// One commit in an editor tab.
#[derive(Debug)]
pub struct CommitTab {
    /// The full object id.
    pub commit: String,
    /// The work-tree-relative path of the file whose history opened the tab (`None`: unknown):
    /// its stat row is highlighted and Enter / a click opens its diff.
    pub file: Option<String>,
    /// `None` until the load ends.
    pub show: Option<Arc<Show>>,
    /// The rendered lines (`git_show::render` at `rendered_width`).
    lines: Arc<Vec<Line>>,
    rendered_width: usize,
    /// The last load failed: no retry until the tab is reopened (a commit never changes, so
    /// only a missing repository or commit can fail it).
    pub error: Option<String>,
    /// A load is running (`begin` / `finish`).
    pub running: bool,
    /// The first line shown.
    pub top: usize,
    pub cursor: TextPos,
    /// The selection's other end; a movement without Shift clears it.
    pub anchor: Option<TextPos>,
    /// The column Up / Down aim for.
    pub goal_col: usize,
    /// Scroll the cursor's line into view at the next draw (centred when off screen).
    pub reveal: bool,
    /// The lines that fit, written by drawing.
    pub height: usize,
}

impl CommitTab {
    pub fn new(commit: String, file: Option<String>) -> CommitTab {
        CommitTab {
            commit,
            file,
            show: None,
            lines: Arc::new(Vec::new()),
            rendered_width: 0,
            error: None,
            running: false,
            top: 0,
            cursor: TextPos::default(),
            anchor: None,
            goal_col: 0,
            reveal: false,
            height: 0,
        }
    }

    /// Whether to load: a commit never changes, so only a tab without a `show` (and not loading
    /// or failed) does.
    pub fn want(&self) -> Want {
        if self.show.is_some() || self.running || self.error.is_some() {
            Want::Nothing
        } else {
            Want::Now
        }
    }

    pub fn begin(&mut self) {
        self.running = true;
    }

    /// The load ended (`false`: not the one running). The first load puts the cursor at the top.
    pub fn finish(&mut self, result: Result<Show, String>) -> bool {
        if !self.running {
            return false;
        }
        self.running = false;
        match result {
            Ok(show) => {
                let show = Arc::new(show);
                if self.rendered_width > 0 {
                    self.lines = Arc::new(git_show::render(&show, self.rendered_width));
                }
                self.show = Some(show);
                self.error = None;
                self.top = 0;
                self.cursor = TextPos::default();
                self.anchor = None;
                self.goal_col = 0;
                self.reveal = false;
            }
            Err(e) => {
                tracing::warn!(commit = %self.commit, "git show failed: {e}");
                self.error = Some(e);
            }
        }
        true
    }

    /// The commit loaded (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))] // tests wait for it
    pub fn loaded(&self) -> bool {
        self.show.is_some()
    }

    /// The text at `width` columns, rendered again when the width changed (no I/O).
    pub fn lines(&mut self, width: usize) -> &[Line] {
        if let Some(show) = &self.show
            && (self.rendered_width != width || self.lines.is_empty())
        {
            self.lines = Arc::new(git_show::render(show, width));
            self.rendered_width = width;
        }
        &self.lines
    }

    pub fn rows(&self) -> usize {
        self.lines.len()
    }

    /// The tab bar's label: the commit's 7-hex short id, as a commit diff's title uses it.
    pub fn title(&self) -> String {
        self.commit[..7.min(self.commit.len())].to_string()
    }

    /// The file whose stat line `row` is, if it is one.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        self.lines.get(row).and_then(|l| l.file)
    }

    /// The row of `Show::files[k]`'s stat line.
    pub fn file_row(&self, k: usize) -> Option<usize> {
        self.lines.iter().position(|l| l.file == Some(k))
    }

    /// The row of the tab's own file's stat line (`file`, looked up among the commit's files).
    pub fn own_file_row(&self) -> Option<usize> {
        let file = self.file.as_deref()?;
        let show = self.show.as_ref()?;
        let k = show.files.iter().position(|f| f.path == file)?;
        self.file_row(k)
    }

    /// The selection's ordered ends, when it has one (an empty cursor has none).
    fn selection(&self) -> Option<(TextPos, TextPos)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(if anchor < self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        })
    }

    /// The characters selected on `row` (`None`: none there).
    pub fn row_selection(&self, row: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection()?;
        if row < start.row || row > end.row {
            return None;
        }
        let len = self.line_len(row);
        let from = if row == start.row { start.col.min(len) } else { 0 };
        let to = if row == end.row { end.col.min(len) } else { len };
        Some((from, to))
    }

    /// What Cmd+C copies: the selection's text, lines joined with `\n`; an empty cursor copies
    /// its whole line (and a line break), as the editor's does.
    pub fn copy_text(&self) -> Copied {
        let whole_line = |row: usize| Copied {
            text: format!("{}\n", self.line_text(row)),
            whole_line: true,
            pieces: Vec::new(),
        };
        let Some((start, end)) = self.selection() else {
            return whole_line(self.cursor.row);
        };
        let mut text = String::new();
        for row in start.row..=end.row {
            if row > start.row {
                text.push('\n');
            }
            let line = self.line_text(row);
            let len = line.chars().count();
            let from = if row == start.row { start.col.min(len) } else { 0 };
            let to = if row == end.row { end.col.min(len) } else { len };
            text.extend(line.chars().skip(from).take(to.saturating_sub(from)));
        }
        Copied {
            text,
            whole_line: false,
            pieces: Vec::new(),
        }
    }

    /// A copy for a split: the same commit, file and loaded text, its own cursor and scroll.
    pub fn copy(&self) -> CommitTab {
        CommitTab {
            commit: self.commit.clone(),
            file: self.file.clone(),
            show: self.show.clone(),
            lines: self.lines.clone(),
            rendered_width: self.rendered_width,
            error: self.error.clone(),
            // A load in flight reports to every tab showing the commit, this one too.
            running: self.running,
            top: self.top,
            cursor: self.cursor,
            anchor: self.anchor,
            goal_col: self.goal_col,
            reveal: false,
            height: self.height,
        }
    }

    /// Moves the cursor (Shift extends the selection), clamped to the text.
    pub fn move_motion(&mut self, motion: Motion, select: bool) {
        let before = self.cursor;
        let last = self.lines.len().saturating_sub(1);
        let page = self.height.saturating_sub(1).max(1);
        let mut goal = self.goal_col;
        let mut c = self.cursor;
        match motion {
            Motion::Up => {
                c.row = c.row.saturating_sub(1);
                c.col = goal;
            }
            Motion::Down => {
                c.row = (c.row + 1).min(last);
                c.col = goal;
            }
            Motion::PageUp => {
                c.row = c.row.saturating_sub(page);
                c.col = goal;
            }
            Motion::PageDown => {
                c.row = (c.row + page).min(last);
                c.col = goal;
            }
            // A read-only editor's lines flow into each other: Left at a line's start is the
            // previous line's end, Right at its end the next line's start.
            Motion::Left => {
                if c.col > 0 {
                    c.col -= 1;
                } else if c.row > 0 {
                    c.row -= 1;
                    c.col = self.line_len(c.row);
                }
            }
            Motion::Right => {
                let len = self.line_len(c.row);
                if c.col < len {
                    c.col += 1;
                } else if c.row < last {
                    c.row += 1;
                    c.col = 0;
                }
            }
            Motion::WordLeft => c.col = self.word_left(c.row, c.col),
            Motion::WordRight => c.col = self.word_right(c.row, c.col),
            Motion::LineStart => c.col = 0,
            Motion::LineEnd => c.col = self.line_len(c.row),
            Motion::DocStart => (c.row, c.col) = (0, 0),
            Motion::DocEnd => {
                c.row = last;
                c.col = self.line_len(last);
            }
        }
        c.row = c.row.min(last);
        c.col = c.col.min(self.line_len(c.row));
        if !matches!(motion, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown) {
            goal = c.col;
        }
        self.cursor = c;
        self.goal_col = goal;
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(before);
            }
        } else {
            self.anchor = None;
        }
        self.reveal = true;
    }

    /// Cmd+A: the whole text selected.
    pub fn select_all(&mut self) {
        let last = self.lines.len().saturating_sub(1);
        self.anchor = Some(TextPos::new(0, 0));
        self.cursor = TextPos::new(last, self.line_len(last));
        self.goal_col = self.cursor.col;
        self.reveal = true;
    }

    /// Esc: no selection, the cursor stays.
    pub fn cancel_selection(&mut self) {
        self.anchor = None;
    }

    /// The cursor to a clicked place (the selection cleared).
    pub fn set_cursor(&mut self, pos: TextPos) {
        let row = pos.row.min(self.lines.len().saturating_sub(1));
        self.cursor = TextPos::new(row, pos.col.min(self.line_len(row)));
        self.goal_col = self.cursor.col;
        self.anchor = None;
        self.reveal = false;
    }

    /// A drag from `anchor` to `to`: the selection between them.
    pub fn drag_select(&mut self, anchor: TextPos, to: TextPos) {
        let row = to.row.min(self.lines.len().saturating_sub(1));
        self.cursor = TextPos::new(row, to.col.min(self.line_len(row)));
        self.anchor = Some(anchor);
        self.goal_col = self.cursor.col;
        self.reveal = false;
    }

    /// The char column at display column `x` (0 at the line's start) of `row`, clamped to the
    /// line's end: a click or a drag.
    pub fn col_at(&self, row: usize, x: usize) -> usize {
        let mut used = 0;
        for (i, c) in self.line_text(row).chars().enumerate() {
            let w = display_width(c);
            if used + w > x {
                return i;
            }
            used += w;
        }
        self.line_len(row)
    }

    /// Scrolls by `delta` lines, the cursor staying put (the wheel).
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(self.height.max(1));
        self.top = self.top.saturating_add_signed(delta).min(max);
    }

    /// The scroll for `height` lines on screen: the cursor's line shown if asked (centred when
    /// it was off screen), the last page full.
    pub fn scroll_to_cursor(&mut self, height: usize) {
        self.height = height;
        let height = height.max(1);
        if self.reveal {
            self.reveal = false;
            if self.cursor.row < self.top || self.cursor.row >= self.top + height {
                self.top = self.cursor.row.saturating_sub(height / 2);
            }
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        let rows = self.lines.len();
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.row));
        self.top = self.top.min(rows.saturating_sub(self.height.max(1)));
    }

    /// The line's text ("" past the last).
    pub fn line_text(&self, row: usize) -> String {
        self.lines.get(row).map(Line::text).unwrap_or_default()
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, |l| l.text().chars().count())
    }

    fn word_left(&self, row: usize, col: usize) -> usize {
        let chars: Vec<char> = self.line_text(row).chars().collect();
        let mut i = col.min(chars.len());
        while i > 0 && !crate::buffer::pos::is_word_char(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && crate::buffer::pos::is_word_char(chars[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_right(&self, row: usize, col: usize) -> usize {
        let chars: Vec<char> = self.line_text(row).chars().collect();
        let mut i = col.min(chars.len());
        while i < chars.len() && !crate::buffer::pos::is_word_char(chars[i]) {
            i += 1;
        }
        while i < chars.len() && crate::buffer::pos::is_word_char(chars[i]) {
            i += 1;
        }
        i
    }
}

/// A char's columns in a drawn line (`\t` four).
pub fn display_width(c: char) -> usize {
    match c {
        '\t' => 4,
        c if c.is_control() => 0,
        c => unicode_width::UnicodeWidthChar::width(c).unwrap_or(0),
    }
}

impl Scrollable for CommitTab {
    fn scroll(&self) -> Scroll {
        Scroll::rows(self.lines.len(), self.height, self.top)
    }

    /// The pointer decides where the view is: no reveal of the cursor's line.
    fn set_top(&mut self, top: usize) {
        self.top = top;
        self.reveal = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::show::{Content, File};

    fn line(text: &str, kind: git_show::Kind) -> Line {
        Line {
            segments: vec![git_show::Segment {
                text: text.into(),
                kind,
            }],
            file: None,
        }
    }

    fn show() -> Show {
        Show {
            id: "abcdef0123456789abcdef0123456789abcdef01".into(),
            first_parent: None,
            parents: Vec::new(),
            author: "t".into(),
            email: "t@example.com".into(),
            time: 0,
            offset: 0,
            message: "first\n".into(),
            files: vec![
                File {
                    path: "a.txt".into(),
                    old_path: None,
                    content: Content::Text { added: 2, deleted: 0 },
                },
                File {
                    path: "b.txt".into(),
                    old_path: None,
                    content: Content::Text { added: 0, deleted: 1 },
                },
            ],
        }
    }

    /// A tab with hand-made lines (no git): the header's lines, then `a.txt`'s stat line.
    fn tab() -> CommitTab {
        let mut t = CommitTab::new("abcdef0123456789abcdef0123456789abcdef01".into(), Some("a.txt".into()));
        t.show = Some(Arc::new(show()));
        let mut stat = line(" a.txt | 2 ++", git_show::Kind::StatName);
        stat.file = Some(0);
        let mut stat_b = line(" b.txt | 1 -", git_show::Kind::StatName);
        stat_b.file = Some(1);
        t.lines = Arc::new(vec![
            line("commit abcdef0", git_show::Kind::Commit),
            line("Author: t <t@example.com>", git_show::Kind::Author),
            line("", git_show::Kind::Plain),
            line("    first", git_show::Kind::Message),
            line("", git_show::Kind::Plain),
            stat,
            stat_b,
            line(" 2 files changed", git_show::Kind::Summary),
        ]);
        t.rendered_width = 80;
        t
    }

    #[test]
    fn want_and_finish() {
        let mut t = CommitTab::new("a".repeat(40), None);
        assert_eq!(t.want(), Want::Now);
        t.begin();
        assert_eq!(t.want(), Want::Nothing, "running");
        assert!(t.finish(Ok(show())), "finishes");
        assert!(t.loaded());
        assert_eq!(t.want(), Want::Nothing, "loaded");
        assert_eq!(t.title(), "aaaaaaa");
        // A failed load isn't retried.
        let mut t = CommitTab::new("b".repeat(40), None);
        t.begin();
        assert!(t.finish(Err("gone".into())));
        assert_eq!(t.error.as_deref(), Some("gone"));
        assert_eq!(t.want(), Want::Nothing);
        assert!(!t.finish(Ok(show())), "nothing running");
    }

    #[test]
    fn lines_re_render_at_a_new_width() {
        let mut t = CommitTab::new("c".repeat(40), None);
        t.begin();
        assert!(t.finish(Ok(show())));
        assert_eq!(t.rendered_width, 0, "nothing drawn yet");
        assert_eq!(t.lines(20).len(), t.rows(), "rendered on the first draw");
        assert_eq!(t.rows(), 9, "the header, message and two stat lines");
        let narrow = t.lines(20)[0].text();
        assert!(narrow.starts_with("commit "), "{narrow:?}");
        assert_eq!(t.rendered_width, 20);
        // A width change renders again at the new width (a commit is read once).
        let at_20 = t.lines(20).iter().map(Line::text).collect::<Vec<_>>().join("\n");
        let at_70 = t.lines(70).iter().map(Line::text).collect::<Vec<_>>().join("\n");
        assert_eq!(t.rendered_width, 70);
        assert_eq!(t.lines(70).len(), 9, "the same lines at another width");
        assert_eq!(at_20, at_70, "this stat fits either width");
        // A tab without a show keeps its lines.
        let mut t = CommitTab::new("d".repeat(40), None);
        assert!(t.lines(70).is_empty());
    }

    /// A show whose stat is laid out differently at different widths: a long name and many
    /// changed lines (git shrinks the graph and truncates the name).
    fn wide_show() -> Show {
        let mut show = show();
        show.files = vec![File {
            path: "a_very_long_file_name/that_needs/the_whole_width.txt".into(),
            old_path: None,
            content: Content::Text { added: 100, deleted: 0 },
        }];
        show
    }

    fn text_at(t: &mut CommitTab, width: usize) -> String {
        t.lines(width).iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn the_stat_is_rendered_for_the_width() {
        let mut t = CommitTab::new("c".repeat(40), None);
        t.begin();
        assert!(t.finish(Ok(wide_show())));
        let narrow = text_at(&mut t, 30);
        let wide = text_at(&mut t, 90);
        assert_ne!(narrow, wide, "the stat depends on the tab's width");
        assert_eq!(t.rendered_width, 90, "re-rendered at the new width");
        // And back again, with no show read twice.
        assert_eq!(text_at(&mut t, 30), narrow);
    }

    #[test]
    fn stat_rows_name_their_file() {
        let t = tab();
        assert_eq!(t.file_at(0), None);
        assert_eq!(t.file_at(5), Some(0));
        assert_eq!(t.file_at(6), Some(1));
        assert_eq!(t.file_row(0), Some(5));
        assert_eq!(t.file_row(1), Some(6));
        assert_eq!(t.file_row(2), None);
        assert_eq!(t.own_file_row(), Some(5), "the tab's file");
        let mut t = tab();
        t.file = Some("b.txt".into());
        assert_eq!(t.own_file_row(), Some(6));
        t.file = Some("gone.txt".into());
        assert_eq!(t.own_file_row(), None);
    }

    #[test]
    fn movements_clamp_and_shift_selects() {
        let mut t = tab();
        t.height = 3;
        t.move_motion(Motion::DocEnd, false);
        assert_eq!(t.cursor, TextPos::new(7, " 2 files changed".chars().count()));
        t.move_motion(Motion::LineStart, false);
        assert_eq!(t.cursor, TextPos::new(7, 0));
        t.move_motion(Motion::Up, false);
        assert_eq!(t.cursor, TextPos::new(6, 0), "Up at column 0");
        t.move_motion(Motion::LineEnd, false);
        assert_eq!(t.cursor.col, " b.txt | 1 -".chars().count());
        // Down keeps the goal column as far as the line allows.
        t.move_motion(Motion::Down, false);
        assert_eq!((t.cursor.row, t.cursor.col), (7, 12), "the goal column kept");
        t.move_motion(Motion::Up, false);
        assert_eq!(t.cursor.col, " b.txt | 1 -".chars().count(), "the goal column");
        // Page moves by the view's height less one.
        t.move_motion(Motion::DocStart, false);
        t.move_motion(Motion::PageDown, false);
        assert_eq!(t.cursor.row, 2);
        t.move_motion(Motion::PageDown, false);
        assert_eq!(t.cursor.row, 4);
        // Shift extends from where it started; another movement without Shift clears it.
        t.cursor = TextPos::new(1, 2);
        t.move_motion(Motion::Down, true);
        assert_eq!(t.anchor, Some(TextPos::new(1, 2)));
        t.move_motion(Motion::Down, true);
        assert_eq!(t.anchor, Some(TextPos::new(1, 2)), "the anchor stays");
        t.move_motion(Motion::Down, false);
        assert_eq!(t.anchor, None);
        // Word movement.
        let mut t = tab();
        t.cursor = TextPos::new(1, "Author: t <t@example.com>".chars().count());
        t.move_motion(Motion::WordLeft, false);
        assert_eq!(t.line_text(1).chars().skip(t.cursor.col).collect::<String>(), "com>");
        t.move_motion(Motion::WordLeft, false);
        assert_eq!(
            t.line_text(1).chars().skip(t.cursor.col).collect::<String>(),
            "example.com>"
        );
        // Select all and cancel.
        t.select_all();
        assert_eq!(t.anchor, Some(TextPos::new(0, 0)));
        assert_eq!(t.cursor.row, 7);
        t.cancel_selection();
        assert_eq!(t.anchor, None);
        // An empty tab's movements stay at the start.
        let mut empty = CommitTab::new("e".repeat(40), None);
        empty.move_motion(Motion::Down, false);
        empty.move_motion(Motion::DocEnd, false);
        assert_eq!((empty.cursor, empty.top), (TextPos::default(), 0));
    }

    #[test]
    fn left_and_right_cross_line_ends() {
        let mut t = tab();
        // Left at a line's start: the previous line's end; home stays at the line's start.
        t.cursor = TextPos::new(1, 0);
        t.move_motion(Motion::Left, false);
        assert_eq!(t.cursor, TextPos::new(0, t.line_len(0)));
        t.move_motion(Motion::LineStart, false);
        assert_eq!(t.cursor, TextPos::new(0, 0));
        // Left at the document's first column stays.
        t.move_motion(Motion::Left, false);
        assert_eq!(t.cursor, TextPos::new(0, 0));
        // Right at a line's end: the next line's start; End stays at the line's end.
        t.move_motion(Motion::LineEnd, false);
        assert_eq!(t.cursor, TextPos::new(0, t.line_len(0)));
        t.move_motion(Motion::Right, false);
        assert_eq!(t.cursor, TextPos::new(1, 0));
        t.move_motion(Motion::LineEnd, false);
        t.move_motion(Motion::Right, false);
        assert_eq!(t.cursor, TextPos::new(2, 0), "an empty line crossed");
        // Right at the document's end stays.
        t.move_motion(Motion::DocEnd, false);
        t.move_motion(Motion::Right, false);
        assert_eq!(t.cursor, TextPos::new(7, t.line_len(7)));
        // Shift extends across lines from the anchor.
        t.cursor = TextPos::new(0, 1);
        t.move_motion(Motion::Right, true);
        t.move_motion(Motion::Right, true);
        assert_eq!(t.anchor, Some(TextPos::new(0, 1)));
        assert_eq!(t.cursor, TextPos::new(0, 3));
        for _ in 0..12 {
            t.move_motion(Motion::Right, true);
        }
        assert_eq!(t.cursor, TextPos::new(1, 0), "crossed into the next line");
        t.move_motion(Motion::Right, true);
        assert_eq!(t.cursor, TextPos::new(1, 1));
        assert_eq!(t.row_selection(0), Some((1, t.line_len(0))), "the first line's end");
        assert_eq!(t.row_selection(1), Some((0, 1)), "the next line's start");
    }

    #[test]
    fn copy_text_is_the_selection_or_the_line() {
        let mut t = tab();
        // An empty cursor: its whole line and a line break.
        t.cursor = TextPos::new(1, 3);
        let copied = t.copy_text();
        assert_eq!(copied.text, "Author: t <t@example.com>\n");
        assert!(copied.whole_line && copied.pieces.is_empty());
        // One row's range (a char range, end exclusive).
        t.anchor = Some(TextPos::new(1, 0));
        t.cursor = TextPos::new(1, 6);
        assert_eq!(t.copy_text().text, "Author");
        assert!(!t.copy_text().whole_line);
        // Several rows: the middle ones whole, the ends cut.
        t.anchor = Some(TextPos::new(1, 8));
        t.cursor = TextPos::new(3, 5);
        assert_eq!(t.copy_text().text, "t <t@example.com>\n\n    f");
        // Reversed ends (the anchor after the cursor) copy the same text, in reading order.
        t.anchor = Some(TextPos::new(3, 5));
        t.cursor = TextPos::new(1, 8);
        assert_eq!(t.copy_text().text, "t <t@example.com>\n\n    f");
        assert_eq!(t.row_selection(1), Some((8, 25)), "the first selected row");
        assert_eq!(t.row_selection(3), Some((0, 5)), "the last selected row");
        assert_eq!(t.row_selection(0), None);
        assert_eq!(t.row_selection(4), None);
        // The same place either way is still no selection.
        t.cursor = TextPos::new(3, 5);
        t.anchor = Some(TextPos::new(3, 5));
        assert_eq!(t.copy_text().text, "    first\n");
        // The selected characters of a row, for drawing.
        t.anchor = Some(TextPos::new(1, 2));
        t.cursor = TextPos::new(3, 4);
        assert_eq!(t.row_selection(1), Some((2, 25)));
        assert_eq!(t.row_selection(2), Some((0, 0)));
        assert_eq!(t.row_selection(3), Some((0, 4)));
        assert_eq!(t.row_selection(4), None);
    }

    #[test]
    fn splitting_copies_the_tab_with_its_own_place() {
        let mut t = tab();
        t.cursor = TextPos::new(5, 3);
        t.anchor = Some(TextPos::new(4, 0));
        t.top = 2;
        t.height = 4;
        let copy = t.copy();
        assert_eq!(copy.commit, t.commit);
        assert_eq!(copy.file, t.file);
        assert!(Arc::ptr_eq(copy.show.as_ref().unwrap(), t.show.as_ref().unwrap()));
        assert_eq!((copy.cursor, copy.anchor, copy.top), (t.cursor, t.anchor, 2));
        // Its own cursor from here.
        let mut copy = copy;
        copy.move_motion(Motion::Down, false);
        assert_eq!(t.cursor, TextPos::new(5, 3), "the original stayed");
        // A copy of a running load finishes with the original's event.
        let mut loading = CommitTab::new("f".repeat(40), None);
        loading.begin();
        let mut copy = loading.copy();
        assert!(copy.running && copy.want() == Want::Nothing);
        assert!(copy.finish(Ok(show())));
        assert!(copy.loaded());
    }

    #[test]
    fn clicking_maps_a_cell_to_a_char() {
        let t = tab();
        assert_eq!(t.col_at(1, 0), 0);
        assert_eq!(t.col_at(1, 5), 5);
        assert_eq!(t.col_at(1, 200), t.line_len(1), "past the end");
        // A wide char takes two cells: the cell after it is its end.
        let mut t = tab();
        t.lines = Arc::new(vec![line("a中 b", git_show::Kind::Plain)]);
        assert_eq!(t.col_at(0, 1), 1);
        assert_eq!(t.col_at(0, 2), 1, "inside the wide char");
        assert_eq!(t.col_at(0, 3), 2);
    }

    #[test]
    fn the_scroll_bar_scrolls_the_lines() {
        let mut t = tab();
        t.height = 3;
        assert_eq!(t.scroll(), Scroll::rows(8, 3, 0));
        t.scroll_by(100);
        assert_eq!(t.scroll().top, 5);
        t.reveal = true;
        t.set_top(1);
        assert_eq!(t.top, 1);
        assert!(!t.reveal, "the pointer decides");
        t.cursor = TextPos::new(7, 0);
        t.reveal = true;
        t.scroll_to_cursor(3);
        assert_eq!(t.top, 5, "the last page, the cursor shown");
        assert!(!t.reveal);
    }
}
