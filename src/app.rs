//! Event loop. Owns all state and sends commands where they need to go.

use std::time::{Duration, Instant};

use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::buffer::Buffer;
use crate::buffer::pos::{self, DisplayCol};
use crate::command::Command;
use crate::config::keymap::{Keymap, Platform};
use crate::edit::{self, Settings};
use crate::selection::{Selection, Selections};
use crate::terminal::Term;
use crate::theme::{self, Theme};
use crate::view::View;
use crate::{panic, ui};

const SLOW_FRAME: Duration = Duration::from_millis(16);
const SLOW_SAVE: Duration = Duration::from_millis(500);
const WHEEL_LINES: isize = 3;

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

pub enum Prompt {
    SaveBeforeQuit,
}

/// Results from background work.
enum AppEvent {
    SaveDone {
        state: u64,
        result: Result<(), String>,
        elapsed: Duration,
    },
}

pub struct App {
    pub buffer: Buffer,
    pub view: View,
    pub keymap: Keymap,
    pub theme: Theme,
    pub settings: Settings,
    pub prompt: Option<Prompt>,
    pub message: Option<Message>,
    /// Text area from the last render, for mouse hit-testing.
    pub text_area: Rect,
    quit_after_save: bool,
    should_quit: bool,
    needs_redraw: bool,
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl App {
    pub fn new(buffer: Buffer, kitty: bool) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = Self {
            buffer,
            view: View::new(),
            keymap: Keymap::default_for(Platform::current()),
            theme: Theme::dark_plus(theme::terminal_has_truecolor()),
            settings: Settings::default(),
            prompt: None,
            message: None,
            text_area: Rect::default(),
            quit_after_save: false,
            should_quit: false,
            needs_redraw: true,
            tx,
            rx,
        };
        if let Some(reason) = &app.buffer.read_only {
            app.notify(Level::Warning, format!("Read-only: {reason}"));
        }
        if !kitty {
            app.notify(
                Level::Warning,
                "Terminal lacks the kitty keyboard protocol: some keys won't work. Quit with Ctrl+Q".into(),
            );
        }
        app
    }

    pub async fn run(&mut self, terminal: &mut Term) -> anyhow::Result<()> {
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
            tokio::select! {
                ev = events.next() => match ev {
                    Some(ev) => self.handle_event(ev?),
                    None => return Ok(()),
                },
                Some(msg) = self.rx.recv() => self.handle_app_event(msg),
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
    }

    fn handle_event(&mut self, ev: Event) {
        self.needs_redraw = true;
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                tracing::trace!(target: "input", ?key);
                self.message = None;
                if self.prompt.is_some() {
                    self.handle_prompt_key(key);
                } else if let Some(cmd) = self.keymap.lookup(&key) {
                    self.execute(cmd);
                } else if let KeyCode::Char(c) = key.code
                    && (key.modifiers - KeyModifiers::SHIFT).is_empty()
                    && self.writable()
                {
                    edit::insert_char(&mut self.buffer, &mut self.view, c);
                }
            }
            Event::Paste(text) => {
                tracing::trace!(target: "input", len = text.len(), "paste");
                if self.prompt.is_none() && self.writable() {
                    edit::paste(&mut self.buffer, &mut self.view, &text);
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
            Command::Save => self.save(),
            Command::Quit => self.request_quit(),
            _ if edit::is_edit(cmd) && !self.writable() => {}
            _ => {
                edit::run(cmd, &mut self.buffer, &mut self.view, &self.settings);
            }
        }
        tracing::debug!(elapsed_us = t.elapsed().as_micros() as u64);
    }

    fn writable(&mut self) -> bool {
        match self.buffer.read_only.clone() {
            Some(reason) => {
                self.notify(Level::Error, format!("Read-only: {reason}"));
                false
            }
            None => true,
        }
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp => self.view.scroll_lines(&self.buffer.rope, -WHEEL_LINES),
            MouseEventKind::ScrollDown => self.view.scroll_lines(&self.buffer.rope, WHEEL_LINES),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(at) = self.hit_test(m.column, m.row) {
                    self.view.selections = Selections::single(Selection::cursor(at));
                    self.buffer.seal_undo();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(at) = self.hit_test(m.column, m.row) {
                    let anchor = self.view.selections.primary().anchor;
                    self.view.selections = Selections::single(Selection::new(anchor, at));
                    self.view.reveal_cursor = true;
                }
            }
            _ => {}
        }
    }

