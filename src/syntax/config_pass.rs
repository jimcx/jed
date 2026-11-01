//! The settings pass over a TOML tree for jed's own config files (`settings.toml`, `.workspace`):
//! which bytes of a range are unknown or wrong, by `config::schema`. The TOML query already
//! colours keys, table names and values; `highlight::compute` paints `invalid` over what this
//! returns. Unknown table headers, unknown keys (and every key outside a table but a table's own,
//! like `settings.tab_width = 4`) and values the key's `ValueKind` rejects are reported. A
//! sub-table is known when it is a key's: anything goes under an `Any` key
//! (`[lsp.python.init_options]`, deeper too), a `StrTable` key's (`[tasks.build.env]`) holds
//! strings; `[lsp]` / `[tasks]` alone are accepted. The values of unknown keys and the contents of
//! unknown tables are left alone, and so is any node the grammar flagged as an error.
//! O(the range): the cursor jumps to the first child at the range's start at each level.

use ropey::Rope;
use tree_sitter::{Node, Tree};

use crate::config::schema::{self, ConfigFile, TableSchema, ValueKind};

/// A value longer than this isn't checked (it is parsed as TOML to be).
const MAX_VALUE_BYTES: usize = 64 * 1024;

/// The byte ranges overlapping `start..end` that `file`'s schema rejects (sorted by position;
/// they may reach past the range's ends). `keybindings.toml` has no pass yet: none.
pub fn invalid_ranges(file: ConfigFile, tree: &Tree, rope: &Rope, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if file == ConfigFile::Keybindings {
        return out;
    }
    let mut cur = tree.root_node().walk();
    if cur.goto_first_child_for_byte(start).is_none() {
        return out;
    }
    loop {
        let n = cur.node();
        if n.start_byte() >= end {
            break;
        }
        if n.end_byte() > start {
            match n.kind() {
                "pair" => check_pair(file, None, n, rope, &mut out),
                "table" | "table_array_element" => table(file, n, rope, start, end, &mut out),
                _ => {}
            }
        }
        if !cur.goto_next_sibling() {
            break;
        }
    }
    out
}

fn is_key(n: Node) -> bool {
    matches!(n.kind(), "bare_key" | "quoted_key" | "dotted_key")
}

/// The segments of a (possibly dotted) key, in order.
fn segments(key: Node, rope: &Rope, out: &mut Vec<String>) {
    if key.kind() == "dotted_key" {
        let mut c = key.walk();
        for k in key.named_children(&mut c).filter(|k| is_key(*k)) {
            segments(k, rope, out);
        }
    } else {
        let text = rope.slice(key.start_byte()..key.end_byte()).to_string();
        out.push(if key.kind() == "quoted_key" {
            text.trim_matches(['"', '\'']).to_string()
        } else {
            text
        });
    }
}

fn table(file: ConfigFile, n: Node, rope: &Rope, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let mut c = n.walk();
    let Some(key) = n.named_children(&mut c).find(|k| is_key(*k)) else {
        return;
    };
    let mut path = Vec::new();
    segments(key, rope, &mut path);
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let array = n.kind() == "table_array_element";
    let schema = match header(file, &path, array) {
        Header::Known(schema) => Some(schema),
        Header::Strings => None,
        Header::Free => return,
        Header::Unknown => {
            if key.end_byte() > start && key.start_byte() < end {
                out.push((key.start_byte(), key.end_byte()));
            }
            return;
        }
    };
    let mut cur = n.walk();
    if cur.goto_first_child_for_byte(start).is_none() {
        return;
    }
    loop {
        let p = cur.node();
        if p.start_byte() >= end {
            break;
        }
        if p.kind() == "pair" && p.end_byte() > start {
            match schema {
                Some(schema) => check_pair(file, Some(schema), p, rope, out),
                None => check_string_pair(p, out),
            }
        }
        if !cur.goto_next_sibling() {
            break;
        }
    }
}

