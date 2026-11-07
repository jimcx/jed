//! One task run in a PTY (`portable-pty`), its output parsed into a `vt100` screen with
//! scrollback.
//!
//! Threads per run (none on the UI thread): the spawn (`spawn_blocking`: `openpty`, fork / exec),
//! then a reader (PTY output → `Shared::parser`), a waiter (the exit status) and a tokio task that
//! turns output into at most one `Event::Output` per `FRAME`. The child is a session leader
//! (portable-pty's `setsid`) and the PTY its controlling terminal, so its process group is
//! everything the shell started: kills signal the whole group.
//!
//! Input (`write`: keys, pastes) goes through a channel to a writer thread, so a process that
//! doesn't read never blocks the UI thread. Input written while the run is still starting is kept
//! and sent first once the process runs.
//!
//! The waiter sees the exit with `waitid(WNOWAIT)` and reaps only with the state locked, so a kill
//! never signals a reaped (reusable) pid. It reaps after the reader hit end of output (bounded by
//! `OUTPUT_GRACE`: a background process can hold the PTY open), so `Exited` means the screen holds
//! all the output.
//!
//! A shell's tab follows the OSC codes its terminal sends: `133;B` (a command starts) and
//! `133;D;<code>` (it finished) from the shell integration, and a program's own window title
//! (OSC 0 / 2, vim's for one) while it runs. `TitleCallback` turns them into
//! `Event::Foreground`. The same codes (plus `633;E`, the command line, and OSC 7, the directory)
//! fill a ring of the last `COMMAND_RING` `CommandRecord`s, each anchored to absolute rows so
//! `Run::command_output` can read its text back while the scrollback still holds it. A run with
//! `JED_IPC` also gets `JED_TERMINAL=<RunId>` (set in `Run::start`).
//!
//! The scroll bar (`TerminalScroll`, `Run::scrollable`) is the scrollback as a `Scrollable` in
//! rows; the kept-line count the bar needs as `max_top` is `Screen::scrollback_len()`
//! (`scrollbar_extent`).

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

use super::select;
use super::templates::Spec;
use crate::scrollbar::{Scroll, Scrollable};

/// Lines of output kept above the screen unless the `scrollback` setting says otherwise.
pub const SCROLLBACK: usize = 5_000;

/// How a run keeps its output above the screen (the `scrollback` and `scrollback_compression`
/// settings, read when the run is created). A bare line count means no compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrollback {
    pub lines: usize,
    /// Keep old lines zstd-compressed (`vt100`'s `compress` feature).
    pub compress: bool,
}

impl From<usize> for Scrollback {
    fn from(lines: usize) -> Self {
        Self { lines, compress: false }
    }
}
/// At most one `Event::Output` per run this often.
pub const FRAME: Duration = Duration::from_millis(20);
/// `kill`: SIGHUP first, SIGKILL if the process is still there this much later.
pub const KILL_GRACE: Duration = Duration::from_secs(1);
/// After the process exits, how long to wait for the rest of its output.
const OUTPUT_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

/// What a run tells the app (through the `Sink`); its `status()` / `screen()` say the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The process is running.
    Started(RunId),
    /// New output is on the screen (at most one per `FRAME`).
    Output(RunId),
    /// The terminal's title changed through its OSC codes: `name` runs in it now, or the shell is
    /// back at its prompt (`idle`), the last command having failed (`error`). A shell's tab title
    /// follows it.
    Foreground {
        id: RunId,
        name: String,
        idle: bool,
        error: bool,
    },
    /// The process exited (all its output read) or never started: `Exited` / `Failed`.
    Ended(RunId),
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The title a terminal sets through its OSC codes (FinalTerm's semantic prompts, which the shell
/// integration's preexec / precmd hooks send, and a program's own OSC 0 / 2). Runs on the reader
/// thread inside the parser: `133;B` starts a command (the `OSC 0 <first word>` that follows names
/// it), `133;D;<code>` finishes it (a non-zero code marks `error`), and an OSC 0 / 2 title shows
/// while it runs until `133;D` resets it. Reports each state as `Event::Foreground`.
pub struct TitleCallback {
    id: RunId,
    sink: Sink,
    /// The shell's own name: the title at its prompt, and until a command names itself.
    base: String,
    /// The title the terminal last set (sanitized); `None` means the base one.
    title: Option<String>,
    /// Between `133;B` and `133;D`: a command runs.
    running: bool,
    /// The last command finished with a non-zero exit code.
    error: bool,
    /// The last [`COMMAND_RING`] commands, oldest first.
    commands: VecDeque<CommandRecord>,
    /// The id the next command gets.
    next_command: u64,
    /// The directory the shell last reported (OSC 7), the cwd of the next command.
    cwd: Option<PathBuf>,
}

/// How many finished commands a run remembers.
pub const COMMAND_RING: usize = 50;

/// One command a shell with the integration ran: what it typed (`633;E`, `None` from a snippet
/// that doesn't send it), where, when, how it ended, and where its output sits in the terminal as
/// absolute rows (`Screen::scrolled_lines` + the row on screen) so the text can be read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    /// Counts up per run, from 1.
    pub id: u64,
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,
    pub started: Instant,
    /// `None` while it runs.
    pub finished: Option<Instant>,
    pub exit: Option<i32>,
    /// The first row of its output.
    pub start_row: u64,
    /// One past the last row of its output (not known while it runs).
    pub end_row: u64,
}

/// A command's output as read back from the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub text: String,
    /// Lines were dropped: the scrollback no longer holds the start, or `max_lines` cut it (the
    /// last lines are kept).
    pub truncated: bool,
    pub running: bool,
}

impl TitleCallback {
    fn new(id: RunId, sink: Sink, base: String) -> TitleCallback {
        TitleCallback {
            id,
            sink,
            base,
            title: None,
            running: false,
            error: false,
            commands: VecDeque::new(),
            next_command: 1,
            cwd: None,
        }
    }

    /// The remembered commands, oldest first.
    pub fn commands(&self) -> &VecDeque<CommandRecord> {
        &self.commands
    }

    /// Starts a record at the cursor's row, unless one is running: a second `133;B` is bash's DEBUG
    /// trap firing for the next simple command of the same line (it changes nothing).
    fn start_command(&mut self, screen: &vt100::Screen) {
        let row = absolute_cursor_row(screen);
        if self.commands.back().is_some_and(|c| c.finished.is_none()) {
            return;
        }
        if self.commands.len() == COMMAND_RING {
            self.commands.pop_front();
        }
        self.commands.push_back(CommandRecord {
            id: self.next_command,
            command: None,
            cwd: self.cwd.clone(),
            started: Instant::now(),
            finished: None,
            exit: None,
            start_row: row,
            end_row: row,
        });
        self.next_command += 1;
    }

