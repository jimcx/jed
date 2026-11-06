//! Suggestions in review comments (model `crate::suggestion`): Apply Suggestion and Insert Suggestion.
//!
//! **Apply** takes the first suggestion in the thread under the caret (or the selected CONVERSATIONS
//! row). Only right-side threads that are still placed (and not resolved) qualify. The suggestion's
//! range is moved to where the thread is placed now, and the text the comment's commit has at the
//! suggestion's lines is read off-thread (`Staged`, `ReviewEvent::Suggestion`). When it arrives the
//! file's buffer (opened in the background if it isn't) must still read the same at those lines,
//! else nothing happens and "Suggestion is out of date" is said; this also refuses a second,
//! overlapping suggestion and one the buffer's edits (or the placement's file on disk) moved. The
//! buffer's revision is checked again at that moment. A good apply is one `EditKind::Other` step
//! through `edit::paste` on a scratch view; nothing is committed, saved or resolved.
//!
//! **Insert** puts a ```` ```suggestion ```` fence round the diff's lines the open comment box is on
//! into the box (at its caret), the caret inside the fence. GitLab's `suggestion:-N+M` is never
//! written: its range is the box's own lines.

use std::ops::Range;
use std::path::PathBuf;

use super::review::ReviewEvent;
use super::{App, AppEvent, Level};
use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx};
use crate::edit;
use crate::forge::Side;
use crate::git::Repo;
use crate::review_anchor::{self, Placement};
use crate::selection::{Selection, Selections};
use crate::suggestion;
use crate::view::View;

/// An apply on its way: the live lines and text, and what the comment's commit has there.
pub(super) struct Staged {
    path: PathBuf,
    /// 0-based, half-open lines of the buffer the suggestion replaces.
    lines: Range<usize>,
    replacement: String,
    /// The buffer's revision when the apply started (`None`: the file was not open).
    revision: Option<u64>,
    /// The commit's text at the suggestion's lines (no line endings); `None`: not readable.
    base: Option<Vec<String>>,
}

/// Blocking (git): lines `start..=end` (1-based) of `path` as `commit` has them, `None` when the
/// commit, the file or the lines aren't there.
fn read_base(repo: &Repo, commit: &str, path: &str, start: u32, end: u32) -> Option<Vec<String>> {
    let git = repo.open().ok()?;
    let commit = git.find_commit(git2::Oid::from_str(commit).ok()?).ok()?;
    let blob = review_anchor::blob_at(&git, &commit, path)?;
    let text = String::from_utf8_lossy(&blob);
    let all: Vec<&str> = text.split('\n').collect();
    // A final line break doesn't start another line.
    let n = if text.ends_with('\n') { all.len() - 1 } else { all.len() };
    let (first, last) = (start.max(1) as usize - 1, end as usize);
    (first < last && last <= n).then(|| {
        all[first..last]
            .iter()
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect()
    })
}

/// The lines of `rope` (a final empty line after a line break isn't one).
fn real_lines(rope: &ropey::Rope) -> usize {
    let n = pos::line_count(rope);
    if n > 0 && pos::line_start(rope, n - 1).0 == rope.len() {
        n - 1
    } else {
        n
    }
}

