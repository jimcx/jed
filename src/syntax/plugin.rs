//! Syntax plugins' manifests (`~/.config/jed/languages/<id>/language.toml`) and the registry of
//! the languages they define. A manifest is parsed leniently, like `settings.toml`: unknown keys
//! and bad values are reported, the rest applies. The registry is filled once in `main`
//! (`load_dir` + `register`); entries are leaked, so a `Language::Plugin(PluginId)` stays `Copy`
//! and its id, name and `LanguageConfig` are `'static`. Grammars come later; here a plugin only
//! has its names, file patterns, editing rules and the node kinds `Language::scope_of` consults.

use std::path::Path;
use std::sync::RwLock;

use super::config::{self, AutoPair, LanguageConfig};
use super::{Language, Scope};

/// The manifest's file name inside a plugin's directory.
pub const MANIFEST: &str = "language.toml";

/// An index into the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginId(u32);

/// A parsed `language.toml`: owned, with the defaults filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub symbol: String,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub shebangs: Vec<String>,
    pub line_comment: Option<String>,
    pub brackets: Vec<(char, char)>,
    pub pairs: Vec<AutoPair>,
    pub surround: Vec<(char, char)>,
    pub insert_spaces: Option<bool>,
    pub tree_tokens: bool,
    pub comment_kinds: Vec<String>,
    pub string_kinds: Vec<String>,
    pub code_kinds: Vec<String>,
}

/// A registered plugin (leaked). The file patterns and `symbol` are read by detection and loading
/// (later subtasks).
#[derive(Debug)]
#[allow(dead_code)] // `symbol` is read by loading (a later subtask).
pub struct Plugin {
    pub id: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub extensions: Vec<String>,
    pub filenames: Vec<String>,
    pub shebangs: Vec<String>,
    pub config: LanguageConfig,
    pub comment_kinds: Vec<String>,
    pub string_kinds: Vec<String>,
    pub code_kinds: Vec<String>,
}

static REGISTRY: RwLock<Vec<&'static Plugin>> = RwLock::new(Vec::new());

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn leak_slice<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

/// Adds a plugin to the registry. Fails (the plugin is skipped) for an id a built-in language
/// uses or one already registered.
pub fn register(m: Manifest) -> Result<PluginId, String> {
    if Language::BUILT_IN.iter().any(|l| l.id() == m.id) {
        return Err(format!("`{}` is a built-in language's id", m.id));
    }
    let mut registry = REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    if registry.iter().any(|p| p.id == m.id) {
        return Err(format!("language `{}` is defined twice", m.id));
    }
    let config = LanguageConfig {
        line_comment: m.line_comment.as_deref().map(leak_str),
        pairs: leak_slice(m.pairs),
        brackets: leak_slice(m.brackets),
        tree_tokens: m.tree_tokens,
        insert_spaces: m.insert_spaces,
        surround: leak_slice(m.surround),
        ..config::config(None, &[], &[], &[])
    };
    let plugin: &'static Plugin = Box::leak(Box::new(Plugin {
        id: leak_str(&m.id),
        name: leak_str(&m.name),
        symbol: leak_str(&m.symbol),
        extensions: m.extensions,
        filenames: m.filenames,
        shebangs: m.shebangs,
        config,
        comment_kinds: m.comment_kinds,
        string_kinds: m.string_kinds,
        code_kinds: m.code_kinds,
    }));
    registry.push(plugin);
    Ok(PluginId(registry.len() as u32 - 1))
}

