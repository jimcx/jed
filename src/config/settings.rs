//! Editor settings: the `[settings]` table shared by the user's `~/.config/red/settings.toml` and a
//! workspace's `.workspace`, and how the two layer over the defaults.
//!
//! | key                | type | default   | what it does                                                   |
//! |--------------------|------|-----------|----------------------------------------------------------------|
//! | `tab_width`        | int  | 4         | Columns per tab stop (0 is ignored)                            |
//! | `insert_spaces`    | bool | true      | Tab inserts spaces                                             |
//! | `syntax_max_bytes` | int  | 8388608   | No parse tree (no highlighting) past this size; 0: never parse |
//! | `auto_close_pairs` | bool | true      | Brackets/quotes auto-close, type over, delete in pairs, wrap   |
//! | `auto_indent`      | bool | true      | Enter indents; a closing bracket first on a line outdents it    |
//! | `match_brackets`   | bool | true      | Highlight the bracket next to the cursor and its partner       |
//!
//! Precedence, per key: defaults ← `settings.toml` ← `.workspace` (`layer`).
//!
//! Errors: in `settings.toml` an unknown key or a value of the wrong type is reported and skipped,
//! the other keys still apply (VS Code warns about unknown settings and ignores them); a TOML syntax
//! error keeps the defaults. `.workspace` is stricter (`deny_unknown_fields`): any error rejects the
//! whole file and keeps its last good version, since it also holds the commands trust covers.
//!
//! Adding a setting: a field in `edit::Settings` (and its default), an `Option` field here, a line in
//! `apply`, a row in the table above, a line in `TEMPLATE` and the field in
//! `template_lists_every_key_at_its_default`'s literal (it won't compile until then).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::edit::Settings;

pub const FILE_NAME: &str = "settings.toml";

/// Settings a file sets; `None` leaves the value below it (see `layer`).
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsOverrides {
    pub tab_width: Option<usize>,
    pub insert_spaces: Option<bool>,
    /// No parse tree (so no highlighting) for buffers bigger than this; 0 turns parsing off.
    pub syntax_max_bytes: Option<usize>,
    /// Brackets and quotes auto-close, type over and delete in pairs, and wrap a selection.
    pub auto_close_pairs: Option<bool>,
    /// Enter keeps / adds / removes indentation; a closing bracket typed first on a line outdents
    /// it (VS Code's `editor.autoIndent`, off = `none`).
    pub auto_indent: Option<bool>,
    /// Highlight the bracket at the cursor and its partner (VS Code's `editor.matchBrackets`).
    pub match_brackets: Option<bool>,
}

impl SettingsOverrides {
    /// Sets every key this names on `s`.
    pub fn apply(&self, s: &mut Settings) {
        if let Some(w) = self.tab_width.filter(|w| *w > 0) {
            s.tab_width = w;
        }
        if let Some(sp) = self.insert_spaces {
            s.insert_spaces = sp;
        }
        if let Some(max) = self.syntax_max_bytes {
            s.syntax_max_bytes = max;
        }
        if let Some(on) = self.auto_close_pairs {
            s.auto_close_pairs = on;
        }
        if let Some(on) = self.auto_indent {
            s.auto_indent = on;
        }
        if let Some(on) = self.match_brackets {
            s.match_brackets = on;
        }
    }
}

/// The settings in effect: defaults, then the user's `settings.toml`, then `.workspace`, per key.
pub fn layer(user: &SettingsOverrides, workspace: &SettingsOverrides) -> Settings {
    let mut s = Settings::default();
    user.apply(&mut s);
    workspace.apply(&mut s);
    s
}

/// The user's `settings.toml`: what it sets, and a message per problem found loading it.
#[derive(Debug, Default, Clone)]
pub struct UserSettings {
    pub overrides: SettingsOverrides,
    pub errors: Vec<String>,
}

/// Blocking: call before the UI starts (as `main` does) or from `spawn_blocking`.
/// A missing file is no settings and no error.
pub fn load(path: &Path) -> UserSettings {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UserSettings::default(),
        Err(e) => UserSettings {
            overrides: SettingsOverrides::default(),
            errors: vec![format!("{FILE_NAME}: {e}")],
        },
    }
}

/// Blocking. Writes `TEMPLATE` to `path` (creating its directory) unless the file exists; never
/// overwrites one, even if it appears meanwhile (`create_new`).
pub fn create_if_missing(path: &Path) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let fail = |e: std::io::Error| format!("Can't create {}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => f.write_all(TEMPLATE.as_bytes()).map_err(fail)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(fail(e)),
    }
    Ok(path.to_path_buf())
}

