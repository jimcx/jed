//! Hover (no drawing; `ui::hover` draws the popup): a `textDocument/hover` answer read into
//! lines, simple markdown, wrapping, and the popup's state.
//!
//! - `parse` takes `MarkupContent` (markdown or plain text), a legacy `MarkedString` (a markdown
//!   string, or `{ language, value }`: a code block) or an array of them (sections separated by
//!   a rule), and the optional `range`. Nothing to show (null, empty) is `None`.
//! - Markdown is rendered simply, line by line (no paragraph joining: servers' hover text is
//!   line oriented): fenced code blocks are `Kind::Code` lines (kept verbatim, tabs as four
//!   spaces; not syntax highlighted), `#` headings `Kind::Heading`, `---` / `***` / `___` a
//!   `Kind::Rule`; inline code spans keep their text and are marked (`Line::code`), `**` is
//!   dropped, links show their text, backslash escapes and the common HTML entities are
//!   unescaped, list markers become "•", a quote's `>` is dropped; blank lines are collapsed
//!   and trimmed around rules and at the ends. Single `*` / `_` stay (`*args`, `snake_case`).
//! - `diagnostic_lines` puts the diagnostics under the pointer / cursor in the popup before the
//!   server's answer (VS Code's): per diagnostic, most severe first, a `Kind::Diagnostic` line
//!   (the severity icon, then the message's first line), the message's other lines (plain text,
//!   kept) and a `Kind::Source` line (`source(code)`, drawn dimmed), each indented past the icon;
//!   several separated by a rule.
//! - `wrap` breaks lines to a width (text at spaces, else anywhere; code anywhere) into `Row`s
//!   that point into the lines.

use std::ops::Range;
use std::time::Instant;

use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::buffer::BufferId;
use crate::buffer::pos::{ByteIdx, LspPos};
use crate::diagnostics::{Diagnostic, Severity};
use crate::layout::GroupId;

/// How long the mouse rests on text before its hover is asked for (VS Code's `hover.delay`).
pub const DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// The popup's widest content (cells; VS Code's hover is ~500 px).
pub const MAX_WIDTH: usize = 72;
/// Its tallest content (rows) before it scrolls.
pub const MAX_HEIGHT: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Heading,
    Code,
    Rule,
    /// A diagnostic's first line: its severity's icon (drawn in that colour), then its message.
    Diagnostic(Severity),
    /// Who reports a diagnostic, `source(code)` (drawn dimmed).
    Source,
    /// A clickable row (the diagnostics' "Quick Fix…", in the link colour).
    Link,
}

/// One line of hover content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub kind: Kind,
    /// Inline code spans: byte ranges of `text`, in order.
    pub code: Vec<Range<usize>>,
}

impl Line {
    fn new(kind: Kind, text: String) -> Self {
        Self {
            text,
            kind,
            code: Vec::new(),
        }
    }

    /// A rule between sections.
    pub fn rule() -> Self {
        Self::new(Kind::Rule, String::new())
    }

    /// A clickable row.
    pub fn link(text: &str) -> Self {
        Self::new(Kind::Link, text.to_string())
    }

    fn is_blank(&self) -> bool {
        self.kind != Kind::Rule && self.text.trim().is_empty()
    }
}

/// A hover answer, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Hover {
    pub lines: Vec<Line>,
    /// The range the hover is about, if the server said.
    pub range: Option<(LspPos, LspPos)>,
}

/// Reads a `textDocument/hover` result; `None` when there's nothing to show.
pub fn parse(result: &Value) -> Option<Hover> {
    let contents = result.get("contents")?;
    let mut lines = Vec::new();
    read_contents(contents, &mut lines);
    let lines = tidy(lines);
    if lines.iter().all(|l| l.is_blank() || l.kind == Kind::Rule) {
        return None;
    }
    let pos = |p: &Value| {
        Some(LspPos {
            line: p.get("line")?.as_u64()? as u32,
            character: p.get("character")?.as_u64()? as u32,
        })
    };
    let range = result
        .get("range")
        .and_then(|r| Some((pos(r.get("start")?)?, pos(r.get("end")?)?)))
        .filter(|(s, e)| s <= e);
    Some(Hover { lines, range })
}

