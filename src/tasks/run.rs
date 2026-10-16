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

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};

use super::templates::Spec;

/// Lines of output kept above the screen.
pub const SCROLLBACK: usize = 5_000;
/// At most one `Event::Output` per run this often.
pub const FRAME: Duration = Duration::from_millis(20);
/// `kill`: SIGHUP first, SIGKILL if the process is still there this much later.
pub const KILL_GRACE: Duration = Duration::from_secs(1);
/// After the process exits, how long to wait for the rest of its output.
const OUTPUT_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

/// What a run tells the app (through the `Sink`); its `status()` / `screen()` say the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The process is running.
    Started(RunId),
    /// New output is on the screen (at most one per `FRAME`).
    Output(RunId),
    /// The process exited (all its output read) or never started: `Exited` / `Failed`.
    Ended(RunId),
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

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
    parser: Mutex<vt100::Parser>,
    state: Mutex<State>,
    /// Output arrived since the last `Event::Output`.
    pending: AtomicBool,
    /// The reader is done (end of output) or there's none (failed to start).
    reader_done: AtomicBool,
    /// Wakes the notifier task (output, or the reader ending).
    notify: tokio::sync::Notify,
}

struct State {
    status: Status,
    /// The process (group) id while it can be signalled: set when spawned, until reaped.
    pid: Option<i32>,
    master: Option<Box<dyn MasterPty + Send>>,
    /// To the writer thread (the PTY's input), while the process runs.
    input: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// Input written while starting, sent once the writer thread exists.
    queued: Vec<u8>,
    /// The process group (= the first process's pid), kept after reaping: background processes
    /// it started may still hold the PTY (`reader_done` false), and dropping the run kills them.
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
    /// one the run fails at once). `sink` hears about it from other threads.
    pub fn start(id: RunId, spec: Spec, rows: u16, cols: u16, sink: Sink) -> Run {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let shared = Arc::new(Shared {
            id,
            parser: Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)),
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
    pub fn screen(&self) -> MutexGuard<'_, vt100::Parser> {
        lock(&self.shared.parser)
    }

    /// Resizes the screen and the PTY (the process gets SIGWINCH). Zero is taken as one.
    pub fn resize(&self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let state = lock(&self.shared.state);
        let mut parser = lock(&self.shared.parser);
        if parser.screen().size() == (rows, cols) {
            return;
        }
        parser.screen_mut().set_size(rows, cols);
        if let Some(master) = &state.master
            && let Err(e) = master.resize(pty_size(rows, cols))
        {
            tracing::debug!(run = self.id.0, "resize: {e}");
        }
    }

    /// Sends `bytes` to the process's terminal input (keys, a paste); while it's starting they're
    /// queued and sent once it runs (dropped if it fails to start). False once it ended.
    pub fn write(&self, bytes: &[u8]) -> bool {
        let mut state = lock(&self.shared.state);
        match (&state.status, &state.input) {
            (Status::Running, Some(input)) => input.send(bytes.to_vec()).is_ok(),
            (Status::Starting, _) => {
                state.queued.extend_from_slice(bytes);
                true
            }
            _ => false,
        }
    }

    /// Scrolls the view through the scrollback by `lines` (negative: up, to older output),
    /// clamped; 0 rows back is the live screen.
    pub fn scroll(&self, lines: isize) {
        let mut parser = self.screen();
        let back = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(back.saturating_add_signed(-lines));
    }

    /// Scrolls to the oldest output kept (`top`) or back to the live screen.
    pub fn scroll_to(&self, top: bool) {
        self.screen()
            .screen_mut()
            .set_scrollback(if top { usize::MAX } else { 0 });
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
                            // The leader died of the SIGHUP, but processes it started ignored it
                            // and still hold the terminal.
                            (Status::Exited(_), _, Some(group)) if !shared.reader_done.load(Ordering::Acquire) => {
                                signal_group(group, true)
                            }
                            _ => {}
                        }
                    });
                }
            }
            // Leftovers in its group still writing to the PTY.
            Status::Exited(_) if !self.reader_done.load(Ordering::Acquire) => {
                if let Some(group) = state.group {
                    signal_group(group, true);
                }
            }
            Status::Exited(_) | Status::Failed(_) => {}
        }
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

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Blocking: opens the PTY and spawns the process, then starts the reader and the waiter.
fn spawn(shared: Arc<Shared>, spec: Spec, sink: Sink) {
    if !spec.cwd.is_dir() {
        return shared.fail(format!("{}: no such folder", spec.cwd.display()), &sink);
    }
    let (rows, cols) = lock(&shared.parser).screen().size();
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
        let size = lock(&shared.parser).screen().size();
        if size != (rows, cols) {
            let _ = pair.master.resize(pty_size(size.0, size.1));
        }
        state.master = Some(pair.master);
        let (input, keys) = std::sync::mpsc::channel();
        let queued = std::mem::take(&mut state.queued);
        if !queued.is_empty() {
            let _ = input.send(queued);
        }
        let spawned = std::thread::Builder::new()
            .name(format!("task-{}-in", shared.id.0))
            .spawn(move || write_input(writer, keys));
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

/// Reader thread: PTY output into the screen until the end of output.
fn read_output(shared: Arc<Shared>, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                lock(&shared.parser).process(&buf[..n]);
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

/// Writer thread: input into the PTY until the run is dropped (or the terminal is gone).
fn write_input(mut writer: Box<dyn Write + Send>, input: std::sync::mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = input.recv() {
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
            return;
        }
    }
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
