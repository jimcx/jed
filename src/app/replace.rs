//! The Search view's Replace (VS Code's): a results line's matches, a file's, or every result
//! not dismissed (`SearchCommand::ReplaceOne` / `ReplaceAll`, the rows' `⇄` buttons, the
//! `⇄ All` button), only while the replace input shows. An empty replace text replaces the
//! matches with nothing (VS Code's).
//!
//! - Replace All over more than one file asks first (`Prompt::ConfirmReplace`).
//! - Each file is searched again with the search's matcher (`workspace_search::FileReplace`),
//!   so what's replaced is what's there now. Dismissed files and lines are never touched; when a
//!   file's matches have moved to other lines since the search, a replace that has to pick lines
//!   (dismissed ones, or a line row's) leaves that file alone with a note.
//! - An open buffer is edited through `Buffer::edit` (`apply_changes`: one `EditKind::Other`
//!   undo step, left unsaved, every view following via `follow_edits`); a read-only one is
//!   skipped. A file not open is rewritten off the UI thread (`FileJob::run`): read again,
//!   refused when it has become binary or isn't UTF-8, written atomically as a save is, with
//!   its text before and after recorded in file history (the before first, so it can be got
//!   back: a file whose before can't be recorded is left alone). Its watcher event then reaches no buffer (none has the file), only listings, git and
//!   diff tabs, as any outside change would.
//! - One message with the totals and the files skipped; then the search runs again, keeping
//!   the list's place and (unless another search started meanwhile) the dismissals, their lines
//!   moved with the replaced text.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;

use super::history::Job;
use super::{App, AppEvent, Level, Prompt};
use crate::buffer::change::ChangeSet;
use crate::buffer::file;
use crate::search_view::RowKey;
use crate::workspace_search::{Dismissed, FileReplace, Refused};

/// What a replace covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every result not dismissed.
    All,
    /// One file's (relative path).
    File(PathBuf),
    /// One line's matches (relative path, 0-based line as the search found it).
    Line(PathBuf, usize),
}

/// The replace rewriting files off-thread, if one is.
#[derive(Default)]
pub struct ReplaceState {
    running: Option<Run>,
    /// Replaces finished (tests wait on this).
    pub done: u64,
}

impl ReplaceState {
    pub fn running(&self) -> bool {
        self.running.is_some()
    }
}

/// A replace's tally so far.
struct Run {
    /// The replace text, for the message.
    with: String,
    /// The search it replaced from: its dismissals carry over only while it's still current.
    generation: u64,
    occurrences: usize,
    files: usize,
    /// "a.txt (binary)" per file left alone.
    skipped: Vec<String>,
    /// File history failures (the replace itself went through).
    warnings: Vec<String>,
    /// The search's dismissals, lines moved with the edits, for the search that follows.
    dismissed: HashMap<PathBuf, Dismissed>,
}

impl Run {
    /// A file done: `n` matches replaced, its dismissed lines now at `skip`.
    fn file_done(&mut self, rel: PathBuf, n: usize, skip: BTreeSet<usize>) {
        self.occurrences += n;
        self.files += usize::from(n > 0);
        if !skip.is_empty() {
            self.dismissed.insert(rel, Dismissed::Lines(skip));
        }
    }

    /// A file left alone; when its matches `moved`, its dismissed line numbers name other lines
    /// now, so they're dropped (the search that follows shows the file in full).
    fn skip(&mut self, rel: &std::path::Path, why: impl std::fmt::Display, moved: bool) {
        self.skipped.push(format!("{} ({why})", rel.display()));
        if moved {
            self.dismissed.remove(rel);
        }
    }