fn read_contents(v: &Value, out: &mut Vec<Line>) {
    match v {
        Value::String(s) => out.extend(markdown(s)),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(Line::new(Kind::Rule, String::new()));
                }
                read_contents(item, out);
            }
        }
        Value::Object(o) => {
            let value = o.get("value").and_then(Value::as_str).unwrap_or("");
            match (o.get("kind").and_then(Value::as_str), o.get("language")) {
                (Some("markdown"), _) => out.extend(markdown(value)),
                (Some(_), _) => out.extend(plaintext(value)),
                // `MarkedString { language, value }`: a code block.
                (None, Some(_)) => out.extend(value.lines().map(|l| Line::new(Kind::Code, code_text(l)))),
                (None, None) => out.extend(plaintext(value)),
            }
        }
        _ => {}
    }
}

/// Plain text: its lines as they are (control characters as spaces).
pub fn plaintext(text: &str) -> Vec<Line> {
    text.lines().map(|l| Line::new(Kind::Text, clean(l))).collect()
}

/// Markdown, rendered simply (see the module doc).
pub fn markdown(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(f) && trimmed.trim_end().chars().all(|c| c == f.as_bytes()[0] as char) {
                fence = None;
            } else {
                out.push(Line::new(Kind::Code, code_text(raw)));
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        let t = trimmed.trim_end();
        let rule = |m: char| t.chars().all(|c| c == m || c == ' ') && t.chars().filter(|&c| c == m).count() >= 3;
        if rule('-') || rule('*') || rule('_') {
            out.push(Line::new(Kind::Rule, String::new()));
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let mut l = inline(trimmed[hashes..].trim());
            l.kind = Kind::Heading;
            out.push(l);
            continue;
        }
        let indent = &raw[..raw.len() - trimmed.len()];
        let mut body = trimmed;
        while let Some(rest) = body.strip_prefix('>') {
            body = rest.trim_start();
        }
        let mut prefix = clean(indent);
        for marker in ["- ", "* ", "+ "] {
            if let Some(rest) = body.strip_prefix(marker) {
                prefix.push_str("• ");
                body = rest;
                break;
            }
        }
        // A hard break (two trailing spaces or a backslash) is a line end here anyway.
        let body = body.trim_end();
        let body = body.strip_suffix('\\').unwrap_or(body);
        let mut l = inline(body);
        if !prefix.is_empty() {
            l.text.insert_str(0, &prefix);
            for r in &mut l.code {
                *r = r.start + prefix.len()..r.end + prefix.len();
            }
        }
        out.push(l);
    }
    out
}

/// Inline markdown: code spans (kept, marked), `**` dropped, links as their text, escapes and
/// entities unescaped.
fn inline(s: &str) -> Line {
    let mut line = Line::new(Kind::Text, String::new());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            let ticks = rest.bytes().take_while(|&b| b == b'`').count();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                let inner = &rest[ticks..ticks + end];
                // One space padding on both sides is dropped (CommonMark).
                let inner = match inner.strip_prefix(' ').and_then(|i| i.strip_suffix(' ')) {
                    Some(i) if !i.trim().is_empty() => i,
                    _ => inner,
                };
                let start = line.text.len();
                line.text.push_str(&clean(inner));
                line.code.push(start..line.text.len());
                rest = &rest[2 * ticks + end..];
                continue;
            }
            line.text.push_str(fence);
            rest = &rest[ticks..];
            continue;
        }
        if c == '\\'
            && let Some(next) = rest[1..].chars().next()
            && next.is_ascii_punctuation()
        {
            line.text.push(next);
            rest = &rest[1 + next.len_utf8()..];
            continue;
        }
        if rest.starts_with("**") {
            rest = &rest[2..];
            continue;
        }
        if (c == '[' || rest.starts_with("!["))
            && let Some((text, len)) = link(rest)
        {
            line.text.push_str(&inline(text).text);
            rest = &rest[len..];
            continue;
        }
        if c == '&'
            && let Some((ch, len)) = entity(rest)
        {
            line.text.push(ch);
            rest = &rest[len..];
            continue;
        }
        line.text.push(if c.is_control() { ' ' } else { c });
        rest = &rest[c.len_utf8()..];
    }
    line
}

/// `[text](url)` / `![alt](url)` at the start of `s`: the text and the length of the whole.
fn link(s: &str) -> Option<(&str, usize)> {
    let open = if s.starts_with('!') { 2 } else { 1 };
    let close = s[open..].find("](")? + open;
    let end = s[close + 2..].find(')')? + close + 2;
    Some((&s[open..close], end + 1))
}

