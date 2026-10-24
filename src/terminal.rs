//! Entering and leaving the full-screen terminal mode, the terminal's cursor and pointer
//! shapes, and the window's identity (title, user vars, OSC 7).

use std::io::{self, Stdout, Write, stdout};
use std::path::{Path, PathBuf};
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
/// `start` sent an `Identity` (`restore` pops the title and clears the user vars).
static IDENTITY_SET: AtomicBool = AtomicBool::new(false);

/// What the terminal is told about jed's window: its title (`jed <workspace root>`, or `jed
/// <file>` in single file mode, `~` for home; `.workspace` `[window] title` replaces the root),
/// the user vars `jed` (always) and `jed_workspace`
/// (the root, workspace mode only; kitty's `--when-focus-on var:jed` scopes mappings to jed,
/// `kitten @ ls` finds the window by `jed_workspace`) and OSC 7's current directory (the root,
/// workspace mode only: kitty starts new windows opened from jed's in it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The default `jed <root>` (or `jed <file>`) title.
    title: String,
    /// `[window] title`, when set: `jed <title>` instead.
    title_override: Option<String>,
    workspace: Option<PathBuf>,
    host: String,
}

impl Identity {
    /// `single`: single file mode's file (the title shows it; no workspace var or OSC 7).
    pub fn new(root: &Path, single: Option<&Path>, home: &Path) -> Self {
        Self {
            title: format!("jed {}", crate::recent::display(single.unwrap_or(root), home)),
            title_override: None,
            workspace: single.is_none().then(|| root.to_path_buf()),
            host: hostname(),
        }
    }

    /// `.workspace` `[window] title`: names the window `jed <title>` instead of the root.
    pub fn with_title(mut self, title: Option<&str>) -> Self {
        self.set_title(title);
        self
    }

    /// Changes the title's override; `None` is the root's title again.
    pub fn set_title(&mut self, title: Option<&str>) {
        self.title_override = title.map(str::to_owned);
    }

    pub fn title_override(&self) -> Option<&str> {
        self.title_override.as_deref()
    }

    /// OSC 2: the window's title, control characters dropped; not wrapped (tmux takes titles
    /// itself). `start` sends it after pushing the title stack; a `.workspace` reload only
    /// re-sends it (`App::sync_window_title`).
    pub fn title_sequence(&self) -> String {
        let title = match &self.title_override {
            Some(title) => format!("jed {title}"),
            None => self.title.clone(),
        };
        let title: String = title.chars().filter(|c| !c.is_control()).collect();
        format!("\x1b]2;{title}\x1b\\")
    }

    /// The sequences `start` sends: push the title stack (`CSI 22;0 t`), the title (OSC 2), the
    /// user vars (OSC 1337 `SetUserVar`, base64 values, wrapped for tmux) and OSC 7.
    fn start(&self) -> String {
        let mut out = format!("\x1b[22;0t{}", self.title_sequence());
        out.push_str(&passthrough(&set_user_var("jed", Some(b"1"))));
        if let Some(root) = &self.workspace {
            use std::os::unix::ffi::OsStrExt;
            out.push_str(&passthrough(&set_user_var(
                "jed_workspace",
                Some(root.as_os_str().as_bytes()),
            )));
            out.push_str(&format!("\x1b]7;file://{}{}\x1b\\", self.host, percent_encode(root)));
        }
        out
    }
}

/// What `finish` sends after an `Identity`: the user vars cleared and the title popped. OSC 7
/// needs nothing: the shell sends its own at its next prompt.
fn identity_finish() -> String {
    let mut out = passthrough(&set_user_var("jed", None));
    out.push_str(&passthrough(&set_user_var("jed_workspace", None)));
    out.push_str("\x1b[23;0t");
    out
}

/// OSC 1337 `SetUserVar` (iTerm2's; kitty and WezTerm read it too, others ignore it): `None`
/// clears the var. The value's raw bytes are encoded (a non-UTF-8 root stays exact).
fn set_user_var(name: &str, value: Option<&[u8]>) -> String {
    match value {
        Some(v) => format!("\x1b]1337;SetUserVar={name}={}\x07", crate::clipboard::base64(v)),
        None => format!("\x1b]1337;SetUserVar={name}\x07"),
    }
}

