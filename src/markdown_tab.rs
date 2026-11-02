//! A Markdown preview tab (`group::Tab::Markdown`): the rendered `markdown::Doc` of a text
//! buffer, read only, with a caret (line, display column) and a selection for movement and copy,
//! the scroll (rows; columns when a table cannot shrink to the width) and the width it was laid
//! out at. `ui::markdown` fits it to the area each frame (`fit`: re-renders when the width
//! changes, clamps the scroll, reveals a moved caret); `app/markdown.rs` runs its commands and
//! mouse. It keeps a snapshot of the source text (`text`), so a split copy re-renders on its own.
//! `app/markdown.rs` keeps it in step with the source (re-render after edits, scroll following,
//! source closed); the fields below marked "follow" are that bookkeeping.

use std::path::PathBuf;
use std::sync::Arc;

use unicode_width::UnicodeWidthChar;

use crate::buffer::BufferId;
use crate::buffer::pos::is_word_char;
use crate::command::Motion;
use crate::markdown::{self, Doc, Kind};
use crate::scrollbar::ListScroll;

/// A place in the rendered text: line index and display column (on a character boundary).
pub type Place = (usize, usize);

#[derive(Debug, Clone)]
pub struct MarkdownTab {
    /// Unique per tab opened (a split's copy gets its own).
    pub id: u64,
    /// The buffer previewed.
    pub source: BufferId,
    /// The source's path (`None`: untitled), for the title and tooltip.
    pub path: Option<PathBuf>,
    /// The source's display name ("README.md", "Untitled-1"), as it was when opened.
    pub name: String,
    /// The source text as rendered.
    pub text: Arc<str>,
    pub doc: Doc,
    /// The width `doc` was laid out at.
    pub width: usize,
    /// The first row shown, the first column shown.
    pub top: usize,
    pub left: usize,
    /// Rows and columns on screen, as drawn last.
    pub height: usize,
    pub view_width: usize,
    pub cursor: Place,
    /// The column Up / Down aim for.
    pub goal: usize,
    pub anchor: Option<Place>,
    /// The next frame scrolls the caret into view.
    pub reveal: bool,
    /// Follow: the source buffer's revision `text` was taken at.
    pub revision: u64,
    /// Follow: the latest source revision seen (a change restarts the debounce).
    pub seen: u64,
    /// Follow: counts background renders asked for; only the newest result is taken.
    pub generation: u64,
    /// Follow: the (revision, width) a background render was asked for, still running.
    pub requested: Option<(u64, usize)>,
    /// Follow: scrolling with the source's top line (false once the user scrolled the preview,
    /// until the source scrolls again).
    pub following: bool,
    /// Follow: the source's top line as last seen (`usize::MAX`: not yet, a restored preview).
    pub src_top: usize,
    /// Follow: no tab shows the source any more (the title says so; nothing updates it).
    pub closed: bool,
}

/// Documents up to this many bytes render on the UI thread (a few ms); bigger ones in the
/// background.
pub const SYNC_LIMIT: usize = 32 * 1024;