fn entity(s: &str) -> Option<(char, usize)> {
    for (name, c) in [
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&amp;", '&'),
        ("&quot;", '"'),
        ("&#39;", '\''),
        ("&nbsp;", ' '),
    ] {
        if s.starts_with(name) {
            return Some((c, name.len()));
        }
    }
    None
}

/// The hover's diagnostics section (see the module doc); empty for none.
pub fn diagnostic_lines(diags: &[&Diagnostic]) -> Vec<Line> {
    let mut sorted = diags.to_vec();
    sorted.sort_by_key(|d| d.severity);
    let mut out = Vec::new();
    for (i, d) in sorted.iter().enumerate() {
        if i > 0 {
            out.push(Line::new(Kind::Rule, String::new()));
        }
        let icon = d.severity.icon();
        let indent = " ".repeat(icon.width() + 1);
        let message = d.message.trim_end();
        let mut lines = message.lines();
        let first = lines.next().unwrap_or("");
        let first = format!("{icon} {}", clean(first.trim_end()));
        out.push(Line::new(Kind::Diagnostic(d.severity), first.trim_end().to_string()));
        out.extend(lines.map(|l| {
            let text = if l.trim().is_empty() {
                String::new()
            } else {
                format!("{indent}{}", clean(l.trim_end()))
            };
            Line::new(Kind::Text, text)
        }));
        let source = match (&d.source, &d.code) {
            (Some(s), Some(c)) => Some(format!("{s}({c})")),
            (Some(s), None) => Some(s.clone()),
            (None, Some(c)) => Some(c.clone()),
            (None, None) => None,
        };
        if let Some(source) = source {
            out.push(Line::new(Kind::Source, format!("{indent}{}", clean(&source))));
        }
    }
    out
}

/// Control characters as spaces.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// A code line: tabs as four spaces, other control characters as spaces.
fn code_text(s: &str) -> String {
    clean(&s.replace('\t', "    "))
}

/// Collapses runs of blank lines, drops blank lines next to rules, repeated rules and both ends'
/// blanks and rules.
fn tidy(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for l in lines {
        let blank = l.is_blank() && l.kind != Kind::Code;
        match out.last() {
            None if blank || l.kind == Kind::Rule => continue,
            Some(prev) if blank && (prev.is_blank() && prev.kind != Kind::Code || prev.kind == Kind::Rule) => continue,
            Some(prev) if l.kind == Kind::Rule && prev.kind == Kind::Rule => continue,
            Some(prev) if l.kind == Kind::Rule && prev.is_blank() && prev.kind != Kind::Code => {
                out.pop();
            }
            _ => {}
        }
        out.push(l);
    }
    while out
        .last()
        .is_some_and(|l| l.kind == Kind::Rule || (l.is_blank() && l.kind != Kind::Code))
    {
        out.pop();
    }
    out
}

/// The widest line, in cells.
pub fn width(lines: &[Line]) -> usize {
    lines
        .iter()
        .filter(|l| l.kind != Kind::Rule)
        .map(|l| l.text.width())
        .max()
        .unwrap_or(0)
}

/// One screen row of wrapped content: `lines[line].text[range]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub range: Range<usize>,
}

/// Breaks `lines` into rows at most `width` cells wide: text at the last space that fits (the
/// space ends the row), else at the last grapheme that fits; code at the last grapheme that fits.
pub fn wrap(lines: &[Line], width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.kind == Kind::Rule || l.text.is_empty() {
            rows.push(Row { line: i, range: 0..0 });
            continue;
        }
        let text = l.text.as_str();
        let mut start = 0;
        while start < text.len() {
            let mut cells = 0;
            let mut end = text.len();
            let mut space = None;
            for (off, g) in text[start..].grapheme_indices(true) {
                let w = g.width();
                if cells + w > width && off > 0 {
                    end = start + off;
                    break;
                }
                cells += w;
                if g == " " && l.kind != Kind::Code {
                    space = Some(start + off);
                }
            }
            // At a space (which isn't drawn at the row's end), else after the last one that fits.
            let (row_end, next) = if end >= text.len() {
                (end, end)
            } else if l.kind != Kind::Code && text[end..].starts_with(' ') {
                (end, end + 1)
            } else {
                match space {
                    Some(s) if s > start => (s, s + 1),
                    _ => (end, end),
                }
            };
            rows.push(Row {
                line: i,
                range: start..row_end,
            });
            start = next;
        }
    }
    rows
}

