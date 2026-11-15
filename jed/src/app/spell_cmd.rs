//! The spell checker's commands (`Command::Spell`, palette only) and quick fixes: Go to Next /
//! Previous Problem (the caret to a misspelling's start, wrapping), Add Word to the User /
//! Workspace Dictionary (one rewrite in `spawn_blocking`, then the files are read again so the marks
//! go in every open buffer), Ignore Word in This File (`buf.spell.ignored`, until the buffer
//! closes), Toggle Spell Check (this buffer) and Open User Dictionary (the file is created when
//! missing). Quick fixes on a spelling mark (Cmd+. and the lightbulb) are built here
//! (`spell_actions`) and applied by `apply_spell_fix`.
//!
//! The word is the spelling mark each selection touches, else (an empty selection) the word at the
//! caret; the add / ignore commands take the distinct words of the first `MAX_SELECTIONS`
//! selections, primary first. A replace fix replaces the primary's piece and every other selection's
//! piece that is the same word, as one `EditKind::Other` step. All of them refuse (a notice) in a
//! tab that is not an editable text editor, except Open User Dictionary.

use std::collections::HashSet;

use super::{App, AppEvent, Level};
use crate::buffer::BufferId;
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx};
use crate::code_action::{Action, SpellFix};
use crate::command::SpellCommand;
use crate::selection::{Selection, Selections};
use crate::spell_dict;

use super::spell::{Added, SpellEvent};

/// Selections looked at for the add / ignore commands (60k selections must not write 60k words).
const MAX_SELECTIONS: usize = 256;

