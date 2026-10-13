//! Entering and leaving the full-screen terminal mode.

use std::io::{self, Stdout, stdout};
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
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}
