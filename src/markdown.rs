//! The Markdown preview's model: CommonMark + GFM (tables, strikethrough, task lists, YAML front
//! matter) parsed with `pulldown-cmark` and laid out to a width as lines of styled spans. No app
//! state and nothing drawn: spans carry a theme key (`markdown.*`, see `Theme::dark_plus`) plus
//! bold / italic / strikethrough flags, and `ui` turns them into styles. `render(text, width)` is
//! called again when the width changes. Code blocks and tables are plain placeholders
//! (`Builder::code_block`, `Builder::table`) until the next subtask.
#![allow(dead_code)] // used by the preview tab (markdown_preview-c)

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// How a span is drawn: a theme key and inline flags (the theme can't express nesting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub key: &'static str,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

impl SpanStyle {
    pub const fn key(key: &'static str) -> Self {
        Self {
            key,
            bold: false,
            italic: false,
            strike: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
}

impl Span {
    fn new(text: impl Into<String>, key: &'static str) -> Self {
        Span {
            text: text.into(),
            style: SpanStyle::key(key),
        }
    }
}

/// What a line is, for the UI (copy, following the source, extra drawing such as a code background).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Blank,
    Text,
    /// A heading's text lines (level 1-6).
    Heading(u8),
    /// The `═` / `─` line under an H1 / H2.
    HeadingRule(u8),
    Rule,
    Code,
    Html,
    FrontMatter,
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    pub kind: Kind,
}

impl Line {
    /// The line's plain text (what copy puts on the clipboard).
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Display width in columns.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub lines: Vec<Line>,
}

/// Parses `text` and lays it out to `width` columns (at least 1).
pub fn render(text: &str, width: usize) -> Doc {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut b = Builder::new(width.max(1));
    for ev in Parser::new_ext(text, opts) {
        b.event(ev);
    }
    b.flush_run(Kind::Text);
    Doc { lines: b.lines }
}

type Chars = Vec<(char, SpanStyle)>;

enum Cont {
    Quote {
        used: bool,
    },
    Item {
        marker: Vec<Span>,
        width: usize,
        used: bool,
        first: bool,
    },
}

enum Inline {
    Link { url: String, start: usize },
    Image { url: String },
}

struct ListB {
    /// The next number (ordered) or `None` (bullets).
    next: Option<u64>,
    items: usize,
}

#[derive(Default)]
struct TableB {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span>>>,
    row: Vec<Vec<Span>>,
}

struct Builder {
    width: usize,
    lines: Vec<Line>,
    stack: Vec<Cont>,
    run: Chars,
    bold: u32,
    italic: u32,
    strike: u32,
    heading: Option<u8>,
    inline: Vec<Inline>,
    lists: Vec<ListB>,
    last_blank: bool,
    code: Option<(String, String)>,
    html: Option<String>,
    meta: Option<String>,
    table: Option<TableB>,
}

fn level(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn heading_key(n: u8) -> &'static str {
    [
        "markdown.h1",
        "markdown.h2",
        "markdown.h3",
        "markdown.h4",
        "markdown.h5",
        "markdown.h6",
    ][usize::from(n - 1)]
}

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

impl Builder {
    fn new(width: usize) -> Self {
        Builder {
            width,
            lines: Vec::new(),
            stack: Vec::new(),
            run: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            inline: Vec::new(),
            lists: Vec::new(),
            last_blank: false,
            code: None,
            html: None,
            meta: None,
            table: None,
        }
    }

    fn cur(&self) -> SpanStyle {
        let key = if !self.inline.is_empty() {
            "markdown.link"
        } else if let Some(n) = self.heading {
            heading_key(n)
        } else {
            "markdown.text"
        };
        SpanStyle {
            key,
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
        }
    }

