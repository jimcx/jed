//! The spell checker in the app: dictionaries loaded off-thread, each buffer's misspellings kept
//! in `buf.spell.marks` (a `diagnostics::Marks` of their own, under `ServerId::SPELL`), checked
//! in full once (open, a settings or dictionary change) and afterwards only on the lines an edit
//! touched (`Buffer::spell` collects them), `DELAY` after the last edit. Small checks run here;
//! big ones run on a snapshot in `spawn_blocking` and report as `SpellEvent::Done`, dropped when
//! the buffer moved on.
//!
//! The half-typed word: a misspelling touching the word the caret is in is held back after an
//! edit-driven check (no `recieve` squiggle under every prefix of `recieveMessage`), and checked
//! again once the caret leaves that word.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ropey::Rope;
use tree_sitter::Tree;

use super::{App, AppEvent, Level, SLOW_TASK};
use crate::buffer::BufferId;
use crate::buffer::pos::{self, ByteIdx};
use crate::diagnostics::{Diagnostic, Mark, Severity};
use crate::edit::SpellScope;
use crate::lsp::ServerId;
use crate::spell_dict::{self, UserWords, Which};
use crate::spell_scope;
use crate::spellcheck::{Checker, Dictionary, Misspelling};
use crate::syntax::Language;

/// How long after the last edit the touched lines are checked again.
pub const DELAY: Duration = Duration::from_millis(300);

/// Checks over this many bytes run in the background.
const INLINE_MAX: usize = 16 * 1024;

/// The settings a check depends on: a change checks every buffer again.
#[derive(Debug, Clone, PartialEq)]
struct Signature {
    scope: SpellScope,
    min_length: usize,
    language: String,
    ignore_words: Vec<String>,
    off_languages: Vec<String>,
    max_bytes: usize,
}

/// The app's side of the spell checker.
#[derive(Default)]
pub struct SpellApp {
    dict: Option<Arc<Dictionary>>,
    loading: bool,
    user: UserWords,
    workspace: UserWords,
    /// User + workspace + `spell_check_ignore_words`, lower-case.
    extra: Arc<HashSet<String>>,
    checkers: HashMap<Language, Arc<Checker>>,
    /// Bumped by a settings or dictionary change: buffers checked under an older one start over.
    generation: u64,
    signature: Option<Signature>,
    pub(super) deadline: Option<Instant>,
    told_missing: Option<String>,
    /// Tests: bytes scanned on this thread / checks started in the background.
    #[cfg(test)]
    pub scanned: usize,
    #[cfg(test)]
    pub background: usize,
}

impl SpellApp {
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

/// Results of the spell checker's background work (`AppEvent::Spell`).
pub(super) enum SpellEvent {
    /// A background spell check finished.
    Done(Box<Done>),
    /// The dictionary or word files were read.
    Loaded(Box<Loaded>),
    /// Words were written to a dictionary file (`spell_cmd.rs`).
    Added(Box<Added>),
    /// "Open User Dictionary": the file exists now (or why not).
    DictionaryReady(Result<std::path::PathBuf, String>),
}

/// The outcome of adding words to a dictionary file.
pub struct Added {
    pub user: bool,
    /// Words newly written.
    pub added: Vec<String>,
    /// Words the file had already.
    pub known: usize,
    pub error: Option<String>,
}

impl App {
    pub(super) fn handle_spell_event(&mut self, ev: SpellEvent) {
        match ev {
            SpellEvent::Done(done) => self.spell_done(*done),
            SpellEvent::Loaded(loaded) => self.spell_loaded(*loaded),
            SpellEvent::Added(added) => self.spell_words_added(*added),
            SpellEvent::DictionaryReady(result) => match result {
                Ok(path) => {
                    self.watch_config_dir();
                    self.open_file(path, true, None);
                }
                Err(e) => self.notify(Level::Error, e),
            },
        }
    }
}

/// What a background load found (`None`: that part is unchanged).
pub struct Loaded {
    dict: Option<Arc<Dictionary>>,
    user: Option<UserWords>,
    workspace: Option<UserWords>,
}

/// A finished background check.
pub struct Done {
    buffer: BufferId,
    revision: u64,
    generation: u64,
    regions: Vec<Range<ByteIdx>>,
    full: bool,
    found: Vec<Misspelling>,
}

/// What a check reads: a snapshot.
struct Job {
    checker: Arc<Checker>,
    extra: Arc<HashSet<String>>,
    /// The buffer's "Ignore Word in This File" words.
    ignored: Arc<HashSet<String>>,
    language: Language,
    scope: SpellScope,
    rope: Rope,
    tree: Option<Tree>,
    regions: Vec<Range<ByteIdx>>,
}

/// The misspellings in `job.regions`, in order. Blocking for big regions: call from
/// `spawn_blocking`.
fn scan(job: &Job) -> Vec<Misspelling> {
    let mut found = Vec::new();
    for region in &job.regions {
        let spans = spell_scope::check_ranges_in(job.language, job.tree.as_ref(), &job.rope, job.scope, region.clone());
        if spans.is_empty() {
            continue;
        }
        let base = region.start.0;
        let text = job.rope.slice(base..region.end.0).to_string();
        let extra = [&*job.extra, &*job.ignored];
        let local: Vec<Range<ByteIdx>> = spans
            .into_iter()
            .map(|s| ByteIdx(s.start.0.saturating_sub(base))..ByteIdx(s.end.0.saturating_sub(base)))
            .collect();
        for mut m in job.checker.check(&text, &local, &extra) {
            m.range = ByteIdx(m.range.start.0 + base)..ByteIdx(m.range.end.0 + base);
            found.push(m);
        }
    }
    found
}

/// `ranges` widened to whole lines, merged.
fn line_regions(rope: &Rope, ranges: &[Range<ByteIdx>]) -> Vec<Range<ByteIdx>> {
    let len = rope.len();
    let mut out: Vec<Range<ByteIdx>> = Vec::new();
    for r in ranges {
        let start = pos::line_start(rope, pos::line_of(rope, r.start.min(ByteIdx(len))));
        let end = if r.end.0 >= len {
            ByteIdx(len)
        } else {
            pos::line_bounds(rope, pos::line_of(rope, r.end)).2
        };
        match out.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => out.push(start..end),
        }
    }
    out
}

