//! Workspace search (M7): the Search view's commands and input (`app.search_view`), starting a
//! search and taking in its results (`app.search`, drawn by `ui::search`), and opening results.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};

use super::files::Goto;
use super::{App, AppEvent, Focus, Level};
use crate::buffer::pos;
use crate::command::SearchCommand as S;
use crate::diff_view::{DiffTab, Pair};
use crate::field::{Outcome, TextField};
use crate::group::Tab;
use crate::layout::SidePanel;
use crate::path_filter;
use crate::search::SearchError;
use crate::search_view::{Input, Target};
use crate::workspace_search::{self, Batch, Job, Query, Results};

impl App {
    /// The Search view's query input has the keys (context `search`).
    pub(super) fn search_focused(&self) -> bool {
        self.ui.focus == Focus::SidePanel && self.ui.side_panel == SidePanel::Search
    }

    /// Cmd+Shift+F: shows the Search view and focuses its query, seeded with the primary
    /// selection when it's non-empty and on one line (escaped when regex is on, as the find
    /// widget does; else the last query stays), selected so typing replaces it; searches now.
    pub(super) fn find_in_files(&mut self) {
        self.seed_search();
        self.search_view.query.select_all();
        self.run_search();
    }

    /// Cmd+Shift+H: Find in Files with the replace input shown; it takes the keys (its text
    /// selected) when the query has text, else the query does (VS Code).
    fn replace_in_files(&mut self) {
        self.seed_search();
        let view = &mut self.search_view;
        view.replacing = true;
        if view.query.text.is_empty() {
            view.input = Input::Query;
        } else {
            view.input = Input::Replace;
            view.replace.select_all();
        }
        view.query.select_all();
        self.run_search();
    }

    /// Shows and focuses the Search view, its query seeded with the primary selection when it's
    /// non-empty and on one line (escaped for a regex).
    fn seed_search(&mut self) {
        let regex = self.search_view.options.regex;
        let seed = self
            .active_buffer()
            .zip(self.group().active_tab())
            .and_then(|(buf, tab)| {
                let sel = tab.view.selections.primary();
                let rope = &buf.rope;
                let one_line = pos::line_of(rope, sel.start()) == pos::line_of(rope, sel.end());
                (!sel.is_empty() && one_line).then(|| rope.slice(sel.start().0..sel.end().0).to_string())
            });
        if let Some(text) = seed {
            self.search_view.query = TextField::new(if regex { regex::escape(&text) } else { text });
        }
        self.focus_side(SidePanel::Search);
        self.search_view.input = Input::Query;
    }

