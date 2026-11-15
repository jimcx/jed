//! Searching a shell or task tab's output (`app.term_search`, model `tasks::search`): Cmd+F
//! (Ctrl+Shift+F on Linux) opens a bar over the terminal's top right, typing edits the query (the
//! program gets no keys while it has them: `Context::TermSearch`), and every query edit, step or
//! option searches the whole scrollback again (`find_all`, with the screen lock held, off no
//! thread: it is a pass over the kept rows) and scrolls the view to the match. Enter / Shift+Enter
//! (F3) step, wrapping; Esc closes. The highlights are searched per frame over the rows shown
//! (`ui::task`), so they follow new output; the count and the current match don't until the next
//! step.

use super::{App, Level};
use crate::command::TermSearchCommand as Ts;
use crate::field::Outcome;
use crate::tasks::search::{self, Match, TermSearch};
use crossterm::event::KeyEvent;

impl App {
    /// The run the bar stands over, if it is the terminal that has the keys.
    pub(super) fn term_search_active(&self) -> bool {
        self.term_search
            .as_ref()
            .is_some_and(|s| self.active_terminal().is_some_and(|r| r.id == s.run))
    }

    pub(super) fn term_search_command(&mut self, cmd: Ts) {
        match cmd {
            Ts::Open => self.open_term_search(),
            Ts::Close => self.term_search = None,
            Ts::Next | Ts::Previous if self.term_search_active() => self.term_search_step(cmd == Ts::Next),
            Ts::ToggleCase if self.term_search_active() => {
                if let Some(s) = &mut self.term_search {
                    s.case = !s.case;
                }
                self.term_research();
            }
            _ => {}
        }
    }

    /// Shows the bar over the terminal with the keys: the query kept (selected, so typing replaces
    /// it), seeded from a one-line mouse selection; a message on any other tab.
    fn open_term_search(&mut self) {
        let Some(run) = self.active_terminal() else {
            self.notify(Level::Info, "Not in a terminal".into());
            return;
        };
        let id = run.id;
        let seed = run.copy_selection().filter(|t| !t.is_empty() && !t.contains('\n'));
        let mut s = match self.term_search.take() {
            Some(s) if s.run == id => s,
            _ => TermSearch::new(id),
        };
        if let Some(seed) = seed {
            s.field = crate::field::TextField::new(seed);
        }
        s.field.select_all();
        self.term_search = Some(s);
        self.term_research();
    }

    pub(super) fn term_search_key(&mut self, key: KeyEvent) {
        let Some(mut s) = self.term_search.take() else {
            return;
        };
        let mut field = std::mem::take(&mut s.field);
        let outcome = self.field_key(&mut field, Some(crate::config::keymap::Context::TermSearch), key);
        s.field = field;
        self.term_search = Some(s);
        if outcome == Outcome::Edited {
            self.term_research();
        }
    }

    pub(super) fn term_search_paste(&mut self, text: &str) {
        if let Some(s) = &mut self.term_search {
            s.field.insert(&text.replace(['\r', '\n'], ""));
        }
        self.term_research();
    }

    /// The query or case changed: the newest match becomes current.
    fn term_research(&mut self) {
        self.term_search_apply(|matches, _| search::newest(matches));
    }

    /// Next (`forward`) or previous match from the current one, wrapping.
    fn term_search_step(&mut self, forward: bool) {
        self.term_search_apply(|matches, current| search::step_pick(matches, current, forward));
    }

    /// Searches again and lets `pick` choose the current match among them; scrolls to it.
    fn term_search_apply(&mut self, pick: impl FnOnce(&[Match], Option<Match>) -> Option<usize>) {
        let Some(mut s) = self.term_search.take() else {
            return;
        };
        if let Some(run) = self.active_terminal().filter(|r| r.id == s.run) {
            let matches = search::find_in(run, &s.field.text, s.case);
            s.index = pick(&matches, s.current);
            s.current = s.index.map(|i| matches[i]);
            s.count = matches.len();
            if let Some(m) = s.current {
                search::reveal(run, m);
            }
        }
        self.term_search = Some(s);
    }
}
