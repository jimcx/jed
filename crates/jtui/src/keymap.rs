//! Key combos to commands: `KeyCombo` (from a key event, a config spelling, a label), `Platform`, and
//! `Keymap<Ctx, Cmd>`, generic over the app's contexts and commands. A binding applies in one context
//! or everywhere; a context may fall back to a parent. The app supplies its defaults as data;
//! a user's keybindings file (`[[binding]]` tables) replaces bindings command by command.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

/// The app's key contexts (which part has the focus).
pub trait KeyContext: Copy + Eq + Hash + Debug {
    /// The context whose bindings apply when this one has none for a key.
    fn parent(self) -> Option<Self>;
    /// A context from its config spelling.
    fn parse(name: &str) -> Option<Self>;
    /// Every config spelling, for the "unknown context" message.
    fn names() -> Vec<&'static str>;
    /// Whether `combo` skips the global (context-free) bindings in this context.
    fn passes_through(self, _combo: &KeyCombo) -> bool {
        false
    }
}

/// The app's commands, as a keymap needs them.
pub trait KeyCommand: Copy + Eq + Hash + Debug {
    /// A command from its config name.
    fn from_name(name: &str) -> Option<Self>;
    /// The config name (logged for user bindings).
    fn name(&self) -> String;
    /// The display title (sorts `context_bindings`).
    fn title(&self) -> String;
}

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
pub const NAMED_KEYS: [(KeyCode, &str, &str); 15] = [
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
    /// since kitty-protocol terminals report Cmd+Shift+1 as Cmd+`!`; likewise `_` becomes `-` + Shift
    /// and `<` / `>` become `,` / `.` + Shift.
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
            // Shift+, / Shift+. as alternate keys report them (Focus Previous / Next Editor Group).
            KeyCode::Char('<') if chord => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char(',')
            }
            KeyCode::Char('>') if chord => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char('.')
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

/// Where a binding came from: the built-in defaults or the user's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    User,
}

/// One key combo bound to a command, in a context (`None` = everywhere), and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding<Ctx, Cmd> {
    pub combo: KeyCombo,
    pub context: Option<Ctx>,
    pub command: Cmd,
    pub source: Source,
}

pub struct Keymap<Ctx, Cmd> {
    /// `None` context = everywhere; each binding's command and where it came from.
    bindings: HashMap<(KeyCombo, Option<Ctx>), (Cmd, Source)>,
    /// The binding the palette shows for each command (its context too): the first one that
    /// still applies.
    shown: HashMap<Cmd, (KeyCombo, Option<Ctx>)>,
}

/// A user's keybindings file: a list of `[[binding]]` tables.
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

