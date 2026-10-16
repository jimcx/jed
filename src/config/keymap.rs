//! Key combos to commands. Keymaps are data; commands never look at keys.
//! A binding applies in one context (editor, explorer, palette, find, replace, search,
//! searchResults, problems, locations, suggest) or everywhere.
//! The defaults are VS Code's; `keybindings.toml` replaces bindings command by command.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::command::{
    ColumnMotion, Command, ExplorerCommand as X, FindCommand as F, LocationsCommand as L, Motion, PaletteCommand as P,
    ProblemsCommand as Pr, SearchCommand as S, SuggestCommand as Sg,
};

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

/// Shift+1 … Shift+0 on a US keyboard.
const SHIFTED_DIGITS: &str = "!@#$%^&*()";

impl KeyCombo {
    /// Uppercase letters become lowercase + Shift, so `Cmd+Shift+Z` matches however the terminal reports it.
    /// With Cmd, Ctrl or Alt held, a shifted digit (US layout: `!` … `)`) becomes the digit + Shift,
    /// since kitty-protocol terminals report Cmd+Shift+1 as Cmd+`!`.
    pub fn from_event(ev: &KeyEvent) -> Self {
        let mut mods =
            ev.modifiers & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let chord = mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let code = match ev.code {
            // Ctrl+Space as a legacy terminal may report it (NUL, read as Ctrl+@ or Ctrl+Shift+2).
            KeyCode::Char('@') if mods - KeyModifiers::SHIFT == KeyModifiers::CONTROL => {
                mods = KeyModifiers::CONTROL;
                KeyCode::Char(' ')
            }
            KeyCode::Char(c) if c.is_uppercase() => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
            }
            KeyCode::Char(c) if chord && SHIFTED_DIGITS.contains(c) => {
                mods |= KeyModifiers::SHIFT;
                let i = SHIFTED_DIGITS.chars().position(|d| d == c).unwrap_or(0);
                KeyCode::Char(char::from(b'0' + ((i + 1) % 10) as u8))
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
        // As a key event would arrive: `ctrl+!` is `ctrl+shift+1`, or it could never match.
        Ok(Self::from_event(&KeyEvent::new(code, mods)))
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
    /// The find widget's find input has the focus.
    Find,
    /// Its replace input has the focus; bindings not found here come from `Find`.
    Replace,
    /// The Search view's query input has the focus.
    Search,
    /// The Search view's results list has the focus; bindings not found here come from `Search`.
    SearchResults,
    /// The bottom panel's Problems list has the focus.
    Problems,
    /// The bottom panel's Locations list (definitions, references) has the focus.
    Locations,
    /// The editor has the focus and the completion list is shown; bindings not found here come
    /// from `Editor`.
    Suggest,
}

impl Context {
    const ALL: [(Context, &'static str); 10] = [
        (Context::Editor, "editor"),
        (Context::Explorer, "explorer"),
        (Context::Palette, "palette"),
        (Context::Find, "find"),
        (Context::Replace, "replace"),
        (Context::Search, "search"),
        (Context::SearchResults, "searchResults"),
        (Context::Problems, "problems"),
        (Context::Locations, "locations"),
        (Context::Suggest, "suggest"),
    ];

    fn parse(name: &str) -> Option<Context> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }

    /// The context looked at next, before the global bindings.
    fn parent(self) -> Option<Context> {
        match self {
            Context::Replace => Some(Context::Find),
            Context::SearchResults => Some(Context::Search),
            Context::Suggest => Some(Context::Editor),
            _ => None,
        }
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

    /// A binding for the current context wins over its parent's (replace → find), which wins
    /// over a global one.
    pub fn lookup(&self, ev: &KeyEvent, context: Context) -> Option<Command> {
        let combo = KeyCombo::from_event(ev);
        let mut ctx = Some(context);
        while let Some(c) = ctx {
            if let Some(cmd) = self.bindings.get(&(combo, Some(c))) {
                return Some(*cmd);
            }
            ctx = c.parent();
        }
        self.bindings.get(&(combo, None)).copied()
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
        (KeyCode::Up, cmd | alt, Command::AddCursorAbove),
        (KeyCode::Down, cmd | alt, Command::AddCursorBelow),
        (KeyCode::Char('i'), alt | shift, Command::CursorsAtLineEnds),
        (
            KeyCode::Left,
            cmd | alt | shift,
            Command::ColumnSelect(ColumnMotion::Left),
        ),
        (
            KeyCode::Right,
            cmd | alt | shift,
            Command::ColumnSelect(ColumnMotion::Right),
        ),
        (KeyCode::Up, cmd | alt | shift, Command::ColumnSelect(ColumnMotion::Up)),
        (
            KeyCode::Down,
            cmd | alt | shift,
            Command::ColumnSelect(ColumnMotion::Down),
        ),
        (
            KeyCode::PageUp,
            cmd | alt | shift,
            Command::ColumnSelect(ColumnMotion::PageUp),
        ),
        (
            KeyCode::PageDown,
            cmd | alt | shift,
            Command::ColumnSelect(ColumnMotion::PageDown),
        ),
        (KeyCode::Up, alt, Command::MoveLinesUp),
        (KeyCode::Down, alt, Command::MoveLinesDown),
        (KeyCode::Up, alt | shift, Command::CopyLinesUp),
        (KeyCode::Down, alt | shift, Command::CopyLinesDown),
        (KeyCode::Char('/'), cmd, Command::CommentLine),
        // No chords, so not VS Code's Cmd+K Cmd+I.
        (KeyCode::Char('i'), cmd | alt, Command::ShowHover),
        (KeyCode::F(12), none, Command::RevealDefinition),
        (KeyCode::F(12), shift, Command::GoToReferences),
        // The explorer's F2 renames a file (its own context).
        (KeyCode::F(2), none, Command::RenameSymbol),
        // Ctrl on macOS too, as in VS Code.
        (KeyCode::Char(' '), ctrl, Command::TriggerSuggest),
    ]);
    // The find widget: opening it, and what also works from the editor (VS Code: `editorFocus`).
    // VS Code's toggles are Cmd+Alt+C / W / R on macOS, Alt+C / W / R on Linux.
    let toggle = if mac { cmd | alt } else { alt };
    let open_replace = if mac {
        (KeyCode::Char('f'), cmd | alt)
    } else {
        (KeyCode::Char('h'), ctrl)
    };
    let find_keys = [
        (KeyCode::Char('f'), cmd, Command::OpenFind),
        (open_replace.0, open_replace.1, Command::OpenReplace),
        (KeyCode::F(3), none, Command::Find(F::Next)),
        (KeyCode::F(3), shift, Command::Find(F::Previous)),
        (KeyCode::Char('c'), toggle, Command::Find(F::ToggleCase)),
        (KeyCode::Char('w'), toggle, Command::Find(F::ToggleWholeWord)),
        (KeyCode::Char('r'), toggle, Command::Find(F::ToggleRegex)),
        (KeyCode::Char('1'), cmd | shift, Command::Find(F::ReplaceOne)),
        (KeyCode::Enter, cmd | alt, Command::Find(F::ReplaceAll)),
        (KeyCode::Enter, alt, Command::Find(F::SelectAllMatches)),
    ];
    editor.extend(find_keys);
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
        (KeyCode::Char('f'), cmd | shift, Command::FindInFiles),
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
        (KeyCode::F(4), none, Command::Search(S::NextResult)),
        (KeyCode::F(4), shift, Command::Search(S::PreviousResult)),
        (KeyCode::Char('j'), cmd, Command::TogglePanel),
        (KeyCode::Char('m'), cmd | shift, Command::ToggleProblems),
        (KeyCode::F(8), none, Command::Problems(Pr::Next)),
        (KeyCode::F(8), shift, Command::Problems(Pr::Previous)),
    ]);

