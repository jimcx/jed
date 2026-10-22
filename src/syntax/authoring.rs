//! The tools for writing a syntax plugin: `jed --new-language <id>` and `jed --check-language
//! <id> [--sample <file>]`. Both run in `main` before the UI and return the text to print, so
//! tests check the text; the check shares `build::build`, `PluginId::load` and the query code
//! with the app. Blocking: call from `main` (or a test), never the UI thread.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use super::build::{self, Outcome, Toolchain};
use super::highlight::{ALIASES, KEYS, highlight_for};
use super::plugin::{self, LIBRARY, MANIFEST, Manifest};
use super::{Language, Scope};
use crate::theme::{ANSI_RESET, Theme};

/// The commented template (`examples/languages/cmake/language.toml`): jed's CMake as a plugin.
const TEMPLATE: &str = include_str!("../../examples/languages/cmake/language.toml");

/// Replaces the template's line starting with `prefix`.
fn set_line(text: &str, prefix: &str, new: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(if line.starts_with(prefix) { new } else { line });
        out.push('\n');
    }
    out
}

/// The template made into a new language's manifest: `id`, `name`, `symbol` and `[files]` filled
/// in, CMake's own node kinds and comment character cleared.
fn manifest_text(id: &str) -> String {
    let mut name = id.to_string();
    if let Some(first) = name.get(..1) {
        name = first.to_uppercase() + &name[1..];
    }
    let symbol = format!("tree_sitter_{}", id.replace(['-', '.'], "_"));
    let mut text = TEMPLATE.replacen(
        "# A jed syntax plugin: jed's built-in CMake support, redone as a plugin. Copy this directory to",
        "# A jed syntax plugin, started by `jed --new-language`. Copy this directory to",
        1,
    );
    for (prefix, new) in [
        ("id = ", format!("id = \"{id}\"")),
        ("name = ", format!("name = \"{name}\"")),
        ("symbol = ", format!("symbol = \"{symbol}\"")),
        (
            "line_comment = ",
            "# line_comment = \"#\"   # set this to the language's comment start".into(),
        ),
        ("comment_kinds = ", "comment_kinds = []".into()),
        ("string_kinds = ", "string_kinds = []".into()),
        ("code_kinds = ", "code_kinds = []".into()),
        (
            "extensions = ",
            format!("extensions = [\"{id}\"]   # edit: the file extensions"),
        ),
        ("filenames = ", "filenames = []".into()),
    ] {
        text = set_line(&text, prefix, &new);
    }
    text
}

/// `jed --new-language <id>`: creates `<languages_dir>/<id>/` with `language.toml`, an empty
/// `src/` and an empty `queries/`, and returns the text to print (the next steps). Never
/// overwrites: an existing directory is an error.
pub fn new_language(languages_dir: &Path, id: &str) -> Result<String, String> {
    if !id.starts_with(|c: char| c.is_ascii_alphanumeric()) || !plugin::valid_id(id) {
        return Err(format!(
            "`{id}` is not a language id: start with a letter or digit, then letters, digits, `_`, `-` or `.`"
        ));
    }
    if Language::BUILT_IN.iter().any(|l| l.id() == id) {
        return Err(format!("`{id}` is a built-in language's id; choose another"));
    }
    let dir = languages_dir.join(id);
    if dir.exists() {
        return Err(format!("{} already exists; not overwriting it", dir.display()));
    }
    let io = |e: std::io::Error| format!("{}: {e}", dir.display());
    std::fs::create_dir_all(dir.join("src")).map_err(io)?;
    std::fs::create_dir_all(dir.join("queries")).map_err(io)?;
    std::fs::write(dir.join(MANIFEST), manifest_text(id)).map_err(io)?;
    let d = dir.display();
    Ok(format!(
        "Created {d}\n\
         \n\
         Next:\n\
         \x20 1. Copy the grammar repository's src/ (parser.c, scanner.c if it has one, tree_sitter/*.h)\n\
         \x20    into {d}/src/ and its queries/highlights.scm into {d}/queries/\n\
         \x20 2. Edit {d}/{MANIFEST}: the file extensions, `line_comment`, `brackets`, `pairs`;\n\
         \x20    `symbol` if the grammar's C function is not tree_sitter_{sym}\n\
         \x20 3. Run: jed --check-language {id}\n",
        sym = id.replace(['-', '.'], "_"),
    ))
}

