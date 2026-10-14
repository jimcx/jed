//! Key combos to commands. Keymaps are data; commands never look at keys.
//! A binding applies in one context (editor, explorer, palette) or everywhere.
//! The defaults are VS Code's; `keybindings.toml` replaces bindings command by command.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::command::{Command, ExplorerCommand as X, Motion, PaletteCommand as P};

pub const FILE_NAME: &str = "keybindings.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// Modifiers in display and config order.
const MODIFIERS: [(KeyModifiers, &str, &str); 4] = [
    (KeyModifiers::SUPER, "cmd", "Cmd"),
    (KeyModifiers::CONTROL, "ctrl", "Ctrl"),
    (KeyModifiers::ALT, "alt", "Alt"),
    (KeyModifiers::SHIFT, "shift", "Shift"),
];

/// Named keys: config spelling, display label.
const NAMED_KEYS: [(KeyCode, &str, &str); 15] = [
    (KeyCode::Left, "left", "Left"),
    (KeyCode::Right, "right", "Right"),
    (KeyCode::Up, "up", "Up"),
    (KeyCode::Down, "down", "Down"),
    (KeyCode::Home, "home", "Home"),
    (KeyCode::End, "end", "End"),
    (KeyCode::PageUp, "pageup", "PageUp"),
    (KeyCode::PageDown, "pagedown", "PageDown"),
    (KeyCode::Backspace, "backspace", "Backspace"),
    (KeyCode::Delete, "delete", "Delete"),
    (KeyCode::Insert, "insert", "Insert"),
    (KeyCode::Enter, "enter", "Enter"),
    (KeyCode::Tab, "tab", "Tab"),
    (KeyCode::Esc, "esc", "Esc"),
    (KeyCode::Char(' '), "space", "Space"),
];

impl KeyCombo {
    /// Uppercase letters become lowercase + Shift, so `Cmd+Shift+Z` matches however the terminal reports it.
    pub fn from_event(ev: &KeyEvent) -> Self {
        let mut mods =
            ev.modifiers & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let code = match ev.code {
            KeyCode::Char(c) if c.is_uppercase() => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
            }
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            c => c,
        };
        Self { code, mods }
    }

    /// Config spelling, e.g. `cmd+shift+p`, `ctrl+alt+backslash`, `f2`: modifiers in the order
    /// cmd, ctrl, alt, shift (`super`, `control`, `option` also accepted); lowercase key names.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let lower = spec.trim().to_lowercase();
        let mut parts: Vec<&str> = lower.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|k| !k.is_empty()).ok_or("no key")?;
        let mut mods = KeyModifiers::NONE;
        for m in parts {
            mods |= match m {
                "cmd" | "super" | "command" => KeyModifiers::SUPER,
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "option" | "opt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("unknown modifier \"{m}\"")),
            };
        }
        let code = match key {
            "backslash" => KeyCode::Char('\\'),
            "plus" => KeyCode::Char('+'),
            "escape" => KeyCode::Esc,
            "return" => KeyCode::Enter,
            k => match NAMED_KEYS.iter().find(|(_, name, _)| *name == k) {
                Some((code, ..)) => *code,
                None => match (
                    k.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()),
                    k.chars().count(),
                ) {
                    (Some(n), _) if (1..=24).contains(&n) => KeyCode::F(n),
                    (_, 1) => KeyCode::Char(k.chars().next().unwrap_or(' ')),
                    _ => return Err(format!("unknown key \"{k}\"")),
                },
            },
        };
        Ok(Self { code, mods })
    }

    /// The config spelling `parse` reads (also the spelling in docs/terminals/README.md).
    pub fn spec(&self) -> String {
        self.join("+", |(_, spec, _)| spec, |code| key_name(code, false))
    }

    /// For people, e.g. `Cmd+Shift+P`.
    pub fn label(&self) -> String {
        self.join("+", |(_, _, label)| label, |code| key_name(code, true))
    }

    fn join(
        &self,
        sep: &str,
        modifier: impl Fn(&(KeyModifiers, &'static str, &'static str)) -> &'static str,
        key: impl Fn(KeyCode) -> String,
    ) -> String {
        let mut parts: Vec<String> = MODIFIERS
            .iter()
            .filter(|(m, ..)| self.mods.contains(*m))
            .map(|m| modifier(m).to_string())
            .collect();
        parts.push(key(self.code));
        parts.join(sep)
    }
}