impl PluginId {
    pub fn get(self) -> &'static Plugin {
        REGISTRY.read().unwrap_or_else(|e| e.into_inner())[self.0 as usize]
    }

    /// The registered plugin with this language id.
    pub fn find(id: &str) -> Option<PluginId> {
        let registry = REGISTRY.read().unwrap_or_else(|e| e.into_inner());
        registry.iter().position(|p| p.id == id).map(|i| PluginId(i as u32))
    }

    /// Every registered plugin, in registration order.
    pub fn all() -> Vec<PluginId> {
        let n = REGISTRY.read().unwrap_or_else(|e| e.into_inner()).len();
        (0..n as u32).map(PluginId).collect()
    }

    /// The scope a plugin's `comment_kinds` / `string_kinds` / `code_kinds` give a node kind.
    pub(super) fn scope_of(self, kind: &str) -> Option<Scope> {
        let p = self.get();
        let has = |kinds: &[String]| kinds.iter().any(|k| k == kind);
        if has(&p.code_kinds) {
            Some(Scope::Code)
        } else if has(&p.comment_kinds) {
            Some(Scope::Comment)
        } else if has(&p.string_kinds) {
            Some(Scope::String)
        } else {
            None
        }
    }
}

/// Reads `<dir>/*/language.toml` (a missing `dir` is no plugins), registers each plugin and
/// returns the errors for the user (each names its file, e.g. `languages/lua/language.toml`).
/// Blocking: call from `main`, before raw mode.
pub fn load_and_register(dir: &Path) -> Vec<String> {
    let (manifests, mut errors) = load_dir(dir);
    for m in manifests {
        let id = m.id.clone();
        if let Err(e) = register(m) {
            errors.push(format!("languages/{id}/{MANIFEST}: {e}"));
        }
    }
    errors
}

/// Parses every `<dir>/<name>/language.toml`, in directory order. A plugin whose `id` isn't its
/// directory's name is reported and skipped.
/// Blocking: call from `spawn_blocking` (or `main` before raw mode).
pub fn load_dir(dir: &Path) -> (Vec<Manifest>, Vec<String>) {
    let mut manifests = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (manifests, errors);
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let label = format!("languages/{name}/{MANIFEST}");
        let text = match std::fs::read_to_string(dir.join(&name).join(MANIFEST)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push(format!("{label}: {e}"));
                continue;
            }
        };
        let (manifest, errs) = parse(&text, &label);
        errors.extend(errs);
        let Some(manifest) = manifest else { continue };
        if manifest.id != name {
            errors.push(format!(
                "{label}: `id` is \"{}\" but the directory is \"{name}\"",
                manifest.id
            ));
            continue;
        }
        manifests.push(manifest);
    }
    (manifests, errors)
}

