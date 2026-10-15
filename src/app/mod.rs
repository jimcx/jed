//! Event loop. Owns all state and sends commands where they need to go.

mod cursors;
mod files;
mod find;
mod mouse;
mod palette;
mod prompt;
mod side;
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
use crate::layout::{GroupId, HitMap, SidePanel, SplitDir};
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    /// The side panel: the explorer tree, or the Search view's query input (`ui.side_panel`).
    SidePanel,
    /// The find widget's input (`app.find.field` says which one).
    Find,
}

pub struct UiState {
    pub side_visible: bool,
    pub side_width: u16,
    pub side_panel: SidePanel,
    pub focus: Focus,
}

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
        result: Result<(), String>,
        elapsed: Duration,
    },
    Opened {
        path: PathBuf,
        result: Result<Buffer, LoadError>,
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
    /// Workspace search results; any generation (stale ones are dropped).
    SearchResults(crate::workspace_search::Batch),
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
            #[cfg(test)]
            last_command: None,
        };
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

    /// Background work that needs the runtime: watching and the first listing.
    fn start(&mut self) {
        let tx = self.tx.clone();
        match watcher::start(&self.workspace.root, move |paths| {
            let _ = tx.send(AppEvent::FsChanged(paths));
        }) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => self.notify(Level::Warning, format!("Not watching for file changes: {e}")),
        }
        let root = self.workspace.root.clone();
        self.request_listing(root);
        self.reveal_active();
    }

    pub async fn run(&mut self, terminal: &mut Term) -> anyhow::Result<()> {
        self.start();
        let mut events = EventStream::new();
        loop {
            if self.should_quit {
                return Ok(());
            }
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
                    None => return Ok(()),
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
    /// debounce), or forever when none is pending; `run` and `settle` both wait on it. The future
    /// borrows nothing, so it can sit in a `select!` beside `self.rx`.
    fn next_timer(&self) -> impl Future<Output = Timer> + 'static {
        let fs = self.fs_deadline.map(|d| (d, Timer::FsChanges));
        let search = self
            .search_view
            .deadline()
            .map(|d| (tokio::time::Instant::from_std(d), Timer::SearchDebounce));
        let next = match (fs, search) {
            (Some(f), Some(s)) => Some(if s.0 < f.0 { s } else { f }),
            (f, s) => f.or(s),
        };
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
        }
    }

    /// Replaces the keymap (the defaults plus `keybindings.toml`), reporting problems loading it.
    pub fn set_keymap(&mut self, keymap: Keymap, errors: Vec<String>) {
        self.keymap = keymap;
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
        let group = self.groups.active_group_mut();
        let tab = group.tabs.get_mut(group.active)?;
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
            Focus::Editor => Context::Editor,
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search && self.search_view.list_focused => {
                Context::SearchResults
            }
            Focus::SidePanel if self.ui.side_panel == SidePanel::Search => Context::Search,
            Focus::SidePanel => Context::Explorer,
            Focus::Find if self.find.field == Field::Replace => Context::Replace,
            Focus::Find => Context::Find,
        }
    }

    fn handle_event(&mut self, ev: Event) {
        self.needs_redraw = true;
        self.note_pre_event();
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                tracing::trace!(target: "input", ?key);
                self.message = None;
                if self.prompt.is_some() {
                    self.handle_prompt_key(key);
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
                    && let Some((buf, view)) = self.active_mut()
                {
                    edit::insert_char(buf, view, c);
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
    }

    /// Edits go through the active tab's view; every other view on an edited buffer
    /// (in any group) follows the changes so its cursors and viewport stay on the same text.
    /// Then records the active view's selection change for Cmd+U (`track_selections`).
    fn sync_views(&mut self) {
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
            for (gid, group) in self.groups.iter_mut() {
                for (i, tab) in group.tabs.iter_mut().enumerate() {
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
        self.track_selections(edited);
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
                if let Some(tab) = self.group().active_tab() {
                    self.save(tab.buffer);
                }
            }
            Command::Quit => self.request_quit(),
            Command::CloseTab => self.close_tab(self.groups.active, self.group().active),
            Command::NextTab => self.cycle_tab(1),
            Command::PrevTab => self.cycle_tab(-1),
            Command::ToggleSidebar => {
                self.ui.side_visible = !self.ui.side_visible;
                if !self.ui.side_visible {
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
            Command::Palette(c) => self.palette_command(c),
            Command::AddNextMatch | Command::SelectAllMatches | Command::SkipToNextMatch => self.match_command(cmd),
            Command::CursorUndo => self.cursor_undo(),
            Command::OpenFind => self.open_find(false),
            Command::OpenReplace => self.open_find(true),
            Command::Find(c) => self.find_command(c),
            Command::FindInFiles => self.find_in_files(),
            Command::Search(c) => self.search_command(c),
            // Esc in the editor closes the find widget before it drops cursors (VS Code).
            Command::CancelSelection if self.find.visible && self.ui.focus == Focus::Editor => self.close_find(),
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
        let group = self.groups.active_group_mut();
        if let Some(tab) = group.tabs.get_mut(group.active)
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
            AppEvent::SearchResults(batch) => self.search_results(batch),
        }
        self.refresh_find();
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

    /// Handles every background result already queued, without waiting (as the event loop
    /// would between frames).
    pub fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_app_event(msg);
        }
    }

    /// Handles background results (and the Search view's debounce) until `done` holds; false after 5 s.
    /// Rechecks every 20 ms too, for conditions outside the app (files written in the background).
    pub async fn settle(&mut self, done: impl Fn(&App) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done(self) {
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