    /// A `133;C`: the running command's output starts at the cursor, where its command line ended.
    fn reanchor_command(&mut self, screen: &vt100::Screen) {
        let row = absolute_cursor_row(screen);
        if let Some(c) = self.commands.back_mut().filter(|c| c.finished.is_none()) {
            c.start_row = row;
        } else {
            self.start_command(screen);
        }
    }

    fn finish_command(&mut self, screen: &vt100::Screen, exit: Option<i32>) {
        let (row, col) = screen.cursor_position();
        // Output that ended without a newline leaves its last line under the cursor.
        let end = screen.scrolled_lines() + u64::from(row) + u64::from(col > 0);
        if let Some(c) = self.commands.back_mut().filter(|c| c.finished.is_none()) {
            c.finished = Some(Instant::now());
            c.exit = exit;
            c.end_row = end.max(c.start_row);
        }
    }

    /// Reports the current state through the sink.
    fn report(&self) {
        (self.sink)(Event::Foreground {
            id: self.id,
            name: self.title.clone().unwrap_or_else(|| self.base.clone()),
            idle: !self.running,
            error: self.error,
        });
    }
}

impl vt100::Callbacks for TitleCallback {
    /// OSC 0 and 2: the window title (a program's own, while it runs).
    fn set_window_title(&mut self, _screen: &mut vt100::Screen, title: &[u8]) {
        self.title = sanitized(&String::from_utf8_lossy(title));
        self.report();
    }

    /// OSC 133: FinalTerm's semantic-prompt markers. Only `B` (preexec) and `D` (precmd) carry
    /// what a tab shows; the command's start (`C`) doesn't change it. A prompt's start (`A`)
    /// marks the cursor's row for Terminal: Clear and changes nothing else.
    fn unhandled_osc(&mut self, screen: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"133", b"A", ..] => screen.mark_prompt(),
            [b"133", b"B"] => {
                self.start_command(screen);
                self.running = true;
                self.error = false;
                // The snippet's OSC 0 with the command's first word comes next.
                self.title = None;
                self.report();
            }
            // The command's output starts here (the snippets send only `B`).
            [b"133", b"C"] => self.reanchor_command(screen),
            // `633;E;<command line>` (VS Code's): what was typed, escaped (`\\`, `\xNN`).
            [b"633", b"E", rest @ ..] => {
                let line = rest.join(&b';');
                // The first one wins: bash sends one per simple command of a line.
                if let Some(c) = self
                    .commands
                    .back_mut()
                    .filter(|c| c.finished.is_none() && c.command.is_none())
                {
                    c.command = Some(unescape_command(&line));
                }
            }
            // OSC 7 `file://host/path`: the shell's directory (a `;` in it split the params).
            [b"7", rest @ ..] => self.cwd = file_url_path(&rest.join(&b';')),
            // `133;D;<code>`; FinalTerm also allows `133;D` with no code, a success.
            [b"133", b"D", rest @ ..] => {
                let code = match rest {
                    [] => Some(0),
                    [c, ..] => std::str::from_utf8(c).ok().and_then(|c| c.parse().ok()),
                };
                self.finish_command(screen, code);
                self.running = false;
                self.error = !matches!(rest, [] | [b"0"]);
                self.title = None;
                self.report();
            }
            _ => {}
        }
    }
}

/// The cursor's row counted from the first row the terminal ever had.
fn absolute_cursor_row(screen: &vt100::Screen) -> u64 {
    screen.scrolled_lines() + u64::from(screen.cursor_position().0)
}

/// Undoes `633;E`'s escaping: `\\` is a backslash and `\xNN` a byte.
fn unescape_command(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' {
            if raw.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if raw.get(i + 1) == Some(&b'x')
                && let Some(b) = raw.get(i + 2..i + 4).and_then(hex_byte)
            {
                out.push(b);
                i += 4;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_byte(two: &[u8]) -> Option<u8> {
    // `from_str_radix` alone would take a sign (`+f`).
    if !two.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u8::from_str_radix(std::str::from_utf8(two).ok()?, 16).ok()
}

/// The path of a `file://host/path` URL, percent-decoded; `None` for anything else.
fn file_url_path(url: &[u8]) -> Option<PathBuf> {
    let rest = url.strip_prefix(b"file://")?;
    let path = &rest[rest.iter().position(|&b| b == b'/')?..];
    let mut out = Vec::with_capacity(path.len());
    let mut i = 0;
    while i < path.len() {
        if path[i] == b'%'
            && let Some(b) = path.get(i + 1..i + 3).and_then(hex_byte)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(path[i]);
            i += 1;
        }
    }
    // Paths are bytes on Unix: a non-UTF-8 name must survive.
    #[cfg(unix)]
    let path = PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(out));
    #[cfg(not(unix))]
    let path = PathBuf::from(String::from_utf8_lossy(&out).into_owned());
    (!path.as_os_str().as_encoded_bytes().contains(&0)).then_some(path)
}

