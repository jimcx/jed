//! The find / replace widget (`find::FindWidget`, drawn by `ui::find`): opening and closing it,
//! keys and paste in its inputs, incremental search, next / previous, the option toggles (which
//! Cmd+D borrows), replace one / all and select all matches. It searches the active tab: on a
//! hex tab its bytes (`find::HexFind`): the `0x` toggle (`find_options.hex`, converting the
//! query), case only, no replace or select all, next / previous from the cursor, highlights drawn
//! from the visible bytes, and the whole file's count off the UI thread (`AppEvent::HexCount`).

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use super::{App, AppEvent, Focus, Level};
use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::undo::EditKind;
use crate::command::FindCommand as F;
use crate::field::{Outcome, TextField};
use crate::find::{Field, FindWidget};
use crate::hex::{self, Mode};
use crate::search::{Match, Replacement};
use crate::selection::{Selection, Selections};
use crate::view::View;

impl App {
    /// Cmd+F / Cmd+Alt+F: shows the widget and focuses it. The query is seeded from the primary
    /// selection when it's on one line, else from the word under an empty cursor (else the last
    /// query stays), escaped when regex is on, and is selected, so typing replaces it. A seeded
    /// query selects its match.
    pub(super) fn open_find(&mut self, replace: bool) {
        if self.active_hex().is_some() {
            return self.open_hex_find();
        }
        let regex = self.find_options.regex;
        let Some((buf, view)) = self.active_mut() else {
            return;
        };
        let rope = &buf.rope;
        let sel = *view.selections.primary();
        let seed = if sel.is_empty() {
            pos::word_at(rope, sel.head)
        } else if pos::line_of(rope, sel.start()) == pos::line_of(rope, sel.end()) {
            Some((sel.start(), sel.end()))
        } else {
            None
        };
        // With regex on, the seed is escaped so it finds itself (VS Code).
        let seed = seed.map(|(s, e)| {
            let text = rope.slice(s.0..e.0).to_string();
            (s, if regex { regex::escape(&text) } else { text })
        });
        let f = &mut self.find;
        f.visible = true;
        f.replace_visible = replace;
        if let Some((_, text)) = &seed {
            f.query = TextField::new(text.clone());
        }
        f.query.select_all();
        f.field = if replace && !f.query.text.is_empty() {
            f.replace.select_all();
            Field::Replace
        } else {
            Field::Find
        };
        self.ui.focus = Focus::Find;
        self.refresh_find();
        if let Some((start, _)) = seed {
            self.find_from(start);
        }
    }

    pub(super) fn close_find(&mut self) {
        self.find.visible = false;
        if self.ui.focus == Focus::Find {
            self.ui.focus = Focus::Editor;
        }
    }

    pub(super) fn find_command(&mut self, cmd: F) {
        // Bytes have no replace and no select all (and Tab has no replace input to go to).
        if self.active_hex().is_some()
            && matches!(
                cmd,
                F::SelectAllMatches | F::ReplaceOne | F::ReplaceAll | F::SwitchField
            )
        {
            return;
        }
        match cmd {
            F::Next | F::Previous => {
                if !self.find.visible && self.find.query.text.is_empty() {
                    return self.open_find(false);
                }
                // F3 from the editor shows the widget without taking focus (VS Code).
                self.find.visible = true;
                self.refresh_find();
                self.find_step(cmd == F::Next);
            }
            F::ToggleCase | F::ToggleWholeWord | F::ToggleRegex => self.toggle_find_option(cmd),
            F::ToggleHex => self.toggle_find_hex(),
            F::SelectAllMatches => self.select_all_find_matches(),
            F::ReplaceOne | F::ReplaceAll if !self.find.visible => self.open_find(true),
            // With the replace input hidden, the first press shows it.
            F::ReplaceOne | F::ReplaceAll if !self.find.replace_visible => {
                self.find.replace_visible = true;
                self.find.field = Field::Replace;
                self.ui.focus = Focus::Find;
            }
            F::ReplaceOne => self.replace_one(),
            F::ReplaceAll => self.replace_all(),
            F::SwitchField => {
                let f = &mut self.find;
                if f.visible && f.replace_visible {
                    f.field = match f.field {
                        Field::Find => Field::Replace,
                        Field::Replace => Field::Find,
                    };
                    self.ui.focus = Focus::Find;
                }
            }
            F::Close => self.close_find(),
        }
    }