fn mark_of(m: &Misspelling) -> Mark {
    Mark {
        start: m.range.start,
        end: m.range.end,
        server: ServerId::SPELL,
        diag: Arc::new(Diagnostic {
            // A hint: the weakest severity, so a real error or warning wins the status bar, the
            // squiggle where they overlap and the hover's order; drawn in its own colour.
            severity: Severity::Hint,
            message: format!("Unknown word \"{}\"", m.word),
            source: Some("Spelling".into()),
            code: None,
            raw: serde_json::Value::Null,
        }),
    }
}

impl App {
    /// Each loop turn: loads the dictionaries on first use, notices a settings change, and
    /// starts the checks due (setting the delay's deadline for the others).
    pub(crate) fn sync_spell(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.spell.dict.is_none() {
            if !self.spell.loading {
                self.spell.loading = true;
                self.load_spell(true, true, true);
            }
            return;
        }
        if self
            .spell
            .signature
            .as_ref()
            .is_some_and(|g| g.language != self.settings.spell_check_language)
        {
            // Another dictionary: keep the old one until the new one is read.
            if !self.spell.loading {
                self.spell.loading = true;
                self.load_spell(true, false, false);
            }
        }
        self.sync_spell_config();
        let now = Instant::now();
        let caret = self
            .group()
            .active_tab()
            .map(|t| (t.buffer, t.view.selections.primary().head));
        let ids: Vec<BufferId> = self.buffers.iter().map(|(id, _)| id).collect();
        let mut next: Option<Instant> = None;
        for id in ids {
            if let Some(at) = self.sync_spell_buffer(id, now, caret) {
                next = Some(next.map_or(at, |n| n.min(at)));
            }
        }
        self.spell.deadline = next;
    }

    /// A changed setting makes every buffer start over.
    fn sync_spell_config(&mut self) {
        let s = &self.settings;
        let same = self.spell.signature.as_ref().is_some_and(|g| {
            g.scope == s.spell_check_scope
                && g.min_length == s.spell_check_min_length
                && g.language == s.spell_check_language
                && g.max_bytes == s.syntax_max_bytes
                && g.ignore_words == s.spell_check_ignore_words
                && g.off_languages == s.spell_check_off_languages
        });
        if same {
            return;
        }
        self.spell.signature = Some(Signature {
            scope: s.spell_check_scope,
            min_length: s.spell_check_min_length,
            language: s.spell_check_language.clone(),
            ignore_words: s.spell_check_ignore_words.clone(),
            off_languages: s.spell_check_off_languages.clone(),
            max_bytes: s.syntax_max_bytes,
        });
        self.spell_words_changed();
    }

    /// Dictionaries or settings changed: rebuild the accepted words and check everything again.
    fn spell_words_changed(&mut self) {
        let ignored = UserWords::from_words(&self.settings.spell_check_ignore_words);
        let all = self.spell.user.merged(&self.spell.workspace).merged(&ignored);
        self.spell.extra = Arc::new(all.extra_set().clone());
        self.spell.checkers.clear();
        self.spell.generation += 1;
    }

