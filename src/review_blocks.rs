//! Thread blocks in a review diff tab (no app state, no drawing): comment threads shown between
//! the code's rows. `diff_view::Row` stays as it is; this layer maps the screen's *visual* rows
//! (the code's rows plus the blocks' rows) to the aligned rows and back, so ordinary diff tabs
//! (no blocks) see the identity.
//!
//! - [`Blocks`]: the sorted `Block { after_row, height }`s with prefix sums, so every conversion
//!   is a binary search and a frame only walks the rows it draws ([`Blocks::walk`]).
//! - [`Threads`]: what a `DiffTab` keeps: the threads for its file as plain data, which are
//!   collapsed, and the cached layout (anchor rows when the rows or threads change, heights when
//!   the width or a collapse changes).
//! - [`lines`]: one thread's text lines for a width (drawing computes them for visible blocks only).

use std::collections::HashSet;
use std::sync::Arc;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff_view::Aligned;

/// Rows under a thread's comments for buttons (Reply / Resolve / Apply, added by later subtasks).
fn button_rows(_: &ThreadBlock) -> usize {
    0
}

/// Columns the blocks are indented by (under the line numbers) and kept free on the right.
pub const INDENT: usize = 3;
const MARGIN: usize = 1;

/// A review thread as the tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadBlock {
    /// The forge's thread id (the collapse state is kept by it).
    pub id: String,
    /// The side the lines belong to (`true`: the work tree's).
    pub right: bool,
    /// The thread's last line (0-based) on that side: the block goes under its row.
    pub line: usize,
    pub resolved: bool,
    /// `(author, body)` of each comment, in order.
    pub comments: Vec<(String, String)>,
}

/// A block of `height` rows shown after aligned row `after_row`; `thread` indexes
/// `Threads::list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub after_row: usize,
    pub height: usize,
    pub thread: usize,
}

/// What a visual row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// An aligned row.
    Row(usize),
    /// Row `offset` of block `index` (of [`Blocks::list`]).
    Block { index: usize, offset: usize },
}

/// The blocks of a tab, sorted by `after_row`, with prefix sums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocks {
    list: Vec<Block>,
    /// The visual row each block starts at.
    starts: Vec<usize>,
    /// `cum[i]`: the heights of blocks before `i` added up (`cum[len]`: all).
    cum: Vec<usize>,
}

impl Default for Blocks {
    fn default() -> Blocks {
        Blocks::new(Vec::new())
    }
}

impl Blocks {
    /// `list` sorted by `after_row` (ties keep their order).
    pub fn new(mut list: Vec<Block>) -> Blocks {
        list.sort_by_key(|b| b.after_row);
        let mut starts = Vec::with_capacity(list.len());
        let mut cum = Vec::with_capacity(list.len() + 1);
        let mut sum = 0;
        for b in &list {
            cum.push(sum);
            starts.push(b.after_row + 1 + sum);
            sum += b.height;
        }
        cum.push(sum);
        Blocks { list, starts, cum }
    }

    pub fn list(&self) -> &[Block] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The visual row aligned row `row` is shown on.
    pub fn visual_of_row(&self, row: usize) -> usize {
        row + self.cum[self.list.partition_point(|b| b.after_row < row)]
    }

    /// The visual rows of `rows` aligned rows.
    pub fn visual_len(&self, rows: usize) -> usize {
        rows + self.cum[self.list.len()]
    }

    /// The visual row block `i` starts at.
    #[cfg(test)]
    pub fn start(&self, i: usize) -> usize {
        self.starts[i]
    }

    /// What visual row `v` shows (a row past the end maps past the last aligned row).
    pub fn locate(&self, v: usize) -> Spot {
        let k = self.starts.partition_point(|&s| s <= v);
        if k > 0 && v < self.starts[k - 1] + self.list[k - 1].height {
            return Spot::Block {
                index: k - 1,
                offset: v - self.starts[k - 1],
            };
        }
        Spot::Row(v - self.cum[k])
    }