    /// Keys the keymap doesn't bind in the widget edit the focused input.
    pub(super) fn find_key(&mut self, key: KeyEvent) {
        let mut field = std::mem::take(self.find.field_mut());
        let outcome = self.field_key(&mut field, Some(self.context()), key);
        *self.find.field_mut() = field;
        if outcome == Outcome::Edited {
            self.find_edited();
        }
    }

    pub(super) fn find_paste(&mut self, text: &str) {
        self.find.field_mut().insert(&text.replace(['\r', '\n'], ""));
        self.find_edited();
    }

    fn find_edited(&mut self) {
        if self.find.field == Field::Find {
            self.research();
        }
    }

    /// The query or an option changed: search again, and move to the first match from the
    /// selection's start (incremental search, as VS Code).
    fn research(&mut self) {
        self.refresh_find();
        if let Some(h) = self.active_hex() {
            let from = h.selection().map_or(h.cursor, |s| s.start);
            return self.hex_find_from(from, true);
        }
        if let Some(view) = self.group().active_tab().map(|t| &t.view) {
            let from = view.selections.primary().start();
            self.find_from(from);
        }
    }

    /// Keeps the widget's matches current for the active tab (one `find_all` per change of
    /// buffer, text, query or options). Runs after every event; closes the widget without a tab.
    pub(super) fn refresh_find(&mut self) {
        if !self.find.visible {
            return;
        }
        if self.active_hex().is_some() {
            return self.refresh_hex_find();
        }
        let Some(tab) = self.groups.active_group().active_tab() else {
            self.close_find();
            return;
        };
        if let Some(buf) = self.buffers.get(tab.buffer) {
            self.find
                .refresh(tab.buffer, buf.revision(), &buf.rope, self.find_options);
        }
    }

    /// The widget, and the active tab's buffer and view, borrowed together.
    fn find_target(&mut self) -> Option<(&FindWidget, &mut Buffer, &mut View)> {
        let group = self.groups.active_group_mut();
        let tab = group.active_tab_mut()?;
        let buf = self.buffers.get_mut(tab.buffer)?;
        Some((&self.find, buf, &mut tab.view))
    }

    /// Selects the first match at or after `from`, wrapping; an empty match at `from` counts
    /// (VS Code: only next / previous force a move). The cache is current here.
    fn find_from(&mut self, from: ByteIdx) {
        let Some((find, buf, view)) = self.find_target() else {
            return;
        };
        let all = find.matches();
        let i = all.partition_point(|m| m.start < from);
        if let Some(&m) = all.get(i).or(all.first()) {
            select_match(buf, view, m);
        }
    }

    /// Enter / Shift+Enter, F3 / Shift+F3: the next match after the primary selection, or the
    /// previous one before it, wrapping.
    fn find_step(&mut self, forward: bool) {
        if let Some(h) = self.active_hex() {
            // Overlapping matches are each a step: forward from the byte after the selection's
            // start (or from the cursor's byte without a selection).
            return match (forward, h.selection()) {
                (true, Some(s)) => self.hex_find_from(s.start + 1, true),
                (true, None) => self.hex_find_from(h.cursor, true),
                (false, s) => self.hex_find_from(s.map_or(h.cursor, |s| s.start), false),
            };
        }
        let Some((find, buf, view)) = self.find_target() else {
            return;
        };
        let Some(matcher) = find.matcher() else { return };
        let sel = view.selections.primary();
        let found = if forward {
            matcher.find_next(&buf.rope, sel.end(), true)
        } else {
            matcher.find_prev(&buf.rope, sel.start(), true)
        };
        if let Some(m) = found {
            select_match(buf, view, m);
        }
    }

    fn toggle_find_option(&mut self, cmd: F) {
        // Whole word and regex don't apply to bytes (drawn disabled).
        if self.active_hex().is_some() && cmd != F::ToggleCase {
            return;
        }
        let o = &mut self.find_options;
        let (on, name) = match cmd {
            F::ToggleCase => (flip(&mut o.case_sensitive), "Match Case"),
            F::ToggleWholeWord => (flip(&mut o.whole_word), "Match Whole Word"),
            _ => (flip(&mut o.regex), "Use Regular Expression"),
        };
        self.find_options_changed();
        if self.find.visible {
            self.research();
        } else {
            let state = if on { "on" } else { "off" };
            self.notify(Level::Info, format!("Find: {name} {state}"));
        }
    }