/// The hover popup on screen.
#[derive(Debug, Clone)]
pub struct Popup {
    /// The editor group whose view it's drawn over, and the buffer its tab shows.
    pub group: GroupId,
    pub buffer: BufferId,
    /// The buffer's revision when it opened: any edit closes it.
    pub revision: u64,
    /// The view's scroll when it opened (top line, left column): scrolling closes it.
    pub scroll: (usize, usize),
    /// Where it's drawn from: the start of the server's range, else of the word.
    pub anchor: ByteIdx,
    /// What it's about (the server's range or the word, and the diagnostics' squiggles): the
    /// mouse may move within it.
    pub range: (ByteIdx, ByteIdx),
    pub lines: Vec<Line>,
    /// Where the diagnostics section ends in `lines` (the "Quick Fix…" row goes there).
    pub fixes_at: usize,
    /// The union of the shown diagnostics' drawn ranges (`None`: none shown): fixes that come
    /// later get their row only while they're for these.
    pub diagnostics: Option<(ByteIdx, ByteIdx)>,
    /// The screen row the "Quick Fix…" row was drawn on (set by drawing).
    pub link_row: Option<u16>,
    /// Opened by the mouse resting on text (moving off closes it), else by the command.
    pub mouse: bool,
    /// The first row shown (the wheel scrolls a long one).
    pub top: usize,
    /// Rows in all and rows shown in the last frame (set by drawing).
    pub rows: usize,
    pub shown: usize,
}

impl Popup {
    /// Scrolls its content by `delta` rows, within what the last frame drew.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.rows.saturating_sub(self.shown);
        self.top = self.top.saturating_add_signed(delta).min(max);
    }
}

/// The mouse resting on text: once `deadline` passes, its hover is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rest {
    pub deadline: Instant,
    pub group: GroupId,
    pub buffer: BufferId,
    pub at: ByteIdx,
    /// The run of same-class characters under the pointer: moving within it keeps the timer.
    pub word: (ByteIdx, ByteIdx),
}

/// The hover popup and the mouse's resting timer.
#[derive(Debug, Default)]
pub struct HoverState {
    pub popup: Option<Popup>,
    pub rest: Option<Rest>,
    /// Where the mouse is over text (group, buffer, the run under it), from its last move.
    pub mouse: Option<(GroupId, BufferId, (ByteIdx, ByteIdx))>,
}

