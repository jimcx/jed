//! Format document's model (no drawing; `app/format.rs` drives it): the `FormattingOptions` a
//! `textDocument/formatting` request sends, reading its `TextEdit[]` answer, and shrinking each
//! edit to what it really changes.
//!
//! - `options(settings)`: `tabSize` and `insertSpaces` from the buffer language's settings (jed
//!   has no trim-whitespace / final-newline settings; VS Code sends those only when they're on).
//! - `parse(answer)`: `TextEdit[]`; `null` is no edits.
//! - `minimize(rope, changes)`: each change trimmed of the text it keeps at its start and end (a
//!   server may replace whole lines or the whole document to change a little), so cursors and
//!   other views outside the real change stay where they were. VS Code goes further: a big edit
//!   is diffed line by line (`computeMoreMinimalEdits`); here one edit stays one change.

use ropey::Rope;
use serde_json::{Value, json};

use crate::buffer::change::ChangeSet;
use crate::buffer::pos::ByteIdx;
use crate::edit::Settings;
use crate::rename::{self, TextEdit};

/// `FormattingOptions` for a buffer whose language's settings are `s`.
pub fn options(s: &Settings) -> Value {
    json!({ "tabSize": s.tab_width, "insertSpaces": s.insert_spaces })
}

/// A formatting answer's edits (`null`: none). `Err` holds why it can't be read.
pub fn parse(value: &Value) -> Result<Vec<TextEdit>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    rename::text_edits(value)
}

/// `changes` (on `rope`) with each change shrunk to the part that differs; changes that alter
/// nothing are dropped.
pub fn minimize(rope: &Rope, changes: &ChangeSet) -> ChangeSet {
    let mut out = ChangeSet::with_capacity(changes.len(), 0);
    for (start, end, new) in changes.changes() {
        let old = rope.slice(start.0..end.0).to_string();
        let (o, n) = (old.as_bytes(), new.as_bytes());
        let mut prefix = o.iter().zip(n).take_while(|(a, b)| a == b).count();
        // Equal bytes up to here: a char boundary in one is one in the other.
        prefix = new.floor_char_boundary(prefix);
        let max_suffix = o.len().min(n.len()) - prefix;
        let mut suffix = o
            .iter()
            .rev()
            .zip(n.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        while !new.is_char_boundary(n.len() - suffix) {
            suffix -= 1;
        }
        out.push(
            ByteIdx(start.0 + prefix),
            ByteIdx(end.0 - suffix),
            &new[prefix..n.len() - suffix],
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::buffer::change::Change;
    use crate::buffer::pos::{LspPos, PosEncoding};

    #[test]
    fn options_from_settings() {
        let s = Settings {
            tab_width: 2,
            insert_spaces: false,
            ..Settings::default()
        };
        assert_eq!(options(&s), json!({ "tabSize": 2, "insertSpaces": false }));
    }

    #[test]
    fn answers() {
        assert_eq!(parse(&Value::Null), Ok(vec![]));
        assert_eq!(parse(&json!([])), Ok(vec![]));
        let edits = parse(&json!([{
            "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 2 } },
            "newText": "x",
        }]))
        .unwrap();
        assert_eq!(
            edits,
            [TextEdit {
                start: LspPos { line: 0, character: 1 },
                end: LspPos { line: 0, character: 2 },
                text: "x".into(),
            }]
        );
        assert!(parse(&json!({ "nope": 1 })).is_err());
    }

    fn shrink(text: &str, changes: Vec<Change>) -> (Vec<(usize, usize, String)>, String) {
        let mut rope = Rope::from_str(text);
        let set = minimize(&rope, &ChangeSet::new(changes));
        let got = set.changes().map(|(s, e, t)| (s.0, e.0, t.to_string())).collect();
        set.apply(&mut rope);
        (got, rope.to_string())
    }

    fn change(start: usize, end: usize, text: &str) -> Change {
        Change {
            start: ByteIdx(start),
            end: ByteIdx(end),
            text: text.into(),
        }
    }

    #[test]
    fn edits_shrink_to_what_changes() {
        // The whole document replaced to re-indent one line.
        let text = "int f() {\nreturn 1;\n}\n";
        let (got, after) = shrink(text, vec![change(0, text.len(), "int f() {\n    return 1;\n}\n")]);
        assert_eq!(got, [(10, 10, "    ".to_string())]);
        assert_eq!(after, "int f() {\n    return 1;\n}\n");
        // An edit that changes nothing goes; one that shortens keeps the right end.
        let (got, after) = shrink("a  b\n", vec![change(0, 2, "a"), change(2, 5, " b\n")]);
        assert_eq!(got, [(1, 2, String::new())]);
        assert_eq!(after, "a b\n");
        // Repeated chars: prefix and suffix never overlap.
        let (got, after) = shrink("aaa", vec![change(0, 3, "aaaa")]);
        assert_eq!(got, [(3, 3, "a".to_string())]);
        assert_eq!(after, "aaaa");
        // Multi-byte chars sharing a first byte are never split (é = C3 A9, è = C3 A8).
        let (got, after) = shrink("xéy", vec![change(0, 4, "xèy")]);
        assert_eq!(got, [(1, 3, "è".to_string())]);
        assert_eq!(after, "xèy");
        // And sharing a last byte (Ā = C4 80, À = C3 80).
        let (got, after) = shrink("xĀ", vec![change(0, 3, "xÀ")]);
        assert_eq!(got, [(1, 3, "À".to_string())]);
        assert_eq!(after, "xÀ");
    }

    #[test]
    fn utf16_edits_round_trip() {
        let rope = Rope::from_str("😀 a=1\n");
        let set = rename::changes(
            &rope,
            &[TextEdit {
                start: LspPos { line: 0, character: 0 },
                end: LspPos { line: 1, character: 0 },
                text: "😀 a = 1\n".into(),
            }],
            PosEncoding::Utf16,
        )
        .unwrap();
        let mut rope2 = rope.clone();
        let set = minimize(&rope, &set);
        set.apply(&mut rope2);
        assert_eq!(rope2.to_string(), "😀 a = 1\n");
        assert_eq!(set.len(), 1);
    }
}
