//! The diff view's model: two texts of one file compared line by line in an editor tab (no
//! drawing: `ui::diff`; no app state: `app/diff_view.rs` opens, loads and stages).
//!
//! A [`DiffTab`] compares a [`Pair`] of [`Text`]s of one file: the working tree against the
//! index (unstaged changes), the index against HEAD (staged ones), or the file on disk against
//! the open buffer (the changed-on-disk banner's Show Diff). A "live" side (the working tree, the
//! buffer) is the open buffer's text when the file is open, so unsaved edits show (VS Code), else
//! the file on disk. Both sides are read only.
//!
//! Loading ([`load`], blocking) reads both texts, diffs them with `git::diff` (context 0: one
//! hunk per change) and aligns them into rows ([`align`]): unchanged lines side by side, a
//! change's old lines beside its new ones (the extra lines of the longer side against filler),
//! or, in the one-column (inline) layout, a change's old lines above its new ones. Both layouts
//! are built once per load, so drawing and scrolling only index into them. Each side's syntax
//! colours are computed in the load too ([`Doc::highlight`]: the side's whole text parsed, so a
//! comment or string over several lines colours every one of them).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use git2::Oid;
use ropey::Rope;

use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::git::Repo;
use crate::git::diff::{self, Hunk};
use crate::git::stage::{self, FileDiff, Side};
use crate::history::{self, Snapshot};
use crate::quick_diff::Want;
use crate::syntax::{self, Language, Span};

/// How long after an edit of a live side's buffer the diff is taken again (the gutter's delay).
pub const DELAY: Duration = crate::quick_diff::DELAY;

/// A diff tab narrower than this shows one column (inline) instead of two.
pub const SIDE_BY_SIDE_MIN: u16 = 90;

/// One side of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Text {
    /// The file in HEAD's tree (empty when not in it).
    Head,
    /// The file in the index (empty when not in it).
    Index,
    /// The open buffer's text, else the file on disk.
    WorkTree,
    /// The file on disk, even when it's open.
    Disk,
    /// The open buffer's text; the file on disk while it isn't open (File History's Compare of
    /// a file not open: the diff reads the buffer once the file opens).
    Buffer,
    /// An entry of the file's history (`.jed/history/`).
    History(Snapshot),
    /// The file's blob in a commit's tree (a commit tab's stat row: the parent's text against
    /// the commit's). Empty when the commit doesn't have the path.
    Commit(Oid),
    /// The empty tree: the left side of a root commit's diff (it has no parent to compare
    /// against; git compares it with the empty tree).
    Empty,
}

impl Text {
    pub fn title(self) -> String {
        match self {
            Text::Head => "HEAD".into(),
            Text::Index => "Index".into(),
            Text::WorkTree => "Working Tree".into(),
            Text::Disk => "On Disk".into(),
            Text::Buffer => "Yours".into(),
            Text::History(s) => format!("History: {}", history::ago(s.time, history::now_ms())),
            Text::Commit(id) => short_commit(id),
            Text::Empty => "Empty Tree".into(),
        }
    }

    /// Read from the open buffer when there is one.
    pub fn live(self) -> bool {
        matches!(self, Text::WorkTree | Text::Buffer)
    }

    fn git(self) -> bool {
        matches!(self, Text::Head | Text::Index | Text::Commit(_) | Text::Empty)
    }
}

/// A commit id's short form (7 hex digits, as the tab titles show it).
fn short_commit(id: Oid) -> String {
    let id = id.to_string();
    id[..7.min(id.len())].to_string()
}

/// The two texts a diff tab compares: `left` (old) and `right` (new).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pair {
    pub left: Text,
    pub right: Text,
}

impl Pair {
    /// Unstaged changes: the working tree against the index.
    pub const UNSTAGED: Pair = Pair {
        left: Text::Index,
        right: Text::WorkTree,
    };
    /// Staged changes: the index against HEAD.
    pub const STAGED: Pair = Pair {
        left: Text::Head,
        right: Text::Index,
    };
    /// The changed-on-disk banner's Show Diff: the file on disk against the buffer.
    pub const SAVED: Pair = Pair {
        left: Text::Disk,
        right: Text::Buffer,
    };

    /// File History's Compare: an entry against the buffer.
    pub fn history(snapshot: Snapshot) -> Pair {
        Pair {
            left: Text::History(snapshot),
            right: Text::Buffer,
        }
    }

    /// A commit's change to a file (a commit tab's stat row): the parent's blob against the
    /// commit's (`parent` `None` for a root commit: the empty tree against the commit's blob).
    pub fn commit(commit: Oid, parent: Option<Oid>) -> Pair {
        Pair {
            left: parent.map_or(Text::Empty, Text::Commit),
            right: Text::Commit(commit),
        }
    }

    /// The history entry compared, if any.
    pub fn snapshot(self) -> Option<Snapshot> {
        match self.left {
            Text::History(s) => Some(s),
            _ => None,
        }
    }

    pub fn git(side: Side) -> Pair {
        match side {
            Side::Unstaged => Pair::UNSTAGED,
            Side::Staged => Pair::STAGED,
        }
    }

    /// What this view stages: its hunks (`Unstaged`: Stage) or unstages them (`Staged`).
    pub fn staging(self) -> Option<Side> {
        match self {
            Pair::UNSTAGED => Some(Side::Unstaged),
            Pair::STAGED => Some(Side::Staged),
            _ => None,
        }
    }

    /// Reads git's texts: needs a repository.
    pub fn needs_repo(self) -> bool {
        self.left.git() || self.right.git()
    }

