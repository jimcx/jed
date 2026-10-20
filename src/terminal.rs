//! Entering and leaving the full-screen terminal mode.

use std::io::{self, Stdout, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static KITTY_PUSHED: AtomicBool = AtomicBool::new(false);
/// A pointer shape other than the default was set (`restore` puts it back).
static POINTER_SET: AtomicBool = AtomicBool::new(false);

/// The mouse pointer's shape over jed's window (OSC 22, CSS names: kitty, foot, WezTerm,
/// Ghostty; other terminals ignore it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pointer {
    #[default]
    Default,
    /// Over a border dragged left / right (the side panel's edge, a side-by-side split).
    ColResize,
    /// Over a border dragged up / down (the bottom panel's top, a stacked split, Git History's).
    RowResize,
}

impl Pointer {
    fn name(self) -> &'static str {
        match self {
            Pointer::Default => "default",
            Pointer::ColResize => "ew-resize",
            Pointer::RowResize => "ns-resize",
        }
    }
}

/// Asks the terminal for `pointer`'s shape.
pub fn set_pointer(out: &mut impl Write, pointer: Pointer) -> io::Result<()> {
    POINTER_SET.store(pointer != Pointer::Default, Ordering::SeqCst);
    out.write_all(passthrough(&format!("\x1b]22;{}\x1b\\", pointer.name())).as_bytes())?;
    out.flush()
}

/// `seq` wrapped for tmux so it reaches the outer terminal (needs `set -g allow-passthrough on`).
pub fn passthrough(seq: &str) -> String {
    if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq.to_string()
    }
}

/// Asks the terminal whether it implements the kitty keyboard protocol.
pub fn kitty_supported() -> bool {
    crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
}

pub fn enter(kitty: bool) -> io::Result<Term> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    if kitty {
        execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
        KITTY_PUSHED.store(true, Ordering::SeqCst);
    }
    Terminal::new(CrosstermBackend::new(out))
}

/// Puts the terminal back as it was. Idempotent and safe to call from the panic hook.
pub fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = stdout();
    if KITTY_PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if POINTER_SET.load(Ordering::SeqCst) {
        let _ = set_pointer(&mut out, Pointer::Default);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pointer_sends_osc_22() {
        if std::env::var_os("TMUX").is_some() {
            return; // wrapped for tmux: covered by `passthrough`
        }
        let mut out = Vec::new();
        set_pointer(&mut out, Pointer::ColResize).unwrap();
        set_pointer(&mut out, Pointer::RowResize).unwrap();
        set_pointer(&mut out, Pointer::Default).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b]22;ew-resize\x1b\\\x1b]22;ns-resize\x1b\\\x1b]22;default\x1b\\"
        );
    }
}