    /// The spell state of `id` for this turn: returns when it next needs a look.
    fn sync_spell_buffer(&mut self, id: BufferId, now: Instant, caret: Option<(BufferId, ByteIdx)>) -> Option<Instant> {
        let generation = self.spell.generation;
        let problems = self.settings.spell_check_problems;
        let (scope, off, max) = (
            self.settings.spell_check_scope,
            &self.settings.spell_check_off_languages,
            self.settings.syntax_max_bytes,
        );
        let buf = self.buffers.get_mut(id)?;
        buf.spell.listed = problems;
        let language = buf.syntax.language();
        let applicable = buf.log.is_none()
            && buf.commit.is_none()
            && buf.spell.enabled
            && spell_scope::enabled(scope, language, off, buf.rope.len(), max);
        buf.spell.off = !applicable;
        if !applicable {
            if !buf.spell.marks.is_empty() {
                buf.spell.marks.clear_all();
                self.needs_redraw = true;
            }
            buf.spell.want_full();
            buf.spell.generation = generation;
            return None;
        }
        if buf.spell.generation != generation {
            buf.spell.want_full();
            buf.spell.generation = generation;
        }
        // A held word whose caret left it is checked again.
        if !buf.spell.held.is_empty() {
            let word = caret
                .filter(|&(b, _)| b == id)
                .and_then(|(_, head)| pos::word_at(&buf.rope, head));
            let held = std::mem::take(&mut buf.spell.held);
            let (stay, free): (Vec<_>, Vec<_>) = held
                .into_iter()
                .partition(|h| word.is_some_and(|(ws, we)| h.start <= we && h.end >= ws));
            buf.spell.held = stay;
            for r in free {
                buf.spell.touch(r);
            }
        }
        if buf.spell.running.is_some() || !buf.spell.wants() {
            return None;
        }
        // Grammar languages need a current tree (the parse's result wakes the loop).
        if language.grammar().is_some() && buf.syntax.tree().is_none() {
            return None;
        }
        if !buf.spell.full
            && let Some(at) = buf.spell.edited_at
            && at + DELAY > now
        {
            return Some(at + DELAY);
        }
        let revision = buf.revision();
        let full = std::mem::take(&mut buf.spell.full);
        let regions = if full {
            buf.spell.take_dirty();
            vec![ByteIdx(0)..ByteIdx(buf.rope.len())]
        } else {
            let dirty = buf.spell.take_dirty();
            line_regions(&buf.rope, &dirty)
        };
        let bytes: usize = regions.iter().map(|r| r.end.0 - r.start.0).sum();
        let rope = buf.rope.clone();
        let tree = buf.syntax.tree().cloned();
        let ignored = buf.spell.ignored.clone();
        let checker = self.spell_checker(language)?;
        let job = Job {
            checker,
            extra: self.spell.extra.clone(),
            ignored,
            language,
            scope,
            rope,
            tree,
            regions,
        };
        if bytes <= INLINE_MAX {
            #[cfg(test)]
            {
                self.spell.scanned += bytes;
            }
            let found = scan(&job);
            self.spell_apply(id, &job.regions, full, found);
        } else {
            #[cfg(test)]
            {
                self.spell.background += 1;
            }
            if let Some(buf) = self.buffers.get_mut(id) {
                buf.spell.running = Some(revision);
            }
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let t = Instant::now();
                let found = scan(&job);
                if t.elapsed() > SLOW_TASK {
                    tracing::warn!(elapsed = ?t.elapsed(), bytes, "slow spell check");
                }
                let _ = tx.send(AppEvent::Spell(SpellEvent::Done(Box::new(Done {
                    buffer: id,
                    revision,
                    generation,
                    regions: job.regions,
                    full,
                    found,
                }))));
            });
        }
        None
    }

    /// `SpellEvent::Done`: a background check's misspellings. Dropped when the text or the
    /// settings moved on since (the buffer is checked again).
    pub(super) fn spell_done(&mut self, done: Done) {
        let generation = self.spell.generation;
        let Some(buf) = self.buffers.get_mut(done.buffer) else {
            return;
        };
        buf.spell.running = None;
        if buf.revision() != done.revision || done.generation != generation {
            tracing::debug!(target: "jed::spell", "dropping a stale spell check");
            buf.spell.want_full();
            return;
        }
        self.spell_apply(done.buffer, &done.regions, done.full, done.found);
    }

    /// Puts `found` (the misspellings of `regions`) into the buffer's marks, holding back the
    /// word the caret is in after an edit-driven check.
    fn spell_apply(&mut self, id: BufferId, regions: &[Range<ByteIdx>], full: bool, found: Vec<Misspelling>) {
        let caret = self
            .group()
            .active_tab()
            .filter(|t| t.buffer == id)
            .map(|t| t.view.selections.primary().head);
        let Some(buf) = self.buffers.get_mut(id) else {
            return;
        };
        let typing = if full {
            None
        } else {
            caret.and_then(|head| pos::word_at(&buf.rope, head))
        };
        let mut marks = Vec::with_capacity(found.len());
        for m in &found {
            if let Some((ws, we)) = typing
                && m.range.start <= we
                && m.range.end >= ws
            {
                buf.spell.held.push(m.range.clone());
                continue;
            }
            marks.push(mark_of(m));
        }
        if full {
            buf.spell.marks.replace(ServerId::SPELL, marks);
        } else {
            buf.spell.marks.splice(ServerId::SPELL, regions, marks);
        }
        self.needs_redraw = true;
    }

    /// The checker for `language` (built from the loaded dictionary), `None` before it loads.
    pub(super) fn spell_checker(&mut self, language: Language) -> Option<Arc<Checker>> {
        let dict = self.spell.dict.clone()?;
        let min = self.settings.spell_check_min_length;
        Some(
            self.spell
                .checkers
                .entry(language)
                .or_insert_with(|| Arc::new(Checker::new(dict, language, min)))
                .clone(),
        )
    }

    /// Reads the dictionary (`dict`) and the user and workspace word files (`user`,
    /// `workspace`) in the background; each finished read is `SpellEvent::Loaded`.
    pub(super) fn load_spell(&mut self, dict: bool, user: bool, workspace: bool) {
        let dir = self.config_dir.clone();
        let root = self.workspace.root.clone();
        let language = self.settings.spell_check_language.clone();
        let data_dir = self.data_dir.clone().filter(|_| dict);
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(data) = data_dir {
                match crate::assets::sync(&data) {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!(target: "jed::spell", n, dir = %data.display(), "wrote the bundled dictionary")
                    }
                    Err(e) => {
                        tracing::warn!(target: "jed::spell", dir = %data.display(), "writing the bundled dictionary: {e}")
                    }
                }
            }
            let loaded = Loaded {
                dict: dict.then(|| Dictionary::load(&dir.join("dictionaries"), &language)),
                user: user.then(|| spell_dict::load(&spell_dict::user_path(&dir))),
                workspace: workspace.then(|| spell_dict::load(&spell_dict::workspace_path(&root))),
            };
            let _ = tx.send(AppEvent::Spell(SpellEvent::Loaded(Box::new(loaded))));
        });
    }

    /// `SpellEvent::Loaded`.
    pub(super) fn spell_loaded(&mut self, loaded: Loaded) {
        if let Some(dict) = loaded.dict {
            if let Some(lang) = dict.missing_language()
                && self.spell.told_missing.as_deref() != Some(lang)
            {
                self.spell.told_missing = Some(lang.to_string());
                // Not in tests: the message would land on every screen a test compares.
                if !cfg!(test) {
                    self.notify(
                        Level::Warning,
                        format!(
                            "No spell-check dictionary for {lang}: using en_AU. Install it with scripts/fetch-dictionary (docs/spell-check.md)"
                        ),
                    );
                }
            }
            self.spell.dict = Some(dict);
            self.spell.loading = false;
        }
        if let Some(user) = loaded.user {
            self.spell.user = user;
        }
        if let Some(workspace) = loaded.workspace {
            self.spell.workspace = workspace;
        }
        self.spell_words_changed();
    }

    /// A dictionary file changed on disk (`flush_fs_changes`): read it again.
    pub(super) fn spell_dict_changed(&mut self, which: Which) {
        if self.spell.dict.is_none() {
            return;
        }
        self.load_spell(false, which == Which::User, which == Which::Workspace);
    }

    /// Tests: every buffer is checked as the current settings and dictionaries say (or is off).
    #[cfg(test)]
    pub fn test_spell_idle(&self) -> bool {
        self.spell.dict.is_some()
            && self.spell.signature.is_some()
            && self.buffers.iter().all(|(_, b)| {
                b.spell.running.is_none()
                    && b.spell.generation == self.spell.generation
                    && (b.spell.off || !b.spell.wants())
            })
    }

    /// Tests: the file watcher saw `paths` change.
    #[cfg(test)]
    pub fn test_fs_changed(&mut self, paths: Vec<std::path::PathBuf>) {
        self.fs_changed(paths);
    }

    /// Tests: handles the next app event, waiting for it (nothing else runs).
    #[cfg(test)]
    pub async fn test_handle_one(&mut self) {
        if let Some(ev) = self.rx.recv().await {
            self.handle_app_event(ev);
        }
    }
}
