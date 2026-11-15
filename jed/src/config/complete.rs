//! Completion for the config files (`settings.toml`, `.workspace`, `keybindings.toml`) without a
//! server: what to offer at a cursor, from the text of its line and the table header above it.
//! Pure functions over the schema (`config::schema`); `app/completion.rs` turns a [`Suggest`]
//! into the popup's items.

use super::schema::{self, ConfigFile, ValueKind};

/// What to offer: `entries` replace the text from byte `start` of the line to the cursor (and
/// `after` bytes past it).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggest {
    pub start: usize,
    pub after: usize,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub label: String,
    pub detail: String,
    /// What replaces the typed text.
    pub text: String,
    /// `CompletionItemKind`.
    pub kind: u64,
}

const PROPERTY: u64 = 10;
const VALUE: u64 = 12;
const ENUM_MEMBER: u64 = 20;
const FUNCTION: u64 = 3;
const KEYWORD: u64 = 14;
const MODULE: u64 = 9;

/// The table a header line names: its dotted path and whether it is `[[array]]`. `None` when the
/// line is no header.
pub fn parse_header(line: &str) -> Option<(Vec<String>, bool)> {
    let t = line.trim();
    let (inner, array) = if let Some(r) = t.strip_prefix("[[") {
        (r.split_once("]]")?.0, true)
    } else {
        (t.strip_prefix('[')?.split_once(']')?.0, false)
    };
    let ok = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '"' | '\'' | ' ');
    if inner.trim().is_empty() || !inner.chars().all(ok) {
        return None;
    }
    let path = inner
        .split('.')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .collect();
    Some((path, array))
}

fn entry(label: &str, detail: &str, text: String, kind: u64) -> Entry {
    Entry {
        label: label.to_string(),
        detail: detail.to_string(),
        text,
        kind,
    }
}

/// What to offer at the end of `before` (the line's text before the cursor) in `file`, in the
/// table named by `header` (the nearest header above, from `parse_header`; `None` before any);
/// `after` is the line's text past the cursor.
pub fn suggest(file: ConfigFile, header: Option<&(Vec<String>, bool)>, before: &str, after: &str) -> Option<Suggest> {
    let trimmed = before.trim_start();
    let indent = before.len() - trimmed.len();
    if trimmed.starts_with('#') {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        return headers(file, indent, rest, after);
    }
    let Some(eq) = find_eq(trimmed) else {
        // A key being typed.
        if !trimmed.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-')) {
            return None;
        }
        let (path, array) = header?;
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let table = schema::table_at(file, &path).filter(|t| t.array == *array)?;
        let has_value = after.trim_start().starts_with('=');
        let entries = table
            .keys
            .iter()
            .map(|k| {
                let text = if has_value {
                    k.name.to_string()
                } else {
                    format!("{} = ", k.name)
                };
                entry(k.name, k.doc, text, PROPERTY)
            })
            .collect();
        return Some(Suggest {
            start: indent,
            after: 0,
            entries,
        });
    };
    let key = trimmed[..eq].trim().trim_matches('"');
    let (path, array) = header?;
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let table = schema::table_at(file, &path).filter(|t| t.array == *array)?;
    let kind = table.key(key)?.kind;
    let value = &trimmed[eq + 1..];
    let vstart = before.len() - value.trim_start().len();
    let v = value.trim_start();
    // Inside an unclosed string: the closing quote that follows the cursor is part of the token.
    let open = v.strip_prefix('"').filter(|s| !s.contains('"'));
    let closing = after.starts_with('"');
    match kind {
        ValueKind::Command | ValueKind::Context => {
            open?;
            let entries = schema::candidates(kind)
                .into_iter()
                .map(|c| {
                    let text = if closing {
                        c.text.clone()
                    } else {
                        format!("{}\"", c.text)
                    };
                    entry(
                        &c.text,
                        &c.detail,
                        text,
                        if kind == ValueKind::Command {
                            FUNCTION
                        } else {
                            ENUM_MEMBER
                        },
                    )
                })
                .collect();
            Some(Suggest {
                start: vstart + 1,
                after: 0,
                entries,
            })
        }
        ValueKind::Key => {
            let typed = open?;
            let piece = typed.rfind('+').map_or(0, |i| i + 1);
            let entries = schema::key_pieces()
                .into_iter()
                .map(|p| {
                    let text = if closing || p.ends_with('+') {
                        p.clone()
                    } else {
                        format!("{p}\"")
                    };
                    entry(&p, "", text, KEYWORD)
                })
                .collect();
            Some(Suggest {
                start: vstart + 1 + piece,
                after: 0,
                entries,
            })
        }
        ValueKind::Enum(_) => {
            if v.starts_with('"') && open.is_none() {
                return None; // a closed string
            }
            if !v.starts_with('"') && !v.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return None;
            }
            let entries = schema::candidates(kind)
                .into_iter()
                .map(|c| entry(&c.text, &c.detail, c.text.clone(), ENUM_MEMBER))
                .collect();
            Some(Suggest {
                start: vstart,
                after: usize::from(open.is_some() && closing),
                entries,
            })
        }
        ValueKind::Bool | ValueKind::Int { pow2: true, .. } => {
            if !v.chars().all(|c| c.is_alphanumeric()) {
                return None;
            }
            let entries = schema::candidates(kind)
                .into_iter()
                .map(|c| entry(&c.text, &c.detail, c.text.clone(), VALUE))
                .collect();
            Some(Suggest {
                start: vstart,
                after: 0,
                entries,
            })
        }
        _ => None,
    }
}

