//! The keyboard shortcuts view's model ("Preferences: Keyboard Shortcuts"): every binding with
//! its command's title, context and source, plus a row for each command no key is bound to, with
//! a fuzzy search over the titles, names and key text. `app/keys.rs` opens and drives it,
//! `ui/keys.rs` draws it.

use std::collections::HashSet;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

use crate::command::Command;
use crate::config::keymap::{Keymap, Source};
use crate::scrollbar::{Scroll, Scrollable};

/// One row: a binding, or a command with no key at all (its `key`, `context` and `source` empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub command: Command,
    /// The combo's label (`Cmd+Shift+P`), empty for an unbound command.
    pub key: String,
    /// The combo's config spelling (`cmd+shift+p`), empty for an unbound command: Enter writes it
    /// into a `keybindings.toml` stub.
    pub spec: String,
    /// The context's title (`Editor`, `Find`, …), empty for a global binding or an unbound command.
    pub context: String,
    /// Where the binding came from (`Default` / `User`), empty for an unbound command.
    pub source: String,
}

/// The open view: every row, the ones `query` keeps, the selection and the scroll (`height` is
/// set while drawing).
#[derive(Debug, Clone)]
pub struct KeysView {
    /// The search box's text.
    pub query: String,
    /// Recording keys: the next combo pressed becomes the search (Alt+K starts it, Esc cancels).
    pub record: bool,
    /// Every row, by command title then key.
    pub all: Vec<Row>,
    /// The rows `query` matches (all of them for an empty query), best match first.
    pub rows: Vec<Row>,
    pub selected: usize,
    pub top: usize,
    /// Rows on screen (render-time state).
    pub height: usize,
}

impl KeysView {
    pub fn new(keymap: &Keymap) -> Self {
        let all = build(keymap);
        Self {
            query: String::new(),
            record: false,
            rows: all.clone(),
            all,
            selected: 0,
            top: 0,
            height: 0,
        }
    }

    /// Re-runs the search: the new match list, the selection back at its top.
    pub fn refilter(&mut self) {
        self.rows = filter(&self.query, &self.all);
        self.selected = 0;
        self.top = 0;
    }

    /// The selected row (Enter opens its `keybindings.toml` stub).
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Scrolls `delta` rows without moving the selection (the wheel).
    pub fn scroll_by(&mut self, delta: isize) {
        self.set_top(self.top.saturating_add_signed(delta));
    }

    /// Selects row `index`, clamped, keeping it in view.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
        let height = self.height.max(1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + height {
            self.top = self.selected + 1 - height;
        }
    }

    /// Moves the selection by `delta` rows, clamped, keeping it in view.
    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.select(self.selected.saturating_add_signed(delta).min(self.rows.len() - 1));
    }

    /// Moves the selection a page up (`-1`) or down, keeping it in view.
    pub fn page(&mut self, delta: isize) {
        let page = self.height.max(1) as isize * delta;
        self.move_selection(page);
    }
}

impl Scrollable for KeysView {
    fn scroll(&self) -> Scroll {
        Scroll::rows(self.rows.len(), self.height, self.top)
    }

    fn set_top(&mut self, top: usize) {
        self.top = top.min(self.rows.len().saturating_sub(self.height.max(1)));
    }
}

/// Every row: each binding, then each command no key is bound to, by title then key.
pub fn build(keymap: &Keymap) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut bound = HashSet::new();
    for (combo, context, command, source) in keymap.bindings_with_source() {
        bound.insert(command);
        rows.push(Row {
            command,
            key: combo.label(),
            spec: combo.spec(),
            context: context.map(|c| c.title().to_string()).unwrap_or_default(),
            source: match source {
                Source::Default => "Default",
                Source::User => "User",
            }
            .to_string(),
        });
    }
    for command in Command::all() {
        if bound.insert(command) {
            rows.push(Row {
                command,
                key: String::new(),
                spec: String::new(),
                context: String::new(),
                source: String::new(),
            });
        }
    }
    rows.sort_by_cached_key(|r| (r.command.title(), r.key.clone()));
    rows
}

fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)
}