    pub(super) fn search_command(&mut self, cmd: S) {
        match cmd {
            S::ToggleCase | S::ToggleWholeWord | S::ToggleRegex => {
                let o = &mut self.search_view.options;
                let flag = match cmd {
                    S::ToggleCase => &mut o.case_sensitive,
                    S::ToggleWholeWord => &mut o.whole_word,
                    _ => &mut o.regex,
                };
                *flag = !*flag;
                self.run_search();
            }
            S::Refresh => self.run_search(),
            S::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
            S::FocusResults => {
                if let Some(results) = &self.search
                    && !results.files.is_empty()
                {
                    let view = &mut self.search_view;
                    view.list_focused = true;
                    if view.selected_row(results).is_none() {
                        view.select(results, 0);
                    }
                }
            }
            S::Up | S::Down | S::PageUp | S::PageDown => {
                let page = self.search_view.list.height.max(2) as isize - 1;
                let delta = match cmd {
                    S::Up => -1,
                    S::Down => 1,
                    S::PageUp => -page,
                    _ => page,
                };
                let Some(results) = &self.search else { return };
                if !self.search_view.move_selection(results, delta) {
                    self.search_view.list_focused = false;
                }
            }
            S::First | S::Last => {
                let Some(results) = &self.search else { return };
                let row = if cmd == S::First { 0 } else { usize::MAX };
                self.search_view.select(results, row);
            }
            S::Open => {
                let target = self.search.as_mut().and_then(|r| self.search_view.target(r));
                if let Some(t) = target {
                    self.open_search_target(t, true);
                }
            }
            S::NextResult | S::PreviousResult => {
                let forward = cmd == S::NextResult;
                let target = self.search.as_mut().and_then(|r| self.search_view.step(r, forward));
                if let Some(t) = target {
                    self.open_search_target(t, false);
                }
            }
            S::ToggleDetails => {
                // From elsewhere (the palette, the editor) the view is shown and focused first.
                if !self.search_focused() {
                    self.focus_side(SidePanel::Search);
                }
                self.search_view.toggle_details();
            }
            S::NextInput | S::PreviousInput => {
                self.search_view.cycle(self.search.as_ref(), cmd == S::NextInput);
            }
            S::Collapse | S::Expand => {
                if let Some(results) = &mut self.search {
                    self.search_view.collapse_key(results, cmd == S::Collapse);
                }
            }
            S::CollapseAll | S::ExpandAll => {
                if let Some(results) = &mut self.search {
                    self.search_view.collapse_all(results, cmd == S::CollapseAll);
                }
            }
            S::Dismiss => {
                if let Some(key) = self.search_view.list.selected().cloned() {
                    self.search_dismiss(&key);
                }
            }
            S::ToggleReplace => {
                if !self.search_focused() {
                    self.focus_side(SidePanel::Search);
                }
                self.search_view.toggle_replace();
                self.refresh_replace_previews();
            }
            S::ReplaceInFiles => self.replace_in_files(),
            S::ReplaceOne => {
                let scope = self.search_view.list.selected().and_then(|key| self.replace_scope(key));
                if let Some(scope) = scope {
                    self.search_replace(scope);
                }
            }
            S::ReplaceAll => self.search_replace(super::replace::Scope::All),
        }
    }

    /// Dismisses a results row (Delete, or its `×`); an emptied list gives the keys back to the
    /// query.
    fn search_dismiss(&mut self, key: &crate::search_view::RowKey) {
        let Some(results) = &mut self.search else { return };
        self.search_view.dismiss(results, key);
        if results.files.is_empty() {
            self.search_view.list_focused = false;
        }
        self.refresh_replace_previews();
    }