impl App {
    /// "Code Review: Apply Suggestion".
    pub(super) fn review_apply_suggestion(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.apply_suggestion_of(row);
        }
    }

    /// Applies the first suggestion of `row`'s thread.
    pub(super) fn apply_suggestion_of(&mut self, row: crate::review::ThreadRow) {
        let Placement::Placed { side, lines } = &row.place else {
            return self.notify(
                Level::Info,
                "This thread is outdated: its suggestion can't be applied".into(),
            );
        };
        if *side != Side::Right {
            return self.notify(Level::Info, "Suggestions apply to the new side".into());
        }
        if row.thread.resolved {
            return self.notify(Level::Info, "The thread is resolved".into());
        }
        let a = &row.thread.anchor;
        let Some(s) = row
            .thread
            .comments
            .iter()
            .find_map(|c| suggestion::parse(&c.body, a).into_iter().next())
        else {
            return self.notify(Level::Info, "No suggestion in this thread".into());
        };
        // Where the thread's lines are now, against where the comment had them.
        let shift = lines.start as i64 - (a.start_line as i64 - 1);
        let first = s.start_line as i64 - 1 + shift;
        let end = s.end_line as i64 + shift;
        let (Some(repo), Some(workdir)) = (
            self.git.status.as_ref().map(|s| s.repo.clone()),
            self.git.status.as_ref().map(|s| s.repo.workdir.clone()),
        ) else {
            return;
        };
        if first < 0 || end <= first {
            return self.notify(Level::Error, "Suggestion is out of date".into());
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let path = workdir.join(&a.path);
        let revision = self
            .buffer_of(&path)
            .and_then(|id| self.buffers.get(id))
            .map(Buffer::revision);
        let mut staged = Staged {
            path,
            lines: first as usize..end as usize,
            replacement: s.replacement,
            revision,
            base: None,
        };
        let (commit, rel) = (a.commit.clone(), a.path.clone());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            staged.base = read_base(&repo, &commit, &rel, s.start_line, s.end_line);
            let _ = tx.send(AppEvent::Review(ReviewEvent::Suggestion(staged)));
        });
    }

    /// The commit's lines were read: the file's buffer is found (opened first when needed).
    pub(super) fn suggestion_read(&mut self, staged: Staged) {
        let Some(base) = &staged.base else {
            return self.notify(
                Level::Error,
                "Suggestion is out of date: the commit's text is not readable".into(),
            );
        };
        if base.len() != staged.lines.len() {
            return self.notify(Level::Error, "Suggestion is out of date".into());
        }
        if self.buffer_of(&staged.path).is_some() {
            return self.suggestion_apply(staged);
        }
        if self.suggest.is_some() {
            return self.notify(Level::Info, "Another suggestion is waiting for its file".into());
        }
        if !self.opening.contains_key(&staged.path) {
            let request = super::files::OpenRequest {
                background: true,
                ..Default::default()
            };
            self.start_open(staged.path.clone(), request);
        }
        self.suggest = Some(staged);
    }

    /// A file finished opening: a suggestion that waited for it applies.
    pub(super) fn suggestion_opened(&mut self, path: &std::path::Path) {
        if self.suggest.as_ref().is_none_or(|s| s.path != path) {
            return;
        }
        if let Some(s) = self.suggest.take() {
            if self.buffer_of(path).is_some() {
                self.suggestion_apply(s);
            } else {
                self.notify(
                    Level::Error,
                    "The file did not open: the suggestion was not applied".into(),
                );
            }
        }
    }

    fn suggestion_apply(&mut self, s: Staged) {
        let Some(id) = self.buffer_of(&s.path) else { return };
        let settings_free = self.buffers.get(id).map(|b| (b.revision(), b.read_only.clone()));
        let Some((revision, read_only)) = settings_free else {
            return;
        };
        if let Some(reason) = read_only {
            return self.notify(Level::Error, format!("Read-only: {reason}"));
        }
        let stale = s.revision.is_some_and(|r| r != revision);
        let Some(buf) = self.buffers.get_mut(id) else { return };
        let rope = &buf.rope;
        let base = s.base.unwrap_or_default();
        let same = !stale
            && s.lines.end <= real_lines(rope)
            && s.lines.clone().zip(&base).all(|(line, want)| {
                let (start, end, _) = pos::line_bounds(rope, line);
                let have = rope.slice(start.0..end.0).to_string();
                have.strip_suffix('\r').unwrap_or(&have) == want
            });
        if !same {
            return self.notify(Level::Error, "Suggestion is out of date".into());
        }
        let n = pos::line_count(rope);
        let from = pos::line_start(rope, s.lines.start);
        let to = if s.lines.end < n {
            pos::line_start(rope, s.lines.end)
        } else {
            ByteIdx(rope.len())
        };
        // The last line may have no line break: neither does its replacement.
        // (The range ends at the last line only when that line has no break: `same` required
        // `end <= real_lines`, which is one short of `n` after a final line break.)
        let ends_open = s.lines.end == n;
        let text = if ends_open {
            s.replacement.strip_suffix('\n').unwrap_or(&s.replacement)
        } else {
            &s.replacement
        };
        let mut view = View::new();
        view.selections = Selections::single(Selection::new(from, to));
        edit::paste(buf, &mut view, text, false, &[]);
        self.needs_redraw = true;
        self.notify(
            Level::Info,
            "Suggestion applied (not saved or committed; undo reverts it)".into(),
        );
    }

    /// "Code Review: Insert Suggestion": the open comment box gets the fence.
    pub(super) fn review_insert_suggestion(&mut self) {
        let Some(c) = &self.composer else {
            return self.notify(Level::Info, "Open a comment box first".into());
        };
        if !c.right {
            return self.notify(Level::Info, "Suggestions apply to the new side".into());
        }
        let (tab, first, last) = (c.tab, c.start.max(1) as usize - 1, c.end.max(c.start) as usize);
        let Some(doc) = self
            .groups
            .diff_tabs_mut()
            .find(|(_, d)| d.id == tab)
            .and_then(|(_, d)| d.doc.as_ref())
        else {
            return;
        };
        if last > doc.right.len() {
            return self.notify(Level::Info, "The diff is updating, try again".into());
        }
        let lines: Vec<String> = (first..last).map(|l| doc.right.line(l).to_string()).collect();
        let (text, caret) = suggestion::template(&lines);
        let Some(c) = &mut self.composer else { return };
        let at = c.view.selections.primary().start();
        edit::paste(&mut c.buf, &mut c.view, &text, false, &[]);
        let inside = ByteIdx(at.0 + caret);
        c.view.selections = Selections::single(Selection::new(inside, inside));
        self.composer_changed();
    }
}
