//! Event loop. Owns all state and sends commands where they need to go.

mod backup;
mod blame;
mod cheatsheet;
mod code_action;
mod commit_tabs;
mod completion;
mod cursors;
mod diff_view;
mod disk;
mod field;
mod files;
mod find;
mod format;
mod git;
mod git_history;
mod hex;
mod history;
mod hover;
mod indentation;
mod locations;
mod lsp;
mod mode;
mod mouse;
mod navigation;
mod palette;
mod problems;
mod prompt;
mod quick_diff;
mod recent;
mod rename;
mod replace;
mod scm;
mod scrollbar;
mod session;
mod side;
mod syntax;
mod tasks;
mod tools;
mod workspace_search;

pub use mode::Mode;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ignore::overrides::Override;
use tokio::sync::mpsc;

use crate::buffer::file::LoadError;
use crate::buffer::{Buffer, BufferId, Buffers};
use crate::clipboard::{self, Clipboard};
use crate::command::Command;
use crate::config::keymap::{Context, Keymap, Platform};
use crate::edit::{self, RenderWhitespace, Settings};
use crate::explorer::{self, Explorer};
use crate::find::{Field, FindWidget};
use crate::group::{Group, Groups};
use crate::layout::{GroupId, HitMap, PanelTab, SidePanel, SplitDir};
use crate::palette::{FileMatcher, Palette, Picker};
use crate::search_view::SearchView;
use crate::terminal::Term;
use crate::theme::{self, Theme};
use crate::view::View;
use crate::watcher::{self, Watcher};
use crate::workspace::Workspace;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::{Trust, TrustStore};
use crate::{logging, search, ui};

pub use mouse::ClickTracker;
pub use prompt::{InputKind, Prompt};

const SLOW_FRAME: Duration = Duration::from_millis(16);
const SLOW_TASK: Duration = Duration::from_millis(500);
/// File system events are batched for this long before acting on them.
const FS_DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

pub struct Message {
    pub text: String,
    pub level: Level,
}

/// Which deadline `App::next_timer` woke for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timer {
    FsChanges,
    SearchDebounce,
    /// The mouse has rested on text for `hover::DELAY`.
    Hover,
    /// A word char was typed `completion::QUICK_DELAY` ago.
    Completion,
    /// The cursor rested `code_action::BULB_DELAY`: the lightbulb's request.
    Bulb,
    /// Edited buffers' gutter markers are due (`quick_diff::DELAY`).
    QuickDiff,
    /// Diff tabs of edited buffers are due (`diff_view::DELAY`).
    Diff,
    /// Edited buffers' inline blame is due (`blame::DELAY`).
    Blame,
    /// Dirty buffers changed since their backup are due (`backup::INTERVAL`).
    Backup,
    /// Log tabs may wake the loop again (`tools::LOG_FRAME` after the last wake).
    Logs,
    /// Quick open's matching was still running at the last tick (`FILES_POLL`).
    Files,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    /// The side panel: the explorer tree, or the Search view's query input (`ui.side_panel`).
    SidePanel,
    /// The find widget's input (`app.find.field` says which one).
    Find,
    /// The bottom panel (its tab, `ui.panel_tab`: the Problems list, the Locations list, a task).
    Panel,
}

pub struct UiState {
    pub side_visible: bool,
    pub side_width: u16,
    pub side_panel: SidePanel,
    pub focus: Focus,
    /// The bottom panel: shown, its height (border and tab strip included; clamped by
    /// `layout::compute`) and which tab it shows.
    pub panel_visible: bool,
    pub panel_height: u16,
    pub panel_tab: PanelTab,
    /// "View: Toggle Render Whitespace" flipped the `render_whitespace` setting for this session
    /// (`App::render_whitespace`). Never written to a settings file; a `.workspace` reload keeps it.
    pub whitespace_toggled: bool,
    /// The scroll bar whose thumb is under the pointer (drawn in its hover colour).
    pub scrollbar_hover: Option<crate::layout::ScrollTarget>,
}

/// The bottom panel's height until it's resized.
pub const PANEL_HEIGHT: u16 = 10;

/// What to do once pending saves finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterSave {
    Quit,
    Close(GroupId, BufferId),
}