/// Parses a manifest; `file` labels the errors. `None` when it has no usable `id` or isn't valid
/// TOML; otherwise every key that parses applies and the others are reported.
pub fn parse(text: &str, file: &str) -> (Option<Manifest>, Vec<String>) {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            let msg = e.message().trim_end();
            let err = match e.span() {
                Some(at) => {
                    let line = text[..at.start.min(text.len())].matches('\n').count() + 1;
                    format!("{file}:{line}: {msg}")
                }
                None => format!("{file}: {msg}"),
            };
            return (None, vec![err]);
        }
    };
    let mut p = Parser {
        file,
        errors: Vec::new(),
    };
    let mut id = None;
    let mut name = None;
    let mut symbol = None;
    let mut m = Manifest {
        id: String::new(),
        name: String::new(),
        symbol: String::new(),
        extensions: Vec::new(),
        filenames: Vec::new(),
        shebangs: Vec::new(),
        line_comment: None,
        brackets: config::BRACKET_PAIRS.to_vec(),
        pairs: config::BRACKETS.to_vec(),
        surround: config::SURROUND.to_vec(),
        insert_spaces: None,
        tree_tokens: true,
        comment_kinds: Vec::new(),
        string_kinds: Vec::new(),
        code_kinds: Vec::new(),
    };
    for (key, value) in table {
        match key.as_str() {
            "id" => {
                id = p
                    .string(&key, &value)
                    .filter(|s| p.check(&key, s, valid_id(s), "letters, digits, `_`, `-`, `.`"))
            }
            "name" => {
                name = p
                    .string(&key, &value)
                    .filter(|s| p.check(&key, s, !s.trim().is_empty(), "not empty"))
            }
            "symbol" => {
                symbol = p
                    .string(&key, &value)
                    .filter(|s| p.check(&key, s, valid_symbol(s), "a C identifier"))
            }
            "files" => match value {
                toml::Value::Table(files) => {
                    for (k, v) in files {
                        match k.as_str() {
                            "extensions" => {
                                m.extensions = p
                                    .strings(&format!("files.{k}"), &v)
                                    .into_iter()
                                    .map(|e| e.trim_start_matches('.').to_lowercase())
                                    .filter(|e| !e.is_empty())
                                    .collect()
                            }
                            "filenames" => m.filenames = p.strings(&format!("files.{k}"), &v),
                            "shebangs" => m.shebangs = p.strings(&format!("files.{k}"), &v),
                            _ => p.error(format!("unknown key `files.{k}`")),
                        }
                    }
                }
                _ => p.error("`files` must be a table".into()),
            },
            "line_comment" => {
                m.line_comment = p
                    .string(&key, &value)
                    .filter(|s| p.check(&key, s, !s.trim().is_empty(), "not empty"))
            }
            "brackets" => {
                if let Some(v) = p.char_pairs(&key, &value) {
                    m.brackets = v;
                }
            }
            "surround" => {
                if let Some(v) = p.char_pairs(&key, &value) {
                    m.surround = v;
                }
            }
            "pairs" => {
                if let Some(v) = p.auto_pairs(&key, &value) {
                    m.pairs = v;
                }
            }
            "insert_spaces" => m.insert_spaces = p.boolean(&key, &value),
            "tree_tokens" => m.tree_tokens = p.boolean(&key, &value).unwrap_or(true),
            "comment_kinds" => m.comment_kinds = p.strings(&key, &value),
            "string_kinds" => m.string_kinds = p.strings(&key, &value),
            "code_kinds" => m.code_kinds = p.strings(&key, &value),
            _ => p.error(format!("unknown key `{key}`")),
        }
    }
    let Some(id) = id else {
        if !p.errors.iter().any(|e| e.contains("`id`")) {
            p.error("missing `id`".into());
        }
        return (None, p.errors);
    };
    m.name = name.unwrap_or_else(|| id.clone());
    m.symbol = symbol.unwrap_or_else(|| format!("tree_sitter_{}", id.replace(['-', '.'], "_")));
    m.id = id;
    (Some(m), p.errors)
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn valid_symbol(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct Parser<'a> {
    file: &'a str,
    errors: Vec<String>,
}

