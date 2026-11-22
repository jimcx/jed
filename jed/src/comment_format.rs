//! The formatting buttons of a review comment box (review_comment_format): which Markdown each
//! style writes for a forge, and `apply`, which edits a text with it at every selection. No app
//! state, nothing drawn: `app/review_format.rs` runs it on the box, the toolbar
//! (`review_blocks::composer_tool_cols`) lists `styles(kind)`.
//!
//! What each forge renders in a merge request comment (research for review_comment_format-a;
//! "unverified" = not confirmed from a source):
//!
//! | style       | GitHub                    | GitLab                | Gitea / Forgejo           |
//! |-------------|---------------------------|-----------------------|---------------------------|
//! | bold        | `**x**`                   | `**x**`               | `**x**`                   |
//! | italic      | `_x_` (toolbar's form)    | `_x_`                 | `_x_`                     |
//! | underline   | `<ins>x</ins>`            | `<ins>x</ins>`, unverified | `<ins>x</ins>`       |
//! | strike      | `~~x~~`                   | `~~x~~`               | `~~x~~`                   |
//! | inline code | `` `x` ``                 | `` `x` ``             | `` `x` ``                 |
//! | code block  | ```` ``` ```` fence       | fence                 | fence                     |
//! | link        | `[x](url)`                | `[x](url)`            | `[x](url)`                |
//! | quote       | `> `                      | `> `                  | `> `                      |
//! | bullets     | `- `                      | `- `                  | `- `                      |
//! | numbers     | `1. `, `2. `              | `1. `, `2. `          | `1. `, `2. `              |
//! | task list   | `- [ ] `                  | `- [ ] ` (also `[~]`) | `- [ ] `                  |
//! | heading     | `### `                    | `### `                | `### `                    |
//!
//! - GitHub: github/markdown-toolbar-element (`src/index.ts`) writes `**`, `_`, `` ` `` (a fence
//!   over several lines), `> `, `[x](url)`, `- `, `1. `, `- [ ] `, `### ` (its default level) and has
//!   no underline button. The sanitizer (html-pipeline's `SanitizationFilter` allowlist) keeps
//!   `ins`, `del`, `s`, `strike`, `kbd`, `sub`, `sup`, `details` and does not keep `u`: so underline
//!   is `<ins>`. GFM renders `~~x~~` as strikethrough and `- [ ]` items as task lists in comments.
//! - GitLab: docs.gitlab.com's GitLab Flavored Markdown page: `~~x~~` (and `~x~`) strikethrough,
//!   task lists in "issues, merge requests, epics, and comments" (`[ ]`, `[x]`, `[~]`), `#` to
//!   `######` headings, fences, `>` quotes. It says only that its sanitizer allows the
//!   html-pipeline list plus `span`, `abbr`, `details`, `summary`; neither it nor its editor
//!   (no underline button; an open issue proposes one that wraps `<ins>`) confirms `<ins>`, and
//!   the handbook's Markdown guide names `<ins>` as the way to underline. So `<ins>` is offered
//!   (unverified: check on a throwaway merge request).
//! - Gitea: `modules/markup/sanitizer_default.go` keeps `ins`, `del`, `sup`, `sub`, `kbd`, `s`,
//!   `strike`, `details` and `input type=checkbox`, and bluemonday's `UGCPolicy` adds `u`, `b`,
//!   `i`, `strike`; goldmark runs with the GFM strikethrough, table and task-list extensions.
//!   `<ins>` is used (kept by both). Forgejo's sanitizer is a fork of Gitea's: assumed the same
//!   (unverified). jed's `Kind::Gitea` covers both.
//! - No forge lacks a style today, so `styles` lists all eleven for every kind; `markup` is
//!   `None` for a style a kind's list doesn't hold, which is how a forge that keeps neither
//!   underline tag would drop it.

use std::fmt::Write as _;

use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::ByteIdx;
use crate::buffer::pos::{self, LineWalker};
use crate::forge::Kind;
use crate::selection::{Selection, Selections};

/// One formatting button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Link,
    Quote,
    Bullets,
    Numbers,
    Tasks,
    Heading,
}

