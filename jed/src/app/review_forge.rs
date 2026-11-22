//! Saving review drafts in the forge's pending review (`review_drafts_on_forge`; the draft jobs of
//! `crate::review_write`).
//!
//! The local draft is always written first. With the setting on, the app then queues a
//! [`DraftOp`]; ops run one at a time, in order, and not while another write is on its way
//! (`review.writing`), so an edit can't overtake its add. Ids and text are read from the store when
//! an op starts. A refusal or failure keeps the draft local (its reason in `drafts.errors`); a 404
//! on an edit or delete means the draft is gone on the forge, so it is dropped here too.
//!
//! Claude never posts: a draft with `by_claude` is never queued. Only the user editing and saving
//! it (which makes it theirs) sends it.

use super::review_comment::ComposerKind;
use super::{App, Level};
use crate::forge::{DraftComment, NewDraft};
use crate::review::DraftOp;
use crate::review_blocks::SubmitLabel;
use crate::review_drafts::{Draft, FileHunks, ForgeDraft};
use crate::review_write::{Done, Job};

/// What the forge is asked to hold for `d`.
pub(super) fn new_draft(d: &Draft, hunks: &std::collections::BTreeMap<String, FileHunks>) -> NewDraft {
    match &d.reply_to {
        Some(thread) => NewDraft::Reply {
            thread_id: thread.clone(),
            body: d.body.clone(),
            lines: (d.start, d.end),
            resolve: d.resolve,
        },
        None => NewDraft::Line(DraftComment {
            anchor: d.anchor(),
            body: d.body.clone(),
            other: super::review_write::other_lines(d, hunks),
        }),
    }
}

impl App {
    /// The comment box's save button.
    pub(super) fn submit_label(&self) -> SubmitLabel {
        if !matches!(
            self.composer.as_ref().map(|c| &c.kind),
            Some(ComposerKind::Draft | ComposerKind::Reply { .. })
        ) {
            return SubmitLabel::Comment;
        }
        if !self.settings.review_drafts_on_forge {
            SubmitLabel::SaveDraft
        } else if self.pending_review_id().is_some() {
            SubmitLabel::AddToReview
        } else {
            SubmitLabel::StartReview
        }
    }

    /// The forge's pending review of the shown merge request, once a draft made or found it.
    pub(crate) fn pending_review_id(&self) -> Option<&str> {
        let (key, id) = self.review.drafts.pending.as_ref()?;
        (self.review.drafts_key().as_deref() == Some(key.as_str())).then_some(id.as_str())
    }