impl Parser<'_> {
    fn error(&mut self, msg: String) {
        self.errors.push(format!("{}: {msg}", self.file));
    }

    /// Reports `key`'s value unless `ok`; returns `ok`.
    fn check(&mut self, key: &str, value: &str, ok: bool, want: &str) -> bool {
        if !ok {
            self.error(format!("`{key}`: \"{value}\" must be {want}"));
        }
        ok
    }

    fn string(&mut self, key: &str, v: &toml::Value) -> Option<String> {
        match v.as_str() {
            Some(s) => Some(s.to_owned()),
            None => {
                self.error(format!("`{key}` must be a string"));
                None
            }
        }
    }

    fn boolean(&mut self, key: &str, v: &toml::Value) -> Option<bool> {
        let b = v.as_bool();
        if b.is_none() {
            self.error(format!("`{key}` must be true or false"));
        }
        b
    }

    /// An array of strings; other entries are reported and dropped.
    fn strings(&mut self, key: &str, v: &toml::Value) -> Vec<String> {
        let Some(items) = v.as_array() else {
            self.error(format!("`{key}` must be an array of strings"));
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in items {
            match item.as_str() {
                Some(s) => out.push(s.to_owned()),
                None => self.error(format!("`{key}`: {item} is not a string")),
            }
        }
        out
    }

    /// `["()", "[]"]`: each entry exactly two chars. `None` when `v` isn't an array.
    fn char_pairs(&mut self, key: &str, v: &toml::Value) -> Option<Vec<(char, char)>> {
        if !v.is_array() {
            self.error(format!(
                "`{key}` must be an array of two-character strings such as \"()\""
            ));
            return None;
        }
        let mut out = Vec::new();
        for s in self.strings(key, v) {
            let mut chars = s.chars();
            match (chars.next(), chars.next(), chars.next()) {
                (Some(a), Some(b), None) => out.push((a, b)),
                _ => self.error(format!(
                    "`{key}`: \"{s}\" must be exactly two characters (open and close)"
                )),
            }
        }
        Some(out)
    }

    /// `[{ open = "'", close = "'", not_in = ["string", "comment"] }]`.
    fn auto_pairs(&mut self, key: &str, v: &toml::Value) -> Option<Vec<AutoPair>> {
        let Some(items) = v.as_array() else {
            self.error(format!("`{key}` must be an array of {{ open, close, not_in }} tables"));
            return None;
        };
        let single = |v: Option<&toml::Value>| {
            let mut chars = v?.as_str()?.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let mut out = Vec::new();
        for item in items {
            let Some(t) = item.as_table() else {
                self.error(format!("`{key}`: {item} is not a table"));
                continue;
            };
            if let Some(k) = t.keys().find(|k| !matches!(k.as_str(), "open" | "close" | "not_in")) {
                self.error(format!("`{key}`: unknown key `{k}`"));
            }
            let (Some(open), Some(close)) = (single(t.get("open")), single(t.get("close"))) else {
                self.error(format!(
                    "`{key}`: {item} needs `open` and `close`, each a single character"
                ));
                continue;
            };
            let (mut not_in_string, mut not_in_comment) = (false, false);
            if let Some(not_in) = t.get("not_in") {
                for s in self.strings(&format!("{key}.not_in"), not_in) {
                    match s.as_str() {
                        "string" => not_in_string = true,
                        "comment" => not_in_comment = true,
                        _ => self.error(format!("`{key}.not_in`: \"{s}\" must be \"string\" or \"comment\"")),
                    }
                }
            }
            out.push(AutoPair {
                open,
                close,
                not_in_string,
                not_in_comment,
            });
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A registry id no other test uses.
    fn unique(prefix: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!("{prefix}_{}", N.fetch_add(1, Ordering::Relaxed))
    }

    fn ok(text: &str) -> Manifest {
        let (m, errors) = parse(text, "f");
        assert!(errors.is_empty(), "{errors:?}");
        m.unwrap()
    }

    /// The template is jed's built-in CMake: same editing rules, same detection.
    #[test]
    fn syntax_plugins_template_matches_built_in_cmake() {
        let text = include_str!("../../examples/languages/cmake/language.toml");
        let m = ok(text);
        assert_eq!((m.id.as_str(), m.name.as_str()), ("cmake", "CMake"));
        let want = Language::CMake.config();
        assert_eq!(m.line_comment.as_deref(), want.line_comment);
        assert_eq!(m.pairs, want.pairs);
        assert_eq!(m.brackets, want.brackets);
        assert_eq!(m.surround, want.surround);
        assert_eq!((m.insert_spaces, m.tree_tokens), (want.insert_spaces, want.tree_tokens));
        // Register a copy under another id with nothing claimed, to compare the whole config.
        let mut copy = m.clone();
        copy.id = unique("template");
        copy.extensions.clear();
        copy.filenames.clear();
        let c = &register(copy).unwrap().get().config;
        assert_eq!(c, want);
        // Detection: the same names and extensions (checked against the built-in).
        let root = Path::new("/ws");
        let none = super::super::language::Associations::default();
        for path in ["/ws/CMakeLists.txt", "/ws/a/Foo.cmake", "/ws/x.CMAKE"] {
            assert_eq!(
                super::super::language::detect(Path::new(path), root, "", &none),
                Language::CMake
            );
            let file = Path::new(path).file_name().unwrap().to_str().unwrap();
            let lower = file.to_ascii_lowercase();
            assert!(
                m.filenames.iter().any(|f| f == file) || m.extensions.iter().any(|e| lower.ends_with(&format!(".{e}"))),
                "{path}"
            );
        }
        assert_eq!(
            (m.filenames.clone(), m.extensions.clone()),
            (vec!["CMakeLists.txt".into()], vec!["cmake".into()])
        );
        assert!(m.shebangs.is_empty());
        // The kind lists say what the built-in table says of CMake's node kinds.
        for (kinds, scope) in [
            (&m.comment_kinds, Scope::Comment),
            (&m.string_kinds, Scope::String),
            (&m.code_kinds, Scope::Code),
        ] {
            assert!(!kinds.is_empty());
            for k in kinds {
                assert_eq!(super::super::scope_of(k), Some(scope), "{k}");
            }
        }
    }

    #[test]
    fn defaults() {
        let m = ok("id = \"lua\"");
        assert_eq!((m.name.as_str(), m.symbol.as_str()), ("lua", "tree_sitter_lua"));
        assert_eq!(m.line_comment, None);
        assert_eq!(m.brackets, config::BRACKET_PAIRS);
        assert_eq!(m.pairs, config::BRACKETS);
        assert_eq!(m.surround, config::SURROUND);
        assert!(m.tree_tokens && m.insert_spaces.is_none());
        assert!(m.extensions.is_empty() && m.comment_kinds.is_empty());
        // Registered, the config equals `config(None, BRACKETS, BRACKET_PAIRS, SURROUND)`.
        let mut m = m;
        m.id = unique("test_defaults");
        let c = register(m).unwrap().get();
        let want = config::config(None, &config::BRACKETS, config::BRACKET_PAIRS, config::SURROUND);
        assert_eq!(format!("{:?}", c.config), format!("{want:?}"));
        assert_eq!(c.symbol, "tree_sitter_lua");
        // `-` and `.` in an id become `_` in the default symbol.
        assert_eq!(ok("id = \"my-lang.x\"").symbol, "tree_sitter_my_lang_x");
    }

    #[test]
    fn pair_not_in_values() {
        let (m, errors) = parse(
            r#"
id = "x"
tree_tokens = 1
pairs = [
  { open = "'", close = "'", not_in = ["comment"] },
  { open = '"', close = '"', not_in = ["string"] },
  { open = "`", close = "`", not_in = "string" },
  { open = "<", close = ">", not_in = ["string", 3] },
  { open = "(", close = ")", nope = 1 },
]
"#,
            "f",
        );
        let m = m.unwrap();
        let flags: Vec<_> = m
            .pairs
            .iter()
            .map(|p| (p.open, p.not_in_string, p.not_in_comment))
            .collect();
        // A bad `not_in` (not an array, or a bad entry) is reported and the pair kept with the rest.
        assert_eq!(
            flags,
            [
                ('\'', false, true),
                ('"', true, false),
                ('`', false, false),
                ('<', true, false),
                ('(', false, false)
            ]
        );
        assert!(m.tree_tokens, "a bad `tree_tokens` keeps the default");
        for want in [
            "`tree_tokens` must be true or false",
            "`pairs.not_in` must be an array",
            "`pairs.not_in`: 3 is not a string",
            "`pairs`: unknown key `nope`",
        ] {
            assert!(errors.iter().any(|e| e.contains(want)), "{want}: {errors:?}");
        }
        assert_eq!(errors.len(), 4, "{errors:?}");
    }

    #[test]
    fn every_key() {
        let m = ok(r#"
id = "my-lang"
name = "My Lang"
symbol = "tree_sitter_mine"
line_comment = "--"
brackets = ["()", "<>"]
surround = ["()", "''"]
pairs = [{ open = "'", close = "'", not_in = ["string", "comment"] }, { open = "(", close = ")" }]
insert_spaces = false
tree_tokens = false
comment_kinds = ["note"]
string_kinds = ["text"]
code_kinds = ["interp"]
[files]
extensions = [".Mine", "ml2"]
filenames = ["Minefile"]
shebangs = ["mine"]
"#);
        assert_eq!(
            (m.id.as_str(), m.name.as_str(), m.symbol.as_str()),
            ("my-lang", "My Lang", "tree_sitter_mine")
        );
        assert_eq!(m.line_comment.as_deref(), Some("--"));
        assert_eq!(m.brackets, [('(', ')'), ('<', '>')]);
        assert_eq!(m.surround, [('(', ')'), ('\'', '\'')]);
        assert_eq!(
            m.pairs,
            [
                AutoPair {
                    open: '\'',
                    close: '\'',
                    not_in_string: true,
                    not_in_comment: true
                },
                AutoPair {
                    open: '(',
                    close: ')',
                    not_in_string: false,
                    not_in_comment: false
                },
            ]
        );
        assert_eq!((m.insert_spaces, m.tree_tokens), (Some(false), false));
        assert_eq!(
            (m.comment_kinds, m.string_kinds, m.code_kinds),
            (vec!["note".into()], vec!["text".into()], vec!["interp".into()])
        );
        assert_eq!(
            (m.extensions, m.filenames, m.shebangs),
            (
                vec!["mine".into(), "ml2".into()],
                vec!["Minefile".into()],
                vec!["mine".into()]
            )
        );
    }

    #[test]
    fn bad_values_are_reported_and_the_rest_applies() {
        let (m, errors) = parse(
            r#"
id = "x"
bogus = 1
name = 3
symbol = "1bad"
line_comment = 7
brackets = ["()", "abc", 4]
pairs = [{ open = "ab", close = "b" }, { open = "<", close = ">", not_in = ["str"] }]
insert_spaces = "no"
comment_kinds = "note"
string_kinds = ["s"]
[files]
extensions = ["a"]
shebang = []
"#,
            "languages/x/language.toml",
        );
        let m = m.unwrap();
        assert_eq!(m.id, "x");
        assert_eq!((m.name.as_str(), m.symbol.as_str()), ("x", "tree_sitter_x"));
        assert_eq!(m.brackets, [('(', ')')]);
        assert_eq!(m.pairs.len(), 1);
        assert_eq!(m.pairs[0].open, '<');
        assert_eq!(m.string_kinds, ["s"]);
        assert_eq!(m.extensions, ["a"]);
        assert!(m.comment_kinds.is_empty() && m.insert_spaces.is_none() && m.line_comment.is_none());
        for want in [
            "unknown key `bogus`",
            "`name` must be a string",
            "`symbol`",
            "`line_comment` must be a string",
            "\"abc\" must be exactly two characters",
            "`brackets`: 4 is not a string",
            "`pairs`:",
            "`pairs.not_in`: \"str\"",
            "`insert_spaces` must be true or false",
            "`comment_kinds` must be an array",
            "unknown key `files.shebang`",
        ] {
            assert!(errors.iter().any(|e| e.contains(want)), "{want}: {errors:?}");
        }
        assert!(
            errors.iter().all(|e| e.starts_with("languages/x/language.toml: ")),
            "{errors:?}"
        );
    }

    #[test]
    fn missing_id_and_syntax_errors() {
        let (m, errors) = parse("name = \"X\"\n", "f");
        assert!(m.is_none());
        assert_eq!(errors, ["f: missing `id`"]);
        let (m, errors) = parse("id = \"a b\"\n", "f");
        assert!(m.is_none());
        assert_eq!(errors.len(), 1, "{errors:?}");
        let (m, errors) = parse("id = \"x\"\n\nname = = 1\n", "f");
        assert!(m.is_none());
        assert!(errors[0].starts_with("f:3: "), "{errors:?}");
    }

    #[test]
    fn registry_and_language() {
        let id = unique("test_reg");
        let mut m = ok(&format!("id = \"{id}\"\nname = \"Reg\"\nline_comment = \"--\""));
        m.name = "Reg".into();
        let pid = register(m.clone()).unwrap();
        assert!(register(m).unwrap_err().contains("twice"));
        let l = Language::Plugin(pid);
        assert_eq!((l.id(), l.name()), (id.as_str(), "Reg"));
        assert_eq!(l.config().line_comment, Some("--"));
        assert_eq!(Language::from_id(&id), Some(l));
        assert!(Language::all().contains(&l));
        assert_eq!(Language::all()[..Language::BUILT_IN.len()], Language::BUILT_IN);
        assert!(l.grammar().is_none() && l.highlight_queries().is_empty());
        assert_eq!(PluginId::find(&id), Some(pid));
    }

    #[test]
    fn built_in_ids_are_rejected() {
        let mut m = ok("id = \"rust\"");
        assert!(register(m.clone()).unwrap_err().contains("built-in"));
        m.id = "plaintext".into();
        assert!(register(m).is_err());
        assert_eq!(Language::from_id("rust"), Some(Language::Rust));
    }

    #[test]
    fn scope_of_uses_the_plugins_kinds() {
        let mut m = ok("id = \"x\"");
        m.id = unique("test_scope");
        m.comment_kinds = vec!["text".into(), "string_note".into()];
        m.string_kinds = vec!["quoted".into()];
        m.code_kinds = vec!["string_interp".into(), "ident".into()];
        let l = Language::Plugin(register(m).unwrap());
        assert_eq!(l.scope_of("text"), Some(Scope::Comment));
        assert_eq!(l.scope_of("string_note"), Some(Scope::Comment));
        assert_eq!(l.scope_of("quoted"), Some(Scope::String));
        assert_eq!(l.scope_of("string_interp"), Some(Scope::Code));
        // The name rule still applies to kinds the lists don't mention.
        assert_eq!(l.scope_of("line_comment"), Some(Scope::Comment));
        assert_eq!(l.scope_of("string_literal"), Some(Scope::String));
        assert_eq!(l.scope_of("other"), None);
        // Built-ins are unaffected.
        assert_eq!(Language::Rust.scope_of("text"), None);
    }

    #[test]
    fn load_dir_reads_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
            std::fs::write(dir.path().join(name).join(MANIFEST), text).unwrap();
        };
        write("alpha", "id = \"alpha\"\nname = \"Alpha\"");
        write("beta", "id = \"gamma\"");
        write("delta", "name = \"no id\"");
        write("bad", "id = = ");
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        std::fs::write(dir.path().join("stray.txt"), "x").unwrap();
        let (ms, errors) = load_dir(dir.path());
        assert_eq!(ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["alpha"]);
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors.iter().any(|e| e.starts_with("languages/bad/language.toml:1: ")));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("languages/beta/") && e.contains("directory is \"beta\""))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("languages/delta/") && e.contains("missing `id`"))
        );
        assert_eq!(load_dir(&dir.path().join("nope")), (Vec::new(), Vec::new()));
    }

    #[test]
    fn load_and_register_reports_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let id = unique("test_load");
        for (name, text) in [
            ("rust", "id = \"rust\"".to_owned()),
            (
                id.as_str(),
                format!("id = \"{id}\"\nname = \"Loaded\"\nline_comment = \";\""),
            ),
        ] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
            std::fs::write(dir.path().join(name).join(MANIFEST), text).unwrap();
        }
        let errors = load_and_register(dir.path());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with("languages/rust/language.toml: ") && errors[0].contains("built-in"),
            "{errors:?}"
        );
        assert_eq!(Language::from_id("rust"), Some(Language::Rust));
        let l = Language::from_id(&id).expect("registered");
        assert!(matches!(l, Language::Plugin(_)));
        assert_eq!((l.name(), l.config().line_comment), ("Loaded", Some(";")));
        // A `.workspace` association can name the plugin's id.
        let map = std::collections::BTreeMap::from([("*.zz".to_owned(), id.clone())]);
        let (assoc, errs) = super::super::Associations::new(&map);
        assert!(errs.is_empty(), "{errs:?}");
        let root = Path::new("/w");
        assert_eq!(super::super::detect(&root.join("a.zz"), root, "", &assoc), l);
    }
}