impl<Ctx: KeyContext, Cmd: KeyCommand> Keymap<Ctx, Cmd> {
    /// Every binding: combo, context (`None` = everywhere), command and where it came from.
    pub fn bindings_with_source(&self) -> impl Iterator<Item = (KeyCombo, Option<Ctx>, Cmd, Source)> + '_ {
        self.bindings
            .iter()
            .map(|((combo, ctx), (command, source))| (*combo, *ctx, *command, *source))
    }

    /// A binding for the current context wins over its parent's (replace → find), which wins
    /// over a global one.
    pub fn lookup(&self, ev: &KeyEvent, context: Ctx) -> Option<Cmd> {
        let combo = KeyCombo::from_event(ev);
        let mut ctx = Some(context);
        while let Some(c) = ctx {
            if let Some(cmd) = self.bindings.get(&(combo, Some(c))) {
                return Some(cmd.0);
            }
            ctx = c.parent();
        }
        if context.passes_through(&combo) {
            return None;
        }
        self.bindings.get(&(combo, None)).map(|(cmd, _)| *cmd)
    }

    /// The key shown next to a command in the palette.
    pub fn binding_for(&self, cmd: Cmd) -> Option<KeyCombo> {
        self.shown.get(&cmd).map(|(combo, _)| *combo)
    }

    /// The commands bound in `context` itself (not its parents or globally), each with its
    /// palette key when that's one of them, else its first by label; sorted by command title.
    pub fn context_bindings(&self, context: Ctx) -> Vec<(KeyCombo, Cmd)> {
        let mut by_cmd: HashMap<Cmd, KeyCombo> = HashMap::new();
        for ((combo, ctx), (cmd, _)) in &self.bindings {
            if *ctx != Some(context) {
                continue;
            }
            let shown = self.binding_for(*cmd) == Some(*combo);
            by_cmd
                .entry(*cmd)
                .and_modify(|c| {
                    if shown || (self.binding_for(*cmd) != Some(*c) && combo.label() < c.label()) {
                        *c = *combo;
                    }
                })
                .or_insert(*combo);
        }
        let mut rows: Vec<(KeyCombo, Cmd)> = by_cmd.into_iter().map(|(cmd, combo)| (combo, cmd)).collect();
        rows.sort_by_cached_key(|(combo, cmd)| (cmd.title(), combo.label()));
        rows
    }

    /// Later bindings win on the same key and context; the palette shows a command's first.
    pub fn from_list(list: Vec<Binding<Ctx, Cmd>>) -> Self {
        let mut bindings = HashMap::new();
        for b in &list {
            bindings.insert((b.combo, b.context), (b.command, b.source));
        }
        let mut shown = HashMap::new();
        for b in &list {
            if bindings.get(&(b.combo, b.context)).map(|(command, _)| *command) == Some(b.command) {
                shown.entry(b.command).or_insert((b.combo, b.context));
            }
        }
        Self { bindings, shown }
    }

    /// The defaults, with each command named in `user` bound only to the user's keys.
    /// A user key also takes over whatever the default had on it, in that context.
    pub fn with_user(defaults: Vec<Binding<Ctx, Cmd>>, user: &[Binding<Ctx, Cmd>]) -> Self {
        let rebound: HashSet<Cmd> = user.iter().map(|b| b.command).collect();
        let mut list: Vec<Binding<Ctx, Cmd>> = defaults.into_iter().filter(|b| !rebound.contains(&b.command)).collect();
        list.extend_from_slice(user);
        Self::from_list(list)
    }

    /// Blocking. `defaults` with the user's file at `path` (called `file_name` in messages) on
    /// top, plus one message per problem. Bad entries are skipped; an unreadable file gives the
    /// defaults.
    pub fn load(path: &Path, file_name: &str, defaults: Vec<Binding<Ctx, Cmd>>) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_text(file_name, defaults.clone(), &text) {
                Ok(r) => r,
                Err(e) => (Self::from_list(defaults), vec![e]),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::from_list(defaults), Vec::new()),
            Err(e) => (Self::from_list(defaults), vec![format!("{file_name}: {e}")]),
        }
    }

    /// Blocking, for a live reload. Like `load`, but a file that can't be read or parsed at all is
    /// an `Err`, so the caller keeps the last good keymap; problems in single bindings still apply
    /// the rest, reported.
    pub fn reload(
        path: &Path,
        file_name: &str,
        defaults: Vec<Binding<Ctx, Cmd>>,
    ) -> Result<(Self, Vec<String>), String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_text(file_name, defaults, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::from_list(defaults), Vec::new())),
            Err(e) => Err(format!("{file_name}: {e}")),
        }
    }

    /// The keymap `text` builds on `defaults`, or its syntax error.
    pub fn from_text(
        file_name: &str,
        defaults: Vec<Binding<Ctx, Cmd>>,
        text: &str,
    ) -> Result<(Self, Vec<String>), String> {
        let (user, errors) = user_bindings::<Ctx, Cmd>(file_name, user_file(file_name, text)?);
        for b in &user {
            tracing::info!(key = %b.combo.spec(), command = %b.command.name(), context = ?b.context, "user keybinding");
        }
        Ok((Self::with_user(defaults, &user), errors))
    }
}

/// The bindings in a user's keybindings file, and a message for each one that couldn't be used.
pub fn parse_user<Ctx: KeyContext, Cmd: KeyCommand>(
    file_name: &str,
    text: &str,
) -> (Vec<Binding<Ctx, Cmd>>, Vec<String>) {
    match user_file(file_name, text) {
        Ok(file) => user_bindings(file_name, file),
        Err(e) => (Vec::new(), vec![e]),
    }
}

/// The file's TOML, or the message for a syntax error (the whole file is then unusable).
fn user_file(file_name: &str, text: &str) -> Result<UserFile, String> {
    toml::from_str(text).map_err(|e| format!("{file_name}: {}", e.message()))
}

