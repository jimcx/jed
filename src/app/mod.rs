//! Event loop. Owns all state and sends commands where they need to go.

mod completion;
mod cursors;
mod diff_view;
mod disk;
mod files;
mod find;
mod format;
mod git;
mod hover;
mod locations;
mod lsp;
mod mouse;
mod palette;
mod problems;
mod prompt;
mod quick_diff;
mod rename;
mod scm;
mod side;
mod syntax;
mod workspace_search;

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
use crate::edit::{self, Settings};
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
use crate::{logging, panic, search, ui};

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
    /// Edited buffers' gutter markers are due (`quick_diff::DELAY`).
    QuickDiff,
    /// Diff tabs of edited buffers are due (`diff_view::DELAY`).
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    /// The side panel: the explorer tree, or the Search view's query input (`ui.side_panel`).
    SidePanel,
    /// The find widget's input (`app.find.field` says which one).
    Find,
    /// The bottom panel (its tab, `ui.panel_tab`: the Problems list).
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
    },
    Opened {
        path: PathBuf,
        /// Boxed: a `Buffer` is several times bigger than any other event.
        result: Result<Box<Buffer>, LoadError>,
    },
    Listed {
        dir: PathBuf,
        result: std::io::Result<Vec<explorer::Entry>>,
    },
    FsChanged(Vec<PathBuf>),
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
    /// A background parse finished (`Syntax::parsed` drops a superseded one).
    Parsed {
        buffer: BufferId,
        parsed: crate::syntax::Parsed,
    },
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
    /// An open file was read to see whether it changed on disk (`app/disk.rs`).
    DiskChecked(Box<disk::Checked>),
    /// A buffer's gutter markers were computed (`app/quick_diff.rs`).
    QuickDiff {
        buffer: BufferId,
        computed: crate::quick_diff::Computed,
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
}

/// The file to open at startup, with an optional 1-based line and column.
pub struct InitialFile {
    pub buffer: Buffer,
    pub goto: Option<(usize, usize)>,
}

pub struct App {
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
    /// Quick open's file list; kept between openings so its threads are reused.
    files: Option<FileMatcher>,
    /// A `FilesMatched` event is queued.
    files_pending: Arc<AtomicBool>,
    /// Changes the log filter; `None` when logging is off (and in tests, unless set).
    pub log: Option<logging::LevelHandle>,
    pub log_dir: PathBuf,
    /// Holds `settings.toml` (for "Preferences: Open User Settings"); `paths::config_dir()`.
    pub config_dir: PathBuf,
    pub message: Option<Message>,
    after_save: Option<AfterSave>,
    /// A trust decision is needed but another prompt is open.
    trust_pending: bool,
    should_quit: bool,
    needs_redraw: bool,
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
    /// The Locations list (go to definition's places), while there is one.
    pub locations: Option<crate::locations::Locations>,
    /// Numbers Locations lists, so file reads for a replaced one are dropped.
    locations_generation: u64,
    /// The git status (branch, changed files) and its refresh.
    pub git: git::GitState,
    /// Open files' checks against the disk.
    pub disk: disk::DiskChecks,
    /// The gutter markers' schedule, the peek, stages waiting for a save.
    pub quick_diff: quick_diff::QuickDiffState,
    /// Diff tabs' ids, their reload delay, stages started from them.
    pub diffs: diff_view::DiffViews,
    /// The Source Control view: collapsed sections, scroll, selection.
    pub scm: crate::scm::ScmView,
    /// Stage All waiting for these buffers' saves to end.
    stage_all_after_save: Option<HashSet<BufferId>>,
    /// The last command `execute` ran, for tests.
    #[cfg(test)]
    pub last_command: Option<Command>,
}