    fn push_text(&mut self, text: &str, style: SpanStyle) {
        self.run
            .extend(text.chars().map(|c| (if c.is_whitespace() { ' ' } else { c }, style)));
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((_, body)) = &mut self.code {
                    body.push_str(&t);
                } else if let Some(h) = &mut self.html {
                    h.push_str(&t);
                } else if let Some(m) = &mut self.meta {
                    m.push_str(&t);
                } else {
                    let st = self.cur();
                    self.push_text(&t, st);
                }
            }
            Event::Code(t) => {
                let mut st = self.cur();
                st.key = "markdown.code";
                self.push_text(&t, st);
            }
            Event::Html(t) => {
                if let Some(h) = &mut self.html {
                    h.push_str(&t);
                }
            }
            Event::InlineHtml(t) => {
                let mut st = self.cur();
                st.key = "markdown.muted";
                self.push_text(&t, st);
            }
            Event::SoftBreak => {
                let st = self.cur();
                self.run.push((' ', st));
            }
            Event::HardBreak => {
                let st = self.cur();
                self.run.push(('\n', st));
            }
            Event::Rule => {
                self.flush_run(Kind::Text);
                self.gap();
                let w = self.avail();
                self.emit(vec![Span::new("─".repeat(w), "markdown.rule")], Kind::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Cont::Item { marker, width, .. }) = self.stack.last_mut() {
                    let m = if done { "☑ " } else { "☐ " };
                    *marker = vec![Span::new(m, "markdown.bullet")];
                    *width = m.width();
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.flush_run(Kind::Text);
                // A loose list's later items are separated by a blank line.
                if matches!(
                    self.stack.last(),
                    Some(Cont::Item {
                        used: false,
                        first: false,
                        ..
                    })
                ) {
                    if !self.last_blank {
                        self.blank();
                    }
                } else {
                    self.gap();
                }
            }
            Tag::Heading { level: l, .. } => {
                self.flush_run(Kind::Text);
                self.gap();
                let n = level(l);
                self.heading = Some(n);
                if n >= 3 {
                    self.push_text(
                        &format!("{} ", "#".repeat(usize::from(n))),
                        SpanStyle::key("markdown.muted"),
                    );
                }
            }
            Tag::BlockQuote(_) => {
                self.flush_run(Kind::Text);
                self.gap();
                self.stack.push(Cont::Quote { used: false });
            }
            Tag::CodeBlock(kind) => {
                self.flush_run(Kind::Text);
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush_run(Kind::Text);
                self.html = Some(String::new());
            }
            Tag::MetadataBlock(_) => self.meta = Some(String::new()),
            Tag::List(start) => {
                self.flush_run(Kind::Text);
                if !matches!(self.stack.last(), Some(Cont::Item { .. })) {
                    self.gap();
                }
                self.lists.push(ListB { next: start, items: 0 });
            }
            Tag::Item => {
                self.flush_run(Kind::Text);
                let depth = self.lists.len().saturating_sub(1);
                let (marker, first) = match self.lists.last_mut() {
                    Some(l) => {
                        l.items += 1;
                        let m = match &mut l.next {
                            Some(n) => {
                                let m = format!("{n}. ");
                                *n += 1;
                                m
                            }
                            None => format!("{} ", ["•", "◦", "▪"][depth % 3]),
                        };
                        (m, l.items == 1)
                    }
                    None => ("• ".to_string(), true),
                };
                self.stack.push(Cont::Item {
                    width: marker.width(),
                    marker: vec![Span::new(marker, "markdown.bullet")],
                    used: false,
                    first,
                });
            }
            Tag::Table(aligns) => {
                self.flush_run(Kind::Text);
                self.table = Some(TableB {
                    aligns,
                    ..Default::default()
                });
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                let start = self.run.len();
                self.inline.push(Inline::Link {
                    url: dest_url.to_string(),
                    start,
                });
            }
            Tag::Image { dest_url, .. } => {
                self.push_text("[", SpanStyle::key("markdown.link"));
                self.inline.push(Inline::Image {
                    url: dest_url.to_string(),
                });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_run(Kind::Text),
            TagEnd::Heading(l) => {
                let n = level(l);
                self.flush_run(Kind::Heading(n));
                self.heading = None;
                if n <= 2 {
                    let ch = if n == 1 { "═" } else { "─" };
                    let w = self.avail();
                    self.emit(vec![Span::new(ch.repeat(w), heading_key(n))], Kind::HeadingRule(n));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_run(Kind::Text);
                self.stack.pop();
            }
            TagEnd::CodeBlock => {
                if let Some((lang, body)) = self.code.take() {
                    self.gap();
                    self.code_block(&lang, &body);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(h) = self.html.take() {
                    self.gap();
                    self.raw_lines(h.trim_end_matches('\n'), "markdown.muted", Kind::Html);
                }
            }
            TagEnd::MetadataBlock(_) => {
                if let Some(m) = self.meta.take() {
                    self.gap();
                    self.raw_lines(m.trim_end_matches('\n'), "markdown.muted", Kind::FrontMatter);
                }
            }
            TagEnd::List(_) => {
                self.flush_run(Kind::Text);
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush_run(Kind::Text);
                self.stack.pop();
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.gap();
                    self.table(&t.aligns, &t.rows);
                }
            }
            TagEnd::TableCell => {
                let run = std::mem::take(&mut self.run);
                if let Some(t) = &mut self.table {
                    t.row.push(to_spans(&run));
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(Inline::Link { url, start }) = self.inline.pop() {
                    let same = self.run[start..].iter().map(|(c, _)| *c).eq(url.chars());
                    if !same && !url.is_empty() {
                        self.push_text(&format!(" ({url})"), SpanStyle::key("markdown.url"));
                    }
                }
            }
            TagEnd::Image => {
                if let Some(Inline::Image { url }) = self.inline.pop() {
                    self.push_text("]", SpanStyle::key("markdown.link"));
                    if !url.is_empty() {
                        self.push_text(&format!(" ({url})"), SpanStyle::key("markdown.url"));
                    }
                }
            }
            _ => {}
        }
    }

    // ---- placeholders for the next subtask ----

    /// Placeholder: the source lines in code style. `lang` is the fence's first word.
    fn code_block(&mut self, _lang: &str, body: &str) {
        self.raw_lines(body.trim_end_matches('\n'), "markdown.code", Kind::Code);
    }

    /// Placeholder: one `a | b` line per row.
    fn table(&mut self, _aligns: &[Alignment], rows: &[Vec<Vec<Span>>]) {
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .map(|c| c.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect();
            self.raw_lines(&cells.join(" | "), "markdown.text", Kind::Table);
        }
    }

    // ---- layout ----

    /// Columns left for content inside the containers.
    fn avail(&self) -> usize {
        let used: usize = self
            .stack
            .iter()
            .map(|c| match c {
                Cont::Quote { .. } => 2,
                Cont::Item { width, .. } => *width,
            })
            .sum();
        self.width.saturating_sub(used).max(1)
    }

    /// Pushes a line behind the containers' prefix (a list marker the first time).
    fn emit(&mut self, content: Vec<Span>, kind: Kind) {
        let mut spans = Vec::new();
        for c in &mut self.stack {
            match c {
                Cont::Quote { used } => {
                    *used = true;
                    spans.push(Span::new("▎ ", "markdown.quote"));
                }
                Cont::Item {
                    marker, width, used, ..
                } => {
                    if *used {
                        spans.push(Span::new(" ".repeat(*width), "markdown.text"));
                    } else {
                        *used = true;
                        spans.extend(marker.iter().cloned());
                    }
                }
            }
        }
        spans.extend(content);
        self.lines.push(Line { spans, kind });
        self.last_blank = false;
    }

    /// A blank line, keeping the quote gutters.
    fn blank(&mut self) {
        let mut spans: Vec<Span> = Vec::new();
        for c in &self.stack {
            match c {
                Cont::Quote { .. } => spans.push(Span::new("▎ ", "markdown.quote")),
                Cont::Item { width, .. } => spans.push(Span::new(" ".repeat(*width), "markdown.text")),
            }
        }
        while let Some(last) = spans.last_mut() {
            let t = last.text.trim_end().len();
            last.text.truncate(t);
            if last.text.is_empty() {
                spans.pop();
            } else {
                break;
            }
        }
        self.lines.push(Line {
            spans,
            kind: Kind::Blank,
        });
        self.last_blank = true;
    }

    /// The blank line between blocks (none at the start, twice in a row, or right after a marker
    /// or quote gutter that has nothing yet).
    fn gap(&mut self) {
        let fresh = matches!(
            self.stack.last(),
            Some(Cont::Item { used: false, .. } | Cont::Quote { used: false })
        );
        if self.lines.is_empty() || self.last_blank || fresh {
            return;
        }
        self.blank();
    }

    /// Wraps the pending inline run into lines.
    fn flush_run(&mut self, kind: Kind) {
        if self.run.is_empty() || self.table.is_some() {
            return; // a table cell's run is taken by `TagEnd::TableCell`
        }
        let run = std::mem::take(&mut self.run);
        let avail = self.avail();
        for line in wrap(&run, avail) {
            self.emit(to_spans(&line), kind);
        }
    }

    /// Source-like lines, cut at the width.
    fn raw_lines(&mut self, text: &str, key: &'static str, kind: Kind) {
        let avail = self.avail();
        for l in text.split('\n') {
            for piece in hard_wrap(&l.replace('\t', "    "), avail) {
                self.emit(vec![Span::new(piece, key)], kind);
            }
        }
    }
}

fn to_spans(chars: &[(char, SpanStyle)]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for &(c, st) in chars {
        match out.last_mut() {
            Some(s) if s.style == st => s.text.push(c),
            _ => out.push(Span {
                text: c.to_string(),
                style: st,
            }),
        }
    }
    out
}

/// Cuts `s` into pieces of at most `width` columns (at least one char each); "" stays one piece.
fn hard_wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut w = 0;
    for c in s.chars() {
        let cwid = cw(c);
        if w + cwid > width && w > 0 {
            out.push(String::new());
            w = 0;
        }
        out.last_mut().unwrap().push(c);
        w += cwid;
    }
    out
}

/// Greedy word wrap of a styled run: spaces collapse and break lines, `\n` forces a break, words
/// longer than `width` are cut. Lines never start with a space.
fn wrap(run: &[(char, SpanStyle)], width: usize) -> Vec<Chars> {
    let mut lines: Vec<Chars> = Vec::new();
    let mut cur: Chars = Vec::new();
    let mut curw = 0;
    let mut space: Option<SpanStyle> = None;
    let mut i = 0;
    while i < run.len() {
        let (c, st) = run[i];
        if c == '\n' {
            lines.push(std::mem::take(&mut cur));
            curw = 0;
            space = None;
            i += 1;
            continue;
        }
        if c == ' ' {
            if !cur.is_empty() && space.is_none() {
                space = Some(st);
            }
            i += 1;
            continue;
        }
        let mut j = i;
        while j < run.len() && run[j].0 != ' ' && run[j].0 != '\n' {
            j += 1;
        }
        let word = &run[i..j];
        let ww: usize = word.iter().map(|(c, _)| cw(*c)).sum();
        let sp = usize::from(space.is_some());
        if !cur.is_empty() && curw + sp + ww > width {
            lines.push(std::mem::take(&mut cur));
            curw = 0;
            space = None;
        }
        if let Some(s) = space.take() {
            cur.push((' ', s));
            curw += 1;
        }
        if ww <= width {
            cur.extend_from_slice(word);
            curw += ww;
        } else {
            for &(c, st) in word {
                let cwid = cw(c);
                if curw + cwid > width && curw > 0 {
                    lines.push(std::mem::take(&mut cur));
                    curw = 0;
                }
                cur.push((c, st));
                curw += cwid;
            }
        }
        i = j;
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str, w: usize) -> Vec<String> {
        render(src, w).lines.iter().map(Line::text).collect()
    }

    fn keys(l: &Line) -> Vec<&'static str> {
        l.spans.iter().map(|s| s.style.key).collect()
    }