impl App {
    pub(super) fn spell_command(&mut self, c: SpellCommand) {
        if c == SpellCommand::OpenUserDictionary {
            let path = spell_dict::user_path(&self.config_dir);
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = spell_dict::ensure_file(&path)
                    .map(|()| path)
                    .map_err(|e| format!("Could not create the user dictionary: {e}"));
                let _ = tx.send(AppEvent::Spell(SpellEvent::DictionaryReady(result)));
            });
            return;
        }
        match self.active_buffer().map(|b| b.read_only.clone()) {
            Some(None) => {}
            Some(Some(reason)) => {
                self.notify(Level::Error, format!("Read-only: {reason}"));
                return;
            }
            None => {
                self.notify(Level::Info, "Spelling commands need a text editor".into());
                return;
            }
        }
        match c {
            SpellCommand::Next => self.spell_goto(true),
            SpellCommand::Previous => self.spell_goto(false),
            SpellCommand::AddToUser => self.add_spell_words(true),
            SpellCommand::AddToWorkspace => self.add_spell_words(false),
            SpellCommand::Ignore => {
                let words = self.spell_words();
                let Some(tab) = self.group().active_tab() else { return };
                self.ignore_spell_words(tab.buffer, words);
            }
            SpellCommand::Toggle => {
                if let Some((buf, _)) = self.active_mut() {
                    buf.spell.enabled = !buf.spell.enabled;
                    let on = buf.spell.enabled;
                    self.needs_redraw = true;
                    let state = if on { "on" } else { "off" };
                    self.notify(Level::Info, format!("Spell check {state} for this file"));
                }
            }
            SpellCommand::OpenUserDictionary => {}
        }
    }

    /// The caret to the next / previous misspelling's start, wrapping.
    fn spell_goto(&mut self, forward: bool) {
        let Some((buf, view)) = self.active_mut() else { return };
        let head = view.selections.primary().head;
        let marks = buf.spell.marks.all();
        // Sorted by start: binary search, not a scan of every mark.
        let target = if forward {
            marks.get(marks.partition_point(|m| m.start <= head)).or(marks.first())
        } else {
            marks
                .partition_point(|m| m.start < head)
                .checked_sub(1)
                .and_then(|i| marks.get(i))
                .or(marks.last())
        }
        .map(|m| m.start);
        let Some(at) = target else {
            self.notify(Level::Info, "No misspelled words".into());
            return;
        };
        buf.seal_undo();
        view.selections = Selections::single(Selection::cursor(at));
        view.reveal_range = Some((at, at));
        self.needs_redraw = true;
    }

    /// The distinct lower-case words under the active editor's selections (primary first).
    fn spell_words(&self) -> Vec<String> {
        let Some(tab) = self.group().active_tab() else {
            return Vec::new();
        };
        let Some(buf) = self.buffers.get(tab.buffer) else {
            return Vec::new();
        };
        let sels = &tab.view.selections;
        let mut seen = HashSet::new();
        let mut words = Vec::new();
        for s in std::iter::once(sels.primary()).chain(sels.iter().take(MAX_SELECTIONS)) {
            let range = buf
                .spell
                .marks
                .overlapping(s.start(), s.end())
                .next()
                .map(|m| (m.start, m.end))
                .or_else(|| {
                    if s.is_empty() {
                        pos::word_at(&buf.rope, s.head)
                    } else {
                        None
                    }
                });
            let Some((a, b)) = range else { continue };
            let word = buf.rope.slice(a.0..b.0).to_string().to_lowercase();
            if seen.insert(word.clone()) {
                words.push(word);
            }
        }
        words
    }

    /// Adds the words under the selections to the user or workspace dictionary file.
    fn add_spell_words(&mut self, user: bool) {
        let words = self.spell_words();
        self.write_spell_words(user, words);
    }

    fn write_spell_words(&mut self, user: bool, words: Vec<String>) {
        if words.is_empty() {
            self.notify(Level::Info, "No word at the cursor".into());
            return;
        }
        let path = if user {
            spell_dict::user_path(&self.config_dir)
        } else {
            spell_dict::workspace_path(&self.workspace.root)
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            // One rewrite of the file for all the words (up to `MAX_SELECTIONS` of them).
            let (added, error) = match spell_dict::add_words(&path, &words) {
                Ok(added) => (added, None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            };
            let known = words.len() - added.len();
            let done = Added {
                user,
                added,
                known,
                error,
            };
            let _ = tx.send(AppEvent::Spell(SpellEvent::Added(Box::new(done))));
        });
    }

    /// `SpellEvent::Added`: says what was written and reads the word file again (every open
    /// buffer is then checked under the new words).
    pub(super) fn spell_words_added(&mut self, done: Added) {
        let which = if done.user { "user" } else { "workspace" };
        if let Some(e) = &done.error {
            self.notify(Level::Error, format!("Could not add to the {which} dictionary: {e}"));
        } else if let [word] = done.added.as_slice() {
            self.notify(Level::Info, format!("Added \"{word}\" to the {which} dictionary"));
        } else if done.added.is_empty() {
            self.notify(
                Level::Info,
                format!("Already in the {which} dictionary ({})", done.known),
            );
        } else {
            let n = done.added.len();
            self.notify(Level::Info, format!("Added {n} words to the {which} dictionary"));
        }
        if !done.added.is_empty() {
            self.spell_dict_changed(if done.user {
                spell_dict::Which::User
            } else {
                spell_dict::Which::Workspace
            });
        }
    }

    fn ignore_spell_words(&mut self, buffer: BufferId, words: Vec<String>) {
        if words.is_empty() {
            self.notify(Level::Info, "No word at the cursor".into());
            return;
        }
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        let text = match words.as_slice() {
            [w] => format!("Ignoring \"{w}\" in this file"),
            w => format!("Ignoring {} words in this file", w.len()),
        };
        std::sync::Arc::make_mut(&mut buf.spell.ignored).extend(words);
        buf.spell.want_full();
        self.needs_redraw = true;
        self.notify(Level::Info, text);
    }

    /// The spell checker's quick fixes for the misspelling at `range` (the primary selection; an
    /// empty one is the caret) of `buffer`: the suggestions, then add to the user / workspace
    /// dictionary and ignore. Empty when no spelling mark is there (or the buffer is read-only).
    /// Suggestions scan the dictionary: asked for one word, on demand.
    pub(super) fn spell_actions(&mut self, buffer: BufferId, range: (ByteIdx, ByteIdx)) -> Vec<Action> {
        let Some(buf) = self.buffers.get(buffer).filter(|b| b.read_only.is_none()) else {
            return Vec::new();
        };
        let Some(first) = buf.spell.marks.overlapping(range.0, range.1).next() else {
            return Vec::new();
        };
        let (start, end) = (first.start, first.end);
        let word = buf.rope.slice(start.0..end.0).to_string();
        let language = buf.syntax.language();
        // The piece of every selection that is this same word.
        let mut ranges = vec![(start, end)];
        let shown = self.groups.get(self.groups.active).and_then(|g| g.active_tab());
        if let Some(tab) = shown.filter(|t| t.buffer == buffer) {
            ranges.clear();
            for s in tab.view.selections.iter() {
                for m in buf.spell.marks.overlapping(s.start(), s.end()) {
                    let fresh = ranges.last().is_none_or(|&(_, e)| e <= m.start);
                    if fresh && buf.rope.slice(m.start.0..m.end.0) == word.as_str() {
                        ranges.push((m.start, m.end));
                    }
                }
            }
            if ranges.is_empty() {
                ranges.push((start, end));
            }
        }
        let suggestions = self
            .spell_checker(language)
            .map(|c| c.suggest(&word))
            .unwrap_or_default();
        let lower = word.to_lowercase();
        let mut actions: Vec<Action> = suggestions
            .into_iter()
            .map(|s| {
                let fix = SpellFix::Replace {
                    ranges: ranges.clone(),
                    text: s.clone(),
                };
                Action::spell(format!("Replace with \"{s}\""), fix)
            })
            .collect();
        actions.push(Action::spell(
            format!("Add \"{lower}\" to user dictionary"),
            SpellFix::AddToUser(lower.clone()),
        ));
        actions.push(Action::spell(
            format!("Add \"{lower}\" to workspace dictionary"),
            SpellFix::AddToWorkspace(lower.clone()),
        ));
        actions.push(Action::spell("Ignore in this file".into(), SpellFix::Ignore(lower)));
        actions
    }

    /// Whether a spelling mark touches the primary caret of the active editor (the lightbulb).
    pub(super) fn spell_mark_at_caret(&self) -> bool {
        let Some(tab) = self.group().active_tab() else {
            return false;
        };
        let head = tab.view.selections.primary().head;
        self.buffers
            .get(tab.buffer)
            .is_some_and(|b| b.read_only.is_none() && b.spell.marks.overlapping(head, head).next().is_some())
    }

    /// Applies a spell quick fix chosen in the list of `buffer`.
    pub(super) fn apply_spell_fix(&mut self, buffer: BufferId, fix: SpellFix) {
        match fix {
            SpellFix::Replace { ranges, text } => {
                if self.buffers.get(buffer).is_none_or(|b| b.read_only.is_some()) {
                    return;
                }
                let mut changes = ChangeSet::with_capacity(ranges.len(), text.len() * ranges.len());
                for (s, e) in ranges {
                    changes.push(s, e, &text);
                }
                self.apply_changes(buffer, changes);
                self.follow_edits();
            }
            SpellFix::AddToUser(w) => self.write_spell_words(true, vec![w]),
            SpellFix::AddToWorkspace(w) => self.write_spell_words(false, vec![w]),
            SpellFix::Ignore(w) => self.ignore_spell_words(buffer, vec![w]),
        }
    }
}