    /// The tab's title for the file `name` (VS Code's: `name (Working Tree)`, `name (Index)`;
    /// a commit's change: `name (abc1234)`).
    pub fn tab_title(self, name: &str) -> String {
        match self {
            Pair::UNSTAGED | Pair::STAGED => format!("{name} ({})", self.right.title()),
            Pair {
                right: Text::Commit(id),
                ..
            } => format!("{name} ({})", short_commit(id)),
            Pair {
                left: Text::History(_),
                right: Text::Buffer,
            } => format!("{name} ({})", self.left.title()),
            _ => format!("{name} ({} ↔ {})", self.left.title(), self.right.title()),
        }
    }

    /// The side read from the open buffer, if any.
    fn live(self) -> bool {
        self.left.live() || self.right.live()
    }
}

/// One side's text as lines (a UTF-8 BOM dropped, invalid UTF-8 replaced): git's lines, each
/// ending after a `\n` (shown without it, or without `\r\n`); a last line without one is a line
/// too, and an empty text has none.
#[derive(Debug, Default)]
pub struct Lines {
    text: String,
    starts: Vec<usize>,
    /// Syntax colours over `text` (byte ranges of it: line breaks counted), sorted; empty when
    /// not highlighted (plain text, too large, binary).
    spans: Vec<Span>,
}

impl Lines {
    pub fn new(bytes: &[u8]) -> Lines {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let text = String::from_utf8_lossy(bytes).into_owned();
        let mut starts = Vec::new();
        if !text.is_empty() {
            starts.push(0);
            starts.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&i| i < text.len()));
        }
        Lines {
            text,
            starts,
            spans: Vec::new(),
        }
    }

    /// Blocking: colours the whole text in `language` (one parse, so a comment or string over
    /// several lines colours each of them), unless it's over `max_bytes` (0: never).
    fn highlight(&mut self, language: Language, max_bytes: usize) {
        if max_bytes == 0 || self.text.len() > max_bytes {
            return;
        }
        self.spans = syntax::highlight_text(language, &self.text);
    }

    /// Where line `i` starts in the text (its length past the end): spans' coordinates.
    pub fn start(&self, i: usize) -> usize {
        self.starts.get(i).copied().unwrap_or(self.text.len())
    }

    /// The syntax colours (empty: plain).
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The first span ending after line `i`'s start (a binary search: where a walk over the
    /// visible lines starts).
    pub fn first_span(&self, i: usize) -> usize {
        let start = self.start(i);
        self.spans.partition_point(|s| s.end.0 <= start)
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    /// Line `i`'s text without its line break ("" past the end).
    pub fn line(&self, i: usize) -> &str {
        let Some(&start) = self.starts.get(i) else { return "" };
        let end = self.starts.get(i + 1).copied().unwrap_or(self.text.len());
        let l = &self.text[start..end];
        let l = l.strip_suffix('\n').unwrap_or(l);
        l.strip_suffix('\r').unwrap_or(l)
    }
}

/// One row of the view: a line of either side or both, and the hunk it belongs to (`None`:
/// unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub hunk: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Same,
    /// A left line only (the right side is filler, or the row is the inline layout's `-`).
    Removed,
    /// A right line only.
    Added,
    /// A changed line beside the line it became.
    Changed,
}

impl Row {
    pub fn kind(&self) -> Kind {
        match (self.left, self.right, self.hunk) {
            (Some(_), Some(_), None) => Kind::Same,
            (Some(_), Some(_), Some(_)) => Kind::Changed,
            (Some(_), None, _) => Kind::Removed,
            _ => Kind::Added,
        }
    }

    /// The row's line on the right side, else on the left (where the cursor is).
    fn line(&self) -> (bool, usize) {
        match (self.right, self.left) {
            (Some(r), _) => (true, r),
            (None, Some(l)) => (false, l),
            _ => (true, 0),
        }
    }
}

/// The rows of one layout, and each hunk's first row.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Aligned {
    pub rows: Vec<Row>,
    pub starts: Vec<usize>,
}

impl Aligned {
    /// The hunk whose first row is the next after `row` (`forward`) or the last before it,
    /// wrapping around; `None` without hunks.
    pub fn step(&self, row: usize, forward: bool) -> Option<usize> {
        let s = &self.starts;
        if forward {
            s.iter().position(|&r| r > row).or((!s.is_empty()).then_some(0))
        } else {
            s.iter().rposition(|&r| r < row).or(s.len().checked_sub(1))
        }
    }

    /// The row showing line `line` of the right side (`right`) or the left, else the nearest
    /// row before where it would be.
    fn row_of(&self, right: bool, line: usize) -> usize {
        let side = |r: &Row| if right { r.right } else { r.left };
        // Rows are in increasing line order on both sides; filler rows have no line.
        let mut best = 0;
        for (i, r) in self.rows.iter().enumerate() {
            match side(r) {
                Some(l) if l == line => return i,
                Some(l) if l > line => break,
                Some(_) => best = i,
                None => {}
            }
        }
        best
    }
}

/// The rows of a diff whose (context 0) `hunks` turn `left_len` lines into `right_len`: one
/// column (`inline`: a change's old lines, then its new ones) or two (side by side).
pub fn align(hunks: &[Hunk], left_len: usize, right_len: usize, inline: bool) -> Aligned {
    let mut out = Aligned {
        rows: Vec::with_capacity(left_len.max(right_len) + 16),
        starts: Vec::with_capacity(hunks.len()),
    };
    let (mut l, mut r) = (0, 0);
    let same = |out: &mut Aligned, l: &mut usize, r: &mut usize, until: usize| {
        while *l < until && *l < left_len && *r < right_len {
            out.rows.push(Row {
                left: Some(*l),
                right: Some(*r),
                hunk: None,
            });
            *l += 1;
            *r += 1;
        }
    };
    for (k, h) in hunks.iter().enumerate() {
        same(&mut out, &mut l, &mut r, h.old.start);
        // Unchanged lines are in step, so `r` is now `h.new.start`.
        let (old, new) = (h.old.clone(), h.new.clone());
        out.starts.push(out.rows.len());
        let hunk = Some(k);
        if inline {
            out.rows.extend(old.clone().map(|i| Row {
                left: Some(i),
                right: None,
                hunk,
            }));
            out.rows.extend(new.clone().map(|i| Row {
                left: None,
                right: Some(i),
                hunk,
            }));
        } else {
            for i in 0..old.len().max(new.len()) {
                out.rows.push(Row {
                    left: (i < old.len()).then_some(old.start + i),
                    right: (i < new.len()).then_some(new.start + i),
                    hunk,
                });
            }
        }
        (l, r) = (old.end, new.end);
    }
    same(&mut out, &mut l, &mut r, usize::MAX);
    out
}

