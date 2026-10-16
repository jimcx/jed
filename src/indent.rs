//! Auto-indent at every cursor (VS Code's `editor.autoIndent: full`, approximated): Enter keeps
//! the line's indentation and adds or removes a level, and typing a closing bracket (or a Python
//! `else:` …) as a line's first char re-indents the line. Off with `Settings::auto_indent`, which
//! makes Enter a plain line break.
//!
//! Enter, per selection (the selection is replaced):
//! - The new line gets the leading whitespace of the cursor's line up to the cursor (not past
//!   it, so Enter inside the indentation keeps the text after it where it was). A line holding
//!   only whitespace after the cursor loses that whitespace (VS Code).
//! - The text before the cursor, trailing whitespace and a trailing comment left out, ending
//!   with an opening bracket (a token of its own when there is a parse tree, so not one in a
//!   string or comment) adds a level (`Settings::insert_spaces`: `tab_width` spaces, else a tab);
//!   with its closing bracket next after the cursor (whitespace between allowed), the closing
//!   bracket goes on a line of its own at the old indentation and the cursor on the indented
//!   middle line (VS Code's `IndentAction.IndentOutdent`).
//! - C/C++ and Rust (VS Code's `indentationRules` for them, when nothing above applied): an
//!   opening bracket left unclosed anywhere before the cursor (`g(a,`, `{ x;`) adds a level;
//!   text after the cursor starting with a closing bracket removes one (`x;|}` puts the `}` a
//!   level left). Rust's rules leave `[` out and stop at a quote; jed uses C's for both.
//! - YAML (its `indentationRules`' increase pattern): a line ending with `:` or `-` (a mapping
//!   key, a sequence dash) adds a level.
//! - Python (VS Code's `onEnterRules`, which come before brackets): a line starting with a block
//!   keyword (`def`, `class`, `if`, `for`, …) and ending with `:` adds a level; a line starting
//!   with `return`, `pass`, `break`, `continue` or `raise` removes one.
//! - Comments: with a tree, the comment token the line ends in; without one, from the language's
//!   line comment token when no quote comes before it.
//!
//! Typing:
//! - A closing bracket typed with only whitespace before it on the line (and not inside a
//!   multi-line string or comment, with a tree) re-indents the line to the indentation of the
//!   line holding the opening bracket it closes (`brackets::Openers`; VS Code's
//!   `decreaseIndentPattern` `^\s*[}\])]`).
//! - Python: `:` completing `else:`, `finally:`, `elif …:` or `except …:` (nothing after the
//!   cursor) moves the line to the indentation of the nearest less indented line above, when that
//!   line opens a block the keyword continues (`if`/`elif` for `elif`, …; up to `PYTHON_SCAN`
//!   lines up).
//!
//! Not done (VS Code differences): indentation isn't normalized to the tab / space setting; no
//! `trimAutoWhitespace` (a line left holding only the carried indentation keeps it); no
//! re-indent on paste or line moves; without a tree, brackets in strings and comments count;
//! C/C++'s other `onEnterRules` (`// ` continued when Enter splits a line comment; outdent after
//! the statement of a brace-less `if` / `for` / `while`); a closing bracket typed first on a line
//! goes to its opener's indentation (VS Code's electric characters) rather than the level
//! `indentationRules` infer from the lines above, and per cursor (VS Code: all or none).

use crate::brackets::Openers;
use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx, LineWalker};
use crate::buffer::undo::EditKind;
use crate::edit::{Land, Settings, replace_each, replace_each_landing};
use crate::syntax::{Language, Scope, Tokens, scope_of};
use crate::view::View;

/// How many lines up a Python `else:` looks for its `if`.
const PYTHON_SCAN: usize = 500;

const BLOCK_KEYWORDS: &[&str] = &[
    "def", "class", "for", "if", "elif", "else", "while", "try", "with", "finally", "except", "async", "match", "case",
];
const DEDENT_KEYWORDS: &[&str] = &["return", "pass", "break", "continue", "raise"];

/// One indentation level.
pub fn unit(s: &Settings) -> String {
    if s.insert_spaces {
        " ".repeat(s.tab_width.max(1))
    } else {
        "\t".to_string()
    }
}

