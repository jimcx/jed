//! What the config files may contain, as data: which of `settings.toml`, `.workspace` and
//! `keybindings.toml` a path is (`ConfigFile::of`, kept on `Buffer::config_file`), their tables and
//! keys with the kind of value each takes, and the literal values worth offering. The settings-aware
//! highlighter and the completer both read this; nothing here draws or offers anything.
//!
//! Drift guards in the tests tie the tables to the real parsers (`SettingsOverrides`,
//! `WorkspaceConfig`, `Keymap`) and to the two templates.

use std::path::Path;

use crate::command::Command;
use crate::config::keymap::{Context, KeyCombo, NAMED_KEYS};

/// A config file jed knows the shape of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigFile {
    /// `<config dir>/settings.toml`.
    Settings,
    /// `<workspace root>/.workspace`.
    Workspace,
    /// `<config dir>/keybindings.toml`.
    Keybindings,
}

impl ConfigFile {
    /// Which config file `path` is (an exact match of the full normalised absolute path; a
    /// `settings.toml` elsewhere, or a `.workspace` below the root, is none).
    pub fn of(path: &Path, config_dir: &Path, workspace_root: &Path) -> Option<ConfigFile> {
        if path == config_dir.join(crate::config::settings::FILE_NAME) {
            Some(ConfigFile::Settings)
        } else if path == config_dir.join(crate::config::keymap::FILE_NAME) {
            Some(ConfigFile::Keybindings)
        } else if path == workspace_root.join(crate::workspace::FILE_NAME) {
            Some(ConfigFile::Workspace)
        } else {
            None
        }
    }
}

/// What a key's value may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    /// An integer in `min..=max`; `pow2`: also a power of two.
    Int {
        min: i64,
        max: i64,
        pow2: bool,
    },
    Str,
    /// One of these strings.
    Enum(&'static [&'static str]),
    IntList,
    StrList,
    /// A table of string values (`env`, `associations`).
    StrTable,
    /// Anything (`init_options`).
    Any,
    /// An array of tables (`[[window.buttons]]`), whose keys the array table's own schema names.
    Tables,
    /// A command name (`Command::from_name`).
    Command,
    /// A `Context` name.
    Context,
    /// A key combination (`KeyCombo::parse`).
    Key,
}

#[derive(Debug, Clone, Copy)]
pub struct KeySchema {
    pub name: &'static str,
    pub kind: ValueKind,
    /// One line, for the completion's detail.
    pub doc: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct TableSchema {
    /// Dotted table path; a `*` segment matches any name (`lsp.*` is `[lsp.python]`).
    pub pattern: &'static str,
    /// `[[name]]` (an array of tables) rather than `[name]`.
    pub array: bool,
    pub keys: &'static [KeySchema],
}

impl TableSchema {
    pub fn key(&self, name: &str) -> Option<&KeySchema> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// Whether the table at `path` (e.g. `["lsp", "python"]`) is this one.
    pub fn matches(&self, path: &[&str]) -> bool {
        let mut pat = self.pattern.split('.');
        let mut got = path.iter();
        loop {
            match (pat.next(), got.next()) {
                (None, None) => return true,
                (Some("*"), Some(_)) => {}
                (Some(p), Some(g)) if p == *g => {}
                _ => return false,
            }
        }
    }
}

const NONNEG: ValueKind = ValueKind::Int {
    min: 0,
    max: i64::MAX,
    pow2: false,
};

const WHITESPACE: &[&str] = &["none", "boundary", "selection", "trailing", "all"];
const CURSOR_STYLES: &[&str] = &["line", "block", "underline"];
const ON_OFF: &[&str] = &["on", "off"];
const WRAP: &[&str] = &["off", "on"];
const SPELL_SCOPES: &[&str] = &["all", "comments_and_strings", "off"];
const SPELL_LANGUAGES: &[&str] = &["en_AU"];
const STATUS_COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "cyan", "blue", "purple", "magenta", "grey", "auto",
];

const fn key(name: &'static str, kind: ValueKind, doc: &'static str) -> KeySchema {
    KeySchema { name, kind, doc }
}

