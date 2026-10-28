//! Discarding working tree changes (`git.clean`, `git.cleanAll`, the Changes rows' `↶` button):
//! after a `Prompt::ConfirmDiscard` naming the files, a modified or deleted file gets the
//! index's text back (`git checkout -- path`; staged changes are never touched). An open buffer
//! is edited through `Buffer::edit` (the whole text, one undo step, left unsaved) with the text
//! read off-thread; a closed file is rewritten off-thread (`stage::discard_file`), its text
//! before kept in the file history. An untracked file goes to the Trash and its open tabs are
//! marked deleted. Conflicted files, staged-only ones and files with a save in flight are
//! refused. Everything ends in `AppEvent::Discarded`.

use std::path::PathBuf;

use super::{App, AppEvent, Level, Prompt};
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::ByteIdx;
use crate::git::Change;
use crate::git::stage;
use crate::scm::{self, Row, Section};

/// What a discard will do, from the status now.
#[derive(Default)]
struct Plan {
    /// Tracked files with an open buffer, and without (absolute paths).
    open: Vec<PathBuf>,
    closed: Vec<PathBuf>,
    /// Untracked files, to the Trash.
    trash: Vec<PathBuf>,
    /// Why files were left out, one message each.
    refused: Vec<String>,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.open.is_empty() && self.closed.is_empty() && self.trash.is_empty()
    }

    fn files(&self) -> Vec<PathBuf> {
        [&self.open, &self.closed, &self.trash]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// `AppEvent::Discarded`'s payload.
pub struct Done {
    /// Open buffers' paths and the index's text for them.
    texts: Vec<(PathBuf, Vec<u8>)>,
    trashed: Vec<PathBuf>,
    /// Closed files rewritten.
    restored: usize,
    errors: Vec<String>,
}

impl App {
    /// `git.clean` with the Source Control view focused: the selected row (a Changes header:
    /// all of them).
    pub(super) fn scm_discard(&mut self) {
        let Some(status) = self.git.status.as_ref() else {
            return self.notify(Level::Info, "Not in a git repository".into());
        };
        let rows = self.scm.rows(status);
        match self.scm.selected(&rows) {
            Some(Row::Header {
                section: Section::Changes,
                ..
            }) => self.discard_all(),
            Some(Row::Header { .. }) => self.notify(Level::Info, "Only working tree changes can be discarded".into()),
            Some(Row::File { entry, .. }) => {
                let path = status.repo.workdir.join(&entry.path);
                self.discard_request(vec![path]);
            }
            None => {}
        }
    }

    /// `git.clean` elsewhere: the active file.
    pub(super) fn discard_command(&mut self) {
        self.ensure_status_loaded();
        if let Some(path) = self.active_path() {
            self.discard_request(vec![path]);
        }
    }

    /// `git.cleanAll`: every file in the Changes section.
    pub(super) fn discard_all(&mut self) {
        let Some(status) = self.git.status.as_ref() else {
            return self.notify(Level::Info, "Not in a git repository".into());
        };
        let files: Vec<PathBuf> = scm::files(status, Section::Changes)
            .map(|(e, _)| status.repo.workdir.join(&e.path))
            .collect();
        if files.is_empty() {
            return self.notify(Level::Info, "No changes to discard".into());
        }
        self.discard_request(files);
    }

    fn discard_plan(&self, files: &[PathBuf]) -> Plan {
        let mut plan = Plan::default();
        let Some(status) = self.git.status.as_ref() else {
            return plan;
        };
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let refuse = |why: &str| format!("Can't discard changes in {name}: {why}");
            let entry = status.repo.relative(path).and_then(|rel| status.entry(&rel));
            let Some(entry) = entry else {
                plan.refused.push(format!("{name} has no changes to discard"));
                continue;
            };
            if entry.unstaged == Some(Change::Conflicted) || entry.staged == Some(Change::Conflicted) {
                plan.refused.push(refuse("it has merge conflicts"));
                continue;
            }
            let Some(change) = entry.unstaged else {
                plan.refused.push(refuse("its changes are staged"));
                continue;
            };
            let buffer = self.buffer_of(path);
            if buffer
                .and_then(|b| self.buffers.get(b))
                .is_some_and(|b| b.save_in_flight())
            {
                plan.refused.push(refuse("it is being saved"));
            } else if change == Change::Untracked {
                plan.trash.push(path.clone());
            } else if buffer.is_some() {
                plan.open.push(path.clone());
            } else {
                plan.closed.push(path.clone());
            }
        }
        plan
    }

    /// Asks before discarding `files` (absolute paths).
    pub(super) fn discard_request(&mut self, files: Vec<PathBuf>) {
        let plan = self.discard_plan(&files);
        if plan.is_empty() {
            if let Some(why) = plan.refused.first() {
                self.notify(Level::Info, why.clone());
            }
            return;
        }
        self.prompt = Some(Prompt::ConfirmDiscard {
            open: !plan.open.is_empty(),
            closed: !plan.closed.is_empty(),
            trash: plan.trash.len(),
            files: plan.files(),
        });
    }

    /// The prompt's Discard: plans again (the status may have moved on) and starts the work.
    pub(super) fn discard_now(&mut self, files: Vec<PathBuf>) {
        let plan = self.discard_plan(&files);
        if plan.is_empty() {
            if let Some(why) = plan.refused.first() {
                self.notify(Level::Info, why.clone());
            }
            return;
        }
        let Some(repo) = self.git.status.as_ref().map(|s| s.repo.clone()) else {
            return;
        };
        let rel = |p: &PathBuf| repo.relative(p).unwrap_or_else(|| p.clone());
        let closed: Vec<_> = plan.closed.iter().map(|p| (rel(p), self.history_job(p))).collect();
        for _ in closed.iter().filter(|(_, job)| job.is_some()) {
            self.history_started();
        }
        let open: Vec<(PathBuf, PathBuf)> = plan.open.iter().map(|p| (p.clone(), rel(p))).collect();
        let trash = plan.trash;
        self.diffs.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut done = Done {
                texts: Vec::new(),
                trashed: Vec::new(),
                restored: 0,
                errors: Vec::new(),
            };
            let fail = |path: &std::path::Path, why: String| {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                format!("Can't discard changes in {name}: {why}")
            };
            for (rel, job) in closed {
                let result = stage::discard_file(&repo, &rel);
                if let Some(job) = job {
                    let recorded = match &result {
                        Ok(Some(before)) => job.record(before),
                        _ => Ok(()),
                    };
                    super::history::send_done(&tx, recorded);
                }
                match result {
                    Ok(_) => done.restored += 1,
                    Err(e) => done.errors.push(fail(&rel, e.to_string())),
                }
            }
            for (abs, rel) in open {
                match stage::index_text(&repo, &rel) {
                    Ok(Some(t)) if t.mode != 0o120000 => done.texts.push((abs, t.bytes)),
                    Ok(_) => done
                        .errors
                        .push(fail(&rel, "it isn't a regular file in the index".into())),
                    Err(e) => done.errors.push(fail(&rel, e.to_string())),
                }
            }
            for path in trash {
                match crate::explorer::ops::delete(&path) {
                    Ok(()) => done.trashed.push(path),
                    Err(e) => done.errors.push(fail(&path, e.to_string())),
                }
            }
            let _ = tx.send(AppEvent::Discarded(Box::new(done)));
        });
    }

    /// `AppEvent::Discarded`: open buffers get the index's text (one undo step each), trashed
    /// files' tabs are marked deleted, the result is said and the status read again.
    pub(super) fn discard_done(&mut self, done: Done) {
        self.diffs.staging -= 1;
        let Done {
            texts,
            trashed,
            restored,
            mut errors,
        } = done;
        let mut edited = 0;
        for (path, bytes) in texts {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let Some(id) = self.buffer_of(&path) else { continue };
            let buf = self.buffers.get(id).expect("an open buffer");
            if let Some(reason) = &buf.read_only {
                errors.push(format!("Can't discard changes in {name}: read-only: {reason}"));
                continue;
            }
            let bytes = if buf.bom {
                bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)
            } else {
                &bytes
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                errors.push(format!("Can't discard changes in {name}: the index's text isn't UTF-8"));
                continue;
            };
            if buf.rope != text {
                let mut changes = ChangeSet::with_capacity(1, text.len());
                changes.push(ByteIdx(0), ByteIdx(buf.rope.len()), text);
                self.apply_changes(id, changes);
                self.follow_edits();
            }
            edited += 1;
        }
        for path in &trashed {
            for (_, buf) in self.buffers.iter_mut() {
                if buf.has_file() && buf.path == *path {
                    buf.deleted = true;
                }
            }
        }
        let total = restored + edited + trashed.len();
        if let Some(e) = errors.first() {
            let more = if errors.len() > 1 {
                format!(" (and {} more)", errors.len() - 1)
            } else {
                String::new()
            };
            self.notify(Level::Error, format!("{e}{more}"));
        } else {
            let what = if total == 1 {
                "1 file".to_string()
            } else {
                format!("{total} files")
            };
            let unsaved = if edited > 0 { " (open files not saved)" } else { "" };
            self.notify(Level::Info, format!("Discarded changes in {what}{unsaved}"));
        }
        self.quick_diff_index_changed();
        self.refresh_git(None);
    }
}