/// Puts `LINE:` after the file name in a manifest error that names a key, when the key is found
/// in `text` (parse errors from TOML syntax already carry their line).
fn with_line(error: &str, file: &str, text: &str) -> String {
    let rest = error.strip_prefix(file).and_then(|r| r.strip_prefix(": "));
    let Some(rest) = rest else { return error.to_string() };
    // `files.extensions` is `extensions` under `[files]`; `pairs.not_in` is inside `pairs`'s
    // inline tables, so its line is `pairs`'s.
    let Some(path) = rest.split('`').nth(1) else {
        return error.to_string();
    };
    let (table, key) = match path.split_once('.') {
        Some(("files", k)) => ("files", k),
        Some((k, _)) => ("", k),
        None => ("", path),
    };
    if key.is_empty() {
        return error.to_string();
    }
    let mut current = "";
    let line = text.lines().position(|l| {
        let l = l.trim_start();
        if let Some(header) = l.strip_prefix('[') {
            current = header.split(']').next().unwrap_or("").trim();
            // An unknown table (`[foo]`) is found at its header.
            return table.is_empty() && current == key;
        }
        current == table
            && l.strip_prefix(key)
                .or_else(|| l.strip_prefix('"').and_then(|l| l.strip_prefix(key)))
                .is_some_and(|r| r.starts_with([' ', '=', '"']))
    });
    match line {
        Some(n) => format!("{file}:{}: {rest}", n + 1),
        None => error.to_string(),
    }
}

/// Theme keys a capture name could be renamed to: those sharing its first or last dotted segment.
fn suggestions(capture: &str) -> Vec<&'static str> {
    let segs = |s: &str| {
        let mut it = s.split('.');
        let first = it.next().unwrap_or("").to_string();
        let last = s.rsplit('.').next().unwrap_or("").to_string();
        (first, last)
    };
    let (first, last) = segs(capture);
    let near = |name: &str| {
        let (f, l) = segs(name);
        f == first || l == last || f == last || l == first
    };
    let mut found: Vec<&'static str> = KEYS.iter().copied().filter(|k| near(k)).collect();
    for (alias, key) in ALIASES {
        if near(alias) && !found.contains(key) {
            found.push(key);
        }
    }
    found
}

/// Appends `text`'s lines indented under a step.
fn indent(out: &mut String, text: &str) {
    for line in text.lines() {
        let _ = writeln!(out, "      {line}");
    }
}

