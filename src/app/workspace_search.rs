//! Workspace search (M7): the Search view's commands and input (`app.search_view`), starting a
//! search and taking in its results (`app.search`, drawn by `ui::search`).

use std::time::Instant;

use crossterm::event::{KeyEvent, MouseEvent};

use super::{App, AppEvent, Focus};
use crate::buffer::pos;
use crate::command::SearchCommand as S;
use crate::field::{Outcome, TextField};
use crate::layout::SidePanel;
use crate::search::SearchError;
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
        self.search_view.query.select_all();
        self.run_search();
    }

    pub(super) fn search_command(&mut self, cmd: S) {
        match cmd {
            S::ToggleCase | S::ToggleRegex => {
                let o = &mut self.search_view.options;
                if cmd == S::ToggleCase {
                    o.case_sensitive = !o.case_sensitive;
                } else {
                    o.regex = !o.regex;
                }
                self.run_search();
            }
            S::Refresh => self.run_search(),
            S::FocusEditor => {
                if self.group().active_tab().is_some() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
    }

    /// Keys the keymap doesn't bind in the Search view edit its query. An edit restarts the
    /// search after the debounce; emptying the query clears the results at once.
    pub(super) fn search_key(&mut self, key: KeyEvent) {
        if self.search_view.query.key(key) == Outcome::Edited {
            self.search_edited();
        }
    }

    pub(super) fn search_paste(&mut self, text: &str) {
        self.search_view.query.insert(&text.replace(['\r', '\n'], ""));
        self.search_edited();
    }

    fn search_edited(&mut self) {
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

    /// Searches for the view's query now, cancelling the running search; an invalid regex
    /// shows its error instead.
    fn run_search(&mut self) {
        self.search_view.started();
        self.search_view.error = self.start_search(self.search_view.query()).err();
    }

    /// A click in the Search view: an option toggle, else the panel (and its input) takes the
    /// focus. Result rows are 7c's.
    pub(super) fn search_click(&mut self, m: MouseEvent) {
        self.ui.focus = Focus::SidePanel;
        let Some(hit) = self.hits.search else { return };
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        if hit.toggles[0].contains(at) {
            self.search_command(S::ToggleCase);
        } else if hit.toggles[1].contains(at) {
            self.search_command(S::ToggleRegex);
        }
    }

    /// The mouse wheel over the Search view scrolls its results.
    pub(super) fn search_scroll(&mut self, delta: isize) {
        if let Some(results) = &self.search {
            self.search_view.scroll_by(results, delta);
        }
    }

    /// Starts searching the workspace for `query`, cancelling the running search. Open buffers
    /// with unsaved changes inside the root are searched from their text. An empty pattern
    /// gives finished, empty results without searching; an invalid regex clears the results
    /// and returns the error (nothing runs).
    pub fn start_search(&mut self, query: Query) -> Result<(), SearchError> {
        self.search_task = None; // cancels it
        self.search_generation += 1;
        let generation = self.search_generation;
        let matcher = match workspace_search::matcher(&query) {
            Ok(m) => m,
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
        assert!(app.search_view.query.all_selected);
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