/// Explorer file operations, run off the UI thread.
#[derive(Debug, Clone)]
enum FileOp {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

/// Results from background work.
enum AppEvent {
    SaveDone {
        buffer: BufferId,
        state: u64,
        /// What was written.
        result: Result<crate::buffer::file::Stamp, crate::buffer::SaveError>,
        elapsed: Duration,
        /// A file history snapshot of it is being recorded: `HistoryDone` follows.
        history: bool,
    },
    Opened {
        path: PathBuf,
        result: Result<files::Loaded, LoadError>,
    },
    /// Find's whole-file count on a hex tab (`HexTab::id`) for one query generation
    /// (`find::HexFind`); an older generation's is dropped.
    HexCount {
        tab: u64,
        generation: u64,
        count: crate::hex::Count,
    },
    Listed {
        dir: PathBuf,
        result: std::io::Result<Vec<explorer::Entry>>,
    },
    FsChanged(Vec<PathBuf>),
    /// A file history task (a save's snapshot, a rename's move) ended; `Err` = the message.
    HistoryDone(Result<(), String>),
    /// A file's history entries were read for the picker.
    HistoryListed(Box<history::Listed>),
    /// A history entry was read and diffed against its buffer, to restore it.
    HistoryRestore(Box<history::Restore>),
    ConfigLoaded(Result<WorkspaceConfig, String>),
    FileOpDone {
        op: FileOp,
        result: Result<(), String>,
    },
    /// Quick open's matcher has new results (sent at most once until handled).
    FilesMatched,
    /// Quick open's walk finished.
    FilesWalked {
        generation: u64,
    },
    LogFileFound(Option<PathBuf>),
    /// `settings.toml` exists (created from the template if it didn't), or why it couldn't be.
    UserSettingsReady(Result<PathBuf, String>),
    /// Workspace search results; any generation (stale ones are dropped).
    SearchResults(crate::workspace_search::Batch),
    /// The Search view's Replace rewrote the files that aren't open (`app/replace.rs`).
    Replaced(Box<replace::FilesDone>),
    /// A background parse finished (`Syntax::parsed` drops a superseded one).
    Parsed {
        buffer: BufferId,
        parsed: crate::syntax::Parsed,
    },
    /// A syntax plugin's grammar was loaded (or why not) by `load_grammars`.
    Grammar {
        language: crate::syntax::Language,
        /// What building did (none: nothing to build, or the plugin was only loaded).
        build: Option<crate::syntax::build::Build>,
        /// A forced rebuild of a grammar this process already uses: the new library is used from
        /// the next start.
        replaced_in_use: bool,
        result: Result<(), String>,
    },
    /// `load_grammars` starts on this plugin: its build is running.
    GrammarBuilding(crate::syntax::plugin::PluginId),
    /// From a language server's task.
    Lsp(crate::lsp::Event),
    /// Files read for the Locations list's previews (`generation`: the list they're for).
    LocationsRead {
        generation: u64,
        files: Vec<(PathBuf, ropey::Rope)>,
    },
    /// A git status refresh finished (`app/git.rs`).
    GitRefreshed {
        result: Result<crate::git::Refreshed, String>,
        elapsed: Duration,
    },
    /// A page of the active file's commits was read (`app/git_history.rs`).
    GitHistory {
        path: PathBuf,
        skip: usize,
        generation: u64,
        result: Result<crate::git::log::Page, String>,
    },
    /// An open file was read to see whether it changed on disk (`app/disk.rs`).
    DiskChecked(Box<disk::Checked>),
    /// A hex tab's file was read to see whether it changed on disk (`app/disk.rs`).
    HexChecked {
        path: PathBuf,
        result: std::io::Result<crate::hex::OnDisk>,
    },
    /// A buffer's gutter markers were computed (`app/quick_diff.rs`).
    QuickDiff {
        buffer: BufferId,
        computed: crate::quick_diff::Computed,
    },
    /// A buffer's inline blame was computed (`app/blame.rs`).
    Blame {
        buffer: BufferId,
        computed: crate::blame::Computed,
    },
    /// A change was staged or unstaged (or wasn't).
    Staged {
        at: quick_diff::StageAt,
        unstage: bool,
        result: Result<Option<crate::git::Hunk>, String>,
    },
    /// A diff tab's texts were read and diffed (`app/diff_view.rs`).
    DiffLoaded {
        id: u64,
        generation: u64,
        result: Box<Result<crate::diff_view::Doc, String>>,
    },
    /// A stage / unstage from a diff tab (or of a whole file) ended: the message to show.
    DiffStaged(Result<String, String>),
    /// A commit tab's `git show` was read (`app/commit_tabs.rs`).
    CommitLoaded {
        commit: String,
        result: Result<crate::git::show::Show, String>,
    },
    /// A task run started, has new output (throttled) or ended (`app/tasks.rs`).
    Task(crate::tasks::Event),
    /// The workspace's session was read, with its text tabs' files (`app/session.rs`).
    SessionRead(Box<Result<Option<crate::session::Restore>, String>>),
    /// The workspace's crash backups were looked for at start (`app/backup.rs`).
    BackupsFound(Vec<crate::backup::Backup>),
    /// A backup job (removals, writes) ended.
    BackupDone(backup::Done),
    /// A backup was diffed against its buffer, to restore it.
    BackupRestore(Box<backup::Restoring>),
    /// The workspace was recorded in the recent workspaces list (`app/recent.rs`).
    RecentRecorded(Result<(), String>),
    /// The recent workspaces were read for "Open Recent Workspace".
    RecentListed(Box<recent::Listed>),
    /// A tool log got lines since the app last rearmed the store's waker (`app/tools.rs`).
    LogsChanged,
}

/// The file to open at startup, with an optional 1-based line and column.
pub struct InitialFile {
    pub buffer: Buffer,
    pub goto: Option<(usize, usize)>,
}

pub struct App {
    /// Workspace or single file mode (`main`'s `-s`); `mode.allows(cmd)` gates commands and views.
    pub mode: Mode,
    pub workspace: Workspace,
    pub trust: Trust,
    trust_store: TrustStore,
    pub buffers: Buffers,
    pub groups: Groups,
    pub explorer: Explorer,
    excludes: Override,
    pub ui: UiState,
    /// Where things were drawn in the last frame, for mouse hit-testing.
    pub hits: HitMap,
    pub keymap: Keymap,
    pub theme: Theme,
    pub settings: Settings,
    pub clipboard: Clipboard,
    pub prompt: Option<Prompt>,
    /// Command palette / quick open, while open. Below a prompt: a prompt gets keys first.
    pub palette: Option<Palette>,
    /// The keyboard cheat sheet overlay, while open (`app/cheatsheet.rs`): it takes every key
    /// and mouse event a prompt doesn't.
    pub cheat_sheet: Option<crate::cheatsheet::CheatSheet>,
    /// Quick open's file list; kept between openings so its threads are reused.
    files: Option<FileMatcher>,
    /// A `FilesMatched` event is queued.
    files_pending: Arc<AtomicBool>,
    /// When to tick quick open's matcher again: its last tick found matching still running,
    /// and nucleo can miss the wake-up for that run (it checks `should_notify` before our
    /// tick sets it), so polling is what takes in its results.
    files_poll: Option<Instant>,
    /// Changes the log filter; `None` when logging is off (and in tests, unless set).
    pub log: Option<logging::LevelHandle>,
    pub log_dir: PathBuf,
    /// The tool logs (language servers', "jed"'s); `main` puts in the store its `tracing`
    /// layer feeds. `start` hands it to the language servers and, with a `state_dir`, gives it
    /// `<state dir>/logs/tools` to write to.
    pub logs: crate::logs::Logs,
    /// Log tabs' wake-up throttle (`app/tools.rs`).
    pub log_tabs: tools::LogTabs,
    /// Holds `settings.toml` (for "Preferences: Open User Settings"); `paths::config_dir()`.
    pub config_dir: PathBuf,
    /// Where the session file goes (`paths::state_dir()`); `None` turns sessions off (tests,
    /// unless set before `test_start`).
    pub state_dir: Option<PathBuf>,
    /// The session's restore at start and save at quit.
    pub session: session::SessionState,
    /// Crash backups: the periodic writes, removals, the start's offer.
    pub backups: backup::BackupState,
    /// Recent workspaces: the start's record, the picker's entries.
    pub recent: recent::RecentState,
    pub message: Option<Message>,
    after_save: Option<AfterSave>,
    /// A trust decision is needed but another prompt is open.
    trust_pending: bool,
    should_quit: bool,
    /// Also set by drawing itself (the horizontal bar's row appeared or went: draw again).
    pub(crate) needs_redraw: bool,
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    watcher: Option<Watcher>,
    fs_pending: HashSet<PathBuf>,
    fs_deadline: Option<tokio::time::Instant>,
    listing: HashSet<PathBuf>,
    /// Files being opened, and where their selection goes once loaded.
    opening: HashMap<PathBuf, files::OpenRequest>,
    clicks: ClickTracker,
    drag: Option<mouse::Drag>,
    /// The mouse pointer's shape wanted (`run` sends it when it changes).
    pub pointer: crate::terminal::Pointer,
    /// The cursor shape the terminal has (`terminal::enter` sent the settings'; `run` sends a
    /// new one through `sync_cursor_shape` when a `.workspace` reload changes it).
    cursor_shape: crate::terminal::CursorShape,
    /// The find widget's options (case, whole word, regex), which Cmd+D also uses for selected
    /// text (VS Code's `selectedTextMatchMode: findOptions`; it ignores `regex`).
    pub find_options: search::Options,
    /// The find / replace widget; its query outlives closing it.
    pub find: FindWidget,
    /// The running Cmd+D / Cmd+Shift+L / Cmd+Alt+D session.
    match_session: Option<cursors::MatchSession>,
    /// A match command ran during this event (its selection change keeps the session).
    session_changed: bool,
    pre_event: Option<cursors::PreEvent>,
    /// The last workspace search's results (`None` before the first, or after an invalid regex).
    pub search: Option<crate::workspace_search::Results>,
    /// The Search view: its query, options, debounce and scroll.
    pub search_view: SearchView,
    /// The running workspace search; dropping it cancels it.
    search_task: Option<crate::workspace_search::Handle>,
    /// Numbers searches, so results of a replaced one are dropped.
    search_generation: u64,
    /// `.workspace` `[files] associations`, compiled.
    associations: crate::syntax::Associations,
    /// The Problems list's scroll and selection.
    pub problems: crate::problems::ProblemsView,
    /// Where the last F8 / Shift+F8 went: file, problem index, cursor position (so a cursor
    /// still there goes on from that problem).
    last_problem: Option<(PathBuf, usize, crate::buffer::pos::ByteIdx)>,
    /// Language servers.
    pub lsp: crate::lsp::Manager,
    /// Open files' languages, the config or trust changed: `sync_servers` has work to do.
    servers_dirty: bool,
    /// Language server requests waiting for their answers, by server and request id.
    requests: HashMap<(crate::lsp::ServerId, i64), lsp::Pending>,
    /// The hover popup and the mouse's resting timer.
    pub hover: crate::hover::HoverState,
    /// The completion list, its pending request and the quick-suggestion timer.
    pub completion: crate::completion::Completion,
    /// The code action list, the lightbulb's rest and cache, the hover's quick fixes.
    pub code_actions: crate::code_action::CodeActions,
    /// The Locations list (go to definition's places), while there is one.
    pub locations: Option<crate::locations::Locations>,
    /// Numbers Locations lists, so file reads for a replaced one are dropped.
    locations_generation: u64,
    /// The git status (branch, changed files) and its refresh.
    pub git: git::GitState,
    /// Open files' checks against the disk.
    pub disk: disk::DiskChecks,
    /// File history tasks (snapshots after saves, moves after renames).
    pub file_history: history::HistoryTasks,
    /// The Search view's Replace (`app/replace.rs`): files being rewritten off-thread.
    pub replace: replace::ReplaceState,
    /// The gutter markers' schedule, the peek, stages waiting for a save.
    pub quick_diff: quick_diff::QuickDiffState,
    /// Inline blame's toggle and its recompute delay.
    pub blame: blame::BlameState,
    /// Diff tabs' ids, their reload delay, stages started from them.
    pub diffs: diff_view::DiffViews,
    /// The Source Control view: collapsed sections, scroll, selection.
    pub scm: crate::scm::ScmView,
    /// The Tools view's list (`app/tools.rs`, drawn by `ui::tools`).
    pub tools_view: crate::tools_view::ToolsView,
    /// The Explorer view's "Git History" section: the active file's commits and the section's
    /// state (`app/git_history.rs`).
    pub git_history: crate::git_history::GitHistory,
    /// Stage All waiting for these buffers' saves to end.
    stage_all_after_save: Option<HashSet<BufferId>>,
    /// Task templates from `tasks.toml` and the task runs (`app/tasks.rs`).
    pub tasks: crate::tasks::Tasks,
    /// The syntax plugins whose grammars `start` loads (`app/syntax.rs`): `main` sets every
    /// registered one; tests name their own (the registry is global). Empty by default.
    pub syntax_plugins: Vec<crate::syntax::plugin::PluginId>,
    /// Each syntax plugin's grammar as the Tools view shows it (`app/syntax.rs`); empty until
    /// `load_grammars` queues them.
    pub grammars: Vec<(String, crate::tools_view::GrammarState)>,
    /// Back / forward navigation's history (`app/navigation.rs`).
    pub nav: navigation::NavState,
    /// The last command `execute` ran, for tests.
    #[cfg(test)]
    pub last_command: Option<Command>,
    /// Every path `flush_fs_changes` handled, for tests.
    #[cfg(test)]
    pub fs_flushed: Vec<PathBuf>,
}

impl App {
    /// `new_in` workspace mode (tests; `main` passes its mode).
    #[cfg(test)]
    pub fn new(workspace: Workspace, trust_store: TrustStore, initial: Option<InitialFile>, kitty: bool) -> Self {
        Self::new_in(Mode::Workspace, workspace, trust_store, initial, kitty)
    }

