//! The bottom panel and its Problems list (`app.problems`, drawn by `ui::panel`): showing,
//! hiding and focusing the panel, the list's keys and clicks, opening a problem, and next /
//! previous problem (F8 / Shift+F8).

use crossterm::event::MouseEvent;

use super::files::Goto;
use super::{App, Focus, Level, Message};
use crate::command::ProblemsCommand as P;
use crate::layout::PanelTab;
use crate::problems::{self, Target};

impl App {
    /// The bottom panel has the keys (context `problems`).
    pub(super) fn panel_focused(&self) -> bool {
        self.ui.focus == Focus::Panel
    }

    /// Cmd+J: shows the panel and gives it the keys, or hides it (the editor gets the keys back
    /// if the panel had them), as VS Code's "Toggle Panel".
    pub(super) fn toggle_panel(&mut self) {
        if self.ui.panel_visible {
            self.hide_panel();
        } else {
            self.focus_panel(self.ui.panel_tab);
        }
    }

    /// Cmd+Shift+M / a click on the status bar's counts: shows the Problems list and gives it
    /// the keys, or hides the panel when the list already has them (VS Code's toggle).
    pub(super) fn toggle_problems(&mut self) {
        if self.ui.panel_visible && self.ui.panel_tab == PanelTab::Problems && self.panel_focused() {
            self.hide_panel();
        } else {
            self.focus_panel(PanelTab::Problems);
        }
    }

    /// A click on the status bar's counts (VS Code): shows the Problems list and gives it the
    /// keys, or hides the panel when it already shows the list (focused or not).
    pub(super) fn problem_counts_clicked(&mut self) {
        if self.ui.panel_visible && self.ui.panel_tab == PanelTab::Problems {
            self.hide_panel();
        } else {
            self.focus_panel(PanelTab::Problems);
        }
    }

    fn hide_panel(&mut self) {
        self.ui.panel_visible = false;
        if self.panel_focused() {
            self.focus_editor_or_side();
        }
    }

    /// Shows the panel on `tab` and gives it the keys; the Problems list selects its first row
    /// when nothing is selected.
    pub(super) fn focus_panel(&mut self, tab: PanelTab) {
        self.ui.panel_visible = true;
        self.ui.panel_tab = tab;
        self.ui.focus = Focus::Panel;
        let files = problems::files(&self.lsp.diagnostics, &self.buffers);
        if self.problems.list.selected_row(&files).is_none() {
            self.problems.list.select(&files, 0);
        }
    }

    /// Back to the editor when there's a tab, else the side panel when it's shown.
    fn focus_editor_or_side(&mut self) {
        self.ui.focus = if self.group().active_tab().is_none() && self.ui.side_visible {
            Focus::SidePanel
        } else {
            Focus::Editor
        };
    }

    pub(super) fn problems_command(&mut self, cmd: P) {
        let files = problems::files(&self.lsp.diagnostics, &self.buffers);
        let list = &mut self.problems.list;
        match cmd {
            P::Up | P::Down | P::PageUp | P::PageDown => {
                let page = list.height.max(2) as isize - 1;
                let delta = match cmd {
                    P::Up => -1,
                    P::Down => 1,
                    P::PageUp => -page,
                    _ => page,
                };
                // Up from the first row stays there (nothing above the list takes the keys).
                list.move_selection(&files, delta);
            }
            P::First | P::Last => list.select(&files, if cmd == P::First { 0 } else { usize::MAX }),
            P::Open => {
                if let Some(t) = self.problems.target(&files) {
                    drop(files);
                    self.open_problem(t, true, true);
                }
            }
            P::FocusEditor => {
                drop(files);
                if self.group().active_tab().is_some() {
                    self.ui.focus = Focus::Editor;
                }
            }
            P::Next | P::Previous => {
                drop(files);
                self.step_problem(cmd == P::Next);
            }
        }
    }

