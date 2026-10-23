//! Crash backups (`crate::backup`): dirty buffers written under the state dir every
//! `backup::INTERVAL`, removed once their buffer is clean or closed and all of them on a clean
//! quit; the ones a killed or crashed jed left are offered at the next start.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::{App, AppEvent, Level, Prompt};
use crate::backup::{self, Backup, Key, Snapshot};
use crate::buffer::change::ChangeSet;
use crate::buffer::{Buffer, BufferId};

/// The backups' schedule, writes in flight and the start's offer (`app.backups`).
#[derive(Debug, Default)]
pub struct BackupState {
    /// When changed dirty buffers are next written: set once a dirty buffer differs from its
    /// backup, not pushed back by more edits.
    deadline: Option<Instant>,
    /// What each backed-up buffer's backup file holds: its path and revision then.
    written: HashMap<BufferId, (Key, u64)>,
    /// Backup files to remove (buffers saved, reverted to clean, closed; discarded offers).
    remove: HashSet<Key>,
    /// The running job (removals, then writes): one at a time, so a removal never overtakes the
    /// write it undoes. `shutdown` awaits it.
    task: Option<tokio::task::JoinHandle<()>>,
    /// The deadline passed while a job or the start's scan ran: write once it ends.
    overdue: bool,
    /// The start's scan for this workspace's backups is running (nothing is written meanwhile:
    /// it would replace a backup before it's offered).
    scanning: bool,
    /// Backups found at start, until the offer is answered.
    pub offered: Vec<Backup>,
    /// The offer waits for another prompt (the trust dialog) to close.
    offer_pending: bool,
    /// Put off with "Later": left on disk by a clean quit, offered again next start (unless this
    /// session backs up the same file, replacing them).
    kept: HashSet<Key>,
    /// Restores in flight (a file opening, a diff off-thread), and their outcome so far.
    restoring: usize,
    restored: Vec<String>,
    changed_on_disk: Vec<String>,
    /// A write failed and was reported (once per session; later failures are only logged).
    reported: bool,
    /// Jobs finished, for tests.
    pub jobs_done: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
impl BackupState {
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// A job or the start's scan is running.
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.scanning
    }

    /// A buffer restored from the very backup file: it holds this text at `revision`.
    pub(super) fn mark_written(&mut self, id: BufferId, key: Key, revision: u64) {
        self.written.insert(id, (key, revision));
    }

    pub fn restoring(&self) -> usize {
        self.restoring
    }
}

/// A finished backup job (`AppEvent::BackupDone`).
pub struct Done {
    /// Buffers whose write failed: due again.
    failed: Vec<BufferId>,
    errors: Vec<String>,
}

/// A backup diffed against its buffer, to restore it (`AppEvent::BackupRestore`).
pub struct Restoring {
    buffer: BufferId,
    revision: u64,
    backup: Backup,
    changes: ChangeSet,
}