fn next_id() -> u64 {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Display columns at which each character of `text` starts, then its width (the line's end).
fn bounds(text: &str) -> Vec<usize> {
    let mut v = Vec::with_capacity(text.len() + 1);
    let mut col = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if w > 0 {
            v.push(col);
        }
        col += w;
    }
    v.push(col);
    v
}

impl MarkdownTab {
    /// A preview of `text`, the source's text at `revision`, at an assumed 80 columns until the
    /// first frame fits it. Rendered now when small; a big document starts empty and counts as
    /// stale (`App::sync_markdown` renders it in the background).
    pub fn new(source: BufferId, path: Option<PathBuf>, name: String, text: Arc<str>, revision: u64) -> Self {
        let (doc, revision) = if text.len() <= SYNC_LIMIT {
            (markdown::render(&text, 80), revision)
        } else {
            (Doc::default(), u64::MAX)
        };
        Self {
            id: next_id(),
            source,
            path,
            name,
            text,
            doc,
            width: 80,
            top: 0,
            left: 0,
            height: 1,
            view_width: 80,
            cursor: (0, 0),
            goal: 0,
            anchor: None,
            reveal: false,
            revision,
            seen: revision,
            generation: 0,
            requested: None,
            following: true,
            src_top: 0,
            closed: false,
        }
    }

    /// Takes a finished render of `text` (source `revision`, laid out at `width`): the caret
    /// stays on its line and the scroll where it was, both clamped at the next `fit`.
    pub fn apply(&mut self, text: Arc<str>, doc: Doc, width: usize, revision: u64) {
        self.text = text;
        self.doc = doc;
        self.width = width;
        self.revision = revision;
        self.requested = None;
        let last = self.doc.lines.len().saturating_sub(1);
        self.cursor.0 = self.cursor.0.min(last);
        self.cursor.1 = self.snap(self.cursor.0, self.cursor.1);
        if self.anchor.is_some_and(|a| a.0 > last) {
            self.anchor = None;
        }
        self.top = self.top.min(self.max_top());
        self.left = self.left.min(self.max_left());
    }

    /// Scrolls to follow the source's top line `line` of `lines`, in proportion to the rows.
    pub fn follow(&mut self, line: usize, lines: usize) {
        self.top = (line * self.rows() / lines.max(1)).min(self.max_top());
    }

    /// A split's copy: its own id and scroll state, the same source.
    pub fn copy(&self) -> Self {
        Self {
            id: next_id(),
            ..self.clone()
        }
    }

    /// The tab's title.
    pub fn title(&self) -> String {
        if self.closed {
            format!("Preview {} (source closed)", self.name)
        } else {
            format!("Preview {}", self.name)
        }
    }

    /// Fits to the area drawn: re-renders when `width` changed, clamps the scroll and the
    /// caret, scrolls a moved caret into view.
    pub fn fit(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        self.height = height.max(1);
        self.view_width = width;
        // A big document is laid out again in the background (`App::sync_markdown`).
        if width != self.width && self.text.len() <= SYNC_LIMIT {
            self.doc = markdown::render(&self.text, width);
            self.width = width;
        }
        let last = self.doc.lines.len().saturating_sub(1);
        self.cursor.0 = self.cursor.0.min(last);
        self.cursor.1 = self.snap(self.cursor.0, self.cursor.1);
        if self.anchor.is_some_and(|a| a.0 > last) {
            self.anchor = None;
        }
        if self.reveal {
            self.reveal = false;
            self.following = false;
            let (l, c) = self.cursor;
            if l < self.top {
                self.top = l;
            } else if l >= self.top + self.height {
                self.top = l + 1 - self.height;
            }
            if c < self.left {
                self.left = c;
            } else if c >= self.left + width {
                self.left = c + 1 - width;
            }
        }
        // Not rendered yet (a big document): a restored scroll waits for the rows (`apply`).
        if self.revision != u64::MAX {
            self.top = self.top.min(self.max_top());
        }
        self.left = self.left.min(self.max_left());
    }

    /// What scrolls sideways: the widest rendered line (`ScrollWidth`'s rules: the bar's row
    /// kept while it is wider than the view or the view is scrolled; the last column one blank
    /// past it).
    pub fn scroll_width(&self) -> crate::scrollbar::ScrollWidth {
        let mut w = crate::scrollbar::ScrollWidth::default();
        w.measure(self.doc.max_width());
        w
    }

    /// The largest `left`: 0 while everything fits.
    fn max_left(&self) -> usize {
        if self.doc.overflows(self.view_width) {
            self.scroll_width().max_left(self.view_width)
        } else {
            0
        }
    }

    pub fn rows(&self) -> usize {
        self.doc.lines.len()
    }

    fn max_top(&self) -> usize {
        self.list().max_top(self.rows())
    }

    fn list(&self) -> ListScroll {
        ListScroll {
            top: self.top,
            height: self.height,
        }
    }

    /// Scrolls by `rows` (the wheel), the caret staying where it is.
    pub fn scroll_by(&mut self, rows: isize) {
        self.following = false;
        self.top = self.top.saturating_add_signed(rows).min(self.max_top());
    }

    /// Scrolls sideways by `cols` (only a document wider than the view moves).
    pub fn scroll_cols(&mut self, cols: isize) {
        self.left = self.left.saturating_add_signed(cols).min(self.max_left());
    }

    fn line_text(&self, line: usize) -> String {
        self.doc.lines.get(line).map(|l| l.text()).unwrap_or_default()
    }

    /// `col` moved back to the character boundary at or before it (and within the line).
    fn snap(&self, line: usize, col: usize) -> usize {
        let b = bounds(&self.line_text(line));
        b.iter().rev().find(|&&c| c <= col).copied().unwrap_or(0)
    }

    /// The caret moved by `motion`; Shift (`select`) keeps / starts the selection, else drops it.
    pub fn motion(&mut self, motion: Motion, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        let (line, col) = self.cursor;
        let last = self.rows().saturating_sub(1);
        let text = self.line_text(line);
        let b = bounds(&text);
        let at = b.iter().position(|&c| c == col).unwrap_or(0);
        let mut keep_goal = false;
        self.cursor = match motion {
            Motion::Left if at > 0 => (line, b[at - 1]),
            Motion::Left if line > 0 => (line - 1, *bounds(&self.line_text(line - 1)).last().unwrap_or(&0)),
            Motion::Right if at + 1 < b.len() => (line, b[at + 1]),
            Motion::Right if line < last => (line + 1, 0),
            Motion::Left | Motion::Right => (line, col),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let n = match motion {
                    Motion::Up => 1,
                    Motion::Down => 1,
                    _ => self.height.saturating_sub(1).max(1),
                };
                let to = if matches!(motion, Motion::Up | Motion::PageUp) {
                    line.saturating_sub(n)
                } else {
                    (line + n).min(last)
                };
                keep_goal = true;
                if matches!(motion, Motion::PageUp | Motion::PageDown) {
                    let delta = to as isize - line as isize;
                    self.top = (self.top as isize + delta).clamp(0, self.max_top() as isize) as usize;
                }
                (to, self.snap(to, self.goal))
            }
            Motion::LineStart => (line, 0),
            Motion::LineEnd => (line, *b.last().unwrap_or(&0)),
            Motion::DocStart => (0, 0),
            Motion::DocEnd => (last, *bounds(&self.line_text(last)).last().unwrap_or(&0)),
            Motion::WordLeft | Motion::WordRight => self.word_move(motion == Motion::WordRight),
        };
        if !keep_goal {
            self.goal = self.cursor.1;
        }
        self.reveal = true;
    }

    /// One word to the left / right: over spaces, then a run of word characters or of
    /// punctuation; at a line's end onto the next / previous line.
    fn word_move(&self, forward: bool) -> Place {
        let (line, col) = self.cursor;
        let chars: Vec<(usize, char)> = {
            let mut c = 0;
            self.line_text(line)
                .chars()
                .map(|ch| {
                    let s = (c, ch);
                    c += ch.width().unwrap_or(0);
                    s
                })
                .collect()
        };
        let end = bounds(&self.line_text(line)).last().copied().unwrap_or(0);
        let class = |ch: char| {
            if ch.is_whitespace() {
                0
            } else if is_word_char(ch) {
                1
            } else {
                2
            }
        };
        if forward {
            if col >= end {
                return if line + 1 < self.rows() {
                    (line + 1, 0)
                } else {
                    (line, col)
                };
            }
            let mut i = chars.iter().position(|&(c, _)| c >= col).unwrap_or(chars.len());
            while i < chars.len() && class(chars[i].1) == 0 {
                i += 1;
            }
            if let Some(&(_, ch)) = chars.get(i) {
                let k = class(ch);
                while i < chars.len() && class(chars[i].1) == k {
                    i += 1;
                }
            }
            (line, chars.get(i).map_or(end, |&(c, _)| c))
        } else {
            if col == 0 {
                return if line > 0 {
                    (line - 1, bounds(&self.line_text(line - 1)).last().copied().unwrap_or(0))
                } else {
                    (line, col)
                };
            }
            let mut i = chars.iter().position(|&(c, _)| c >= col).unwrap_or(chars.len());
            while i > 0 && class(chars[i - 1].1) == 0 {
                i -= 1;
            }
            if i > 0 {
                let k = class(chars[i - 1].1);
                while i > 0 && class(chars[i - 1].1) == k {
                    i -= 1;
                }
            }
            (line, chars.get(i).map_or(0, |&(c, _)| c))
        }
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        let last = self.rows().saturating_sub(1);
        self.anchor = Some((0, 0));
        self.cursor = (last, bounds(&self.line_text(last)).last().copied().unwrap_or(0));
        self.goal = self.cursor.1;
        self.reveal = true;
    }

    /// The caret put at screen cell (`row`, `col`) of the text area (`extend`: keeps / starts
    /// the selection, else drops it).
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        let line = (self.top + row).min(self.rows().saturating_sub(1));
        self.cursor = (line, self.snap(line, self.left + col));
        self.goal = self.cursor.1;
    }

    /// The selection, ordered, when it isn't empty.
    pub fn selection(&self) -> Option<(Place, Place)> {
        let a = self.anchor?;
        (a != self.cursor).then(|| {
            if a < self.cursor {
                (a, self.cursor)
            } else {
                (self.cursor, a)
            }
        })
    }

    /// The selected columns of `line` (`None`: not selected; a line break inside the selection
    /// counts one cell past the end).
    pub fn selected_cols(&self, line: usize) -> Option<std::ops::Range<usize>> {
        let (s, e) = self.selection()?;
        if line < s.0 || line > e.0 {
            return None;
        }
        let end = self.doc.lines.get(line).map_or(0, |l| l.width());
        let from = if line == s.0 { s.1 } else { 0 };
        let to = if line == e.0 { e.1 } else { end + 1 };
        (from < to).then_some(from..to)
    }

    /// The selected rendered text (code labels left out), lines joined by `\n`.
    pub fn copy_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        let mut out = Vec::new();
        for line in s.0..=e.0.min(self.rows().saturating_sub(1)) {
            if self.doc.lines[line].kind == Kind::CodeLabel {
                continue;
            }
            let from = if line == s.0 { s.1 } else { 0 };
            let to = if line == e.0 { e.1 } else { usize::MAX };
            let mut col = 0;
            let mut piece = String::new();
            for ch in self.doc.lines[line].text().chars() {
                if col >= from && col < to {
                    piece.push(ch);
                }
                col += ch.width().unwrap_or(0);
            }
            out.push(piece);
        }
        Some(out.join("\n"))
    }
}