/// `[settings]` (`config::settings::SettingsOverrides`), the same in both files.
const SETTINGS_KEYS: &[KeySchema] = &[
    key("tab_width", NONNEG, "Columns per tab stop"),
    key("insert_spaces", ValueKind::Bool, "Tab inserts spaces"),
    key(
        "detect_indentation",
        ValueKind::Bool,
        "A file's own indentation overrides the two above",
    ),
    key(
        "syntax_max_bytes",
        NONNEG,
        "No highlighting past this size; 0: never parse",
    ),
    key("auto_close_pairs", ValueKind::Bool, "Brackets and quotes auto-close"),
    key(
        "auto_indent",
        ValueKind::Bool,
        "Enter indents; a closing bracket outdents",
    ),
    key(
        "match_brackets",
        ValueKind::Bool,
        "Highlight the bracket pair at the cursor",
    ),
    key(
        "semantic_highlighting",
        ValueKind::Enum(ON_OFF),
        "The language server's semantic colours",
    ),
    key(
        "quick_suggestions",
        ValueKind::Bool,
        "Typing a word asks the language server for completions",
    ),
    key(
        "render_whitespace",
        ValueKind::Enum(WHITESPACE),
        "Which whitespace is drawn",
    ),
    key(
        "word_wrap",
        ValueKind::Enum(WRAP),
        "Wrap long lines at the editor's width",
    ),
    key(
        "tabs_at_bottom",
        ValueKind::Bool,
        "Dock each group's tab bar at the bottom of the group",
    ),
    key("cursor_style", ValueKind::Enum(CURSOR_STYLES), "The cursor's shape"),
    key("cursor_blinking", ValueKind::Bool, "The cursor blinks"),
    key(
        "hex_bytes_per_line",
        ValueKind::Int {
            min: 1,
            max: crate::hex::MAX_BYTES_PER_ROW as i64,
            pow2: true,
        },
        "A hex tab's widest row: a power of two from 1 to 64",
    ),
    key("rulers", ValueKind::IntList, "Columns that get a vertical line"),
    key(
        "scrollback",
        NONNEG,
        "Lines kept above the screen, for shell and task tabs",
    ),
    key(
        "spell_check_scope",
        ValueKind::Enum(SPELL_SCOPES),
        "What the spell checker looks at: comments, strings and identifiers, or less",
    ),
    key(
        "spell_check_min_length",
        NONNEG,
        "Words shorter than this are not spell-checked",
    ),
    key(
        "spell_check_problems",
        ValueKind::Bool,
        "List misspellings in the Problems panel",
    ),
    key(
        "spell_check_language",
        ValueKind::Enum(SPELL_LANGUAGES),
        "The spell checker's dictionary",
    ),
    key(
        "spell_check_ignore_words",
        ValueKind::StrList,
        "Extra words the spell checker accepts",
    ),
    key(
        "spell_check_off_languages",
        ValueKind::StrList,
        "Language ids that are not spell-checked",
    ),
    key(
        "scrollback_compression",
        ValueKind::Bool,
        "Keep old scrollback lines compressed (less memory, a little CPU)",
    ),
];

const WINDOW_KEYS: &[KeySchema] = &[
    key("title", ValueKind::Str, "The window title is \"jed <title>\""),
    key("status_bar", ValueKind::Enum(STATUS_COLORS), "The status bar's colour"),
    key(
        "title_bar",
        ValueKind::Bool,
        "A row at the top naming the window, with buttons",
    ),
    key(
        "buttons",
        ValueKind::Tables,
        "[[window.buttons]]: command or task, and an optional label",
    ),
];

const LSP_KEYS: &[KeySchema] = &[
    key("command", ValueKind::Str, "The server's command (a name on PATH)"),
    key("args", ValueKind::StrList, "Arguments for the command"),
    key(
        "init_options",
        ValueKind::Any,
        "Sent to the server as initializationOptions",
    ),
];

const FILES_KEYS: &[KeySchema] = &[
    key(
        "exclude",
        ValueKind::StrList,
        "Globs hidden from the explorer, quick open and search",
    ),
    key("associations", ValueKind::StrTable, "Glob = language id"),
];