/// `indent` (whitespace) one level less: a trailing tab, or trailing spaces back to the previous
/// tab stop (VS Code's `unshiftIndent`).
fn unindent(indent: &str, tab_width: usize) -> &str {
    if let Some(rest) = indent.strip_suffix('\t') {
        return rest;
    }
    let tab_width = tab_width.max(1);
    let cols = width(indent, tab_width);
    if cols == 0 {
        return indent;
    }
    let target = (cols - 1) / tab_width * tab_width;
    let spaces = indent.len() - indent.trim_end_matches(' ').len();
    &indent[..indent.len() - spaces.min(cols - target)]
}

/// Columns `indent` (spaces and tabs) takes.
fn width(indent: &str, tab_width: usize) -> usize {
    indent.chars().fold(0, |col, c| {
        if c == '\t' {
            col + tab_width - col % tab_width
        } else {
            col + 1
        }
    })
}

fn leading_ws(text: &str) -> &str {
    &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
}

/// Whether `text` starts with `word` as a whole word.
fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| !rest.starts_with(pos::is_word_char))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Keep,
    Indent,
    IndentOutdent,
    Outdent,
}

/// `content` (a line's text from `start` up to the cursor, trailing whitespace dropped) without
/// a trailing comment, trailing whitespace dropped again.
fn without_comment<'a>(
    content: &'a str,
    start: usize,
    comments: Option<&mut Tokens>,
    line_comment: Option<&str>,
) -> &'a str {
    let Some(last) = content.len().checked_sub(1) else {
        return content;
    };
    let cut = match comments {
        Some(tokens) => tokens
            .node_at(start + last)
            .filter(|n| scope_of(n.kind()) == Some(Scope::Comment) && n.start_byte() >= start)
            .map(|n| n.start_byte() - start),
        None => line_comment
            .and_then(|token| content.find(token))
            .filter(|&i| !content[..i].contains(['"', '\''])),
    };
    match cut {
        Some(i) => content[..i].trim_end_matches([' ', '\t']),
        None => content,
    }
}

/// VS Code's C/C++ `increaseIndentPattern` (`^.*(\{[^}]*|\([^)]*|\[[^\]]*)$`): `code` (a line
/// from byte `start` up to the cursor, trailing comment left out) holds an opening bracket with no
/// closing bracket of its kind after it. Only bracket tokens count (VS Code drops the brackets in
/// strings and comments first): `tokens` is asked only when `code` holds a quote or `/*` (walking
/// the tree per line costs ~45 ms at 65k cursors; the text alone is exact otherwise).
fn opens_unclosed(code: &str, start: usize, tokens: Option<&mut Tokens>) -> bool {
    // Per kind: whether its last bracket seen opens.
    let mut open = [false; 3];
    let mut see = |b: u8| match b {
        b'{' | b'}' => open[0] = b == b'{',
        b'(' | b')' => open[1] = b == b'(',
        b'[' | b']' => open[2] = b == b'[',
        _ => {}
    };
    let bytes = code.as_bytes();
    let mut maybe_text = false;
    for (i, &b) in bytes.iter().enumerate() {
        maybe_text |= b == b'"' || b == b'\'' || (b == b'*' && i > 0 && bytes[i - 1] == b'/');
        see(b);
    }
    if maybe_text && let Some(t) = tokens {
        open = [false; 3];
        let mut see = |b: u8| match b {
            b'{' | b'}' => open[0] = b == b'{',
            b'(' | b')' => open[1] = b == b'(',
            b'[' | b']' => open[2] = b == b'[',
            _ => {}
        };
        t.each_leaf(start, start + code.len(), |_, kind| {
            if let &[b] = kind.as_bytes() {
                see(b);
            }
        });
    }
    open.contains(&true)
}

/// Enter at every selection with auto-indent (see the module docs). One edit, `EditKind::Insert`
/// (typing and Enter group into one undo step).
pub fn newline(buf: &mut Buffer, view: &mut View, s: &Settings) {
    let eol = buf.eol.as_str();
    if !s.auto_indent {
        replace_each(buf, view, EditKind::Insert, |_, sel| (sel.start(), sel.end(), eol));
        return;
    }
    let (edits, text) = enter_edits(buf, view, s, eol);
    let mut edits = edits.into_iter();
    replace_each_landing(buf, view, EditKind::Insert, |_, sel| match edits.next() {
        Some((start, end, range, land)) => (start, end, &text[range], land),
        None => (sel.start(), sel.end(), "", Land::AfterText),
    });
}