    /// Must not start background work: it may run before the runtime is entered. `start` does that.
    /// Single file mode starts with the side panel hidden and asks no trust.
    pub fn new_in(
        mode: Mode,
        workspace: Workspace,
        trust_store: TrustStore,
        initial: Option<InitialFile>,
        kitty: bool,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let root = workspace.root.clone();
        let settings = workspace.settings();
        let mut app = Self {
            mode,
            trust: trust_store.evaluate(&root, &workspace.config),
            trust_store,
            cursor_shape: crate::terminal::CursorShape::of(&settings),
            settings,
            excludes: explorer::excludes(&root, &workspace.config.files.exclude),
            explorer: Explorer::new(root),
            workspace,
            buffers: Buffers::default(),
            groups: Groups::default(),
            ui: UiState {
                side_visible: !mode.is_single(),
                side_width: 30,
                side_panel: SidePanel::Explorer,
                focus: Focus::Editor,
                panel_visible: false,
                panel_height: PANEL_HEIGHT,
                panel_tab: PanelTab::Problems,
                whitespace_toggled: false,
                scrollbar_hover: None,
            },
            hits: HitMap::default(),
            keymap: Keymap::default_for(Platform::current()),
            theme: Theme::dark_plus(theme::terminal_has_truecolor()),
            #[cfg(not(test))]
            clipboard: Clipboard::system(),
            #[cfg(test)]
            clipboard: Clipboard::memory(),
            prompt: None,
            palette: None,
            cheat_sheet: None,
            files: None,
            files_pending: Arc::new(AtomicBool::new(false)),
            files_poll: None,
            log: None,
            log_dir: logging::log_dir(),
            logs: Default::default(),
            log_tabs: Default::default(),
            config_dir: crate::paths::config_dir(),
            #[cfg(not(test))]
            state_dir: Some(crate::paths::state_dir()),
            #[cfg(test)]
            state_dir: None,
            session: Default::default(),
            backups: Default::default(),
            recent: Default::default(),
            message: None,
            after_save: None,
            trust_pending: false,
            should_quit: false,
            needs_redraw: true,
            tx,
            rx,
            watcher: None,
            fs_pending: HashSet::new(),
            fs_deadline: None,
            listing: HashSet::new(),
            opening: HashMap::new(),
            clicks: ClickTracker::default(),
            drag: None,
            pointer: crate::terminal::Pointer::Default,
            find_options: search::Options::default(),
            find: FindWidget::default(),
            match_session: None,
            session_changed: false,
            pre_event: None,
            search: None,
            search_view: SearchView::default(),
            search_task: None,
            search_generation: 0,
            associations: crate::syntax::Associations::default(),
            problems: Default::default(),
            last_problem: None,
            // Tests run no servers unless they plug in a launcher (`lsp.set_launcher`).
            #[cfg(not(test))]
            lsp: crate::lsp::Manager::new(Some(Arc::new(crate::lsp::ProcessLauncher)))
                .with_support_dir(crate::paths::state_dir().join("lsp")),
            #[cfg(test)]
            lsp: crate::lsp::Manager::new(None),
            servers_dirty: true,
            requests: HashMap::new(),
            hover: Default::default(),
            completion: Default::default(),
            code_actions: Default::default(),
            locations: None,
            locations_generation: 0,
            git: Default::default(),
            disk: Default::default(),
            file_history: Default::default(),
            replace: Default::default(),
            quick_diff: Default::default(),
            blame: Default::default(),
            diffs: Default::default(),
            scm: Default::default(),
            tools_view: Default::default(),
            git_history: Default::default(),
            stage_all_after_save: None,
            tasks: Default::default(),
            syntax_plugins: Vec::new(),
            grammars: Vec::new(),
            nav: Default::default(),
            #[cfg(test)]
            last_command: None,
            #[cfg(test)]
            fs_flushed: Vec::new(),
        };
        app.load_associations();
        match initial {
            Some(f) => {
                app.session.initial_goto = f.goto.is_some();
                app.add_buffer(f.buffer, f.goto.map(|(l, c)| files::Goto::Cursor(l, c)), true)
            }
            None if !mode.is_single() => app.ui.focus = Focus::SidePanel,
            None => {}
        }
        if !kitty {
            app.notify(
                Level::Warning,
                "Terminal lacks the kitty keyboard protocol: some keys won't work. Quit with Ctrl+Q".into(),
            );
        }
        if let Some(e) = app.workspace.config_error.clone() {
            app.notify(Level::Error, e);
        }
        // Single file mode runs no servers or tasks: nothing to trust.
        if app.trust == Trust::Undecided && !mode.is_single() {
            app.ask_trust();
        }
        app
    }