const TASK_KEYS: &[KeySchema] = &[
    key("command", ValueKind::Str, "The command line to run"),
    key("cwd", ValueKind::Str, "Working directory, relative to the workspace"),
    key("env", ValueKind::StrTable, "Environment variables"),
];

const HISTORY_KEYS: &[KeySchema] = &[
    key("enabled", ValueKind::Bool, "Record a snapshot on every save"),
    key("max_entries", NONNEG, "Entries kept per file"),
    key("max_file_size", NONNEG, "Larger files (bytes) aren't snapshotted"),
];

const BINDING_KEYS: &[KeySchema] = &[
    key("key", ValueKind::Key, "The key combination, e.g. ctrl+shift+p"),
    key("command", ValueKind::Command, "The command's name"),
    key(
        "context",
        ValueKind::Context,
        "Where the binding applies; unset: everywhere",
    ),
];

const BUTTON_KEYS: &[KeySchema] = &[
    key("command", ValueKind::Command, "A palette command (this or task)"),
    key("task", ValueKind::Str, "A task's name (this or command)"),
    key("label", ValueKind::Str, "The button's text, e.g. one character"),
];

const fn table(pattern: &'static str, keys: &'static [KeySchema]) -> TableSchema {
    TableSchema {
        pattern,
        array: false,
        keys,
    }
}

const SETTINGS_TABLES: &[TableSchema] = &[
    table("settings", SETTINGS_KEYS),
    table("window", WINDOW_KEYS),
    TableSchema {
        pattern: "window.buttons",
        array: true,
        keys: BUTTON_KEYS,
    },
    table("lsp.*", LSP_KEYS),
];

const WORKSPACE_TABLES: &[TableSchema] = &[
    table("settings", SETTINGS_KEYS),
    table("window", WINDOW_KEYS),
    TableSchema {
        pattern: "window.buttons",
        array: true,
        keys: BUTTON_KEYS,
    },
    table("files", FILES_KEYS),
    table("lsp.*", LSP_KEYS),
    table("tasks.*", TASK_KEYS),
    // Any NAME = "value": the variables are the user's own.
    table("environments.*", &[]),
    table("history", HISTORY_KEYS),
];

const KEYBINDING_TABLES: &[TableSchema] = &[TableSchema {
    pattern: "binding",
    array: true,
    keys: BINDING_KEYS,
}];

/// The tables `file` may hold.
pub fn tables(file: ConfigFile) -> &'static [TableSchema] {
    match file {
        ConfigFile::Settings => SETTINGS_TABLES,
        ConfigFile::Workspace => WORKSPACE_TABLES,
        ConfigFile::Keybindings => KEYBINDING_TABLES,
    }
}

/// The schema of the table at `path` in `file` (`None`: not a table that file knows).
pub fn table_at(file: ConfigFile, path: &[&str]) -> Option<&'static TableSchema> {
    tables(file).iter().find(|t| t.matches(path))
}