    #[test]
    fn empty_and_unclosed() {
        assert!(render("", 20).lines.is_empty());
        assert!(render("\n\n", 20).lines.is_empty());
        for src in [
            "**bold",
            "`code",
            "[link](",
            "> > ",
            "- [ ",
            "```rust\nfn",
            "| a |\n|",
            "<div>",
            "---\nx: 1",
        ] {
            let d = render(src, 10);
            assert!(d.lines.iter().all(|l| l.width() <= 10), "{src:?}");
        }
    }

    #[test]
    fn heading_levels_are_styled_differently() {
        let src = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n";
        let d = render(src, 20);
        let t: Vec<String> = d.lines.iter().map(Line::text).collect();
        assert_eq!(t[0], "One");
        assert_eq!(t[1], "═".repeat(20));
        assert_eq!(t[2], "");
        assert_eq!(t[3], "Two");
        assert_eq!(t[4], "─".repeat(20));
        assert_eq!(t[6], "### Three");
        let mut seen = Vec::new();
        for l in &d.lines {
            if let Kind::Heading(n) = l.kind {
                let text = l.spans.last().unwrap();
                assert_eq!(text.style.key, heading_key(n));
                if n >= 3 {
                    assert_eq!(l.spans[0].style.key, "markdown.muted");
                }
                seen.push(text.style.key);
            }
        }
        seen.dedup();
        assert_eq!(seen.len(), 6);
        assert!(matches!(d.lines[1].kind, Kind::HeadingRule(1)));
        assert!(matches!(d.lines[4].kind, Kind::HeadingRule(2)));
    }