    /// Background work that needs the runtime: watching, the first listing and git status,
    /// language servers.
    fn start(&mut self) {
        self.start_tool_logs();
        self.load_grammars();
        if self.mode.is_single() {
            self.start_single();
            return;
        }
        let tx = self.tx.clone();
        self.lsp.start(Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Lsp(ev));
        }));
        let tx = self.tx.clone();
        self.tasks.start(Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Task(ev));
        }));
        let tx = self.tx.clone();
        match watcher::start(&self.workspace.root, move |paths| {
            let _ = tx.send(AppEvent::FsChanged(paths));
        }) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => self.notify(Level::Warning, format!("Not watching for file changes: {e}")),
        }
        self.refresh_git(None);
        self.restore_session();
        self.scan_backups();
        self.record_recent();
        let root = self.workspace.root.clone();
        self.request_listing(root);
        self.reveal_active();
        self.sync_syntax();
        self.sync_servers();
        self.sync_quick_diff();
        self.sync_blame();
        self.sync_diffs();
        self.sync_commit_tabs();
        self.sync_git_history();
    }

    /// `start` in single file mode: only the file is watched (for disk changes) and crash
    /// backups scanned; no servers, tasks, git, session, recent entry or listing.
    fn start_single(&mut self) {
        if let Some(path) = self.active_path() {
            let tx = self.tx.clone();
            match watcher::start_file(&path, move |paths| {
                let _ = tx.send(AppEvent::FsChanged(paths));
            }) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => self.notify(Level::Warning, format!("Not watching for file changes: {e}")),
            }
        }
        self.scan_backups();
        self.reveal_active();
        self.sync_syntax();
    }

    /// Draws a frame when one was asked for. A frame can ask for one more (a horizontal bar's
    /// row appeared or went, `ui::render`): that one is drawn now, not at the next event.
    pub(crate) fn redraw<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
    ) -> Result<(), B::Error> {
        for _ in 0..2 {
            if !std::mem::take(&mut self.needs_redraw) {
                break;
            }
            terminal.draw(|f| ui::render(f, self))?;
        }
        Ok(())
    }

    pub async fn run(&mut self, terminal: &mut Term) -> anyhow::Result<()> {
        self.start();
        let mut events = EventStream::new();
        let mut pointer = crate::terminal::Pointer::Default;
        loop {
            if self.should_quit {
                self.shutdown().await;
                return Ok(());
            }
            // Once per turn, so a burst of keys is one incremental re-parse.
            self.sync_syntax();
            self.sync_servers();
            self.sync_quick_diff();
            self.sync_blame();
            self.sync_diffs();
            self.sync_commit_tabs();
            self.sync_git_history();
            self.sync_backups();
            self.report_log_errors();
            self.sync_logs();
            if self.needs_redraw {
                let t = Instant::now();
                self.redraw(terminal)?;
                if t.elapsed() > SLOW_FRAME {
                    tracing::warn!(elapsed_ms = t.elapsed().as_millis() as u64, "slow frame");
                }
            }
            if self.pointer != pointer {
                pointer = self.pointer;
                crate::terminal::set_pointer(terminal.backend_mut(), pointer)?;
            }
            self.sync_cursor_shape(terminal.backend_mut())?;
            let timer = self.next_timer();
            tokio::select! {
                ev = events.next() => match ev {
                    Some(ev) => self.handle_event(ev?),
                    None => {
                        self.shutdown().await;
                        return Ok(());
                    }
                },
                Some(msg) = self.rx.recv() => self.handle_app_event(msg),
                t = timer => self.on_timer(t),
            }
            // Drain whatever else is already queued, so a burst becomes one frame.
            // Polled from this task (not `now_or_never`), so the stream keeps our real waker.
            loop {
                tokio::select! {
                    biased;
                    Some(ev) = events.next() => self.handle_event(ev?),
                    _ = std::future::ready(()) => break,
                }
            }
            self.update_crash_snapshot();
        }
    }

    /// Sleeps until the earliest pending deadline (the fs-event batch, the Search view's
    /// debounce, the mouse's hover rest, quick suggestions, the gutter markers', diff tabs' and
    /// inline blame's delays, crash backups), or forever when none is pending; `run` and `settle` both
    /// wait on it. The future borrows nothing, so it can sit in a `select!` beside `self.rx`.
    fn next_timer(&self) -> impl Future<Output = Timer> + 'static {
        let fs = self.fs_deadline.map(|d| (d, Timer::FsChanges));
        let search = self
            .search_view
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::SearchDebounce));
        let hover = self
            .hover
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Hover));
        let completion = self
            .completion
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Completion));
        let bulb = self
            .code_actions
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Bulb));
        let quick_diff = self
            .quick_diff
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::QuickDiff));
        let diff = self
            .diffs
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Diff));
        let blame = self
            .blame
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Blame));
        let backup = self
            .backups
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Backup));
        let logs = self
            .log_tabs
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Logs));
        let files = self
            .files_poll
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Files));
        let next = [
            fs, search, hover, completion, bulb, quick_diff, diff, blame, backup, logs, files,
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(at, _)| *at);
        async move {
            match next {
                Some((at, timer)) => {
                    tokio::time::sleep_until(at).await;
                    timer
                }
                None => std::future::pending().await,
            }
        }
    }

    /// A deadline from `next_timer` passed.
    fn on_timer(&mut self, timer: Timer) {
        match timer {
            Timer::FsChanges => self.flush_fs_changes(),
            Timer::SearchDebounce => self.search_tick(Instant::now()),
            Timer::Hover => self.hover_tick(Instant::now()),
            Timer::Completion => self.completion_tick(Instant::now()),
            Timer::Bulb => self.bulb_tick(Instant::now()),
            Timer::QuickDiff => self.sync_quick_diff(),
            Timer::Diff => self.sync_diffs(),
            Timer::Blame => self.sync_blame(),
            Timer::Backup => self.backup_tick(Instant::now()),
            Timer::Logs => self.logs_tick(),
            Timer::Files => self.files_matched(),
        }
    }

    /// Replaces the keymap (the defaults plus `keybindings.toml`), reporting problems loading it.
    pub fn set_keymap(&mut self, keymap: Keymap, errors: Vec<String>) {
        self.keymap = keymap;
        self.report_errors(errors);
    }

    /// Shows the first of some problems loading a config file (and how many more there are).
    pub fn report_errors(&mut self, errors: Vec<String>) {
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            self.notify(Level::Error, format!("{first}{more}"));
        }
    }

    fn notify(&mut self, level: Level, text: String) {
        match level {
            Level::Info => tracing::info!("{text}"),
            Level::Warning => tracing::warn!("{text}"),
            Level::Error => tracing::error!("{text}"),
        }
        self.message = Some(Message { text, level });
        self.needs_redraw = true;
    }

    /// The active tab's buffer and view.
    /// The active editor group.
    pub fn group(&self) -> &Group {
        self.groups.active_group()
    }

    pub fn group_mut(&mut self) -> &mut Group {
        self.groups.active_group_mut()
    }

    pub fn active_mut(&mut self) -> Option<(&mut Buffer, &mut View)> {
        let tab = self.groups.active_group_mut().active_tab_mut()?;
        let buf = self.buffers.get_mut(tab.buffer)?;
        Some((buf, &mut tab.view))
    }

    /// Sends the settings' cursor shape (DECSCUSR) to `out` when it isn't the one sent last.
    pub fn sync_cursor_shape(&mut self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let shape = crate::terminal::CursorShape::of(&self.settings);
        if shape != self.cursor_shape {
            crate::terminal::set_cursor_shape(out, shape)?;
            self.cursor_shape = shape;
        }
        Ok(())
    }

    /// The whitespace the editor draws: the `render_whitespace` setting, or — after "View: Toggle
    /// Render Whitespace" — `none`, or the setting's mode again (`all` if that is `none`).
    pub fn render_whitespace(&self) -> RenderWhitespace {
        match (self.settings.render_whitespace, self.ui.whitespace_toggled) {
            (mode, false) => mode,
            (RenderWhitespace::None, true) => RenderWhitespace::All,
            (_, true) => RenderWhitespace::None,
        }
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.group().active_tab()?.buffer)
    }

    pub(super) fn context(&self) -> Context {
        if self.palette.is_some() {
            return Context::Palette;
        }
        match self.ui.focus {
            Focus::Editor if self.code_actions_shown() => Context::CodeActions,
            Focus::Editor if self.completion_shown() => Context::Suggest,
            Focus::Editor if self.group().active_diff().is_some() => Context::Diff,
            Focus::Editor if self.group().active_hex().is_some() => Context::Hex,
            Focus::Editor if self.active_commit().is_some() => Context::Commit,
            Focus::Editor => Context::Editor,
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search && self.search_view.list_focused => {
                Context::SearchResults
            }
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search => Context::Search,
            Focus::SidePanel if self.ui.side_panel == SidePanel::SourceControl => Context::Scm,
            Focus::SidePanel if self.ui.side_panel == SidePanel::Tools => Context::Tools,
            // The history section (in the Explorer) comes before the tree's context.
            Focus::SidePanel if self.git_history_focused() => Context::GitHistory,
            Focus::SidePanel => Context::Explorer,
            Focus::Find if self.find.field == Field::Replace => Context::Replace,
            Focus::Find => Context::Find,
            Focus::Panel => match self.ui.panel_tab {
                PanelTab::Problems => Context::Problems,
                PanelTab::Locations => Context::Locations,
                PanelTab::Task(_) => Context::Task,
            },
        }
    }

    fn handle_event(&mut self, ev: Event) {
        self.needs_redraw = true;
        self.note_pre_event();
        let here = self.nav_begin();
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                tracing::trace!(target: "input", ?key);
                self.message = None;
                // Any key closes the hover popup; Esc in the editor does only that.
                let popup = self.hover.popup.is_some();
                if !matches!(key.code, KeyCode::Modifier(_)) {
                    self.close_hover();
                }
                let esc = key.code == KeyCode::Esc && key.modifiers.is_empty();
                let enter = key.code == KeyCode::Enter && key.modifiers.is_empty();
                // The gutter's peek takes Enter (stage) and Esc in the editor; other keys close it.
                let peek = self.quick_diff.peek.is_some()
                    && self.prompt.is_none()
                    && self.palette.is_none()
                    && self.ui.focus == Focus::Editor
                    && (esc || enter);
                if !peek && !matches!(key.code, KeyCode::Modifier(_)) {
                    self.close_peek();
                }
                if self.prompt.is_some() {
                    self.handle_prompt_key(key);
                } else if self.cheat_sheet.is_some() {
                    self.cheat_sheet_key(key);
                } else if popup && esc && self.palette.is_none() && self.ui.focus == Focus::Editor {
                    // Closed above.
                } else if peek {
                    if enter {
                        self.stage_peek();
                    } else {
                        self.close_peek();
                    }
                } else if let Some(cmd) = self.keymap.lookup(&key, self.context())
                    && !(self.field_focused() && field::has_field_meaning(cmd))
                {
                    self.execute(cmd);
                } else if self.palette.is_some() {
                    self.palette_key(key);
                } else if self.ui.focus == Focus::Find {
                    self.find_key(key);
                } else if self.search_focused() {
                    self.search_key(key);
                } else if self.task_focused().is_some() {
                    self.task_key(key);
                } else if let KeyCode::Char(c) = key.code
                    && (key.modifiers - KeyModifiers::SHIFT).is_empty()
                    && self.ui.focus == Focus::Editor
                    && self.active_writable()
                {
                    // A commit character inserts the selected completion first.
                    self.completion_commit(c);
                    let settings = self.settings.clone();
                    if let Some((buf, view)) = self.active_mut() {
                        edit::insert_char(buf, view, c, &settings);
                    }
                    self.completion_typed(c);
                }
            }
            Event::Paste(text) => {
                tracing::trace!(target: "input", len = text.len(), "paste");
                if let Some(Prompt::Input(input)) = &mut self.prompt {
                    input.insert(&text.replace(['\r', '\n'], ""));
                } else if self.cheat_sheet.is_some() {
                    // Swallowed.
                } else if self.prompt.is_none() && self.palette.is_some() {
                    self.palette_paste(&text);
                } else if self.prompt.is_none() && self.ui.focus == Focus::Find {
                    self.find_paste(&text);
                } else if self.prompt.is_none() && self.search_focused() {
                    self.search_paste(&text);
                } else if self.prompt.is_none() && self.task_focused().is_some() {
                    self.task_paste(&text);
                } else if self.prompt.is_none() && self.ui.focus == Focus::Editor {
                    self.paste(&text);
                }
            }
            Event::Mouse(m) if self.prompt.is_none() && self.cheat_sheet.is_some() => self.cheat_sheet_mouse(m),
            Event::Mouse(m) if self.prompt.is_none() && self.palette.is_some() => self.palette_mouse(m),
            Event::Mouse(m) if self.prompt.is_none() => self.handle_mouse(m),
            _ => {}
        }
        self.nav_end(here, true);
        self.sync_views();
        self.refresh_find();
        self.check_completion();
        self.check_requests();
        self.check_hover();
        self.check_code_actions();
        self.check_peek();
    }

    /// Edits go through the active tab's view; every other view on an edited buffer
    /// (in any group) follows the changes so its cursors and viewport stay on the same text.
    /// Then records the active view's selection change for Cmd+U (`track_selections`).
    fn sync_views(&mut self) {
        let edited = self.follow_edits();
        self.track_selections(edited);
    }

    /// `sync_views` without the Cmd+U / Cmd+D tracking (for edits made by background results:
    /// a rename's `WorkspaceEdit`). True when the active view's buffer changed. The active view
    /// itself only re-anchors its viewport: whoever edited set its selections.
    fn follow_edits(&mut self) -> bool {
        let origin = self.groups.active;
        let origin_tab = self.group().active;
        let origin_buf = self.group().active_tab().map(|t| t.buffer);
        let mut edited = false;
        for (id, buf) in self.buffers.iter_mut() {
            let changes = buf.take_applied();
            if changes.is_empty() {
                continue;
            }
            edited |= Some(id) == origin_buf;
            self.completion.follow(id, &changes, buf.sets_applied());
            self.quick_diff.follow(id, &changes);
            self.nav.history.map(&buf.path, &changes);
            for (gid, group) in self.groups.iter_mut() {
                for (i, tab) in group.text_tabs_mut() {
                    if tab.buffer != id {
                        continue;
                    }
                    if gid == origin && i == origin_tab {
                        // The editing view: keep its viewport anchor current.
                        tab.view.set_top_line(&buf.rope, tab.view.top_line());
                    } else {
                        tab.view.map_through(&changes, &buf.rope);
                    }
                }
            }
        }
        edited
    }

    fn execute(&mut self, cmd: Command) {
        let _span = tracing::debug_span!("command", name = %cmd.name()).entered();
        let t = Instant::now();
        #[cfg(test)]
        {
            self.last_command = Some(cmd);
        }
        if !self.mode.allows(cmd) {
            self.notify(Level::Info, mode::UNAVAILABLE.into());
            return;
        }
        // Anything but the palette's own keys (and reopening it) closes it first, as losing focus does in VS Code.
        if self.palette.is_some()
            && !matches!(
                cmd,
                Command::Palette(_) | Command::ShowCommands | Command::QuickOpen | Command::GoToLine
            )
        {
            self.close_palette();
        }
        if matches!(
            cmd,
            Command::Move {
                motion: crate::command::Motion::DocStart | crate::command::Motion::DocEnd,
                ..
            }
        ) {
            self.nav_jump();
        }
        match cmd {
            Command::Save => {
                // A diff tab saves its file's buffer.
                if let Some(id) = self.active_file_buffer() {
                    self.save(id);
                }
            }
            Command::Quit => self.request_quit(),
            Command::CloseTab => self.close_tab(self.groups.active, self.group().active),
            Command::NextTab => self.cycle_tab(1),
            Command::PrevTab => self.cycle_tab(-1),
            Command::ToggleSidebar => {
                self.ui.side_visible = !self.ui.side_visible;
                if !self.ui.side_visible && self.ui.focus == Focus::SidePanel {
                    self.ui.focus = Focus::Editor;
                }
            }
            Command::FocusExplorer => self.focus_side(SidePanel::Explorer),
            Command::FocusGroup(n) => {
                let ids = self.groups.ids();
                match ids.get(n as usize - 1) {
                    Some(&g) => self.focus_group(g),
                    None if n as usize == ids.len() + 1 => self.split(SplitDir::Right),
                    None => {}
                }
            }
            Command::SplitRight => self.split(SplitDir::Right),
            Command::SplitDown => self.split(SplitDir::Down),
            Command::MoveTabToNextGroup => self.move_tab_to_group(true),
            Command::MoveTabToPreviousGroup => self.move_tab_to_group(false),
            Command::MoveTabLeft => self.move_tab(-1),
            Command::MoveTabRight => self.move_tab(1),
            Command::Explorer(c) => self.explorer_command(c),
            Command::Copy => self.copy(false),
            Command::Cut => self.copy(true),
            Command::Paste => self.paste_clipboard(),
            Command::ShowCommands => self.open_palette(Picker::QuickOpen, ">"),
            Command::CheatSheet => self.toggle_cheat_sheet(),
            Command::QuickOpen => self.open_palette(Picker::QuickOpen, ""),
            Command::GoToLine => self.open_palette(Picker::QuickOpen, ":"),
            Command::SetLogLevel => self.open_palette(Picker::LogLevel, ""),
            Command::ManageWorkspaceTrust => self.manage_trust(),
            Command::OpenLogFile => self.open_log_file(),
            Command::OpenUserSettings => self.open_user_settings(),
            Command::Palette(c) => self.palette_command(c),
            Command::AddNextMatch | Command::SelectAllMatches | Command::SkipToNextMatch => self.match_command(cmd),
            Command::CursorUndo => self.cursor_undo(),
            Command::OpenFind => self.open_find(false),
            Command::OpenReplace => self.open_find(true),
            Command::Find(c) => self.find_command(c),
            Command::FindInFiles => self.find_in_files(),
            Command::Search(c) => self.search_command(c),
            Command::TogglePanel => self.toggle_panel(),
            Command::ToggleProblems => self.toggle_problems(),
            Command::Problems(c) => self.problems_command(c),
            Command::ShowHover => self.show_hover(),
            Command::RevealDefinition => self.go_to_definition(),
            Command::Locations(c) => self.locations_command(c),
            Command::TriggerSuggest => self.trigger_suggest(),
            Command::Suggest(c) => self.suggest_command(c),
            Command::QuickFix => self.quick_fix(),
            Command::CodeActions(c) => self.code_action_command(c),
            Command::GoToReferences => self.go_to_references(),
            Command::RenameSymbol => self.rename_symbol(),
            Command::FormatDocument => self.format_document(),
            Command::ReloadFromDisk => self.reload_from_disk(),
            Command::KeepMine => self.keep_mine(),
            Command::StageHunk | Command::UnstageHunk if self.group().active_diff().is_some() => {
                self.diff_stage(cmd == Command::UnstageHunk, None)
            }
            Command::StageHunk => self.stage_command(false),
            Command::UnstageHunk => self.stage_command(true),
            Command::RevertHunk => self.diff_revert(None),
            Command::NextChange | Command::PreviousChange if self.diff_command(cmd) => {}
            Command::NextChange => self.go_to_change(true),
            Command::PreviousChange => self.go_to_change(false),
            Command::CompareWithSaved => self.compare_with_saved(),
            Command::OpenChange => self.open_changes(),
            Command::ToggleChanges => self.toggle_changes(),
            Command::StageFile | Command::UnstageFile if self.scm_focused() => {
                self.scm_stage(cmd == Command::UnstageFile)
            }
            Command::StageFile => self.stage_file_command(false),
            Command::UnstageFile => self.stage_file_command(true),
            Command::StageAll => self.stage_all(),
            Command::UnstageAll => self.unstage_all(),
            Command::FocusScm => self.focus_scm(),
            Command::Scm(c) => self.scm_command(c),
            Command::GitHistory(c) => self.git_history_command(c),
            Command::Commit(crate::command::CommitCommand::Open) => self.open_commit_line(),
            Command::ShowFileHistory => self.show_file_history(false),
            Command::RestoreFileHistory => self.show_file_history(true),
            Command::RunTask => self.show_run_task(),
            Command::OpenRecentWorkspace => self.show_recent(),
            Command::ToggleRenderWhitespace => self.ui.whitespace_toggled = !self.ui.whitespace_toggled,
            Command::IndentUsingSpaces
            | Command::IndentUsingTabs
            | Command::ChangeTabDisplaySize
            | Command::DetectIndentation => self.indentation_command(cmd),
            Command::ConvertIndentationToSpaces => self.convert_indentation(true),
            Command::ConvertIndentationToTabs => self.convert_indentation(false),
            Command::ReopenAsHex => self.reopen_as_hex(),
            Command::ReopenAsText => self.reopen_as_text(),
            Command::ToggleBlame => self.toggle_blame(),
            Command::KillTask => self.kill_task(),
            Command::CloseTask => self.close_shown_task(),
            Command::Task(c) => self.task_command(c),
            Command::NavBack => self.navigate(crate::navigation::Dir::Back),
            Command::NavForward => self.navigate(crate::navigation::Dir::Forward),
            Command::OpenToolLog => self.show_log_picker(),
            Command::ClearToolLog => self.clear_tool_log(),
            Command::ShowTools => self.focus_side(SidePanel::Tools),
            Command::Tools(c) => self.tools_command(c),
            Command::ToggleToolTrace => self.toggle_tool_trace(),
            Command::RestartServer => self.restart_server(),
            Command::RebuildSyntaxPlugins => self.rebuild_grammars(),
            // Esc in the editor closes the find widget before it drops cursors (VS Code).
            Command::CancelSelection if self.find.visible && self.ui.focus == Focus::Editor => self.close_find(),
            // A diff or hex tab: its own movement keys; other editor commands do nothing.
            _ if self.diff_command(cmd) => {}
            _ if self.hex_command(cmd) => {}
            _ => {
                if edit::is_edit(cmd) && !self.active_writable() {
                    return;
                }
                let settings = self.settings.clone();
                if let Some((buf, view)) = self.active_mut() {
                    edit::run(cmd, buf, view, &settings);
                }
            }
        }
        tracing::debug!(elapsed_us = t.elapsed().as_micros() as u64);
    }

    /// Copies (or cuts) the active view's selections to the clipboard.
    fn copy(&mut self, cut: bool) {
        if !cut && self.hex_copy() {
            return;
        }
        if cut && !self.active_writable() {
            return;
        }
        let Some((buf, view)) = self.active_mut() else {
            return;
        };
        let copied = edit::copy_text(buf, view);
        if cut {
            edit::cut(buf, view);
        }
        self.clipboard.write(copied);
    }

    fn paste_clipboard(&mut self) {
        if let Some(text) = self.read_clipboard() {
            self.paste(&text);
        }
    }

    /// The system clipboard's text; `None` with a message when it can't be read.
    fn read_clipboard(&mut self) -> Option<String> {
        match self.clipboard.read() {
            Ok(text) => return Some(text),
            Err(clipboard::ReadError::Unavailable) => {
                let key = match Platform::current() {
                    Platform::MacOs => "Cmd+V",
                    Platform::Linux => "Ctrl+Shift+V",
                };
                self.notify(
                    Level::Info,
                    format!("No system clipboard here: paste with the terminal ({key})"),
                );
            }
            Err(clipboard::ReadError::Failed(e)) => self.notify(Level::Error, format!("Clipboard read failed: {e}")),
        }
        None
    }

    /// Pastes into the active view: a whole-line copy goes above the cursor's line, and jed's
    /// own copy of several selections spreads one per cursor when the counts match.
    fn paste(&mut self, text: &str) {
        if !self.active_writable() {
            return;
        }
        let copied = self.clipboard.copied(text);
        let (whole_line, pieces) = copied.map_or((false, &[][..]), |c| (c.whole_line, c.pieces.as_slice()));
        if let Some(tab) = self.groups.active_group_mut().active_tab_mut()
            && let Some(buf) = self.buffers.get_mut(tab.buffer)
        {
            edit::paste(buf, &mut tab.view, text, whole_line, pieces);
        }
    }

    /// False (with a message) if the active buffer is read-only; false if there's none.
    fn active_writable(&mut self) -> bool {
        match self.active_buffer().map(|b| b.read_only.clone()) {
            Some(None) => true,
            Some(Some(reason)) => {
                self.notify(Level::Error, format!("Read-only: {reason}"));
                false
            }
            None => false,
        }
    }

    fn focus_side(&mut self, panel: SidePanel) {
        self.ui.side_visible = true;
        self.ui.side_panel = panel;
        self.ui.focus = Focus::SidePanel;
        self.search_view.list_focused = false;
        if panel == SidePanel::SourceControl {
            self.scm_select_first();
        }
        if panel == SidePanel::Tools {
            self.tools_select_first();
        }
        if panel == SidePanel::Explorer {
            // Focusing the Explorer gives the keys to the tree, not the history section.
            self.git_history.set_focused(false);
            if self.explorer.selected.is_none() {
                self.explorer.select_index(0);
            }
        }
    }

    fn handle_app_event(&mut self, ev: AppEvent) {
        // New log lines redraw only what shows them: log tabs take them in at the turn's
        // `sync_logs`; the Tools view only when its error badges changed.
        if let AppEvent::LogsChanged = ev {
            if self.logs_changed() {
                self.needs_redraw = true;
            }
            return;
        }
        self.needs_redraw = true;
        // A session's restore replaces the layout: not a jump.
        let here = match ev {
            AppEvent::SessionRead(_) => None,
            _ => self.nav_begin(),
        };
        match ev {
            AppEvent::SaveDone {
                buffer,
                state,
                result,
                elapsed,
                history,
            } => {
                if history {
                    self.history_started();
                }
                self.save_done(buffer, state, result, elapsed)
            }
            AppEvent::Opened { path, result } => self.opened(path, result),
            AppEvent::HexCount { tab, generation, count } => self.hex_counted(tab, generation, count),
            AppEvent::Listed { dir, result } => self.listed(dir, result),
            AppEvent::FsChanged(paths) => self.fs_changed(paths),
            AppEvent::HistoryDone(result) => self.history_done(result),
            AppEvent::HistoryListed(listed) => self.history_listed(*listed),
            AppEvent::HistoryRestore(restore) => self.history_restored(*restore),
            AppEvent::ConfigLoaded(result) => self.config_loaded(result),
            AppEvent::FileOpDone { op, result } => self.file_op_done(op, result),
            AppEvent::FilesMatched => self.files_matched(),
            AppEvent::FilesWalked { generation } => self.files_walked(generation),
            AppEvent::LogFileFound(path) => self.log_file_found(path),
            AppEvent::UserSettingsReady(result) => match result {
                Ok(path) => self.open_file(path, true, None),
                Err(e) => self.notify(Level::Error, e),
            },
            AppEvent::SearchResults(batch) => self.search_results(batch),
            AppEvent::Replaced(done) => self.files_replaced(*done),
            AppEvent::Parsed { buffer, parsed } => self.parsed(buffer, parsed),
            AppEvent::Grammar {
                language,
                build,
                replaced_in_use,
                result,
            } => self.grammar_loaded(language, build, replaced_in_use, result),
            AppEvent::GrammarBuilding(p) => self.grammar_building(p),
            AppEvent::Lsp(ev) => self.lsp_event(ev),
            AppEvent::LocationsRead { generation, files } => self.locations_read(generation, files),
            AppEvent::GitRefreshed { result, elapsed } => self.git_refreshed(result, elapsed),
            AppEvent::GitHistory {
                path,
                skip,
                generation,
                result,
            } => self.git_history_done(path, skip, generation, result),
            AppEvent::DiskChecked(checked) => self.disk_checked(*checked),
            AppEvent::HexChecked { path, result } => self.hex_checked(path, result),
            AppEvent::QuickDiff { buffer, computed } => self.quick_diff_done(buffer, computed),
            AppEvent::Blame { buffer, computed } => self.blame_done(buffer, computed),
            AppEvent::Staged { at, unstage, result } => self.staged(at, unstage, result),
            AppEvent::DiffLoaded { id, generation, result } => self.diff_loaded(id, generation, *result),
            AppEvent::DiffStaged(message) => self.diff_staged(message),
            AppEvent::CommitLoaded { commit, result } => self.commit_loaded(commit, result),
            AppEvent::Task(ev) => self.task_event(ev),
            AppEvent::SessionRead(result) => self.session_read(*result),
            AppEvent::BackupsFound(found) => self.backups_found(found),
            AppEvent::BackupDone(done) => self.backup_done(done),
            AppEvent::BackupRestore(r) => self.backup_restored(*r),
            AppEvent::RecentRecorded(result) => self.recent_recorded(result),
            AppEvent::RecentListed(listed) => self.recent_listed(*listed),
            AppEvent::LogsChanged => {} // handled above: no redraw, no navigation
        }
        self.nav_end(here, false);
        self.refresh_find();
        self.check_completion();
        self.check_hover();
        self.check_code_actions();
        self.check_peek();
    }
}

