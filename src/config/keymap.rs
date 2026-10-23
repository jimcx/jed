//! Key combos to commands. Keymaps are data; commands never look at keys.
//! A binding applies in one context (editor, explorer, palette, find, replace, search,
//! searchResults, problems, locations, suggest, codeActions, diff, scm, gitHistory, task) or everywhere.
//! The defaults are VS Code's; `keybindings.toml` replaces bindings command by command.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::command::{
    CodeActionCommand as Ca, ColumnMotion, Command, CommitCommand as Cc, ExplorerCommand as X, FindCommand as F,
    GitHistoryCommand as G, LocationsCommand as L, Motion, PaletteCommand as P, ProblemsCommand as Pr,
    ScmCommand as Sc, SearchCommand as S, ShellCommand as Sh, SuggestCommand as Sg, TaskCommand as T,
    ToolsCommand as Tl,
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
    /// since kitty-protocol terminals report Cmd+Shift+1 as Cmd+`!`; likewise `_` becomes `-` + Shift.
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
            // Shift+- as a kitty-protocol terminal with alternate keys reports it (Go Forward).
            KeyCode::Char('_') if chord => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char('-')
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
    /// The editor has the focus and the code action list (Cmd+.) is shown; bindings not found
    /// here come from `Editor` (anything else they do closes the list).
    CodeActions,
    /// The editor has the focus and its active tab is a diff view; bindings not found here come
    /// from `Editor`.
    Diff,
    /// The Source Control view's list has the focus.
    Scm,
    /// The Tools view's list has the focus.
    Tools,
    /// The Explorer view's "Git History" section has the keys; unbound keys do nothing.
    GitHistory,
    /// The editor has the focus and its active tab is a commit; bindings not found here come
    /// from `Editor` (movement, copy, select all).
    Commit,
    /// A task tab in the bottom panel has the focus: keys bound to nothing (here or globally)
    /// go to the task's terminal.
    Task,
    /// The editor has the focus and its active tab is a hex view; bindings not found here come
    /// from `Editor` (movement, selection, copy, Tab).
    Hex,
    /// The editor has the focus and its active tab is a shell: keys bound to nothing (here or
    /// globally) go to the shell's terminal.
    Shell,
}

