//! A commit tab's model (no drawing, no app state: `app/commit_tabs.rs` opens, loads and fits it).
//!
//! A commit tab is an ordinary read-only text tab, as a log tab is: a `Buffer` with
//! `commit: Some(CommitDoc)` (`Buffer::commit`) — no path, `read_only`, its text only ever changed
//! by `Buffer::append` — so the editor's cursor, selection, find, copy and splits work on it as
//! on any text. Its text is `git show --stat --format=medium` at the width of the view showing it
//! ([`git_show::render`], one buffer line per [`Line`], no final line break); a width change
//! re-renders it without reading git again (a commit never changes). This keeps what the text
//! came from: the commit, the file whose history opened it, the [`Show`] read, the lines last
//! rendered (each stat line's file, each segment's colour: [`CommitDoc::spans`]).

use std::ops::Range;
use std::sync::Arc;

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};
use crate::git::show::Show;
use crate::git_show::{self, Kind, Line};
use crate::quick_diff::Want;

/// What a commit buffer shows (`Buffer::commit`).
#[derive(Debug, Clone)]
pub struct CommitDoc {
    /// The full object id.
    pub commit: String,
    /// The work-tree-relative path of the file whose history opened the tab (`None`: unknown):
    /// the first load puts the cursor on its stat line, where Enter opens its diff.
    pub file: Option<String>,
    /// `None` until the load ends.
    pub show: Option<Arc<Show>>,
    /// The lines the text holds (`git_show::render` at the width `shown` names).
    lines: Vec<Line>,
    shown: Shown,
    /// The last load failed (the text says why): no retry until the tab is reopened (a commit
    /// never changes, so only a missing repository or commit can fail it).
    pub error: Option<String>,
    /// A load is running (`begin` / `finish`).
    pub running: bool,
}

impl CommitDoc {
    pub fn new(commit: String, file: Option<String>) -> CommitDoc {
        CommitDoc {
            commit,
            file,
            show: None,
            lines: Vec::new(),
            shown: Shown::Nothing,
            error: None,
            running: false,
        }
    }

    /// Whether to load: a commit never changes, so only a doc without a `show` (and not loading
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

    /// The load ended (`false`: not the one running). The text follows with `render`.
    pub fn finish(&mut self, result: Result<Arc<Show>, String>) -> bool {
        if !self.running {
            return false;
        }
        self.running = false;
        match result {
            Ok(show) => {
                self.show = Some(show);
                self.error = None;
            }
            Err(e) => {
                tracing::warn!(commit = %self.commit, "git show failed: {e}");
                self.error = Some(e);
            }
        }
        true
    }

    /// The text for `width` columns when it isn't the one shown (`None`: unchanged): the load's
    /// error, or the commit rendered at that width (no I/O).
    pub fn render(&mut self, width: usize) -> Option<String> {
        let Some(show) = &self.show else {
            let error = self.error.as_ref().filter(|_| self.shown != Shown::Error)?;
            self.shown = Shown::Error;
            return Some(error.clone());
        };
        if self.shown == Shown::Width(width) {
            return None;
        }
        self.lines = git_show::render(show, width);
        self.shown = Shown::Width(width);
        let mut text = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&line.text());
        }
        Some(text)
    }

    /// The tab bar's label: the commit's 7-hex short id, as a commit diff's title uses it.
    pub fn title(&self) -> String {
        self.commit[..7.min(self.commit.len())].to_string()
    }

    /// The file whose stat line is buffer line `line`, if it is one.
    pub fn file_at(&self, line: usize) -> Option<usize> {
        self.lines.get(line).and_then(|l| l.file)
    }

    /// The buffer line of `Show::files[k]`'s stat line.
    pub fn file_row(&self, k: usize) -> Option<usize> {
        self.lines.iter().position(|l| l.file == Some(k))
    }

    /// The buffer line of the tab's own file's stat line (`file`, among the commit's files).
    pub fn own_file_row(&self) -> Option<usize> {
        let file = self.file.as_deref()?;
        let show = self.show.as_ref()?;
        let k = show.files.iter().position(|f| f.path == file)?;
        self.file_row(k)
    }

    /// git's colours on `lines` of `rope` (the buffer's text, as `render` made it): byte ranges
    /// with their theme key, in order — the commit id `gitShow.commit`, a stat's `+` graph
    /// `gitShow.add`, its `-` graph (and a binary's old size) `gitShow.del`; the rest is the
    /// editor's colour. Only the lines asked for (what the editor draws).
    pub fn spans(&self, rope: &Rope, lines: Range<usize>) -> Vec<(ByteIdx, ByteIdx, &'static str)> {
        let mut out = Vec::new();
        for i in lines {
            let Some(line) = self.lines.get(i) else { break };
            let mut at = pos::line_start(rope, i).0;
            for segment in &line.segments {
                let end = at + segment.text.len();
                if let Some(key) = theme_key(segment.kind) {
                    out.push((ByteIdx(at), ByteIdx(end), key));
                }
                at = end;
            }
        }
        out
    }
}