    let palette = [
        (KeyCode::Esc, none, P::Close),
        (KeyCode::Enter, none, P::Accept),
        (KeyCode::Down, none, P::Next),
        (KeyCode::Up, none, P::Previous),
        (KeyCode::PageDown, none, P::PageDown),
        (KeyCode::PageUp, none, P::PageUp),
    ];

    // The find input, then the replace input (which falls back to the find input's keys).
    let mut find = vec![
        (KeyCode::Enter, none, Command::Find(F::Next)),
        (KeyCode::Enter, shift, Command::Find(F::Previous)),
        (KeyCode::Esc, none, Command::Find(F::Close)),
        (KeyCode::Tab, none, Command::Find(F::SwitchField)),
        (KeyCode::Tab, shift, Command::Find(F::SwitchField)),
    ];
    find.extend(find_keys);
    let replace = [(KeyCode::Enter, none, Command::Find(F::ReplaceOne))];

    // The Search view's input: the find widget's toggle keys (no whole word in v1).
    let search = [
        (KeyCode::Char('c'), toggle, S::ToggleCase),
        (KeyCode::Char('r'), toggle, S::ToggleRegex),
        (KeyCode::Enter, none, S::Refresh),
        (KeyCode::Esc, none, S::FocusEditor),
        (KeyCode::Down, none, S::FocusResults),
    ];
    // Its results list (falls back to the input's keys: toggles, Esc).
    let search_results = [
        (KeyCode::Up, none, S::Up),
        (KeyCode::Down, none, S::Down),
        (KeyCode::PageUp, none, S::PageUp),
        (KeyCode::PageDown, none, S::PageDown),
        (KeyCode::Home, none, S::First),
        (KeyCode::End, none, S::Last),
        (KeyCode::Enter, none, S::Open),
    ];