#[cfg(test)]
impl App {
    pub fn test_start(&mut self) {
        self.start();
    }

    /// Whitespace drawn as plain blanks, as before `render_whitespace` (default `all`): for tests
    /// that read drawn text with spaces in it. Uses the session toggle, so it lasts through
    /// `.workspace` reloads (while the setting isn't changed to `none`).
    pub fn test_plain_whitespace(&mut self) {
        self.ui.whitespace_toggled = self.settings.render_whitespace != RenderWhitespace::None;
    }

    /// Opens `buffer` in a new active tab, as a finished file open would (no file I/O).
    pub fn test_open(&mut self, buffer: Buffer) {
        self.add_buffer(buffer, None, true);
    }

    pub fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.handle_event(Event::Key(crossterm::event::KeyEvent::new(code, mods)));
    }

    /// Presses the platform's file start key: Cmd+Up on macOS (VS Code's; Cmd+Home is unbound
    /// there), Ctrl+Home on Linux.
    pub fn press_file_start(&mut self) {
        if cfg!(target_os = "macos") {
            self.press(KeyCode::Up, KeyModifiers::SUPER);
        } else {
            self.press(KeyCode::Home, KeyModifiers::CONTROL);
        }
    }

    /// Presses the platform's file end key: Cmd+Down on macOS, Ctrl+End on Linux.
    pub fn press_file_end(&mut self) {
        if cfg!(target_os = "macos") {
            self.press(KeyCode::Down, KeyModifiers::SUPER);
        } else {
            self.press(KeyCode::End, KeyModifiers::CONTROL);
        }
    }

    /// Runs a command as a key bound to it would.
    pub fn test_execute(&mut self, cmd: Command) {
        let here = self.nav_begin();
        self.execute(cmd);
        self.nav_end(here, true);
        self.sync_views();
        self.refresh_find();
        self.check_completion();
        self.check_requests();
        self.check_hover();
        self.check_code_actions();
        self.check_peek();
    }

    pub fn handle_test_event(&mut self, ev: Event) {
        self.handle_event(ev);
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    pub fn mouse(&mut self, kind: crossterm::event::MouseEventKind, x: u16, y: u16) {
        self.mouse_with(kind, x, y, KeyModifiers::NONE);
    }

    /// A mouse event with modifiers held (Alt+Click, Shift+Alt+drag).
    pub fn mouse_with(&mut self, kind: crossterm::event::MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) {
        self.handle_event(Event::Mouse(crossterm::event::MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers,
        }));
    }

    pub fn quitting(&self) -> bool {
        self.should_quit
    }

    /// Takes in a workspace search batch as if the running search had sent it.
    pub fn test_search_batch(&mut self, batch: crate::workspace_search::Batch) {
        self.handle_app_event(AppEvent::SearchResults(batch));
    }

    /// Fires the hover resting timer as the event loop would at `now` (no sleeping in tests).
    pub fn test_hover_tick(&mut self, now: Instant) {
        self.hover_tick(now);
    }

    /// Fires the backup timer as the event loop would `after` from now (after the turn's
    /// `sync_backups`, which sets the deadline): with `backup::INTERVAL`, what's due is written.
    pub fn test_backup_tick(&mut self, after: Duration) {
        self.sync_backups();
        self.backup_tick(Instant::now() + after);
    }

    /// What the panic hook backs up if jed panics now: the loop turn's crash snapshot, then the
    /// hook's own write (`panic::back_up_dirty`), returning the files written.
    pub fn test_panic_backup(&self) -> Vec<PathBuf> {
        self.update_crash_snapshot();
        crate::panic::back_up_dirty()
    }

    /// Fires the quick-suggestion timer as the event loop would at `now`.
    pub fn test_completion_tick(&mut self, now: Instant) {
        self.completion_tick(now);
    }

    /// Takes in file texts read for the Locations list `generation`, as if the background read
    /// had sent them (to test that a replaced list's reads are dropped).
    pub fn test_locations_read(&mut self, generation: u64, files: Vec<(PathBuf, ropey::Rope)>) {
        self.handle_app_event(AppEvent::LocationsRead { generation, files });
    }

    /// Takes in a page of a file's commits for `path` and `generation`, as if the background
    /// read had sent it (to test that another file's or a replaced list's pages are dropped).
    pub fn test_git_history_page(
        &mut self,
        path: PathBuf,
        skip: usize,
        generation: u64,
        result: Result<crate::git::log::Page, String>,
    ) {
        self.git_history_done(path, skip, generation, result);
    }

    /// Language server requests waiting for answers: (method kind, buffer position) pairs.
    pub fn pending_requests(&self) -> Vec<(&'static str, usize)> {
        let mut v: Vec<_> = self
            .requests
            .values()
            .map(|p| {
                let kind = match p.request {
                    lsp::Request::Hover { word: None } => "hover",
                    lsp::Request::Hover { word: Some(_) } => "mouse hover",
                    lsp::Request::Definition { .. } => "definition",
                    lsp::Request::Completion { .. } => "completion",
                    lsp::Request::Resolve { accepted: None, .. } => "resolve",
                    lsp::Request::Resolve { accepted: Some(_), .. } => "accepted resolve",
                    lsp::Request::References { .. } => "references",
                    lsp::Request::PrepareRename { .. } => "prepare rename",
                    lsp::Request::Rename { .. } => "rename",
                    lsp::Request::Format => "format",
                    lsp::Request::CodeActions {
                        purpose: crate::code_action::Purpose::List,
                        ..
                    } => "code actions",
                    lsp::Request::CodeActions {
                        purpose: crate::code_action::Purpose::Bulb,
                        ..
                    } => "lightbulb",
                    lsp::Request::CodeActions {
                        purpose: crate::code_action::Purpose::Hover,
                        ..
                    } => "hover fixes",
                    lsp::Request::ResolveAction => "resolve action",
                    lsp::Request::ExecuteCommand { .. } => "execute command",
                };
                (kind, p.at.0)
            })
            .collect();
        v.sort();
        v
    }

    /// Takes in a language server event as if it had come through the channel, before the turn's
    /// `sync_servers` (as when a key and the event arrive in one loop turn).
    pub fn test_lsp_event(&mut self, event: crate::lsp::Event) {
        self.handle_app_event(AppEvent::Lsp(event));
    }

    /// Takes in a hex find count as if it had come through the channel (`AppEvent::HexCount`):
    /// for tests of stale counts.
    pub fn test_hex_count(&mut self, tab: u64, generation: u64, count: crate::hex::Count) {
        self.handle_app_event(AppEvent::HexCount { tab, generation, count });
    }

    /// Handles every background result already queued, without waiting (as the event loop
    /// would between frames).
    pub fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_app_event(msg);
        }
        self.sync_syntax();
        self.sync_servers();
        self.sync_quick_diff();
        self.sync_blame();
        self.sync_diffs();
        self.sync_commit_tabs();
        self.sync_git_history();
        self.sync_backups();
        self.report_log_errors();
        self.sync_logs();
    }

    /// Whether something asked for a redraw since the last call (clears it, as a frame would).
    pub fn test_take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.needs_redraw)
    }

    /// One turn of `run` without a terminal: waits up to `within` for an app event or the
    /// earliest timer as `run`'s `select!` does (no polling, unlike `settle`, so a test sees
    /// what really wakes the loop), handles it and what else is queued, then runs the turn's
    /// syncs (`pump`). False when nothing woke it in time.
    pub async fn test_turn(&mut self, within: Duration) -> bool {
        let timer = self.next_timer();
        let woke = tokio::select! {
            Some(msg) = self.rx.recv() => {
                self.handle_app_event(msg);
                true
            }
            t = timer => {
                self.on_timer(t);
                true
            }
            _ = tokio::time::sleep(within) => false,
        };
        self.pump();
        woke
    }

    /// Handles background results (and the Search view's debounce) until `done` holds; false after 5 s.
    /// Rechecks every 20 ms too, for conditions outside the app (files written in the background).
    pub async fn settle(&mut self, done: impl Fn(&App) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            // As the event loop does each turn.
            self.sync_syntax();
            self.sync_servers();
            self.sync_quick_diff();
            self.sync_blame();
            self.sync_diffs();
            self.sync_commit_tabs();
            self.sync_git_history();
            self.sync_backups();
            self.report_log_errors();
            self.sync_logs();
            if done(self) {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            let timer = self.next_timer();
            tokio::select! {
                Some(msg) = self.rx.recv() => self.handle_app_event(msg),
                t = timer => self.on_timer(t),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        true
    }
}