/// [`Run::command_output`] on a parser.
fn command_text(parser: &mut vt100::Parser<TitleCallback>, id: u64, max_lines: usize) -> Option<CommandOutput> {
    let rec = parser.callbacks().commands().iter().find(|c| c.id == id)?.clone();
    let screen = parser.screen_mut();
    let running = rec.finished.is_none();
    let top = screen.scrolled_lines();
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let end = if running {
        top + u64::from(cursor_row) + u64::from(cursor_col > 0)
    } else {
        rec.end_row
    };
    let oldest = top - screen.scrollback_len() as u64;
    let mut truncated = rec.start_row < oldest;
    let mut start = rec.start_row.max(oldest);
    let wanted = max_lines.max(1) as u64;
    if end.saturating_sub(start) > wanted {
        start = end - wanted;
        truncated = true;
    }
    let saved = screen.scrollback();
    let mut text = String::new();
    let mut row = start;
    while row < end {
        // Scroll so `row` is the top line (rows on the live screen: no scrolling, skip down).
        screen.set_scrollback(top.saturating_sub(row) as usize);
        let skip = row - (top - screen.scrollback() as u64);
        let shown = (end - row).min(u64::from(rows.get()) - skip);
        for (i, line) in screen
            .rows(0, cols.get())
            .enumerate()
            .skip(skip as usize)
            .take(shown as usize)
        {
            text.push_str(&line);
            if !screen.row_wrapped(i as u16) {
                text.push('\n');
            }
        }
        row += shown;
    }
    screen.set_scrollback(saved);
    Some(CommandOutput {
        text,
        truncated,
        running,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Being spawned off the UI thread.
    Starting,
    Running,
    Exited(Exit),
    /// Couldn't start (no such cwd, spawn error): the message.
    Failed(String),
}

impl Status {
    pub fn is_running(&self) -> bool {
        matches!(self, Status::Starting | Status::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// The exit code (1 when killed by a signal, as portable-pty reports it).
    pub code: u32,
    /// The signal that ended it, as `strsignal` names it ("Hangup", "Killed").
    pub signal: Option<String>,
    /// `kill` (or dropping the run) ended it.
    pub killed: bool,
}

impl Exit {
    pub fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }
}

/// A task run. Dropping it kills the process (group) if it's still running.
pub struct Run {
    pub id: RunId,
    pub name: String,
    /// The command line, as the template has it.
    pub command: String,
    pub cwd: PathBuf,
    shared: Arc<Shared>,
}

struct Shared {
    id: RunId,
    parser: Mutex<vt100::Parser<TitleCallback>>,
    state: Mutex<State>,
    /// The text a mouse drag selected, in the screen's visible-viewport cells. Cleared when the
    /// view moves (new output, a scroll) or a click clears it.
    selection: Mutex<Option<select::Selection>>,
    /// A left drag is held on this screen (`Run::set_dragging`): output then moves only the
    /// selection's anchor, and `drag_lines` counts the lines it scrolled meanwhile.
    dragging: AtomicBool,
    drag_lines: AtomicUsize,
    /// Output arrived since the last `Event::Output`.
    pending: AtomicBool,
    /// The reader is done (end of output) or there's none (failed to start).
    reader_done: AtomicBool,
    /// Wakes the notifier task (output, or the reader ending).
    notify: tokio::sync::Notify,
}

/// What the writer thread is sent.
enum Input {
    /// Keys, a paste: always written.
    Bytes(Vec<u8>),
    /// A mouse report (`motion`: a move or drag, of which only the latest matters): written only
    /// while the program still asks for the mouse, so reports queued behind a slow writer never
    /// reach the shell after the program reset the mode.
    Mouse { bytes: Vec<u8>, motion: bool },
}

struct State {
    status: Status,
    /// The process (group) id while it can be signalled: set when spawned, until reaped.
    pid: Option<i32>,
    master: Option<Box<dyn MasterPty + Send>>,
    /// To the writer thread (the PTY's input), while the process runs.
    input: Option<std::sync::mpsc::Sender<Input>>,
    /// Input written while starting, sent once the writer thread exists.
    queued: Vec<u8>,
    /// The process group (= the first process's pid), kept after reaping: background processes
    /// it started may still be in it (`Shared::has_leftovers`), and dropping the run kills them.
    group: Option<i32>,
    /// A kill asked for before the process existed: `Some(force)`.
    kill_when_started: Option<bool>,
    killed: bool,
}

/// Locks, ignoring poisoning (a panicking reader leaves the screen usable).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Run {
    /// Starts `spec` in a `rows` × `cols` PTY, off the UI thread (needs a tokio runtime; without
    /// one the run fails at once). `sink` hears about it from other threads; `scrollback` is the
    /// lines kept above the screen (the `scrollback` setting, read when the run is created).
    pub fn start(
        id: RunId,
        mut spec: Spec,
        rows: u16,
        cols: u16,
        sink: Sink,
        scrollback: impl Into<Scrollback>,
    ) -> Run {
        // A run that can reach this jed (it has `JED_IPC`) says which terminal it is.
        if spec.env.iter().any(|(k, _)| k == crate::ipc::ENV) {
            spec.env.retain(|(k, _)| k != crate::ipc::TERMINAL_ENV);
            spec.env.push((crate::ipc::TERMINAL_ENV.into(), id.0.to_string()));
        }
        let scrollback = scrollback.into();
        let (rows, cols) = (rows.max(1), cols.max(1));
        let callbacks = TitleCallback::new(id, sink.clone(), spec.name.clone());
        let shared = Arc::new(Shared {
            id,
            parser: Mutex::new({
                let mut parser =
                    vt100::Parser::new_with_callbacks(nonzero(rows), nonzero(cols), scrollback.lines, callbacks);
                if scrollback.compress {
                    parser.screen_mut().set_scrollback_compression(true);
                }
                parser
            }),
            selection: Mutex::new(None),
            dragging: AtomicBool::new(false),
            drag_lines: AtomicUsize::new(0),
            state: Mutex::new(State {
                status: Status::Starting,
                pid: None,
                group: None,
                master: None,
                input: None,
                queued: Vec::new(),
                kill_when_started: None,
                killed: false,
            }),
            pending: AtomicBool::new(false),
            reader_done: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        });
        let run = Run {
            id,
            name: spec.name.clone(),
            command: spec.command.clone(),
            cwd: spec.cwd.clone(),
            shared: shared.clone(),
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(notifier(shared.clone(), sink.clone()));
                rt.spawn_blocking(move || spawn(shared, spec, sink));
            }
            Err(_) => shared.fail("no runtime to run tasks".into(), &sink),
        }
        run
    }

    pub fn status(&self) -> Status {
        lock(&self.shared.state).status.clone()
    }

    /// The process (group) id while it runs (also between its exit and its reaping).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pid(&self) -> Option<u32> {
        lock(&self.shared.state).pid.map(|p| p as u32)
    }

    /// The terminal: `screen()` for drawing (`tui-term`), `screen().contents()`, scrollback via
    /// `screen_mut().set_scrollback(n)`. Held briefly: the reader waits on it.
    pub fn screen(&self) -> MutexGuard<'_, vt100::Parser<TitleCallback>> {
        lock(&self.shared.parser)
    }

    /// The remembered commands (a shell with the integration), oldest first.
    pub fn commands(&self) -> Vec<CommandRecord> {
        self.screen().callbacks().commands().iter().cloned().collect()
    }

    /// Command `id`'s output read back from the screen and scrollback, at most the last
    /// `max_lines` lines; `None` for an id that's gone from the ring. A command still running
    /// returns what's on screen so far (up to the cursor).
    pub fn command_output(&self, id: u64, max_lines: usize) -> Option<CommandOutput> {
        command_text(&mut self.screen(), id, max_lines)
    }

    /// Resizes the screen and the PTY (the process gets SIGWINCH). Zero is taken as one. A size
    /// change reflows the rows, so it clears the mouse selection.
    pub fn resize(&self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let state = lock(&self.shared.state);
        let mut parser = lock(&self.shared.parser);
        let (nz_rows, nz_cols) = (nonzero(rows), nonzero(cols));
        if parser.screen().size() == (nz_rows, nz_cols) {
            return;
        }
        parser.screen_mut().set_size(nz_rows, nz_cols);
        if let Some(master) = &state.master
            && let Err(e) = master.resize(pty_size(rows, cols))
        {
            tracing::debug!(run = self.id.0, "resize: {e}");
        }
        self.clear_selection();
    }

    /// Sends `bytes` to the process's terminal input (keys, a paste); while it's starting they're
    /// queued and sent once it runs (dropped if it fails to start). False once it ended.
    pub fn write(&self, bytes: &[u8]) -> bool {
        let mut state = lock(&self.shared.state);
        match (&state.status, &state.input) {
            (Status::Running, Some(input)) => input.send(Input::Bytes(bytes.to_vec())).is_ok(),
            (Status::Starting, _) => {
                state.queued.extend_from_slice(bytes);
                true
            }
            _ => false,
        }
    }

    /// Sends a mouse report (`tasks::mouse::encode`) to the process. Unlike `write` it is dropped
    /// when the program no longer asks for the mouse by the time it would be written, and while
    /// the writer is behind a `motion` report is replaced by a newer one. Nothing while starting.
    pub fn write_mouse(&self, bytes: Vec<u8>, motion: bool) -> bool {
        let state = lock(&self.shared.state);
        match (&state.status, &state.input) {
            (Status::Running, Some(input)) => input.send(Input::Mouse { bytes, motion }).is_ok(),
            _ => false,
        }
    }

    /// Scrolls the view through the scrollback by `lines` (negative: up, to older output),
    /// clamped; 0 rows back is the live screen. The selection stays on its text (its cells are
    /// scroll-independent).
    pub fn scroll(&self, lines: isize) {
        let mut parser = self.screen();
        let back = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(back.saturating_add_signed(-lines));
    }

    /// Terminal: Clear. At a shell's prompt with a prompt mark (`OSC 133;A` from the shell
    /// integration) everything before the last mark goes, scrollback included, and the prompt
    /// (all its lines) is the top of the screen. Without a mark, or while a command runs (between
    /// `133;B` and `133;D`: going back to its prompt would keep all its output), it blanks the
    /// screen (cursor home) and empties the kept lines. Either way the
    /// view is back on the live screen, and only the parser is touched, never the PTY: the program
    /// runs on and its next prompt redraws. The mouse selection is dropped (its rows are gone). On
    /// the alternate screen (a full-screen program) it blanks that screen until the program redraws
    /// and empties only its (empty) scrollback: the main screen's kept lines are back when the
    /// program leaves.
    pub fn clear(&self) {
        {
            let mut parser = self.screen();
            let at_prompt = !parser.callbacks().running;
            if !(at_prompt && parser.screen_mut().clear_before_prompt()) {
                parser.process(b"\x1b[2J\x1b[H");
                parser.screen_mut().clear_scrollback();
            }
            parser.screen_mut().set_scrollback(0);
        }
        self.clear_selection();
    }

    /// Scrolls to the oldest output kept (`top`) or back to the live screen. The selection stays
    /// on its text.
    pub fn scroll_to(&self, top: bool) {
        self.screen()
            .screen_mut()
            .set_scrollback(if top { usize::MAX } else { 0 });
    }

    /// The scroll bar's extent: `(back, max, rows)` — the current offset (0 = the live screen),
    /// the kept lines, and the screen's rows.
    pub fn scrollbar_extent(&self) -> (usize, usize, usize) {
        let mut parser = self.screen();
        let screen = parser.screen_mut();
        (
            screen.scrollback(),
            screen.scrollback_len(),
            screen.size().0.get() as usize,
        )
    }

    /// Puts the scrollback `top` rows from the oldest output (the `Scroll::rows` space; `max` is
    /// the live screen). Keeps the selection, as `scroll` / `scroll_to` do.
    pub fn set_scrollback_from_top(&self, top: usize) {
        let mut parser = self.screen();
        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let max = screen.scrollback();
        screen.set_scrollback(max.saturating_sub(top));
    }

    /// The scroll bar (`TerminalScroll`): the scrollback as a [`Scrollable`] whose `Scroll` is in
    /// rows. `set_top` is `top` rows from the oldest output, so the live screen is `top == max`
    /// (thumb at the bottom) and the oldest output `top == 0` (thumb at the top).
    pub fn scrollable(&self) -> TerminalScroll<'_> {
        TerminalScroll { run: self }
    }

    /// The mouse selection on the screen, if any.
    pub fn selection(&self) -> Option<select::Selection> {
        *lock(&self.shared.selection)
    }

    /// Sets the mouse selection to the `anchor`–`head` cells (`select::Cell`, scroll-independent).
    pub fn set_selection(&self, selection: select::Selection) {
        *lock(&self.shared.selection) = Some(selection);
    }

    /// Marks a left drag as held (or released). While held, output keeps the selection's anchor
    /// on its text and leaves the head on its screen cell; starting one forgets `take_drag_lines`.
    pub fn set_dragging(&self, on: bool) {
        if on {
            self.shared.drag_lines.store(0, Ordering::Release);
        }
        self.shared.dragging.store(on, Ordering::Release);
    }

    /// The lines output scrolled into the scrollback since the last call (or since the drag
    /// began), for the app's copy of the drag's anchor.
    pub fn take_drag_lines(&self) -> usize {
        self.shared.drag_lines.swap(0, Ordering::AcqRel)
    }

    /// Drops the mouse selection (a click, a resize, Clear, or output that changed its text).
    pub fn clear_selection(&self) {
        *lock(&self.shared.selection) = None;
    }

    /// The selection's text (`select::copy_text`: `contents_between`, trailing spaces trimmed),
    /// `None` without a selection.
    pub fn copy_selection(&self) -> Option<String> {
        let selection = self.selection()?;
        let mut parser = self.screen();
        Some(select::copy_text(parser.screen_mut(), &selection))
    }

    /// Writes `text` on a line of its own below the output (attributes reset): the exit note.
    pub fn append_line(&self, text: &str) {
        let mut parser = self.screen();
        let at_line_start = parser.screen().cursor_position().1 == 0;
        let mut bytes = Vec::with_capacity(text.len() + 8);
        if !at_line_start {
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"\x1b[0m");
        bytes.extend_from_slice(text.as_bytes());
        parser.process(&bytes);
    }

    /// SIGHUP to the process group (what closing a terminal sends), SIGKILL to the group after
    /// `KILL_GRACE` if it's still running, or if it ended but processes it started still hold the
    /// terminal. Once it ended: SIGKILL to such leftovers, else nothing.
    pub fn kill(&self) {
        self.shared.kill(false);
    }

    /// SIGKILL to the process group now.
    pub fn force_kill(&self) {
        self.shared.kill(true);
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.shared.kill(false);
    }
}