/// One selection's Enter: the range replaced, its new text (a range of the shared string) and
/// where the cursor lands.
type EnterEdit = (ByteIdx, ByteIdx, std::ops::Range<usize>, Land);

/// Each selection's Enter, the new texts all in one string (60k cursors, no allocation each).
fn enter_edits(buf: &Buffer, view: &View, s: &Settings, eol: &str) -> (Vec<EnterEdit>, String) {
    let language = buf.syntax.language();
    let config = language.config();
    let unit = unit(s);
    let rope = &buf.rope;
    // Tree lookups go through walks asked in increasing order: `tokens` for the end of the text
    // before each cursor, `cut` for the bracket before a trailing comment (further back).
    let (mut tokens, mut cut) = (buf.syntax.tokens(), buf.syntax.tokens());
    // C/C++'s unclosed-bracket scan from each line's start, and the line it last scanned.
    let (mut scan, mut scan_line) = (buf.syntax.tokens(), None);
    let comment_start = |content: &str| match config.line_comment {
        Some("//") => content.contains("//") || content.contains("/*"),
        Some(token) => content.contains(token),
        None => false,
    };
    let mut walk = LineWalker::new(rope);
    let mut edits = Vec::with_capacity(view.selections.len());
    let mut out = String::new();
    for sel in view.selections.iter() {
        let (start, mut end) = (sel.start(), sel.end());
        let line = walk.line_at(start);
        let line_start = line.start().0;
        let before = &line.text()[..(start.0 - line_start).min(line.text().len())];
        let indent = leading_ws(before);
        let content = before.trim_end_matches([' ', '\t']);
        let code = if comment_start(content) {
            without_comment(content, line_start, tokens.as_mut(), config.line_comment)
        } else {
            content
        };
        let mut action = Action::Keep;
        if language == Language::Python {
            let words = code.trim_start();
            if code.ends_with(':') && BLOCK_KEYWORDS.iter().any(|k| starts_with_word(words, k)) {
                action = Action::Indent;
            } else if !indent.is_empty() && DEDENT_KEYWORDS.iter().any(|k| starts_with_word(words, k)) {
                action = Action::Outdent;
            }
        }
        if language == Language::Yaml && code.ends_with([':', '-']) {
            action = Action::Indent;
        }
        let mut close = None;
        if action == Action::Keep
            && let Some(last) = code.chars().next_back()
            && let Some((_, c)) = config.brackets().find(|&(o, _)| o == last)
        {
            let at = line_start + code.len() - 1;
            let walk = if code.len() == content.len() {
                &mut tokens
            } else {
                &mut cut
            };
            if walk.as_mut().is_none_or(|t| t.is_token(at, &code[code.len() - 1..])) {
                action = Action::Indent;
                close = Some(c);
            }
        }
        // C/C++ and Rust (VS Code's `indentationRules`, used when no rule above applied): a level
        // more after an opening bracket left unclosed anywhere before the cursor, one less when
        // the text after the cursor starts with a closing bracket.
        let rules = action == Action::Keep && matches!(language, Language::C | Language::Cpp | Language::Rust);
        if rules {
            // Not for a second cursor on a line (the walk would go back).
            let walk = scan.as_mut().filter(|_| scan_line != Some(line_start));
            scan_line = Some(line_start);
            if opens_unclosed(code, line_start, walk) {
                action = Action::Indent;
            }
        }
        let indent_len = indent.len();
        let from = out.len();
        out.push_str(eol);
        out.push_str(indent);
        // What follows the selection on its line.
        let end_line = walk.line_at(end);
        let after = &end_line.text()[(end.0 - end_line.start().0).min(end_line.text().len())..];
        let after_trimmed = after.trim_start_matches([' ', '\t']);
        if after_trimmed.is_empty() && end_line.text()[..end_line.text().len() - after.len()].trim().is_empty() {
            // Only whitespace on the line after the cursor (and before it): dropped.
            end = end_line.end();
        }
        if close.is_some_and(|c| after_trimmed.starts_with(c)) {
            action = Action::IndentOutdent;
        } else if rules && after_trimmed.starts_with(['}', ')', ']']) {
            action = if action == Action::Indent {
                Action::Keep
            } else {
                Action::Outdent
            };
        }
        let mut land = Land::AfterText;
        match action {
            Action::Keep => {}
            Action::Indent => out.push_str(&unit),
            Action::Outdent => {
                let indent = &out[from + eol.len()..];
                let keep = unindent(indent, s.tab_width).len();
                out.truncate(from + eol.len() + keep);
            }
            Action::IndentOutdent => {
                out.push_str(&unit);
                land = Land::Inside(out.len() - from);
                out.push_str(eol);
                out.extend_from_within(from + eol.len()..from + eol.len() + indent_len);
            }
        }
        edits.push((start, end, from..out.len(), land));
    }
    (edits, out)
}