    /// "Replaced 41 occurrences across 6 files with 'bar$1'", then any files skipped and history
    /// failures.
    fn message(&self) -> (Level, String) {
        let mut text = if self.occurrences == 0 {
            "Nothing replaced".to_string()
        } else {
            let with = if self.with.is_empty() {
                "nothing".to_string()
            } else {
                format!("'{}'", self.with)
            };
            format!(
                "Replaced {} across {} with {with}",
                plural(self.occurrences, "occurrence"),
                plural(self.files, "file")
            )
        };
        if !self.skipped.is_empty() {
            const SHOWN: usize = 3;
            let mut list = self.skipped[..self.skipped.len().min(SHOWN)].join(", ");
            if self.skipped.len() > SHOWN {
                list.push_str(&format!(" and {} more", self.skipped.len() - SHOWN));
            }
            text.push_str(&format!("; skipped {}: {list}", plural(self.skipped.len(), "file")));
        }
        for w in &self.warnings {
            text.push_str("; ");
            text.push_str(w);
        }
        let level = if self.skipped.is_empty() && self.warnings.is_empty() {
            Level::Info
        } else {
            Level::Warning
        };
        (level, text)
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A file not open, to rewrite off the UI thread.
struct FileJob {
    path: PathBuf,
    rel: PathBuf,
    replace: FileReplace,
    history: Option<Job>,
}

/// What rewriting one file came to.
pub(super) struct FileDone {
    rel: PathBuf,
    /// Matches replaced and the dismissed lines' new numbers, or why it was left alone (and
    /// whether that's `Refused::Moved`).
    outcome: Result<(usize, BTreeSet<usize>), (String, bool)>,
    warnings: Vec<String>,
}

/// The files of a replace rewritten (`AppEvent::Replaced`).
pub(super) struct FilesDone {
    files: Vec<FileDone>,
}

impl FileJob {
    /// Blocking: call from spawn_blocking.
    fn run(self) -> FileDone {
        let mut warnings = Vec::new();
        let outcome = self.rewrite(&mut warnings);
        match &outcome {
            Ok((n, _)) => tracing::debug!(target: "jed::app", rel = %self.rel.display(), n, "replaced in file"),
            Err((e, _)) => tracing::info!(target: "jed::app", rel = %self.rel.display(), "replace skipped a file: {e}"),
        }
        FileDone {
            rel: self.rel,
            outcome,
            warnings,
        }
    }

    fn rewrite(&self, warnings: &mut Vec<String>) -> Result<(usize, BTreeSet<usize>), (String, bool)> {
        let bytes = std::fs::read(&self.path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => ("deleted".to_string(), false),
            _ => (e.to_string(), false),
        })?;
        let (out, replaced) = self
            .replace
            .rewrite(&bytes)
            .map_err(|r| (r.to_string(), r == Refused::Moved))?;
        let n = replaced.edits.len();
        if n == 0 {
            return Ok((0, replaced.skip));
        }
        // The text before must be in history before it's overwritten: no snapshot, no write.
        if let Some(h) = &self.history
            && let Err(e) = h.record(&bytes)
        {
            tracing::warn!(target: "jed::app", "{e}");
            return Err(("its file history couldn't be saved".to_string(), false));
        }
        file::atomic_write(&self.path, &out).map_err(|e| (format!("can't write: {e}"), false))?;
        if let Some(h) = &self.history
            && let Err(e) = h.record(&out)
        {
            warnings.push(e);
        }
        Ok((n, replaced.skip))
    }
}

impl App {
    /// The replace a results row's key stands for: a line row's matches, a file row's file.
    pub(super) fn replace_scope(&self, key: &RowKey) -> Option<Scope> {
        let results = self.search.as_ref()?;
        let f = &results.files[results.file_index(&key.0)?];
        Some(if key.1 == 0 || f.collapsed {
            Scope::File(key.0.clone())
        } else {
            Scope::Line(key.0.clone(), f.lines.get(key.1 - 1)?.line)
        })
    }

    /// Replace (a line, a file, or all): only while the replace input shows and the search has
    /// finished, one replace at a time; Replace All over more than one file asks first.
    pub(super) fn search_replace(&mut self, scope: Scope) {
        let refuse = if !self.search_view.replacing {
            Some("Show the replace input first (Search: Toggle Replace)")
        } else if self.replace.running() {
            Some("Still replacing…")
        } else if self.search.as_ref().is_none_or(|r| !r.done) || self.search_view.deadline().is_some() {
            Some("The search is still running")
        } else if self.search.as_ref().is_some_and(|r| r.files.is_empty()) {
            Some("Nothing to replace")
        } else {
            None
        };
        if let Some(why) = refuse {
            self.notify(Level::Info, why.into());
            return;
        }
        let results = self.search.as_ref().expect("checked");
        if scope == Scope::All && results.files.len() > 1 {
            self.prompt = Some(Prompt::ConfirmReplace {
                occurrences: results.total,
                files: results.files.len(),
                with: self.search_view.replace.text.clone(),
            });
            return;
        }
        self.replace_now(scope);
    }