    #[test]
    fn setext_equals_atx() {
        assert_eq!(
            texts("Title\n=====\n\nSub\n---\n", 12),
            texts("# Title\n\n## Sub\n", 12)
        );
    }

    #[test]
    fn blank_before_heading_but_not_at_start() {
        let t = texts("text\n### H\n", 20);
        assert_eq!(t, ["text", "", "### H"]);
    }

    #[test]
    fn paragraph_wraps_to_width() {
        let t = texts("the quick brown fox jumps over the lazy dog", 12);
        assert_eq!(t, ["the quick", "brown fox", "jumps over", "the lazy dog"]);
        for w in 1..30 {
            for l in texts("a verylongwordindeed and some 世界世界 text here", w) {
                assert!(l.width() <= w.max(2), "{l:?} at {w}");
            }
        }
    }

    #[test]
    fn wide_chars_count_two_columns() {
        assert_eq!(texts("世界世界世界", 5), ["世界", "世界", "世界"]);
    }

    #[test]
    fn width_one_does_not_panic() {
        let src = "# H\n\n- a b\n  - c\n\n> q\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```\ncode\n```\n";
        let d = render(src, 1);
        assert!(!d.lines.is_empty());
        render(src, 0);
    }

    #[test]
    fn inline_styles() {
        let d = render("a *em* **st** ~~del~~ `co` ***both***", 80);
        let l = &d.lines[0];
        assert_eq!(l.text(), "a em st del co both");
        let find = |s: &str| l.spans.iter().find(|x| x.text.contains(s)).unwrap().style;
        assert!(find("em").italic && !find("em").bold);
        assert!(find("st").bold && !find("st").italic);
        assert!(find("del").strike);
        assert_eq!(find("co").key, "markdown.code");
        assert!(find("both").bold && find("both").italic);
    }