fn key_name(code: KeyCode, label: bool) -> String {
    if let Some((_, spec, l)) = NAMED_KEYS.iter().find(|(c, ..)| *c == code) {
        return if label { l } else { spec }.to_string();
    }
    match code {
        KeyCode::Char('\\') if !label => "backslash".into(),
        KeyCode::Char('+') if !label => "plus".into(),
        KeyCode::Char(c) if label => c.to_uppercase().collect(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) if label => format!("F{n}"),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Editor,
    Explorer,
    /// The command palette and quick open, while open.
    Palette,
}

impl Context {
    const ALL: [(Context, &'static str); 3] = [
        (Context::Editor, "editor"),
        (Context::Explorer, "explorer"),
        (Context::Palette, "palette"),
    ];

    fn parse(name: &str) -> Option<Context> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// One key combo bound to a command, in a context (`None` = everywhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub combo: KeyCombo,
    pub context: Option<Context>,
    pub command: Command,
}

pub struct Keymap {
    /// `None` context = everywhere.
    bindings: HashMap<(KeyCombo, Option<Context>), Command>,
    /// The binding the palette shows for each command: the first one that still applies.
    shown: HashMap<Command, KeyCombo>,
}

/// `keybindings.toml`: a list of `[[binding]]` tables.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserFile {
    #[serde(default)]
    binding: Vec<UserBinding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserBinding {
    key: String,
    command: String,
    #[serde(default)]
    context: Option<String>,
}

impl Keymap {
    /// Every binding: combo, context (`None` = everywhere) and command.
    #[cfg(test)]
    pub fn bindings(&self) -> impl Iterator<Item = (KeyCombo, Option<Context>, Command)> + '_ {
        self.bindings.iter().map(|((combo, ctx), c)| (*combo, *ctx, *c))
    }

    /// A binding for the current context wins over a global one.
    pub fn lookup(&self, ev: &KeyEvent, context: Context) -> Option<Command> {
        let combo = KeyCombo::from_event(ev);
        self.bindings
            .get(&(combo, Some(context)))
            .or_else(|| self.bindings.get(&(combo, None)))
            .copied()
    }

    /// The key shown next to a command in the palette.
    pub fn binding_for(&self, cmd: Command) -> Option<KeyCombo> {
        self.shown.get(&cmd).copied()
    }

    /// Later bindings win on the same key and context.
    fn from_list(list: Vec<Binding>) -> Self {
        let mut bindings = HashMap::new();
        for b in &list {
            bindings.insert((b.combo, b.context), b.command);
        }
        let mut shown = HashMap::new();
        for b in &list {
            if bindings.get(&(b.combo, b.context)) == Some(&b.command) {
                shown.entry(b.command).or_insert(b.combo);
            }
        }
        Self { bindings, shown }
    }

    pub fn default_for(platform: Platform) -> Self {
        Self::from_list(defaults(platform))
    }

    /// The defaults, with each command named in `user` bound only to the user's keys.
    /// A user key also takes over whatever the default had on it, in that context.
    pub fn with_user(platform: Platform, user: &[Binding]) -> Self {
        let rebound: HashSet<Command> = user.iter().map(|b| b.command).collect();
        let mut list: Vec<Binding> = defaults(platform)
            .into_iter()
            .filter(|b| !rebound.contains(&b.command))
            .collect();
        list.extend_from_slice(user);
        Self::from_list(list)
    }

    /// Blocking. The keymap for `platform` with the user's `keybindings.toml` (if any) on top,
    /// plus one message per problem. Bad entries are skipped; an unreadable file gives the defaults.
    pub fn load(path: &Path, platform: Platform) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (user, errors) = parse_user(&text);
                for b in &user {
                    tracing::info!(key = %b.combo.spec(), command = %b.command.name(), context = ?b.context, "user keybinding");
                }
                (Self::with_user(platform, &user), errors)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default_for(platform), Vec::new()),
            Err(e) => (Self::default_for(platform), vec![format!("{FILE_NAME}: {e}")]),
        }
    }
}