    /// Screen cell to buffer position; clicks past the end of a line land at its end.
    fn hit_test(&self, x: u16, y: u16) -> Option<pos::ByteIdx> {
        let a = self.text_area;
        if y < a.y || y >= a.y + a.height || x + 1 < a.x {
            return None;
        }
        let rope = &self.buffer.rope;
        let line = (self.view.top_line + (y - a.y) as usize).min(pos::line_count(rope) - 1);
        let col = self.view.left_col + x.saturating_sub(a.x) as usize;
        Some(pos::byte_at_col(rope, line, DisplayCol(col), self.settings.tab_width))
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        match (self.prompt.as_ref(), key.code) {
            (Some(Prompt::SaveBeforeQuit), KeyCode::Char('s' | 'S') | KeyCode::Enter) => {
                self.prompt = None;
                self.quit_after_save = true;
                self.save();
            }
            (Some(Prompt::SaveBeforeQuit), KeyCode::Char('d' | 'D')) => self.should_quit = true,
            (Some(Prompt::SaveBeforeQuit), KeyCode::Char('c' | 'C') | KeyCode::Esc) => self.prompt = None,
            _ => {}
        }
    }

    fn save(&mut self) {
        if let Some(reason) = self.buffer.read_only.clone() {
            self.quit_after_save = false;
            self.notify(Level::Error, format!("Can't save, read-only: {reason}"));
            return;
        }
        if self.buffer.save_in_flight() {
            self.notify(Level::Info, "Save already in progress".into());
            return;
        }
        let job = self.buffer.begin_save();
        let state = job.state;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let result = match tokio::task::spawn_blocking(move || job.run()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::SaveDone {
                state,
                result,
                elapsed: t.elapsed(),
            });
        });
    }

    fn request_quit(&mut self) {
        if self.buffer.save_in_flight() {
            self.quit_after_save = true;
            self.notify(Level::Info, "Waiting for save to finish…".into());
        } else if self.buffer.is_dirty() {
            self.prompt = Some(Prompt::SaveBeforeQuit);
        } else {
            self.should_quit = true;
        }
    }

    fn handle_app_event(&mut self, ev: AppEvent) {
        self.needs_redraw = true;
        match ev {
            AppEvent::SaveDone { state, result, elapsed } => {
                if elapsed > SLOW_SAVE {
                    tracing::warn!(elapsed_ms = elapsed.as_millis() as u64, "slow save");
                }
                let ok = result.is_ok();
                self.buffer.finish_save(state, ok);
                match result {
                    Ok(()) => self.notify(Level::Info, format!("Saved {}", self.buffer.path.display())),
                    Err(e) => {
                        self.quit_after_save = false;
                        self.notify(Level::Error, format!("Save failed: {e}"));
                    }
                }
                if ok && std::mem::take(&mut self.quit_after_save) {
                    self.request_quit();
                }
            }
        }
    }

    fn update_crash_snapshot(&self) {
        let snapshots = if self.buffer.is_dirty() {
            vec![panic::Snapshot {
                path: std::path::absolute(&self.buffer.path).unwrap_or_else(|_| self.buffer.path.clone()),
                rope: self.buffer.rope.clone(),
                bom: self.buffer.bom,
            }]
        } else {
            Vec::new()
        };
        panic::set_dirty(snapshots);
    }
}