impl crate::scrollbar::Scrollable for MarkdownTab {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.rows(), self.height, self.top)
    }

    /// The pointer decides where the view is: no reveal of the caret.
    fn set_top(&mut self, top: usize) {
        self.top = top.min(self.max_top());
        self.reveal = false;
        self.following = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(text: &str) -> MarkdownTab {
        let mut t = MarkdownTab::new(1, None, "x.md".into(), text.into(), 0);
        t.fit(40, 5);
        t
    }

    #[test]
    fn moves_select_and_copy() {
        let mut t = tab("hello world\n\nsecond line\n");
        t.motion(Motion::WordRight, true);
        assert_eq!(t.copy_text().as_deref(), Some("hello"));
        t.motion(Motion::DocEnd, true);
        assert!(t.copy_text().unwrap().ends_with("second line"));
        t.motion(Motion::Left, false);
        assert!(t.selection().is_none());
        t.select_all();
        assert_eq!(t.copy_text().unwrap(), "hello world\n\nsecond line");
    }

    #[test]
    fn wide_chars_step_whole() {
        let mut t = tab("a\u{4e2d}b\n");
        t.motion(Motion::Right, false);
        t.motion(Motion::Right, false);
        assert_eq!(t.cursor, (0, 3));
        t.motion(Motion::Left, false);
        assert_eq!(t.cursor, (0, 1));
    }

    #[test]
    fn max_top_keeps_the_last_page_full() {
        let text = (0..30).map(|i| format!("line {i}\n\n")).collect::<String>();
        let mut t = tab(&text);
        t.fit(40, 5);
        assert_eq!(t.max_top(), t.rows() - 5);
        t.fit(40, 0);
        assert_eq!(t.max_top(), t.rows() - 1, "a height of 0 counts as 1");
    }

    #[test]
    fn refits_to_the_width_and_scrolls() {
        let text = (0..30).map(|i| format!("line {i}\n\n")).collect::<String>();
        let mut t = tab(&text);
        t.motion(Motion::DocEnd, false);
        t.fit(40, 5);
        assert_eq!(t.top, t.rows() - 5);
        t.fit(20, 5);
        assert_eq!(t.width, 20);
    }

    #[test]
    fn code_labels_are_not_copied() {
        let mut t = tab("```rust\nlet a = 1;\n```\n");
        t.select_all();
        let copied = t.copy_text().unwrap();
        assert!(copied.contains("let a = 1;") && !copied.contains("rust"));
    }
}