/// The bindings in a `keybindings.toml`, and a message for each one that couldn't be used.
fn parse_user(text: &str) -> (Vec<Binding>, Vec<String>) {
    let file: UserFile = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => return (Vec::new(), vec![format!("{FILE_NAME}: {}", e.message())]),
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for b in file.binding {
        let combo = KeyCombo::parse(&b.key).map_err(|e| format!("key \"{}\": {e}", b.key));
        let command = Command::from_name(&b.command).ok_or_else(|| format!("unknown command \"{}\"", b.command));
        let context = match b.context.as_deref() {
            None => Ok(None),
            Some(name) => Context::parse(name).map(Some).ok_or_else(|| {
                let names: Vec<&str> = Context::ALL.iter().map(|(_, n)| *n).collect();
                format!("unknown context \"{name}\" (one of {})", names.join(", "))
            }),
        };
        match (combo, command, context) {
            (Ok(combo), Ok(command), Ok(context)) => out.push(Binding {
                combo,
                context,
                command,
            }),
            (combo, command, context) => {
                let e = [combo.err(), command.err(), context.err()].into_iter().flatten();
                errors.extend(e.map(|e| format!("{FILE_NAME}: {e}")));
            }
        }
    }
    (out, errors)
}

/// VS Code's default bindings, in priority order for what the palette shows.
fn defaults(platform: Platform) -> Vec<Binding> {
    let none = KeyModifiers::NONE;
    let shift = KeyModifiers::SHIFT;
    let ctrl = KeyModifiers::CONTROL;
    let alt = KeyModifiers::ALT;
    let mac = platform == Platform::MacOs;
    // Cmd on macOS, Ctrl on Linux.
    let cmd = if mac {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    };
    // Word-wise movement: Alt on macOS, Ctrl on Linux.
    let word = if mac { KeyModifiers::ALT } else { KeyModifiers::CONTROL };

    let mut editor: Vec<(KeyCode, KeyModifiers, Command)> = Vec::new();
    let mut motion = |code: KeyCode, mods: KeyModifiers, m: Motion| {
        editor.push((
            code,
            mods,
            Command::Move {
                motion: m,
                select: false,
            },
        ));
        editor.push((
            code,
            mods | shift,
            Command::Move {
                motion: m,
                select: true,
            },
        ));
    };
    motion(KeyCode::Left, none, Motion::Left);
    motion(KeyCode::Right, none, Motion::Right);
    motion(KeyCode::Up, none, Motion::Up);
    motion(KeyCode::Down, none, Motion::Down);
    motion(KeyCode::Left, word, Motion::WordLeft);
    motion(KeyCode::Right, word, Motion::WordRight);
    motion(KeyCode::Home, none, Motion::LineStart);
    motion(KeyCode::End, none, Motion::LineEnd);
    motion(KeyCode::PageUp, none, Motion::PageUp);
    motion(KeyCode::PageDown, none, Motion::PageDown);
    if mac {
        motion(KeyCode::Left, cmd, Motion::LineStart);
        motion(KeyCode::Right, cmd, Motion::LineEnd);
        motion(KeyCode::Up, cmd, Motion::DocStart);
        motion(KeyCode::Down, cmd, Motion::DocEnd);
    } else {
        motion(KeyCode::Home, ctrl, Motion::DocStart);
        motion(KeyCode::End, ctrl, Motion::DocEnd);
    }
    editor.extend([
        (KeyCode::Backspace, none, Command::DeleteLeft),
        (KeyCode::Backspace, shift, Command::DeleteLeft),
        (KeyCode::Delete, none, Command::DeleteRight),
        (KeyCode::Backspace, word, Command::DeleteWordLeft),
        (KeyCode::Delete, word, Command::DeleteWordRight),
        (KeyCode::Enter, none, Command::InsertNewline),
        (KeyCode::Tab, none, Command::InsertTab),
        (KeyCode::Esc, none, Command::CancelSelection),
        (KeyCode::Char('a'), cmd, Command::SelectAll),
        (KeyCode::Char('z'), cmd, Command::Undo),
        (KeyCode::Char('z'), cmd | shift, Command::Redo),
        (KeyCode::Char('c'), cmd, Command::Copy),
        (KeyCode::Char('x'), cmd, Command::Cut),
        (KeyCode::Char('d'), cmd, Command::AddNextMatch),
        (KeyCode::Char('l'), cmd | shift, Command::SelectAllMatches),
        // No chords, so not VS Code's Cmd+K Cmd+D.
        (KeyCode::Char('d'), cmd | alt, Command::SkipToNextMatch),
        (KeyCode::Char('u'), cmd, Command::CursorUndo),
    ]);
    if mac {
        editor.push((KeyCode::Backspace, cmd, Command::DeleteToLineStart));
    } else {
        editor.push((KeyCode::Char('y'), ctrl, Command::Redo));
        // macOS leaves Cmd+V to the terminal: it arrives as a bracketed paste.
        editor.push((KeyCode::Char('v'), ctrl, Command::Paste));
    }

    let mut explorer = vec![
        (KeyCode::Up, none, X::Up),
        (KeyCode::Down, none, X::Down),
        (KeyCode::PageUp, none, X::PageUp),
        (KeyCode::PageDown, none, X::PageDown),
        (KeyCode::Home, none, X::First),
        (KeyCode::End, none, X::Last),
        (KeyCode::Left, none, X::Left),
        (KeyCode::Right, none, X::Right),
        (KeyCode::Enter, none, X::Open),
        (KeyCode::Char(' '), none, X::Preview),
        (KeyCode::F(2), none, X::Rename),
        (KeyCode::Delete, none, X::Delete),
        (KeyCode::Backspace, cmd, X::Delete),
        (KeyCode::Char('n'), cmd | alt, X::NewFile),
        (KeyCode::Char('n'), cmd | alt | shift, X::NewFolder),
    ];
    if mac {
        explorer.push((KeyCode::Down, cmd, X::Open));
    }

    let mut global = vec![
        (KeyCode::Char('s'), cmd, Command::Save),
        (KeyCode::Char('q'), cmd, Command::Quit),
        // Works in any terminal, even without the kitty protocol.
        (KeyCode::Char('q'), ctrl, Command::Quit),
        (KeyCode::Char('w'), cmd, Command::CloseTab),
        (KeyCode::Char('b'), cmd, Command::ToggleSidebar),
        (KeyCode::Char('e'), cmd | shift, Command::FocusExplorer),
        (KeyCode::Char('1'), cmd, Command::FocusGroup(1)),
        (KeyCode::Char('2'), cmd, Command::FocusGroup(2)),
        (KeyCode::Char('3'), cmd, Command::FocusGroup(3)),
        (KeyCode::Char('\\'), cmd, Command::SplitRight),
        // No chords, so not VS Code's Cmd+K Cmd+\\.
        (KeyCode::Char('\\'), cmd | alt, Command::SplitDown),
        (KeyCode::PageDown, ctrl, Command::NextTab),
        (KeyCode::PageUp, ctrl, Command::PrevTab),
    ];
    if mac {
        global.push((KeyCode::Right, cmd | alt, Command::NextTab));
        global.push((KeyCode::Left, cmd | alt, Command::PrevTab));
    }
    global.extend([
        (KeyCode::Char('p'), cmd | shift, Command::ShowCommands),
        (KeyCode::F(1), none, Command::ShowCommands),
        (KeyCode::Char('p'), cmd, Command::QuickOpen),
        // Ctrl on macOS too, as in VS Code.
        (KeyCode::Char('g'), ctrl, Command::GoToLine),
    ]);

    let palette = [
        (KeyCode::Esc, none, P::Close),
        (KeyCode::Enter, none, P::Accept),
        (KeyCode::Down, none, P::Next),
        (KeyCode::Up, none, P::Previous),
        (KeyCode::PageDown, none, P::PageDown),
        (KeyCode::PageUp, none, P::PageUp),
    ];

    let bind = |code, mods, context, command| Binding {
        combo: KeyCombo { code, mods },
        context,
        command,
    };
    let mut out = Vec::new();
    out.extend(global.into_iter().map(|(k, m, c)| bind(k, m, None, c)));
    out.extend(editor.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Editor), c)));
    out.extend(
        explorer
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Explorer), Command::Explorer(c))),
    );
    out.extend(
        palette
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Palette), Command::Palette(c))),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn shifted_letters_normalize() {
        let km = Keymap::default_for(Platform::MacOs);
        let redo = Some(Command::Redo);
        let ed = Context::Editor;
        assert_eq!(km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER), ed), redo);
        assert_eq!(
            km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER | KeyModifiers::SHIFT), ed),
            redo
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT), ed),
            redo
        );
    }

    #[test]
    fn platform_modifiers() {
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let ed = Context::Editor;
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::SUPER), ed),
            Some(Command::Save)
        );
        assert_eq!(mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), ed), None);
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), ed),
            Some(Command::Save)
        );
    }

    #[test]
    fn context_bindings_win_over_global_and_stay_in_context() {
        let km = Keymap::default_for(Platform::MacOs);
        let down = key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(km.lookup(&down, Context::Explorer), Some(Command::Explorer(X::Down)));
        assert_eq!(
            km.lookup(&down, Context::Editor),
            Some(Command::Move {
                motion: Motion::Down,
                select: false
            })
        );
        let save = key(KeyCode::Char('s'), KeyModifiers::SUPER);
        assert_eq!(km.lookup(&save, Context::Explorer), Some(Command::Save));
    }

    #[test]
    fn specs_parse_back_and_label() {
        for platform in [Platform::MacOs, Platform::Linux] {
            for (combo, ..) in Keymap::default_for(platform).bindings() {
                assert_eq!(KeyCombo::parse(&combo.spec()), Ok(combo), "{}", combo.spec());
            }
        }
        let p = KeyCombo::parse("Shift+Cmd+P").unwrap();
        assert_eq!(p.spec(), "cmd+shift+p");
        assert_eq!(p.label(), "Cmd+Shift+P");
        assert_eq!(KeyCombo::parse("ctrl+alt+backslash").unwrap().label(), "Ctrl+Alt+\\");
        assert_eq!(KeyCombo::parse("escape").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyCombo::parse("f12").unwrap().label(), "F12");
        assert!(KeyCombo::parse("hyper+x").is_err());
        assert!(KeyCombo::parse("cmd+").is_err());
        assert!(KeyCombo::parse("cmd+nope").is_err());
    }

    #[test]
    fn user_bindings_replace_defaults_per_command() {
        let toml = r#"
            [[binding]]
            key = "ctrl+alt+s"
            command = "file.save"

            [[binding]]
            key = "cmd+b"
            command = "edit.undo"
            context = "editor"

            [[binding]]
            key = "cmd+k"
            command = "nope"

            [[binding]]
            key = "hyper+k"
            command = "file.save"
            context = "terminal"
        "#;
        let (user, errors) = parse_user(toml);
        assert_eq!(user.len(), 2);
        assert_eq!(errors.len(), 3, "{errors:?}");
        let km = Keymap::with_user(Platform::MacOs, &user);
        let ed = Context::Editor;
        let save = KeyCombo::parse("ctrl+alt+s").unwrap();
        assert_eq!(km.lookup(&key(save.code, save.mods), ed), Some(Command::Save));
        assert_eq!(
            km.lookup(&key(KeyCode::Char('s'), KeyModifiers::SUPER), ed),
            None,
            "default replaced"
        );
        assert_eq!(km.binding_for(Command::Save), Some(save));

        // Cmd+B is Undo in the editor, still Toggle Sidebar elsewhere; Cmd+Z no longer undoes.
        let cmd_b = key(KeyCode::Char('b'), KeyModifiers::SUPER);
        assert_eq!(km.lookup(&cmd_b, ed), Some(Command::Undo));
        assert_eq!(km.lookup(&cmd_b, Context::Explorer), Some(Command::ToggleSidebar));
        assert_eq!(km.lookup(&key(KeyCode::Char('z'), KeyModifiers::SUPER), ed), None);
        assert_eq!(
            km.binding_for(Command::ToggleSidebar).map(|c| c.label()).as_deref(),
            Some("Cmd+B")
        );

        let (_, errors) = parse_user("[[binding]]\nkey = 1\n");
        assert_eq!(errors.len(), 1, "a bad file is one error");
    }

    #[test]
    fn missing_file_is_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (km, errors) = Keymap::load(&dir.path().join(FILE_NAME), Platform::MacOs);
        assert!(errors.is_empty());
        assert_eq!(
            km.binding_for(Command::Save).map(|c| c.label()).as_deref(),
            Some("Cmd+S")
        );
    }

    #[test]
    fn every_bound_command_is_listed() {
        let all = Command::all();
        for platform in [Platform::MacOs, Platform::Linux] {
            for (combo, _, c) in Keymap::default_for(platform).bindings() {
                assert!(
                    all.contains(&c),
                    "{} bound to {c:?}, which Command::all() lacks",
                    combo.spec()
                );
            }
        }
    }

    /// The terminal docs must list exactly the macOS Cmd bindings, so the
    /// per-terminal snippets can't silently fall behind the keymap.
    #[test]
    fn terminal_docs_list_every_cmd_binding() {
        let readme = include_str!("../../docs/terminals/README.md");
        let documented: std::collections::BTreeSet<String> = readme
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .filter(|k| k.starts_with("cmd+"))
            .map(str::to_string)
            .collect();
        let bound: std::collections::BTreeSet<String> = Keymap::default_for(Platform::MacOs)
            .bindings()
            .filter(|(combo, ..)| combo.mods.contains(KeyModifiers::SUPER))
            .map(|(combo, ..)| combo.spec())
            .collect();
        let missing: Vec<_> = bound.difference(&documented).collect();
        let stale: Vec<_> = documented.difference(&bound).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "docs/terminals/README.md is out of date (update it and every snippet).\nmissing: {missing:?}\nstale: {stale:?}"
        );
    }
}