/// `jed --check-language <id> [--sample <file>]`: the report to print and whether the language
/// is usable (the manifest, the build, the library and the query passed, and the sample was
/// read). Registers the plugin, so it runs once per process.
/// Blocking: runs the C compiler.
pub fn check_language(languages_dir: &Path, id: &str, sample: Option<&Path>, tools: &Toolchain) -> (String, bool) {
    let mut out = String::new();
    let mut ok;
    macro_rules! pass {
        ($($t:tt)*) => {{ let _ = writeln!(out, "  ✓ {}", format!($($t)*)); }};
    }
    macro_rules! fail {
        ($($t:tt)*) => {{ let _ = writeln!(out, "  ✗ {}", format!($($t)*)); }};
    }
    let dir = languages_dir.join(id);
    let _ = writeln!(out, "Checking language `{id}` in {}", dir.display());

    // 1. The manifest.
    let _ = writeln!(out, "Manifest");
    let file = format!("{id}/{MANIFEST}");
    let text = match std::fs::read_to_string(dir.join(MANIFEST)) {
        Ok(t) => t,
        Err(e) => {
            fail!("{file}: {e} (`jed --new-language {id}` creates it)");
            return (out, false);
        }
    };
    let (manifest, errors) = plugin::parse(&text, &file);
    ok = errors.is_empty();
    for e in &errors {
        fail!("{}", with_line(e, &file, &text));
    }
    let Some(manifest) = manifest else { return (out, false) };
    if manifest.id != id {
        fail!("{file}: `id` is \"{}\" but the directory is \"{id}\"", manifest.id);
        return (out, false);
    }
    let m = Manifest {
        dir: dir.clone(),
        ..manifest
    };
    let (name, symbol) = (m.name.clone(), m.symbol.clone());
    let plugin = match plugin::register(m) {
        Ok(p) => p,
        Err(e) => {
            fail!("{file}: {e}");
            return (out, false);
        }
    };
    if errors.is_empty() {
        pass!("{file}: `{name}`, symbol `{symbol}`");
    }

    // 2. The build.
    let _ = writeln!(out, "Build");
    let built = build::build(&dir, tools, false);
    let log = built.log.join("\n");
    match built.outcome {
        Outcome::NoSources if plugin.get().loadable() => pass!("no src/parser.c: using the prebuilt {LIBRARY}"),
        Outcome::NoSources => {
            fail!("no src/parser.c and no prebuilt {LIBRARY} in {}", dir.display());
            return (out, false);
        }
        Outcome::Fresh => pass!("{LIBRARY} is up to date"),
        Outcome::Built => {
            pass!("built {LIBRARY}");
            indent(&mut out, &log);
        }
        Outcome::Failed(why) => {
            fail!("build failed: {why}");
            indent(&mut out, &log);
            return (out, false);
        }
    }

    // 3. The library, its symbol and ABI; 4. the query.
    let _ = writeln!(out, "Load");
    if let Err(e) = plugin.load() {
        if e.contains("highlight query") {
            pass!("loaded {LIBRARY}");
            let _ = writeln!(out, "Query");
            fail!("{e}");
        } else {
            fail!("{e}");
        }
        return (out, false);
    }
    let grammar = plugin.get().grammar.get().expect("loaded").clone();
    pass!("loaded {LIBRARY} (grammar ABI {})", grammar.abi_version());
    let _ = writeln!(out, "Query");
    pass!("queries compiled");

    // 5. Captures with no theme key.
    let source = plugin.get().query_source().map(|(s, _)| s).unwrap_or_default();
    if let Ok(query) = tree_sitter::Query::new(&grammar, &source) {
        let unmapped: BTreeSet<&str> = query
            .capture_names()
            .iter()
            .copied()
            .filter(|n| !n.starts_with('_') && highlight_for(n).is_none())
            .collect();
        if unmapped.is_empty() {
            pass!("every capture maps to a theme key");
        }
        for capture in unmapped {
            let near = suggestions(capture);
            let hint = if near.is_empty() {
                "no similar key".to_string()
            } else {
                format!("could be: {}", near.join(", "))
            };
            let _ = writeln!(
                out,
                "  - note: @{capture} maps to no theme key and paints nothing ({hint})"
            );
        }
    }

    // 6. Comment and string node kinds.
    let _ = writeln!(out, "Node kinds");
    let language = Language::Plugin(plugin);
    let mut comments = BTreeSet::new();
    let mut strings = BTreeSet::new();
    for id in 0..grammar.node_kind_count() as u16 {
        let Some(kind) = grammar.node_kind_for_id(id) else {
            continue;
        };
        if !grammar.node_kind_is_named(id) || kind.starts_with('_') {
            continue;
        }
        match language.scope_of(kind) {
            Some(Scope::Comment) => comments.insert(kind),
            Some(Scope::String) => strings.insert(kind),
            _ => false,
        };
    }
    let list = |s: &BTreeSet<&str>| {
        if s.is_empty() {
            "(none)".to_string()
        } else {
            s.iter().copied().collect::<Vec<_>>().join(", ")
        }
    };
    let _ = writeln!(
        out,
        "  comments (auto-close and indent treat these as comments): {}",
        list(&comments)
    );
    let _ = writeln!(out, "  strings (and these as strings): {}", list(&strings));
    let _ = writeln!(
        out,
        "  fix with comment_kinds / string_kinds / code_kinds in {MANIFEST}"
    );

    // 7. The sample.
    if let Some(path) = sample {
        let _ = writeln!(out, "Sample {}", path.display());
        match std::fs::read_to_string(path) {
            Ok(text) => {
                out.push_str(&colour(language, &text));
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            Err(e) => {
                ok = false;
                fail!("{}: {e}", path.display());
            }
        }
    }
    let _ = writeln!(out, "{}", if ok { "OK" } else { "FAILED" });
    (out, ok)
}

/// `text` with each highlighted span in its Dark+ colour as ANSI escapes.
fn colour(language: Language, text: &str) -> String {
    let theme = Theme::dark_plus(true);
    let mut out = String::new();
    let mut at = 0;
    for span in super::highlight_text(language, text) {
        let (start, end) = (span.start.0, span.end.0);
        out.push_str(&text[at..start]);
        let on = theme.ansi(KEYS[span.highlight.index()]);
        if on.is_empty() {
            out.push_str(&text[start..end]);
        } else {
            let _ = write!(out, "{on}{}{ANSI_RESET}", &text[start..end]);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::plugin::test_lib;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique(prefix: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!("{prefix}_{}", N.fetch_add(1, Ordering::Relaxed))
    }

    fn cmake() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/languages/cmake")
    }

    /// A plugin made as the docs say: `new_language`, the CMake grammar's `src/` and query copied
    /// in, `symbol` set. `None` (with a note) without a C compiler.
    fn made(parent: &Path, id: &str) -> Option<PathBuf> {
        test_lib::library(false)?;
        new_language(parent, id).unwrap();
        let dir = parent.join(id);
        for f in [
            "parser.c",
            "scanner.c",
            "tree_sitter/alloc.h",
            "tree_sitter/array.h",
            "tree_sitter/parser.h",
        ] {
            let to = dir.join("src").join(f);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(cmake().join("src").join(f), to).unwrap();
        }
        std::fs::copy(
            cmake().join("queries/highlights.scm"),
            dir.join("queries/highlights.scm"),
        )
        .unwrap();
        edit_manifest(&dir, |t| set_line(t, "symbol = ", "symbol = \"tree_sitter_cmake\""));
        Some(dir)
    }

    fn edit_manifest(dir: &Path, f: impl FnOnce(&str) -> String) {
        let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
        std::fs::write(dir.join(MANIFEST), f(&text)).unwrap();
    }

    fn check(parent: &Path, id: &str) -> (String, bool) {
        check_language(parent, id, None, &Toolchain::from_env())
    }

    #[test]
    fn syntax_plugins_new_language_writes_the_template() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring-new");
        let text = new_language(tmp.path(), &id).unwrap();
        let dir = tmp.path().join(&id);
        assert!(dir.join("src").is_dir() && dir.join("queries").is_dir());
        for step in [
            "src/",
            "queries/highlights.scm",
            MANIFEST,
            &format!("jed --check-language {id}"),
        ] {
            assert!(text.contains(step), "{step}: {text}");
        }
        let (m, errors) = plugin::parse(&std::fs::read_to_string(dir.join(MANIFEST)).unwrap(), "f");
        assert!(errors.is_empty(), "{errors:?}");
        let m = m.unwrap();
        assert_eq!(m.id, id);
        assert!(m.name.starts_with("Authoring-new"), "{}", m.name);
        assert_eq!(m.symbol, format!("tree_sitter_{}", id.replace('-', "_")));
        assert_eq!(m.extensions, std::slice::from_ref(&id));
        assert!(m.filenames.is_empty() && m.shebangs.is_empty() && m.line_comment.is_none());
        assert!(m.comment_kinds.is_empty() && m.string_kinds.is_empty() && m.code_kinds.is_empty());
        // The template's explanations stay.
        assert!(
            std::fs::read_to_string(dir.join(MANIFEST))
                .unwrap()
                .contains("VS Code: `autoClosingPairs`")
        );
    }

    #[test]
    fn syntax_plugins_new_language_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_twice");
        new_language(tmp.path(), &id).unwrap();
        assert!(new_language(tmp.path(), &id).unwrap_err().contains("already exists"));
        assert!(new_language(tmp.path(), "rust").unwrap_err().contains("built-in"));
        for bad in ["", "..", "a/b", ".hidden", "sp ace"] {
            assert!(
                new_language(tmp.path(), bad).unwrap_err().contains("not a language id"),
                "{bad}"
            );
        }
    }

    #[test]
    fn syntax_plugins_check_reports_manifest_errors_with_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_manifest");
        new_language(tmp.path(), &id).unwrap();
        let dir = tmp.path().join(&id);
        edit_manifest(&dir, |t| format!("{t}\nbogus_key = 1\n"));
        edit_manifest(&dir, |t| t.replace("tree_tokens = true", "tree_tokens = \"yes\""));
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok, "{out}");
        let lines: Vec<_> = std::fs::read_to_string(dir.join(MANIFEST))
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        let at = |k: &str| lines.iter().position(|l| l.starts_with(k)).unwrap() + 1;
        assert!(
            out.contains(&format!(
                "{id}/{MANIFEST}:{}: unknown key `files.bogus_key`",
                at("bogus_key")
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!("{MANIFEST}:{}: `tree_tokens`", at("tree_tokens"))),
            "{out}"
        );
        // A TOML syntax error carries its own line.
        edit_manifest(&dir, |t| format!("{t}\nname = = 1\n"));
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok && out.contains(&format!("{MANIFEST}:")), "{out}");
        // No manifest at all.
        let (out, ok) = check(tmp.path(), "authoring_missing");
        assert!(!ok && out.contains("--new-language"), "{out}");
    }

    #[test]
    fn syntax_plugins_manifest_error_lines_follow_tables() {
        let f = "x/language.toml";
        let text = "# extensions = [] mentioned in a comment\n\
                    id = \"x\"\n\
                    extensions = 1\n\
                    pairs = [\n\
                    \x20   { open = \"(\", close = \")\", not_in = [\"nowhere\"] },\n\
                    ]\n\
                    [files]\n\
                    # extensions: a comment\n\
                    extensions = 2\n\
                    [bogus]\n";
        let at = |e: &str| with_line(&format!("{f}: {e}"), f, text);
        // `[files]`'s key, not the top-level one or a comment.
        assert_eq!(
            at("`files.extensions` must be an array of strings"),
            format!("{f}:9: `files.extensions` must be an array of strings")
        );
        // A top-level key, not the one under `[files]`.
        assert_eq!(
            at("unknown key `extensions`"),
            format!("{f}:3: unknown key `extensions`")
        );
        // Inside `pairs`'s inline tables: `pairs`'s line.
        assert!(at("`pairs.not_in`: \"nowhere\" must be").starts_with(&format!("{f}:4: ")));
        // An unknown table: its header.
        assert!(at("unknown key `bogus`").starts_with(&format!("{f}:10: ")));
        // Not found, or no key named: unchanged.
        assert_eq!(at("unknown key `absent`"), format!("{f}: unknown key `absent`"));
        assert_eq!(at("no key here"), format!("{f}: no key here"));
        let toml_error = format!("{f}:3: expected a value");
        assert_eq!(with_line(&toml_error, f, text), toml_error);
    }

    /// The done-when flow: `--new-language`, the grammar's `src/` and `highlights.scm` copied in,
    /// `extensions`, `line_comment`, `brackets` and `pairs` set by editing the generated lines.
    #[test]
    fn syntax_plugins_check_passes_the_new_language_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_flow");
        let Some(dir) = made(tmp.path(), &id) else { return };
        let ext = format!("{id}x");
        edit_manifest(&dir, |t| {
            let t = set_line(t, "# line_comment = ", "line_comment = \"--\"");
            let t = set_line(&t, "extensions = ", &format!("extensions = [\"{ext}\"]"));
            let t = set_line(&t, "brackets = ", "brackets = [\"{}\", \"[]\", \"()\"]");
            t.replace(
                "    { open = \"\\\"\", close = \"\\\"\", not_in = [\"string\"] },",
                "    { open = \"\\\"\", close = \"\\\"\", not_in = [\"string\"] },\n    { open = \"'\", close = \"'\", not_in = [\"string\", \"comment\"] },",
            )
        });
        let (out, ok) = check(tmp.path(), &id);
        assert!(ok, "{out}");
        assert!(!out.contains('✗'), "{out}");
        assert!(out.trim_end().ends_with("OK"), "{out}");
        let pid = plugin::PluginId::find(&id).unwrap();
        let plugin = pid.get();
        assert_eq!(plugin.config.line_comment, Some("--"));
        assert!(
            plugin.config.pairs.iter().any(|p| p.open == '\'' && p.not_in_comment),
            "{out}"
        );
        assert!(pid.loaded());
        let lang = crate::syntax::detect_path(&tmp.path().join(format!("a.{ext}")), tmp.path(), &Default::default());
        assert_eq!(lang, Some(Language::Plugin(pid)));
    }

    /// A manifest error alone fails the check: the plugin is otherwise good and every later step
    /// passes.
    #[test]
    fn syntax_plugins_check_fails_on_a_manifest_error_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_only_manifest");
        let Some(dir) = made(tmp.path(), &id) else { return };
        edit_manifest(&dir, |t| format!("bogus_top_key = 1\n{t}"));
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok, "{out}");
        assert!(out.contains("unknown key `bogus_top_key`"), "{out}");
        assert!(
            out.contains("queries compiled"),
            "the later steps ran and passed: {out}"
        );
        assert!(
            out.trim_end().ends_with("FAILED") && !out.trim_end().ends_with("OK"),
            "{out}"
        );
    }

    #[test]
    fn syntax_plugins_check_reports_a_wrong_id_and_a_built_in_id() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_id");
        new_language(tmp.path(), &id).unwrap();
        edit_manifest(&tmp.path().join(&id), |t| {
            t.replacen(&format!("id = \"{id}\""), "id = \"other\"", 1)
        });
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok && out.contains("the directory is"), "{out}");
        let dir = tmp.path().join("rust");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), "id = \"rust\"\n").unwrap();
        let (out, ok) = check(tmp.path(), "rust");
        assert!(!ok && out.contains("built-in"), "{out}");
    }

    #[test]
    fn syntax_plugins_check_without_sources_or_library() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_empty");
        new_language(tmp.path(), &id).unwrap();
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok && out.contains("no src/parser.c and no prebuilt"), "{out}");
    }

    #[test]
    fn syntax_plugins_check_passes_a_good_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_good");
        let Some(dir) = made(tmp.path(), &id) else { return };
        // A capture jed has no key for.
        let q = dir.join("queries/highlights.scm");
        let query = std::fs::read_to_string(&q).unwrap();
        std::fs::write(
            &q,
            format!("{query}\n(line_comment) @foo.string\n(bracket_comment) @_private\n"),
        )
        .unwrap();
        let sample = tmp.path().join("sample.txt");
        std::fs::write(&sample, "if(X)\n  message(\"hi\") # note\nendif()\n").unwrap();
        let (out, ok) = check_language(tmp.path(), &id, Some(&sample), &Toolchain::from_env());
        assert!(ok, "{out}");
        assert!(out.contains("built parser"), "{out}");
        assert!(
            out.contains("cc -shared") || out.contains("-shared"),
            "the command is shown: {out}"
        );
        assert!(out.contains("queries compiled"), "{out}");
        assert!(out.contains("@foo.string maps to no theme key"), "{out}");
        assert!(!out.contains("@_private"), "{out}");
        assert!(out.contains("could be: string"), "{out}");
        assert!(
            out.contains("comments") && out.contains("bracket_comment") && out.contains("line_comment"),
            "{out}"
        );
        assert!(out.contains("quoted_argument"), "{out}");
        // The sample is coloured: `if` is a control keyword, in Dark+'s colour.
        let on = Theme::dark_plus(true).ansi("keyword.control");
        assert!(
            !on.is_empty() && out.contains(&format!("{on}if{ANSI_RESET}")),
            "{out:?}"
        );
        assert!(out.trim_end().ends_with("OK"), "{out}");
        // A sample that can't be read fails the check.
        let id = unique("authoring_good");
        made(tmp.path(), &id).unwrap();
        let (out, ok) = check_language(tmp.path(), &id, Some(&tmp.path().join("nope")), &Toolchain::from_env());
        assert!(!ok && out.contains("nope"), "{out}");
    }

    #[test]
    fn syntax_plugins_check_shows_a_compile_error() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_cc");
        let Some(dir) = made(tmp.path(), &id) else { return };
        let parser = dir.join("src/parser.c");
        let text = std::fs::read_to_string(&parser).unwrap();
        std::fs::write(&parser, format!("{text}\nint broken( {{ nonsense;\n")).unwrap();
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok, "{out}");
        assert!(out.contains("build failed"), "{out}");
        assert!(
            out.contains("parser.c") && out.contains("error"),
            "the compiler's output: {out}"
        );
    }

    #[test]
    fn syntax_plugins_check_reports_a_missing_symbol() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_sym");
        let Some(dir) = made(tmp.path(), &id) else { return };
        edit_manifest(&dir, |t| t.replace("tree_sitter_cmake", "tree_sitter_nothing"));
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok && out.contains("no symbol `tree_sitter_nothing`"), "{out}");
    }

    #[test]
    fn syntax_plugins_check_reports_an_old_abi() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_abi");
        let Some(old) = test_lib::library(true) else { return };
        test_lib::plugin_dir(tmp.path(), &id, &id, "tree_sitter_cmake", old, "");
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok, "{out}");
        assert!(
            out.contains("prebuilt") && out.contains("ABI version 12 is not supported"),
            "{out}"
        );
        // The range jed accepts, and what to do.
        let range = format!(
            "jed accepts {} to {}",
            tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
            tree_sitter::LANGUAGE_VERSION
        );
        assert!(out.contains(&range) && out.contains("regenerate"), "{out}");
    }

    #[test]
    fn syntax_plugins_check_reports_a_broken_query_with_its_position() {
        let tmp = tempfile::tempdir().unwrap();
        let id = unique("authoring_query");
        let Some(dir) = made(tmp.path(), &id) else { return };
        let q = dir.join("queries/highlights.scm");
        let query = std::fs::read_to_string(&q).unwrap();
        let row = query.matches('\n').count() + 2;
        std::fs::write(&q, format!("{query}\n(no_such_node) @keyword\n")).unwrap();
        let (out, ok) = check(tmp.path(), &id);
        assert!(!ok, "{out}");
        assert!(out.contains("highlights.scm: highlight query, line"), "{out}");
        assert!(out.contains(&format!("line {row} column 2")), "{out}");
    }
}