    /// Alt+Enter: a selection at every match; the one at the selected match is primary.
    /// Focus goes back to the editor.
    fn select_all_find_matches(&mut self) {
        if !self.find.visible {
            return;
        }
        self.refresh_find();
        let Some((find, buf, view)) = self.find_target() else {
            return;
        };
        let matches = find.matches();
        if matches.is_empty() {
            return;
        }
        let sel = view.selections.primary();
        let primary = find.index_of(sel.start(), sel.end()).unwrap_or(0);
        let list: Vec<Selection> = matches.iter().map(|m| Selection::new(m.start, m.end)).collect();
        buf.seal_undo();
        view.selections = Selections::with_newest(list, primary, primary);
        view.reveal_cursor = true;
        self.ui.focus = Focus::Editor;
    }

    /// Replaces the selected match (if the primary selection is one) and goes to the next match.
    fn replace_one(&mut self) {
        if !self.active_writable() {
            return;
        }
        self.refresh_find();
        let regex = self.find_options.regex;
        let Some((find, buf, view)) = self.find_target() else {
            return;
        };
        let sel = *view.selections.primary();
        let current = find.index_of(sel.start(), sel.end()).map(|i| find.matches()[i]);
        if let Some(m) = current
            && let Some(text) = find.replacement_for(&buf.rope, m, regex)
        {
            let mut changes = ChangeSet::with_capacity(1, text.len());
            changes.push(m.start, m.end, &text);
            let after = Selections::single(Selection::cursor(ByteIdx(m.start.0 + text.len())));
            buf.edit(changes, view.selections.clone(), after.clone(), EditKind::Other);
            view.selections = after;
            self.refresh_find();
        }
        self.find_step(true);
    }

    /// Replaces every match as one undo step; selections follow the edit.
    fn replace_all(&mut self) {
        if !self.active_writable() {
            return;
        }
        self.refresh_find();
        let regex = self.find_options.regex;
        let Some((find, buf, view)) = self.find_target() else {
            return;
        };
        let Some(matcher) = find.matcher() else { return };
        let matches = find.matches();
        if matches.is_empty() {
            return;
        }
        let count = matches.len();
        let with = Replacement::new(&find.replace.text, regex);
        let mut changes;
        match with.literal() {
            // The cached matches are current: no second search.
            Some(text) => {
                changes = ChangeSet::with_capacity(count, count * text.len());
                for m in matches {
                    changes.push(m.start, m.end, text);
                }
            }
            None => {
                changes = ChangeSet::with_capacity(count, 0);
                let all = ByteIdx(0)..ByteIdx(buf.rope.len());
                matcher.replace_in(&buf.rope, all, &with, |m: Match, text| {
                    changes.push(m.start, m.end, text)
                });
            }
        }
        let before = view.selections.clone();
        let after = {
            let mapper = changes.mapper();
            let list = before
                .iter()
                .map(|s| Selection::new(mapper.map(s.anchor, Assoc::Before), mapper.map(s.head, Assoc::Before)))
                .collect();
            Selections::with_newest(list, before.primary_index(), before.newest_index())
        };
        buf.edit(changes, before, after.clone(), EditKind::Other);
        view.selections = after;
        view.reveal_cursor = true;
        let noun = if count == 1 { "match" } else { "matches" };
        self.notify(Level::Info, format!("Replaced {count} {noun}"));
    }

    /// A click on the widget: an option toggle, or an input (which takes the focus).
    /// False for anything else (wheel and drags pass through to what's under it).
    pub(super) fn find_mouse(&mut self, m: MouseEvent) -> bool {
        let Some(hit) = self.hits.find else { return false };
        let inside = |r: ratatui::layout::Rect| {
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
        };
        if !self.find.visible || m.kind != MouseEventKind::Down(MouseButton::Left) || !inside(hit.area) {
            return false;
        }
        let toggles = [F::ToggleCase, F::ToggleWholeWord, F::ToggleRegex, F::ToggleHex];
        if let Some(i) = hit.toggles.iter().position(|r| inside(*r)) {
            match toggles[i] {
                F::ToggleHex => self.toggle_find_hex(),
                cmd => self.toggle_find_option(cmd),
            }
        } else {
            self.find.field = match hit.replace {
                Some(r) if m.row == r.y => Field::Replace,
                _ => Field::Find,
            };
            self.ui.focus = Focus::Find;
        }
        true
    }
}

/// Most bytes a selection seeds the hex find query with (a bigger one leaves the query as is).
const HEX_SEED_MAX: usize = 256;