impl App {
    /// Looks for backups a killed or crashed jed left for this workspace (`start`) →
    /// `AppEvent::BackupsFound` → `backups_found`. Nothing without a state dir.
    pub(super) fn scan_backups(&mut self) {
        let Some(dir) = self.state_dir.clone() else { return };
        let root = self.workspace.root.clone();
        self.backups.scanning = true;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::BackupsFound(backup::list(&dir, &root)));
        });
    }

    /// Offers the backups found once the session is restored (its tabs exist).
    pub(super) fn backups_found(&mut self, found: Vec<Backup>) {
        self.backups.scanning = false;
        let mut found = found;
        // Single file mode offers only its own file's backup; the workspace's others stay.
        if self.mode.is_single() {
            found.retain(|b| b.untitled.is_none() && self.buffers.iter().any(|(_, buf)| buf.path == b.path));
        }
        tracing::info!(count = found.len(), "backups found");
        self.backups.offered = found;
        self.drop_open_untitled_offers();
        if !self.session.loading() {
            self.offer_backups();
        }
        if std::mem::take(&mut self.backups.overdue) {
            self.start_backup_job(true);
        }
    }

    /// Untitled backups whose buffer is open (the session restored it from the very file) aren't
    /// offered again.
    pub(super) fn drop_open_untitled_offers(&mut self) {
        let open: HashSet<&str> = self.buffers.iter().filter_map(|(_, b)| b.untitled_id()).collect();
        self.backups
            .offered
            .retain(|b| b.untitled.as_deref().is_none_or(|id| !open.contains(id)));
    }

    /// The restore prompt, when there are backups to offer and no other prompt is open (else
    /// once it closes: `close_prompt`).
    pub(super) fn offer_backups(&mut self) {
        if self.backups.offered.is_empty() {
            return;
        }
        if self.prompt.is_some() {
            self.backups.offer_pending = true;
            return;
        }
        self.backups.offer_pending = false;
        self.prompt = Some(Prompt::RestoreBackups);
    }

    pub(super) fn backup_offer_pending(&self) -> bool {
        self.backups.offer_pending
    }

    /// The prompt's Restore: each backup's text into its buffer (the file opened if needed).
    pub(super) fn restore_backups(&mut self) {
        for b in std::mem::take(&mut self.backups.offered) {
            self.backups.restoring += 1;
            self.restore_backup(b);
        }
    }

    /// The prompt's Discard: the backups are removed.
    pub(super) fn discard_backups(&mut self) {
        let n = self.backups.offered.len();
        for b in std::mem::take(&mut self.backups.offered) {
            self.backups.remove.insert(b.key());
        }
        self.sync_backups();
        self.notify(Level::Info, format!("Discarded {}", files(n, "backup")));
    }

    /// The prompt's Later (Esc): the backups stay on disk, offered again at the next start.
    pub(super) fn keep_backups(&mut self) {
        let n = self.backups.offered.len();
        for b in std::mem::take(&mut self.backups.offered) {
            self.backups.kept.insert(b.key());
        }
        self.notify(
            Level::Info,
            format!(
                "Kept {}: offered again next time jed opens this folder",
                files(n, "backup")
            ),
        );
    }

    /// Diffs `b` against its buffer off-thread (→ `AppEvent::BackupRestore`); a file not open
    /// is opened first (`OpenRequest::backup`, restored once `opened`).
    pub(super) fn restore_backup(&mut self, b: Backup) {
        if let Some(uid) = b.untitled.clone() {
            return self.restore_untitled_backup(b, uid);
        }
        let Some(id) = self.buffers.find_path(&b.path) else {
            let path = b.path.clone();
            self.open_file(path.clone(), false, None);
            match self.opening.get_mut(&path) {
                Some(request) => request.backup = Some(Box::new(b)),
                // `open_file` always queues a file that isn't loaded.
                None => {
                    tracing::warn!("backup of {} not restored: its file isn't opening", path.display());
                    self.backups.kept.insert(Key::File(path));
                    self.backup_restore_ended();
                }
            }
            return;
        };
        let buf = self.buffers.get(id).expect("just found");
        if let Some(reason) = buf.read_only.clone() {
            let text = format!("Backup of {} not restored: read-only, {reason}", buf.name());
            self.backups.kept.insert(b.key());
            self.notify(Level::Error, text);
            self.backup_restore_ended();
            return;
        }
        let (rope, revision) = (buf.rope.clone(), buf.revision());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let changes = crate::disk::changes(&rope, &b.text);
            let r = Restoring {
                buffer: id,
                revision,
                backup: b,
                changes,
            };
            let _ = tx.send(AppEvent::BackupRestore(Box::new(r)));
        });
    }

    /// An untitled buffer's backup: a new "Untitled-N" tab with its text, left dirty, keeping the
    /// backup's id (the file is the buffer's backup until it is clean). Already open: nothing.
    fn restore_untitled_backup(&mut self, b: Backup, uid: String) {
        if self.buffers.iter().any(|(_, buf)| buf.untitled_id() == Some(&uid)) {
            return self.backup_restore_ended();
        }
        let n = self.buffers.free_untitled_number();
        let buffer = Buffer::untitled_restored(n, uid, b.text, b.bom);
        let name = buffer.name();
        let (id, view) = self.register_buffer(buffer, None);
        self.group_mut().open(id, view);
        let rev = self.buffers.get(id).map_or(0, |b| b.revision());
        self.backups
            .written
            .insert(id, (Key::Untitled(b.untitled.expect("untitled")), rev));
        self.backups.restored.push(name);
        self.backup_restore_ended();
    }

    /// A backup's file couldn't be opened: the backup stays for a later start.
    pub(super) fn backup_not_opened(&mut self, b: Backup, error: String) {
        let text = format!("Backup of {} not restored: {error}", b.path.display());
        self.backups.kept.insert(b.key());
        self.notify(Level::Error, text);
        self.backup_restore_ended();
    }

    /// `AppEvent::BackupRestore`: the backup's text replaces the buffer's as one undo step
    /// (`apply_changes`), left dirty, nothing written. A buffer edited meanwhile is diffed
    /// again. A file changed on disk since the backup was taken: the buffer is based on the
    /// backup's version (`Buffer::set_disk_base`), so a disk check shows the banner and a save
    /// refuses to overwrite the newer file unasked.
    pub(super) fn backup_restored(&mut self, r: Restoring) {
        let Restoring {
            buffer,
            revision,
            backup,
            changes,
        } = r;
        let Some(buf) = self.buffers.get(buffer).filter(|b| b.path == backup.path) else {
            // Closed meanwhile: by path again.
            return self.restore_backup(backup);
        };
        if buf.revision() != revision {
            return self.restore_backup(backup);
        }
        let name = buf.name();
        if !changes.is_empty() {
            self.apply_changes(buffer, changes);
            self.follow_edits();
        }
        let buf = self.buffers.get_mut(buffer).expect("checked above");
        // The backup file holds this text now: removed once the buffer is clean.
        self.backups
            .written
            .insert(buffer, (Key::File(backup.path), buf.revision()));
        if buf.has_edits() && buf.disk() != backup.disk {
            buf.set_disk_base(backup.disk);
            self.backups.changed_on_disk.push(name.clone());
            self.check_disk(buffer, false);
        }
        self.backups.restored.push(name);
        self.backup_restore_ended();
    }

    /// One restore ended; the last one says what was restored.
    fn backup_restore_ended(&mut self) {
        self.backups.restoring = self.backups.restoring.saturating_sub(1);
        if self.backups.restoring > 0 {
            return;
        }
        let restored = std::mem::take(&mut self.backups.restored);
        let changed = std::mem::take(&mut self.backups.changed_on_disk);
        if restored.is_empty() {
            return;
        }
        let mut text = format!("Restored unsaved changes to {} (not saved)", restored.join(", "));
        if changed.is_empty() {
            self.notify(Level::Info, text);
        } else {
            text.push_str(&format!("; changed on disk since: {}", changed.join(", ")));
            self.notify(Level::Warning, text);
        }
    }

    /// Each loop turn (and `pump` / `settle`): backups of buffers now clean, closed or renamed
    /// are queued for removal (and removed at once when no job runs); the deadline is set when
    /// a dirty buffer differs from its backup. O(buffers).
    pub(super) fn sync_backups(&mut self) {
        if self.state_dir.is_none() {
            return;
        }
        let buffers = &self.buffers;
        let remove = &mut self.backups.remove;
        self.backups.written.retain(|&id, (key, _)| {
            let keep = buffers
                .get(id)
                .is_some_and(|b| b.is_dirty() && buffer_key(b).as_ref() == Some(key));
            if !keep {
                remove.insert(key.clone());
            }
            keep
        });
        if self.backups.deadline.is_none()
            && !self.backups.overdue
            && self.buffers.iter().any(|(id, b)| self.backup_due(id, b))
        {
            self.backups.deadline = Some(Instant::now() + backup::INTERVAL);
        }
        if self.backups.task.is_none() && !self.backups.remove.is_empty() {
            self.start_backup_job(false);
        }
    }

    /// Dirty and different from its backup (if it has one), and not a file whose backup is
    /// still being offered.
    fn backup_due(&self, id: BufferId, b: &crate::buffer::Buffer) -> bool {
        let Some(key) = buffer_key(b) else { return false };
        b.is_dirty()
            && self
                .backups
                .written
                .get(&id)
                .is_none_or(|(k, rev)| *rev != b.revision() || *k != key)
            && !self.backups.offered.iter().any(|o| o.key() == key)
    }

    /// The backup deadline (`Timer::Backup`, `test_backup_tick`): writes every buffer due,
    /// or once the running job (or the start's scan) ends.
    pub(super) fn backup_tick(&mut self, now: Instant) {
        if self.backups.deadline.is_none_or(|d| d > now) {
            return;
        }
        self.backups.deadline = None;
        if self.backups.busy() {
            self.backups.overdue = true;
        } else {
            self.start_backup_job(true);
        }
    }

    /// Starts a job: the queued removals, then (with `write`) every buffer due, in
    /// `spawn_blocking` → `AppEvent::BackupDone`. Nothing to do: nothing starts.
    fn start_backup_job(&mut self, write: bool) {
        let Some(dir) = self.state_dir.clone() else { return };
        let mut writes = Vec::new();
        if write {
            for (id, b) in self.buffers.iter() {
                if self.backup_due(id, b) {
                    writes.push((id, snapshot(b)));
                }
            }
        }
        for (id, s) in &writes {
            let rev = self.buffers.get(*id).map_or(0, |b| b.revision());
            self.backups.written.insert(*id, (s.key(), rev));
            self.backups.kept.remove(&s.key());
            // Replaced by the write.
            self.backups.remove.remove(&s.key());
        }
        let removes: Vec<Key> = self.backups.remove.drain().collect();
        if writes.is_empty() && removes.is_empty() {
            return;
        }
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        self.backups.task = Some(tokio::task::spawn_blocking(move || {
            let mut done = Done {
                failed: Vec::new(),
                errors: Vec::new(),
            };
            for key in removes {
                if let Err(e) = backup::remove(&dir, &root, &key) {
                    done.errors
                        .push(format!("removing the backup of {}: {e}", key.describe()));
                }
            }
            let time = crate::history::now_ms();
            for (id, s) in writes {
                match backup::write(&dir, &root, &s, time) {
                    Ok(at) => tracing::debug!("backed up {} to {}", s.key().describe(), at.display()),
                    Err(e) => {
                        done.failed.push(id);
                        done.errors.push(format!("{}: {e}", s.key().describe()));
                    }
                }
            }
            let _ = tx.send(AppEvent::BackupDone(done));
        }));
    }

    /// `AppEvent::BackupDone`: failed writes are due again (reported once); an overdue write
    /// starts now.
    pub(super) fn backup_done(&mut self, done: Done) {
        self.backups.task = None;
        self.backups.jobs_done += 1;
        for id in done.failed {
            self.backups.written.remove(&id);
        }
        for e in &done.errors {
            tracing::warn!("backup: {e}");
        }
        if let Some(first) = done.errors.first()
            && !std::mem::replace(&mut self.backups.reported, true)
        {
            self.notify(Level::Warning, format!("Unsaved changes not backed up: {first}"));
        }
        if std::mem::take(&mut self.backups.overdue) {
            self.start_backup_job(true);
        }
        self.sync_backups();
    }

    /// `shutdown`, after the session's write: the running job ends first. A clean quit (every
    /// buffer saved or discarded) removes this workspace's backups, except those put off with
    /// Later or not yet answered; the terminal going away writes every dirty buffer's backup
    /// instead. While the start's scan still runs its backups aren't offered yet: a clean quit
    /// removes nothing and the terminal going away writes only files that have no backup.
    pub(super) async fn finish_backups(&mut self) {
        if let Some(task) = self.backups.task.take() {
            let _ = task.await;
        }
        let Some(dir) = self.state_dir.clone() else { return };
        let scanning = self.backups.scanning;
        if scanning && self.should_quit {
            return;
        }
        let root = self.workspace.root.clone();
        let result = if self.should_quit {
            let mut keep = self.backups.kept.clone();
            keep.extend(self.backups.offered.iter().map(|b| b.key()));
            tokio::task::spawn_blocking(move || {
                backup::remove_all(&dir, &root, &keep)
                    .map(|n| tracing::debug!("{n} backups removed"))
                    .map_err(|e| e.to_string())
            })
            .await
        } else {
            let offered: HashSet<Key> = self.backups.offered.iter().map(|b| b.key()).collect();
            let snaps: Vec<Snapshot> = self
                .buffers
                .iter()
                .filter(|(_, b)| b.is_dirty() && buffer_key(b).is_some_and(|k| !offered.contains(&k)))
                .map(|(_, b)| snapshot(b))
                .collect();
            tokio::task::spawn_blocking(move || {
                let time = crate::history::now_ms();
                for s in &snaps {
                    if scanning && backup::key_file(&dir, &root, &s.key()).exists() {
                        tracing::info!("backup of {} not replaced: not offered yet", s.key().describe());
                        continue;
                    }
                    backup::write(&dir, &root, s, time).map_err(|e| format!("{}: {e}", s.key().describe()))?;
                }
                Ok(())
            })
            .await
        };
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("backups at quit: {e}"),
            Err(e) => tracing::warn!("backups at quit: {e}"),
        }
    }

    /// Every loop turn: the dirty buffers the panic hook backs up (same files and format).
    pub(super) fn update_crash_snapshot(&self) {
        let Some(dir) = self.state_dir.clone() else { return };
        let snapshots = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_dirty() && buffer_key(b).is_some())
            .map(|(_, b)| snapshot(b))
            .collect();
        crate::panic::set_dirty(dir, self.workspace.root.clone(), snapshots);
    }
}

/// What `b`'s backup is keyed by: its path, or an untitled buffer's id (`None`: a log, a commit).
fn buffer_key(b: &Buffer) -> Option<Key> {
    if b.has_file() {
        Some(Key::File(b.path.clone()))
    } else {
        b.untitled_id().map(|id| Key::Untitled(id.to_string()))
    }
}

fn snapshot(b: &Buffer) -> Snapshot {
    Snapshot {
        path: b.path.clone(),
        untitled: b.untitled_id().map(str::to_string),
        rope: b.rope.clone(),
        bom: b.bom,
        disk: b.disk(),
    }
}

/// "1 file" / "3 files" style counts.
fn files(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}