impl Context {
    const ALL: [(Context, &'static str); 19] = [
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
        (Context::CodeActions, "codeActions"),
        (Context::Diff, "diff"),
        (Context::Scm, "scm"),
        (Context::Tools, "tools"),
        (Context::GitHistory, "gitHistory"),
        (Context::Commit, "commit"),
        (Context::Task, "task"),
        (Context::Hex, "hex"),
        (Context::Shell, "shell"),
    ];

    fn parse(name: &str) -> Option<Context> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(c, _)| *c)
    }

    /// The context looked at next, before the global bindings.
    pub fn parent(self) -> Option<Context> {
        match self {
            Context::Replace => Some(Context::Find),
            Context::SearchResults => Some(Context::Search),
            Context::Suggest | Context::CodeActions | Context::Diff | Context::Commit | Context::Hex => {
                Some(Context::Editor)
            }
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

    /// The commands bound in `context` itself (not its parents or globally), each with its
    /// palette key when that's one of them, else its first by label; sorted by command title.
    pub fn context_bindings(&self, context: Context) -> Vec<(KeyCombo, Command)> {
        let mut by_cmd: HashMap<Command, KeyCombo> = HashMap::new();
        for ((combo, ctx), cmd) in &self.bindings {
            if *ctx != Some(context) {
                continue;
            }
            let shown = self.shown.get(cmd) == Some(combo);
            by_cmd
                .entry(*cmd)
                .and_modify(|c| {
                    if shown || (self.shown.get(cmd) != Some(c) && combo.label() < c.label()) {
                        *c = *combo;
                    }
                })
                .or_insert(*combo);
        }
        let mut rows: Vec<(KeyCombo, Command)> = by_cmd.into_iter().map(|(cmd, combo)| (combo, cmd)).collect();
        rows.sort_by_cached_key(|(combo, cmd)| (cmd.title(), combo.label()));
        rows
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
    // VS Code's: Shift+Alt+F on macOS, Ctrl+Shift+I on Linux.
    let format_document = if mac {
        (KeyCode::Char('f'), alt | shift)
    } else {
        (KeyCode::Char('i'), ctrl | shift)
    };
    editor.extend([
        (KeyCode::Backspace, none, Command::DeleteLeft),
        (KeyCode::Backspace, shift, Command::DeleteLeft),
        (KeyCode::Delete, none, Command::DeleteRight),
        (KeyCode::Backspace, word, Command::DeleteWordLeft),
        (KeyCode::Delete, word, Command::DeleteWordRight),
        (KeyCode::Enter, none, Command::InsertNewline),
        (KeyCode::Tab, none, Command::InsertTab),
        (KeyCode::Tab, shift, Command::OutdentLines),
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
        (KeyCode::Char(']'), cmd, Command::IndentLines),
        (KeyCode::Char('['), cmd, Command::OutdentLines),
        // No chords, so not VS Code's Cmd+K Cmd+I.
        (KeyCode::Char('i'), cmd | alt, Command::ShowHover),
        (KeyCode::F(12), none, Command::RevealDefinition),
        (KeyCode::F(12), shift, Command::GoToReferences),
        // The explorer's F2 renames a file (its own context).
        (KeyCode::F(2), none, Command::RenameSymbol),
        (format_document.0, format_document.1, Command::FormatDocument),
        // Ctrl on macOS too, as in VS Code.
        (KeyCode::Char(' '), ctrl, Command::TriggerSuggest),
        (KeyCode::Char('.'), cmd, Command::QuickFix),
        // VS Code's `workbench.action.editor.nextChange` / `previousChange`.
        (KeyCode::F(5), alt, Command::NextChange),
        (KeyCode::F(5), alt | shift, Command::PreviousChange),
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
        (KeyCode::Char('x'), toggle, Command::Find(F::ToggleHex)),
        (KeyCode::Char('1'), cmd | shift, Command::Find(F::ReplaceOne)),
        (KeyCode::Enter, cmd | alt, Command::Find(F::ReplaceAll)),
        (KeyCode::Enter, alt, Command::Find(F::SelectAllMatches)),
    ];
    editor.extend(find_keys);
    if mac {
        editor.push((KeyCode::Backspace, cmd, Command::DeleteToLineStart));
        // macOS's Emacs-style keys, as VS Code binds them on macOS (after Home / End / arrows /
        // Backspace / Delete, so the palette shows those).
        // Shift selects, as VS Code's Ctrl+Shift+A / E / B / F / P / N.
        for (c, motion) in [
            ('a', Motion::LineStart),
            ('e', Motion::LineEnd),
            ('b', Motion::Left),
            ('f', Motion::Right),
            ('p', Motion::Up),
            ('n', Motion::Down),
        ] {
            editor.push((KeyCode::Char(c), ctrl, Command::Move { motion, select: false }));
            editor.push((KeyCode::Char(c), ctrl | shift, Command::Move { motion, select: true }));
        }
        editor.extend([
            (KeyCode::Char('h'), ctrl, Command::DeleteLeft),
            (KeyCode::Char('d'), ctrl, Command::DeleteRight),
        ]);
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
        (KeyCode::Char('n'), cmd, Command::NewUntitledFile),
        (KeyCode::Char('s'), cmd, Command::Save),
        (KeyCode::Char('s'), cmd | shift, Command::SaveAs),
        (KeyCode::Char('s'), cmd | alt, Command::SaveAll),
        (KeyCode::Char('q'), cmd, Command::Quit),
        // Works in any terminal, even without the kitty protocol.
        (KeyCode::Char('q'), ctrl, Command::Quit),
        (KeyCode::Char('w'), cmd, Command::CloseTab),
        (KeyCode::Char('b'), cmd, Command::ToggleSidebar),
        (KeyCode::Char('e'), cmd | shift, Command::FocusExplorer),
        // Ctrl on macOS too, as in VS Code.
        (KeyCode::Char('g'), ctrl | shift, Command::FocusScm),
        (KeyCode::Char('f'), cmd | shift, Command::FindInFiles),
        (KeyCode::Char('h'), cmd | shift, Command::Search(S::ReplaceInFiles)),
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
    // VS Code's: Ctrl+Cmd+Right / Left on macOS, Ctrl+Alt+Right / Left on Linux.
    let group_move = if mac { cmd | ctrl } else { ctrl | alt };
    global.extend([
        (KeyCode::Right, group_move, Command::MoveTabToNextGroup),
        (KeyCode::Left, group_move, Command::MoveTabToPreviousGroup),
        (KeyCode::PageUp, cmd | shift, Command::MoveTabLeft),
        (KeyCode::PageDown, cmd | shift, Command::MoveTabRight),
    ]);
    global.extend([
        (KeyCode::Char('p'), cmd | shift, Command::ShowCommands),
        (KeyCode::F(1), none, Command::ShowCommands),
        // F1 is the palette, so Shift+F1 on both platforms (no Cmd: docs/terminals/ unchanged).
        (KeyCode::F(1), shift, Command::CheatSheet),
        (KeyCode::Char('p'), cmd, Command::QuickOpen),
        // Ctrl on macOS too, as in VS Code.
        (KeyCode::Char('g'), ctrl, Command::GoToLine),
        // VS Code's Open Recent: Ctrl on macOS too. Global, so a task tab doesn't get it either.
        (KeyCode::Char('r'), ctrl, Command::OpenRecentWorkspace),
        (KeyCode::F(4), none, Command::Search(S::NextResult)),
        (KeyCode::F(4), shift, Command::Search(S::PreviousResult)),
        (KeyCode::Char('j'), cmd, Command::TogglePanel),
        (KeyCode::Char('m'), cmd | shift, Command::ToggleProblems),
        // VS Code's Output (on Linux VS Code has Ctrl+K Ctrl+H; no chords here).
        (KeyCode::Char('u'), cmd | shift, Command::ShowTools),
        (KeyCode::F(8), none, Command::Problems(Pr::Next)),
        (KeyCode::F(8), shift, Command::Problems(Pr::Previous)),
        // VS Code's Go Back / Go Forward: Ctrl on macOS too; Ctrl+Alt+- on Linux.
        (
            KeyCode::Char('-'),
            if mac { ctrl } else { ctrl | alt },
            Command::NavBack,
        ),
        (KeyCode::Char('-'), ctrl | shift, Command::NavForward),
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

    // The Search view's input: the find widget's toggle keys.
    let search = [
        (KeyCode::Char('c'), toggle, S::ToggleCase),
        (KeyCode::Char('w'), toggle, S::ToggleWholeWord),
        (KeyCode::Char('r'), toggle, S::ToggleRegex),
        (KeyCode::Enter, none, S::Refresh),
        (KeyCode::Esc, none, S::FocusEditor),
        (KeyCode::Down, none, S::FocusResults),
        (KeyCode::Char('j'), cmd | shift, S::ToggleDetails),
        (KeyCode::Tab, none, S::NextInput),
        (KeyCode::Tab, shift, S::PreviousInput),
        // VS Code's `search.action.replaceAll` (the list falls back to these keys).
        (KeyCode::Enter, cmd | alt, S::ReplaceAll),
    ];
    // Its results list (falls back to the input's keys: toggles, Esc).
    let mut search_results = vec![
        (KeyCode::Up, none, S::Up),
        (KeyCode::Down, none, S::Down),
        (KeyCode::PageUp, none, S::PageUp),
        (KeyCode::PageDown, none, S::PageDown),
        (KeyCode::Home, none, S::First),
        (KeyCode::End, none, S::Last),
        (KeyCode::Enter, none, S::Open),
        (KeyCode::Left, none, S::Collapse),
        (KeyCode::Right, none, S::Expand),
        (KeyCode::Delete, none, S::Dismiss),
        // VS Code's `search.action.replace` / `replaceAllInFile`: Cmd+`!` as terminals send it.
        (KeyCode::Char('1'), cmd | shift, S::ReplaceOne),
    ];
    if mac {
        // VS Code's `search.action.remove` on macOS.
        search_results.push((KeyCode::Backspace, cmd, S::Dismiss));
    }

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
    let mut suggest = vec![
        (KeyCode::Up, none, Sg::Up),
        (KeyCode::Down, none, Sg::Down),
        (KeyCode::PageUp, none, Sg::PageUp),
        (KeyCode::PageDown, none, Sg::PageDown),
        (KeyCode::Enter, none, Sg::Accept),
        (KeyCode::Tab, none, Sg::Accept),
        (KeyCode::Esc, none, Sg::Close),
    ];
    let code_actions = vec![
        (KeyCode::Up, none, Ca::Up),
        (KeyCode::Down, none, Ca::Down),
        (KeyCode::Enter, none, Ca::Accept),
        (KeyCode::Esc, none, Ca::Close),
    ];
    if mac {
        // VS Code's: the list's selection, not the editor's Ctrl+P / Ctrl+N cursor moves.
        suggest.push((KeyCode::Char('p'), ctrl, Sg::Up));
        suggest.push((KeyCode::Char('n'), ctrl, Sg::Down));
    }

    // A diff tab (falls back to the editor's keys): VS Code's diff editor's next / previous
    // difference.
    let diff = [
        (KeyCode::F(7), none, Command::NextChange),
        (KeyCode::F(7), shift, Command::PreviousChange),
    ];

    // The Source Control view's list: the explorer's keys (Space too).
    let scm = [
        (KeyCode::Up, none, Sc::Up),
        (KeyCode::Down, none, Sc::Down),
        (KeyCode::PageUp, none, Sc::PageUp),
        (KeyCode::PageDown, none, Sc::PageDown),
        (KeyCode::Home, none, Sc::First),
        (KeyCode::End, none, Sc::Last),
        (KeyCode::Left, none, Sc::Collapse),
        (KeyCode::Right, none, Sc::Expand),
        (KeyCode::Enter, none, Sc::Open),
        (KeyCode::Char(' '), none, Sc::Preview),
        (KeyCode::Esc, none, Sc::FocusEditor),
    ];

    let tools = [
        (KeyCode::Up, none, Tl::Up),
        (KeyCode::Down, none, Tl::Down),
        (KeyCode::PageUp, none, Tl::PageUp),
        (KeyCode::PageDown, none, Tl::PageDown),
        (KeyCode::Home, none, Tl::First),
        (KeyCode::End, none, Tl::Last),
        (KeyCode::Enter, none, Tl::Open),
        (KeyCode::Esc, none, Tl::FocusEditor),
    ];

    // The Explorer's "Git History" section: the explorer's movement keys, plus Left / Esc back
    // to the tree (Enter opens a commit later).
    let git_history = [
        (KeyCode::Up, none, G::Up),
        (KeyCode::Down, none, G::Down),
        (KeyCode::PageUp, none, G::PageUp),
        (KeyCode::PageDown, none, G::PageDown),
        (KeyCode::Home, none, G::First),
        (KeyCode::End, none, G::Last),
        (KeyCode::Enter, none, G::Open),
        (KeyCode::Left, none, G::FocusTree),
        (KeyCode::Esc, none, G::FocusTree),
    ];

    // A commit tab: Enter opens the cursor's stat row's diff (falls back to the editor's
    // movement, Home / End, Cmd+C and Cmd+A).
    let commit = [(KeyCode::Enter, none, Command::Commit(Cc::Open))];

    // A task tab: scrolling through its output (VS Code's terminal keys, Ctrl rather than Cmd on
    // macOS), back to the editor, close. Every other key not bound globally goes to the task.
    let task = [
        (KeyCode::PageUp, shift, Command::Task(T::PageUp)),
        (KeyCode::PageDown, shift, Command::Task(T::PageDown)),
        (KeyCode::Up, ctrl | shift, Command::Task(T::ScrollUp)),
        (KeyCode::Down, ctrl | shift, Command::Task(T::ScrollDown)),
        (KeyCode::Home, ctrl, Command::Task(T::Top)),
        (KeyCode::End, ctrl, Command::Task(T::Bottom)),
        (KeyCode::Esc, none, Command::Task(T::FocusEditor)),
        (KeyCode::Char('w'), cmd, Command::CloseTask),
    ];

    // A shell tab: scrolling through its scrollback (VS Code's terminal keys, Ctrl rather than
    // Cmd on macOS). Cmd+W and the global keys still work; every other key goes to the shell.
    let shell = [
        (KeyCode::PageUp, shift, Command::Shell(Sh::PageUp)),
        (KeyCode::PageDown, shift, Command::Shell(Sh::PageDown)),
        (KeyCode::Up, ctrl | shift, Command::Shell(Sh::ScrollUp)),
        (KeyCode::Down, ctrl | shift, Command::Shell(Sh::ScrollDown)),
        (KeyCode::Home, ctrl, Command::Shell(Sh::Top)),
        (KeyCode::End, ctrl, Command::Shell(Sh::Bottom)),
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
    out.extend(
        code_actions
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::CodeActions), Command::CodeActions(c))),
    );
    out.extend(diff.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Diff), c)));
    out.extend(
        scm.into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Scm), Command::Scm(c))),
    );
    out.extend(
        tools
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::Tools), Command::Tools(c))),
    );
    out.extend(
        git_history
            .into_iter()
            .map(|(k, m, c)| bind(k, m, Some(Context::GitHistory), Command::GitHistory(c))),
    );
    out.extend(commit.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Commit), c)));
    out.extend(task.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Task), c)));
    out.extend(shell.into_iter().map(|(k, m, c)| bind(k, m, Some(Context::Shell), c)));
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
    fn source_control_is_ctrl_shift_g_on_both_platforms() {
        let g = key(KeyCode::Char('g'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        for p in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(p);
            for ctx in [Context::Editor, Context::Explorer, Context::Scm, Context::Search] {
                assert_eq!(km.lookup(&g, ctx), Some(Command::FocusScm), "{p:?} {ctx:?}");
            }
            let enter = key(KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(km.lookup(&enter, Context::Scm), Some(Command::Scm(Sc::Open)));
            let x = key(KeyCode::Char('x'), KeyModifiers::NONE);
            assert_eq!(km.lookup(&x, Context::Scm), None, "plain keys don't type");
        }
        assert_eq!(Context::parse("scm"), Some(Context::Scm));
    }

    #[test]
    fn open_recent_workspace_is_ctrl_r_on_both_platforms() {
        let r = key(KeyCode::Char('r'), KeyModifiers::CONTROL);
        for p in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(p);
            for ctx in [
                Context::Editor,
                Context::Explorer,
                Context::Palette,
                Context::Find,
                Context::Task,
            ] {
                assert_eq!(km.lookup(&r, ctx), Some(Command::OpenRecentWorkspace), "{p:?} {ctx:?}");
            }
            let shown = km.binding_for(Command::OpenRecentWorkspace).map(|k| k.label());
            assert_eq!(shown.as_deref(), Some("Ctrl+R"), "{p:?}");
        }
        let mac = Keymap::default_for(Platform::MacOs);
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('r'), KeyModifiers::SUPER), Context::Editor),
            None
        );
    }

    #[test]
    fn task_tabs_keep_global_keys_and_leave_the_rest_to_the_task() {
        for p in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(p);
            let cmd = if p == Platform::MacOs {
                KeyModifiers::SUPER
            } else {
                KeyModifiers::CONTROL
            };
            let t = Context::Task;
            assert_eq!(km.lookup(&key(KeyCode::Char('w'), cmd), t), Some(Command::CloseTask));
            assert_eq!(
                km.lookup(&key(KeyCode::Char('p'), cmd | KeyModifiers::SHIFT), t),
                Some(Command::ShowCommands)
            );
            assert_eq!(
                km.lookup(&key(KeyCode::PageUp, KeyModifiers::SHIFT), t),
                Some(Command::Task(T::PageUp))
            );
            for unbound in [
                key(KeyCode::Char('y'), KeyModifiers::NONE),
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                key(KeyCode::Char('d'), KeyModifiers::CONTROL),
                key(KeyCode::Enter, KeyModifiers::NONE),
                key(KeyCode::Up, KeyModifiers::NONE),
                key(KeyCode::Tab, KeyModifiers::NONE),
            ] {
                assert_eq!(km.lookup(&unbound, t), None, "{p:?} {unbound:?}");
            }
        }
        assert_eq!(Context::parse("task"), Some(Context::Task));
    }

    #[test]
    fn shell_tabs_keep_global_keys_and_leave_the_rest_to_the_shell() {
        for p in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(p);
            let cmd = if p == Platform::MacOs {
                KeyModifiers::SUPER
            } else {
                KeyModifiers::CONTROL
            };
            let sh = Context::Shell;
            assert_eq!(km.lookup(&key(KeyCode::Char('w'), cmd), sh), Some(Command::CloseTab));
            assert_eq!(
                km.lookup(&key(KeyCode::Char('p'), cmd | KeyModifiers::SHIFT), sh),
                Some(Command::ShowCommands)
            );
            assert_eq!(
                km.lookup(&key(KeyCode::Char('q'), KeyModifiers::CONTROL), sh),
                Some(Command::Quit),
                "Ctrl+Q quits from a shell"
            );
            assert_eq!(
                km.lookup(&key(KeyCode::PageUp, KeyModifiers::SHIFT), sh),
                Some(Command::Shell(Sh::PageUp))
            );
            assert_eq!(
                km.lookup(&key(KeyCode::End, KeyModifiers::CONTROL), sh),
                Some(Command::Shell(Sh::Bottom))
            );
            for unbound in [
                key(KeyCode::Char('y'), KeyModifiers::NONE),
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                key(KeyCode::Char('d'), KeyModifiers::CONTROL),
                key(KeyCode::Enter, KeyModifiers::NONE),
                key(KeyCode::Up, KeyModifiers::NONE),
                key(KeyCode::Esc, KeyModifiers::NONE),
                key(KeyCode::Tab, KeyModifiers::NONE),
            ] {
                assert_eq!(km.lookup(&unbound, sh), None, "{p:?} {unbound:?}");
            }
        }
        assert_eq!(Context::parse("shell"), Some(Context::Shell));
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
    fn format_document_keys() {
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let ed = Context::Editor;
        let fmt = Some(Command::FormatDocument);
        let shift_alt = KeyModifiers::SHIFT | KeyModifiers::ALT;
        assert_eq!(mac.lookup(&key(KeyCode::Char('F'), shift_alt), ed), fmt);
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(linux.lookup(&key(KeyCode::Char('I'), ctrl_shift), ed), fmt);
        // Shift+Alt+I stays "cursors at line ends" on both.
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('I'), shift_alt), ed),
            Some(Command::CursorsAtLineEnds)
        );
        assert_eq!(
            mac.binding_for(Command::FormatDocument).map(|k| k.label()).as_deref(),
            Some("Alt+Shift+F")
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
            let w = key(KeyCode::Char('w'), toggle);
            assert_eq!(km.lookup(&w, s), Some(Command::Search(S::ToggleWholeWord)));
            assert_eq!(km.lookup(&w, Context::Editor), Some(Command::Find(F::ToggleWholeWord)));
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
            assert_eq!(km.lookup(&w, l), Some(Command::Search(S::ToggleWholeWord)));
            let list = [
                (KeyCode::Left, S::Collapse),
                (KeyCode::Right, S::Expand),
                (KeyCode::Delete, S::Dismiss),
            ];
            for (code, c) in list {
                assert_eq!(km.lookup(&key(code, none), l), Some(Command::Search(c)));
                assert_eq!(km.lookup(&key(code, none), s), None, "the query's own");
            }

            // VS Code's Cmd+Backspace dismisses on macOS (Linux has no such key).
            let cmd_bs = key(KeyCode::Backspace, cmd);
            let dismiss = (platform == Platform::MacOs).then_some(Command::Search(S::Dismiss));
            assert_eq!(km.lookup(&cmd_bs, l), dismiss);
            // Replace in Files from anywhere.
            let h = key(KeyCode::Char('h'), cmd | KeyModifiers::SHIFT);
            for ctx in [Context::Editor, Context::Explorer, s, l] {
                assert_eq!(km.lookup(&h, ctx), Some(Command::Search(S::ReplaceInFiles)));
            }

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
    fn go_back_and_forward_keys_as_terminals_report_them() {
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let ctrl = KeyModifiers::CONTROL;
        let shift = KeyModifiers::SHIFT;
        let alt = KeyModifiers::ALT;
        for ctx in [
            Context::Editor,
            Context::Explorer,
            Context::Diff,
            Context::Problems,
            Context::Task,
        ] {
            assert_eq!(mac.lookup(&key(KeyCode::Char('-'), ctrl), ctx), Some(Command::NavBack));
            assert_eq!(
                linux.lookup(&key(KeyCode::Char('-'), ctrl | alt), ctx),
                Some(Command::NavBack)
            );
            for km in [&mac, &linux] {
                // Plain disambiguated Ctrl+Shift+-, and with alternate keys: Ctrl+`_` (+ Shift).
                for ev in [
                    key(KeyCode::Char('-'), ctrl | shift),
                    key(KeyCode::Char('_'), ctrl | shift),
                    key(KeyCode::Char('_'), ctrl),
                ] {
                    assert_eq!(km.lookup(&ev, ctx), Some(Command::NavForward), "{ev:?} {ctx:?}");
                }
            }
        }
        assert_eq!(linux.lookup(&key(KeyCode::Char('-'), ctrl), Context::Editor), None);
        let label = |km: &Keymap, c| km.binding_for(c).map(|k| k.label());
        assert_eq!(label(&mac, Command::NavBack).as_deref(), Some("Ctrl+-"));
        assert_eq!(label(&mac, Command::NavForward).as_deref(), Some("Ctrl+Shift+-"));
        assert_eq!(label(&linux, Command::NavBack).as_deref(), Some("Ctrl+Alt+-"));
        assert_eq!(label(&linux, Command::NavForward).as_deref(), Some("Ctrl+Shift+-"));
        // Written either way in keybindings.toml.
        assert_eq!(
            KeyCombo::parse("ctrl+_").unwrap(),
            KeyCombo::parse("ctrl+shift+-").unwrap()
        );
        // Typed without a chord, `_` stays a character.
        let typed = KeyCombo::from_event(&key(KeyCode::Char('_'), shift));
        assert_eq!((typed.code, typed.mods), (KeyCode::Char('_'), shift));
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
    fn macos_emacs_keys_in_the_editor() {
        let ctrl = KeyModifiers::CONTROL;
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let ed = Context::Editor;
        let mv = |motion| Command::Move { motion, select: false };
        for (c, cmd) in [
            ('a', mv(Motion::LineStart)),
            ('e', mv(Motion::LineEnd)),
            ('b', mv(Motion::Left)),
            ('f', mv(Motion::Right)),
            ('p', mv(Motion::Up)),
            ('n', mv(Motion::Down)),
            ('h', Command::DeleteLeft),
            ('d', Command::DeleteRight),
        ] {
            assert_eq!(mac.lookup(&key(KeyCode::Char(c), ctrl), ed), Some(cmd), "Ctrl+{c}");
            if let Command::Move { motion, .. } = cmd {
                let select = Command::Move { motion, select: true };
                let shifted = key(KeyCode::Char(c), ctrl | KeyModifiers::SHIFT);
                assert_eq!(mac.lookup(&shifted, ed), Some(select), "Ctrl+Shift+{c}");
            }
        }
        // The palette still shows the usual keys.
        assert_eq!(
            mac.binding_for(Command::DeleteRight).map(|k| k.label()).as_deref(),
            Some("Delete")
        );
        assert_eq!(
            mac.binding_for(mv(Motion::LineStart)).map(|k| k.label()).as_deref(),
            Some("Home")
        );
        let s = Context::Suggest;
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('p'), ctrl), s),
            Some(Command::Suggest(Sg::Up))
        );
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('n'), ctrl), s),
            Some(Command::Suggest(Sg::Down))
        );
        // Linux keeps VS Code's Linux keys.
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('d'), ctrl), ed),
            Some(Command::AddNextMatch)
        );
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('h'), ctrl), ed),
            Some(Command::OpenReplace)
        );
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('a'), ctrl), ed),
            Some(Command::SelectAll)
        );
    }

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
        let down = Command::Move {
            motion: Motion::Down,
            select: false,
        };
        assert_eq!(
            km.lookup(&ctrl_n, Context::Editor),
            Some(down),
            "the editor's own Ctrl+N"
        );
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
