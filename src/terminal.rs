//! Entering and leaving the full-screen terminal mode, and the terminal's cursor and pointer
//! shapes.

use std::io::{self, Stdout, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{SetCursorStyle, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::edit::CursorStyle;

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

/// The terminal cursor's shape: `cursor_style` and `cursor_blinking` (DECSCUSR `CSI n q`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorShape {
    pub style: CursorStyle,
    pub blinking: bool,
}

impl CursorShape {
    pub fn of(settings: &crate::edit::Settings) -> Self {
        Self {
            style: settings.cursor_style,
            blinking: settings.cursor_blinking,
        }
    }

    fn command(self) -> SetCursorStyle {
        match (self.style, self.blinking) {
            (CursorStyle::Line, true) => SetCursorStyle::BlinkingBar,
            (CursorStyle::Line, false) => SetCursorStyle::SteadyBar,
            (CursorStyle::Block, true) => SetCursorStyle::BlinkingBlock,
            (CursorStyle::Block, false) => SetCursorStyle::SteadyBlock,
            (CursorStyle::Underline, true) => SetCursorStyle::BlinkingUnderScore,
            (CursorStyle::Underline, false) => SetCursorStyle::SteadyUnderScore,
        }
    }
}

/// Asks the terminal for the cursor `shape`. Not wrapped for tmux: tmux takes DECSCUSR itself
/// and passes it on to the outer terminal (its `Ss` / `Se` capabilities; older tmux needs them in
/// `terminal-overrides`), per pane.
pub fn set_cursor_shape(out: &mut impl Write, shape: CursorShape) -> io::Result<()> {
    execute!(out, shape.command())
}

/// Asks the terminal whether it implements the kitty keyboard protocol.
pub fn kitty_supported() -> bool {
    crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
}

/// Raw mode, then `start`'s sequences on stdout. `cursor`: the settings' shape (`App::run`
/// sends it again when a `.workspace` reload changes it).
pub fn enter(kitty: bool, cursor: CursorShape) -> io::Result<Term> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = stdout();
    start(&mut out, kitty, cursor)?;
    KITTY_PUSHED.store(kitty, Ordering::SeqCst);
    Terminal::new(CrosstermBackend::new(out))
}

/// What `enter` writes: the alternate screen, mouse capture, bracketed paste, the kitty keyboard
/// flags (when `kitty`) and the cursor's shape.
fn start(out: &mut impl Write, kitty: bool, cursor: CursorShape) -> io::Result<()> {
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    if kitty {
        execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        )?;
    }
    set_cursor_shape(out, cursor)
}

/// Puts the terminal back as it was. Idempotent and safe to call from the panic hook.
pub fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let kitty = KITTY_PUSHED.swap(false, Ordering::SeqCst);
    let _ = finish(&mut stdout(), kitty, POINTER_SET.load(Ordering::SeqCst));
    let _ = disable_raw_mode();
}

/// What `restore` writes: undoes `start` (the kitty flags when `kitty` pushed them, the pointer
/// when `pointer` was set), the cursor back to the user's shape (`CSI 0 q`) and shown.
fn finish(out: &mut impl Write, kitty: bool, pointer: bool) -> io::Result<()> {
    if kitty {
        execute!(out, PopKeyboardEnhancementFlags)?;
    }
    if pointer {
        set_pointer(out, Pointer::Default)?;
    }
    execute!(
        out,
        SetCursorStyle::DefaultUserShape,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(f: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn each_cursor_shape_is_its_decscusr() {
        for (style, blinking, want) in [
            (CursorStyle::Block, true, "\x1b[1 q"),
            (CursorStyle::Block, false, "\x1b[2 q"),
            (CursorStyle::Underline, true, "\x1b[3 q"),
            (CursorStyle::Underline, false, "\x1b[4 q"),
            (CursorStyle::Line, true, "\x1b[5 q"),
            (CursorStyle::Line, false, "\x1b[6 q"),
        ] {
            let shape = CursorShape { style, blinking };
            assert_eq!(written(|o| set_cursor_shape(o, shape)), want, "{shape:?}");
        }
    }

    #[test]
    fn enter_sets_the_cursor_shape_and_restore_resets_it() {
        let line = CursorShape {
            style: CursorStyle::Line,
            blinking: true,
        };
        let entered = written(|o| start(o, false, line));
        assert!(
            entered.starts_with("\x1b[?1049h"),
            "the alternate screen first: {entered:?}"
        );
        assert!(entered.ends_with("\x1b[5 q"), "{entered:?}");
        let block = CursorShape {
            style: CursorStyle::Block,
            blinking: false,
        };
        let entered = written(|o| start(o, true, block));
        assert!(
            entered.ends_with("\x1b[>5u\x1b[2 q"),
            "after the kitty flags: {entered:?}"
        );

        let restored = written(|o| finish(o, false, false));
        assert!(restored.starts_with("\x1b[0 q"), "the user's shape: {restored:?}");
        assert!(restored.contains("\x1b[?1049l"), "{restored:?}");
        assert_eq!(restored.matches(" q").count(), 1, "{restored:?}");
        let restored = written(|o| finish(o, true, false));
        assert!(restored.starts_with("\x1b[<1u\x1b[0 q"), "{restored:?}");
    }

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