/// Whether `value` is acceptable for `kind`; the error says what is wrong.
pub fn check(kind: ValueKind, value: &toml::Value) -> Result<(), String> {
    use toml::Value as V;
    let wrong = |what: &str| Err(format!("expected {what}"));
    match (kind, value) {
        (ValueKind::Any, _) => Ok(()),
        (ValueKind::Tables, V::Array(a)) if a.iter().all(V::is_table) => Ok(()),
        (ValueKind::Tables, _) => wrong("a list of tables"),
        (ValueKind::Bool, V::Boolean(_)) => Ok(()),
        (ValueKind::Bool, _) => wrong("true or false"),
        (ValueKind::Int { min, max, pow2 }, V::Integer(n)) => {
            if *n < min || *n > max {
                Err(format!("must be from {min} to {max}"))
            } else if pow2 && !(*n as u64).is_power_of_two() {
                Err("must be a power of two".into())
            } else {
                Ok(())
            }
        }
        (ValueKind::Int { .. }, _) => wrong("an integer"),
        (ValueKind::Str, V::String(_)) => Ok(()),
        (ValueKind::Str, _) => wrong("a string"),
        (ValueKind::Enum(options), V::String(s)) => {
            if options.contains(&s.as_str()) {
                Ok(())
            } else {
                Err(format!("must be one of {}", options.join(", ")))
            }
        }
        (ValueKind::Enum(_), _) => wrong("a string"),
        (ValueKind::IntList, V::Array(a)) => {
            if a.iter().all(|v| matches!(v, V::Integer(n) if *n >= 0)) {
                Ok(())
            } else {
                wrong("a list of non-negative integers")
            }
        }
        (ValueKind::IntList, _) => wrong("a list of integers"),
        (ValueKind::StrList, V::Array(a)) => {
            if a.iter().all(V::is_str) {
                Ok(())
            } else {
                wrong("a list of strings")
            }
        }
        (ValueKind::StrList, _) => wrong("a list of strings"),
        (ValueKind::StrTable, V::Table(t)) => {
            if t.values().all(V::is_str) {
                Ok(())
            } else {
                wrong("a table of strings")
            }
        }
        (ValueKind::StrTable, _) => wrong("a table of strings"),
        (ValueKind::Command, V::String(s)) => match Command::from_name(s) {
            Some(_) => Ok(()),
            None => Err(format!("unknown command \"{s}\"")),
        },
        (ValueKind::Context, V::String(s)) => {
            if Context::ALL.iter().any(|(_, n)| n == s) {
                Ok(())
            } else {
                Err(format!("unknown context \"{s}\""))
            }
        }
        (ValueKind::Key, V::String(s)) => KeyCombo::parse(s).map(|_| ()),
        (ValueKind::Command | ValueKind::Context | ValueKind::Key, _) => wrong("a string"),
    }
}

/// A literal value worth offering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The value as written in TOML (strings quoted: `"all"`, `true`, `16`); command and context
    /// names are bare, since the cursor is already inside the quotes there.
    pub text: String,
    /// A title for the list, empty when there is none.
    pub detail: String,
}

fn plain(text: impl Into<String>) -> Candidate {
    Candidate {
        text: text.into(),
        detail: String::new(),
    }
}