fn user_bindings<Ctx: KeyContext, Cmd: KeyCommand>(
    file_name: &str,
    file: UserFile,
) -> (Vec<Binding<Ctx, Cmd>>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for b in file.binding {
        let combo = KeyCombo::parse(&b.key).map_err(|e| format!("key \"{}\": {e}", b.key));
        let command = Cmd::from_name(&b.command).ok_or_else(|| format!("unknown command \"{}\"", b.command));
        let context = match b.context.as_deref() {
            None => Ok(None),
            Some(name) => Ctx::parse(name)
                .map(Some)
                .ok_or_else(|| format!("unknown context \"{name}\" (one of {})", Ctx::names().join(", "))),
        };
        match (combo, command, context) {
            (Ok(combo), Ok(command), Ok(context)) => out.push(Binding {
                combo,
                context,
                command,
                source: Source::User,
            }),
            (combo, command, context) => {
                let e = [combo.err(), command.err(), context.err()].into_iter().flatten();
                errors.extend(e.map(|e| format!("{file_name}: {e}")));
            }
        }
    }
    (out, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Ctx {
        Main,
        Find,
        Replace,
        Term,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Cmd {
        Save,
        Undo,
        Next,
        Quit,
    }

    impl KeyContext for Ctx {
        fn parent(self) -> Option<Self> {
            (self == Ctx::Replace).then_some(Ctx::Find)
        }
        fn parse(name: &str) -> Option<Self> {
            [Ctx::Main, Ctx::Find, Ctx::Replace, Ctx::Term]
                .into_iter()
                .find(|c| format!("{c:?}").to_lowercase() == name)
        }
        fn names() -> Vec<&'static str> {
            vec!["main", "find", "replace", "term"]
        }
        fn passes_through(self, combo: &KeyCombo) -> bool {
            self == Ctx::Term && combo.mods == KeyModifiers::CONTROL && combo.code != KeyCode::Char('q')
        }
    }

    impl KeyCommand for Cmd {
        fn from_name(name: &str) -> Option<Self> {
            [Cmd::Save, Cmd::Undo, Cmd::Next, Cmd::Quit]
                .into_iter()
                .find(|c| c.name() == name)
        }
        fn name(&self) -> String {
            format!("{self:?}").to_lowercase()
        }
        fn title(&self) -> String {
            format!("{self:?}")
        }
    }

    type B = Binding<Ctx, Cmd>;

    fn b(spec: &str, context: Option<Ctx>, command: Cmd) -> B {
        Binding {
            combo: KeyCombo::parse(spec).unwrap(),
            context,
            command,
            source: Source::Default,
        }
    }

    fn key(spec: &str) -> KeyEvent {
        let c = KeyCombo::parse(spec).unwrap();
        KeyEvent::new(c.code, c.mods)
    }

    fn defaults() -> Vec<B> {
        vec![
            b("ctrl+s", None, Cmd::Save),
            b("ctrl+z", None, Cmd::Undo),
            b("f3", Some(Ctx::Find), Cmd::Next),
            b("ctrl+q", None, Cmd::Quit),
            b("ctrl+z", None, Cmd::Save),
        ]
    }

    #[test]
    fn specs_parse_back_and_label() {
        let p = KeyCombo::parse("Shift+Cmd+P").unwrap();
        assert_eq!(p.spec(), "cmd+shift+p");
        assert_eq!(p.label(), "Cmd+Shift+P");
        assert_eq!(KeyCombo::parse(&p.spec()), Ok(p));
        assert_eq!(KeyCombo::parse("ctrl+alt+backslash").unwrap().label(), "Ctrl+Alt+\\");
        assert_eq!(KeyCombo::parse("escape").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyCombo::parse("f12").unwrap().label(), "F12");
        assert!(KeyCombo::parse("hyper+x").is_err());
        assert!(KeyCombo::parse("cmd+").is_err());
        assert!(KeyCombo::parse("cmd+nope").is_err());
    }

    #[test]
    fn events_normalize() {
        let ev = KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SUPER);
        assert_eq!(KeyCombo::from_event(&ev), KeyCombo::parse("cmd+shift+z").unwrap());
        let ev = KeyEvent::new(KeyCode::Char('!'), KeyModifiers::CONTROL);
        assert_eq!(KeyCombo::from_event(&ev), KeyCombo::parse("ctrl+shift+1").unwrap());
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(KeyCombo::from_event(&ev), KeyCombo::parse("shift+tab").unwrap());
    }

    #[test]
    fn lookup_falls_back_to_the_parent_then_global() {
        let km = Keymap::from_list(defaults());
        assert_eq!(km.lookup(&key("f3"), Ctx::Find), Some(Cmd::Next));
        assert_eq!(km.lookup(&key("f3"), Ctx::Replace), Some(Cmd::Next), "parent");
        assert_eq!(km.lookup(&key("f3"), Ctx::Main), None);
        assert_eq!(km.lookup(&key("ctrl+s"), Ctx::Replace), Some(Cmd::Save), "global");
    }

    #[test]
    fn a_childs_binding_beats_its_parents_and_a_global() {
        let mut list = defaults();
        list.push(b("f3", Some(Ctx::Replace), Cmd::Quit));
        list.push(b("ctrl+s", Some(Ctx::Find), Cmd::Undo));
        let km = Keymap::from_list(list);
        assert_eq!(
            km.lookup(&key("f3"), Ctx::Replace),
            Some(Cmd::Quit),
            "child over parent"
        );
        assert_eq!(km.lookup(&key("f3"), Ctx::Find), Some(Cmd::Next));
        assert_eq!(
            km.lookup(&key("ctrl+s"), Ctx::Replace),
            Some(Cmd::Undo),
            "parent over global"
        );
        assert_eq!(km.lookup(&key("ctrl+s"), Ctx::Main), Some(Cmd::Save));
    }

    #[test]
    fn later_bindings_win_and_the_first_is_shown() {
        let km = Keymap::from_list(defaults());
        assert_eq!(km.lookup(&key("ctrl+z"), Ctx::Main), Some(Cmd::Save));
        assert_eq!(km.binding_for(Cmd::Save), Some(KeyCombo::parse("ctrl+s").unwrap()));
        assert_eq!(km.binding_for(Cmd::Undo), None, "overridden");
    }

    #[test]
    fn passes_through_skips_global_bindings() {
        let km = Keymap::from_list(defaults());
        assert_eq!(km.lookup(&key("ctrl+s"), Ctx::Term), None);
        assert_eq!(km.lookup(&key("ctrl+q"), Ctx::Term), Some(Cmd::Quit));
    }

    #[test]
    fn user_bindings_replace_defaults_per_command() {
        let text = "[[binding]]\nkey = \"ctrl+alt+s\"\ncommand = \"save\"\n\n\
                    [[binding]]\nkey = \"ctrl+n\"\ncommand = \"next\"\ncontext = \"find\"\n";
        let (km, errors) = Keymap::from_text("keys.toml", defaults(), text).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(km.lookup(&key("ctrl+alt+s"), Ctx::Main), Some(Cmd::Save));
        assert_eq!(km.lookup(&key("ctrl+s"), Ctx::Main), None, "default replaced");
        assert_eq!(km.lookup(&key("f3"), Ctx::Find), None, "default replaced");
        assert_eq!(km.lookup(&key("ctrl+n"), Ctx::Replace), Some(Cmd::Next));
        assert_eq!(km.binding_for(Cmd::Save), Some(KeyCombo::parse("ctrl+alt+s").unwrap()));
        let user = km.bindings_with_source().filter(|(.., s)| *s == Source::User);
        assert_eq!(user.count(), 2);
    }

    #[test]
    fn the_reader_reports_each_problem_with_the_file_name() {
        let text = "[[binding]]\nkey = \"hyper+k\"\ncommand = \"nope\"\ncontext = \"zzz\"\n\n\
                    [[binding]]\nkey = \"ctrl+b\"\ncommand = \"undo\"\ncontext = \"main\"\n";
        let (user, errors) = parse_user::<Ctx, Cmd>("keys.toml", text);
        assert_eq!(user.len(), 1);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().all(|e| e.starts_with("keys.toml: ")), "{errors:?}");
        assert!(
            errors.iter().any(|e| e.contains("one of main, find, replace, term")),
            "{errors:?}"
        );
        let (_, errors) = parse_user::<Ctx, Cmd>("keys.toml", "[[binding]]\nkey = 1\n");
        assert_eq!(errors.len(), 1, "a bad file is one error");
    }

    #[test]
    fn load_and_reload_handle_missing_and_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys.toml");
        let (km, errors) = Keymap::load(&path, "keys.toml", defaults());
        assert!(errors.is_empty());
        assert_eq!(km.lookup(&key("ctrl+s"), Ctx::Main), Some(Cmd::Save));
        assert!(Keymap::reload(&path, "keys.toml", defaults()).is_ok());
        std::fs::write(&path, "[[binding").unwrap();
        assert!(Keymap::reload(&path, "keys.toml", defaults()).is_err());
        let (km, errors) = Keymap::load(&path, "keys.toml", defaults());
        assert_eq!(errors.len(), 1);
        assert_eq!(
            km.lookup(&key("ctrl+s"), Ctx::Main),
            Some(Cmd::Save),
            "defaults on a bad file"
        );
    }

    #[test]
    fn context_bindings_are_sorted_by_title() {
        let km = Keymap::from_list(vec![
            b("f4", Some(Ctx::Find), Cmd::Save),
            b("f3", Some(Ctx::Find), Cmd::Next),
            b("f5", Some(Ctx::Main), Cmd::Quit),
        ]);
        let rows = km.context_bindings(Ctx::Find);
        assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), vec![Cmd::Next, Cmd::Save]);
    }
}