/// Parses `settings.toml`, skipping (and reporting) unknown keys and badly typed values.
pub fn parse(text: &str) -> UserSettings {
    let mut errors = Vec::new();
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            return UserSettings {
                overrides: SettingsOverrides::default(),
                errors: vec![match e.span() {
                    Some(at) => {
                        let line = text[..at.start.min(text.len())].matches('\n').count() + 1;
                        format!("{FILE_NAME}: line {line}: {}", e.message().trim_end())
                    }
                    None => format!("{FILE_NAME}: {}", e.message().trim_end()),
                }],
            };
        }
    };
    let mut settings = toml::Table::new();
    for (key, value) in table {
        match (key.as_str(), value) {
            ("settings", toml::Value::Table(t)) => settings = t,
            ("settings", _) => errors.push(format!("{FILE_NAME}: `settings` must be a table")),
            (_, toml::Value::Table(_)) => errors.push(format!("{FILE_NAME}: unknown table [{key}]")),
            _ => errors.push(format!("{FILE_NAME}: `{key}` belongs under [settings]")),
        }
    }
    // Each key on its own, so one bad key doesn't lose the others.
    settings.retain(|key, value| {
        let one = toml::Table::from_iter([(key.to_string(), value.clone())]);
        match toml::Value::Table(one).try_into::<SettingsOverrides>() {
            Ok(_) => true,
            Err(e) if e.message().starts_with("unknown field") => {
                errors.push(format!("{FILE_NAME}: unknown setting `{key}`"));
                false
            }
            Err(e) => {
                errors.push(format!("{FILE_NAME}: `{key}`: {}", e.message().trim_end()));
                false
            }
        }
    });
    let overrides = toml::Value::Table(settings)
        .try_into()
        .unwrap_or_else(|e: toml::de::Error| {
            errors.push(format!("{FILE_NAME}: {}", e.message().trim_end()));
            SettingsOverrides::default()
        });
    UserSettings { overrides, errors }
}

/// What "Preferences: Open User Settings" writes when there's no `settings.toml` yet: every key,
/// commented out, at its default.
pub const TEMPLATE: &str = "\
# red user settings. Uncomment a line to change it.
# A workspace's .workspace [settings] table overrides these, key by key.
# Changes apply the next time red starts.

[settings]
# tab_width = 4
# insert_spaces = true
# syntax_max_bytes = 8388608
# auto_close_pairs = true
# auto_indent = true
# match_brackets = true
";

#[cfg(test)]
mod tests {
    use super::*;

    fn set(text: &str) -> SettingsOverrides {
        let u = parse(text);
        assert_eq!(u.errors, Vec::<String>::new());
        u.overrides
    }

    #[test]
    fn parses_every_key() {
        let o = set(
            "[settings]\ntab_width = 2\ninsert_spaces = false\nsyntax_max_bytes = 10\nauto_close_pairs = false\n\
             auto_indent = false\nmatch_brackets = false\n",
        );
        assert_eq!(
            o,
            SettingsOverrides {
                tab_width: Some(2),
                insert_spaces: Some(false),
                syntax_max_bytes: Some(10),
                auto_close_pairs: Some(false),
                auto_indent: Some(false),
                match_brackets: Some(false),
            }
        );
    }

    #[test]
    fn empty_file_sets_nothing() {
        assert_eq!(set(""), SettingsOverrides::default());
        assert_eq!(set("[settings]\n"), SettingsOverrides::default());
        assert_eq!(set("# just a comment\n"), SettingsOverrides::default());
    }