/// The parts of a changed pair of lines that differ: the byte ranges left after dropping their
/// common prefix and suffix (on char boundaries), for the text-level highlight.
pub fn changed_parts(old: &str, new: &str) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let (o, n) = (&old[prefix..], &new[prefix..]);
    let suffix: usize = o
        .chars()
        .rev()
        .zip(n.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (prefix..old.len() - suffix, prefix..new.len() - suffix)
}

/// A loaded diff: both sides' lines, the hunks between them, both layouts.
#[derive(Debug, Default)]
pub struct Doc {
    pub left: Lines,
    pub right: Lines,
    pub hunks: Vec<Hunk>,
    pub split: Aligned,
    pub inline: Aligned,
    /// Either text is binary: no rows.
    pub binary: bool,
    /// The texts as compared (BOM included), for staging a hunk exactly as shown
    /// (`stage::stage_hunk` / `unstage_hunk` refuse when the repository moved on).
    pub shown: FileDiff,
}

impl Doc {
    /// Blocking (the diff): `old` against `new`.
    pub fn new(old: Vec<u8>, new: Vec<u8>) -> Result<Doc, String> {
        let d = diff::diff(&old, &new, 0).map_err(|e| e.message().to_string())?;
        let (left, right) = (Lines::new(&old), Lines::new(&new));
        let (split, inline) = if d.binary {
            Default::default()
        } else {
            (
                align(&d.hunks, left.len(), right.len(), false),
                align(&d.hunks, left.len(), right.len(), true),
            )
        };
        Ok(Doc {
            left,
            right,
            hunks: d.hunks.clone(),
            split,
            inline,
            binary: d.binary,
            shown: FileDiff { old, new, diff: d },
        })
    }

    /// Blocking: colours both sides in `language` (`None`: the one the text's shebang names),
    /// each side only up to `max_bytes` (0: never); a binary diff stays plain.
    pub fn highlight(&mut self, language: Option<Language>, max_bytes: usize) {
        if self.binary {
            return;
        }
        let language = language.unwrap_or_else(|| {
            let first = if self.right.len() > 0 { &self.right } else { &self.left };
            syntax::detect_shebang(first.line(0))
        });
        if language.grammar().is_none() {
            return;
        }
        self.left.highlight(language, max_bytes);
        self.right.highlight(language, max_bytes);
    }

    pub fn aligned(&self, inline: bool) -> &Aligned {
        if inline { &self.inline } else { &self.split }
    }
}

/// The edit reverting hunk `k` of `shown` (an unstaged diff whose new side is an open buffer's
/// encoded text; `bom`: the buffer has one) in that buffer: the byte range of the hunk's lines
/// in the buffer's text (its BOM not counted) and the index's lines for it, bytes as stored.
/// `None`: no such hunk, or the index's lines aren't UTF-8.
pub fn revert_edit(shown: &FileDiff, k: usize, bom: bool) -> Option<(ByteIdx, ByteIdx, String)> {
    let h = shown.diff.hunks.get(k)?;
    let new = diff::lines(&shown.new);
    let len = |lines: &[&[u8]]| lines.iter().map(|l| l.len()).sum::<usize>();
    let start = len(&new[..h.new.start.min(new.len())]);
    let end = start + len(&new[h.new.start.min(new.len())..h.new.end.min(new.len())]);
    let shift = if bom { BOM.len() } else { 0 };
    let old = diff::lines(&shown.old);
    let mut text = old[h.old.start.min(old.len())..h.old.end.min(old.len())].concat();
    // The buffer keeps its own BOM: the index's isn't text.
    if h.old.start == 0 && text.starts_with(BOM) {
        text.drain(..BOM.len());
    }
    let text = String::from_utf8(text).ok()?;
    Some((
        ByteIdx(start.saturating_sub(shift)),
        ByteIdx(end.saturating_sub(shift)),
        text,
    ))
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// What `load` reads.
pub struct Job {
    pub repo: Option<Repo>,
    /// The workspace root (a history side reads `.jed/history/` there).
    pub root: PathBuf,
    /// Absolute.
    pub path: PathBuf,
    pub pair: Pair,
    /// The open buffer's text and BOM (a live side reads it).
    pub live: Option<(Rope, bool)>,
    /// The file's language for the syntax colours (the open buffer's, else by its path); `None`:
    /// the one its text's shebang names.
    pub language: Option<Language>,
    /// `Settings::syntax_max_bytes`: a bigger side stays plain (0: always).
    pub syntax_max_bytes: usize,
}

/// Blocking: reads both sides of `job` and diffs them.
pub fn load(job: Job) -> Result<Doc, String> {
    let left = read(&job, job.pair.left).map_err(|e| e.to_string())?;
    let right = read(&job, job.pair.right).map_err(|e| e.to_string())?;
    let mut doc = Doc::new(left, right)?;
    doc.highlight(job.language, job.syntax_max_bytes);
    Ok(doc)
}

fn read(job: &Job, text: Text) -> Result<Vec<u8>, stage::Error> {
    if text.live()
        && let Some((rope, bom)) = &job.live
    {
        return Ok(crate::buffer::file::encode(rope, *bom));
    }
    let rel = || -> Result<(&Repo, PathBuf), stage::Error> {
        let repo = job.repo.as_ref().ok_or_else(|| not_found("not in a git repository"))?;
        let rel = repo
            .relative(&job.path)
            .ok_or_else(|| not_found("outside the repository"))?;
        Ok((repo, rel))
    };
    let bytes = match text {
        Text::Head => {
            let (repo, rel) = rel()?;
            stage::head_text(repo, &rel)?.map(|t| t.bytes)
        }
        Text::Index => {
            let (repo, rel) = rel()?;
            stage::index_text(repo, &rel)?.map(|t| t.bytes)
        }
        Text::WorkTree => match &job.repo {
            Some(_) => {
                let (repo, rel) = rel()?;
                stage::disk_text(repo, &rel)?.map(|t| t.bytes)
            }
            None => disk(&job.path)?,
        },
        Text::Disk => disk(&job.path)?,
        Text::Buffer => disk(&job.path)?,
        Text::History(snapshot) => {
            let rel = history::relative(&job.root, &job.path).ok_or_else(|| not_found("outside the workspace"))?;
            let bytes = history::read_snapshot(&job.root, &rel, snapshot)
                .map_err(|e| stage::Error::Io(std::io::Error::other(e)))?;
            Some(bytes)
        }
        // The file's blob in the commit's tree; a missing path is empty (a deleted file, or a
        // root commit's empty-tree side).
        Text::Commit(id) => {
            let (repo, rel) = rel()?;
            let r = repo.open()?;
            let tree = r.find_commit(id)?.tree()?;
            match tree.get_path(&rel) {
                Ok(entry) => r.find_blob(entry.id()).ok().map(|b| b.content().to_vec()),
                Err(_) => None,
            }
        }
        Text::Empty => Some(Vec::new()),
    };
    Ok(bytes.unwrap_or_default())
}

fn disk(path: &Path) -> Result<Option<Vec<u8>>, stage::Error> {
    Ok(crate::buffer::file::read(path)?)
}

fn not_found(what: &str) -> stage::Error {
    stage::Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, what.to_string()))
}