impl App {
    /// Must not start background work: it may run before the runtime is entered. `start` does that.
    pub fn new(workspace: Workspace, trust_store: TrustStore, initial: Option<InitialFile>, kitty: bool) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let root = workspace.root.clone();
        let mut app = Self {
            trust: trust_store.evaluate(&root, &workspace.config),
            trust_store,
            settings: workspace.settings(),
            excludes: explorer::excludes(&root, &workspace.config.files.exclude),
            explorer: Explorer::new(root),
            workspace,
            buffers: Buffers::default(),
            groups: Groups::default(),
            ui: UiState {
                side_visible: true,
                side_width: 30,
                side_panel: SidePanel::Explorer,
                focus: Focus::Editor,
                panel_visible: false,
                panel_height: PANEL_HEIGHT,
                panel_tab: PanelTab::Problems,
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
            files: None,
            files_pending: Arc::new(AtomicBool::new(false)),
            log: None,
            log_dir: logging::log_dir(),
            config_dir: crate::paths::config_dir(),
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
            locations: None,
            locations_generation: 0,
            git: Default::default(),
            disk: Default::default(),
            quick_diff: Default::default(),
            diffs: Default::default(),
            scm: Default::default(),
            stage_all_after_save: None,
            #[cfg(test)]
            last_command: None,
        };
        app.load_associations();
        match initial {
            Some(f) => app.add_buffer(f.buffer, f.goto.map(|(l, c)| files::Goto::Cursor(l, c)), true),
            None => app.ui.focus = Focus::SidePanel,
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
        if app.trust == Trust::Undecided {
            app.ask_trust();
        }
        app
    }

    /// Background work that needs the runtime: watching, the first listing and git status,
    /// language servers.
    fn start(&mut self) {
        let tx = self.tx.clone();
        self.lsp.start(Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Lsp(ev));
        }));
        let tx = self.tx.clone();
        match watcher::start(&self.workspace.root, move |paths| {
            let _ = tx.send(AppEvent::FsChanged(paths));
        }) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => self.notify(Level::Warning, format!("Not watching for file changes: {e}")),
        }
        self.refresh_git(None);
        let root = self.workspace.root.clone();
        self.request_listing(root);
        self.reveal_active();
        self.sync_syntax();
        self.sync_servers();
        self.sync_quick_diff();
        self.sync_diffs();
    }

    pub async fn run(&mut self, terminal: &mut Term) -> anyhow::Result<()> {
        self.start();
        let mut events = EventStream::new();
        loop {
            if self.should_quit {
                self.shutdown_servers().await;
                return Ok(());
            }
            // Once per turn, so a burst of keys is one incremental re-parse.
            self.sync_syntax();
            self.sync_servers();
            self.sync_quick_diff();
            self.sync_diffs();
            if self.needs_redraw {
                let t = Instant::now();
                terminal.draw(|f| ui::render(f, self))?;
                if t.elapsed() > SLOW_FRAME {
                    tracing::warn!(elapsed_ms = t.elapsed().as_millis() as u64, "slow frame");
                }
                self.needs_redraw = false;
            }
            let timer = self.next_timer();
            tokio::select! {
                ev = events.next() => match ev {
                    Some(ev) => self.handle_event(ev?),
                    None => {
                        self.shutdown_servers().await;
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
    /// debounce, the mouse's hover rest, quick suggestions, the gutter markers' delay), or forever when none is pending; `run` and `settle` both
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
        let quick_diff = self
            .quick_diff
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::QuickDiff));
        let diff = self
            .diffs
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::Diff));
        let next = [fs, search, hover, completion, quick_diff, diff]
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
            Timer::QuickDiff => self.sync_quick_diff(),
            Timer::Diff => self.sync_diffs(),
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

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.group().active_tab()?.buffer)
    }

    fn context(&self) -> Context {
        if self.palette.is_some() {
            return Context::Palette;
        }
        match self.ui.focus {
            Focus::Editor if self.completion_shown() => Context::Suggest,
            Focus::Editor if self.group().active_diff().is_some() => Context::Diff,
            Focus::Editor => Context::Editor,
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search && self.search_view.list_focused => {
                Context::SearchResults
            }
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search => Context::Search,
            Focus::SidePanel if self.ui.side_panel == SidePanel::SourceControl => Context::Scm,
            Focus::SidePanel => Context::Explorer,
            Focus::Find if self.find.field == Field::Replace => Context::Replace,
            Focus::Find => Context::Find,
            Focus::Panel => match self.ui.panel_tab {
                PanelTab::Problems => Context::Problems,
                PanelTab::Locations => Context::Locations,
            },
        }
    }

    fn handle_event(&mut self, ev: Event) {
        self.needs_redraw = true;
        self.note_pre_event();
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
                } else if popup && esc && self.palette.is_none() && self.ui.focus == Focus::Editor {
                    // Closed above.
                } else if peek {
                    if enter {
                        self.stage_peek();
                    } else {
                        self.close_peek();
                    }
                } else if let Some(cmd) = self.keymap.lookup(&key, self.context()) {
                    self.execute(cmd);
                } else if self.palette.is_some() {
                    self.palette_key(key);
                } else if self.ui.focus == Focus::Find {
                    self.find_key(key);
                } else if self.search_focused() {
                    self.search_key(key);
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
                } else if self.prompt.is_none() && self.palette.is_some() {
                    self.palette_paste(&text);
                } else if self.prompt.is_none() && self.ui.focus == Focus::Find {
                    self.find_paste(&text);
                } else if self.prompt.is_none() && self.search_focused() {
                    self.search_paste(&text);
                } else if self.prompt.is_none() && self.ui.focus == Focus::Editor {
                    self.paste(&text);
                }
            }
            Event::Mouse(m) if self.prompt.is_none() && self.palette.is_some() => self.palette_mouse(m),
            Event::Mouse(m) if self.prompt.is_none() => self.handle_mouse(m),
            _ => {}
        }
        self.sync_views();
        self.refresh_find();
        self.check_completion();
        self.check_requests();
        self.check_hover();
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
        // Anything but the palette's own keys (and reopening it) closes it first, as losing focus does in VS Code.
        if self.palette.is_some()
            && !matches!(
                cmd,
                Command::Palette(_) | Command::ShowCommands | Command::QuickOpen | Command::GoToLine
            )
        {
            self.close_palette();
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
            Command::Explorer(c) => self.explorer_command(c),
            Command::Copy => self.copy(false),
            Command::Cut => self.copy(true),
            Command::Paste => self.paste_clipboard(),
            Command::ShowCommands => self.open_palette(Picker::QuickOpen, ">"),
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
            Command::NextChange | Command::PreviousChange if self.diff_command(cmd) => {}
            Command::NextChange => self.go_to_change(true),
            Command::PreviousChange => self.go_to_change(false),
            Command::CompareWithSaved => self.compare_with_saved(),
            Command::OpenChange => self.open_changes(),
            Command::StageFile | Command::UnstageFile if self.scm_focused() => {
                self.scm_stage(cmd == Command::UnstageFile)
            }
            Command::StageFile => self.stage_file_command(false),
            Command::UnstageFile => self.stage_file_command(true),
            Command::StageAll => self.stage_all(),
            Command::UnstageAll => self.unstage_all(),
            Command::FocusScm => self.focus_scm(),
            Command::Scm(c) => self.scm_command(c),
            // Esc in the editor closes the find widget before it drops cursors (VS Code).
            Command::CancelSelection if self.find.visible && self.ui.focus == Focus::Editor => self.close_find(),
            // A diff tab: its own movement keys; other editor commands do nothing.
            _ if self.diff_command(cmd) => {}
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
        match self.clipboard.read() {
            Ok(text) => self.paste(&text),
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
    }

    /// Pastes into the active view: a whole-line copy goes above the cursor's line, and red's
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
        if panel == SidePanel::Explorer && self.explorer.selected.is_none() {
            self.explorer.select_index(0);
        }
    }

    fn handle_app_event(&mut self, ev: AppEvent) {
        self.needs_redraw = true;
        match ev {
            AppEvent::SaveDone {
                buffer,
                state,
                result,
                elapsed,
            } => self.save_done(buffer, state, result, elapsed),
            AppEvent::Opened { path, result } => self.opened(path, result),
            AppEvent::Listed { dir, result } => self.listed(dir, result),
            AppEvent::FsChanged(paths) => {
                self.fs_pending.extend(paths);
                self.fs_deadline
                    .get_or_insert_with(|| tokio::time::Instant::now() + FS_DEBOUNCE);
            }
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
            AppEvent::Parsed { buffer, parsed } => self.parsed(buffer, parsed),
            AppEvent::Lsp(ev) => self.lsp_event(ev),
            AppEvent::LocationsRead { generation, files } => self.locations_read(generation, files),
            AppEvent::GitRefreshed { result, elapsed } => self.git_refreshed(result, elapsed),
            AppEvent::DiskChecked(checked) => self.disk_checked(*checked),
            AppEvent::QuickDiff { buffer, computed } => self.quick_diff_done(buffer, computed),
            AppEvent::Staged { at, unstage, result } => self.staged(at, unstage, result),
            AppEvent::DiffLoaded { id, generation, result } => self.diff_loaded(id, generation, *result),
            AppEvent::DiffStaged(message) => self.diff_staged(message),
        }
        self.refresh_find();
        self.check_completion();
        self.check_hover();
        self.check_peek();
    }

    fn update_crash_snapshot(&self) {
        let snapshots = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_dirty())
            .map(|(_, b)| panic::Snapshot {
                path: b.path.clone(),
                rope: b.rope.clone(),
                bom: b.bom,
            })
            .collect();
        panic::set_dirty(snapshots);
    }
}