    #[test]
    fn links_and_images() {
        let d = render("see [docs](http://x.io/a) and ![logo](l.png) <http://auto.io>", 80);
        let l = &d.lines[0];
        assert_eq!(l.text(), "see docs (http://x.io/a) and [logo] (l.png) http://auto.io");
        let k = |s: &str| l.spans.iter().find(|x| x.text.contains(s)).unwrap().style.key;
        assert_eq!(k("docs"), "markdown.link");
        assert_eq!(k("(http://x.io/a)"), "markdown.url");
        assert_eq!(k("(l.png)"), "markdown.url");
    }

    #[test]
    fn breaks() {
        assert_eq!(texts("a\nb", 20), ["a b"]);
        assert_eq!(texts("a  \nb", 20), ["a", "b"]);
        assert_eq!(texts("a\\\nb", 20), ["a", "b"]);
    }

    #[test]
    fn rule() {
        let d = render("a\n\n---\n\nb", 8);
        assert_eq!(d.lines[2].text(), "─".repeat(8));
        assert_eq!(d.lines[2].kind, Kind::Rule);
        assert_eq!(d.lines.len(), 5);
    }

    #[test]
    fn bullet_lists_nest_and_hang() {
        let t = texts("- one two three four\n- b\n  - c\n    - d\n", 12);
        assert_eq!(t, ["• one two", "  three four", "• b", "  ◦ c", "    ▪ d"]);
    }

    #[test]
    fn ordered_lists() {
        let t = texts("3. a\n4. b\n   1. x\n", 20);
        assert_eq!(t, ["3. a", "4. b", "   1. x"]);
    }

    #[test]
    fn loose_list_has_blank_lines() {
        let t = texts("- a\n\n- b\n", 20);
        assert_eq!(t, ["• a", "", "• b"]);
    }

    #[test]
    fn task_lists() {
        let d = render("- [ ] todo\n- [x] done\n- plain\n", 20);
        let t: Vec<String> = d.lines.iter().map(Line::text).collect();
        assert_eq!(t, ["☐ todo", "☑ done", "• plain"]);
    }