/// A run's scroll bar (`Run::scrollable`): the scrollback as a [`Scrollable`]. Its `Scroll` is in
/// rows — `Scroll::rows(max + rows, rows, max - back)` — so the live screen is `top == max` (thumb
/// at the bottom), the oldest output `top == 0` (thumb at the top), and the last page stays full
/// as a terminal view is always `rows` tall. `set_top` moves the scrollback there, keeping the
/// selection on its text. A `Scrollable` on a borrow, not on `Run`, so its `scroll()` doesn't
/// clash with `Run::scroll(&self, isize)`.
pub struct TerminalScroll<'a> {
    pub run: &'a Run,
}

impl Scrollable for TerminalScroll<'_> {
    fn scroll(&self) -> Scroll {
        let (back, max, rows) = self.run.scrollbar_extent();
        Scroll::rows(max + rows, rows, max - back)
    }

    fn set_top(&mut self, top: usize) {
        self.run.set_scrollback_from_top(top);
    }
}

/// Test hooks for the lock order: the spawn holds the state lock while it takes the screen's, so
/// a caller that took the screen first and then waited for the state would deadlock with it.
#[cfg(test)]
impl Run {
    /// Holds the state lock until the guard is dropped.
    pub(crate) fn test_hold_state(&self) -> impl Drop + '_ {
        lock(&self.shared.state)
    }

    /// Whether the screen's lock is free right now (nothing holds it).
    pub(crate) fn test_screen_free(&self) -> bool {
        self.shared.parser.try_lock().is_ok()
    }
}