/// The rows of `rows` matching `query` by command title, name or key text (`ctrl+shift` finds
/// `Ctrl+Shift+P`), best first; all of them, in order, for an empty query.
pub fn filter(query: &str, rows: &[Row]) -> Vec<Row> {
    if query.is_empty() {
        return rows.to_vec();
    }
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, Row)> = rows
        .iter()
        .filter_map(|row| {
            let title = row.command.title();
            let name = row.command.name();
            let texts = [title.as_str(), name.as_str(), &row.key];
            let best = texts
                .iter()
                .filter_map(|text| pattern.score(Utf32Str::new(text, &mut buf), &mut matcher))
                .max()?;
            Some((best, row.clone()))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then_with(|| a.command.title().cmp(&b.command.title()))
            .then_with(|| a.key.cmp(&b.key))
    });
    scored.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::FindCommand;

    fn linux() -> Keymap {
        Keymap::default_for(crate::config::keymap::Platform::Linux)
    }

    #[test]
    fn build_lists_every_binding_and_unbound_command() {
        let keymap = linux();
        let rows = build(&keymap);
        let bindings: Vec<_> = keymap.bindings_with_source().collect();
        let unbound: Vec<_> = Command::all()
            .into_iter()
            .filter(|c| bindings.iter().all(|(_, _, cmd, _)| cmd != c))
            .collect();
        assert_eq!(rows.len(), bindings.len() + unbound.len());

        // Sorted by title then key.
        let mut sorted = rows.clone();
        sorted.sort_by_cached_key(|r| (r.command.title(), r.key.clone()));
        assert_eq!(rows, sorted);

        // A global binding, a context one, and a command with no key at all.
        let save = rows.iter().find(|r| r.command == Command::Save).unwrap();
        assert_eq!(save.key, "Ctrl+S");
        assert_eq!(save.spec, "ctrl+s");
        assert_eq!(save.context, "");
        assert_eq!(save.source, "Default");
        assert!(
            rows.iter()
                .any(|r| r.command == Command::Find(FindCommand::Next) && r.context == "Find"),
            "a context binding"
        );
        let shortcuts = rows.iter().find(|r| r.command == Command::KeyboardShortcuts).unwrap();
        assert_eq!(
            (
                shortcuts.key.as_str(),
                shortcuts.spec.as_str(),
                shortcuts.source.as_str()
            ),
            ("", "", "")
        );
    }

    #[test]
    fn filter_matches_title_name_and_key_text() {
        let rows = build(&linux());
        let all = rows.len();
        assert_eq!(filter("", &rows).len(), all);

        let by_title = filter("keyboard shortcuts", &rows);
        assert_eq!(by_title.first().map(|r| r.command), Some(Command::KeyboardShortcuts));

        let by_name = filter("preferences.keyboardShortcuts", &rows);
        assert_eq!(by_name.first().map(|r| r.command), Some(Command::KeyboardShortcuts));

        // Key text: every match has that as a subsequence of its key or is a title / name match.
        let by_key = filter("ctrl+shift", &rows);
        assert!(!by_key.is_empty());
        assert!(by_key.len() < all);
        let save_as = by_key.iter().find(|r| r.command == Command::SaveAs);
        assert!(save_as.is_some_and(|r| r.key == "Ctrl+Shift+S"));
        assert_eq!(filter("zzzznotacommand", &rows).len(), 0);
    }

    #[test]
    fn selection_and_scrolling_stay_in_range() {
        let mut view = KeysView::new(&linux());
        assert!(!view.record, "not recording until asked");
        let len = view.rows.len();
        view.height = 10;
        view.move_selection(-1);
        assert_eq!(view.selected, 0);
        view.move_selection(3);
        assert_eq!(view.selected, 3);
        view.move_selection(len as isize + 100);
        assert_eq!(view.selected, len - 1);
        assert_eq!(view.top, len - 10);
        view.select(0);
        assert_eq!((view.selected, view.top), (0, 0));
        view.page(1);
        assert_eq!(view.selected, 10);
        view.page(-1);
        assert_eq!(view.selected, 0);
        assert!(view.scroll().shown);

        // A search re-runs the list and puts the selection back at the top.
        view.selected = 5;
        view.top = 5;
        view.query = "save".into();
        view.refilter();
        assert!(!view.rows.is_empty());
        assert_eq!((view.selected, view.top), (0, 0));
        assert!(view.selected_row().is_some());

        // Scrolling clamps to the last page.
        let rows = view.rows.len();
        view.height = 2;
        view.set_top(usize::MAX);
        assert_eq!(view.top, rows.saturating_sub(2));
    }

    #[test]
    fn an_empty_result_list_is_safe_and_selects_nothing() {
        let mut view = KeysView::new(&linux());
        view.query = "zzznotacommand".into();
        view.refilter();
        assert!(view.rows.is_empty());
        assert_eq!(view.selected_row(), None);
        // The selection keys and scrolling do nothing and stay in range.
        view.height = 10;
        view.move_selection(1);
        view.page(1);
        view.select(usize::MAX);
        view.set_top(usize::MAX);
        assert_eq!((view.selected, view.top), (0, 0));
        assert!(!view.scroll().shown);
    }
}