    // The Problems list in the bottom panel.
    let problems = [
        (KeyCode::Up, none, Pr::Up),
        (KeyCode::Down, none, Pr::Down),
        (KeyCode::PageUp, none, Pr::PageUp),
        (KeyCode::PageDown, none, Pr::PageDown),
        (KeyCode::Home, none, Pr::First),
        (KeyCode::End, none, Pr::Last),
        (KeyCode::Enter, none, Pr::Open),
        (KeyCode::Esc, none, Pr::FocusEditor),
    ];

    // The Locations list in the bottom panel: the Problems list's keys.
    let locations = [
        (KeyCode::Up, none, L::Up),
        (KeyCode::Down, none, L::Down),
        (KeyCode::PageUp, none, L::PageUp),
        (KeyCode::PageDown, none, L::PageDown),
        (KeyCode::Home, none, L::First),
        (KeyCode::End, none, L::Last),
        (KeyCode::Enter, none, L::Open),
        (KeyCode::Esc, none, L::FocusEditor),
    ];

    // The completion list, while shown (falls back to the editor's keys).
    let suggest = [
        (KeyCode::Up, none, Sg::Up),
        (KeyCode::Down, none, Sg::Down),
        (KeyCode::PageUp, none, Sg::PageUp),
        (KeyCode::PageDown, none, Sg::PageDown),
        (KeyCode::Enter, none, Sg::Accept),
        (KeyCode::Tab, none, Sg::Accept),
        (KeyCode::Esc, none, Sg::Close),
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
    out.extend(find.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Find), c)));
    out.extend(
        replace
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Replace), c)),
    );
    out.extend(
        search
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Search), Command::Search(c))),
    );
    out.extend(
        search_results
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::SearchResults), Command::Search(c))),
    );
    out.extend(
        problems
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Problems), Command::Problems(c))),
    );
    out.extend(
        locations
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Locations), Command::Locations(c))),
    );
    out.extend(
        suggest
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Suggest), Command::Suggest(c))),
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
    fn find_contexts() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            let enter = key(KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(km.lookup(&enter, Context::Find), Some(Command::Find(F::Next)));
            assert_eq!(km.lookup(&enter, Context::Replace), Some(Command::Find(F::ReplaceOne)));
            assert_eq!(km.lookup(&enter, Context::Editor), Some(Command::InsertNewline));
            // The replace input falls back to the find input's keys, then the global ones.
            let esc = key(KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(km.lookup(&esc, Context::Replace), Some(Command::Find(F::Close)));
            let cmd = if platform == Platform::MacOs {
                KeyModifiers::SUPER
            } else {
                KeyModifiers::CONTROL
            };
            let save = key(KeyCode::Char('s'), cmd);
            assert_eq!(km.lookup(&save, Context::Replace), Some(Command::Save));
            // Typing goes to the input: no plain characters are bound there.
            let x = key(KeyCode::Char('x'), KeyModifiers::NONE);
            assert_eq!(km.lookup(&x, Context::Find), None);
            assert_eq!(km.lookup(&x, Context::Replace), None);
        }
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let case = Some(Command::Find(F::ToggleCase));
        let cmd_alt = KeyModifiers::SUPER | KeyModifiers::ALT;
        assert_eq!(mac.lookup(&key(KeyCode::Char('c'), cmd_alt), Context::Find), case);
        assert_eq!(mac.lookup(&key(KeyCode::Char('c'), cmd_alt), Context::Editor), case);
        let alt_c = key(KeyCode::Char('c'), KeyModifiers::ALT);
        assert_eq!(linux.lookup(&alt_c, Context::Replace), case);
        let ctrl_h = key(KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert_eq!(linux.lookup(&ctrl_h, Context::Editor), Some(Command::OpenReplace));
        assert_eq!(
            mac.binding_for(Command::Find(F::Next)).map(|k| k.label()).as_deref(),
            Some("F3")
        );
    }

    #[test]
    fn search_context() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            let (cmd, toggle) = if platform == Platform::MacOs {
                (KeyModifiers::SUPER, KeyModifiers::SUPER | KeyModifiers::ALT)
            } else {
                (KeyModifiers::CONTROL, KeyModifiers::ALT)
            };
            let s = Context::Search;
            let f = key(KeyCode::Char('f'), cmd | KeyModifiers::SHIFT);
            assert_eq!(km.lookup(&f, s), Some(Command::FindInFiles));
            assert_eq!(km.lookup(&f, Context::Editor), Some(Command::FindInFiles));
            let c = key(KeyCode::Char('c'), toggle);
            assert_eq!(km.lookup(&c, s), Some(Command::Search(S::ToggleCase)));
            assert_eq!(km.lookup(&c, Context::Editor), Some(Command::Find(F::ToggleCase)));
            let r = key(KeyCode::Char('r'), toggle);
            assert_eq!(km.lookup(&r, s), Some(Command::Search(S::ToggleRegex)));
            // No whole-word toggle in v1: the key does nothing in the input.
            assert_eq!(km.lookup(&key(KeyCode::Char('w'), toggle), s), None);
            let esc = key(KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(km.lookup(&esc, s), Some(Command::Search(S::FocusEditor)));
            // Explorer keys stay in the explorer; typing goes to the input.
            assert_eq!(km.lookup(&key(KeyCode::Delete, KeyModifiers::NONE), s), None);
            assert_eq!(km.lookup(&key(KeyCode::Char('x'), KeyModifiers::NONE), s), None);

            // The results list: its own arrows and Enter, the input's toggles and Esc.
            let l = Context::SearchResults;
            let none = KeyModifiers::NONE;
            let down = key(KeyCode::Down, none);
            assert_eq!(km.lookup(&down, s), Some(Command::Search(S::FocusResults)));
            assert_eq!(km.lookup(&down, l), Some(Command::Search(S::Down)));
            assert_eq!(km.lookup(&key(KeyCode::Up, none), l), Some(Command::Search(S::Up)));
            assert_eq!(km.lookup(&key(KeyCode::Up, none), s), None);
            let enter = key(KeyCode::Enter, none);
            assert_eq!(km.lookup(&enter, l), Some(Command::Search(S::Open)));
            assert_eq!(km.lookup(&enter, s), Some(Command::Search(S::Refresh)));
            assert_eq!(km.lookup(&c, l), Some(Command::Search(S::ToggleCase)));
            assert_eq!(km.lookup(&esc, l), Some(Command::Search(S::FocusEditor)));
            assert_eq!(km.lookup(&key(KeyCode::Char('x'), none), l), None);

            // F4 / Shift+F4 from anywhere.
            let f4 = key(KeyCode::F(4), none);
            let shift_f4 = key(KeyCode::F(4), KeyModifiers::SHIFT);
            for ctx in [Context::Editor, Context::Explorer, s, l, Context::Find] {
                assert_eq!(km.lookup(&f4, ctx), Some(Command::Search(S::NextResult)));
                assert_eq!(km.lookup(&shift_f4, ctx), Some(Command::Search(S::PreviousResult)));
            }
        }
    }

    #[test]
    fn problems_context_and_panel_keys() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            let cmd = if platform == Platform::MacOs {
                KeyModifiers::SUPER
            } else {
                KeyModifiers::CONTROL
            };
            let none = KeyModifiers::NONE;
            let p = Context::Problems;
            for ctx in [Context::Editor, Context::Explorer, Context::Search, p, Context::Find] {
                assert_eq!(
                    km.lookup(&key(KeyCode::Char('j'), cmd), ctx),
                    Some(Command::TogglePanel)
                );
                assert_eq!(
                    km.lookup(&key(KeyCode::Char('M'), cmd | KeyModifiers::SHIFT), ctx),
                    Some(Command::ToggleProblems)
                );
                assert_eq!(
                    km.lookup(&key(KeyCode::F(8), none), ctx),
                    Some(Command::Problems(Pr::Next))
                );
                assert_eq!(
                    km.lookup(&key(KeyCode::F(8), KeyModifiers::SHIFT), ctx),
                    Some(Command::Problems(Pr::Previous))
                );
            }
            assert_eq!(
                km.lookup(&key(KeyCode::Down, none), p),
                Some(Command::Problems(Pr::Down))
            );
            assert_eq!(
                km.lookup(&key(KeyCode::Enter, none), p),
                Some(Command::Problems(Pr::Open))
            );
            assert_eq!(
                km.lookup(&key(KeyCode::Esc, none), p),
                Some(Command::Problems(Pr::FocusEditor))
            );
            assert_eq!(
                km.lookup(&key(KeyCode::Char('x'), none), p),
                None,
                "typing does nothing"
            );
            assert_eq!(
                km.binding_for(Command::TogglePanel).map(|k| k.label()),
                Some(if platform == Platform::MacOs { "Cmd+J" } else { "Ctrl+J" }.to_string())
            );
        }
    }

    #[test]
    fn shifted_digits_with_a_modifier_are_the_digit() {
        // Kitty-protocol terminals with alternate keys report Cmd+Shift+1 as Cmd+`!`.
        let bang = KeyCombo::from_event(&key(KeyCode::Char('!'), KeyModifiers::SUPER));
        assert_eq!(bang, KeyCombo::parse("cmd+shift+1").unwrap());
        let paren = KeyCombo::from_event(&key(KeyCode::Char(')'), KeyModifiers::CONTROL));
        assert_eq!(paren, KeyCombo::parse("ctrl+shift+0").unwrap());
        // Typed on its own it stays a character.
        let typed = KeyCombo::from_event(&key(KeyCode::Char('!'), KeyModifiers::NONE));
        assert_eq!(typed.code, KeyCode::Char('!'));
        let typed = KeyCombo::from_event(&key(KeyCode::Char('!'), KeyModifiers::SHIFT));
        assert_eq!((typed.code, typed.mods), (KeyCode::Char('!'), KeyModifiers::SHIFT));
        // Written with the shifted character in keybindings.toml, it still matches.
        assert_eq!(
            KeyCombo::parse("ctrl+!").unwrap(),
            KeyCombo::parse("ctrl+shift+1").unwrap()
        );
        assert_eq!(KeyCombo::parse("alt+shift+$").unwrap().spec(), "alt+shift+4");
        assert_eq!(KeyCombo::parse("!").unwrap().code, KeyCode::Char('!'));
        let mac = Keymap::default_for(Platform::MacOs);
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('!'), KeyModifiers::SUPER), Context::Find),
            Some(Command::Find(F::ReplaceOne))
        );
    }

    /// Ctrl+Space triggers completion however it arrives (kitty: Ctrl+Space; a legacy terminal's
    /// NUL: Ctrl+@, maybe with Shift); the list's keys apply only in its context, the editor's
    /// otherwise.
    #[test]
    fn ctrl_space_and_the_suggest_context() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            let ed = Context::Editor;
            for (code, mods) in [
                (KeyCode::Char(' '), KeyModifiers::CONTROL),
                (KeyCode::Char('@'), KeyModifiers::CONTROL),
                (KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            ] {
                assert_eq!(
                    km.lookup(&key(code, mods), ed),
                    Some(Command::TriggerSuggest),
                    "{code:?} {mods:?}"
                );
            }
            assert_eq!(
                km.binding_for(Command::TriggerSuggest).map(|k| k.label()).as_deref(),
                Some("Ctrl+Space")
            );
            let s = Context::Suggest;
            let none = KeyModifiers::NONE;
            for (code, c) in [
                (KeyCode::Up, Sg::Up),
                (KeyCode::Down, Sg::Down),
                (KeyCode::PageDown, Sg::PageDown),
                (KeyCode::Enter, Sg::Accept),
                (KeyCode::Tab, Sg::Accept),
                (KeyCode::Esc, Sg::Close),
            ] {
                assert_eq!(km.lookup(&key(code, none), s), Some(Command::Suggest(c)));
            }
            assert_eq!(km.lookup(&key(KeyCode::Enter, none), ed), Some(Command::InsertNewline));
            assert_eq!(
                km.lookup(&key(KeyCode::Backspace, none), s),
                Some(Command::DeleteLeft),
                "the editor's"
            );
            assert_eq!(km.lookup(&key(KeyCode::Char('@'), none), s), None, "typed");
        }
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

            [[binding]]
            key = "ctrl+n"
            command = "find.next"
            context = "find"
        "#;
        let (user, errors) = parse_user(toml);
        assert_eq!(user.len(), 3);
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
        // A find-context binding: Ctrl+N goes to the next match; Enter and F3 no longer do.
        let ctrl_n = key(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(km.lookup(&ctrl_n, Context::Find), Some(Command::Find(F::Next)));
        assert_eq!(km.lookup(&ctrl_n, Context::Editor), None);
        assert_eq!(km.lookup(&key(KeyCode::Enter, KeyModifiers::NONE), Context::Find), None);

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