/// A diff view in a tab: what it compares, the loaded diff, and where it's scrolled.
#[derive(Debug)]
pub struct DiffTab {
    /// Unique per tab (a split's copy gets its own): loads report back to it.
    pub id: u64,
    /// The file, absolute.
    pub path: PathBuf,
    pub pair: Pair,
    /// `None` until the first load.
    pub doc: Option<Arc<Doc>>,
    /// The last load failed.
    pub error: Option<String>,
    /// The first row shown and the current row (of the layout last drawn), the first display
    /// column shown.
    pub top: usize,
    pub cursor: usize,
    pub left_col: usize,
    /// Written by drawing: the rows that fit, and whether it drew one column.
    pub height: usize,
    pub inline: bool,
    /// Scroll the cursor's row into view at the next draw (centred when off screen).
    pub reveal: bool,
    /// Where the first load puts the cursor instead of the first change: the row of this line
    /// of the right side (`go_to_line` before the diff arrived).
    line: Option<usize>,
    generation: u64,
    running: Option<u64>,
    /// What the last load read the live side from: `Some(None)` the disk, `Some(Some(_))` that
    /// buffer at that revision; `None` before the first load.
    loaded: Option<Option<(BufferId, u64)>>,
    /// The index or the file changed since the last load: load again at once.
    stale: bool,
}

impl crate::scrollbar::Scrollable for DiffTab {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.rows().len(), self.height, self.top)
    }

    /// The pointer decides where the view is: no reveal of the cursor's row.
    fn set_top(&mut self, top: usize) {
        self.top = top;
        self.reveal = false;
    }
}

impl DiffTab {
    pub fn new(id: u64, path: PathBuf, pair: Pair) -> DiffTab {
        DiffTab {
            id,
            path,
            pair,
            doc: None,
            error: None,
            top: 0,
            cursor: 0,
            left_col: 0,
            height: 0,
            inline: false,
            reveal: false,
            line: None,
            generation: 0,
            running: None,
            loaded: None,
            stale: false,
        }
    }

    /// A copy for a split (same diff, same place), with its own `id`.
    pub fn copy(&self, id: u64) -> DiffTab {
        DiffTab {
            id,
            path: self.path.clone(),
            pair: self.pair,
            doc: self.doc.clone(),
            error: self.error.clone(),
            top: self.top,
            cursor: self.cursor,
            left_col: self.left_col,
            height: self.height,
            inline: self.inline,
            reveal: false,
            line: self.line,
            generation: 0,
            running: None,
            loaded: self.loaded,
            // A load in flight reports to the original only.
            stale: self.stale || self.running.is_some(),
        }
    }

    /// The file's name for the tab bar.
    pub fn title(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        self.pair.tab_title(&name)
    }

    /// Whether to load again, given the open buffer of the file and its revision (`None`: not
    /// open). A buffer opened or closed since loads at once; an edit after `DELAY`.
    pub fn want(&self, live: Option<(BufferId, u64)>) -> Want {
        let live = if self.pair.live() { live } else { None };
        match self.loaded {
            _ if self.running.is_some() => Want::Nothing,
            None => Want::Now,
            Some(_) if self.stale => Want::Now,
            Some(was) if was.map(|(b, _)| b) != live.map(|(b, _)| b) => Want::Now,
            Some(was) if was != live => Want::Later,
            Some(_) => Want::Nothing,
        }
    }

    /// The index or the file may have changed: load again.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// A load starts, reading the live side from `live`; returns its generation.
    pub fn begin(&mut self, live: Option<(BufferId, u64)>) -> u64 {
        self.generation += 1;
        self.running = Some(self.generation);
        self.stale = false;
        self.loaded = Some(if self.pair.live() { live } else { None });
        self.generation
    }