/// What the buffer's text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Nothing yet (loading).
    Nothing,
    /// The load's error.
    Error,
    /// The commit at this width.
    Width(usize),
}

/// The theme key a segment's kind draws in (`None`: git's default colour, the editor's).
fn theme_key(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Commit => Some("gitShow.commit"),
        Kind::StatAdd => Some("gitShow.add"),
        Kind::StatDel => Some("gitShow.del"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::show::{Content, File};

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

    fn loaded(file: Option<&str>) -> CommitDoc {
        let mut d = CommitDoc::new(show().id, file.map(String::from));
        d.begin();
        assert!(d.finish(Ok(Arc::new(show()))));
        d
    }

    #[test]
    fn want_and_finish() {
        let mut d = CommitDoc::new("a".repeat(40), None);
        assert_eq!(d.want(), Want::Now);
        d.begin();
        assert_eq!(d.want(), Want::Nothing, "running");
        assert!(d.finish(Ok(Arc::new(show()))), "finishes");
        assert!(d.show.is_some());
        assert_eq!(d.want(), Want::Nothing, "loaded");
        assert_eq!(d.title(), "aaaaaaa");
        // A failed load isn't retried; its text is the error, once.
        let mut d = CommitDoc::new("b".repeat(40), None);
        assert_eq!(d.render(80), None, "nothing loaded yet");
        d.begin();
        assert!(d.finish(Err("gone".into())));
        assert_eq!(d.error.as_deref(), Some("gone"));
        assert_eq!(d.want(), Want::Nothing);
        assert!(!d.finish(Ok(Arc::new(show()))), "nothing running");
        assert_eq!(d.render(80).as_deref(), Some("gone"));
        assert_eq!(d.render(60), None, "the error doesn't depend on the width");
    }

    /// The text is `git_show::render`'s lines at the width asked for, rendered again only when
    /// the width changes.
    #[test]
    fn renders_for_the_width() {
        let mut d = loaded(None);
        let text = d.render(80).expect("the first render");
        let expected: Vec<String> = git_show::render(&show(), 80).iter().map(Line::text).collect();
        assert_eq!(text, expected.join("\n"));
        assert_eq!(d.render(80), None, "same width: unchanged");
        let narrow = d.render(30).expect("a new width");
        let expected: Vec<String> = git_show::render(&show(), 30).iter().map(Line::text).collect();
        assert_eq!(narrow, expected.join("\n"));
    }

    /// Stat lines name their file; the own file's row is found by its path.
    #[test]
    fn stat_rows_name_their_file() {
        let mut d = loaded(Some("b.txt"));
        d.render(80);
        let (a, b) = (d.file_row(0).unwrap(), d.file_row(1).unwrap());
        assert_eq!(b, a + 1);
        assert_eq!((d.file_at(a), d.file_at(b)), (Some(0), Some(1)));
        assert_eq!(d.file_at(0), None, "the commit line");
        assert_eq!(d.own_file_row(), Some(b));
        assert_eq!(loaded(Some("gone.txt")).own_file_row(), None);
    }

    /// The colours are byte ranges of the rope the text went into: the id, the `+` and `-`.
    #[test]
    fn spans_colour_the_id_and_the_graph() {
        let mut d = loaded(None);
        let rope = Rope::from_str(&d.render(80).unwrap());
        let spans = d.spans(&rope, 0..pos::line_count(&rope));
        let text = |(s, e, _): &(ByteIdx, ByteIdx, &str)| rope.slice(s.0..e.0).to_string();
        assert_eq!(text(&spans[0]), show().id);
        assert_eq!(spans[0].2, "gitShow.commit");
        let graph: Vec<(String, &str)> = spans[1..].iter().map(|s| (text(s), s.2)).collect();
        assert_eq!(graph, [("++".into(), "gitShow.add"), ("-".into(), "gitShow.del")]);
        // Only the lines asked for.
        let a = d.file_row(0).unwrap();
        assert_eq!(d.spans(&rope, a..a + 1).len(), 1);
        assert!(d.spans(&rope, 1..2).is_empty(), "the author line");
    }
}