#[cfg(test)]
impl App {
    pub fn test_start(&mut self) {
        self.start();
    }

    /// Opens `buffer` in a new active tab, as a finished file open would (no file I/O).
    pub fn test_open(&mut self, buffer: Buffer) {
        self.add_buffer(buffer, None, true);
    }

    pub fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.handle_event(Event::Key(crossterm::event::KeyEvent::new(code, mods)));
    }

    /// Runs a command as a key bound to it would.
    pub fn test_execute(&mut self, cmd: Command) {
        self.execute(cmd);
        self.sync_views();
        self.refresh_find();
        self.check_completion();
        self.check_requests();
        self.check_hover();
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

    /// Fires the quick-suggestion timer as the event loop would at `now`.
    pub fn test_completion_tick(&mut self, now: Instant) {
        self.completion_tick(now);
    }

    /// Takes in file texts read for the Locations list `generation`, as if the background read
    /// had sent them (to test that a replaced list's reads are dropped).
    pub fn test_locations_read(&mut self, generation: u64, files: Vec<(PathBuf, ropey::Rope)>) {
        self.handle_app_event(AppEvent::LocationsRead { generation, files });
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

    /// Handles every background result already queued, without waiting (as the event loop
    /// would between frames).
    pub fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_app_event(msg);
        }
        self.sync_syntax();
        self.sync_servers();
        self.sync_quick_diff();
        self.sync_diffs();
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
            self.sync_diffs();
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