    /// A load ended (`generation` from `begin`; another one's result is dropped). The cursor
    /// stays on the line it was on, as near as the new diff allows.
    pub fn finish(&mut self, generation: u64, result: Result<Doc, String>) -> bool {
        if self.running != Some(generation) {
            return false;
        }
        self.running = None;
        match result {
            Ok(doc) => {
                let at = self.rows().get(self.cursor).map(Row::line);
                let first = self.doc.is_none();
                self.doc = Some(Arc::new(doc));
                self.error = None;
                match at {
                    Some((right, line)) => self.cursor = self.aligned().row_of(right, line),
                    None => self.cursor = 0,
                }
                if first {
                    // Opening goes to the line asked for, else the first change (VS Code).
                    if let Some(line) = self.line.take() {
                        self.cursor = self.aligned().row_of(true, line);
                    } else if let Some(&row) = self.aligned().starts.first() {
                        self.cursor = row;
                    }
                }
                self.clamp();
                self.reveal = true;
            }
            Err(e) => {
                tracing::warn!(path = %self.path.display(), "diff failed: {e}");
                self.error = Some(e);
            }
        }
        true
    }

    /// A load is running (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn loading(&self) -> bool {
        self.running.is_some()
    }

    /// The rows of the layout last drawn (empty before the first load).
    pub fn aligned(&self) -> &Aligned {
        static EMPTY: Aligned = Aligned {
            rows: Vec::new(),
            starts: Vec::new(),
        };
        self.doc.as_ref().map_or(&EMPTY, |d| d.aligned(self.inline))
    }

    pub fn rows(&self) -> &[Row] {
        &self.aligned().rows
    }

    /// Drawing found the layout to use: the cursor and scroll move to the same lines in it; a
    /// cursor on a change's first row (opening, next / previous change) stays on that change's
    /// first row, so where a first load lands doesn't depend on whether it was drawn before.
    pub fn set_layout(&mut self, inline: bool) {
        if inline == self.inline {
            return;
        }
        let cursor = self.rows().get(self.cursor).map(Row::line);
        let start = self.aligned().starts.binary_search(&self.cursor).ok();
        let top = self.rows().get(self.top).map(Row::line);
        self.inline = inline;
        if let Some(k) = start {
            self.cursor = self.aligned().starts[k];
        } else if let Some((right, line)) = cursor {
            self.cursor = self.aligned().row_of(right, line);
        }
        if let Some((right, line)) = top {
            self.top = self.aligned().row_of(right, line);
        }
        self.clamp();
    }

    /// The right side's line at the cursor (the file's, for Git: Toggle Changes / File): a
    /// removed line's is the next right line, where the removal was; 0 before the first load.
    pub fn cursor_line(&self) -> usize {
        let rows = self.rows();
        let after = rows.get(self.cursor..).unwrap_or_default();
        match after.iter().find_map(|r| r.right) {
            Some(line) => line,
            // Removed lines at the end: below the last right line.
            None => rows.iter().rev().find_map(|r| r.right).map_or(0, |l| l + 1),
        }
    }

    /// The cursor to the row of the right side's line `line` (else the nearest before it),
    /// scrolled into view; before the first load, where that load puts it.
    pub fn go_to_line(&mut self, line: usize) {
        if self.doc.is_none() {
            self.line = Some(line);
            return;
        }
        self.cursor = self.aligned().row_of(true, line);
        self.reveal = true;
    }

    /// The hunk the current row belongs to.
    pub fn hunk_at_cursor(&self) -> Option<usize> {
        self.rows().get(self.cursor).and_then(|r| r.hunk)
    }

    /// Next / previous change: the cursor to its first row, wrapping. False without changes.
    pub fn step(&mut self, forward: bool) -> bool {
        let Some(k) = self.aligned().step(self.cursor, forward) else {
            return false;
        };
        self.cursor = self.aligned().starts[k];
        self.reveal = true;
        true
    }

    /// Moves the cursor by `delta` rows (clamped).
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.rows().len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
        self.reveal = true;
    }

    /// The cursor to the first (`start`) or last row.
    pub fn cursor_to_end(&mut self, start: bool) {
        self.cursor = if start { 0 } else { self.rows().len().saturating_sub(1) };
        self.reveal = true;
    }

    /// Scrolls by `delta` rows, the cursor staying put (the wheel).
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.rows().len().saturating_sub(self.height.max(1));
        self.top = self.top.saturating_add_signed(delta).min(max);
    }

    /// Scrolls sideways by `delta` columns.
    pub fn scroll_columns(&mut self, delta: isize) {
        self.left_col = self.left_col.saturating_add_signed(delta);
    }

    /// The scroll for `height` rows on screen: the cursor's row shown if asked (centred when it
    /// was off screen), the last page full.
    pub fn scroll_to_cursor(&mut self, height: usize) {
        self.height = height;
        let height = height.max(1);
        if self.reveal {
            self.reveal = false;
            if self.cursor < self.top || self.cursor >= self.top + height {
                self.top = self.cursor.saturating_sub(height / 2);
            }
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        let n = self.rows().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
        self.top = self.top.min(n.saturating_sub(self.height.max(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(old: &str, new: &str, inline: bool) -> Vec<(Option<usize>, Option<usize>, Option<usize>)> {
        let doc = Doc::new(old.into(), new.into()).unwrap();
        doc.aligned(inline)
            .rows
            .iter()
            .map(|r| (r.left, r.right, r.hunk))
            .collect()
    }

    /// The buffer text (`new` without a BOM) with `revert_edit` of every hunk, one at a time,
    /// is `diff::revert` of the bytes without the BOM.
    #[test]
    fn revert_edit_is_the_hunk_reverted() {
        let cases: [(&[u8], &[u8]); 5] = [
            (b"a\nb\nc\nd\n", b"a\nB\nc\nd\ne\n"),
            (b"a\r\nb\r\nc", b"x\r\na\r\nc\r\nmore"),
            (b"\xEF\xBB\xBFa\nb\n", b"\xEF\xBB\xBFz\nb\nc\n"),
            (b"one\ntwo\n", b""),
            (b"", b"new\nlines"),
        ];
        for (old, new) in cases {
            let doc = Doc::new(old.to_vec(), new.to_vec()).unwrap();
            let bom = new.starts_with(BOM);
            let text = &new[if bom { BOM.len() } else { 0 }..];
            for k in 0..doc.hunks.len() {
                let (start, end, with) = revert_edit(&doc.shown, k, bom).unwrap();
                let mut got = text[..start.0].to_vec();
                got.extend_from_slice(with.as_bytes());
                got.extend_from_slice(&text[end.0..]);
                let want = diff::revert(old, new, &doc.hunks[k]);
                let want = want.strip_prefix(BOM).unwrap_or(&want);
                assert_eq!(
                    String::from_utf8_lossy(&got),
                    String::from_utf8_lossy(want),
                    "{old:?} {new:?} {k}"
                );
            }
        }
        // Not UTF-8 in the index: no edit.
        let doc = Doc::new(b"\xFF\n".to_vec(), b"x\n".to_vec()).unwrap();
        assert_eq!(revert_edit(&doc.shown, 0, false), None);
        assert_eq!(revert_edit(&doc.shown, 1, false), None);
    }

    #[test]
    fn lines_are_gits_without_their_breaks() {
        let l = Lines::new(b"\xEF\xBB\xBFa\r\nb\nc");
        assert_eq!(l.len(), 3);
        assert_eq!((l.line(0), l.line(1), l.line(2), l.line(3)), ("a", "b", "c", ""));
        assert_eq!(Lines::new(b"a\n").len(), 1);
        assert_eq!(Lines::new(b"").len(), 0);
        assert_eq!(Lines::new(b"\n\n").len(), 2);
        assert_eq!(Lines::new(b"a\xFFb\n").line(0), "a\u{FFFD}b");
    }

    #[test]
    fn side_by_side_pairs_changes_and_fills_the_shorter_side() {
        let s = |l: usize, r: usize| (Some(l), Some(r), None);
        // b → B, X added after c, e deleted.
        let got = rows("a\nb\nc\nd\ne\nf\n", "a\nB\nc\nX\nd\nf\n", false);
        assert_eq!(
            got,
            [
                s(0, 0),
                (Some(1), Some(1), Some(0)),
                s(2, 2),
                (None, Some(3), Some(1)),
                s(3, 4),
                (Some(4), None, Some(2)),
                s(5, 5),
            ]
        );
        // Two lines became three: paired, then one added against filler.
        let got = rows("a\nb\nc\nz\n", "a\nB\nC\nD\nz\n", false);
        assert_eq!(
            got,
            [
                s(0, 0),
                (Some(1), Some(1), Some(0)),
                (Some(2), Some(2), Some(0)),
                (None, Some(3), Some(0)),
                s(3, 4),
            ]
        );
        let kinds: Vec<Kind> = Doc::new("a\nb\nc\nz\n".into(), "a\nB\nC\nD\nz\n".into())
            .unwrap()
            .split
            .rows
            .iter()
            .map(Row::kind)
            .collect();
        assert_eq!(
            kinds,
            [Kind::Same, Kind::Changed, Kind::Changed, Kind::Added, Kind::Same]
        );
    }

    #[test]
    fn inline_puts_old_lines_above_new_ones() {
        let got = rows("a\nb\nc\nz\n", "a\nB\nC\nD\nz\n", true);
        assert_eq!(
            got,
            [
                (Some(0), Some(0), None),
                (Some(1), None, Some(0)),
                (Some(2), None, Some(0)),
                (None, Some(1), Some(0)),
                (None, Some(2), Some(0)),
                (None, Some(3), Some(0)),
                (Some(3), Some(4), None),
            ]
        );
    }

    #[test]
    fn whole_file_additions_and_deletions() {
        assert_eq!(
            rows("", "a\nb\n", false),
            [(None, Some(0), Some(0)), (None, Some(1), Some(0))]
        );
        assert_eq!(rows("a\n", "", true), [(Some(0), None, Some(0))]);
        assert_eq!(rows("", "", false), []);
        // No final line break on one side only: the last line changed.
        assert_eq!(
            rows("a\nb", "a\nb\n", false),
            [(Some(0), Some(0), None), (Some(1), Some(1), Some(0))]
        );
        assert!(Doc::new(b"a\0b".to_vec(), b"c".to_vec()).unwrap().binary);
    }

    #[test]
    fn every_row_once_on_random_texts() {
        // Each side's lines appear once each, in order, in both layouts; hunk starts are the
        // first row of each hunk.
        let mut seed = 7u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut text = |n: u64| -> String { (0..next() % n).map(|_| format!("{}\n", next() % 4)).collect() };
            let (old, new) = (text(12), text(12));
            let doc = Doc::new(old.clone().into(), new.clone().into()).unwrap();
            for inline in [false, true] {
                let a = doc.aligned(inline);
                let left: Vec<usize> = a.rows.iter().filter_map(|r| r.left).collect();
                let right: Vec<usize> = a.rows.iter().filter_map(|r| r.right).collect();
                assert_eq!(left, (0..doc.left.len()).collect::<Vec<_>>(), "{old:?} {new:?}");
                assert_eq!(right, (0..doc.right.len()).collect::<Vec<_>>(), "{old:?} {new:?}");
                for (k, &s) in a.starts.iter().enumerate() {
                    assert_eq!(a.rows[s].hunk, Some(k));
                    assert!(s == 0 || a.rows[s - 1].hunk != Some(k));
                }
                // Unchanged rows show equal lines.
                for r in a.rows.iter().filter(|r| r.kind() == Kind::Same) {
                    assert_eq!(doc.left.line(r.left.unwrap()), doc.right.line(r.right.unwrap()));
                }
            }
        }
    }

    #[test]
    fn steps_wrap_and_previous_goes_to_a_hunks_start() {
        let doc = Doc::new("a\nb\nc\nd\ne\nf\n".into(), "a\nB\nc\nd\nE\nf\n".into()).unwrap();
        let a = &doc.split;
        assert_eq!(a.starts, [1, 4]);
        assert_eq!(a.step(0, true), Some(0));
        assert_eq!(a.step(1, true), Some(1));
        assert_eq!(a.step(4, true), Some(0), "wraps");
        assert_eq!(a.step(4, false), Some(0));
        assert_eq!(a.step(1, false), Some(1), "wraps back");
        assert_eq!(a.step(5, false), Some(1));
        assert_eq!(Aligned::default().step(0, true), None);
    }

    #[test]
    fn changed_parts_drop_the_common_ends() {
        assert_eq!(changed_parts("let x = 1;", "let x = 22;"), (8..9, 8..10));
        assert_eq!(changed_parts("same", "same"), (4..4, 4..4));
        assert_eq!(changed_parts("é1é", "é2é"), (2..3, 2..3));
        assert_eq!(changed_parts("ab", "b"), (0..1, 0..0));
    }

    #[test]
    fn a_tab_maps_its_cursor_to_file_lines_and_back() {
        // "b" removed, "d" changed, "f" removed at the end.
        let doc = || Doc::new("a\nb\nc\nd\ne\nf\n".into(), "a\nc\nD\ne\n".into()).unwrap();
        let mut t = DiffTab::new(1, "/r/f.txt".into(), Pair::UNSTAGED);
        assert_eq!(t.cursor_line(), 0, "nothing loaded");
        // Asked for before the first load: that line's row, not the first change.
        t.go_to_line(2);
        let g = t.begin(None);
        assert!(t.finish(g, Ok(doc())));
        assert_eq!(t.rows()[t.cursor].right, Some(2));
        assert_eq!(t.cursor_line(), 2);
        for inline in [false, true] {
            t.set_layout(inline);
            let rows = t.rows().to_vec();
            let removed_b = rows
                .iter()
                .position(|r| r.left == Some(1) && r.right.is_none())
                .unwrap();
            t.cursor = removed_b;
            assert_eq!(t.cursor_line(), 1, "a removed line: where it was, the next line");
            t.cursor = rows.len() - 1;
            assert_eq!(rows[t.cursor].right, None, "the removed last line");
            assert_eq!(t.cursor_line(), 4, "below the last line");
            t.go_to_line(3);
            assert_eq!(rows[t.cursor].right, Some(3));
        }
    }

    #[test]
    fn a_tab_follows_its_line_through_reloads_and_layouts() {
        let mut t = DiffTab::new(1, "/r/f.txt".into(), Pair::UNSTAGED);
        assert_eq!(t.want(None), Want::Now);
        let g = t.begin(None);
        assert_eq!(t.want(None), Want::Nothing, "running");
        assert!(!t.finish(g + 1, Ok(Doc::default())), "another load's result");
        let doc = Doc::new("a\nb\nc\nd\n".into(), "a\nB\nC\nc\nd\n".into()).unwrap();
        assert!(t.finish(g, Ok(doc)));
        assert_eq!(t.cursor, 1, "opens at the first change");
        assert_eq!(t.title(), "f.txt (Working Tree)");
        // A buffer opened: at once; edited: later.
        assert_eq!(t.want(None), Want::Nothing);
        assert_eq!(t.want(Some((3, 1))), Want::Now);
        t.begin(Some((3, 1)));
        t.running = None;
        assert_eq!(t.want(Some((3, 2))), Want::Later);
        t.mark_stale();
        assert_eq!(t.want(Some((3, 1))), Want::Now);
        // On a change's first row: on its first row in the other layout too.
        assert_eq!((t.cursor, t.rows()[1].right), (1, Some(1)));
        t.set_layout(true);
        assert_eq!(
            (t.cursor, t.rows()[t.cursor].left),
            (1, Some(1)),
            "the change's `-` row"
        );
        t.set_layout(false);
        assert_eq!(t.cursor, 1);
        // The cursor on right line 4 ("d") stays on it in the other layout.
        t.cursor = 4;
        t.set_layout(true);
        assert_eq!((t.cursor, t.rows()[t.cursor].right), (5, Some(4)));
        t.set_layout(false);
        assert_eq!(t.cursor, 4);
        // And through a reload where the change above it went away.
        let g = t.begin(None);
        assert!(t.finish(
            g,
            Ok(Doc::new("a\nB\nC\nc\nd\n".into(), "a\nB\nC\nc\nd\n".into()).unwrap())
        ));
        assert_eq!(t.rows()[t.cursor].right, Some(4));
        // A pair without a live side ignores the buffer.
        let t = DiffTab::new(2, "/r/f.txt".into(), Pair::STAGED);
        assert_eq!(t.title(), "f.txt (Index)");
        assert_eq!(
            DiffTab::new(3, "/r/f.txt".into(), Pair::SAVED).title(),
            "f.txt (On Disk ↔ Yours)"
        );
    }

    #[test]
    fn a_history_entry_against_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let rel = Path::new("a.txt");
        history::record(&root, rel, b"\xEF\xBB\xBFold\n", &Default::default()).unwrap();
        let entry = &history::entries(&root, rel).unwrap()[0];
        let pair = Pair::history(entry.snapshot());
        assert_eq!(pair.snapshot(), Some(entry.snapshot()));
        assert_eq!(pair.staging(), None);
        assert!(!pair.needs_repo());
        assert_eq!(pair.tab_title("a.txt"), "a.txt (History: just now)");
        let job = |live: Option<&str>| Job {
            repo: None,
            root: root.clone(),
            path: root.join(rel),
            pair,
            live: live.map(|t| (Rope::from_str(t), false)),
            language: None,
            syntax_max_bytes: crate::syntax::DEFAULT_MAX_BYTES,
        };
        let doc = load(job(Some("new\n"))).unwrap();
        assert_eq!((doc.left.line(0), doc.right.line(0)), ("old", "new"));
        // Not open: the file on disk (none: empty).
        assert_eq!(load(job(None)).unwrap().right.len(), 0);
        std::fs::write(root.join(rel), "disk\n").unwrap();
        assert_eq!(load(job(None)).unwrap().right.line(0), "disk");
        let gone = Pair::history(Snapshot { time: 1, hash: 1 });
        assert!(
            load(Job {
                pair: gone,
                ..job(Some("x"))
            })
            .unwrap_err()
            .contains("gone")
        );
    }

    /// Each side's spans as (text, key) pairs, line by line.
    fn coloured(lines: &Lines) -> Vec<(usize, String, &'static str)> {
        let mut out = Vec::new();
        for i in 0..lines.len() {
            let (start, line) = (lines.start(i), lines.line(i));
            for sp in &lines.spans()[lines.first_span(i)..] {
                if sp.start.0 >= start + line.len() {
                    break;
                }
                let (a, b) = (sp.start.0.max(start) - start, (sp.end.0 - start).min(line.len()));
                out.push((i, line[a..b].to_string(), sp.highlight.key()));
            }
        }
        out
    }

    #[test]
    fn both_sides_are_coloured_from_their_whole_text() {
        let old = "fn a() {}\n/* one\ntwo */\n";
        let new = "\u{FEFF}fn a() {}\r\n/* one\r\nthree */\r\nlet x = 1;\r\n";
        let mut doc = Doc::new(old.into(), new.into()).unwrap();
        doc.highlight(Some(Language::Rust), crate::syntax::DEFAULT_MAX_BYTES);
        let left = coloured(&doc.left);
        assert!(left.contains(&(0, "fn".into(), "keyword")), "{left:?}");
        // A comment over two lines colours both, each clipped to its line.
        assert!(left.contains(&(1, "/* one".into(), "comment")), "{left:?}");
        assert!(left.contains(&(2, "two */".into(), "comment")), "{left:?}");
        // The BOM dropped and `\r\n` lines: offsets still match the text as shown.
        let right = coloured(&doc.right);
        assert!(right.contains(&(0, "fn".into(), "keyword")), "{right:?}");
        assert!(right.contains(&(2, "three */".into(), "comment")), "{right:?}");
        assert!(right.contains(&(3, "let".into(), "keyword")), "{right:?}");
    }

    #[test]
    fn a_commit_pair_reads_a_commits_blob() {
        use crate::git::test_repo::{git, init};
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        init(&root);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("gone.txt"), "gone\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "one"]);
        let first = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("bin.dat"), b"\x00\x01").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "two"]);
        let second = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let id = |s: &str| Oid::from_str(s).unwrap();
        let repo = Repo::discover(&root).unwrap();
        let job = |pair, path: &str| Job {
            repo: Some(repo.clone()),
            root: root.clone(),
            path: root.join(path),
            pair,
            live: None,
            language: None,
            syntax_max_bytes: crate::syntax::DEFAULT_MAX_BYTES,
        };
        let pair = Pair::commit(id(&second), Some(id(&first)));
        assert_eq!(pair.staging(), None);
        assert!(pair.needs_repo() && !pair.live());
        assert_eq!(pair.tab_title("a.txt"), format!("a.txt ({})", &second[..7]));
        assert_eq!(pair.left.title(), &first[..7]);
        assert_eq!(pair.right.title(), &second[..7]);
        let doc = load(job(pair, "a.txt")).unwrap();
        assert_eq!((doc.left.line(0), doc.right.line(0)), ("one", "one"));
        assert_eq!(doc.right.line(1), "two");
        // A root commit's left side is the empty tree.
        let root_pair = Pair::commit(id(&first), None);
        assert_eq!(root_pair.left, Text::Empty);
        assert_eq!(root_pair.tab_title("a.txt"), format!("a.txt ({})", &first[..7]));
        let doc = load(job(root_pair, "a.txt")).unwrap();
        assert_eq!((doc.left.len(), doc.right.line(0)), (0, "one"));
        // A path the commit doesn't have is empty on both sides.
        let doc = load(job(root_pair, "missing.txt")).unwrap();
        assert_eq!((doc.left.len(), doc.right.len()), (0, 0));
        // A path the commit deleted is in the parent's tree only.
        let doc = load(job(pair, "gone.txt")).unwrap();
        assert_eq!((doc.left.line(0), doc.right.len()), ("gone", 0));
        // A binary blob reads raw: the diff is binary.
        let doc = load(job(pair, "bin.dat")).unwrap();
        assert!(doc.binary);
    }

    #[test]
    fn plain_when_too_large_binary_or_without_a_grammar() {
        let text = "fn a() {}\n";
        let plain = |language, max| {
            let mut doc = Doc::new(text.into(), text.into()).unwrap();
            doc.highlight(language, max);
            doc.left.spans().is_empty() && doc.right.spans().is_empty()
        };
        assert!(!plain(Some(Language::Rust), text.len()));
        assert!(plain(Some(Language::Rust), text.len() - 1));
        assert!(plain(Some(Language::Rust), 0));
        assert!(plain(Some(Language::PlainText), 1 << 20));
        // `None`: the shebang decides.
        assert!(plain(None, 1 << 20));
        let mut doc = Doc::new("#!/bin/sh\necho hi\n".into(), b"#!/bin/sh\n".to_vec()).unwrap();
        doc.highlight(None, 1 << 20);
        assert!(!doc.left.spans().is_empty());
        let mut doc = Doc::new(b"fn\0".to_vec(), text.into()).unwrap();
        doc.highlight(Some(Language::Rust), 1 << 20);
        assert!(doc.binary && doc.right.spans().is_empty());
    }
}