impl Shared {
    fn kill(self: &Arc<Self>, force: bool) {
        let mut state = lock(&self.state);
        match state.status {
            Status::Starting => state.kill_when_started = Some(force || state.kill_when_started == Some(true)),
            Status::Running => {
                let Some(pid) = state.pid else { return };
                state.killed = true;
                signal_group(pid, force);
                if !force {
                    let shared = self.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(KILL_GRACE);
                        let state = lock(&shared.state);
                        match (&state.status, state.pid, state.group) {
                            (Status::Running, Some(pid), _) => signal_group(pid, true),
                            // The leader died of the SIGHUP, but processes it started ignored it.
                            (Status::Exited(_), _, Some(group)) if shared.has_leftovers(group) => {
                                signal_group(group, true)
                            }
                            _ => {}
                        }
                    });
                }
            }
            // Leftovers in its group.
            Status::Exited(_) => {
                if let Some(group) = state.group
                    && self.has_leftovers(group)
                {
                    signal_group(group, true);
                }
            }
            Status::Failed(_) => {}
        }
    }

    /// Processes the ended leader started are still in its group `group`. On Linux they hold the
    /// PTY open, so the reader hasn't seen its end. macOS revokes the terminal when the session
    /// leader exits (the reader ends at once), so there they're looked for: members of the group
    /// in the session the leader made (its id = the group's), which a reused pid can't be unless
    /// it wrapped around and leads a session of its own.
    fn has_leftovers(&self, group: i32) -> bool {
        !self.reader_done.load(Ordering::Acquire) || (cfg!(target_os = "macos") && session_members(group))
    }

    fn fail(&self, message: String, sink: &Sink) {
        tracing::warn!(run = self.id.0, "task failed to start: {message}");
        {
            let mut state = lock(&self.state);
            state.status = Status::Failed(message);
            state.queued = Vec::new();
        }
        self.reader_done.store(true, Ordering::Release);
        self.notify.notify_one();
        sink(Event::Ended(self.id));
    }
}

/// SIGKILL, or SIGHUP + SIGCONT (so a stopped process sees it), to the group `pid` leads.
fn signal_group(pid: i32, force: bool) {
    // SAFETY: plain kill(2) calls on a process group we created and haven't reaped the leader of.
    unsafe {
        if force {
            libc::kill(-pid, libc::SIGKILL);
        } else {
            libc::kill(-pid, libc::SIGHUP);
            libc::kill(-pid, libc::SIGCONT);
        }
    }
}

/// Some process is in the group `group` and the session of the same id.
#[cfg(target_os = "macos")]
fn session_members(group: i32) -> bool {
    // <libproc.h>'s PROC_PGRP_ONLY: the pids in one process group.
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = [0 as libc::pid_t; 64];
    let size = std::mem::size_of_val(&pids) as libc::c_int;
    // SAFETY: the buffer and its size in bytes match; getsid only reads.
    unsafe {
        let bytes = libc::proc_listpids(PROC_PGRP_ONLY, group as u32, pids.as_mut_ptr().cast(), size);
        let n = (bytes.max(0) as usize / std::mem::size_of::<libc::pid_t>()).min(pids.len());
        pids[..n].iter().any(|&pid| pid > 0 && libc::getsid(pid) == group)
    }
}

#[cfg(not(target_os = "macos"))]
fn session_members(_group: i32) -> bool {
    false
}

/// A title for a tab bar, its control characters dropped (as the window title drops them): a
/// program can put ESC or other control bytes into its title, which must not reach the rendered
/// tab bar. `None` when nothing printable is left, so the title falls back to the base one.
fn sanitized(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    (!name.is_empty()).then_some(name)
}

