//! The Tools view's model (no drawing; `app/tools.rs` drives it, `ui/tools.rs` draws it): one
//! row per tool jed runs — each language server (live, failed, or stopped since: its log
//! remains), each task run with a panel tab, each syntax plugin's grammar build, then "jed" (jed's own log) — and the list's
//! selection and scroll. Rows are rebuilt from the app's state whenever they're needed (a few
//! servers and runs); the selection is kept by `RowKey`, so rows coming or going don't move it.

use std::path::{Path, PathBuf};

use crate::logs::Tool;
use crate::lsp::{self, State};
use crate::tasks::{RunId, Status};

/// What a row shows, stable across rebuilds: the selection's key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    /// A language server by its name (`ServerConfig::name`: its log's, shared by restarts).
    Server(String),
    Task(RunId),
    /// A syntax plugin's grammar by its id.
    Grammar(String),
    Jed,
}

/// A syntax plugin's grammar as the view shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrammarState {
    /// Queued behind another build.
    Waiting,
    Building,
    Ready,
    /// Built or loaded unsuccessfully: the language stays uncoloured.
    Failed,
}

impl GrammarState {
    pub fn label(self) -> &'static str {
        match self {
            GrammarState::Waiting => "waiting",
            GrammarState::Building => "building",
            GrammarState::Ready => "ready",
            GrammarState::Failed => "failed",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            GrammarState::Waiting | GrammarState::Building => ServerState::Starting.glyph(),
            GrammarState::Ready => ServerState::Running.glyph(),
            GrammarState::Failed => ServerState::Failed.glyph(),
        }
    }

    pub fn theme_key(self) -> &'static str {
        match self {
            GrammarState::Waiting | GrammarState::Building => ServerState::Starting.theme_key(),
            GrammarState::Ready => ServerState::Running.theme_key(),
            GrammarState::Failed => ServerState::Failed.theme_key(),
        }
    }
}

/// A language server's state as the view shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Running,
    /// Not found, failed to initialize, or crashed.
    Failed,
    /// Stopped (no language resolves to it any more: a `.workspace` change, a trust decision)
    /// or exited without a crash.
    Exited,
}

