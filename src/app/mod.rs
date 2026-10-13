//! Event loop. Owns all state and sends commands where they need to go.

mod files;
mod mouse;
mod prompt;
mod side;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ignore::overrides::Override;
use tokio::sync::mpsc;

use crate::buffer::file::LoadError;
use crate::buffer::{Buffer, BufferId, Buffers};
use crate::command::Command;
use crate::config::keymap::{Context, Keymap, Platform};
use crate::edit::{self, Settings};
use crate::explorer::{self, Explorer};
use crate::group::Group;
use crate::layout::{HitMap, SidePanel};
use crate::terminal::Term;
use crate::theme::{self, Theme};
use crate::view::View;
use crate::watcher::{self, Watcher};
use crate::workspace::Workspace;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::{Trust, TrustStore};
use crate::{panic, ui};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    SidePanel,
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
    Close(BufferId),
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
        focus: bool,
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
    pub group: Group,
    pub explorer: Explorer,
    excludes: Override,
    pub ui: UiState,
    /// Where things were drawn in the last frame, for mouse hit-testing.
    pub hits: HitMap,
    pub keymap: Keymap,
    pub theme: Theme,
    pub settings: Settings,
    pub prompt: Option<Prompt>,
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
    opening: HashSet<PathBuf>,
    clicks: ClickTracker,
    drag: Option<mouse::Drag>,
}

impl App {
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
            group: Group::default(),
            ui: UiState {
                side_visible: true,
                side_width: 30,
                side_panel: SidePanel::Explorer,
                focus: Focus::Editor,
            },
            hits: HitMap::default(),
            keymap: Keymap::default_for(Platform::current()),
            theme: Theme::dark_plus(theme::terminal_has_truecolor()),
            prompt: None,
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
            opening: HashSet::new(),
            clicks: ClickTracker::default(),
            drag: None,
        };
        match initial {
            Some(f) => app.add_buffer(f.buffer, f.goto, true),
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
            let deadline = self.fs_deadline;
            tokio::select! {
                ev = events.next() => match ev {
                    Some(ev) => self.handle_event(ev?),
                    None => return Ok(()),
                },
                Some(msg) = self.rx.recv() => self.handle_app_event(msg),
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                    self.flush_fs_changes();
                }
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
    pub fn active_mut(&mut self) -> Option<(&mut Buffer, &mut View)> {
        let tab = self.group.tabs.get_mut(self.group.active)?;
        let buf = self.buffers.get_mut(tab.buffer)?;
        Some((buf, &mut tab.view))
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.buffers.get(self.group.active_tab()?.buffer)
    }

    fn context(&self) -> Context {
        match self.ui.focus {
            Focus::Editor => Context::Editor,
            Focus::SidePanel => Context::Explorer,
        }
    }

    fn handle_event(&mut self, ev: Event) {
        self.needs_redraw = true;
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                tracing::trace!(target: "input", ?key);
                self.message = None;
                if self.prompt.is_some() {
                    self.handle_prompt_key(key);
                } else if let Some(cmd) = self.keymap.lookup(&key, self.context()) {
                    self.execute(cmd);
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
                } else if self.prompt.is_none()
                    && self.ui.focus == Focus::Editor
                    && self.active_writable()
                    && let Some((buf, view)) = self.active_mut()
                {
                    edit::paste(buf, view, &text);
                }
            }
            Event::Mouse(m) if self.prompt.is_none() => self.handle_mouse(m),
            _ => {}
        }
    }

    fn execute(&mut self, cmd: Command) {
        let _span = tracing::debug_span!("command", name = %cmd.name()).entered();
        let t = Instant::now();
        match cmd {
            Command::Save => {
                if let Some(tab) = self.group.active_tab() {
                    self.save(tab.buffer);
                }
            }
            Command::Quit => self.request_quit(),
            Command::CloseTab => self.close_tab(self.group.active),
            Command::NextTab => self.cycle_tab(1),
            Command::PrevTab => self.cycle_tab(-1),
            Command::ToggleSidebar => {
                self.ui.side_visible = !self.ui.side_visible;
                if !self.ui.side_visible {
                    self.ui.focus = Focus::Editor;
                }
            }
            Command::FocusExplorer => self.focus_side(SidePanel::Explorer),
            Command::FocusEditor => self.ui.focus = Focus::Editor,
            Command::Explorer(c) => self.explorer_command(c),
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
            AppEvent::Opened { path, result, focus } => self.opened(path, result, focus),
            AppEvent::Listed { dir, result } => self.listed(dir, result),
            AppEvent::FsChanged(paths) => {
                self.fs_pending.extend(paths);
                self.fs_deadline
                    .get_or_insert_with(|| tokio::time::Instant::now() + FS_DEBOUNCE);
            }
            AppEvent::ConfigLoaded(result) => self.config_loaded(result),
            AppEvent::FileOpDone { op, result } => self.file_op_done(op, result),
        }
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

    pub fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.handle_event(Event::Key(crossterm::event::KeyEvent::new(code, mods)));
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    pub fn quitting(&self) -> bool {
        self.should_quit
    }

    /// Handles background results until `done` holds; false after 5 s.
    /// Rechecks every 20 ms too, for conditions outside the app (files written in the background).
    pub async fn settle(&mut self, done: impl Fn(&App) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done(self) {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            let fs = self.fs_deadline;
            tokio::select! {
                Some(msg) = self.rx.recv() => self.handle_app_event(msg),
                _ = tokio::time::sleep_until(fs.unwrap_or(deadline)), if fs.is_some() => self.flush_fs_changes(),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        true
    }
}
