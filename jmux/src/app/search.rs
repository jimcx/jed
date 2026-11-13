//! Searching a terminal's output and clicking its links (jed's `app/term_search.rs` and the URL half
//! of `app/term_link.rs`, over `jterm::search`). The bar (`App::search`) stands over one run and
//! shows only while that tab is the active one; while it shows, `Context::Search` takes the keys
//! (the program gets none) and every query edit, step or case toggle searches the whole scrollback
//! again and scrolls the view to the match. A Ctrl / Cmd + left press on a URL opens it through
//! `Config::open_url`, off the UI thread. File paths are not links here: jmux has no editor.

use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jterm::run::Run;
use jterm::search::{self, Match, TermSearch};
use jterm::select;
use jtui::field::{FieldCommand, Outcome, TextField};
use jtui::split::GroupId;
use ratatui::layout::Rect;

use super::App;
use crate::command::SearchCommand;

impl App {
    fn active_run(&self) -> Option<&Run> {
        self.groups.active_group().active_tab().map(|s| &s.run)
    }

    /// The bar is open over the terminal that has the keys.
    pub(super) fn search_active(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|s| self.active_run().is_some_and(|r| r.id == s.run))
    }

    pub(super) fn search_command(&mut self, cmd: SearchCommand) {
        match cmd {
            SearchCommand::Open => self.open_search(),
            SearchCommand::Close => self.search = None,
            SearchCommand::Next | SearchCommand::Previous if self.search_active() => {
                self.search_step(cmd == SearchCommand::Next)
            }
            SearchCommand::ToggleCase if self.search_active() => {
                if let Some(s) = &mut self.search {
                    s.case = !s.case;
                }
                self.research();
            }
            _ => {}
        }
    }

    /// Shows the bar over the active terminal: the query kept (selected, so typing replaces it),
    /// seeded from a one-line mouse selection.
    fn open_search(&mut self) {
        let Some(run) = self.active_run() else { return };
        let id = run.id;
        let seed = run.copy_selection().filter(|t| !t.is_empty() && !t.contains('\n'));
        let mut s = match self.search.take() {
            Some(s) if s.run == id => s,
            _ => TermSearch::new(id),
        };
        if let Some(seed) = seed {
            s.field = TextField::new(seed);
        }
        s.field.select_all();
        self.search = Some(s);
        self.research();
    }

    /// A query edit command (`Command::Field`).
    pub(super) fn search_edit(&mut self, cmd: FieldCommand) {
        let Some(s) = self.search.as_mut() else { return };
        if s.field.run(cmd) == Outcome::Edited {
            self.research();
        }
    }

    /// A key no binding took, while the bar is open: typed into the query, else swallowed.
    pub(super) fn search_type(&mut self, key: KeyEvent) {
        let Some(s) = self.search.as_mut() else { return };
        if s.field.key(key) == Outcome::Edited {
            self.research();
        }
    }

    pub(super) fn search_paste(&mut self, text: &str) {
        if let Some(s) = &mut self.search {
            s.field.insert(&text.replace(['\r', '\n'], ""));
        }
        self.research();
    }

    /// The query or case changed: the newest match becomes current.
    fn research(&mut self) {
        self.search_apply(|matches, _| search::newest(matches));
    }

    /// Next (`forward`) or previous match from the current one, wrapping.
    fn search_step(&mut self, forward: bool) {
        self.search_apply(|matches, current| search::step_pick(matches, current, forward));
    }

    /// Searches again and lets `pick` choose the current match among them; scrolls to it.
    fn search_apply(&mut self, pick: impl FnOnce(&[Match], Option<Match>) -> Option<usize>) {
        let Some(mut s) = self.search.take() else { return };
        if let Some(run) = self.active_run().filter(|r| r.id == s.run) {
            let matches = search::find_in(run, &s.field.text, s.case);
            s.index = pick(&matches, s.current);
            s.current = s.index.map(|i| matches[i]);
            s.count = matches.len();
            if let Some(m) = s.current {
                search::reveal(run, m);
            }
        }
        self.search = Some(s);
    }

    /// A Ctrl / Cmd + left press over a URL in `group`'s terminal text: opens it. False for any
    /// other event or cell, which then goes on as a normal press.
    pub(super) fn link_click(&mut self, group: GroupId, text: Rect, m: MouseEvent) -> bool {
        let link = KeyModifiers::CONTROL | KeyModifiers::SUPER;
        if m.kind != MouseEventKind::Down(MouseButton::Left) || !m.modifiers.intersects(link) {
            return false;
        }
        let Some(run) = self.groups.get(group).and_then(|g| g.active_tab()).map(|s| &s.run) else {
            return false;
        };
        let url = {
            let screen = run.screen();
            let s = screen.screen();
            let at = select::screen_cell(s, text, m.column, m.row);
            select::wrapped_line(s, at).and_then(|(line, at)| jtui::url::url_at(&line, at).map(str::to_string))
        };
        let Some(url) = url else { return false };
        tracing::info!(%url, "opening link");
        let open = self.config.open_url;
        // The server shares the machine with the client's browser; the opener blocks.
        tokio::task::spawn_blocking(move || {
            if let Err(e) = open(&url) {
                tracing::warn!(%url, error = %e, "couldn't open the link");
            }
        });
        true
    }
}