/// `path`'s bytes for a `file://` URL: unreserved characters and `/` as they are, the rest `%XX`.
fn percent_encode(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// This machine's host name for OSC 7 (empty if it can't be read: `file:///path`).
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for `buf.len()` bytes; the name is NUL-terminated within it, or
    // truncated, which the scan below handles.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len])
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-.".contains(*c))
        .collect()
}

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
pub fn enter(kitty: bool, cursor: CursorShape, identity: &Identity) -> io::Result<Term> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = stdout();
    IDENTITY_SET.store(true, Ordering::SeqCst);
    start(&mut out, kitty, cursor, Some(identity))?;
    KITTY_PUSHED.store(kitty, Ordering::SeqCst);
    Terminal::new(CrosstermBackend::new(out))
}

/// What `enter` writes: the alternate screen, the window's `identity`, mouse capture, bracketed
/// paste, the kitty keyboard flags (when `kitty`) and the cursor's shape.
fn start(out: &mut impl Write, kitty: bool, cursor: CursorShape, identity: Option<&Identity>) -> io::Result<()> {
    execute!(out, EnterAlternateScreen)?;
    if let Some(identity) = identity {
        out.write_all(identity.start().as_bytes())?;
    }
    execute!(out, EnableMouseCapture, EnableBracketedPaste)?;
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
    let identity = IDENTITY_SET.swap(false, Ordering::SeqCst);
    let _ = finish(&mut stdout(), kitty, POINTER_SET.load(Ordering::SeqCst), identity);
    let _ = disable_raw_mode();
}