impl ServerState {
    pub fn label(self) -> &'static str {
        match self {
            ServerState::Starting => "starting",
            ServerState::Running => "running",
            ServerState::Failed => "failed",
            ServerState::Exited => "exited",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            ServerState::Starting => "○",
            ServerState::Running => "●",
            ServerState::Failed => "✗",
            ServerState::Exited => "–",
        }
    }

    pub fn theme_key(self) -> &'static str {
        match self {
            ServerState::Starting => "tools.starting",
            ServerState::Running => "tools.running",
            ServerState::Failed => "tools.failed",
            ServerState::Exited => "tools.exited",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerRow {
    pub name: String,
    /// The languages it serves, by name ("C, C++").
    pub languages: String,
    /// Its root, when not the workspace's.
    pub root: Option<PathBuf>,
    pub state: ServerState,
    /// Its latest unfinished work (`Indexing 40% 3/25`), while running.
    pub progress: Option<String>,
    /// Error lines in its log.
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Server(ServerRow),
    Task {
        id: RunId,
        name: String,
        status: Status,
    },
    Grammar {
        id: String,
        state: GrammarState,
        errors: u64,
    },
    Jed {
        errors: u64,
    },
}

impl Row {
    pub fn key(&self) -> RowKey {
        match self {
            Row::Server(s) => RowKey::Server(s.name.clone()),
            Row::Task { id, .. } => RowKey::Task(*id),
            Row::Grammar { id, .. } => RowKey::Grammar(id.clone()),
            Row::Jed { .. } => RowKey::Jed,
        }
    }

    /// Whether this row is `key`'s (without allocating).
    pub fn is(&self, key: &RowKey) -> bool {
        match (self, key) {
            (Row::Server(s), RowKey::Server(name)) => s.name == *name,
            (Row::Task { id, .. }, RowKey::Task(k)) => id == k,
            (Row::Grammar { id, .. }, RowKey::Grammar(k)) => id == k,
            (Row::Jed { .. }, RowKey::Jed) => true,
            _ => false,
        }
    }

    /// The tool whose log the row opens; `None` for a task run (its output is its panel tab).
    pub fn tool(&self) -> Option<Tool> {
        match self {
            Row::Server(s) => Some(Tool::Server(s.name.clone())),
            Row::Task { .. } => None,
            Row::Grammar { id, .. } => Some(Tool::Grammar(id.clone())),
            Row::Jed { .. } => Some(Tool::Jed),
        }
    }

    /// Error lines in its log (none for a task).
    pub fn errors(&self) -> u64 {
        match self {
            Row::Server(s) => s.errors,
            Row::Task { .. } => 0,
            Row::Grammar { errors, .. } => *errors,
            Row::Jed { errors } => *errors,
        }
    }
}

/// A task run's status in words: `starting`, `running`, `exited 0`, `exited 2`, `killed`,
/// `Hangup` (another signal, as `strsignal` names it), `failed to start`.
pub fn task_label(status: &Status) -> String {
    match status {
        Status::Starting => "starting".into(),
        Status::Running => "running".into(),
        Status::Exited(e) if e.killed => "killed".into(),
        Status::Exited(e) => match &e.signal {
            Some(s) => s.clone(),
            None => format!("exited {}", e.code),
        },
        Status::Failed(_) => "failed to start".into(),
    }
}

/// The servers' rows: live ones (starting / running / failed) in start order, then stopped ones
/// not shadowed by a live one of their name, then any other server with a log (pushed to by
/// a server no longer known), each with its log's error count.
pub fn server_rows(manager: &lsp::Manager, errors: &[(Tool, u64)], workspace_root: &Path) -> Vec<ServerRow> {
    let errors_of = |name: &str| {
        errors
            .iter()
            .find(|(t, _)| matches!(t, Tool::Server(n) if n == name))
            .map_or(0, |(_, n)| *n)
    };
    let root_of = |root: &Path| (root != workspace_root).then(|| root.to_path_buf());
    let languages = |ls: &[crate::syntax::Language]| ls.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
    let mut rows: Vec<ServerRow> = Vec::new();
    for s in manager.servers() {
        if rows.iter().any(|r| r.name == s.config.name) {
            continue;
        }
        let state = match s.state {
            State::Starting => ServerState::Starting,
            State::Running { .. } => ServerState::Running,
            State::Failed => ServerState::Failed,
        };
        let progress = match state {
            ServerState::Running => s.progress.latest().map(|w| w.label()),
            _ => None,
        };
        rows.push(ServerRow {
            name: s.config.name.clone(),
            languages: languages(&s.languages),
            root: root_of(&s.root),
            state,
            progress,
            errors: errors_of(&s.config.name),
        });
    }
    for s in manager.stopped() {
        if rows.iter().any(|r| r.name == s.config.name) {
            continue;
        }
        rows.push(ServerRow {
            name: s.config.name.clone(),
            languages: languages(&s.languages),
            root: root_of(&s.root),
            state: ServerState::Exited,
            progress: None,
            errors: errors_of(&s.config.name),
        });
    }
    for (tool, n) in errors {
        if let Tool::Server(name) = tool
            && !rows.iter().any(|r| r.name == *name)
        {
            rows.push(ServerRow {
                name: name.clone(),
                languages: String::new(),
                root: None,
                state: ServerState::Exited,
                progress: None,
                errors: *n,
            });
        }
    }
    rows
}

/// Every row: the servers, the task runs (oldest first), then "jed"; `errors` is
/// `Logs::error_counts`.
pub fn rows(
    manager: &lsp::Manager,
    runs: &[crate::tasks::Run],
    grammars: &[(String, GrammarState)],
    errors: &[(Tool, u64)],
    workspace_root: &Path,
) -> Vec<Row> {
    let mut rows: Vec<Row> = server_rows(manager, errors, workspace_root)
        .into_iter()
        .map(Row::Server)
        .collect();
    rows.extend(runs.iter().map(|r| Row::Task {
        id: r.id,
        name: r.name.clone(),
        status: r.status(),
    }));
    rows.extend(grammars.iter().map(|(id, state)| {
        Row::Grammar {
            id: id.clone(),
            state: *state,
            errors: errors
                .iter()
                .find(|(t, _)| matches!(t, Tool::Grammar(n) if n == id))
                .map_or(0, |(_, n)| *n),
        }
    }));
    let jed = errors.iter().find(|(t, _)| *t == Tool::Jed).map_or(0, |(_, n)| *n);
    rows.push(Row::Jed { errors: jed });
    rows
}

/// The view's list state (`app.tools_view`).
#[derive(Debug, Default)]
pub struct ToolsView {
    /// The top row's index (clamped by drawing).
    top: usize,
    /// The selected row's key, and its index when last seen (where the selection goes when its
    /// row leaves).
    selected: Option<(RowKey, usize)>,
    /// Rows shown in the last frame (set by drawing, `set_height`; page up / down); 0 until
    /// the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
    /// The error counts the last frame drew (`Logs::error_counts`): a log change redraws the
    /// view only when they differ.
    pub errors_drawn: Vec<(Tool, u64)>,
    /// The view was shown at the last loop turn (`App::sync_logs` rearms the store's waker when
    /// it appears, so error counts keep it current).
    pub was_shown: bool,
}

/// The list's scroll bar (`ToolsView::scrollable`).
pub struct ToolsScroll<'a> {
    view: &'a mut ToolsView,
    rows: usize,
}