    /// Opens a problem in the active (last focused) editor group: its range selected (`select`,
    /// the Problems list) or a cursor at its start (F8), centred if off screen; `focus` gives
    /// the editor the keys. A file not open yet is loaded first and placed when it arrives.
    fn open_problem(&mut self, t: Target, select: bool, focus: bool) {
        self.open_file(t.path, focus, Some(Goto::Problem { place: t.place, select }));
    }

    /// F8 (`forward`) / Shift+F8: the next / previous problem from the active editor's cursor,
    /// on through the files with problems (`problems::step`), opened with a cursor at its start
    /// and its message in the status bar (VS Code shows it in a peek under the line instead);
    /// selected in the Problems list too. The focus stays where it is.
    fn step_problem(&mut self, forward: bool) {
        let files = problems::files(&self.lsp.diagnostics, &self.buffers);
        let from = self.active_buffer().zip(self.group().active_tab()).map(|(buf, tab)| {
            let at = tab.view.selections.primary().start();
            // Still on the problem the last step went to: go on from it (several can start at
            // one place).
            let last = self
                .last_problem
                .as_ref()
                .filter(|(path, _, start)| *path == buf.path && *start == at)
                .map(|(_, k, _)| *k);
            (buf.path.clone(), at, last)
        });
        let from_ref = from.as_ref().map(|(path, at, last)| problems::From {
            path,
            at: *at,
            last: *last,
        });
        let Some((i, k)) = problems::step(&files, from_ref, forward) else {
            self.notify(Level::Info, "No problems".into());
            return;
        };
        let file = &files[i];
        let Some(problem) = file.problem(k) else { return };
        let text = problem_message(problem.diag, k, file.count());
        let level = match problem.diag.severity {
            crate::diagnostics::Severity::Error => Level::Error,
            crate::diagnostics::Severity::Warning => Level::Warning,
            _ => Level::Info,
        };
        let target = Target {
            path: file.path.to_path_buf(),
            place: problem.place,
        };
        self.problems.select_problem(&files, file.path, k);
        drop(files);
        let path = target.path.clone();
        self.open_problem(target, false, false);
        // Where the cursor landed (an open buffer: at once; else when it loads, and then the
        // byte offset isn't known yet, so the next F8 goes by position).
        let at = self
            .active_buffer()
            .filter(|b| b.path == path)
            .zip(self.group().active_tab())
            .map(|(_, tab)| tab.view.selections.primary().start());
        self.last_problem = at.map(|at| (path, k, at));
        // Not `notify`: no log line per step. Cleared by the next key, like a notification.
        self.message = Some(Message { text, level });
    }

    /// A click in the Problems list: the row is selected and the list gets the keys; a problem
    /// row opens with the focus staying in the list, a double click (also on a file row: its
    /// first problem) opens it and focuses the editor. Elsewhere in the panel: the keys only.
    pub(super) fn problems_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::Panel;
        let Some(hit) = self.hits.problems.take() else { return };
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        if hit.list.contains(at)
            && let Some(key) = hit.rows.get((at.y - hit.list.y) as usize)
        {
            let files = problems::files(&self.lsp.diagnostics, &self.buffers);
            self.problems.list.select_key(&files, key);
            if key.1 > 0 || count >= 2 {
                let target = self.problems.target(&files);
                drop(files);
                if let Some(t) = target {
                    self.open_problem(t, true, count >= 2);
                }
            }
        }
        self.hits.problems = Some(hit);
    }

    /// The mouse wheel over the panel scrolls its list.
    pub(super) fn problems_scroll(&mut self, delta: isize) {
        let files = problems::files(&self.lsp.diagnostics, &self.buffers);
        self.problems.list.scroll_by(&files, delta);
    }
}

/// The status bar's text for a problem F8 went to: icon, the message on one line, source and
/// code, and where it is in its file ("2 of 5").
fn problem_message(d: &crate::diagnostics::Diagnostic, k: usize, count: usize) -> String {
    let message = d.message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut text = format!("{} {message}", d.severity.icon());
    if let Some(source) = problems::source_label(d) {
        text.push_str(&format!("  {source}"));
    }
    text.push_str(&format!("  ({} of {count})", k + 1));
    text
}