    /// Replaces now: open buffers edited at once, other files rewritten off-thread; the
    /// message and the search again once all are done.
    pub(super) fn replace_now(&mut self, scope: Scope) {
        let Some(results) = &self.search else { return };
        let with = self.search_view.replace.text.clone();
        let one = |path: &PathBuf| results.file_replace(path, &with).map(|r| (path.clone(), r));
        let targets: Vec<(PathBuf, FileReplace)> = match &scope {
            Scope::All => results.files.iter().filter_map(|f| one(&f.path)).collect(),
            Scope::File(path) => one(path).into_iter().collect(),
            Scope::Line(path, line) => one(path)
                .map(|(p, mut r)| {
                    r.only = Some(*line);
                    (p, r)
                })
                .into_iter()
                .collect(),
        };
        let mut run = Run {
            generation: results.generation,
            dismissed: results.dismissed.clone(),
            with,
            occurrences: 0,
            files: 0,
            skipped: Vec::new(),
            warnings: Vec::new(),
        };
        let mut jobs = Vec::new();
        for (rel, replace) in targets {
            let path = self.workspace.root.join(&rel);
            let Some(id) = self.buffers.find_path(&path) else {
                let history = self.history_job(&path);
                jobs.push(FileJob {
                    path,
                    rel,
                    replace,
                    history,
                });
                continue;
            };
            let buf = self.buffers.get(id).expect("found");
            if let Some(reason) = &buf.read_only {
                run.skip(&rel, reason, false);
                continue;
            }
            match replace.replace(&buf.rope) {
                Err(why) => run.skip(&rel, why, why == Refused::Moved),
                Ok(r) => {
                    if !r.edits.is_empty() {
                        let len = r.edits.iter().map(|(_, t)| t.len()).sum();
                        let mut set = ChangeSet::with_capacity(r.edits.len(), len);
                        for (m, text) in &r.edits {
                            set.push(m.start, m.end, text);
                        }
                        self.apply_changes(id, set);
                    }
                    run.file_done(rel, r.edits.len(), r.skip);
                }
            }
        }
        self.follow_edits();
        if jobs.is_empty() {
            self.replace_finished(run);
            return;
        }
        self.replace.running = Some(run);
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let files = jobs.into_iter().map(FileJob::run).collect();
            let _ = tx.send(AppEvent::Replaced(Box::new(FilesDone { files })));
        });
    }

    /// The files not open were rewritten (`AppEvent::Replaced`).
    pub(super) fn files_replaced(&mut self, done: FilesDone) {
        let Some(mut run) = self.replace.running.take() else {
            return;
        };
        for f in done.files {
            run.warnings.extend(f.warnings);
            match f.outcome {
                Ok((n, skip)) => run.file_done(f.rel, n, skip),
                Err((why, moved)) => run.skip(&f.rel, why, moved),
            }
        }
        self.replace_finished(run);
    }

    /// The totals message, then the search again (keeping the list's place, and the
    /// dismissals while no other search started meanwhile).
    fn replace_finished(&mut self, run: Run) {
        self.replace.done += 1;
        let (level, text) = run.message();
        self.notify(level, text);
        let current = self.search.as_ref().map(|r| r.generation) == Some(run.generation);
        self.search_view.restarted();
        self.search_view.error = self.start_search(self.search_view.query()).err();
        if current && let Some(results) = &mut self.search {
            results.dismissed = run.dismissed;
        }
        self.refresh_replace_previews();
        self.needs_redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run {
            with: "bar$1".into(),
            generation: 1,
            occurrences: 0,
            files: 0,
            skipped: Vec::new(),
            warnings: Vec::new(),
            dismissed: HashMap::new(),
        }
    }

    /// The totals, singular and plural, an empty replace text, the files skipped (three named,
    /// then a count) and history failures; anything left alone makes it a warning.
    #[test]
    fn replace_messages() {
        let mut r = run();
        assert_eq!(r.message(), (Level::Info, "Nothing replaced".into()));
        r.file_done("a".into(), 1, BTreeSet::new());
        assert_eq!(
            r.message(),
            (Level::Info, "Replaced 1 occurrence across 1 file with 'bar$1'".into())
        );
        r.file_done("b".into(), 3, BTreeSet::from([4]));
        r.file_done("c".into(), 0, BTreeSet::new());
        assert_eq!(r.message().1, "Replaced 4 occurrences across 2 files with 'bar$1'");
        assert_eq!(
            r.dismissed.get(std::path::Path::new("b")),
            Some(&Dismissed::Lines([4].into()))
        );
        r.with.clear();
        for (f, why) in [("x", "binary"), ("y", "not UTF-8"), ("z", "deleted"), ("w", "deleted")] {
            r.skip(std::path::Path::new(f), why, false);
        }
        r.warnings.push("File history of b not saved: disk full".into());
        assert_eq!(
            r.message(),
            (
                Level::Warning,
                "Replaced 4 occurrences across 2 files with nothing; skipped 4 files: x (binary), \
                 y (not UTF-8), z (deleted) and 1 more; File history of b not saved: disk full"
                    .into()
            )
        );
    }
}