    /// The aligned row at visual row `v`, else (a block) the row it follows.
    pub fn row_near(&self, v: usize) -> usize {
        match self.locate(v) {
            Spot::Row(r) => r,
            Spot::Block { index, .. } => self.list[index].after_row,
        }
    }

    /// The visual rows from `top`, at most `n` of them, of a layout of `rows` aligned rows.
    pub fn walk(&self, top: usize, n: usize, rows: usize) -> Walk<'_> {
        let state = match self.locate(top) {
            Spot::Row(r) => State::Row(r, self.list.partition_point(|b| b.after_row < r)),
            Spot::Block { index, offset } => State::Block(index, offset),
        };
        Walk {
            blocks: self,
            state,
            left: n,
            rows,
        }
    }

    /// The first block after aligned row `row` (strictly), else, wrapping, the first; `None`
    /// without blocks. With `forward` false the last block before it.
    pub fn step(&self, row: usize, forward: bool) -> Option<usize> {
        if self.list.is_empty() {
            return None;
        }
        if forward {
            let i = self.list.partition_point(|b| b.after_row <= row);
            Some(if i == self.list.len() { 0 } else { i })
        } else {
            let i = self.list.partition_point(|b| b.after_row < row);
            Some(if i == 0 { self.list.len() - 1 } else { i - 1 })
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    /// About to emit aligned row `.0`; `.1`: the first block with `after_row >= .0`.
    Row(usize, usize),
    Block(usize, usize),
}

/// The iterator of [`Blocks::walk`].
pub struct Walk<'a> {
    blocks: &'a Blocks,
    state: State,
    left: usize,
    rows: usize,
}

impl Iterator for Walk<'_> {
    type Item = Spot;

    fn next(&mut self) -> Option<Spot> {
        if self.left == 0 {
            return None;
        }
        let list = &self.blocks.list;
        let (item, next) = match self.state {
            State::Row(r, bi) => {
                if r >= self.rows {
                    return None;
                }
                let next = if bi < list.len() && list[bi].after_row == r {
                    State::Block(bi, 0)
                } else {
                    State::Row(r + 1, bi)
                };
                (Spot::Row(r), next)
            }
            State::Block(bi, offset) => {
                let next = if offset + 1 < list[bi].height {
                    State::Block(bi, offset + 1)
                } else if bi + 1 < list.len() && list[bi + 1].after_row == list[bi].after_row {
                    State::Block(bi + 1, 0)
                } else {
                    State::Row(list[bi].after_row + 1, bi + 1)
                };
                (Spot::Block { index: bi, offset }, next)
            }
        };
        self.state = next;
        self.left -= 1;
        Some(item)
    }
}

/// The threads of a review tab and the layout built from them.
#[derive(Debug, Clone, Default)]
pub struct Threads {
    list: Arc<Vec<ThreadBlock>>,
    /// Threads whose collapse differs from the default (resolved ones start collapsed).
    toggled: HashSet<String>,
    list_version: u64,
    toggle_version: u64,
    /// The review generation `list` was read from (`App::sync_review_threads`).
    pub synced: Option<u64>,
    anchors_key: Option<(usize, u64, u64)>,
    /// The aligned row each thread's block follows (`None`: its line isn't in the diff).
    anchors: Vec<Option<usize>>,
    size_key: Option<((usize, u64, u64), usize, u64)>,
    blocks: Blocks,
}

impl Threads {
    pub fn set(&mut self, list: Vec<ThreadBlock>) {
        if *self.list != list {
            self.list = Arc::new(list);
            self.list_version += 1;
        }
    }

    pub fn list(&self) -> &[ThreadBlock] {
        &self.list
    }

    pub fn blocks(&self) -> &Blocks {
        &self.blocks
    }

    pub fn is_collapsed(&self, thread: usize) -> bool {
        let t = &self.list[thread];
        t.resolved != self.toggled.contains(&t.id)
    }

    /// Collapses / expands thread `thread`.
    pub fn toggle(&mut self, thread: usize) {
        let id = self.list[thread].id.clone();
        if !self.toggled.remove(&id) {
            self.toggled.insert(id);
        }
        self.toggle_version += 1;
    }