/// The first `=` outside a quoted key.
fn find_eq(s: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '=' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

/// Table names after a `[` (`rest`: the line from after it; `after`: past the cursor, where an
/// auto-closed `]` / `]]` is replaced, as the names bring their own).
fn headers(file: ConfigFile, indent: usize, rest: &str, after: &str) -> Option<Suggest> {
    if rest.contains(']') {
        return None;
    }
    let double = rest.starts_with('[');
    let name = rest.strip_prefix('[').unwrap_or(rest);
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    let mut entries = Vec::new();
    for t in schema::tables(file) {
        let shown = t.pattern.split('*').next().unwrap_or(t.pattern);
        if t.array {
            let text = if double {
                format!("{shown}]]")
            } else {
                format!("[{shown}]]")
            };
            entries.push(entry(&text, "array of tables", text.clone(), MODULE));
        } else if !double {
            entries.push(entry(shown, "", format!("{shown}]"), MODULE));
        }
    }
    let closing = after.bytes().take_while(|&b| b == b']').count();
    Some(Suggest {
        start: indent + 1 + usize::from(double),
        after: closing.min(1 + usize::from(double)),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConfigFile::{Keybindings, Settings, Workspace};

    fn h(s: &str) -> Option<(Vec<String>, bool)> {
        parse_header(s)
    }

    fn labels(s: &Option<Suggest>) -> Vec<&str> {
        s.as_ref()
            .map_or_else(Vec::new, |s| s.entries.iter().map(|e| e.label.as_str()).collect())
    }

    #[test]
    fn headers_parse() {
        assert_eq!(h("[settings]"), Some((vec!["settings".into()], false)));
        assert_eq!(
            h("[lsp.python]  # x"),
            Some((vec!["lsp".into(), "python".into()], false))
        );
        assert_eq!(h("[[binding]]"), Some((vec!["binding".into()], true)));
        assert_eq!(h("[1, 2]"), None);
        assert_eq!(h("key = 1"), None);
    }

    #[test]
    fn keys_of_the_enclosing_table() {
        let hd = h("[settings]");
        let s = suggest(Settings, hd.as_ref(), "", "");
        assert!(labels(&s).contains(&"render_whitespace"));
        assert_eq!(s.as_ref().unwrap().start, 0);
        let s = suggest(Settings, hd.as_ref(), "  ren", "");
        assert_eq!(s.as_ref().unwrap().start, 2);
        assert!(s.unwrap().entries.iter().all(|e| e.text.ends_with(" = ")));
        // The key names only when a `=` already follows.
        let s = suggest(Settings, hd.as_ref(), "ren", " = \"all\"").unwrap();
        assert!(s.entries.iter().any(|e| e.text == "render_whitespace"));
        // Nested and keyed tables.
        let s = suggest(Workspace, h("[tasks.build]").as_ref(), "", "");
        for k in ["command", "cwd", "env"] {
            assert!(labels(&s).contains(&k), "{k}");
        }
        assert!(!labels(&suggest(Workspace, h("[lsp.python]").as_ref(), "", "")).is_empty());
        // Unknown table, no header, a key with a space, a comment.
        assert_eq!(suggest(Settings, h("[nope]").as_ref(), "", ""), None);
        assert_eq!(suggest(Settings, None, "", ""), None);
        assert_eq!(suggest(Settings, hd.as_ref(), "a b", ""), None);
        assert_eq!(suggest(Settings, hd.as_ref(), "# ren", ""), None);
        assert_eq!(suggest(Settings, hd.as_ref(), "  # ren", ""), None);
    }

    #[test]
    fn values_by_kind() {
        let hd = h("[settings]");
        let s = suggest(Settings, hd.as_ref(), "render_whitespace = ", "").unwrap();
        assert_eq!(s.start, 20);
        assert!(s.entries.iter().any(|e| e.text == "\"all\""));
        // Half-typed, open quote, with and without the auto-closed quote after the cursor.
        let s = suggest(Settings, hd.as_ref(), "render_whitespace = \"a", "").unwrap();
        assert_eq!((s.start, s.after), (20, 0));
        let s = suggest(Settings, hd.as_ref(), "render_whitespace = \"a", "\"").unwrap();
        assert_eq!((s.start, s.after), (20, 1));
        // A finished value, a comment after it.
        assert_eq!(suggest(Settings, hd.as_ref(), "render_whitespace = \"all\"", ""), None);
        assert_eq!(
            suggest(Settings, hd.as_ref(), "render_whitespace = \"all\" # ", ""),
            None
        );
        let s = suggest(Settings, hd.as_ref(), "cursor_blinking = t", "").unwrap();
        assert_eq!(labels(&Some(s)), ["true", "false"]);
        assert_eq!(suggest(Settings, hd.as_ref(), "cursor_blinking = true ", ""), None);
        // Powers of two; a free integer, a string list and an unknown key offer nothing.
        let s = suggest(Settings, hd.as_ref(), "hex_bytes_per_line = ", "").unwrap();
        assert_eq!(&labels(&Some(s))[..3], ["1", "2", "4"]);
        assert_eq!(suggest(Settings, hd.as_ref(), "tab_width = ", ""), None);
        assert_eq!(suggest(Settings, hd.as_ref(), "nonsense = ", ""), None);
    }

    #[test]
    fn table_headers() {
        let s = suggest(Settings, None, "[", "").unwrap();
        assert_eq!(s.start, 1);
        assert!(s.entries.iter().any(|e| e.label == "settings" && e.text == "settings]"));
        assert!(s.entries.iter().any(|e| e.label == "lsp."));
        assert_eq!(suggest(Settings, None, "[settings]", ""), None);
        let s = suggest(Keybindings, None, "[", "").unwrap();
        assert_eq!(labels(&Some(s.clone())), ["[binding]]"]);
        assert_eq!(s.entries[0].text, "[binding]]");
        let s = suggest(Keybindings, None, "[[bi", "").unwrap();
        assert_eq!((s.start, s.entries[0].text.as_str()), (2, "binding]]"));
        // The auto-closed brackets after the cursor are replaced.
        assert_eq!(suggest(Keybindings, None, "[", "]").unwrap().after, 1);
        assert_eq!(suggest(Keybindings, None, "[[bi", "]]").unwrap().after, 2);
        assert_eq!(suggest(Settings, None, "[se", "]]").unwrap().after, 1);
        assert_eq!(suggest(Settings, None, "[se", " x").unwrap().after, 0);
    }

    #[test]
    fn keybindings() {
        let hd = h("[[binding]]");
        assert_eq!(
            labels(&suggest(Keybindings, hd.as_ref(), "", "")),
            ["key", "command", "context"]
        );
        let c = Command::Undo.name();
        let s = suggest(Keybindings, hd.as_ref(), "command = \"cur", "").unwrap();
        assert_eq!(s.start, 11);
        assert!(s.entries.iter().any(|e| e.label == c && e.text == format!("{c}\"")));
        // The auto-closed quote stays, not doubled.
        let s = suggest(Keybindings, hd.as_ref(), "command = \"", "\"").unwrap();
        assert!(s.entries.iter().any(|e| e.text == c));
        // Not in a string.
        assert_eq!(suggest(Keybindings, hd.as_ref(), "command = ", ""), None);
        assert_eq!(suggest(Keybindings, hd.as_ref(), "command = \"x\"", ""), None);
        let s = suggest(Keybindings, hd.as_ref(), "context = \"", "\"").unwrap();
        assert!(labels(&Some(s)).contains(&"editor"));
        let s = suggest(Keybindings, hd.as_ref(), "key = \"ctrl+sh", "").unwrap();
        assert_eq!(s.start, 12);
        assert!(s.entries.iter().any(|e| e.label == "shift+" && e.text == "shift+"));
        assert!(s.entries.iter().any(|e| e.label == "enter" && e.text == "enter\""));
        assert_eq!(suggest(Keybindings, h("[settings]").as_ref(), "", ""), None);
    }

    #[test]
    fn odd_lines() {
        // Headers with spaces, CRLF, a comment after; `[binding]` is not `[[binding]]`.
        assert_eq!(
            h(" [ lsp . python ] \r"),
            Some((vec!["lsp".into(), "python".into()], false))
        );
        assert_eq!(h("[settings]\r"), Some((vec!["settings".into()], false)));
        assert_eq!(h("# [settings]"), None);
        assert_eq!(h("[[binding]] # x"), Some((vec!["binding".into()], true)));
        assert_eq!(suggest(Keybindings, h("[binding]").as_ref(), "", ""), None);
        assert_eq!(suggest(Settings, h("[[settings]]").as_ref(), "", ""), None);
        let hd = h("[settings]");
        // `key=value` without spaces.
        let s = suggest(Settings, hd.as_ref(), "render_whitespace=\"a", "").unwrap();
        assert_eq!(s.start, 18);
        let s = suggest(Settings, hd.as_ref(), "cursor_blinking=", "").unwrap();
        assert_eq!((s.start, labels(&Some(s.clone()))), (16, vec!["true", "false"]));
        // A quoted key.
        assert!(suggest(Settings, hd.as_ref(), "\"cursor_blinking\" = ", "").is_some());
        // Comments after a value, and a CRLF line's `\r` after the cursor.
        assert_eq!(suggest(Settings, hd.as_ref(), "hex_bytes_per_line = 1 # c", ""), None);
        assert_eq!(suggest(Settings, hd.as_ref(), "cursor_blinking = true # t", ""), None);
        let s = suggest(Settings, hd.as_ref(), "cursor_blinking = ", "\r").unwrap();
        assert_eq!(s.after, 0);
        // A key typed in front of an existing `=`, with spaces.
        let s = suggest(Settings, hd.as_ref(), "cur", "  = true").unwrap();
        assert!(s.entries.iter().all(|e| !e.text.contains('=')));
        // Inside a multi-line array: a string element or a nested array is no key or header.
        assert_eq!(suggest(Workspace, h("[tasks.build]").as_ref(), "  \"x", ""), None);
        assert_eq!(suggest(Workspace, h("[tasks.build]").as_ref(), "  [1, 2", ""), None);
        assert_eq!(suggest(Workspace, h("[tasks.build]").as_ref(), "command = [", ""), None);
        assert_eq!(h("  [1, 2],"), None);
        // A command name half typed with a `-` / `.` in it; a closed one offers nothing.
        let hb = h("[[binding]]");
        let s = suggest(Keybindings, hb.as_ref(), "command=\"editor.cursor-", "\"").unwrap();
        assert_eq!(s.start, 9);
        assert_eq!(suggest(Keybindings, hb.as_ref(), "command = \"x\" # \"", ""), None);
        // Key pieces after several modifiers.
        let s = suggest(Keybindings, hb.as_ref(), "key = \"ctrl+shift+e", "\"").unwrap();
        assert_eq!(s.start, 18);
        assert!(s.entries.iter().any(|e| e.text == "enter"));
    }

    #[test]
    fn comment_lines_offer_nothing() {
        let cases = [
            (Settings, h("[settings]")),
            (Workspace, h("[settings]")),
            (Keybindings, h("[[binding]]")),
        ];
        for (file, hd) in &cases {
            for line in [
                "#",
                "# ren",
                "#render_whitespace",
                "  #ren",
                "# render_whitespace = ",
                "#render_whitespace = \"a",
                "# cursor_blinking = t",
                "#[",
                "# [[bi",
                "# command = \"",
                "#key = \"ctrl+",
            ] {
                assert_eq!(suggest(*file, hd.as_ref(), line, ""), None, "{file:?} {line:?}");
                assert_eq!(suggest(*file, None, line, ""), None, "{file:?} {line:?}");
            }
        }
    }

    use crate::command::Command;
}