/// What a table header names, as the files' parsers take it.
enum Header {
    /// A table of the schema.
    Known(&'static TableSchema),
    /// The sub-table a `StrTable` key (`[files.associations]`, `[tasks.build.env]`): every value
    /// a string.
    Strings,
    /// Not checked: inside an `Any` key (`[lsp.python.init_options]`, at any depth), or a prefix
    /// of a pattern (`[lsp]`, `[tasks]`: the parsers take an empty one).
    Free,
    Unknown,
}

fn header(file: ConfigFile, path: &[&str], array: bool) -> Header {
    if let Some(t) = schema::table_at(file, path) {
        return if t.array == array {
            Header::Known(t)
        } else {
            Header::Unknown
        };
    }
    // The longest known table above, and the key of it the header goes into.
    for i in (1..path.len()).rev() {
        if let Some(t) = schema::table_at(file, &path[..i]).filter(|t| !t.array) {
            return match t.key(path[i]).map(|k| k.kind) {
                Some(ValueKind::Any) => Header::Free,
                Some(ValueKind::StrTable) if path.len() == i + 1 && !array => Header::Strings,
                _ => Header::Unknown,
            };
        }
    }
    let prefix = |t: &TableSchema| {
        let pat: Vec<&str> = t.pattern.split('.').collect();
        !t.array && pat.len() > path.len() && pat.iter().zip(path).all(|(p, g)| *p == "*" || p == g)
    };
    if !array && !path.is_empty() && schema::tables(file).iter().any(prefix) {
        Header::Free
    } else {
        Header::Unknown
    }
}

/// A pair in a `Header::Strings` table: its value must be a string; a dotted key would make a
/// table of it.
fn check_string_pair(pair: Node, out: &mut Vec<(usize, usize)>) {
    if pair.has_error() || pair.is_missing() {
        return;
    }
    let mut c = pair.walk();
    let mut named = pair.named_children(&mut c).filter(|n| n.kind() != "comment");
    let (Some(key), Some(value)) = (named.next(), named.last()) else {
        return;
    };
    if key.kind() == "dotted_key" {
        out.push((key.start_byte(), key.end_byte()));
    } else if is_key(key) && value.kind() != "string" {
        out.push((value.start_byte(), value.end_byte()));
    }
}

/// `schema`: the enclosing table (`None`: the root, whose only keys are `file`'s tables, written
/// as `settings.tab_width = 4` or `window = { … }`: not checked).
fn check_pair(file: ConfigFile, schema: Option<&TableSchema>, pair: Node, rope: &Rope, out: &mut Vec<(usize, usize)>) {
    if pair.has_error() || pair.is_missing() {
        return;
    }
    let mut c = pair.walk();
    let mut named = pair.named_children(&mut c).filter(|n| n.kind() != "comment");
    let (Some(key), Some(value)) = (named.next(), named.last()) else {
        return;
    };
    if !is_key(key) {
        return;
    }
    let mut first = key;
    while first.kind() == "dotted_key" {
        let mut kc = first.walk();
        match first.named_children(&mut kc).find(|k| is_key(*k)) {
            Some(k) => first = k,
            None => return,
        }
    }
    let mut name = Vec::new();
    segments(first, rope, &mut name);
    let Some(known) = schema.and_then(|s| s.key(&name[0])) else {
        let table = |t: &TableSchema| t.pattern.split('.').next() == Some(name[0].as_str());
        if schema.is_none() && schema::tables(file).iter().any(table) {
            return;
        }
        out.push((key.start_byte(), key.end_byte()));
        return;
    };
    if key.kind() == "dotted_key" || value.byte_range().len() > MAX_VALUE_BYTES {
        return;
    }
    let text = rope.slice(value.start_byte()..value.end_byte()).to_string();
    if let Ok(mut t) = toml::from_str::<toml::Table>(&format!("v = {text}"))
        && let Some(v) = t.remove("v")
        && schema::check(known.kind, &v).is_err()
    {
        out.push((value.start_byte(), value.end_byte()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    fn tree_of(text: &str) -> Tree {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&Language::Toml.grammar().unwrap()).unwrap();
        parser.parse(text, None).unwrap()
    }

    /// The texts `invalid_ranges` flags in `text`.
    fn flagged(file: ConfigFile, text: &str) -> Vec<String> {
        let rope = Rope::from_str(text);
        invalid_ranges(file, &tree_of(text), &rope, 0, text.len())
            .into_iter()
            .map(|(a, b)| text[a..b].to_string())
            .collect()
    }

    const SETTINGS: &str = "top = 1\n[settings]\ntab_width = 4\nbogus = 1\nrender_whitespace = \"nope\"\n\
        word_wrap = \"on\"\nhex_bytes_per_line = 12\ninsert_spaces = 3\nhex_bytes_per_line.x = 1\n\
        [lsp.python]\ncommand = \"pyright\"\n[nonsense]\nx = 1\n[[window]]\nx = 1\n[window]\nx = 1\n";

    #[test]
    fn settings_toml_flags_unknown_keys_and_bad_values() {
        let got = flagged(ConfigFile::Settings, SETTINGS);
        for want in ["top", "bogus", "\"nope\"", "12", "3", "nonsense", "window"] {
            assert!(got.iter().any(|g| g == want), "{want} in {got:?}");
        }
        assert!(!got.iter().any(|g| g == "4" || g == "\"on\""), "{got:?}");
        assert!(!got.iter().any(|g| g == "lsp.python" || g == "settings"), "{got:?}");
    }

    #[test]
    fn hex_bytes_per_line_must_be_a_power_of_two() {
        let bad = flagged(ConfigFile::Settings, "[settings]\nhex_bytes_per_line = 12\n");
        let ok = flagged(ConfigFile::Settings, "[settings]\nhex_bytes_per_line = 16\n");
        assert_eq!((bad, ok), (vec!["12".to_string()], Vec::<String>::new()));
    }

    #[test]
    fn tables_depend_on_the_file() {
        let text = "[files]\n[tasks.build]\n";
        assert!(flagged(ConfigFile::Workspace, text).is_empty());
        assert_eq!(flagged(ConfigFile::Settings, text), ["files", "tasks.build"]);
    }

    #[test]
    fn keybindings_and_syntax_errors_are_left_alone() {
        assert!(flagged(ConfigFile::Keybindings, "x = 1\n[nope]\n").is_empty());
        let _ = flagged(ConfigFile::Settings, "[settings\ntab_width = = 4\n[[\n= 3\n\"q\n");
    }

    /// A 1 MB file of many tables: the pass over a viewport-sized range in the middle stays cheap
    /// (it jumps to the range), and the whole file is not pathological (debug bounds).
    #[test]
    fn milestone_config_pass_budget_on_a_large_file() {
        let mut text = String::new();
        let mut i = 0;
        while text.len() < 1_000_000 {
            text.push_str(&format!(
                "[lsp.l{i}]\ncommand = \"x\"\nbogus = {i}\n[settings]\ntab_width = \"w\"\n"
            ));
            i += 1;
        }
        let tree = tree_of(&text);
        let rope = Rope::from_str(&text);
        let mid = text.len() / 2;
        let t = std::time::Instant::now();
        let got = invalid_ranges(ConfigFile::Workspace, &tree, &rope, mid, mid + 4000);
        let range = t.elapsed();
        assert!(!got.is_empty());
        assert!(range.as_millis() < 100, "viewport pass took {range:?}");
        let t = std::time::Instant::now();
        let all = invalid_ranges(ConfigFile::Workspace, &tree, &rope, 0, text.len());
        assert!(all.len() > 2 * i - 2);
        assert!(t.elapsed().as_secs() < 20, "whole-file pass took {:?}", t.elapsed());
    }

    /// Drift guard: every text `.workspace`'s parser (and, for the tables it has, settings.toml's)
    /// takes is unflagged; every text it rejects is flagged somewhere.
    #[test]
    fn sub_tables_follow_what_the_parsers_take() {
        use crate::workspace::config::WorkspaceConfig;
        let lsp = "[lsp.python]\ncommand = \"x\"\n[lsp.python.init_options]\na = 1\n\
            [lsp.python.init_options.deep]\nb = [1, \"s\"]\n[[lsp.python.init_options.list]]\nc = { d = 1 }\n";
        let both = [
            lsp.to_string(),
            "[lsp]\n".into(),
            "settings.tab_width = 4\nwindow = { title = \"t\" }\n".into(),
            "[settings]\n\"tab_width\" = 4\nrulers = [\n  80, # c\n  120,\n]\r\n[window]\r\ntitle = 'x'\r\n".into(),
            "[lsp.\"c++\"]\ncommand = \"clangd\"\ninit_options = { a = [1] }\n".into(),
        ];
        let workspace_only = [
            "[tasks.build]\ncommand = \"x\"\n[tasks.build.env]\nA = \"b\"\n\"C D\" = 'e'\n",
            "[files.associations]\n\"*.x\" = \"toml\"\n[files]\nexclude = [\"a\"]\n",
            "[tasks]\n",
        ];
        for text in both.iter().map(String::as_str).chain(workspace_only) {
            assert!(WorkspaceConfig::parse(text).is_ok(), "{text}");
            assert_eq!(flagged(ConfigFile::Workspace, text), Vec::<String>::new(), "{text}");
        }
        for text in &both {
            let errors = crate::config::settings::parse(text).errors;
            assert!(errors.is_empty(), "{text}: {errors:?}");
            assert_eq!(flagged(ConfigFile::Settings, text), Vec::<String>::new(), "{text}");
        }
        let rejected = [
            ("[files.exclude]\n", "files.exclude"),
            ("[settings.tab_width]\n", "settings.tab_width"),
            ("[tasks.build]\ncommand = \"x\"\n[tasks.build.env]\nA = 1\n", "1"),
            (
                "[tasks.build]\ncommand = \"x\"\n[tasks.build.env]\nA.b = \"c\"\n",
                "A.b",
            ),
            (
                "[tasks.build]\ncommand = \"x\"\n[tasks.build.env.x]\n",
                "tasks.build.env.x",
            ),
            (
                "[tasks.build]\ncommand = \"x\"\n[[tasks.build.env]]\n",
                "tasks.build.env",
            ),
            ("[[tasks.build]]\ncommand = \"x\"\n", "tasks.build"),
            ("[[lsp]]\n", "lsp"),
            ("[lsp.python]\ncommand = \"x\"\n[lsp.python.args]\n", "lsp.python.args"),
            ("tab_width = 4\n", "tab_width"),
        ];
        for (text, want) in rejected {
            assert!(WorkspaceConfig::parse(text).is_err(), "{text}");
            assert_eq!(flagged(ConfigFile::Workspace, text), [want], "{text}");
        }
    }

    /// A range starting mid-table, past its header (the viewport scrolled into it): the table's
    /// schema still applies, and nothing above the range is reported.
    #[test]
    fn a_range_inside_a_table_uses_its_header() {
        let text = "bogus = 1\n[files.associations]\n\"*.a\" = \"x\"\n\"*.b\" = 2\n[lsp.py]\ncommand = \"x\"\n\
            [lsp.py.init_options]\nq = 1\n[settings]\nfiller = 1\ntab_width = 4\ninsert_spaces = 1\n";
        let rope = Rope::from_str(text);
        let tree = tree_of(text);
        let got = |from: &str| -> Vec<&str> {
            let from = text.find(from).unwrap();
            invalid_ranges(ConfigFile::Workspace, &tree, &rope, from, text.len())
                .into_iter()
                .map(|(a, b)| &text[a..b])
                .collect()
        };
        assert_eq!(got("tab_width"), ["1"]);
        assert_eq!(got("\"*.b\""), ["2", "filler", "1"]);
        assert_eq!(got("q = 1"), ["filler", "1"]);
        assert_eq!(got("bogus"), ["bogus", "2", "filler", "1"]);
    }

    #[test]
    fn only_the_range_is_checked() {
        let text = "[settings]\nbogus = 1\ntab_width = \"x\"\n";
        let rope = Rope::from_str(text);
        let from = text.find("tab_width").unwrap();
        let got = invalid_ranges(ConfigFile::Settings, &tree_of(text), &rope, from, text.len());
        assert_eq!(got.len(), 1);
        assert_eq!(&text[got[0].0..got[0].1], "\"x\"");
    }
}