/// Typing `c` with auto-indent: a closing bracket (or Python's `:` ending `else:` …) as the
/// first char of a line re-indents that line, at every cursor where it applies (the others type
/// `c` plainly). False when it applies at none (the caller goes on to auto-close and typing).
pub fn type_char(buf: &mut Buffer, view: &mut View, c: char, s: &Settings) -> bool {
    let language = buf.syntax.language();
    let python_colon = c == ':' && language == Language::Python;
    let open = language
        .config()
        .brackets()
        .find(|&(_, close)| close == c)
        .map(|(o, _)| o);
    if open.is_none() && !python_colon {
        return false;
    }
    let Some(changes) = reindents(buf, view, c, open, s.tab_width) else {
        return false;
    };
    let mut changes = changes.into_iter();
    let mut utf8 = [0; 4];
    let typed: &str = c.encode_utf8(&mut utf8);
    replace_each(buf, view, EditKind::Insert, |_, sel| match changes.next().flatten() {
        Some((line_start, text)) => (line_start, sel.end(), format!("{text}{typed}")),
        None => (sel.start(), sel.end(), typed.to_string()),
    });
    true
}

/// Per selection, the line start and its text up to the cursor re-indented, where typing `c`
/// re-indents the line (`open`: the bracket `c` closes, else Python's `:`); `None` if it does at
/// none. A closing bracket's checks read bytes next to the cursor and the opener (`Peek`), not
/// whole lines: stepping two `LineWalker`s cost ~55 ms of a `}` at 65k cursors.
#[allow(clippy::type_complexity)]
fn reindents(
    buf: &Buffer,
    view: &View,
    c: char,
    open: Option<char>,
    tab_width: usize,
) -> Option<Vec<Option<(ByteIdx, String)>>> {
    let rope = &buf.rope;
    let mut tokens = buf.syntax.tokens();
    let mut openers = open.map(|o| Openers::new(rope, &buf.syntax, o, c));
    let mut walk = LineWalker::new(rope);
    // The openers' lines, when one is too long to look back over.
    let mut opener_lines = LineWalker::new(rope);
    // Per selection: the line start and the new indentation, where it changes.
    let mut changes: Vec<Option<(ByteIdx, String)>> = Vec::with_capacity(view.selections.len());
    let mut any = false;
    let mut peek = pos::Peek::new(rope);
    // The last opener and its line's indentation (many cursors in one block share it).
    let mut last_opener: Option<(ByteIdx, String)> = None;
    for sel in view.selections.iter() {
        let start = sel.start();
        let change = if let Some(openers) = openers.as_mut() {
            // Only spaces and tabs before it on its line, and not inside a string or comment.
            // Looked at backwards from the cursor, over those blanks only: no line lookup (a
            // closer typed at 60k cursors).
            let Some(line_start) = peek.blank_line_start(start) else {
                changes.push(None);
                continue;
            };
            let in_text = start.0 > 0
                && tokens
                    .as_mut()
                    .and_then(|t| t.node_at(start.0 - 1))
                    .is_some_and(|n| matches!(scope_of(n.kind()), Some(Scope::String | Scope::Comment)));
            let opener = if in_text { None } else { openers.before(start) };
            opener.and_then(|o| {
                if last_opener.as_ref().is_none_or(|(p, _)| *p != o) {
                    let indent = peek
                        .line_indent(o, 4096)
                        .unwrap_or_else(|| leading_ws(opener_lines.line_at(o).text()).to_string());
                    last_opener = Some((o, indent));
                }
                let indent = &last_opener.as_ref().expect("just set").1;
                // The blanks before the cursor become the opener's indentation.
                let same = start.0 - line_start.0 == indent.len()
                    && indent
                        .bytes()
                        .enumerate()
                        .all(|(i, b)| peek.byte(line_start.0 + i) == b);
                (!same).then(|| (line_start, indent.clone()))
            })
        } else {
            let line = walk.line_at(start);
            let line_start = line.start();
            let before = &line.text()[..(start.0 - line_start.0).min(line.text().len())];
            let after = &line.text()[(sel.end().0 - line_start.0).min(line.text().len())..];
            let new_indent = if pos::line_of(rope, sel.end()) == line.index && after.trim().is_empty() {
                python_block_indent(rope, line.index, before, tab_width)
            } else {
                None
            };
            // The line up to the cursor, re-indented.
            new_indent
                .filter(|n| n != leading_ws(before))
                .map(|n| (line_start, format!("{n}{}", before.trim_start_matches([' ', '\t']))))
        };
        any |= change.is_some();
        changes.push(change);
    }
    any.then_some(changes)
}