    /// "Find in Folder…" (an explorer folder's right-click, `explorer.findInFolder`): the Search
    /// view with its details shown and the folder (relative to the root, escaped) as the files to
    /// include; the query takes the keys, selected, and the search runs now. On a file: a message.
    pub(super) fn find_in_folder(&mut self) {
        let Some(path) = self.explorer.selected.clone() else {
            return;
        };
        if !self.explorer.find(&path).is_some_and(|e| e.is_dir) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            self.notify(
                Level::Info,
                format!("{name} is a file: Find in Folder… searches a folder"),
            );
            return;
        }
        let Ok(rel) = path.strip_prefix(&self.workspace.root) else {
            return;
        };
        let pattern = path_filter::folder_pattern(rel);
        self.focus_side(SidePanel::Search);
        let view = &mut self.search_view;
        view.include = TextField::new(pattern);
        view.details = true;
        view.input = Input::Query;
        view.query.select_all();
        self.run_search();
    }

    /// Opens a search result: its file's replace preview while the replace input has text,
    /// else the file at the match.
    fn open_search_target(&mut self, t: Target, focus: bool) {
        if self.search_view.previewing() {
            self.open_replace_preview(t, focus);
        } else {
            self.open_result(t, focus);
        }
    }

    /// The replace preview of a result's file (VS Code's): a diff tab of the file as it is
    /// against it with every shown match replaced, in the active group — the group's preview
    /// of that file reused and brought up to date — its cursor on the result's line. `focus`
    /// moves the keys to it, else they stay where they were.
    fn open_replace_preview(&mut self, t: Target, focus: bool) {
        let Some(replace) = self
            .search
            .as_ref()
            .and_then(|r| r.file_replace(&t.path, &self.search_view.replace.text))
        else {
            return;
        };
        let path = self.workspace.root.join(&t.path);
        let before = self.ui.focus;
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let existing = self
            .group_mut()
            .tabs
            .iter_mut()
            .find_map(|tab| tab.diff_mut().filter(|d| d.path == path && d.pair == Pair::REPLACE));
        match existing {
            Some(d) => d.set_replace(Some(replace)),
            None => {
                let id = self.next_diff_id();
                let mut d = DiffTab::new(id, path.clone(), Pair::REPLACE);
                d.set_replace(Some(replace));
                self.group_mut().open_tab(Tab::Diff(Box::new(d)));
            }
        }
        self.open_diff(path, Pair::REPLACE);
        if let Some(d) = self.group_mut().active_diff_mut() {
            d.go_to_line(t.line);
        }
        if !focus {
            self.ui.focus = before;
        }
    }

    /// The replace text or the results changed (a new search, a dismissal): every open replace
    /// preview takes the new replace — nothing replaced for a file dismissed or no longer
    /// searched — while there is one (an emptied input leaves them as they are).
    pub(super) fn refresh_replace_previews(&mut self) {
        if !self.search_view.previewing() {
            return;
        }
        let Some(results) = &self.search else { return };
        let root = &self.workspace.root;
        let text = &self.search_view.replace.text;
        for (_, d) in self.groups.diff_tabs_mut() {
            if d.pair == Pair::REPLACE
                && let Ok(rel) = d.path.strip_prefix(root)
            {
                d.set_replace(results.file_replace(rel, text));
            }
        }
    }

    /// Opens a search result in the active (last focused) editor group with the match selected
    /// and centred if off screen; `focus` moves the keys to the editor. A file not open yet is
    /// loaded first and selected when it arrives; the line and offsets are clamped to the text.
    fn open_result(&mut self, t: Target, focus: bool) {
        let path = self.workspace.root.join(&t.path);
        let goto = Goto::Match {
            line: t.line,
            range: t.range,
        };
        self.open_file(path, focus, Some(goto));
    }

    /// Keys the keymap doesn't bind in the Search view edit its focused input (the query, or the
    /// files to include / exclude). An edit restarts the search after the debounce; emptying the
    /// query clears the results at once. In the results list, typing a character goes back to the
    /// input that last had the keys and types it; other keys do nothing.
    pub(super) fn search_key(&mut self, key: KeyEvent) {
        if self.search_view.list_focused {
            let typed = matches!(key.code, KeyCode::Char(_)) && (key.modifiers - KeyModifiers::SHIFT).is_empty();
            if !typed {
                return;
            }
            self.search_view.list_focused = false;
        }
        let mut field = std::mem::take(self.search_view.input_mut());
        let outcome = self.field_key(&mut field, Some(self.context()), key);
        *self.search_view.input_mut() = field;
        if outcome == Outcome::Edited {
            self.search_edited();
        }
    }

    /// Whether the replace input has the keys (its edits re-search nothing).
    fn replace_focused(&self) -> bool {
        !self.search_view.list_focused && self.search_view.input == Input::Replace
    }

    /// A paste goes to the focused input (from the list: the input that last had the keys).
    pub(super) fn search_paste(&mut self, text: &str) {
        self.search_view.list_focused = false;
        self.search_view.input_mut().insert(&text.replace(['\r', '\n'], ""));
        self.search_edited();
    }

    /// An input was edited: the replace text redraws the previews (and open replace previews
    /// follow); the others re-search after the debounce (an emptied query at once).
    fn search_edited(&mut self) {
        if self.replace_focused() {
            self.refresh_replace_previews();
            return;
        }
        if self.search_view.query.text.is_empty() {
            self.run_search();
        } else {
            self.search_view.edited(Instant::now());
        }
    }

    /// Runs a debounced search once it's due (the event loop calls this at the deadline).
    pub fn search_tick(&mut self, now: Instant) {
        if self.search_view.take_due(now) {
            self.run_search();
            self.needs_redraw = true;
        }
    }

    /// Searches for the view's query now, cancelling the running search; an invalid regex or
    /// glob shows its error instead.
    fn run_search(&mut self) {
        self.search_view.started();
        self.search_view.error = self.start_search(self.search_view.query()).err();
        self.refresh_replace_previews();
    }

    /// A click in the Search view: an option toggle or the `…` details button; an input (it takes
    /// the keys); a row's `×` (dismissed) or a file row's chevron (collapsed / expanded); a
    /// result row (selected, and the list takes the keys): a match opens with the focus staying
    /// in the list, a double click opens a match or a file's first match and focuses the editor;
    /// anywhere else the query takes the keys.
    pub(super) fn search_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::SidePanel;
        let Some(hit) = self.hits.search.take() else { return };
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        let input = [Input::Query, Input::Include, Input::Exclude]
            .into_iter()
            .zip(hit.inputs)
            .find(|(_, r)| r.contains(at));
        let toggle = [S::ToggleCase, S::ToggleWholeWord, S::ToggleRegex]
            .into_iter()
            .zip(hit.toggles)
            .find(|(_, r)| r.contains(at));
        let input = input.or_else(|| hit.replace.contains(at).then_some((Input::Replace, hit.replace)));
        let dismiss = hit.dismiss.iter().find(|(r, _)| r.contains(at));
        let replace_row = hit.replace_row.iter().find(|(r, _)| r.contains(at));
        let chevron = hit.chevrons.iter().find(|(r, _)| r.contains(at));
        if let Some((cmd, _)) = toggle {
            self.search_command(cmd);
        } else if hit.replace_all.contains(at) {
            self.search_command(S::ReplaceAll);
        } else if let Some((_, key)) = replace_row {
            self.search_view.list_focused = true;
            if let Some(results) = &self.search {
                self.search_view.select_key(results, key);
            }
            if let Some(scope) = self.replace_scope(key) {
                self.search_replace(scope);
            }
        } else if let Some((_, key)) = dismiss {
            self.search_view.list_focused = true;
            self.search_dismiss(key);
        } else if let Some((_, path)) = chevron
            && let Some(results) = &mut self.search
            && let Some(i) = results.file_index(path)
        {
            self.search_view.list_focused = true;
            let collapse = !results.files[i].collapsed;
            self.search_view.set_collapsed(results, path, collapse);
            self.search_view.select_key(results, &(path.clone(), 0));
        } else if hit.details.contains(at) {
            self.search_command(S::ToggleDetails);
        } else if hit.replace_toggle.contains(at) {
            self.search_command(S::ToggleReplace);
        } else if let Some((input, _)) = input {
            self.search_view.input = input;
            self.search_view.list_focused = false;
        } else if hit.list.contains(at) {
            let row = hit.rows.get((at.y - hit.list.y) as usize);
            if let (Some(key), Some(results)) = (row, &mut self.search) {
                self.search_view.list_focused = true;
                self.search_view.select_key(results, key);
                if key.1 > 0 || count >= 2 {
                    let target = self.search_view.target(results);
                    if let Some(t) = target {
                        self.open_search_target(t, count >= 2);
                    }
                }
            }
        } else {
            self.search_view.list_focused = false;
        }
        self.hits.search = Some(hit);
    }

    /// The pointer moved: the results row under it (its `×` is drawn), if over the view's list.
    pub(super) fn search_hover(&mut self, m: &MouseEvent, hit: crate::layout::Hit) {
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        let over = match (&self.hits.search, hit) {
            (Some(h), crate::layout::Hit::SidePanel) if h.list.contains(at) => {
                h.rows.get((m.row - h.list.y) as usize).cloned()
            }
            _ => None,
        };
        if over != self.search_view.hover {
            self.search_view.hover = over;
            self.needs_redraw = true;
        }
    }

    /// The mouse wheel over the Search view scrolls its results; the hovered row becomes the
    /// one now under the pointer (its `×` moves with it).
    pub(super) fn search_scroll(&mut self, delta: isize, m: &MouseEvent) {
        let Some(results) = &self.search else { return };
        self.search_view.scroll_by(results, delta);
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        if let Some(h) = &self.hits.search
            && h.list.contains(at)
        {
            let row = self.search_view.scroll(results) + (m.row - h.list.y) as usize;
            self.search_view.hover = crate::search_view::rows(results, row, 1).first().map(|s| s.key());
        }
    }

    /// Starts searching the workspace for `query`, cancelling the running search. Open buffers
    /// with unsaved changes inside the root are searched from their text. The query's files to
    /// include / exclude narrow both. An empty pattern gives finished, empty results without
    /// searching; an invalid regex or glob clears the results and returns the error (nothing
    /// runs).
    pub fn start_search(&mut self, query: Query) -> Result<(), SearchError> {
        self.search_task = None; // cancels it
        self.search_generation += 1;
        let generation = self.search_generation;
        let (matcher, filter) =
            match workspace_search::matcher(&query).and_then(|m| Ok((m, workspace_search::filter(&query)?))) {
                Ok(both) => both,
                Err(e) => {
                    self.search = None;
                    return Err(e);
                }
            };
        let mut results = Results::new(generation, query);
        if matcher.is_empty() {
            results.done = true;
            self.search = Some(results);
            return Ok(());
        }
        results.matcher = Some(matcher.clone());
        let root = self.workspace.root.clone();
        let dirty = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_dirty())
            .filter_map(|(_, b)| Some((b.path.strip_prefix(&root).ok()?.to_path_buf(), b.rope.clone())))
            .collect();
        let job = Job {
            root,
            excludes: self.excludes.clone(),
            matcher,
            filter,
            dirty,
            generation,
        };
        let tx = self.tx.clone();
        self.search_task = Some(workspace_search::start(job, move |batch| {
            let _ = tx.send(AppEvent::SearchResults(batch));
        }));
        self.search = Some(results);
        Ok(())
    }

    /// A batch of results: added when it belongs to the current search, else dropped.
    pub(super) fn search_results(&mut self, batch: Batch) {
        let Some(results) = &mut self.search else { return };
        if results.add(batch) && results.done {
            self.search_task = None;
            // Nothing left (all replaced): the keys go back to the query.
            if results.files.is_empty() {
                self.search_view.list_focused = false;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // one-match lines
mod tests {
    use std::fs;

    use crate::buffer::Buffer;
    use crate::workspace_search::{FileMatches, LineMatch};

    use super::*;

    fn batch(generation: u64, done: bool) -> Batch {
        Batch {
            generation,
            files: vec![FileMatches {
                path: "x.txt".into(),
                lines: vec![LineMatch {
                    line: 0,
                    ranges: vec![0..1],
                    text: "x".into(),
                    text_start: 0,
                }],
                collapsed: false,
            }],
            done,
            truncated: false,
        }
    }

    /// A replaced search's batches (even its last one) change nothing and don't release the
    /// running search; the current one's last batch does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_replaced_searchs_batches_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
        let q = |p: &str| Query {
            pattern: p.into(),
            ..Query::default()
        };
        app.start_search(q("needle")).unwrap();
        let old = app.search.as_ref().unwrap().generation;
        app.start_search(q("other")).unwrap();
        let new = app.search.as_ref().unwrap().generation;
        assert!(new > old);
        app.search_results(batch(old, true));
        let r = app.search.as_ref().unwrap();
        assert!(r.files.is_empty() && !r.done);
        assert!(app.search_task.is_some());
        app.search_results(batch(new, false));
        assert!(app.search_task.is_some(), "still running");
        app.search_results(batch(new, true));
        let r = app.search.as_ref().unwrap();
        assert_eq!((r.total, r.done), (1, true), "x.txt replaced, not counted twice");
        assert!(app.search_task.is_none());
    }

    /// The event loop's timer (`next_timer`, shared by `run` and `settle`) wakes for the earliest
    /// deadline: the Search view's debounce before a later fs batch, and never when none is set.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_event_loop_timer_runs_the_debounced_search() {
        use std::time::Duration;

        use super::super::Timer;

        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
        let idle = tokio::time::timeout(Duration::from_millis(50), app.next_timer()).await;
        assert!(idle.is_err(), "nothing pending: never wakes");

        app.fs_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(60));
        app.search_paste("needle");
        let t = std::time::Instant::now();
        let timer = tokio::time::timeout(Duration::from_secs(5), app.next_timer())
            .await
            .unwrap();
        assert_eq!(timer, Timer::SearchDebounce);
        assert!(t.elapsed() >= Duration::from_millis(100), "waits for the debounce");
        app.on_timer(timer);
        assert_eq!(
            app.search_view.deadline(),
            None,
            "taken: the next wait is for the fs batch"
        );
        assert_eq!(app.search.as_ref().unwrap().query.pattern, "needle");

        // A deadline already past fires at once, and running it clears it (no spinning).
        app.fs_deadline = None;
        app.search_view
            .edited(std::time::Instant::now() - Duration::from_secs(1));
        let timer = tokio::time::timeout(Duration::from_millis(50), app.next_timer())
            .await
            .unwrap();
        app.on_timer(timer);
        assert_eq!(app.search_view.deadline(), None);
    }

    /// The Search view's input: seeding (escaped for a regex), the debounce, an empty query and
    /// an invalid regex.
    #[tokio::test(flavor = "multi_thread")]
    async fn search_view_input() {
        use std::time::Instant;

        use crate::buffer::pos::ByteIdx;
        use crate::search_view::DEBOUNCE;
        use crate::selection::{Selection, Selections};

        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::write(root.join("a.txt"), "a.b x\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
        let sel = |app: &mut App, a: usize, b: usize| {
            app.active_mut().unwrap().1.selections = Selections::single(Selection::new(ByteIdx(a), ByteIdx(b)));
        };
        sel(&mut app, 0, 3);
        app.search_view.options.regex = true;
        app.find_in_files();
        assert_eq!(app.search_view.query.text, r"a\.b");
        assert!(app.search_view.query.selection().is_some());
        assert!(app.search_focused());
        assert_eq!(app.search.as_ref().unwrap().query.pattern, r"a\.b");

        // An edit waits for the debounce; the tick runs it once.
        let generation = app.search_generation;
        app.search_paste("(");
        assert_eq!(app.search_view.query.text, "(");
        app.search_tick(Instant::now());
        assert_eq!(app.search_generation, generation);
        app.search_tick(Instant::now() + DEBOUNCE);
        assert_eq!(app.search_generation, generation + 1);
        assert_eq!(
            app.search_view.error.as_ref().map(|e| e.to_string()).as_deref(),
            Some("unclosed group")
        );
        assert!(app.search.is_none() && app.search_task.is_none(), "nothing runs");
        app.search_tick(Instant::now() + DEBOUNCE);
        assert_eq!(app.search_generation, generation + 1, "once");

        // Emptying the query clears the results (and the error) at once.
        app.search_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Backspace));
        assert_eq!(app.search_view.deadline(), None);
        assert!(app.search_view.error.is_none());
        let r = app.search.as_ref().unwrap();
        assert!(r.done && r.files.is_empty());

        // Plain text isn't escaped; an empty selection keeps the query.
        app.search_view.options.regex = false;
        sel(&mut app, 0, 3);
        app.find_in_files();
        assert_eq!(app.search_view.query.text, "a.b");
        sel(&mut app, 4, 4);
        app.find_in_files();
        assert_eq!(app.search_view.query.text, "a.b");
    }
}