    /// Builds what changed since the last call: the anchor rows when the rows or threads did
    /// (one pass over the rows), the heights when the width or a collapse did.
    pub fn fit(&mut self, aligned: &Aligned, doc: (usize, u64), width: usize) {
        if self.list.is_empty() {
            if !self.blocks.is_empty() {
                self.blocks = Blocks::default();
            }
            self.anchors_key = None;
            self.size_key = None;
            return;
        }
        let akey = (doc.0, doc.1, self.list_version);
        if self.anchors_key != Some(akey) {
            self.anchors_key = Some(akey);
            self.size_key = None;
            let mut left = std::collections::HashMap::new();
            let mut right = std::collections::HashMap::new();
            let wanted: HashSet<(bool, usize)> = self.list.iter().map(|t| (t.right, t.line)).collect();
            for (i, r) in aligned.rows.iter().enumerate() {
                if let Some(l) = r.left
                    && wanted.contains(&(false, l))
                {
                    left.insert(l, i);
                }
                if let Some(l) = r.right
                    && wanted.contains(&(true, l))
                {
                    right.insert(l, i);
                }
            }
            self.anchors = self
                .list
                .iter()
                .map(|t| if t.right { right.get(&t.line) } else { left.get(&t.line) }.copied())
                .collect();
        }
        let skey = (akey, width, self.toggle_version);
        if self.size_key != Some(skey) {
            self.size_key = Some(skey);
            let list = self
                .anchors
                .iter()
                .enumerate()
                .filter_map(|(i, a)| {
                    let after_row = (*a)?;
                    let height = height(&self.list[i], self.is_collapsed(i), width);
                    Some(Block {
                        after_row,
                        height,
                        thread: i,
                    })
                })
                .collect();
            self.blocks = Blocks::new(list);
        }
    }
}

/// The width the lines of a block are laid out to, for a body `body` columns wide.
pub fn block_width(body: usize) -> usize {
    body.saturating_sub(INDENT + MARGIN).max(12)
}

/// Which line of a block this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `┌ author ──── ▾`
    Header,
    /// `├ author ────` before a reply.
    Reply,
    Body,
    /// Room for the buttons (drawn by later subtasks).
    Buttons,
    Footer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockLine {
    pub kind: Kind,
    pub text: String,
}

/// The columns (from the block's left edge) of a header's collapse toggle `▾` / `▸`.
pub fn toggle_cols(width: usize) -> std::ops::Range<usize> {
    width.saturating_sub(3)..width
}

/// A thread's height in rows at `width` (the width of [`block_width`]).
pub fn height(t: &ThreadBlock, collapsed: bool, width: usize) -> usize {
    if collapsed {
        return 1;
    }
    let mut h = 1 + button_rows(t) + 1;
    for (i, (_, body)) in t.comments.iter().enumerate() {
        h += usize::from(i > 0) + wrap(body, width.saturating_sub(2)).len();
    }
    h
}

/// A thread's lines at `width`.
pub fn lines(t: &ThreadBlock, collapsed: bool, width: usize) -> Vec<BlockLine> {
    let mark = if collapsed { "▸" } else { "▾" };
    let first = t.comments.first();
    let author = first.map_or("", |c| c.0.as_str());
    let done = if t.resolved { " ✓" } else { "" };
    let mut out = Vec::new();
    let mut extra = String::new();
    let head = if collapsed {
        let body = first.and_then(|c| c.1.lines().next()).unwrap_or("").replace('\t', " ");
        let more = t.comments.len().saturating_sub(1);
        extra = if more > 0 { format!(" (+{more})") } else { String::new() };
        format!("┌ {author}{done}: {body}")
    } else {
        format!("┌ {author}{done}")
    };
    out.push(BlockLine {
        kind: Kind::Header,
        text: rule(&head, &format!("{extra} {mark} "), width),
    });
    if collapsed {
        return out;
    }
    for (i, (who, body)) in t.comments.iter().enumerate() {
        if i > 0 {
            out.push(BlockLine {
                kind: Kind::Reply,
                text: rule(&format!("├ {who}"), "", width),
            });
        }
        for l in wrap(body, width.saturating_sub(2)) {
            out.push(BlockLine {
                kind: Kind::Body,
                text: format!("│ {l}"),
            });
        }
    }
    out.extend((0..button_rows(t)).map(|_| BlockLine {
        kind: Kind::Buttons,
        text: "│".into(),
    }));
    out.push(BlockLine {
        kind: Kind::Footer,
        text: format!("└{}", "─".repeat(width.saturating_sub(1))),
    });
    out
}