impl Style {
    /// In toolbar order.
    pub const ALL: [Style; 11] = [
        Style::Bold,
        Style::Italic,
        Style::Underline,
        Style::Strike,
        Style::Code,
        Style::Link,
        Style::Quote,
        Style::Bullets,
        Style::Numbers,
        Style::Tasks,
        Style::Heading,
    ];

    /// For palette entries ("Code Review: Bold").
    pub fn title(self) -> &'static str {
        match self {
            Style::Bold => "Bold",
            Style::Italic => "Italic",
            Style::Underline => "Underline",
            Style::Strike => "Strikethrough",
            Style::Code => "Code",
            Style::Link => "Link",
            Style::Quote => "Quote",
            Style::Bullets => "Bulleted List",
            Style::Numbers => "Numbered List",
            Style::Tasks => "Task List",
            Style::Heading => "Heading",
        }
    }

    /// The toolbar button's short ASCII label (without brackets).
    pub fn label(self) -> &'static str {
        match self {
            Style::Bold => "B",
            Style::Italic => "I",
            Style::Underline => "U",
            Style::Strike => "S",
            Style::Code => "<>",
            Style::Link => "Link",
            Style::Quote => ">",
            Style::Bullets => "-",
            Style::Numbers => "1.",
            Style::Tasks => "Task",
            Style::Heading => "H",
        }
    }

    /// For command names (`review.format.bold`).
    pub fn name(self) -> &'static str {
        match self {
            Style::Bold => "bold",
            Style::Italic => "italic",
            Style::Underline => "underline",
            Style::Strike => "strike",
            Style::Code => "code",
            Style::Link => "link",
            Style::Quote => "quote",
            Style::Bullets => "bullets",
            Style::Numbers => "numbers",
            Style::Tasks => "tasks",
            Style::Heading => "heading",
        }
    }
}

/// What a style writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Markup {
    /// A pair around the selection (or the caret).
    Inline { open: &'static str, close: &'static str },
    /// Code: backticks on one line, a fence over several.
    Code,
    /// A prefix on every line the selection touches.
    Prefix(&'static str),
    /// `1. `, `2. `, ... on every line.
    Numbered,
    /// `[text](url)`.
    Link,
}

/// The styles `kind`'s merge requests render, in toolbar order.
pub fn styles(kind: Kind) -> &'static [Style] {
    match kind {
        Kind::GitHub | Kind::GitLab | Kind::Gitea => &Style::ALL,
    }
}

/// `style`'s Markdown for `kind`; `None` when the forge doesn't render it.
pub fn markup(kind: Kind, style: Style) -> Option<Markup> {
    if !styles(kind).contains(&style) {
        return None;
    }
    let inline = |open, close| Markup::Inline { open, close };
    Some(match style {
        Style::Bold => inline("**", "**"),
        Style::Italic => inline("_", "_"),
        Style::Underline => inline("<ins>", "</ins>"),
        Style::Strike => inline("~~", "~~"),
        Style::Code => Markup::Code,
        Style::Link => Markup::Link,
        Style::Quote => Markup::Prefix("> "),
        Style::Bullets => Markup::Prefix("- "),
        Style::Numbers => Markup::Numbered,
        Style::Tasks => Markup::Prefix("- [ ] "),
        Style::Heading => Markup::Prefix("### "),
    })
}

const TEXT: &str = "text";
const URL: &str = "url";

/// `markup` applied at every selection: one `ChangeSet` (in order, O(selections · log n)) and the
/// selections after it. Inline marks wrap (the selection stays on its text, inside the marks) or,
/// when the text is already wrapped in them (inside or just outside the selection), come off;
/// an empty selection gets the pair with the caret between. Line styles prefix every line a
/// selection touches (a selection ending at a line's start doesn't take that line; a line two
/// selections share is done once) or, when all of them have it, remove it.
pub fn apply(rope: &Rope, selections: &Selections, markup: Markup) -> (ChangeSet, Selections) {
    match markup {
        Markup::Inline { open, close } => inline(rope, selections, open, close),
        Markup::Code => code(rope, selections),
        Markup::Prefix(p) => lines(rope, selections, Some(p)),
        Markup::Numbered => lines(rope, selections, None),
        Markup::Link => link(rope, selections),
    }
}