/// The values to offer for `kind` (empty when there is no list: `Key` has `key_pieces` instead).
/// Command names come with their titles, context names with theirs.
pub fn candidates(kind: ValueKind) -> Vec<Candidate> {
    match kind {
        ValueKind::Bool => vec![plain("true"), plain("false")],
        ValueKind::Int { min, max, pow2: true } => std::iter::successors(Some(1i64), |n| n.checked_mul(2))
            .take_while(|n| *n <= max)
            .filter(|n| *n >= min)
            .map(|n| plain(n.to_string()))
            .collect(),
        ValueKind::Enum(options) => options.iter().map(|o| plain(format!("\"{o}\""))).collect(),
        ValueKind::Command => keybinding_commands()
            .into_iter()
            .map(|(name, title)| Candidate {
                text: name,
                detail: title,
            })
            .collect(),
        ValueKind::Context => keybinding_contexts()
            .into_iter()
            .map(|(name, title)| Candidate {
                text: name.into(),
                detail: title.into(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Every command name `keybindings.toml` accepts, with its title.
pub fn keybinding_commands() -> Vec<(String, String)> {
    Command::all().into_iter().map(|c| (c.name(), c.title())).collect()
}

/// Every context name `keybindings.toml` accepts, with its title.
pub fn keybinding_contexts() -> Vec<(&'static str, &'static str)> {
    Context::ALL.iter().map(|(c, n)| (*n, c.title())).collect()
}

/// The modifier prefixes of a `key` value.
pub const KEY_MODIFIERS: [&str; 4] = ["ctrl+", "shift+", "alt+", "cmd+"];

/// The key names `KeyCombo::parse` reads beyond a single character.
pub fn key_names() -> Vec<String> {
    let mut names: Vec<String> = NAMED_KEYS.iter().map(|(_, spec, _)| spec.to_string()).collect();
    names.extend(["backslash", "plus"].map(String::from));
    names.extend((1..=24).map(|n| format!("f{n}")));
    names
}

/// What a `key` value is built from: the modifier prefixes, then the key names.
pub fn key_pieces() -> Vec<String> {
    KEY_MODIFIERS.iter().map(|m| m.to_string()).chain(key_names()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keymap::{Keymap, Platform};
    use crate::workspace::config::{StatusBarColor, WorkspaceConfig};
    use std::path::PathBuf;

    fn toml_of(s: &str) -> toml::Value {
        toml::from_str::<toml::Table>(&format!("v = {s}"))
            .unwrap()
            .remove("v")
            .unwrap()
    }

    fn hex_kind() -> ValueKind {
        SETTINGS_KEYS
            .iter()
            .find(|k| k.name == "hex_bytes_per_line")
            .unwrap()
            .kind
    }

    #[test]
    fn config_file_matches_the_exact_paths() {
        let (cfg, root) = (Path::new("/home/u/.config/jed"), Path::new("/w"));
        let of = |p: &str| ConfigFile::of(&PathBuf::from(p), cfg, root);
        assert_eq!(of("/home/u/.config/jed/settings.toml"), Some(ConfigFile::Settings));
        assert_eq!(
            of("/home/u/.config/jed/keybindings.toml"),
            Some(ConfigFile::Keybindings)
        );
        assert_eq!(of("/w/.workspace"), Some(ConfigFile::Workspace));
        assert_eq!(of("/w/Cargo.toml"), None);
        assert_eq!(of("/w/settings.toml"), None);
        assert_eq!(of("/w/sub/.workspace"), None);
        assert_eq!(of("/other/.workspace"), None);
        assert_eq!(of("/home/u/.config/jed/sub/settings.toml"), None);
    }

    #[test]
    fn table_patterns() {
        let t = |f, p: &[&str]| table_at(f, p).map(|t| t.pattern);
        assert_eq!(t(ConfigFile::Workspace, &["lsp", "python"]), Some("lsp.*"));
        assert_eq!(t(ConfigFile::Workspace, &["tasks", "build"]), Some("tasks.*"));
        assert_eq!(t(ConfigFile::Workspace, &["lsp"]), None);
        assert_eq!(t(ConfigFile::Settings, &["tasks", "build"]), None);
        assert_eq!(t(ConfigFile::Settings, &["files"]), None);
        assert_eq!(t(ConfigFile::Keybindings, &["binding"]), Some("binding"));
        assert!(tables(ConfigFile::Keybindings)[0].array);
    }

    #[test]
    fn check_values() {
        let hex = hex_kind();
        assert!(check(hex, &toml_of("16")).is_ok());
        for bad in ["0", "3", "128", "\"16\"", "true"] {
            assert!(check(hex, &toml_of(bad)).is_err(), "{bad}");
        }
        let ws = ValueKind::Enum(WHITESPACE);
        assert!(check(ws, &toml_of("\"all\"")).is_ok());
        assert!(check(ws, &toml_of("\"most\"")).is_err());
        assert!(check(ValueKind::IntList, &toml_of("[80, 120]")).is_ok());
        assert!(check(ValueKind::IntList, &toml_of("[-1]")).is_err());
        assert!(check(ValueKind::StrTable, &toml_of("{ a = \"b\" }")).is_ok());
        assert!(check(ValueKind::StrTable, &toml_of("{ a = 1 }")).is_err());
        assert!(check(ValueKind::Command, &toml_of("\"nope\"")).is_err());
        assert!(check(ValueKind::Command, &toml_of(&format!("\"{}\"", cmds_first()))).is_ok());
        assert!(check(ValueKind::Key, &toml_of("\"ctrl+shift+p\"")).is_ok());
        assert!(check(ValueKind::Key, &toml_of("\"hyper+p\"")).is_err());
        assert!(check(ValueKind::Context, &toml_of("\"searchResults\"")).is_ok());
        assert!(check(ValueKind::Context, &toml_of("\"nowhere\"")).is_err());
    }

    #[test]
    fn candidates_lists() {
        let texts = |k| candidates(k).into_iter().map(|c| c.text).collect::<Vec<_>>();
        assert_eq!(texts(ValueKind::Bool), ["true", "false"]);
        assert_eq!(texts(hex_kind()), ["1", "2", "4", "8", "16", "32", "64"]);
        assert_eq!(
            texts(ValueKind::Enum(CURSOR_STYLES)),
            ["\"line\"", "\"block\"", "\"underline\""]
        );
        assert!(texts(ValueKind::Key).is_empty());
    }

    /// A valid sample for each kind, to build a table setting every key.
    fn sample(kind: ValueKind) -> String {
        match kind {
            ValueKind::Bool => "true".into(),
            ValueKind::Int { .. } => "16".into(),
            ValueKind::Str => "\"x\"".into(),
            ValueKind::Enum(o) => format!("\"{}\"", o[0]),
            ValueKind::IntList => "[80]".into(),
            ValueKind::StrList => "[\"a\"]".into(),
            ValueKind::StrTable => "{ a = \"b\" }".into(),
            ValueKind::Any => "{ a = 1 }".into(),
            ValueKind::Tables => "[{ command = \"file.save\" }]".into(),
            ValueKind::Command => "\"x\"".into(),
            ValueKind::Context => "\"editor\"".into(),
            ValueKind::Key => "\"ctrl+p\"".into(),
        }
    }

    fn header(pattern: &str) -> String {
        pattern.replace('*', "name")
    }

    /// `[[window.buttons]]`'s keys are `RawButton`'s fields, in both files.
    #[test]
    fn button_keys_are_accepted_by_both_files() {
        let text = "[[window.buttons]]\ncommand = \"file.save\"\nlabel = \"S\"\n[[window.buttons]]\ntask = \"t\"\n";
        let w = WorkspaceConfig::parse(text).unwrap();
        assert_eq!(w.window.buttons.unwrap().len(), 2);
        assert!(crate::config::settings::parse(text).errors.is_empty());
        for k in BUTTON_KEYS {
            assert!(text.contains(&format!("{} = ", k.name)), "{}", k.name);
        }
        assert!(WorkspaceConfig::parse("[[window.buttons]]\ncommand = \"file.save\"\nzz = 1\n").is_err());
    }

    #[test]
    fn workspace_schema_matches_the_parser() {
        for t in WORKSPACE_TABLES.iter().filter(|t| !t.array) {
            let mut text = format!("[{}]\n", header(t.pattern));
            for k in t.keys {
                text += &format!("{} = {}\n", k.name, sample(k.kind));
            }
            WorkspaceConfig::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", t.pattern));
            for k in t.keys {
                assert!(check(k.kind, &toml_of(&sample(k.kind))).is_ok(), "{}", k.name);
            }
            let bad = format!("[{}]\ncommand = \"x\"\nno_such_key = 1\n", header(t.pattern));
            assert!(WorkspaceConfig::parse(&bad).is_err(), "{}", t.pattern);
        }
        assert!(WorkspaceConfig::parse("[nope]\nx = 1\n").is_err());
    }

    #[test]
    fn settings_schema_matches_the_parser() {
        let mut text = String::from("[settings]\n");
        for k in SETTINGS_KEYS {
            text += &format!("{} = {}\n", k.name, sample(k.kind));
        }
        text += "[window]\ntitle = \"x\"\nstatus_bar = \"red\"\n";
        let u = crate::config::settings::parse(&text);
        assert!(u.errors.is_empty(), "{:?}", u.errors);
        let u = crate::config::settings::parse("[settings]\nno_such_key = 1\n");
        assert_eq!(u.errors.len(), 1);
        // Every enum candidate is accepted by serde, and a bogus one is not.
        for (table, keys) in [("settings", SETTINGS_KEYS), ("window", WINDOW_KEYS)] {
            for k in keys {
                if let ValueKind::Enum(options) = k.kind {
                    for o in options.iter().copied().chain(["bogus"]) {
                        let u = crate::config::settings::parse(&format!("[{table}]\n{} = \"{o}\"\n", k.name));
                        assert_eq!(u.errors.is_empty(), o != "bogus", "{} = {o}: {:?}", k.name, u.errors);
                    }
                }
            }
        }
        // The pow2 range matches the parser's.
        for n in 0..=130 {
            let u = crate::config::settings::parse(&format!("[settings]\nhex_bytes_per_line = {n}\n"));
            assert_eq!(
                u.errors.is_empty(),
                check(hex_kind(), &toml_of(&n.to_string())).is_ok(),
                "{n}"
            );
        }
    }

    #[test]
    fn status_bar_names_are_the_colors() {
        let mut names: Vec<&str> = StatusBarColor::ALL.iter().map(|c| c.name()).collect();
        names.push("auto");
        assert_eq!(names, STATUS_COLORS);
    }

    /// Every key a template mentions (commented out or not) is in the schema.
    fn template_keys(template: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut current = String::new();
        for line in template.lines() {
            let l = line.strip_prefix("# ").unwrap_or(line).trim();
            if l.starts_with('[') && l.ends_with(']') && !l.contains(' ') && !l.contains('"') {
                current = l.trim_matches(|c| c == '[' || c == ']').to_string();
            } else if let Some((k, _)) = l.split_once(" = ")
                && !k.is_empty()
                && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            {
                out.push((current.clone(), k.to_string()));
            }
        }
        out
    }

    #[test]
    fn templates_use_only_schema_keys() {
        for (file, template) in [
            (ConfigFile::Settings, crate::config::settings::TEMPLATE),
            (ConfigFile::Workspace, crate::workspace::config::TEMPLATE),
        ] {
            let found = template_keys(template);
            assert!(found.len() > 5, "{file:?}: {found:?}");
            for (table, k) in found {
                let path: Vec<&str> = table.split('.').collect();
                let t = table_at(file, &path).unwrap_or_else(|| panic!("{file:?}: no table [{table}]"));
                assert!(t.key(&k).is_some(), "{file:?}: [{table}] {k}");
            }
        }
    }

    #[test]
    fn keybinding_names_round_trip() {
        let cmds = keybinding_commands();
        assert_eq!(cmds.len(), Command::all().len());
        for (name, title) in &cmds {
            assert_eq!(
                Command::from_name(name).map(|c| c.name()).as_deref(),
                Some(name.as_str())
            );
            assert!(!title.is_empty(), "{name}");
        }
        assert_eq!(keybinding_contexts().len(), Context::ALL.len());
        for (name, title) in keybinding_contexts() {
            let c = Context::ALL.iter().find(|(_, n)| *n == name).unwrap().0;
            assert_eq!(c.title(), title);
        }
        let names = key_names();
        for n in &names {
            assert!(KeyCombo::parse(n).is_ok(), "{n}");
            assert!(KeyCombo::parse(&format!("ctrl+shift+{n}")).is_ok(), "{n}");
        }
        for m in KEY_MODIFIERS {
            assert!(KeyCombo::parse(&format!("{m}a")).is_ok(), "{m}");
        }
        for (_, spec, _) in NAMED_KEYS {
            assert!(names.iter().any(|n| n == spec), "{spec}");
        }
        assert_eq!(key_pieces().len(), 4 + names.len());
    }

    #[test]
    fn keybinding_keys_are_the_user_binding_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");
        let load = |text: &str| {
            std::fs::write(&path, text).unwrap();
            Keymap::load(&path, Platform::Linux).1
        };
        let mut good = String::from("[[binding]]\n");
        for k in BINDING_KEYS {
            let v = match k.name {
                "key" => "ctrl+alt+9",
                "command" => &cmds_first(),
                _ => "editor",
            };
            good += &format!("{} = \"{v}\"\n", k.name);
        }
        assert!(load(&good).is_empty(), "{good}");
        assert!(!load("[[binding]]\nkey = \"ctrl+p\"\ncommand = \"left\"\nwhen = \"editor\"\n").is_empty());
    }

    /// The field names serde's `deny_unknown_fields` error lists for `T` ("unknown field `zz`,
    /// expected one of `a`, `b`"): the struct's own fields, so a field added to or dropped from it
    /// without the schema following fails here.
    fn serde_fields<T: serde::de::DeserializeOwned + std::fmt::Debug>() -> Vec<String> {
        let e = toml::from_str::<T>("zz_no_such_field = 1\n").unwrap_err();
        let msg = e.message().to_string();
        let (_, expected) = msg.split_once("expected").unwrap_or_else(|| panic!("{msg}"));
        let mut names: Vec<String> = expected.split('`').skip(1).step_by(2).map(String::from).collect();
        names.sort();
        names
    }

    fn schema_names(keys: &[KeySchema]) -> Vec<String> {
        let mut names: Vec<String> = keys.iter().map(|k| k.name.to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn schema_keys_are_exactly_the_struct_fields() {
        use crate::config::settings::SettingsOverrides;
        use crate::workspace::config::{FilesConfig, HistoryConfig, LspServer, Task, WindowConfig};
        let keys = |pattern: &str| {
            table_at(ConfigFile::Workspace, &header(pattern).split('.').collect::<Vec<_>>())
                .unwrap()
                .keys
        };
        assert_eq!(serde_fields::<SettingsOverrides>(), schema_names(keys("settings")));
        assert_eq!(serde_fields::<WindowConfig>(), schema_names(keys("window")));
        assert_eq!(serde_fields::<FilesConfig>(), schema_names(keys("files")));
        assert_eq!(serde_fields::<HistoryConfig>(), schema_names(keys("history")));
        assert_eq!(serde_fields::<LspServer>(), schema_names(keys("lsp.*")));
        assert_eq!(serde_fields::<Task>(), schema_names(keys("tasks.*")));
        // `.workspace`'s top-level tables are `WorkspaceConfig`'s fields.
        let mut roots: Vec<String> = WORKSPACE_TABLES
            .iter()
            .map(|t| t.pattern.split('.').next().unwrap().to_string())
            .collect();
        roots.sort();
        roots.dedup();
        assert_eq!(serde_fields::<WorkspaceConfig>(), roots);
        // The two files share `[settings]`, `[window]` and `[lsp.*]`.
        for t in SETTINGS_TABLES.iter().filter(|t| !t.array) {
            let w = table_at(ConfigFile::Workspace, &header(t.pattern).split('.').collect::<Vec<_>>()).unwrap();
            assert_eq!(schema_names(t.keys), schema_names(w.keys), "{}", t.pattern);
        }
    }

    /// `settings.toml` takes `[settings]`, `[window]` and `[lsp.*]` (every key of each), and
    /// reports the `.workspace`-only tables.
    #[test]
    fn settings_file_tables_match_the_parser() {
        let mut text = String::new();
        for t in SETTINGS_TABLES.iter().filter(|t| !t.array) {
            text += &format!("[{}]\n", header(t.pattern));
            for k in t.keys {
                text += &format!("{} = {}\n", k.name, sample(k.kind));
            }
        }
        let u = crate::config::settings::parse(&text);
        assert!(u.errors.is_empty(), "{text}{:?}", u.errors);
        assert_eq!(u.lsp.len(), 1);
        for t in WORKSPACE_TABLES.iter().filter(|t| !t.array && !t.keys.is_empty()) {
            let path: Vec<String> = header(t.pattern).split('.').map(String::from).collect();
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let known = table_at(ConfigFile::Settings, &path).is_some();
            let k = &t.keys[0];
            let u =
                crate::config::settings::parse(&format!("[{}]\n{} = {}\n", header(t.pattern), k.name, sample(k.kind)));
            assert_eq!(u.errors.is_empty(), known, "[{}]: {:?}", t.pattern, u.errors);
        }
    }

    #[test]
    fn binding_keys_are_exactly_the_loader_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");
        std::fs::write(&path, "[[binding]]\nzz_no_such_field = 1\n").unwrap();
        let errors = Keymap::load(&path, Platform::Linux).1;
        let msg = errors.join("\n");
        let (_, expected) = msg.split_once("expected").unwrap_or_else(|| panic!("{msg}"));
        let mut names: Vec<String> = expected.split('`').skip(1).step_by(2).map(String::from).collect();
        names.sort();
        assert_eq!(names, schema_names(BINDING_KEYS));
    }

    fn cmds_first() -> String {
        keybinding_commands()[0].0.clone()
    }
}