    #[test]
    fn missing_file_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let u = load(&dir.path().join(FILE_NAME));
        assert!(u.errors.is_empty());
        assert_eq!(u.overrides, SettingsOverrides::default());
    }

    #[test]
    fn bad_keys_are_reported_and_skipped() {
        let u = parse("[settings]\ntab_widht = 2\ninsert_spaces = \"no\"\nauto_close_pairs = false\ntab_width = -1\n");
        assert_eq!(
            u.overrides,
            SettingsOverrides {
                auto_close_pairs: Some(false),
                ..Default::default()
            },
            "the good key still applies"
        );

        assert_eq!(u.errors.len(), 3, "{:?}", u.errors);
        assert!(
            u.errors.iter().any(|e| e.contains("unknown setting `tab_widht`")),
            "{:?}",
            u.errors
        );
        assert!(u.errors.iter().any(|e| e.contains("`insert_spaces`")), "{:?}", u.errors);
        assert!(u.errors.iter().any(|e| e.contains("`tab_width`")), "{:?}", u.errors);
    }

    #[test]
    fn misplaced_keys_and_tables_are_reported() {
        let u = parse("tab_width = 2\n[files]\nexclude = []\n[settings]\nauto_close_pairs = false\n");
        assert_eq!(u.overrides.auto_close_pairs, Some(false));
        assert_eq!(u.overrides.tab_width, None);
        assert_eq!(u.errors.len(), 2, "{:?}", u.errors);
        assert!(
            u.errors
                .iter()
                .any(|e| e.contains("`tab_width` belongs under [settings]"))
        );
        assert!(u.errors.iter().any(|e| e.contains("unknown table [files]")));
        assert_eq!(parse("settings = 3").errors.len(), 1);
    }

    #[test]
    fn syntax_error_keeps_defaults() {
        let u = parse("[settings\ntab_width = 2");
        assert_eq!(u.overrides, SettingsOverrides::default());
        assert_eq!(u.errors.len(), 1);
        assert!(u.errors[0].starts_with("settings.toml: line 1: "), "{:?}", u.errors);
        // Good lines before the error don't apply either.
        let u = parse("[settings]\nauto_close_pairs = false\ntab_width = = 2\n");
        assert_eq!(u.overrides, SettingsOverrides::default());
        assert_eq!(u.errors.len(), 1);
        assert!(u.errors[0].starts_with("settings.toml: line 3: "), "{:?}", u.errors);
    }

    /// Each key on its own: user only, workspace only, both (workspace wins), neither (default).
    #[test]
    fn layering_precedence_per_key() {
        let d = Settings::default();
        let none = SettingsOverrides::default();
        let user = SettingsOverrides {
            tab_width: Some(2),
            syntax_max_bytes: Some(100),
            auto_close_pairs: Some(false),
            ..Default::default()
        };
        let ws = SettingsOverrides {
            tab_width: Some(8),
            insert_spaces: Some(false),
            auto_close_pairs: Some(true),
            ..Default::default()
        };

        let s = layer(&none, &none);
        assert_eq!(
            (s.tab_width, s.insert_spaces, s.syntax_max_bytes, s.auto_close_pairs),
            (d.tab_width, d.insert_spaces, d.syntax_max_bytes, d.auto_close_pairs)
        );
        let s = layer(&user, &none);
        assert_eq!((s.tab_width, s.syntax_max_bytes, s.auto_close_pairs), (2, 100, false));
        assert_eq!(s.insert_spaces, d.insert_spaces);
        let s = layer(&none, &ws);
        assert_eq!((s.tab_width, s.insert_spaces, s.auto_close_pairs), (8, false, true));
        assert_eq!(s.syntax_max_bytes, d.syntax_max_bytes);
        let s = layer(&user, &ws);
        assert_eq!(s.tab_width, 8, "workspace wins");
        assert!(s.auto_close_pairs, "workspace turns it back on");
        assert!(!s.insert_spaces, "workspace only");
        assert_eq!(s.syntax_max_bytes, 100, "user only: the workspace doesn't set it");

        let zero = SettingsOverrides {
            tab_width: Some(0),
            ..Default::default()
        };
        assert_eq!(layer(&user, &zero).tab_width, 2, "tab_width 0 is ignored");
        // `syntax_max_bytes = 0` means "never parse": a workspace 0 beats the user's value, and
        // the user's 0 stays when the workspace doesn't set it.
        let off = SettingsOverrides {
            syntax_max_bytes: Some(0),
            ..Default::default()
        };
        assert_eq!(layer(&user, &off).syntax_max_bytes, 0);
        assert_eq!(layer(&off, &ws).syntax_max_bytes, 0);
        assert_eq!(layer(&off, &none).syntax_max_bytes, 0);
    }

    /// The template, uncommented, sets every key, to its default.
    #[test]
    fn template_lists_every_key_at_its_default() {
        let uncommented: String = TEMPLATE
            .lines()
            .map(|l| l.strip_prefix("# ").filter(|l| l.contains(" = ")).unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        let o = set(&uncommented);
        let keys = toml::Table::try_from(&o).unwrap();
        // Every field named (no `..`): a new field doesn't compile until it's added here.
        let every = SettingsOverrides {
            tab_width: Some(1),
            insert_spaces: Some(true),
            syntax_max_bytes: Some(1),
            auto_close_pairs: Some(true),
            auto_indent: Some(true),
            match_brackets: Some(true),
        };
        let fields = toml::Table::try_from(&every).unwrap();
        assert_eq!(
            keys.keys().collect::<Vec<_>>(),
            fields.keys().collect::<Vec<_>>(),
            "a key is missing from TEMPLATE"
        );
        let mut s = Settings::default();
        o.apply(&mut s);
        let d = Settings::default();
        assert_eq!(
            (s.tab_width, s.insert_spaces, s.syntax_max_bytes, s.auto_close_pairs),
            (d.tab_width, d.insert_spaces, d.syntax_max_bytes, d.auto_close_pairs)
        );
        assert_eq!((s.auto_indent, s.match_brackets), (d.auto_indent, d.match_brackets));
        assert!(parse(TEMPLATE).errors.is_empty());
    }

    #[test]
    fn create_if_missing_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        assert_eq!(create_if_missing(&path).unwrap(), path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        std::fs::write(&path, "[settings]\ntab_width = 3\n").unwrap();
        create_if_missing(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[settings]\ntab_width = 3\n");
    }
}