/// Whether the text at `at` is `s`.
fn eq_at(rope: &Rope, at: usize, s: &str) -> bool {
    if at + s.len() > rope.len() {
        return false;
    }
    let mut want = s.as_bytes();
    for chunk in pos::bytes_in(rope, at, at + s.len()) {
        if !want.starts_with(chunk) {
            return false;
        }
        want = &want[chunk.len()..];
    }
    want.is_empty()
}

/// The selections `changes` leave: each end mapped (`start` after an insertion at it, `end` before
/// one), directions kept.
fn mapped(changes: &ChangeSet, before: &Selections) -> Selections {
    let ends: Vec<_> = before.iter().map(|s| (s.start(), s.end())).collect();
    mapped_ends(changes, before, &ends)
}

/// `mapped`, with each selection's ends given (`ends`, in `before`'s order) instead of its own.
fn mapped_ends(changes: &ChangeSet, before: &Selections, ends: &[(ByteIdx, ByteIdx)]) -> Selections {
    let m = changes.mapper();
    let out = before
        .iter()
        .zip(ends)
        .map(|(s, &(start, end))| {
            let (a, b) = (m.map(start, Assoc::After), m.map(end, Assoc::Before));
            let (a, b) = if b < a { (a, a) } else { (a, b) };
            if s.anchor > s.head {
                Selection::new(b, a)
            } else {
                Selection::new(a, b)
            }
        })
        .collect();
    Selections::with_newest(out, before.primary_index(), before.newest_index())
}

fn inline(rope: &Rope, before: &Selections, open: &str, close: &str) -> (ChangeSet, Selections) {
    let mut changes = ChangeSet::with_capacity(before.len() * 2, before.len() * (open.len() + close.len()));
    let mut last = 0;
    let mut ends = Vec::with_capacity(before.len());
    for s in before.iter() {
        let (a, b) = (s.start().0, s.end().0);
        ends.push((s.start(), s.end()));
        if a < last {
            continue;
        }
        let (ol, cl) = (open.len(), close.len());
        if a < b {
            if b - a >= ol + cl && eq_at(rope, a, open) && eq_at(rope, b - cl, close) {
                changes.push(ByteIdx(a), ByteIdx(a + ol), "");
                changes.push(ByteIdx(b - cl), ByteIdx(b), "");
                last = b;
                continue;
            }
            if a >= last + ol && eq_at(rope, a - ol, open) && eq_at(rope, b, close) {
                changes.push(ByteIdx(a - ol), ByteIdx(a), "");
                changes.push(ByteIdx(b), ByteIdx(b + cl), "");
                last = b + cl;
                continue;
            }
        }
        // The marks go inside the selection's edge blanks: `**a **` isn't bold on the forges.
        let (wa, wb) = trim_blanks(rope, a, b);
        changes.push(ByteIdx(wa), ByteIdx(wa), open);
        changes.push(ByteIdx(wb), ByteIdx(wb), close);
        if let Some(e) = ends.last_mut() {
            *e = (ByteIdx(wa), ByteIdx(wb));
        }
        last = b;
    }
    let after = mapped_ends(&changes, before, &ends);
    (changes, after)
}

/// `a..b` without its leading and trailing spaces and tabs (unchanged when it is all blanks).
fn trim_blanks(rope: &Rope, a: usize, b: usize) -> (usize, usize) {
    let (mut first, mut last) = (None, a);
    let mut at = a;
    for chunk in pos::bytes_in(rope, a, b) {
        for (i, &x) in chunk.iter().enumerate() {
            if x != b' ' && x != b'\t' {
                first.get_or_insert(at + i);
                last = at + i + 1;
            }
        }
        at += chunk.len();
    }
    first.map_or((a, b), |f| (f, last))
}

/// Whether `a..b` holds a line break.
fn multi_line(rope: &Rope, a: usize, b: usize) -> bool {
    pos::bytes_in(rope, a, b).any(|c| c.iter().any(|&x| x == b'\n' || x == b'\r'))
}