impl App {
    /// Cmd+F (or Cmd+Alt+F: no replace here) on a hex tab: the widget, focused, its query seeded
    /// from the selected bytes in the current mode — hex pairs, or the text when every byte is
    /// printable — else the last query stays; selected, so typing replaces it.
    fn open_hex_find(&mut self) {
        let mode = mode(self.find_options.hex);
        let seed = self.active_hex().and_then(|h| {
            let range = h.selection().filter(|r| r.len() <= HEX_SEED_MAX)?;
            let bytes = &h.doc.bytes()[range];
            match mode {
                Mode::Hex => Some(hex::hex_pairs(bytes)),
                Mode::Ascii => hex::printable_text(bytes),
            }
        });
        let f = &mut self.find;
        f.visible = true;
        if let Some(text) = seed {
            f.query = TextField::new(text);
        }
        f.query.select_all();
        f.field = Field::Find;
        self.ui.focus = Focus::Find;
        self.refresh_find();
    }

    /// Keeps the hex search current for the active hex tab: the query read again when it, the
    /// options or the tab changed, and then its count started off the UI thread (a whole-file
    /// scan, `hex::count` up to `HEX_MAX_COUNT`) → `AppEvent::HexCount` → `hex_counted`.
    fn refresh_hex_find(&mut self) {
        // No replace input on a hex tab.
        self.find.field = Field::Find;
        let Some(h) = self.groups.active_group().active_hex() else {
            return;
        };
        let (tab, doc) = (h.id, h.doc.clone());
        let Some(job) = self
            .find
            .hex
            .refresh(tab, doc.bytes(), &self.find.query.text, self.find_options)
        else {
            return;
        };
        // Outside a runtime (unit tests) the count stays "…".
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let tx = self.tx.clone();
        rt.spawn_blocking(move || {
            let t = std::time::Instant::now();
            let count = hex::count(doc.bytes(), &job.query, hex::HEX_MAX_COUNT);
            tracing::debug!(
                found = count.found(),
                capped = count.capped,
                elapsed_us = t.elapsed().as_micros() as u64,
                "hex count"
            );
            let _ = tx.send(AppEvent::HexCount {
                tab,
                generation: job.generation,
                count,
            });
        });
    }

    /// A hex count arrived: kept if it's for the current query and tab, else dropped (stale).
    pub(super) fn hex_counted(&mut self, tab: u64, generation: u64, count: hex::Count) {
        if !self.find.hex.counted(tab, generation, count) {
            tracing::debug!(tab, generation, "stale hex count dropped");
        }
    }

    /// Selects the hex query's next match starting at or after `from` (`forward`), else the
    /// previous one starting before it; both wrap. Searches straight from there (memmem: it
    /// stops at the first match).
    fn hex_find_from(&mut self, from: usize, forward: bool) {
        let Some(q) = self.find.hex.query() else { return };
        let Some(h) = self.groups.active_group_mut().active_hex_mut() else {
            return;
        };
        let bytes = h.doc.bytes();
        let found = if forward {
            hex::find_next(bytes, q, from)
        } else {
            hex::find_prev(bytes, q, from)
        };
        if let Some(m) = found {
            h.select(m);
        }
    }

    /// "Find: Toggle Hex" (Cmd+Alt+X / Alt+X, or a click on `0x`): on a hex tab, the byte search
    /// between ASCII text and hex pairs, kept for the session; a valid query is converted (ASCII
    /// → its bytes as hex pairs, hex → the text when every byte is printable), else re-read as
    /// it is. Does nothing on a text tab (it has no `0x`).
    fn toggle_find_hex(&mut self) {
        if self.active_hex().is_none() {
            tracing::debug!("find.toggleHex: not a hex tab");
            return;
        }
        let on = flip(&mut self.find_options.hex);
        if let Some(text) = hex::convert(&self.find.query.text, mode(on)) {
            let all = self.find.query.selection() == Some(0..self.find.query.text.len());
            self.find.query = TextField::new(text);
            if all {
                self.find.query.select_all();
            }
        }
        if self.find.visible {
            self.research();
        } else {
            let state = if on { "hex" } else { "ASCII" };
            self.notify(Level::Info, format!("Find: {state} bytes"));
        }
    }
}

fn mode(hex: bool) -> Mode {
    if hex { Mode::Hex } else { Mode::Ascii }
}

fn flip(b: &mut bool) -> bool {
    *b = !*b;
    *b
}

/// Selects `m` alone and scrolls it to the middle of the view if it's off screen.
fn select_match(buf: &mut Buffer, view: &mut View, m: Match) {
    buf.seal_undo();
    view.selections = Selections::single(Selection::new(m.start, m.end));
    view.reveal_range = Some((m.start, m.end));
}