/// `n` as the non-zero size `vt100` takes (zero is taken as one).
fn nonzero(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).unwrap_or(std::num::NonZeroU16::MIN)
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Test-only switch: while set, every run fails at the spawn instead of starting its process.
#[cfg(test)]
pub static NO_SPAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Blocking: opens the PTY and spawns the process, then starts the reader and the waiter.
fn spawn(shared: Arc<Shared>, spec: Spec, sink: Sink) {
    // Test-only: hold the run `Starting` until the test releases it, so a quit-while-starting
    // test is deterministic instead of racing this off-thread spawn.
    #[cfg(test)]
    if let Some(gate) = &spec.start_gate {
        gate.wait();
    }
    // Test-only: the fuzzer sets this so no random command, key or click starts a real process.
    #[cfg(test)]
    if NO_SPAWN.load(std::sync::atomic::Ordering::Relaxed) {
        return shared.fail("spawning is disabled in this test".into(), &sink);
    }
    if !spec.cwd.is_dir() {
        return shared.fail(format!("{}: no such folder", spec.cwd.display()), &sink);
    }
    let (rows, cols) = super::select::size(lock(&shared.parser).screen());
    // One PTY opened and spawned at a time: macOS's `openpty` isn't thread-safe (its `ptsname`
    // is a static buffer: concurrent starts failed), and portable-pty marks the new fds
    // close-on-exec only after it, so another run's fork in between would inherit this slave.
    static OPENING: Mutex<()> = Mutex::new(());
    let opening = lock(&OPENING);
    let pair = match portable_pty::native_pty_system().openpty(pty_size(rows, cols)) {
        Ok(p) => p,
        Err(e) => return shared.fail(format!("can't open a terminal: {e}"), &sink),
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => return shared.fail(format!("can't read the terminal: {e}"), &sink),
    };
    let writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => return shared.fail(format!("can't write to the terminal: {e}"), &sink),
    };
    let mut cmd = CommandBuilder::from_argv(spec.argv.clone());
    cmd.cwd(&spec.cwd);
    // A run gets `JED_IPC` only from its spec (this jed's own socket, `Tasks::ipc` /
    // `shell::spec`). Never inherit one leaked from a parent process — a jed started inside
    // another jed's shell would otherwise hand its own tasks the outer jed's socket.
    cmd.env_remove(crate::ipc::ENV);
    cmd.env_remove(crate::ipc::TERMINAL_ENV);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let t = Instant::now();
    let child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => return shared.fail(format!("can't run {}: {e}", spec.argv[0].to_string_lossy()), &sink),
    };
    // Our copy of the slave side must go, or the reader never sees the end of output.
    drop(pair.slave);
    drop(opening);
    let Some(pid) = child.process_id().map(|p| p as i32) else {
        return shared.fail("no process id".into(), &sink);
    };
    tracing::info!(run = shared.id.0, pid, task = %spec.name, cwd = %spec.cwd.display(), elapsed_ms = t.elapsed().as_millis() as u64, "task started");
    let kill = {
        let mut state = lock(&shared.state);
        state.status = Status::Running;
        state.pid = Some(pid);
        state.group = Some(pid);
        // Resized while starting.
        let size = super::select::size(lock(&shared.parser).screen());
        if size != (rows, cols) {
            let _ = pair.master.resize(pty_size(size.0, size.1));
        }
        state.master = Some(pair.master);
        let (input, keys) = std::sync::mpsc::channel();
        let queued = std::mem::take(&mut state.queued);
        if !queued.is_empty() {
            let _ = input.send(Input::Bytes(queued));
        }
        let weak = Arc::downgrade(&shared);
        let spawned = std::thread::Builder::new()
            .name(format!("task-{}-in", shared.id.0))
            .spawn(move || write_input(writer, keys, weak));
        match spawned {
            Ok(_) => state.input = Some(input),
            Err(e) => tracing::error!("task writer thread: {e}"),
        }
        state.kill_when_started.take()
    };
    sink(Event::Started(shared.id));
    if let Some(force) = kill {
        shared.kill(force);
    }
    let reader_shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("task-{}-out", shared.id.0))
        .spawn(move || read_output(reader_shared, reader));
    if let Err(e) = spawned {
        tracing::error!("task reader thread: {e}");
        shared.reader_done.store(true, Ordering::Release);
        shared.notify.notify_one();
    }
    let waiter_shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("task-{}-wait", shared.id.0))
        .spawn(move || wait_exit(waiter_shared, child, pid, sink));
    if let Err(e) = spawned {
        // Can't wait for it: kill it rather than leave it unreaped and unkillable.
        tracing::error!("task waiter thread: {e}");
        signal_group(pid, true);
    }
}

/// Feeds `bytes` to the parser and keeps the mouse selection on its text (`select::follow`).
/// Locks the parser, then the selection (as `resize` does), so a click can't be overwritten.
fn process_chunk(shared: &Shared, bytes: &[u8]) {
    let mut parser = lock(&shared.parser);
    let mut selection = lock(&shared.selection);
    let Some(current) = *selection else {
        parser.process(bytes);
        return;
    };
    let before = select::snapshot(parser.screen_mut(), &current);
    parser.process(bytes);
    let dragging = shared.dragging.load(Ordering::Acquire);
    let scrolled = parser.screen().scrolled_lines().saturating_sub(before.scrolled());
    if dragging {
        shared
            .drag_lines
            .fetch_add(usize::try_from(scrolled).unwrap_or(usize::MAX / 2), Ordering::AcqRel);
    }
    *selection = select::follow(parser.screen_mut(), current, &before, dragging);
}

/// Reader thread: PTY output into the screen until the end of output.
fn read_output(shared: Arc<Shared>, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                process_chunk(&shared, &buf[..n]);
                if !shared.pending.swap(true, Ordering::AcqRel) {
                    shared.notify.notify_one();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // EIO: every slave fd closed (Linux).
            Err(_) => break,
        }
    }
    shared.reader_done.store(true, Ordering::Release);
    shared.notify.notify_one();
}

