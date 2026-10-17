//! Inline blame in the app: "Git: Toggle Inline Blame" (session state, off at start) and keeping
//! the blame of shown buffers current while it's on — computed off-thread at once for a buffer
//! never blamed or when HEAD may have moved (every git status read), `blame::DELAY` after edits.
//! `ui::editor` draws the primary cursor line's label from `buf.blame`.

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
}

impl BlameState {
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
        let shown: HashSet<BufferId> = self
            .groups
            .ids()
            .into_iter()
            .filter_map(|g| Some(self.groups.get(g)?.active_tab()?.buffer))
            .collect();
        let repo = self.git.status.as_ref().map(|s| &s.repo);
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
            let (generation, base, same_text) = buf.blame.begin(revision);
            start.push((
                id,
                Job {
                    repo: repo.clone(),
                    path,
                    rope: buf.rope.clone(),
                    bom: buf.bom,
                    revision,
                    generation,
                    base,
                    same_text,
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
                let _ = tx.send(AppEvent::Blame { buffer, computed });
            });
        }
    }

    /// `AppEvent::Blame`: a buffer's blame was computed.
    pub(super) fn blame_done(&mut self, buffer: BufferId, computed: Computed) {
        if let Some(buf) = self.buffers.get_mut(buffer) {
            buf.blame.finish(computed);
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