/// `left` (cut with `…` to fit), then `─` up to `right` at the end, exactly `width` columns.
fn rule(left: &str, right: &str, width: usize) -> String {
    let room = width.saturating_sub(right.width());
    let mut text = cut(left, room);
    let fill = room.saturating_sub(text.width());
    if fill > 0 {
        text.push(' ');
        text.push_str(&"─".repeat(fill - 1));
    }
    text.push_str(right);
    text
}

/// `s` cut to `width` columns, ending in `…` when cut.
fn cut(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// `text` wrapped to `width` columns: at spaces, else mid-word; its line breaks kept.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        // A tab (or any control character) has no width of its own and is not drawn.
        let line = line.trim_end_matches('\r').replace('\t', "    ");
        let line = line.as_str();
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut w = 0;
        for word in line.split(' ') {
            let ww = word.width();
            if w > 0 && w + 1 + ww <= width {
                cur.push(' ');
                cur.push_str(word);
                w += 1 + ww;
                continue;
            }
            if w > 0 {
                out.push(std::mem::take(&mut cur));
                w = 0;
            }
            if ww <= width {
                cur.push_str(word);
                w = ww;
                continue;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if w + cw > width {
                    out.push(std::mem::take(&mut cur));
                    w = 0;
                }
                cur.push(c);
                w += cw;
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(spec: &[(usize, usize)]) -> Blocks {
        Blocks::new(
            spec.iter()
                .enumerate()
                .map(|(thread, &(after_row, height))| Block {
                    after_row,
                    height,
                    thread,
                })
                .collect(),
        )
    }

    fn thread(id: &str, resolved: bool, body: &str) -> ThreadBlock {
        ThreadBlock {
            id: id.into(),
            right: true,
            line: 0,
            resolved,
            comments: vec![("sam".into(), body.into())],
        }
    }

    #[test]
    fn no_blocks_is_the_identity() {
        let b = Blocks::default();
        assert_eq!(b.visual_of_row(7), 7);
        assert_eq!(b.visual_len(10), 10);
        assert_eq!(b.locate(4), Spot::Row(4));
        assert_eq!(b.row_near(4), 4);
        assert_eq!(b.step(3, true), None);
        let all: Vec<_> = b.walk(2, 3, 10).collect();
        assert_eq!(all, vec![Spot::Row(2), Spot::Row(3), Spot::Row(4)]);
    }

    #[test]
    fn rows_map_to_visual_rows_and_back() {
        // Rows 0..6; a 3-row block after row 1, a 2-row one after row 1 too, one after row 5.
        let b = blocks(&[(1, 3), (1, 2), (5, 4)]);
        assert_eq!(b.visual_len(6), 15);
        let expect = [0, 1, 7, 8, 9, 10];
        for (r, v) in expect.iter().enumerate() {
            assert_eq!(b.visual_of_row(r), *v, "row {r}");
            assert_eq!(b.locate(*v), Spot::Row(r));
        }
        assert_eq!(b.locate(2), Spot::Block { index: 0, offset: 0 });
        assert_eq!(b.locate(4), Spot::Block { index: 0, offset: 2 });
        assert_eq!(b.locate(5), Spot::Block { index: 1, offset: 0 });
        assert_eq!(b.locate(6), Spot::Block { index: 1, offset: 1 });
        assert_eq!(b.locate(11), Spot::Block { index: 2, offset: 0 });
        assert_eq!(b.locate(14), Spot::Block { index: 2, offset: 3 });
        assert_eq!(b.start(2), 11);
        assert_eq!(b.row_near(3), 1);
        // Past the end: rows past the last.
        assert_eq!(b.locate(15), Spot::Row(6));
    }

    #[test]
    fn block_at_the_start_and_the_end() {
        let b = blocks(&[(0, 2), (3, 1)]);
        assert_eq!(b.visual_of_row(0), 0);
        assert_eq!(b.visual_of_row(1), 3);
        assert_eq!(b.visual_of_row(3), 5);
        assert_eq!(b.visual_len(4), 7);
        assert_eq!(b.locate(6), Spot::Block { index: 1, offset: 0 });
    }

    #[test]
    fn walking_visits_every_visual_row_once() {
        let b = blocks(&[(1, 3), (1, 2), (5, 4)]);
        let all: Vec<_> = b.walk(0, 100, 6).collect();
        assert_eq!(all.len(), b.visual_len(6));
        for (v, s) in all.iter().enumerate() {
            assert_eq!(*s, b.locate(v), "visual row {v}");
        }
        // Starting inside a block, with a limit.
        let part: Vec<_> = b.walk(3, 4, 6).collect();
        assert_eq!(part, all[3..7]);
    }

    #[test]
    fn stepping_wraps() {
        let b = blocks(&[(1, 1), (4, 1)]);
        assert_eq!(b.step(0, true), Some(0));
        assert_eq!(b.step(1, true), Some(1));
        assert_eq!(b.step(4, true), Some(0));
        assert_eq!(b.step(4, false), Some(0));
        assert_eq!(b.step(1, false), Some(1));
        assert_eq!(b.step(5, false), Some(1));
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_long_words() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("a\n\nb", 9), ["a", "", "b"]);
        assert_eq!(wrap("日本語 x", 4), ["日本", "語 x"]);
        // Tabs become spaces (a control character would be dropped when drawn).
        assert_eq!(wrap("a\tb", 9), ["a    b"]);
        // No line is wider than the width, whatever the text: wide characters, emoji, long words.
        for text in [
            "日本語日本語日本語日本語日本語",
            "😀😀😀😀😀😀😀😀 😀 ok",
            &"x".repeat(50),
            "a  b   c\t\td",
            "",
        ] {
            for w in 1..14 {
                for l in wrap(text, w) {
                    assert!(l.width() <= w.max(2), "{l:?} at {w}");
                }
            }
        }
        // The empty body is one (empty) line.
        assert_eq!(wrap("", 9), [""]);
    }

    #[test]
    fn lines_have_the_width_and_the_height_matches() {
        let mut t = thread("1", false, "This can underflow when the pane is narrow.");
        t.comments.push(("bob".into(), "agreed".into()));
        for width in [12, 20, 40] {
            let ls = lines(&t, false, width);
            assert_eq!(ls.len(), height(&t, false, width));
            for l in &ls {
                assert!(l.text.width() <= width, "{l:?}");
            }
            assert_eq!(ls[0].kind, Kind::Header);
            assert_eq!(ls.last().unwrap().kind, Kind::Footer);
            assert!(ls[0].text.ends_with(" ▾ "));
        }
        let c = lines(&t, true, 30);
        assert_eq!(c.len(), 1);
        assert!(c[0].text.ends_with(" ▸ "));
        assert!(c[0].text.contains("(+1)"));
        assert_eq!(height(&t, true, 30), 1);
    }

    #[test]
    fn resolved_threads_start_collapsed_and_toggle() {
        let mut ts = Threads::default();
        ts.set(vec![thread("a", true, "x"), thread("b", false, "y")]);
        assert!(ts.is_collapsed(0));
        assert!(!ts.is_collapsed(1));
        ts.toggle(0);
        ts.toggle(1);
        assert!(!ts.is_collapsed(0));
        assert!(ts.is_collapsed(1));
        ts.toggle(1);
        assert!(!ts.is_collapsed(1));
    }
}