/// The indentation for a Python line `before` + `:` that continues a block (`else:`, `elif x:`,
/// `except E:`, `finally:`): that of the nearest less indented line above, if it starts a block
/// this keyword continues. `None` otherwise.
fn python_block_indent(rope: &ropey::Rope, line: usize, before: &str, tab_width: usize) -> Option<String> {
    let indent = leading_ws(before);
    let words = before.trim();
    let openers: &[&str] = if words == "else" {
        &["if", "elif", "for", "while", "try", "except"]
    } else if words == "finally" {
        &["try", "except", "else"]
    } else if starts_with_word(words, "elif") {
        &["if", "elif"]
    } else if starts_with_word(words, "except") {
        &["try", "except"]
    } else {
        return None;
    };
    let current = width(indent, tab_width);
    if current == 0 {
        return None;
    }
    for l in (line.saturating_sub(PYTHON_SCAN)..line).rev() {
        let text = pos::line_text(rope, l);
        let trimmed = text.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let ws = leading_ws(&text);
        if width(ws, tab_width) < current {
            return openers
                .iter()
                .any(|k| starts_with_word(trimmed, k))
                .then(|| ws.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::edit;
    use crate::pairs::tests::{setup, show};

    fn enter_with(language: Language, marked: &str, parse: bool, s: &Settings) -> String {
        let (mut b, mut v) = setup(language, marked, parse);
        edit::run(Command::InsertNewline, &mut b, &mut v, s);
        show(&b, &v)
    }

    fn enter(language: Language, marked: &str) -> String {
        enter_with(language, marked, true, &Settings::default())
    }

    fn typed(language: Language, marked: &str, keys: &str, parse: bool) -> String {
        let (mut b, mut v) = setup(language, marked, parse);
        for c in keys.chars() {
            edit::insert_char(&mut b, &mut v, c, &Settings::default());
            v.auto_closed.validate(b.revision(), &v.selections);
        }
        show(&b, &v)
    }

    const CPP: Language = Language::Cpp;
    const PY: Language = Language::Python;

    #[test]
    fn enter_keeps_the_indent_up_to_the_cursor() {
        assert_eq!(enter(CPP, "    int x;|\n"), "    int x;\n    |\n");
        assert_eq!(enter(CPP, "\tint x;|"), "\tint x;\n\t|");
        // In the middle of the indentation: only what's before the cursor.
        assert_eq!(enter(CPP, "  |  int x;"), "  \n  |  int x;");
        // Mid-line: the text after moves down at the same indent.
        assert_eq!(enter(CPP, "    a = |b;"), "    a = \n    |b;");
        // Trailing whitespace before the cursor isn't carried; a whitespace-only rest is dropped.
        assert_eq!(enter(CPP, "  x;   |"), "  x;   \n  |");
        assert_eq!(
            enter(CPP, "  x;|   "),
            "  x;\n  |   ",
            "whitespace after on a non-blank line moves down"
        );
        assert_eq!(enter(CPP, "    |    \nz"), "    \n    |\nz");
    }

    #[test]
    fn enter_after_an_opening_bracket_indents() {
        assert_eq!(enter(CPP, "int f() {|"), "int f() {\n    |");
        // C/C++: an unclosed bracket anywhere before the cursor (not one in a string).
        assert_eq!(enter(CPP, "  g(a,  |"), "  g(a,  \n      |");
        assert_eq!(
            enter(CPP, "  g(a, h(b)|"),
            "  g(a, h(b)\n  |",
            "VS Code's regex: the last ( is closed"
        );
        assert_eq!(enter(CPP, "  g({ a }, [b|"), "  g({ a }, [b\n      |");
        assert_eq!(enter(CPP, "  int f() { x;|"), "  int f() { x;\n      |");
        assert_eq!(enter(CPP, "  g(a) {}|"), "  g(a) {}\n  |", "closed");
        assert_eq!(enter(CPP, "  s = \"(\";|"), "  s = \"(\";\n  |");
        assert_eq!(
            enter_with(CPP, "  g(a,|", false, &Settings::default()),
            "  g(a,\n      |"
        );
        assert_eq!(enter(CPP, "  g(a) // (|"), "  g(a) // (\n  |");
        assert_eq!(enter(PY, "  g(a,|"), "  g(a,\n  |", "not Python (no indentationRules)");
        assert_eq!(
            enter(CPP, "  a(|b|"),
            "  a(\n      |b\n      |",
            "two cursors on the line"
        );
        assert_eq!(enter(CPP, "  g(|"), "  g(\n      |");
        assert_eq!(enter(CPP, "  x = [  |"), "  x = [  \n      |");
        // A trailing comment is skipped.
        assert_eq!(enter(CPP, "if (x) { // go|"), "if (x) { // go\n    |");
        assert_eq!(
            enter_with(CPP, "if (x) { // go|", false, &Settings::default()),
            "if (x) { // go\n    |"
        );
        // A bracket in a string or comment doesn't count (with the tree).
        assert_eq!(enter(CPP, "s = \"{|\";"), "s = \"{\n|\";");
        assert_eq!(enter(CPP, "// {|"), "// {\n|");
    }

    #[test]
    fn enter_between_brackets_puts_the_closer_on_its_own_line() {
        assert_eq!(enter(CPP, "  int f() {|}"), "  int f() {\n      |\n  }");
        assert_eq!(enter(CPP, "f(|  )"), "f(\n    |\n  )");
        assert_eq!(enter(CPP, "f(|]"), "f(\n    |]", "not its own closer");
        // C/C++: a closing bracket after the cursor goes a level left.
        assert_eq!(enter(CPP, "    x;|}"), "    x;\n|}");
        assert_eq!(enter(CPP, "    x;|  )"), "    x;\n|  )");
        assert_eq!(enter(CPP, "  int f() { x;|}"), "  int f() { x;\n  |}");
        assert_eq!(enter(PY, "    x|]"), "    x\n    |]");
        // Tabs.
        let tabs = Settings {
            insert_spaces: false,
            ..Settings::default()
        };
        assert_eq!(enter_with(CPP, "\tif (x) {|}", true, &tabs), "\tif (x) {\n\t\t|\n\t}");
        let two = Settings {
            tab_width: 2,
            ..Settings::default()
        };
        assert_eq!(enter_with(CPP, "x {|}", true, &two), "x {\n  |\n}");
    }

    #[test]
    fn enter_in_python() {
        assert_eq!(enter(PY, "if x:|"), "if x:\n    |");
        assert_eq!(enter(PY, "    def f(a):  # doc|"), "    def f(a):  # doc\n        |");
        assert_eq!(
            enter_with(PY, "for a in b: # c|", false, &Settings::default()),
            "for a in b: # c\n    |"
        );
        assert_eq!(enter(PY, "x = d[1:|"), "x = d[1:\n|", "not a block keyword");
        assert_eq!(enter(PY, "    return x|"), "    return x\n|");
        assert_eq!(enter(PY, "        pass|"), "        pass\n    |");
        assert_eq!(enter(PY, "passed = 1|"), "passed = 1\n|", "a whole word only");
        assert_eq!(enter(PY, "f(|)"), "f(\n    |\n)");
    }

    #[test]
    fn enter_with_crlf_selection_and_many_cursors() {
        assert_eq!(enter(CPP, "  {|}\r\n"), "  {\r\n      |\r\n  }\r\n");
        // A selection is replaced; the indent is that of its start's line.
        assert_eq!(enter(CPP, "  a«bc\nd»e"), "  a\n  |e");
        // Two cursors on one line, and on adjacent lines.
        assert_eq!(enter(CPP, "  a|b|c"), "  a\n  |b\n  |c");
        assert_eq!(enter(CPP, "  {|}\n    x|\n"), "  {\n      |\n  }\n    x\n    |\n");
        // One undo step, which joins typing.
        let (mut b, mut v) = setup(CPP, "  f(|)", false);
        edit::run(Command::InsertNewline, &mut b, &mut v, &Settings::default());
        edit::insert_char(&mut b, &mut v, 'x', &Settings::default());
        edit::run(Command::Undo, &mut b, &mut v, &Settings::default());
        assert_eq!(show(&b, &v), "  f(|)");
    }

    #[test]
    fn enter_with_auto_indent_off_is_a_plain_line_break() {
        let off = Settings {
            auto_indent: false,
            ..Settings::default()
        };
        assert_eq!(enter_with(CPP, "    f() {|}", true, &off), "    f() {\n|}");
        assert_eq!(typed(CPP, "{\n    |", "}", true), "{\n}|");
        let (mut b, mut v) = setup(CPP, "{\n    |", true);
        edit::insert_char(&mut b, &mut v, '}', &off);
        assert_eq!(show(&b, &v), "{\n    }|");
    }

    #[test]
    fn closing_bracket_outdents_to_its_opener() {
        for parse in [true, false] {
            assert_eq!(
                typed(CPP, "  if (x) {\n      y;\n      |", "}", parse),
                "  if (x) {\n      y;\n  }|"
            );
            assert_eq!(typed(CPP, "f(a,\n        |", ")", parse), "f(a,\n)|");
            // Not the first char on the line.
            assert_eq!(typed(CPP, "  {\n      x|", "}", parse), "  {\n      x}|");
            // Already right.
            assert_eq!(typed(CPP, "  {\n  |", "}", parse), "  {\n  }|");
        }
        // Nested, with a closed pair between.
        assert_eq!(
            typed(CPP, "void f() {\n    if (a) {\n    }\n        |", "}", true),
            "void f() {\n    if (a) {\n    }\n}|"
        );
        // Inside a multi-line comment nothing happens (with the tree).
        assert_eq!(typed(CPP, "{\n/*\n      |\n*/", "}", true), "{\n/*\n      }|\n*/");
        // Nor inside a multi-line string.
        assert_eq!(
            typed(CPP, "{\n  s = R\"(\n      |\n)\";", "}", true),
            "{\n  s = R\"(\n      }|\n)\";"
        );
        // CRLF, and an opener on a line longer than the backward look (the line walked instead).
        for parse in [true, false] {
            assert_eq!(typed(CPP, "  {\r\n      |", "}", parse), "  {\r\n  }|");
            let long = format!("  f({}, {{\n      |", "x".repeat(5000));
            assert_eq!(
                typed(CPP, &long, "}", parse),
                format!("  f({}, {{\n  }}|", "x".repeat(5000))
            );
        }
        // Many cursors in one block share its opener.
        assert_eq!(typed(CPP, "\t{\n\t\t|\n\t\t|\n  |", "}", true), "\t{\n\t}|\n\t}|\n\t}|");
        // At several cursors, each to its own opener (in the text before the keystroke).
        assert_eq!(
            typed(CPP, "  a {\n      x;\n      |\n  b {\n|", "}", true),
            "  a {\n      x;\n  }|\n  b {\n  }|"
        );
    }

    #[test]
    fn python_else_dedents_to_its_if() {
        let src = "def f():\n    if a:\n        x\n        else|";
        assert_eq!(typed(PY, src, ":", true), "def f():\n    if a:\n        x\n    else:|");
        let src = "try:\n    x\n    except ValueError|";
        assert_eq!(typed(PY, src, ":", false), "try:\n    x\nexcept ValueError:|");
        // Already dedented, or not continuing a block: typed plainly.
        assert_eq!(typed(PY, "if a:\n    x\nelse|", ":", true), "if a:\n    x\nelse:|");
        assert_eq!(
            typed(PY, "def f():\n    x\n    else|", ":", true),
            "def f():\n    x\n    else:|"
        );
        assert_eq!(typed(PY, "if a:\n    x = y|", ":", true), "if a:\n    x = y:|");
        // The `if` is looked for up to `PYTHON_SCAN` (500) lines up.
        for (lines, dedent) in [(499, true), (500, false)] {
            let body = "    x\n".repeat(lines);
            let want = if dedent { "else:|" } else { "    else:|" };
            assert_eq!(
                typed(PY, &format!("if a:\n{body}    else|"), ":", false),
                format!("if a:\n{body}{want}"),
                "{lines} lines"
            );
        }
    }

    /// The new languages: generic bracket indent everywhere, C's rules for Rust, YAML's `:`,
    /// Makefiles with tabs, and Markdown's brackets read from the text (its tree's leaves are
    /// whole paragraphs).
    #[test]
    fn enter_in_other_languages() {
        use Language::*;
        assert_eq!(enter(Rust, "fn f() {|}"), "fn f() {\n    |\n}");
        assert_eq!(
            enter(Rust, "    g(a,|"),
            "    g(a,\n        |",
            "Rust's indentationRules"
        );
        assert_eq!(enter(Rust, "    x;|}"), "    x;\n|}");
        assert_eq!(enter(Rust, "let s = \"{|\";"), "let s = \"{\n|\";", "in a string");
        assert_eq!(enter(Rust, "    x; // {|"), "    x; // {\n    |", "in a line_comment");
        assert_eq!(enter(Json, "{|}"), "{\n    |\n}");
        assert_eq!(enter(Json, "  \"a\": [|"), "  \"a\": [\n      |");
        assert_eq!(enter(Json, "  \"a\": \"[|\""), "  \"a\": \"[\n  |\"", "in a string");
        assert_eq!(enter(Json, "  f(a,|"), "  f(a,\n  |", "no C rules");
        assert_eq!(enter(Toml, "x = [|]"), "x = [\n    |\n]");
        assert_eq!(enter(ShellScript, "f() {|"), "f() {\n    |");
        assert_eq!(enter(ShellScript, "echo '{|'"), "echo '{\n|'");
        assert_eq!(enter(CMake, "set(|"), "set(\n    |");
        assert_eq!(enter(CMake, "set(X|"), "set(X\n|", "only after the bracket");
        assert_eq!(enter(Xml, "<a>|</a>"), "<a>\n|</a>");
        // YAML: after a key or a dash, not after a value; a comment is skipped.
        assert_eq!(enter(Yaml, "on:|"), "on:\n    |");
        assert_eq!(enter(Yaml, "  push:  # c|"), "  push:  # c\n      |");
        assert_eq!(enter(Yaml, "  -|"), "  -\n      |");
        assert_eq!(enter(Yaml, "  a: b|"), "  a: b\n  |");
        assert_eq!(enter(Yaml, "  a: [|]"), "  a: [\n      |\n  ]");
        // Makefiles indent with tabs whatever the setting (VS Code's `[makefile]` default).
        assert_eq!(enter(Makefile, "x = $(|)"), "x = $(\n\t|\n)");
        assert_eq!(enter(Makefile, "\tcc a.c|"), "\tcc a.c\n\t|");
        // Markdown: brackets from the text, with a tree too.
        assert_eq!(enter(Markdown, "- a (|)"), "- a (\n    |\n)");
        assert_eq!(enter(Markdown, "x [|"), "x [\n    |");
        // Plain-text rules stay off elsewhere: no Python `:` outside Python.
        assert_eq!(enter(Rust, "  if x:|"), "  if x:\n  |");
    }

    #[test]
    fn closing_bracket_outdents_in_other_languages() {
        use Language::*;
        assert_eq!(
            typed(Rust, "fn f() {\n    x;\n    |", "}", true),
            "fn f() {\n    x;\n}|"
        );
        assert_eq!(typed(Json, "{\n  \"a\": [\n      |", "]", true), "{\n  \"a\": [\n  ]|");
        assert_eq!(typed(Markdown, "(\n    |", ")", true), "(\n)|");
        assert_eq!(typed(Xml, "<a\n    |", ">", true), "<a\n>|");
        // Not in a multi-line attribute value (`AttValue`, a string by `scope_of`).
        assert_eq!(typed(Xml, "<a x=\"\n    |\"/>", ">", true), "<a x=\"\n    >|\"/>");
        // Not in a Rust block comment (its text is in no leaf).
        assert_eq!(
            typed(Rust, "fn f() {\n    /*\n    |\n    */\n}", "}", true),
            "fn f() {\n    /*\n    }|\n    */\n}"
        );
    }

    #[test]
    fn unindent_to_the_previous_stop() {
        assert_eq!(unindent("        ", 4), "    ");
        assert_eq!(unindent("      ", 4), "    ");
        assert_eq!(unindent("\t\t", 4), "\t");
        assert_eq!(unindent("\t  ", 4), "\t");
        assert_eq!(unindent("", 4), "");
    }
}