    /// Folds the pending review the last lookup read into the drafts (`DraftStore::reconcile`):
    /// drafts made on the website appear, ones submitted or deleted there go (one notice), bodies
    /// changed there win. Nothing is done when the setting is off, the read failed, a write to the
    /// forge is on its way or finished since the lookup started, or the drafts aren't read yet.
    pub(super) fn reconcile_drafts(&mut self) {
        let d = &self.review.drafts;
        if !self.settings.review_drafts_on_forge
            || self.review.writing
            || d.inflight.is_some()
            || !d.queue.is_empty()
            || d.forge_writes != d.looked_writes
        {
            return;
        }
        let (Some(review), Some(key)) = (self.review.review(), self.review.drafts_key()) else {
            return;
        };
        if review.pending_error.is_some() || d.store.is_none() {
            return;
        }
        let forge_name = review.kind.display_name();
        let pending = review.pending.clone();
        let threads = review.threads.clone();
        let Some(pending) = pending else {
            // Nothing pending there: every draft that was is gone.
            let gone: Vec<String> = d
                .store
                .as_ref()
                .map(|s| {
                    s.get(&key)
                        .iter()
                        .filter(|x| x.forge.is_some())
                        .map(|x| x.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            self.review.drafts.pending = None;
            return self.reconciled(gone, 0, 0, forge_name);
        };
        self.review.drafts.pending = Some((key.clone(), pending.review.id.clone()));
        let Some(store) = self.review.drafts.store.as_mut() else {
            return;
        };
        let r = store.reconcile(&key, &pending, &threads, &|_| false);
        self.reconciled(r.dropped, r.added, r.changed, forge_name);
    }

    fn reconciled(&mut self, dropped: Vec<String>, added: usize, changed: usize, forge_name: &str) {
        let key = self.review.drafts_key();
        if let (Some(store), Some(key)) = (self.review.drafts.store.as_mut(), key) {
            for id in &dropped {
                store.remove(&key, id);
            }
        }
        if dropped.is_empty() && added == 0 && changed == 0 {
            return;
        }
        let d = &mut self.review.drafts;
        for id in &dropped {
            d.drift.remove(id);
            d.live.remove(id);
            d.at.remove(id);
            d.errors.remove(id);
            d.local_why.remove(id);
        }
        self.save_drafts();
        if !dropped.is_empty() {
            let n = dropped.len();
            let text = if n == 1 {
                format!("1 draft was submitted or deleted on {forge_name}")
            } else {
                format!("{n} drafts were submitted or deleted on {forge_name}")
            };
            self.notify(Level::Info, text);
        }
        let len = self.review.draft_rows().len();
        let p = &mut self.review.view.pending;
        p.select(len, p.selected(len).unwrap_or(0));
        if len == 0 {
            p.focused = false;
        }
        self.review_blocks_changed();
    }

    fn add_pending(&self, id: &str) -> bool {
        let d = &self.review.drafts;
        let is = |op: &DraftOp| matches!(op, DraftOp::Add(i) if i == id);
        d.queue.iter().any(is) || d.inflight.as_ref().is_some_and(is)
    }

    /// Draft `id` was saved (new or edited): sends it to the forge when the setting is on and it
    /// isn't Claude's.
    pub(super) fn draft_saved(&mut self, id: &str) {
        if !self.settings.review_drafts_on_forge {
            return;
        }
        let Some(key) = self.review.drafts_key() else { return };
        let Some(d) = self.review.drafts.store.as_ref().and_then(|s| s.find(&key, id)) else {
            return;
        };
        if d.by_claude {
            return;
        }
        let op = if d.forge.is_some() || self.add_pending(id) {
            DraftOp::Edit(id.to_string())
        } else {
            DraftOp::Add(id.to_string())
        };
        self.review.drafts.errors.remove(id);
        self.review.drafts.local_why.remove(id);
        self.review.drafts.queue.push_back(op);
        self.draft_pump();
    }

    /// Draft `id` is being deleted: gone at once when the forge doesn't have it, else once the
    /// forge says so.
    pub(super) fn draft_deleted(&mut self, id: &str) {
        let key = self.review.drafts_key();
        let on_forge = key
            .as_deref()
            .and_then(|k| self.review.drafts.store.as_ref()?.find(k, id))
            .is_some_and(|d| d.forge.is_some());
        self.review.drafts.queue.retain(|op| op.id() != id);
        if on_forge {
            self.review.drafts.queue.push_back(DraftOp::Delete {
                id: id.to_string(),
                r: None,
            });
            self.review.drafts.errors.remove(id);
            self.review_blocks_changed();
            self.notify(Level::Info, "Deleting the draft on the forge…".into());
            self.draft_pump();
        } else {
            self.drop_draft(id);
            self.notify(Level::Info, "Draft deleted".into());
        }
    }

    /// Starts the next queued draft write, when nothing else is on its way.
    pub(super) fn draft_pump(&mut self) {
        while !self.review.writing && self.review.drafts.inflight.is_none() {
            let Some(op) = self.review.drafts.queue.pop_front() else {
                return;
            };
            let Some(job) = self.draft_job(&op) else { continue };
            if self.review_start(job) {
                self.review.drafts.inflight = Some(op);
            }
        }
    }

    fn draft_job(&self, op: &DraftOp) -> Option<Job> {
        let review = self.review.review()?;
        let key = self.review.drafts_key()?;
        let store = self.review.drafts.store.as_ref()?;
        let number = review.mr.number;
        match op {
            DraftOp::Add(id) => {
                let d = store.find(&key, id)?;
                if d.by_claude || d.forge.is_some() {
                    return None;
                }
                Some(Job::AddDraft {
                    number,
                    head_sha: review.mr.head.sha.clone(),
                    pending: self.pending_review_id().map(str::to_string),
                    id: id.clone(),
                    draft: new_draft(d, &review.hunks),
                })
            }
            DraftOp::Edit(id) => {
                let d = store.find(&key, id)?;
                let r = d.forge.as_ref()?.to_ref();
                Some(Job::EditDraft {
                    number,
                    id: id.clone(),
                    r,
                    body: d.body.clone(),
                    range: (d.end > d.start).then_some((d.start, d.end)),
                    lines: (d.start, d.end),
                })
            }
            DraftOp::Delete { id, r } => {
                let r = match r {
                    Some(r) => r.to_ref(),
                    None => store.find(&key, id)?.forge.as_ref()?.to_ref(),
                };
                Some(Job::DeleteDraft {
                    number,
                    id: id.clone(),
                    r,
                })
            }
        }
    }

    /// A draft job finished (`Done::Draft` / `Done::DraftFailed`, or a `Done::Failed` of one).
    pub(super) fn draft_done(&mut self, done: Done) {
        let op = self.review.drafts.inflight.take();
        let key = self.review.drafts_key();
        let forge_name = self.review.review().map_or("the forge", |r| r.kind.display_name());
        match (done, op) {
            (Done::Draft { id, r: Some(r), .. }, Some(DraftOp::Add(_))) => {
                let place = ForgeDraft::of(&r);
                if let Some(key) = &key {
                    self.review.drafts.pending = Some((key.clone(), r.review_id.clone()));
                }
                let kept = match (&key, self.review.drafts.store.as_mut()) {
                    (Some(k), Some(s)) => s.set_forge(k, &id, Some(place.clone())),
                    _ => false,
                };
                if kept {
                    self.save_drafts();
                } else {
                    // Deleted while the add ran: take it off the forge again.
                    self.review
                        .drafts
                        .queue
                        .push_front(DraftOp::Delete { id, r: Some(place) });
                }
            }
            (Done::Draft { id, .. }, Some(DraftOp::Delete { .. })) => self.drop_draft(&id),
            (Done::DraftFailed { job, error, gone }, _) => {
                let (Job::AddDraft { id, .. } | Job::EditDraft { id, .. } | Job::DeleteDraft { id, .. }) = &job else {
                    return self.draft_pump();
                };
                let id = id.clone();
                if gone && !matches!(job, Job::AddDraft { .. }) {
                    self.drop_draft(&id);
                    self.notify(Level::Info, format!("The draft was already gone on {forge_name}"));
                } else {
                    let text = match job {
                        Job::AddDraft { .. } => format!("Draft kept locally: {error}"),
                        Job::EditDraft { .. } => format!("Draft not updated on {forge_name}: {error}"),
                        _ => format!("Draft not deleted on {forge_name}: {error}"),
                    };
                    if matches!(job, Job::AddDraft { .. }) {
                        self.review.drafts.local_why.insert(id, error);
                    } else {
                        self.review.drafts.errors.insert(id, error);
                    }
                    self.notify(Level::Error, text);
                }
            }
            _ => {}
        }
        self.review_blocks_changed();
        self.draft_pump();
    }

    /// Removes draft `id` from the store and everything keyed by it.
    pub(super) fn drop_draft(&mut self, id: &str) {
        let key = self.review.drafts_key();
        if let (Some(store), Some(key)) = (self.review.drafts.store.as_mut(), key) {
            store.remove(&key, id);
        }
        let d = &mut self.review.drafts;
        d.drift.remove(id);
        d.live.remove(id);
        d.at.remove(id);
        d.errors.remove(id);
        d.local_why.remove(id);
        self.save_drafts();
        let len = self.review.draft_rows().len();
        let p = &mut self.review.view.pending;
        p.select(len, p.selected(len).unwrap_or(0));
        if len == 0 {
            p.focused = false;
        }
        self.review_blocks_changed();
    }
}