/// Writer thread: input into the PTY until the run is dropped (or the terminal is gone). Whatever
/// is queued is taken at once: a mouse move followed by a newer one is dropped, and mouse reports
/// are written only while the screen's mouse mode is on (checked here, at write time).
fn write_input(
    mut writer: Box<dyn Write + Send>,
    input: std::sync::mpsc::Receiver<Input>,
    shared: std::sync::Weak<Shared>,
) {
    let mouse_on = || {
        shared
            .upgrade()
            .is_some_and(|s| lock(&s.parser).screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
    };
    while let Ok(first) = input.recv() {
        let mut batch = vec![first];
        batch.extend(input.try_iter());
        let out = merge(batch, mouse_on);
        if !out.is_empty() && writer.write_all(&out).and_then(|()| writer.flush()).is_err() {
            return;
        }
    }
}

/// The bytes for `batch`: a mouse move directly followed by a newer one is dropped, and mouse
/// reports all go when `mouse_on` (asked once, lazily) says the program no longer wants them.
fn merge(batch: Vec<Input>, mouse_on: impl FnOnce() -> bool) -> Vec<u8> {
    let mut mouse_on = Some(mouse_on);
    let mut on = true;
    let mut out: Vec<u8> = Vec::new();
    let mut last_motion: Option<usize> = None;
    for item in batch {
        match item {
            Input::Bytes(bytes) => {
                out.extend_from_slice(&bytes);
                last_motion = None;
            }
            Input::Mouse { bytes, motion } => {
                if let Some(f) = mouse_on.take() {
                    on = f();
                }
                if !on {
                    continue;
                }
                if motion {
                    if let Some(at) = last_motion {
                        out.truncate(at);
                    }
                    last_motion = Some(out.len());
                } else {
                    last_motion = None;
                }
                out.extend_from_slice(&bytes);
            }
        }
    }
    out
}

/// Waiter thread: blocks until the process exits, lets the reader finish, reaps it.
fn wait_exit(shared: Arc<Shared>, mut child: Box<dyn Child + Send + Sync>, pid: i32, sink: Sink) {
    wait_without_reaping(pid);
    let until = Instant::now() + OUTPUT_GRACE;
    while !shared.reader_done.load(Ordering::Acquire) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    {
        let mut state = lock(&shared.state);
        let status = child.wait();
        state.pid = None;
        state.input = None;
        state.status = match status {
            Ok(s) => Status::Exited(Exit {
                code: s.exit_code(),
                signal: s.signal().map(str::to_string),
                killed: state.killed,
            }),
            Err(e) => Status::Exited(Exit {
                code: 1,
                signal: Some(format!("wait failed: {e}")),
                killed: state.killed,
            }),
        };
        tracing::info!(run = shared.id.0, status = ?state.status, "task ended");
        // The PTY stays open while background processes it started hold it; the reader ends
        // when they do.
    }
    sink(Event::Ended(shared.id));
}

/// Blocks until `pid` has exited, leaving it a zombie (its pid can't be reused until reaped).
fn wait_without_reaping(pid: i32) {
    loop {
        // SAFETY: waitid fills `info`; WNOWAIT leaves the child for `Child::wait` to reap.
        let r = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT)
        };
        if r == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Notifier task: output → at most one `Event::Output` per `FRAME`; ends with the reader.
async fn notifier(shared: Arc<Shared>, sink: Sink) {
    loop {
        shared.notify.notified().await;
        if shared.pending.swap(false, Ordering::AcqRel) {
            sink(Event::Output(shared.id));
        }
        if shared.reader_done.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(FRAME).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parser whose `TitleCallback` reports into a vector, for `zsh` as the shell's name.
    fn title_parser(base: &str) -> (vt100::Parser<TitleCallback>, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = {
            let events = events.clone();
            Arc::new(move |ev| lock(&events).push(ev))
        };
        let parser = vt100::Parser::new_with_callbacks(
            nonzero(24),
            nonzero(80),
            0,
            TitleCallback::new(RunId(7), sink, base.into()),
        );
        (parser, events)
    }

    /// The vt100 fork's calls are reachable through a run's parser: a prompt mark, then
    /// `clear_before_prompt` keeps the marked row on; with no mark nothing changes.
    #[test]
    fn the_fork_clears_before_the_last_prompt_mark() {
        let (mut parser, _) = title_parser("t");
        assert!(!parser.screen_mut().clear_before_prompt());
        parser.process(b"old\r\n");
        parser.screen_mut().mark_prompt();
        parser.process(b"$ ls\r\nout");
        assert!(parser.screen_mut().clear_before_prompt());
        assert_eq!(parser.screen().contents(), "$ ls\nout");
    }

    /// `133;A` marks the cursor's row and nothing else: no event, no title or state change.
    #[test]
    fn prompt_start_marks_the_row_and_reports_nothing() {
        let (mut parser, events) = title_parser("zsh");
        parser.process(b"old\r\n\x1b]133;A\x07$ ");
        assert!(lock(&events).is_empty(), "no tab change");
        parser.process(b"ls\r\nout");
        assert!(parser.screen_mut().clear_before_prompt(), "the row was marked");
        assert_eq!(parser.screen().contents(), "$ ls\nout");
    }

    /// A title for the tab bar drops control characters (ESC, tabs, newlines, DEL) and is `None`
    /// when nothing printable is left, so the title falls back to the shell's name.
    #[test]
    fn titles_drop_control_characters() {
        assert_eq!(sanitized("sh").as_deref(), Some("sh"));
        assert_eq!(sanitized("my shell (2)").as_deref(), Some("my shell (2)"));
        assert_eq!(sanitized("evil\x1b]2;x\x07").as_deref(), Some("evil]2;x"));
        assert_eq!(
            sanitized("a\tb\nc").as_deref(),
            Some("abc"),
            "whitespace controls dropped too"
        );
        assert_eq!(sanitized("\x1b\x07").as_deref(), None, "nothing printable left");
        assert_eq!(sanitized("").as_deref(), None);
    }

    /// The OSC codes a shell integration and its programs send: `133;B` starts a command (the
    /// snippet's `OSC 0 <first word>` then names it), `133;D;<code>` returns to the shell's name
    /// (`code` non-zero = `error`), a program's own OSC 0 / 2 title shows while it runs until
    /// `133;D`, and a title with control bytes is sanitized (an empty one falls back to the base).
    #[test]
    fn osc_codes_title_a_shell_and_its_state() {
        let id = RunId(7);
        let at = |name: &str, idle: bool, error: bool| Event::Foreground {
            id,
            name: name.into(),
            idle,
            error,
        };
        let last = |events: &Arc<Mutex<Vec<Event>>>| lock(events).last().cloned().unwrap();
        let (mut parser, events) = title_parser("zsh");

        // Output without OSC codes (a shell with no integration) sends nothing: the title stays.
        parser.process(b"hello\r\n");
        assert!(lock(&events).is_empty(), "no OSC codes, no title change");

        // Preexec (`133;B`) then the snippet's `OSC 0 sleep`: a command runs.
        parser.process(b"\x1b]133;B\x07\x1b]0;sleep\x07");
        assert_eq!(
            *lock(&events),
            [at("zsh", false, false), at("sleep", false, false)],
            "the base name first, then the command's first word"
        );

        // Precmd (`133;D;0`): back at the prompt, no error.
        parser.process(b"\x1b]133;D;0\x07");
        assert_eq!(last(&events), at("zsh", true, false));

        // A non-zero exit: idle in the error colour.
        parser.process(b"\x1b]133;B\x07\x1b]133;D;2\x07");
        assert_eq!(last(&events), at("zsh", true, true));

        // A program's own OSC 2 title while it runs, until `133;D` resets it.
        parser.process(b"\x1b]133;B\x07\x1b]2;file.rs \xe2\x80\x94 VIM\x07");
        assert_eq!(last(&events), at("file.rs — VIM", false, false));
        parser.process(b"\x1b]133;D\x07");
        assert_eq!(last(&events), at("zsh", true, false), "no code is a success");

        // Control bytes in a title never reach the tab bar; an all-control one is the base name.
        parser.process(b"\x1b]133;B\x07\x1b]2;ev\x01il\x07");
        assert_eq!(last(&events), at("evil", false, false));
        parser.process(b"\x1b]2;\x01\x07");
        assert_eq!(last(&events), at("zsh", false, false));
    }

    /// FinalTerm's other semantic-prompt markers — `133;A` (a prompt starts) and `133;C` (a
    /// command's output starts) — don't change the title: only `B` and `D` carry one.
    #[test]
    fn osc_133_prompt_and_output_markers_are_ignored() {
        let (mut parser, events) = title_parser("zsh");
        parser.process(b"\x1b]133;A\x07\x1b]133;C\x07");
        assert!(lock(&events).is_empty(), "only `133;B` and `133;D` change the title");
    }

    fn mouse(bytes: &str, motion: bool) -> Input {
        Input::Mouse {
            bytes: bytes.as_bytes().to_vec(),
            motion,
        }
    }

    #[test]
    fn queued_mouse_moves_collapse_to_the_latest() {
        let batch = vec![
            mouse("a", true),
            mouse("b", true),
            mouse("P", false),
            mouse("c", true),
            Input::Bytes(b"k".to_vec()),
            mouse("d", true),
            mouse("e", true),
        ];
        assert_eq!(merge(batch, || true), b"bPcke");
    }

    #[test]
    fn mouse_reports_are_dropped_once_the_program_reset_the_mouse() {
        let batch = vec![mouse("a", true), Input::Bytes(b"k".to_vec()), mouse("P", false)];
        assert_eq!(merge(batch, || false), b"k");
    }

    /// `633;E` and OSC 7 fill the command ring: id, escaped command line (`\\`, `\xNN`, a `;`
    /// inside), cwd (percent-decoded, host dropped), exit code, and no command from a snippet
    /// that sends no `633;E`.
    #[test]
    fn command_ring_records_commands_cwds_and_exits() {
        let (mut parser, _) = title_parser("bash");
        parser.process(b"\x1b]7;file://box/home/a%20b\x07\x1b]133;B\x07\x1b]633;E;echo a\\x3bb \\\\ \\x0a\x07out\r\n");
        parser.process(b"\x1b]133;D;3\x07\x1b]133;B\x07old snippet\r\n\x1b]133;D\x07");
        let cmds: Vec<_> = parser.callbacks().commands().iter().cloned().collect();
        assert_eq!(cmds.len(), 2);
        assert_eq!((cmds[0].id, cmds[1].id), (1, 2));
        assert_eq!(cmds[0].command.as_deref(), Some("echo a;b \\ \n"));
        assert_eq!(cmds[0].cwd, Some(PathBuf::from("/home/a b")));
        assert_eq!(cmds[0].exit, Some(3));
        assert_eq!(cmds[1].command, None);
        assert_eq!(cmds[1].exit, Some(0));
        assert!(cmds.iter().all(|c| c.finished.is_some()));
        // Garbage OSC 7 and a `633;E` with no command running change nothing.
        parser.process(b"\x1b]7;http://x/y\x07\x1b]633;E;stray\x07");
        assert_eq!(parser.callbacks().commands().len(), 2);
    }

    /// The ring keeps the last `COMMAND_RING` commands, ids still counting up.
    #[test]
    fn command_ring_is_bounded() {
        let (mut parser, _) = title_parser("bash");
        for _ in 0..COMMAND_RING + 5 {
            parser.process(b"\x1b]133;B\x07x\r\n\x1b]133;D;0\x07");
        }
        let cmds = parser.callbacks().commands();
        assert_eq!(cmds.len(), COMMAND_RING);
        assert_eq!(cmds.front().unwrap().id, 6);
        assert_eq!(cmds.back().unwrap().id, COMMAND_RING as u64 + 5);
    }

    /// Output is read back by absolute row, across the scrollback; rows scrolled off are
    /// `truncated`, `max_lines` keeps the tail, and a partial last line counts.
    #[test]
    fn command_output_reads_the_scrollback() {
        let sink: Sink = Arc::new(|_| {});
        let mut parser = vt100::Parser::new_with_callbacks(
            nonzero(5),
            nonzero(20),
            12,
            TitleCallback::new(RunId(7), sink, "t".into()),
        );
        parser.process(b"$ seq\r\n\x1b]133;B\x07");
        for n in 1..=12 {
            parser.process(format!("line{n}\r\n").as_bytes());
        }
        parser.process(b"tail\x1b]133;D;0\x07\r\n$ ");
        let out = command_text(&mut parser, 1, 100).unwrap();
        let mut want: String = (1..=12).map(|n| format!("line{n}\n")).collect();
        want.push_str("tail\n");
        assert_eq!(
            out,
            CommandOutput {
                text: want,
                truncated: false,
                running: false
            }
        );
        assert_eq!(parser.screen().scrollback(), 0, "the view is restored");
        let out = command_text(&mut parser, 1, 3).unwrap();
        assert_eq!(out.text, "line11\nline12\ntail\n");
        assert!(out.truncated);
        assert!(command_text(&mut parser, 99, 3).is_none());
        // A running command with more output than the 12-line scrollback holds.
        parser.process(b"\x1b]133;B\x07");
        for n in 1..=30 {
            parser.process(format!("n{n}\r\n").as_bytes());
        }
        let running = command_text(&mut parser, 2, 100).unwrap();
        assert!(running.running && running.truncated, "{running:?}");
        assert!(running.text.ends_with("n30\n"), "{running:?}");
        assert!(!running.text.contains("n1\n"), "{running:?}");
        let old = command_text(&mut parser, 1, 100).unwrap();
        assert!(old.truncated, "the first command's start is gone too");
    }

    /// Malformed input: bad escapes stay literal, a `133;D` or `633;E` with no `133;B` make
    /// no record, an unparsable exit code is `None`, a repeated `633;E` / `133;B` changes nothing,
    /// and `133;C` moves the output's start to the cursor.
    #[test]
    fn command_ring_survives_malformed_osc() {
        let same = b"a\\xZZ \\x4 \\x+f \\q end\\";
        assert_eq!(unescape_command(same).as_bytes(), same);
        assert_eq!(unescape_command(b"\\x41\\\\x41"), "A\\x41");
        assert_eq!(unescape_command(b"\\xc3\\xa9"), "\u{e9}");
        assert_eq!(file_url_path(b"file://h/a%2"), Some(PathBuf::from("/a%2")));
        assert_eq!(file_url_path(b"file://h/a%+f"), Some(PathBuf::from("/a%+f")));
        assert_eq!(file_url_path(b"file://nopath"), None);
        assert_eq!(file_url_path(b"file://h/a%00b"), None);
        let (mut parser, _) = title_parser("bash");
        parser.process(b"\x1b]133;D;1\x07\x1b]633;E;x\x07");
        assert!(parser.callbacks().commands().is_empty());
        parser.process(b"\x1b]133;B\x07\x1b]633;E;first\x07\x1b]633;E;second\x07\x1b]133;B\x07\x1b]633;E\x07");
        parser.process(b"$ first\r\n\x1b]133;C\x07o\r\n\x1b]133;D;abc\x07");
        let cmds = parser.callbacks().commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command.as_deref(), Some("first"));
        assert_eq!(cmds[0].exit, None);
        assert!(cmds[0].finished.is_some());
        assert_eq!((cmds[0].start_row, cmds[0].end_row), (1, 2));
    }

    /// Soft-wrapped rows are joined into one line.
    #[test]
    fn command_output_joins_wrapped_rows() {
        let sink: Sink = Arc::new(|_| {});
        let cb = TitleCallback::new(RunId(1), sink, "t".into());
        let mut parser = vt100::Parser::new_with_callbacks(nonzero(5), nonzero(10), 12, cb);
        parser.process(b"\x1b]133;B\x07abcdefghijklmnopq\r\nz\r\n\x1b]133;D;0\x07");
        let out = command_text(&mut parser, 1, 100).unwrap();
        assert_eq!(out.text, "abcdefghijklmnopq\nz\n");
    }
}