    #[test]
    fn quotes_nest_and_hold_lists() {
        let t = texts("> a\n>\n> > b c\n>\n> - x\n> - y\n\nafter", 20);
        assert_eq!(t, ["▎ a", "▎", "▎ ▎ b c", "▎", "▎ • x", "▎ • y", "", "after"]);
        let d = render("> some long quoted words here", 12);
        assert_eq!(d.lines[0].spans[0].style.key, "markdown.quote");
        for l in &d.lines {
            assert!(l.width() <= 12 && l.text().starts_with("▎ "));
        }
    }

    #[test]
    fn list_in_quote_in_list() {
        assert_eq!(texts("- > q\n  > r\n", 20), ["• ▎ q r"]);
    }

    #[test]
    fn html_blocks_are_muted_source() {
        let d = render("<div>\n<b>x</b>\n</div>\n\ntext <i>y</i>", 30);
        assert_eq!(d.lines[0].text(), "<div>");
        assert_eq!(d.lines[0].kind, Kind::Html);
        assert_eq!(keys(&d.lines[1]), ["markdown.muted"]);
        let last = d.lines.last().unwrap();
        assert_eq!(last.text(), "text <i>y</i>");
        assert!(
            last.spans
                .iter()
                .any(|s| s.text == "<i>" && s.style.key == "markdown.muted")
        );
    }

    #[test]
    fn front_matter_is_a_muted_block() {
        let d = render("---\ntitle: x\ntags: [a]\n---\n\n# H\n", 30);
        assert_eq!(d.lines[0].text(), "title: x");
        assert_eq!(d.lines[1].text(), "tags: [a]");
        assert!(
            d.lines[..2]
                .iter()
                .all(|l| l.kind == Kind::FrontMatter && keys(l) == ["markdown.muted"])
        );
        assert_eq!(d.lines[2].text(), "");
        assert_eq!(d.lines[3].text(), "H");
    }

    #[test]
    fn code_block_and_table_placeholders() {
        let d = render("```rust\nfn main() {}\n\nlet x;\n```\n", 40);
        let t: Vec<String> = d.lines.iter().map(Line::text).collect();
        assert_eq!(t, ["fn main() {}", "", "let x;"]);
        assert!(d.lines.iter().all(|l| l.kind == Kind::Code));
        let d = render("| a | b |\n|---|---|\n| 1 | **2** |\n", 40);
        let t: Vec<String> = d.lines.iter().map(Line::text).collect();
        assert_eq!(t, ["a | b", "1 | 2"]);
    }

    #[test]
    fn relayout_changes_with_width() {
        let src = "alpha beta gamma delta epsilon";
        assert_ne!(render(src, 10).lines.len(), render(src, 40).lines.len());
        assert_eq!(render(src, 40).lines.len(), 1);
    }