/// What `restore` writes: undoes `start` (the kitty flags when `kitty` pushed them, the pointer
/// when `pointer` was set, the title and user vars when `identity` was sent), the cursor back to
/// the user's shape (`CSI 0 q`) and shown.
fn finish(out: &mut impl Write, kitty: bool, pointer: bool, identity: bool) -> io::Result<()> {
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
    )?;
    if identity {
        out.write_all(identity_finish().as_bytes())?;
        out.flush()?;
    }
    Ok(())
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
        let entered = written(|o| start(o, false, line, None));
        assert!(
            entered.starts_with("\x1b[?1049h"),
            "the alternate screen first: {entered:?}"
        );
        assert!(entered.ends_with("\x1b[5 q"), "{entered:?}");
        let block = CursorShape {
            style: CursorStyle::Block,
            blinking: false,
        };
        let entered = written(|o| start(o, true, block, None));
        assert!(
            entered.ends_with("\x1b[>5u\x1b[2 q"),
            "after the kitty flags: {entered:?}"
        );

        let restored = written(|o| finish(o, false, false, false));
        assert!(restored.starts_with("\x1b[0 q"), "the user's shape: {restored:?}");
        assert!(restored.contains("\x1b[?1049l"), "{restored:?}");
        assert_eq!(restored.matches(" q").count(), 1, "{restored:?}");
        let restored = written(|o| finish(o, true, false, false));
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

    fn identity(root: &str, single: Option<&str>) -> Identity {
        Identity {
            host: "box.local".into(),
            ..Identity::new(Path::new(root), single.map(Path::new), Path::new("/Users/me"))
        }
    }

    fn cursor() -> CursorShape {
        CursorShape {
            style: CursorStyle::Line,
            blinking: true,
        }
    }

    #[test]
    fn start_names_the_window_and_finish_clears_it() {
        let id = identity("/Users/me/my 100%/é", None);
        let entered = written(|o| start(o, false, cursor(), Some(&id)));
        assert!(
            entered.starts_with("\x1b[?1049h\x1b[22;0t"),
            "the title pushed: {entered:?}"
        );
        assert!(entered.contains("\x1b]2;jed ~/my 100%/é\x1b\\"), "{entered:?}");
        let jed = passthrough("\x1b]1337;SetUserVar=jed=MQ==\x07");
        let root = passthrough("\x1b]1337;SetUserVar=jed_workspace=L1VzZXJzL21lL215IDEwMCUvw6k=\x07");
        assert!(entered.contains(&format!("{jed}{root}")), "both user vars: {entered:?}");
        assert!(
            entered.contains("\x1b]7;file://box.local/Users/me/my%20100%25/%C3%A9\x1b\\"),
            "OSC 7, percent-encoded: {entered:?}"
        );
        assert!(
            entered.ends_with("\x1b[5 q"),
            "the cursor shape still last: {entered:?}"
        );

        let restored = written(|o| finish(o, false, false, true));
        let cleared = format!(
            "{}{}\x1b[23;0t",
            passthrough("\x1b]1337;SetUserVar=jed\x07"),
            passthrough("\x1b]1337;SetUserVar=jed_workspace\x07")
        );
        assert!(restored.ends_with(&cleared), "vars cleared, title popped: {restored:?}");
        let plain = written(|o| finish(o, false, false, false));
        assert!(!plain.contains("1337") && !plain.contains("\x1b[23;0t"), "{plain:?}");
    }

    #[test]
    fn a_workspace_title_overrides_only_the_title() {
        let id = identity("/Users/me/proj", None).with_title(Some("api ~ box"));
        assert_eq!(id.title_sequence(), "\x1b]2;jed api ~ box\x1b\\");
        let entered = written(|o| start(o, false, cursor(), Some(&id)));
        assert!(
            entered.starts_with("\x1b[?1049h\x1b[22;0t\x1b]2;jed api ~ box\x1b\\"),
            "pushed once, then the override: {entered:?}"
        );
        assert!(
            entered.contains(&passthrough(
                "\x1b]1337;SetUserVar=jed_workspace=L1VzZXJzL21lL3Byb2o=\x07"
            )),
            "the real root: {entered:?}"
        );
        assert!(
            entered.contains("\x1b]7;file://box.local/Users/me/proj\x1b\\"),
            "{entered:?}"
        );
        assert_eq!(id.title_override(), Some("api ~ box"));
    }

    #[test]
    fn a_reload_re_sends_the_title_without_pushing_it() {
        let mut id = identity("/Users/me/proj", None);
        id.set_title(Some("api\x1b]2;evil\x07"));
        assert_eq!(
            id.title_sequence(),
            "\x1b]2;jed api]2;evil\x1b\\",
            "control characters dropped"
        );
        assert!(
            !id.title_sequence().contains("\x1b[22;0t"),
            "the title stack is pushed only at start"
        );
        id.set_title(None);
        assert_eq!(id.title_sequence(), "\x1b]2;jed ~/proj\x1b\\", "the root's title again");
    }

    #[test]
    fn a_non_utf8_root_is_sent_as_its_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let root = Path::new(OsStr::from_bytes(b"/tmp/a\xffb"));
        let id = Identity {
            host: String::new(),
            ..Identity::new(root, None, Path::new("/Users/me"))
        };
        let entered = id.start();
        // base64 of the bytes `/tmp/a\xffb`, not of a lossy U+FFFD.
        assert!(
            entered.contains(&passthrough("\x1b]1337;SetUserVar=jed_workspace=L3RtcC9h/2I=\x07")),
            "{entered:?}"
        );
        assert!(entered.contains("\x1b]7;file:///tmp/a%FFb\x1b\\"), "{entered:?}");
        assert!(entered.contains("\x1b]2;jed /tmp/a\u{fffd}b\x1b\\"), "{entered:?}");
    }

    #[test]
    fn single_file_mode_titles_the_file_without_a_workspace() {
        let id = identity("/Users/me/proj", Some("/Users/me/proj/notes.txt"));
        let entered = written(|o| start(o, false, cursor(), Some(&id)));
        assert!(entered.contains("\x1b]2;jed ~/proj/notes.txt\x1b\\"), "{entered:?}");
        assert!(entered.contains(&passthrough("\x1b]1337;SetUserVar=jed=MQ==\x07")));
        assert!(!entered.contains("jed_workspace"), "{entered:?}");
        assert!(!entered.contains("\x1b]7;"), "{entered:?}");
    }

    #[test]
    fn the_title_drops_control_characters() {
        let id = identity("/tmp/a\x1b]2;evil\x07b\u{9b}c\x7f", None);
        let title = id.start();
        assert!(title.contains("\x1b]2;jed /tmp/a]2;evilbc\x1b\\"), "{title:?}");
    }
}