impl crate::scrollbar::Scrollable for ToolsScroll<'_> {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.rows, self.view.height, self.view.scroll_in(self.rows))
    }

    fn set_top(&mut self, top: usize) {
        self.view.top = top;
        self.view.top = self.view.scroll_in(self.rows);
    }
}

impl ToolsView {
    /// The list's scroll bar over `rows` rows.
    pub fn scrollable(&mut self, rows: usize) -> ToolsScroll<'_> {
        ToolsScroll { view: self, rows }
    }

    /// The selected row's index: its key's row, else (that row left) the row now at its last
    /// index, clamped. `None` with nothing selected or no rows.
    pub fn selected_row(&self, rows: &[Row]) -> Option<usize> {
        let (key, last) = self.selected.as_ref()?;
        if rows.is_empty() {
            return None;
        }
        Some(
            rows.iter()
                .position(|r| r.is(key))
                .unwrap_or((*last).min(rows.len() - 1)),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_key(&self) -> Option<&RowKey> {
        self.selected.as_ref().map(|(k, _)| k)
    }

    /// Selects row `row` (clamped; nothing when there are none) and scrolls to it.
    pub fn select(&mut self, rows: &[Row], row: usize) {
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let row = row.min(rows.len() - 1);
        self.selected = Some((rows[row].key(), row));
        self.reveal_selected(rows);
    }

    /// Selects the row `key` names, if it's in the list.
    pub fn select_key(&mut self, rows: &[Row], key: &RowKey) -> bool {
        match rows.iter().position(|r| r.is(key)) {
            Some(i) => {
                self.select(rows, i);
                true
            }
            None => false,
        }
    }

    /// The selection follows to where `selected_row` puts it and remembers its index (each
    /// frame), so a row coming back later doesn't pull it back. The scroll stays.
    pub fn settle(&mut self, rows: &[Row]) {
        if self.selected.is_some() {
            match self.selected_row(rows) {
                Some(i) => self.selected = Some((rows[i].key(), i)),
                None => self.selected = None,
            }
        }
    }

    /// Moves the selection by `delta` rows (clamped); nothing selected selects the first row.
    pub fn move_selection(&mut self, rows: &[Row], delta: isize) {
        match self.selected_row(rows) {
            Some(i) => self.select(rows, i.saturating_add_signed(delta)),
            None => self.select(rows, 0),
        }
    }

    fn scroll_in(&self, rows: usize) -> usize {
        self.top.min(rows.saturating_sub(self.height.max(1)))
    }

    /// The top row (clamped so the last row can reach the bottom).
    pub fn scroll(&self, rows: &[Row]) -> usize {
        self.scroll_in(rows.len())
    }

    pub fn scroll_by(&mut self, rows: &[Row], delta: isize) {
        self.top = self.scroll(rows).saturating_add_signed(delta);
        self.top = self.scroll(rows);
    }

    /// Re-clamps the scroll (the list or its height changed).
    pub fn clamp(&mut self, rows: &[Row]) {
        self.top = self.scroll(rows);
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height(&mut self, rows: &[Row], height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.reveal_selected(rows);
        }
    }

    /// Scrolls the least that shows the selected row; before the first draw (no height yet),
    /// waits for `set_height`.
    pub fn reveal_selected(&mut self, rows: &[Row]) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
        let Some(row) = self.selected_row(rows) else { return };
        let (top, height) = (self.scroll(rows), self.height);
        if row < top {
            self.top = row;
        } else if row >= top + height {
            self.top = row + 1 - height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::Exit;

    fn server(name: &str) -> Row {
        Row::Server(ServerRow {
            name: name.into(),
            languages: String::new(),
            root: None,
            state: ServerState::Running,
            progress: None,
            errors: 0,
        })
    }

    fn task(id: u64) -> Row {
        Row::Task {
            id: RunId(id),
            name: format!("t{id}"),
            status: Status::Running,
        }
    }

    #[test]
    fn the_selection_follows_its_key() {
        let mut v = ToolsView {
            height: 10,
            ..ToolsView::default()
        };
        let rows = vec![server("clangd"), task(1), task(2), Row::Jed { errors: 0 }];
        assert_eq!(v.selected_row(&rows), None);
        v.select(&rows, 2);
        assert_eq!(v.selected_key(), Some(&RowKey::Task(RunId(2))));
        // A server appearing above: the selection stays on run 2.
        let rows = vec![
            server("rust-analyzer"),
            server("clangd"),
            task(1),
            task(2),
            Row::Jed { errors: 0 },
        ];
        assert_eq!(v.selected_row(&rows), Some(3));
        v.settle(&rows);
        // Run 2 closed: the row now at its index (Jed), and it stays there.
        let rows = vec![
            server("rust-analyzer"),
            server("clangd"),
            task(1),
            Row::Jed { errors: 0 },
        ];
        assert_eq!(v.selected_row(&rows), Some(3));
        v.settle(&rows);
        assert_eq!(v.selected_key(), Some(&RowKey::Jed));
        let rows = vec![Row::Jed { errors: 0 }];
        v.settle(&rows);
        assert_eq!(v.selected_row(&rows), Some(0));
        v.settle(&[]);
        assert_eq!(v.selected_key(), None);
    }

    #[test]
    fn moving_and_scrolling_clamp() {
        let mut v = ToolsView {
            height: 3,
            ..ToolsView::default()
        };
        let rows: Vec<Row> = (0..10).map(task).collect();
        v.move_selection(&rows, 1);
        assert_eq!(v.selected_row(&rows), Some(0), "nothing selected: the first row");
        v.move_selection(&rows, 5);
        assert_eq!(v.selected_row(&rows), Some(5));
        assert_eq!(v.scroll(&rows), 3, "scrolled the least to show it");
        v.move_selection(&rows, 100);
        assert_eq!(v.selected_row(&rows), Some(9));
        assert_eq!(v.scroll(&rows), 7);
        v.move_selection(&rows, -100);
        assert_eq!((v.selected_row(&rows), v.scroll(&rows)), (Some(0), 0));
        v.scroll_by(&rows, 100);
        assert_eq!(v.scroll(&rows), 7);
        use crate::scrollbar::Scrollable;
        let s = v.scrollable(rows.len()).scroll();
        assert_eq!((s.top, s.max_top), (7, 7));
        v.scrollable(rows.len()).set_top(2);
        assert_eq!(v.scroll(&rows), 2);
    }

    #[test]
    fn rows_say_what_they_open() {
        assert_eq!(server("clangd").tool(), Some(Tool::Server("clangd".into())));
        assert_eq!(task(1).tool(), None);
        assert_eq!(Row::Jed { errors: 3 }.tool(), Some(Tool::Jed));
        assert_eq!(Row::Jed { errors: 3 }.errors(), 3);
        assert!(task(1).is(&RowKey::Task(RunId(1))) && !task(1).is(&RowKey::Jed));
    }

    #[test]
    fn a_servers_state_has_its_label_mark_and_colour() {
        assert_eq!(
            [
                ServerState::Starting,
                ServerState::Running,
                ServerState::Failed,
                ServerState::Exited
            ]
            .map(|s| (s.label(), s.glyph(), s.theme_key())),
            [
                ("starting", "○", "tools.starting"),
                ("running", "●", "tools.running"),
                ("failed", "✗", "tools.failed"),
                ("exited", "–", "tools.exited"),
            ]
        );
    }

    #[test]
    fn task_labels() {
        let exit = |code, signal, killed| Status::Exited(Exit { code, signal, killed });
        assert_eq!(task_label(&Status::Starting), "starting");
        assert_eq!(task_label(&Status::Running), "running");
        assert_eq!(task_label(&exit(0, None, false)), "exited 0");
        assert_eq!(task_label(&exit(2, None, false)), "exited 2");
        assert_eq!(task_label(&exit(1, Some("Hangup".into()), true)), "killed");
        assert_eq!(task_label(&exit(1, Some("Terminated".into()), false)), "Terminated");
        assert_eq!(task_label(&Status::Failed("no cwd".into())), "failed to start");
    }

    #[test]
    fn server_rows_list_live_stopped_and_logged_servers_once() {
        let manager = lsp::Manager::new(None);
        let errors = vec![
            (Tool::Jed, 4),
            (Tool::Server("clangd".into()), 2),
            (Tool::Server("ty".into()), 0),
        ];
        let rows = server_rows(&manager, &errors, Path::new("/w"));
        let names: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.state, r.errors)).collect();
        assert_eq!(
            names,
            [("clangd", ServerState::Exited, 2), ("ty", ServerState::Exited, 0)]
        );
    }

    /// Before the first draw (no height) a reveal waits for `set_height`.
    #[test]
    fn a_reveal_before_the_first_draw_waits_for_the_height() {
        let rows = vec![server("clangd"), task(1), task(2), task(3), Row::Jed { errors: 0 }];
        let mut v = ToolsView::default();
        v.select(&rows, 1);
        v.set_height(&rows, 2);
        assert_eq!((v.scroll(&rows), v.selected_row(&rows)), (0, Some(1)));

        let mut v = ToolsView::default();
        v.select(&rows, 3);
        assert_eq!(v.top, 0);
        v.set_height(&rows, 2);
        assert_eq!(v.scroll(&rows), 2, "the selected row at the bottom");
        v.top = 0;
        v.set_height(&rows, 3);
        assert_eq!(v.scroll(&rows), 0, "a resize doesn't reveal again");
        // Hidden (no rows drawn) again: the next reveal waits for the next draw.
        v.set_height(&rows, 0);
        v.select(&rows, 4);
        assert_eq!(v.top, 0);
        v.set_height(&rows, 2);
        assert_eq!(v.scroll(&rows), 3);
    }
}