impl HoverState {
    pub fn deadline(&self) -> Option<Instant> {
        self.rest.as_ref().map(|r| r.deadline)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| match l.kind {
                Kind::Rule => "---".to_string(),
                Kind::Code => format!("code:{}", l.text),
                Kind::Heading => format!("h:{}", l.text),
                Kind::Text => l.text.clone(),
                Kind::Diagnostic(s) => format!("{s:?}:{}", l.text),
                Kind::Source => format!("src:{}", l.text),
                Kind::Link => format!("link:{}", l.text),
            })
            .collect()
    }

    /// clangd's and ty's shapes: headings, rules, fences, inline code, escapes.
    #[test]
    fn markdown_renders_simply() {
        let md = "### function `foo`  \n\n---\n→ `int`  \nParameters:  \n- `int x`\n\n---\n```cpp\n// In namespace ns\nint\tfoo(int x)\n```";
        let lines = markdown(md);
        assert_eq!(
            texts(&tidy(lines.clone())),
            [
                "h:function foo",
                "---",
                "→ int",
                "Parameters:",
                "• int x",
                "---",
                "code:// In namespace ns",
                "code:int    foo(int x)"
            ]
        );
        let heading = &lines[0];
        assert_eq!(&heading.text[heading.code[0].clone()], "foo");
        let item = lines.iter().find(|l| l.text.starts_with('•')).unwrap();
        assert_eq!(
            &item.text[item.code[0].clone()],
            "int x",
            "ranges moved past the bullet"
        );

        let l = inline(r"a \*b\* **bold** [link](http://x) &lt;T&gt; `` a`b `` snake_case *args");
        assert_eq!(l.text, "a *b* bold link <T> a`b snake_case *args");
        assert_eq!(&l.text[l.code[0].clone()], "a`b");
        assert_eq!(inline("unclosed `tick").text, "unclosed `tick");
        assert_eq!(texts(&markdown("> quoted\n~~~\nx\n~~~")), ["quoted", "code:x"]);
    }

    #[test]
    fn parse_takes_every_shape() {
        let h = parse(&json!({
            "contents": { "kind": "markdown", "value": "**x**: `int`" },
            "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 3 } },
        }))
        .unwrap();
        assert_eq!(texts(&h.lines), ["x: int"]);
        assert_eq!(
            h.range,
            Some((LspPos { line: 1, character: 2 }, LspPos { line: 1, character: 3 }))
        );
        let h = parse(&json!({ "contents": { "kind": "plaintext", "value": "a **b**\n\n\n\nc" } })).unwrap();
        assert_eq!(texts(&h.lines), ["a **b**", "", "c"], "plain text isn't markdown");
        let h = parse(&json!({ "contents": [{ "language": "cpp", "value": "int x" }, "*doc*"] })).unwrap();
        assert_eq!(texts(&h.lines), ["code:int x", "---", "*doc*"]);
        assert_eq!(
            texts(&parse(&json!({ "contents": "plain `md`" })).unwrap().lines),
            ["plain md"]
        );
        assert!(parse(&Value::Null).is_none());
        assert!(parse(&json!({ "contents": "" })).is_none());
        assert!(parse(&json!({ "contents": [] })).is_none());
        assert!(parse(&json!({ "contents": "\n---\n\n" })).is_none());
        // Rules: one between sections however many (and blank lines) there are; none at the ends.
        let h = parse(&json!({ "contents": ["a\n---\n", "\n***\n\nb\n\n\n", "---\n"] })).unwrap();
        assert_eq!(texts(&h.lines), ["a", "---", "b"]);
    }

    fn diagnostic(severity: Severity, message: &str, source: Option<&str>, code: Option<&str>) -> Diagnostic {
        Diagnostic {
            severity,
            message: message.into(),
            source: source.map(Into::into),
            code: code.map(Into::into),
            raw: Value::Null,
        }
    }

    /// Most severe first (stable among equals), the icon before the message, its other lines
    /// kept and indented past the icon, then `source(code)`; a rule between diagnostics.
    #[test]
    fn diagnostic_lines_show_icon_message_and_source() {
        let warn = diagnostic(Severity::Warning, "unused import\n\nremove it  \n", Some("rustc"), None);
        let err = diagnostic(
            Severity::Error,
            "Name `x` used\twhen not defined",
            Some("ty"),
            Some("unresolved-reference"),
        );
        let hint = diagnostic(Severity::Hint, "", None, Some("E0425"));
        let info = diagnostic(Severity::Info, "first info", None, None);
        let info2 = diagnostic(Severity::Info, "second info", None, None);
        let lines = diagnostic_lines(&[&warn, &info, &hint, &err, &info2]);
        assert_eq!(
            texts(&lines),
            [
                "Error:⊗ Name `x` used when not defined",
                "src:  ty(unresolved-reference)",
                "---",
                "Warning:⚠ unused import",
                "",
                "  remove it",
                "src:  rustc",
                "---",
                "Info:ⓘ first info",
                "---",
                "Info:ⓘ second info",
                "---",
                "Hint:…",
                "src:  E0425",
            ]
        );
        assert!(lines.iter().all(|l| l.code.is_empty()), "plain text, not markdown");
        assert!(diagnostic_lines(&[]).is_empty());
        // Wrapped as text lines are (at spaces).
        let long = diagnostic(Severity::Error, "aaa bbb ccc", None, None);
        let lines = diagnostic_lines(&[&long]);
        let rows = wrap(&lines, 6);
        let shown: Vec<&str> = rows.iter().map(|r| &lines[r.line].text[r.range.clone()]).collect();
        assert_eq!(shown, ["⊗ aaa", "bbb", "ccc"]);
        assert_eq!(width(&lines), 13);
    }

    #[test]
    fn wrapping_breaks_text_at_spaces_and_code_anywhere() {
        let lines = vec![
            Line::new(Kind::Text, "one two three".into()),
            Line::new(Kind::Code, "abcdefgh".into()),
            Line::new(Kind::Text, "averyveryverylongword x".into()),
            Line::new(Kind::Rule, String::new()),
            Line::new(Kind::Text, "日本語です".into()),
        ];
        let rows = wrap(&lines, 7);
        let shown: Vec<&str> = rows.iter().map(|r| &lines[r.line].text[r.range.clone()]).collect();
        assert_eq!(
            shown,
            [
                "one two",
                "three",
                "abcdefg",
                "h",
                "averyve",
                "ryveryl",
                "ongword",
                "x",
                "",
                "日本語",
                "です"
            ]
        );
        assert_eq!(width(&lines), 23);
    }
}