    /// Every key a document can produce is in the theme, and the six heading styles follow the
    /// plan: H1-H4 bold, H5 italic, H6 dim italic, H1 the brightest.
    #[test]
    fn theme_has_every_key_and_heading_styles() {
        use crate::theme::Theme;
        use ratatui::style::{Color, Modifier};
        let theme = Theme::dark_plus(true);
        let src = "---\na: 1\n---\n\n# 1\n\n## 2\n\n### 3\n\n#### 4\n\n##### 5\n\n###### 6\n\n\
                   p *e* `c` [l](u) <b>i</b>\n\n> q\n\n- x\n- [ ] t\n\n---\n\n<div>\n</div>\n\n\
                   ```\nk\n```\n\n| a |\n|---|\n| b |\n";
        let d = render(src, 30);
        let mut used: Vec<&str> = d.lines.iter().flat_map(keys).collect();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), 14, "{used:?}");
        for k in used {
            assert!(theme.find(k).is_some(), "{k} missing from the theme");
        }
        let st = |n: u8| theme.find(heading_key(n)).unwrap();
        for n in 1..=4 {
            assert!(st(n).add_modifier.contains(Modifier::BOLD), "h{n}");
        }
        for n in 5..=6 {
            assert!(st(n).add_modifier.contains(Modifier::ITALIC), "h{n}");
            assert!(!st(n).add_modifier.contains(Modifier::BOLD), "h{n}");
        }
        let lum = |n: u8| match st(n).fg {
            Some(Color::Rgb(r, g, b)) => u32::from(r) + u32::from(g) + u32::from(b),
            other => panic!("h{n} fg {other:?}"),
        };
        assert!((2..=6).all(|n| lum(1) > lum(n)), "H1 is the brightest");
        assert!(lum(6) < lum(5), "H6 is dimmer than H5");
        let mut styles: Vec<String> = (1..=6).map(|n| format!("{:?}", st(n))).collect();
        styles.dedup();
        assert_eq!(styles.len(), 6);
    }

    #[test]
    fn plain_text_has_no_flags_and_styles_end() {
        let d = render("plain **b** mid *i* ~~s~~ end", 80);
        let spans = &d.lines[0].spans;
        for s in spans
            .iter()
            .filter(|s| s.text.contains("plain") || s.text.contains("mid"))
        {
            assert_eq!(s.style, SpanStyle::key("markdown.text"), "{s:?}");
        }
        let end = spans.last().unwrap();
        assert_eq!(end.text, " end");
        assert_eq!(end.style, SpanStyle::key("markdown.text"));
    }

    #[test]
    fn blank_line_before_every_heading() {
        let d = render("- a\n# H\npara\n## H2\n> q\n### H3\n```\nc\n```\n#### H4\n", 20);
        for (i, l) in d.lines.iter().enumerate() {
            if matches!(l.kind, Kind::Heading(_)) {
                assert_eq!(d.lines[i - 1].kind, Kind::Blank, "before {:?}", l.text());
            }
        }
        assert_eq!(d.lines.iter().filter(|l| matches!(l.kind, Kind::Heading(_))).count(), 4);
        // No doubled blank lines anywhere.
        assert!(
            d.lines
                .windows(2)
                .all(|w| !(w[0].kind == Kind::Blank && w[1].kind == Kind::Blank))
        );
        // A heading wraps; its rule spans the width, and inside a quote the space left.
        let d = render("# alpha beta gamma\n\n> ## q\n", 12);
        let t: Vec<String> = d.lines.iter().map(Line::text).collect();
        assert_eq!(t[..3], ["alpha beta", "gamma", &"═".repeat(12)]);
        assert!(d.lines[..2].iter().all(|l| l.kind == Kind::Heading(1)));
        assert_eq!(t[4..], ["▎ q", &format!("▎ {}", "─".repeat(10))]);
    }

    #[test]
    fn rule_bullets_and_quote_keys() {
        let d = render("---\n\n- a\n1. b\n\n> > c", 10);
        assert_eq!(keys(&d.lines[0]), ["markdown.rule"]);
        assert_eq!(d.lines[2].spans[0].style.key, "markdown.bullet");
        assert_eq!(d.lines[4].spans[0].text, "1. ");
        assert_eq!(d.lines[4].spans[0].style.key, "markdown.bullet");
        let q = d.lines.last().unwrap();
        assert_eq!(keys(q), ["markdown.quote", "markdown.quote", "markdown.text"]);
        // A rule inside a quote leaves room for the gutter.
        let d = render("> ---", 10);
        assert_eq!(d.lines[0].text(), format!("▎ {}", "─".repeat(8)));
    }

    #[test]
    fn loose_task_list_and_continuation_paragraphs() {
        assert_eq!(texts("- [ ] a\n\n- [x] b\n", 20), ["☐ a", "", "☑ b"]);
        assert_eq!(
            texts("1. a\n\n   more\n2. b\n", 20),
            ["1. a", "", "   more", "", "2. b"]
        );
        // Ten items: the marker width follows each number, the hang matches it.
        let src: String = (1..=10).map(|i| format!("{i}. w x\n")).collect();
        let t = texts(&src, 5);
        assert_eq!(t[0], "1. w");
        assert_eq!(t[1], "   x");
        assert_eq!(t[18], "10. w");
        assert_eq!(t[19], "    x");
    }

    #[test]
    fn link_text_equal_to_url_is_not_repeated() {
        assert_eq!(texts("[u](u) [v](u) <http://a.b>", 40), ["u v (u) http://a.b"]);
        let d = render("[**b** l](u)", 40);
        assert!(d.lines[0].spans[0].style.bold);
        assert_eq!(d.lines[0].spans[0].style.key, "markdown.link");
    }
}
