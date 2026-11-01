//! Discarding working tree changes (`git.clean`, `git.cleanAll`, the Changes rows' `↶` button),
//! after a `Prompt::ConfirmDiscard` naming the files, at once on disk: a modified or deleted file gets the index's text back on disk
//! (`git checkout -- path`; staged changes are never touched, `stage::discard_file`) off-thread,
//! the text before (an open buffer's, unsaved edits included) first kept in the file history so
//! "File History: Restore" can bring it back. Open buffers then reload from disk (one undo step,
//! clean). An untracked file goes to the Trash and its open tabs are marked deleted. Conflicted
//! files, staged-only ones and files with a save in flight are refused. Everything ends in
//! `AppEvent::Discarded`.

use std::path::PathBuf;

use super::{App, AppEvent, Level, Prompt};
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
    /// Open buffers' files, written on disk (the buffers reload).
    opened: Vec<PathBuf>,
    /// Snapshots sent to the file history.
    recorded: usize,
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
            } else if buffer.and_then(|b| self.buffers.get(b)).is_some_and(|b| b.is_dirty()) {
                plan.refused.push(refuse("it has unsaved changes"));
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

    /// The prompt's Discard: plans again (the status may have moved on) and starts the work. Plans (from the status now) and starts the work: tracked files get the index's text on
    /// disk off-thread, the text before (an open buffer's, unsaved edits included) recorded in
    /// the file history first; untracked ones go to the Trash. Open buffers then reload from
    /// disk in `discard_done`.
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
        // (relative path, history job, the open buffer's text).
        let mut tracked = Vec::new();
        for (paths, open) in [(&plan.closed, false), (&plan.open, true)] {
            for p in paths {
                let before = open
                    .then(|| self.buffer_of(p).and_then(|b| self.buffers.get(b)))
                    .flatten()
                    .map(|b| crate::buffer::file::encode(&b.rope, b.bom));
                let job = self.history_job(p);
                if job.is_some() {
                    self.history_started();
                }
                tracked.push((p.clone(), rel(p), job, before));
            }
        }
        let trash = plan.trash;
        self.diffs.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut done = Done {
                opened: Vec::new(),
                recorded: 0,
                trashed: Vec::new(),
                restored: 0,
                errors: Vec::new(),
            };
            let fail = |path: &std::path::Path, why: String| {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                format!("Can't discard changes in {name}: {why}")
            };
            for (abs, rel, job, buffer) in tracked {
                let open = buffer.is_some();
                // The text kept in the history: the buffer's, else the file's.
                if let (Some(job), Some(text)) = (&job, &buffer) {
                    let recorded = job.record(text);
                    super::history::send_done(&tx, recorded);
                    done.recorded += 1;
                }
                let result = stage::discard_file(&repo, &rel);
                if let Some(job) = job
                    && !open
                {
                    let recorded = match &result {
                        Ok(Some(before)) => job.record(before),
                        _ => Ok(()),
                    };
                    super::history::send_done(&tx, recorded);
                    done.recorded += 1;
                }
                match result {
                    Ok(_) => {
                        done.restored += 1;
                        if open {
                            done.opened.push(abs);
                        }
                    }
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

    /// `AppEvent::Discarded`: open buffers reload from disk (one undo step, clean), trashed
    /// files' tabs are marked deleted, the result is said and the status read again.
    pub(super) fn discard_done(&mut self, done: Done) {
        self.diffs.staging -= 1;
        let Done {
            opened,
            trashed,
            restored,
            recorded,
            errors,
        } = done;
        for path in &opened {
            if let Some(id) = self.buffer_of(path) {
                if let Some(buf) = self.buffers.get_mut(id) {
                    buf.seal_undo();
                }
                self.check_disk(id, true);
            }
        }
        for path in &trashed {
            for (_, buf) in self.buffers.iter_mut() {
                if buf.has_file() && buf.path == *path {
                    buf.deleted = true;
                }
            }
        }
        let total = restored + trashed.len();
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
            let kept = if recorded > 0 {
                " (File History: Restore brings them back)"
            } else {
                ""
            };
            self.notify(Level::Info, format!("Discarded changes in {what}{kept}"));
        }
        self.quick_diff_index_changed();
        self.refresh_git(None);
    }
}