fn code(rope: &Rope, before: &Selections) -> (ChangeSet, Selections) {
    if !before.iter().any(|s| multi_line(rope, s.start().0, s.end().0)) {
        return inline(rope, before, "`", "`");
    }
    // Some selection spans lines: those get a fence, the others backticks.
    let mut changes = ChangeSet::with_capacity(before.len() * 2, before.len() * 6);
    let mut walk = LineWalker::new(rope);
    let mut last = 0;
    for s in before.iter() {
        let (a, b) = (s.start().0, s.end().0);
        if a < last {
            continue;
        }
        if multi_line(rope, a, b) {
            let open = if walk.line_at(s.start()).start().0 == a {
                "```\n"
            } else {
                "\n```\n"
            };
            let end_line = walk.line_at(s.end());
            let close = if end_line.start().0 == b {
                "```\n"
            } else if end_line.end().0 == b {
                "\n```"
            } else {
                "\n```\n"
            };
            changes.push(ByteIdx(a), ByteIdx(a), open);
            changes.push(ByteIdx(b), ByteIdx(b), close);
        } else {
            changes.push(ByteIdx(a), ByteIdx(a), "`");
            changes.push(ByteIdx(b), ByteIdx(b), "`");
        }
        last = b;
    }
    let after = mapped(&changes, before);
    (changes, after)
}

/// The length of a `N. ` prefix at the start of `text`.
fn number_prefix(text: &str) -> Option<usize> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0 && text[digits..].starts_with(". ")).then_some(digits + 2)
}

/// `prefix` (None: numbers) on the touched lines.
fn lines(rope: &Rope, before: &Selections, prefix: Option<&str>) -> (ChangeSet, Selections) {
    let mut walk = LineWalker::new(rope);
    // The touched lines in order, each once.
    let mut touched: Vec<usize> = Vec::with_capacity(before.len());
    for s in before.iter() {
        let first = walk.line_at(s.start()).index;
        let end_line = walk.line_at(s.end());
        let mut last = end_line.index;
        if !s.is_empty() && s.end() == end_line.start() && last > first {
            last -= 1;
        }
        let from = touched.last().map_or(first, |&l| first.max(l + 1));
        touched.extend(from..=last);
    }
    let mut has = true;
    for &l in &touched {
        let text = walk.line(l).text();
        has &= match prefix {
            Some(p) => text.starts_with(p),
            None => number_prefix(text).is_some(),
        };
        if !has {
            break;
        }
    }
    let mut changes = ChangeSet::with_capacity(touched.len(), touched.len() * 4);
    let (mut n, mut prev) = (0, usize::MAX);
    let mut buf = String::new();
    for &l in &touched {
        let line = walk.line(l);
        let start = line.start();
        if has {
            let len = prefix.map_or_else(|| number_prefix(line.text()).unwrap_or(0), str::len);
            changes.push(start, ByteIdx(start.0 + len), "");
        } else {
            let p = match prefix {
                Some(p) => p,
                None => {
                    n = if prev != usize::MAX && l == prev + 1 { n + 1 } else { 1 };
                    buf.clear();
                    let _ = write!(buf, "{n}. ");
                    &buf
                }
            };
            changes.push(start, start, p);
        }
        prev = l;
    }
    // Cursors go after what was added; a selection grows over it.
    let m = changes.mapper();
    let out = before
        .iter()
        .map(|s| {
            let a = m.map(s.start(), if s.is_empty() { Assoc::After } else { Assoc::Before });
            let b = m.map(s.end(), Assoc::After);
            if s.anchor > s.head {
                Selection::new(b, a)
            } else {
                Selection::new(a, b)
            }
        })
        .collect();
    let after = Selections::with_newest(out, before.primary_index(), before.newest_index());
    (changes, after)
}

fn link(rope: &Rope, before: &Selections) -> (ChangeSet, Selections) {
    let mut changes = ChangeSet::with_capacity(before.len() * 2, before.len() * 12);
    let mut out = Vec::with_capacity(before.len());
    let mut delta = 0;
    for s in before.iter() {
        let (a, b) = (s.start().0, s.end().0);
        let select = |from: usize, len: usize| Selection::new(ByteIdx(from), ByteIdx(from + len));
        if a == b {
            let insert = format!("[{TEXT}]({URL})");
            changes.push(s.start(), s.start(), &insert);
            out.push(select(a + delta + 1, TEXT.len()));
            delta += insert.len();
        } else if is_url(rope, a, b) {
            let open = format!("[{TEXT}](");
            changes.push(s.start(), s.start(), &open);
            changes.push(s.end(), s.end(), ")");
            out.push(select(a + delta + 1, TEXT.len()));
            delta += open.len() + 1;
        } else {
            let close = format!("]({URL})");
            changes.push(s.start(), s.start(), "[");
            changes.push(s.end(), s.end(), &close);
            out.push(select(b + delta + 3, URL.len()));
            delta += 1 + close.len();
        }
    }
    let after = Selections::with_newest(out, before.primary_index(), before.newest_index());
    (changes, after)
}

