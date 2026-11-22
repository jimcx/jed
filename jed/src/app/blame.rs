//! Inline blame in the app: "Git: Toggle Inline Blame" (session state, off at start) and keeping
//! the blame of shown buffers current while it's on — computed off-thread at once for a buffer
//! never blamed, when HEAD may have moved (every git status read) or a walk chunk arrived,
//! `blame::DELAY` after edits.
//! `ui::editor` draws the primary cursor line's label from `buf.blame`, `ui::diff` a review
//! diff's caret line's (on its right side) from its file's buffer's.

use std::collections::HashSet;
use std::time::Instant;

use super::{App, AppEvent};
use crate::blame::{self, Computed, Job, Want};
use crate::buffer::BufferId;

/// The app's side of inline blame.
#[derive(Default)]
pub struct BlameState {
    /// The toggle (session only, never written to a file).
    pub on: bool,
    /// When the blame of edited buffers is computed again.
    deadline: Option<Instant>,
    /// Tests: the time slice of the walks started (short, so a chunk is cheap to hold back).
    #[cfg(test)]
    pub slice: Option<std::time::Duration>,
}

impl BlameState {
    /// Tests: a deadline at `at`, as the module's own code would arm it.
    #[cfg(test)]
    pub fn test_arm(&mut self, at: Instant) {
        self.deadline = Some(at);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

impl App {
    /// `git.toggleBlame`. Off forgets every buffer's blame (a running computation's result is
    /// dropped); on computes the shown buffers' at the next turn.
    pub(super) fn toggle_blame(&mut self) {
        self.blame.on = !self.blame.on;
        if !self.blame.on {
            self.blame.deadline = None;
            for (_, buf) in self.buffers.iter_mut() {
                buf.blame.reset();
            }
        }
        self.sync_blame();
    }

    /// Each loop turn while the blame is on: starts the computations due for buffers shown in a
    /// group (never blamed, HEAD may have moved; edited ones once the delay passed) and sets the
    /// delay's deadline for the others.
    pub(super) fn sync_blame(&mut self) {
        if !self.blame.on || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let due = self.blame.deadline.is_some_and(|d| d <= Instant::now());
        if due {
            self.blame.deadline = None;
        }
        // A text tab's buffer, or the file's buffer a review diff shows on its right side.
        let shown: HashSet<BufferId> = self
            .groups
            .ids()
            .into_iter()
            .filter_map(|g| {
                let g = self.groups.get(g)?;
                match g.active_tab() {
                    Some(t) => Some(t.buffer),
                    None => self.buffer_of(&g.active_diff().filter(|d| d.pair.is_review())?.path),
                }
            })
            .collect();
        let repo = self.git.status.as_ref().map(|s| &s.repo);
        #[cfg(test)]
        let slice = self.blame.slice;
        #[cfg(not(test))]
        let slice = None;
        let mut later = false;
        let mut start = Vec::new();
        for id in shown {
            let Some(buf) = self.buffers.get_mut(id) else { continue };
            let revision = buf.revision();
            let want = buf.blame.want(revision);
            if want == Want::Nothing {
                continue;
            }
            let Some((repo, path)) = repo.and_then(|r| Some((r, r.relative(&buf.path)?))) else {
                // No repository (yet: its status read calls `head_changed`), or outside it.
                buf.blame.clear(revision);
                self.needs_redraw = true;
                continue;
            };
            if want == Want::Later && !due {
                later = true;
                continue;
            }
            let begin = buf.blame.begin(revision);
            start.push((
                id,
                Job {
                    repo: repo.clone(),
                    path,
                    rope: buf.rope.clone(),
                    bom: buf.bom,
                    revision,
                    generation: begin.generation,
                    base: begin.base,
                    walking: begin.walking,
                    same_text: begin.same_text,
                    slice,
                },
            ));
        }
        if later && self.blame.deadline.is_none() {
            self.blame.deadline = Some(Instant::now() + blame::DELAY);
        }
        for (buffer, job) in start {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let t = Instant::now();
                let computed = blame::compute(job);
                if t.elapsed() > super::SLOW_TASK {
                    tracing::warn!(elapsed_ms = t.elapsed().as_millis() as u64, "slow git blame");
                }
                let _ = tx.send(AppEvent::Gutter(super::quick_diff::GutterEvent::Blame {
                    buffer,
                    computed,
                }));
            });
        }
    }

    /// `AppEvent::Gutter(GutterEvent::Blame)`: a buffer's blame was computed; a walk it starts
    /// runs on a blocking thread, sending its chunks as `GutterEvent::BlameChunk`.
    pub(super) fn blame_done(&mut self, buffer: BufferId, computed: Computed) {
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        self.needs_redraw = true;
        let Some(start) = buf.blame.finish(computed) else {
            return;
        };
        let tx = self.tx.clone();
        #[cfg(test)]
        let probe = self.blame_probe.clone();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            probe.walks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let id = start.id;
            let send = |result: Result<crate::git::blame::Chunk, String>| {
                // Counted before the send: the app may handle the chunk (and a test look) at once.
                #[cfg(test)]
                probe.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let sent = tx
                    .send(AppEvent::Gutter(super::quick_diff::GutterEvent::BlameChunk {
                        buffer,
                        walk: id,
                        result,
                    }))
                    .is_ok();
                #[cfg(test)]
                probe.wait_if_held();
                sent
            };
            crate::git::blame::stream(*start.walk, &start.cancel, send);
            #[cfg(test)]
            probe.exited.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// A chunk of a buffer's walk: the commits it settled fill the blame in (the next loop turn
    /// computes the hunks again, a diff only). A stopped walk's chunk is dropped.
    pub(super) fn blame_chunk(
        &mut self,
        buffer: BufferId,
        walk: u64,
        result: Result<crate::git::blame::Chunk, String>,
    ) {
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        let taken = match &result {
            Ok(c) => buf.blame.chunk(walk, Ok(&c.settled), c.done),
            Err(e) => buf.blame.chunk(walk, Err(e), true),
        };
        if taken {
            self.needs_redraw = true;
        }
    }

    /// HEAD (or the repository) may have changed: shown buffers' blame is computed again (only
    /// the HEAD check when it didn't).
    pub(super) fn blame_head_changed(&mut self) {
        if !self.blame.on {
            return;
        }
        for (_, buf) in self.buffers.iter_mut() {
            buf.blame.head_changed();
        }
    }
}