/// Whether `a..b` is one `http(s)://` word, which a Link button makes the target.
fn is_url(rope: &Rope, a: usize, b: usize) -> bool {
    (eq_at(rope, a, "http://") || eq_at(rope, a, "https://"))
        && !pos::bytes_in(rope, a, b).any(|c| c.iter().any(u8::is_ascii_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `marked` (`|` a caret, `«…»` a selection) with `style` applied for `kind`, shown the same way.
    fn run(kind: Kind, style: Style, marked: &str) -> String {
        let (mut text, mut list, mut anchor) = (String::new(), Vec::new(), None);
        for c in marked.chars() {
            match c {
                '|' => list.push(Selection::cursor(ByteIdx(text.len()))),
                '«' => anchor = Some(text.len()),
                '»' => list.push(Selection::new(ByteIdx(anchor.take().unwrap()), ByteIdx(text.len()))),
                c => text.push(c),
            }
        }
        let mut rope = Rope::from_str(&text);
        let sel = Selections::new(list, 0);
        let (changes, after) = apply(&rope, &sel, markup(kind, style).unwrap());
        changes.apply(&mut rope);
        let text = rope.to_string();
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in after.iter() {
            if s.is_empty() {
                marks.push((s.head.0, '|'));
            } else {
                marks.push((s.start().0, '«'));
                marks.push((s.end().0, '»'));
            }
        }
        marks.sort_by_key(|m| m.0);
        let mut out = String::new();
        let mut at = 0;
        for (p, c) in marks {
            out.push_str(&text[at..p]);
            out.push(c);
            at = p;
        }
        out.push_str(&text[at..]);
        out
    }

    fn g(style: Style, marked: &str) -> String {
        run(Kind::GitHub, style, marked)
    }

    #[test]
    fn inline_wraps_and_keeps_the_selection_inside() {
        assert_eq!(g(Style::Bold, "a «b c» d"), "a **«b c»** d");
        assert_eq!(g(Style::Italic, "a «b» d"), "a _«b»_ d");
        assert_eq!(g(Style::Strike, "«b»"), "~~«b»~~");
        assert_eq!(g(Style::Underline, "a «b» d"), "a <ins>«b»</ins> d");
    }

    #[test]
    fn inline_marks_move_inside_the_selections_edge_blanks() {
        assert_eq!(g(Style::Bold, "x« a »y"), "x **«a»** y");
        assert_eq!(g(Style::Italic, "«é日 »"), "_«é日»_ ");
        // Only blanks: wrapped as is.
        assert_eq!(g(Style::Bold, "«  »"), "**«  »**");
    }

    #[test]
    fn inline_empty_selection_inserts_the_pair() {
        assert_eq!(g(Style::Bold, "a | b"), "a **|** b");
        assert_eq!(g(Style::Underline, "|"), "<ins>|</ins>");
        assert_eq!(g(Style::Code, "x|"), "x`|`");
    }

    #[test]
    fn inline_toggles_off_inside_and_outside_the_selection() {
        assert_eq!(g(Style::Bold, "a «**b**» d"), "a «b» d");
        assert_eq!(g(Style::Bold, "a **«b»** d"), "a «b» d");
        assert_eq!(g(Style::Underline, "<ins>«b»</ins>"), "«b»");
        assert_eq!(g(Style::Underline, "«<ins>b</ins>»"), "«b»");
        // Other marks are not ours.
        assert_eq!(g(Style::Italic, "**«b»**"), "**_«b»_**");
        // `**` alone is not wrapped text.
        assert_eq!(g(Style::Bold, "«**»"), "**«**»**");
    }

    #[test]
    fn every_forge_has_every_style_in_toolbar_order() {
        for k in [Kind::GitHub, Kind::GitLab, Kind::Gitea] {
            assert_eq!(styles(k), &Style::ALL);
            for s in Style::ALL {
                assert!(markup(k, s).is_some());
            }
        }
        assert_eq!(
            markup(Kind::Gitea, Style::Underline),
            markup(Kind::GitHub, Style::Underline)
        );
        assert_eq!(
            markup(Kind::GitLab, Style::Italic),
            Some(Markup::Inline { open: "_", close: "_" })
        );
    }

    #[test]
    fn code_is_backticks_on_a_line_and_a_fence_over_lines() {
        assert_eq!(g(Style::Code, "a «b» c"), "a `«b»` c");
        assert_eq!(g(Style::Code, "a `«b»` c"), "a «b» c");
        assert_eq!(g(Style::Code, "«a\nb»"), "```\n«a\nb»\n```");
        assert_eq!(g(Style::Code, "x «a\nb» y"), "x \n```\n«a\nb»\n```\n y");
        assert_eq!(g(Style::Code, "«a\nb\n»c"), "```\n«a\nb\n»```\nc");
    }

    #[test]
    fn line_styles_prefix_every_touched_line() {
        assert_eq!(g(Style::Quote, "a «b\nc» d\ne"), "> a «b\n> c» d\ne");
        assert_eq!(g(Style::Bullets, "x|y"), "- x|y");
        assert_eq!(g(Style::Tasks, "«a\nb»"), "«- [ ] a\n- [ ] b»");
        assert_eq!(g(Style::Heading, "a|b"), "### a|b");
        // A selection ending at a line's start doesn't take that line.
        assert_eq!(g(Style::Quote, "«a\n»b"), "«> a\n»b");
    }

    #[test]
    fn numbers_count_from_one_and_restart_after_a_gap() {
        assert_eq!(g(Style::Numbers, "«a\nb\nc»"), "«1. a\n2. b\n3. c»");
        assert_eq!(g(Style::Numbers, "«a»\nx\n«b»"), "«1. a»\nx\n«1. b»");
    }

    #[test]
    fn line_styles_toggle_off_when_every_line_has_them() {
        assert_eq!(g(Style::Quote, "> «a\n> b»"), "«a\nb»");
        assert_eq!(g(Style::Quote, "> «a\nb»"), "> > «a\n> b»");
        assert_eq!(g(Style::Numbers, "1. «a\n2. b»"), "«a\nb»");
        assert_eq!(g(Style::Numbers, "9. a|\n10. b"), "a|\n10. b");
        assert_eq!(g(Style::Tasks, "- [ ] a|"), "a|");
        assert_eq!(g(Style::Heading, "### a|"), "a|");
    }

    #[test]
    fn a_line_two_selections_share_is_done_once() {
        assert_eq!(g(Style::Bullets, "a |b |c"), "- a |b |c");
        assert_eq!(g(Style::Bullets, "|a\n|b\n|c"), "- |a\n- |b\n- |c");
    }

    #[test]
    fn link_wraps_with_the_url_selected() {
        assert_eq!(g(Style::Link, "a «b c» d"), "a [b c](«url») d");
        assert_eq!(g(Style::Link, "a | d"), "a [«text»](url) d");
        assert_eq!(g(Style::Link, "«https://x.io/a»"), "[«text»](https://x.io/a)");
    }

    #[test]
    fn several_selections_make_one_edit() {
        assert_eq!(g(Style::Bold, "«a» «b» |"), "**«a»** **«b»** **|**");
        assert_eq!(g(Style::Bold, "**«a»** «b»"), "«a» **«b»**");
        assert_eq!(g(Style::Link, "«a» «b»"), "[a](«url») [b](«url»)");
        let rope = Rope::from_str("a b");
        let sel = Selections::new(vec![Selection::cursor(ByteIdx(0)), Selection::cursor(ByteIdx(2))], 0);
        let (changes, _) = apply(&rope, &sel, markup(Kind::Gitea, Style::Bold).unwrap());
        assert_eq!(changes.len(), 4);
    }

    #[test]
    fn multi_byte_text_is_wrapped_on_char_boundaries() {
        assert_eq!(g(Style::Bold, "é «日本» ü"), "é **«日本»** ü");
        assert_eq!(g(Style::Bold, "é **«日本»** ü"), "é «日本» ü");
        assert_eq!(g(Style::Quote, "日«本\nü»"), "> 日«本\n> ü»");
        assert_eq!(g(Style::Link, "«日本»"), "[日本](«url»)");
    }
}
