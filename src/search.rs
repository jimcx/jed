//! The one matcher behind find in file, Cmd+D / Cmd+Shift+L and workspace search.
//!
//! A `Matcher` is built once from a pattern and `Options` (plain text or regex, case, whole
//! word) and then searches a rope (`find_next`, `find_prev`, `find_all`, `find_in`) or, through
//! its `grep_matcher::Matcher` impl, files on disk with `grep-searcher`. Both paths share the
//! compiled regex and the whole-word check, so every option means the same thing everywhere.
//!
//! Matching is line by line: each line's content (without its line break) is a haystack, borrowed
//! straight from the rope's chunk when the line sits in one (see `pos::each_line`), so searching
//! never copies the buffer. Matches therefore never span lines, as in `grep`. The exception is a
//! pattern that names a line break (a newline in the text, or `\n`, `\r`, `\W` in a regex — the
//! rule VS Code uses): such a pattern is *multi-line* and searches the whole text at once, with
//! `^`/`$` matching at every line break (CRLF and lone CR included). That's the rare path (e.g.
//! Cmd+D on a selection spanning lines), so it copies the rope into a string.
//! A multi-line pattern matches line breaks byte for byte: `\n` in a regex doesn't match a CRLF.

use std::fmt;
use std::ops::{ControlFlow, Range};

use grep_matcher::{LineTerminator, NoCaptures, NoError};
use regex::bytes::{Regex, RegexBuilder};
use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};

/// Search options; they combine freely. The default is plain text, case-insensitive, any position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Options {
    /// Rust `regex` syntax instead of literal text.
    pub regex: bool,
    pub case_sensitive: bool,
    /// Only matches that start and end at a word boundary (VS Code's rule, see `is_whole_word`).
    pub whole_word: bool,
}

/// One match: `start..end` in the buffer. Empty only for regexes that can match nothing (`^`, `a*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Match {
    pub start: ByteIdx,
    pub end: ByteIdx,
}

/// An invalid regex. `Display` is one line, meant to be shown under the search input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError(String);

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SearchError {}

impl From<regex::Error> for SearchError {
    fn from(e: regex::Error) -> Self {
        match e {
            // The syntax error's last line is `error: <what went wrong>`; the lines above it echo
            // the pattern with a caret, which doesn't fit under a one-line input.
            regex::Error::Syntax(text) => {
                let last = text.lines().last().unwrap_or_default();
                SearchError(last.strip_prefix("error: ").unwrap_or(last).to_string())
            }
            regex::Error::CompiledTooBig(_) => SearchError("pattern is too big".to_string()),
            e => SearchError(e.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Matcher {
    /// `None` for an empty pattern, which matches nothing.
    regex: Option<Regex>,
    whole_word: bool,
    multiline: bool,
}

impl Matcher {
    /// Compiles `pattern`. Fails only for an invalid regex (plain text is always valid).
    pub fn new(pattern: &str, opts: Options) -> Result<Matcher, SearchError> {
        let multiline = if opts.regex {
            names_line_break(pattern)
        } else {
            pattern.contains(['\n', '\r'])
        };
        Self::build(pattern, opts, multiline)
    }

    /// Like `new`, but never multi-line: every line is searched on its own whatever the pattern
    /// says, so a match never contains a line break (workspace search, where matches stay within
    /// one line in v1). `\W` or `[^a]` then never match the line break, and a pattern that needs
    /// one (`a\nb`, `\r`, a pasted line break) matches nothing.
    pub fn new_single_line(pattern: &str, opts: Options) -> Result<Matcher, SearchError> {
        Self::build(pattern, opts, false)
    }

    fn build(pattern: &str, opts: Options, multiline: bool) -> Result<Matcher, SearchError> {
        let regex = if pattern.is_empty() {
            None
        } else {
            let source = if opts.regex {
                pattern.to_string()
            } else {
                regex::escape(pattern)
            };
            let regex = RegexBuilder::new(&source)
                .case_insensitive(!opts.case_sensitive)
                .multi_line(multiline)
                .crlf(multiline)
                .build()?;
            Some(regex)
        };
        Ok(Matcher {
            regex,
            whole_word: opts.whole_word,
            multiline,
        })
    }

    /// True for an empty pattern: nothing matches, and the UI shows no results.
    pub fn is_empty(&self) -> bool {
        self.regex.is_none()
    }

    /// Whether matches can span lines (see the module docs).
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// The first match starting at or after `from`. An empty match exactly at `from` is skipped,
    /// so "find next" from the end of the current match always moves on. With `wrap`, continues
    /// from the start of the text; the result is then before `from` (or the same match again
    /// when it is the only one).
    pub fn find_next(&self, rope: &Rope, from: ByteIdx, wrap: bool) -> Option<Match> {
        let found = self.next_from(rope, from, Some(from));
        if found.is_some() || !wrap {
            return found;
        }
        self.next_from(rope, ByteIdx(0), None)
    }

    /// The last match ending at or before `before` (an empty match exactly at `before` is
    /// skipped). With `wrap`, continues backwards from the end of the text.
    pub fn find_prev(&self, rope: &Rope, before: ByteIdx, wrap: bool) -> Option<Match> {
        let found = self.prev_from(rope, before, Some(before));
        if found.is_some() || !wrap {
            return found;
        }
        self.prev_from(rope, ByteIdx(rope.len()), None)
    }

    /// Every match in the text, in order.
    pub fn find_all(&self, rope: &Rope) -> Vec<Match> {
        self.find_in(rope, ByteIdx(0)..ByteIdx(rope.len()))
    }

    /// Every match overlapping `range` (an empty match counts when it lies inside it), in order,
    /// e.g. the visible lines for highlighting. Lines are always searched whole, so these are
    /// exactly the matches `find_all` reports there.
    pub fn find_in(&self, rope: &Rope, range: Range<ByteIdx>) -> Vec<Match> {
        let mut out = Vec::new();
        self.each_in(rope, range, |m, _, _, _| out.push(m));
        out
    }

    /// Every match overlapping `range` (as `find_in`) with the text that replaces it, in order.
    /// `with` is a `Replacement` built for this matcher's mode: `$1`, `$&`, … are expanded from
    /// each match's groups (searched in the same line, so anchors and `\b` see the context).
    pub fn replace_in(&self, rope: &Rope, range: Range<ByteIdx>, with: &Replacement, mut f: impl FnMut(Match, &str)) {
        if let Some(text) = with.literal() {
            self.each_in(rope, range, |m, _, _, _| f(m, text));
            return;
        }
        let mut out = String::new();
        self.each_in(rope, range, |m, re, hay, offset| {
            out.clear();
            let (s, e) = (m.start.0 - offset, m.end.0 - offset);
            match re.captures_at(hay, s) {
                Some(caps) if caps.get(0).is_some_and(|c| c.start() == s && c.end() == e) => {
                    with.expand(&caps, &mut out)
                }
                // Can't happen (the same search from the match's start), but stay sane.
                _ => out.push_str(&String::from_utf8_lossy(&hay[s..e])),
            }
            f(m, &out);
        });
    }

    /// Calls `f(match, regex, haystack, haystack start)` for every match overlapping `range`.
    fn each_in(&self, rope: &Rope, range: Range<ByteIdx>, mut f: impl FnMut(Match, &Regex, &[u8], usize)) {
        let Some(re) = &self.regex else {
            return;
        };
        let wanted = |m: &Match| {
            if m.start == m.end {
                range.start <= m.start && m.start <= range.end
            } else {
                m.start < range.end && range.start < m.end
            }
        };
        if self.multiline {
            let text = rope.to_string();
            let text = text.as_bytes();
            let mut at = 0;
            let mut last_end = None;
            while let Some((s, e)) = self.find_at(re, text, at, last_end) {
                let m = Match {
                    start: ByteIdx(s),
                    end: ByteIdx(e),
                };
                if m.start > range.end {
                    break;
                }
                if wanted(&m) {
                    f(m, re, text, 0);
                }
                (at, last_end) = step(text, s, e);
            }
            return;
        }
        let first = pos::line_of(rope, range.start);
        pos::each_line(rope, first, false, |start, line| {
            if start > range.end {
                return ControlFlow::Break(());
            }
            let mut at = 0;
            let mut last_end = None;
            while let Some((s, e)) = self.find_at(re, line.as_bytes(), at, last_end) {
                let m = Match {
                    start: ByteIdx(start.0 + s),
                    end: ByteIdx(start.0 + e),
                };
                if wanted(&m) {
                    f(m, re, line.as_bytes(), start.0);
                }
                (at, last_end) = step(line.as_bytes(), s, e);
            }
            ControlFlow::Continue(())
        });
    }

    /// Every match in `line`, one line's text without its line break (workspace search's lines
    /// from `grep-searcher`), as `start..end` byte offsets in order: the matches `find_all` finds
    /// on that line. Stops when `f` returns false. Linear in the line, however many matches.
    pub fn each_in_line(&self, line: &[u8], mut f: impl FnMut(usize, usize) -> bool) {
        let Some(re) = &self.regex else { return };
        let mut at = 0;
        let mut last_end = None;
        while let Some((s, e)) = self.find_at(re, line, at, last_end) {
            if !f(s, e) {
                return;
            }
            (at, last_end) = step(line, s, e);
        }
    }

    /// First match starting at or after `from`, rejecting an empty match at `skip_empty`.
    fn next_from(&self, rope: &Rope, from: ByteIdx, skip_empty: Option<ByteIdx>) -> Option<Match> {
        let re = self.regex.as_ref()?;
        if self.multiline {
            let text = rope.to_string();
            let (s, e) = self.find_at(re, text.as_bytes(), from.0, skip_empty.map(|p| p.0))?;
            return Some(Match {
                start: ByteIdx(s),
                end: ByteIdx(e),
            });
        }
        pos::each_line(rope, pos::line_of(rope, from), false, |start, line| {
            let at = from.0.saturating_sub(start.0);
            if at > line.len() {
                return ControlFlow::Continue(());
            }
            let skip = skip_empty.and_then(|p| p.0.checked_sub(start.0));
            match self.find_at(re, line.as_bytes(), at, skip) {
                Some((s, e)) => ControlFlow::Break(Match {
                    start: ByteIdx(start.0 + s),
                    end: ByteIdx(start.0 + e),
                }),
                None => ControlFlow::Continue(()),
            }
        })
    }

    /// Last match ending at or before `before`, rejecting an empty match at `skip_empty`.
    fn prev_from(&self, rope: &Rope, before: ByteIdx, skip_empty: Option<ByteIdx>) -> Option<Match> {
        let re = self.regex.as_ref()?;
        // The last match in `hay` (searched left to right, as `find_in` does) ending by `limit`.
        let last_in = |hay: &[u8], limit: usize, skip: Option<usize>| {
            let mut best = None;
            let mut at = 0;
            let mut last_end = None;
            while let Some((s, e)) = self.find_at(re, hay, at, last_end) {
                if e > limit {
                    break;
                }
                if !(s == e && Some(s) == skip) {
                    best = Some((s, e));
                }
                (at, last_end) = step(hay, s, e);
            }
            best
        };
        if self.multiline {
            let text = rope.to_string();
            let (s, e) = last_in(text.as_bytes(), before.0, skip_empty.map(|p| p.0))?;
            return Some(Match {
                start: ByteIdx(s),
                end: ByteIdx(e),
            });
        }
        pos::each_line(rope, pos::line_of(rope, before), true, |start, line| {
            let Some(limit) = before.0.checked_sub(start.0) else {
                return ControlFlow::Continue(());
            };
            let skip = skip_empty.and_then(|p| p.0.checked_sub(start.0));
            match last_in(line.as_bytes(), limit, skip) {
                Some((s, e)) => ControlFlow::Break(Match {
                    start: ByteIdx(start.0 + s),
                    end: ByteIdx(start.0 + e),
                }),
                None => ControlFlow::Continue(()),
            }
        })
    }

    /// The first acceptable match in `hay` starting at or after `at`: not an empty match inside a
    /// UTF-8 sequence or at `skip_empty`, and a whole word when that option is on. A rejected
    /// match is retried one character after its start (as VS Code does).
    fn find_at(&self, re: &Regex, hay: &[u8], mut at: usize, skip_empty: Option<usize>) -> Option<(usize, usize)> {
        while at <= hay.len() {
            let m = re.find_at(hay, at)?;
            let (s, e) = (m.start(), m.end());
            let bad_empty = s == e && (Some(s) == skip_empty || !is_char_start(hay, s));
            if !bad_empty && (!self.whole_word || is_whole_word(hay, s, e)) {
                return Some((s, e));
            }
            if s >= hay.len() {
                return None;
            }
            at = next_char(hay, s);
        }
        None
    }

    /// `find_at` over raw bytes that may hold many lines (what `grep-searcher` hands over):
    /// each line (LF, CRLF or lone CR) is searched on its own unless the pattern is multi-line.
    fn find_bytes(&self, hay: &[u8], at: usize) -> Option<(usize, usize)> {
        let re = self.regex.as_ref()?;
        if self.multiline {
            return self.find_at(re, hay, at, None);
        }
        let mut start = hay[..at]
            .iter()
            .rposition(|&b| b == b'\n' || b == b'\r')
            .map_or(0, |i| i + 1);
        if at > 0 && hay[at - 1] == b'\r' && hay.get(at) == Some(&b'\n') {
            start = at + 1; // between the halves of a CRLF
        }
        while start <= hay.len() {
            let end = hay[start..]
                .iter()
                .position(|&b| b == b'\n' || b == b'\r')
                .map_or(hay.len(), |i| start + i);
            if let Some((s, e)) = self.find_at(re, &hay[start..end], at.saturating_sub(start), None) {
                return Some((start + s, start + e));
            }
            if end == hay.len() {
                break;
            }
            start = end + if hay[end..].starts_with(b"\r\n") { 2 } else { 1 };
        }
        None
    }
}

/// What replaces each match (VS Code's `ReplacePattern`). For plain text the text as typed. For a
/// regex: `$1`…`$99` a group (`$12` is group 12 if there is one, else group 1 then `2`; a group
/// that doesn't exist stays as typed), `$&` and `$0` the whole match, `$$` a `$`, and the escapes
/// `\n`, `\t`, `\\`. A group that didn't take part in the match is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Group(usize),
}

impl Replacement {
    pub fn new(template: &str, regex: bool) -> Self {
        if !regex {
            return Self {
                pieces: vec![Piece::Text(template.to_string())],
            };
        }
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.peek() {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some('\\') => text.push('\\'),
                    Some(&other) => {
                        text.push('\\');
                        text.push(other);
                    }
                    None => {
                        text.push('\\');
                        continue;
                    }
                },
                '$' => match chars.peek() {
                    Some('$') => text.push('$'),
                    Some('&') => {
                        pieces.push(Piece::Text(std::mem::take(&mut text)));
                        pieces.push(Piece::Group(0));
                    }
                    Some(&d) if d.is_ascii_digit() => {
                        chars.next();
                        let mut n = d as usize - '0' as usize;
                        if let Some(&d2) = chars.peek()
                            && d2.is_ascii_digit()
                        {
                            n = n * 10 + (d2 as usize - '0' as usize);
                        } else {
                            pieces.push(Piece::Text(std::mem::take(&mut text)));
                            pieces.push(Piece::Group(n));
                            continue;
                        }
                        pieces.push(Piece::Text(std::mem::take(&mut text)));
                        pieces.push(Piece::Group(n));
                    }
                    _ => {
                        text.push('$');
                        continue;
                    }
                },
                c => {
                    text.push(c);
                    continue;
                }
            }
            chars.next();
        }
        pieces.push(Piece::Text(text));
        pieces.retain(|p| !matches!(p, Piece::Text(t) if t.is_empty()));
        Self { pieces }
    }

    /// The replacement when it names no group: the same text for every match.
    pub fn literal(&self) -> Option<&str> {
        match self.pieces.as_slice() {
            [] => Some(""),
            [Piece::Text(t)] => Some(t),
            _ => None,
        }
    }

    fn expand(&self, caps: &regex::bytes::Captures, out: &mut String) {
        let group = |i: usize, out: &mut String| {
            if let Some(g) = caps.get(i) {
                out.push_str(&String::from_utf8_lossy(g.as_bytes()));
            }
        };
        for piece in &self.pieces {
            match piece {
                Piece::Text(t) => out.push_str(t),
                Piece::Group(0) => group(0, out),
                // VS Code's `_substitute`: drop trailing digits until the group exists.
                &Piece::Group(n) => {
                    let mut rest = String::new();
                    let mut i = n;
                    while i > 0 {
                        if i < caps.len() {
                            group(i, out);
                            break;
                        }
                        rest.insert(0, char::from(b'0' + (i % 10) as u8));
                        i /= 10;
                    }
                    if i == 0 {
                        out.push('$');
                    }
                    out.push_str(&rest);
                }
            }
        }
    }
}

/// Lets `grep-searcher` (workspace search) use the same matcher on files. Matches never contain a
/// line break unless the pattern is multi-line, so the searcher may search many lines at once.
impl grep_matcher::Matcher for Matcher {
    type Captures = NoCaptures;
    type Error = NoError;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<grep_matcher::Match>, NoError> {
        Ok(self
            .find_bytes(haystack, at)
            .map(|(s, e)| grep_matcher::Match::new(s, e)))
    }

    fn new_captures(&self) -> Result<NoCaptures, NoError> {
        Ok(NoCaptures::new())
    }

    fn line_terminator(&self) -> Option<LineTerminator> {
        (!self.multiline).then(|| LineTerminator::byte(b'\n'))
    }
}

/// VS Code's test for a regex that can match a line break: a newline, or `\n`, `\r`, `\W`.
fn names_line_break(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\n' => return true,
            '\\' if matches!(chars.next(), Some('n' | 'r' | 'W')) => return true,
            _ => {}
        }
    }
    false
}

/// Where to search next after the match `s..e`, and the end an empty match may not sit at
/// (an empty match right after a match is skipped, as `regex`'s iterators do).
fn step(hay: &[u8], s: usize, e: usize) -> (usize, Option<usize>) {
    if s == e {
        (if e < hay.len() { next_char(hay, e) } else { e + 1 }, None)
    } else {
        (e, Some(e))
    }
}

/// VS Code's whole-word rule: each end of the match is a boundary when it's the edge of the
/// haystack (a line), when the character outside it isn't a word character, or when the
/// character just inside it isn't one (so `.foo` matches in `a.foo`).
fn is_whole_word(hay: &[u8], s: usize, e: usize) -> bool {
    let word = |c: Option<char>| c.is_some_and(pos::is_word_char);
    let left = s == 0 || !word(char_before(hay, s)) || (e > s && !word(char_at(hay, s)));
    let right = e == hay.len() || !word(char_at(hay, e)) || (e > s && !word(char_before(hay, e)));
    left && right
}

fn is_char_start(hay: &[u8], i: usize) -> bool {
    hay.get(i).is_none_or(|&b| b & 0xC0 != 0x80)
}

/// The index after the character starting at `i` (invalid UTF-8 steps byte by byte).
fn next_char(hay: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < hay.len() && !is_char_start(hay, j) {
        j += 1;
    }
    j
}

fn char_at(hay: &[u8], i: usize) -> Option<char> {
    let end = next_char(hay, i).min(hay.len());
    std::str::from_utf8(&hay[i..end]).ok()?.chars().next()
}

fn char_before(hay: &[u8], i: usize) -> Option<char> {
    let mut s = i.checked_sub(1)?;
    while s > 0 && i - s < 4 && !is_char_start(hay, s) {
        s -= 1;
    }
    std::str::from_utf8(&hay[s..i]).ok()?.chars().next()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::buffer::Buffer;

    fn rope(text: &str) -> Rope {
        Buffer::from_str(text).rope
    }

    fn opts(regex: bool, case_sensitive: bool, whole_word: bool) -> Options {
        Options {
            regex,
            case_sensitive,
            whole_word,
        }
    }

    const PLAIN: Options = Options {
        regex: false,
        case_sensitive: false,
        whole_word: false,
    };

    /// `(start, end)` pairs of every match.
    fn all(text: &str, pattern: &str, o: Options) -> Vec<(usize, usize)> {
        let m = Matcher::new(pattern, o).unwrap();
        m.find_all(&rope(text)).iter().map(|m| (m.start.0, m.end.0)).collect()
    }

    fn next(text: &str, pattern: &str, o: Options, from: usize, wrap: bool) -> Option<(usize, usize)> {
        let m = Matcher::new(pattern, o).unwrap();
        m.find_next(&rope(text), ByteIdx(from), wrap)
            .map(|m| (m.start.0, m.end.0))
    }

    fn prev(text: &str, pattern: &str, o: Options, before: usize, wrap: bool) -> Option<(usize, usize)> {
        let m = Matcher::new(pattern, o).unwrap();
        m.find_prev(&rope(text), ByteIdx(before), wrap)
            .map(|m| (m.start.0, m.end.0))
    }

    #[test]
    fn adjacent_matches_are_separate() {
        assert_eq!(all("aaaa", "a", PLAIN), vec![(0, 1), (1, 2), (2, 3), (3, 4)]);
        assert_eq!(all("aaaa", "aa", PLAIN), vec![(0, 2), (2, 4)]);
        assert_eq!(all("aaa", "aa", PLAIN), vec![(0, 2)]);
    }

    #[test]
    fn plain_text_is_literal() {
        assert_eq!(all("a.b axb a.b", "a.b", PLAIN), vec![(0, 3), (8, 11)]);
        assert_eq!(all("(x) [y]", "(x)", PLAIN), vec![(0, 3)]);
        assert_eq!(all(r"a\nb", r"\n", PLAIN), vec![(1, 3)]);
    }

    #[test]
    fn empty_pattern_matches_nothing() {
        let m = Matcher::new("", PLAIN).unwrap();
        assert!(m.is_empty());
        let r = rope("abc");
        assert!(m.find_all(&r).is_empty());
        assert_eq!(m.find_next(&r, ByteIdx(0), true), None);
        assert_eq!(m.find_prev(&r, ByteIdx(3), true), None);
        assert!(Matcher::new("", opts(true, true, true)).unwrap().is_empty());
        assert!(all("", "a", PLAIN).is_empty());
    }

    #[test]
    fn matches_at_start_and_end_of_text_and_lines() {
        assert_eq!(all("foo bar foo", "foo", PLAIN), vec![(0, 3), (8, 11)]);
        assert_eq!(all("foo\nfoo\n", "foo", PLAIN), vec![(0, 3), (4, 7)]);
        let re = opts(true, true, false);
        assert_eq!(all("ab\nab\r\nab", "^ab$", re), vec![(0, 2), (3, 5), (7, 9)]);
        assert_eq!(all("ab\rab", "b$", re), vec![(1, 2), (4, 5)]);
    }

    #[test]
    fn crlf_and_lone_cr_end_lines() {
        // `$` sits before the CR, `.` and `\s` never eat the line break.
        let re = opts(true, true, false);
        assert_eq!(all("x \r\ny\r", r"\s+$", re), vec![(1, 2)]);
        assert_eq!(all("ab\r\ncd", "b.", re), vec![]);
        assert_eq!(all("ab\r\ncd", r"b\s*c", re), vec![]);
        assert_eq!(all("a\r\n\r\nb", "^$", re), vec![(3, 3)]);
        // The empty line after a final line break is searched too.
        assert_eq!(all("ab\r\n", "^$", re), vec![(4, 4)]);
        assert_eq!(prev("ab\n", "^$", re, 3, true), Some((3, 3)));
    }

    #[test]
    fn unicode_and_multibyte_boundaries() {
        let text = "héllo 界 héllo é";
        assert_eq!(all(text, "héllo", PLAIN), vec![(0, 6), (11, 17)]);
        assert_eq!(all(text, "界", PLAIN), vec![(7, 10)]);
        assert_eq!(all(text, "HÉLLO", PLAIN), vec![(0, 6), (11, 17)]);
        assert!(all(text, "HÉLLO", opts(false, true, false)).is_empty());
        // Empty matches land on char boundaries only.
        let re = opts(true, true, false);
        assert_eq!(all("é界", "x*", re), vec![(0, 0), (2, 2), (5, 5)]);
        assert_eq!(all("😀😀", "😀", PLAIN), vec![(0, 4), (4, 8)]);
        // Every match lies on char boundaries.
        let s = "aé界😀b";
        for (a, b) in all(s, ".", re) {
            assert!(s.is_char_boundary(a) && s.is_char_boundary(b));
        }
    }

    #[test]
    fn case_sensitivity() {
        assert_eq!(all("Foo foo FOO", "foo", PLAIN).len(), 3);
        assert_eq!(all("Foo foo FOO", "foo", opts(false, true, false)), vec![(4, 7)]);
        assert_eq!(all("Foo foo FOO", "f[o]+", opts(true, false, false)).len(), 3);
        assert_eq!(all("Foo foo FOO", "F[o]+", opts(true, true, false)), vec![(0, 3)]);
        // An inline flag in the regex wins.
        assert_eq!(all("Foo foo", "(?-i)foo", opts(true, false, false)), vec![(4, 7)]);
    }

    #[test]
    fn whole_word() {
        let ww = opts(false, false, true);
        assert_eq!(all("foo foobar barfoo foo_x (foo)", "foo", ww), vec![(0, 3), (25, 28)]);
        assert_eq!(all("a foo\nfoo", "foo", ww), vec![(2, 5), (6, 9)]);
        // Unicode letters are word characters.
        assert!(all("éfoo fooé", "foo", ww).is_empty());
        assert_eq!(all("界 foo", "foo", ww), vec![(4, 7)]);
        // A 4-byte letter is a word character too; a 4-byte symbol isn't.
        assert!(all("𠀀foo foo𠀀", "foo", ww).is_empty());
        assert_eq!(all("😀foo", "foo", ww), vec![(4, 7)]);
        // VS Code: a match that starts/ends with a non-word char is bounded at that end.
        assert_eq!(all("a.foo", ".foo", ww), vec![(1, 5)]);
        assert_eq!(all("x+ y", "x+", ww), vec![(0, 2)]);
        assert_eq!(all("ax+ y", "x+", ww), vec![]);
        // A rejected match is retried one char later, finding an overlapping whole word.
        assert_eq!(all("aa a", "a", ww), vec![(3, 4)]);
        assert_eq!(all("foo.foo", "o.f", ww), vec![]);
        assert_eq!(all("xa.a.a", "a.a", ww), vec![(3, 6)]);
    }

    #[test]
    fn options_combine() {
        let text = "Foo foo FOObar foo_ FOO";
        assert_eq!(all(text, "foo", opts(false, true, true)), vec![(4, 7)]);
        assert_eq!(
            all(text, "foo", opts(false, false, true)),
            vec![(0, 3), (4, 7), (20, 23)]
        );
        assert_eq!(all(text, "F.O", opts(true, true, true)), vec![(20, 23)]);
        assert_eq!(
            all(text, "f.o", opts(true, false, true)),
            vec![(0, 3), (4, 7), (20, 23)]
        );
        assert_eq!(all(text, "f.o", opts(true, false, false)).len(), 5);
    }

    #[test]
    fn invalid_regex_is_an_error() {
        let err = Matcher::new("(foo", opts(true, false, false)).unwrap_err();
        assert_eq!(err.to_string(), "unclosed group");
        assert!(!err.to_string().contains('\n'));
        assert!(Matcher::new("[a-", opts(true, true, true)).is_err());
        // The same text as plain text is fine.
        assert_eq!(all("(foo", "(foo", PLAIN), vec![(0, 4)]);
    }

    #[test]
    fn find_next_with_wrap() {
        let text = "foo bar foo baz";
        assert_eq!(next(text, "foo", PLAIN, 0, false), Some((0, 3)));
        assert_eq!(next(text, "foo", PLAIN, 1, false), Some((8, 11)));
        assert_eq!(next(text, "foo", PLAIN, 3, false), Some((8, 11)));
        assert_eq!(next(text, "foo", PLAIN, 9, false), None);
        assert_eq!(next(text, "foo", PLAIN, 9, true), Some((0, 3)));
        assert_eq!(next(text, "foo", PLAIN, 15, true), Some((0, 3)));
        // The only match: wrapping finds it again.
        assert_eq!(next("a foo", "foo", PLAIN, 5, true), Some((2, 5)));
        assert_eq!(next("abc", "x", PLAIN, 1, true), None);
        // Across lines.
        assert_eq!(next("x\ny\r\nfoo", "foo", PLAIN, 0, false), Some((5, 8)));
        // Matching starts at `from` but sees the text before it (`\b`, whole word).
        assert_eq!(next("foobar", "bar", opts(false, false, true), 3, false), None);
        assert_eq!(next("foobar", r"\bbar", opts(true, false, false), 3, false), None);
    }

    #[test]
    fn find_next_moves_past_an_empty_match() {
        let re = opts(true, true, false);
        let text = "ab\ncd";
        assert_eq!(next(text, "^", re, 0, false), Some((3, 3)));
        assert_eq!(next(text, "^", re, 3, false), None);
        assert_eq!(next(text, "^", re, 3, true), Some((0, 0)));
        assert_eq!(next(text, "^", re, 1, false), Some((3, 3)));
    }

    #[test]
    fn find_prev_with_wrap() {
        let text = "foo bar foo baz";
        assert_eq!(prev(text, "foo", PLAIN, 15, false), Some((8, 11)));
        assert_eq!(prev(text, "foo", PLAIN, 11, false), Some((8, 11)));
        assert_eq!(prev(text, "foo", PLAIN, 10, false), Some((0, 3)));
        assert_eq!(prev(text, "foo", PLAIN, 8, false), Some((0, 3)));
        assert_eq!(prev(text, "foo", PLAIN, 2, false), None);
        assert_eq!(prev(text, "foo", PLAIN, 2, true), Some((8, 11)));
        assert_eq!(prev(text, "foo", PLAIN, 0, true), Some((8, 11)));
        assert_eq!(prev("foo\nx\r\ny", "foo", PLAIN, 8, false), Some((0, 3)));
        // Matches are the same ones `find_all` finds: `aa` in `aaa` is only (0, 2).
        assert_eq!(prev("aaa", "aa", PLAIN, 3, false), Some((0, 2)));
        let re = opts(true, true, false);
        assert_eq!(prev("ab\ncd", "^", re, 3, false), Some((0, 0)));
        assert_eq!(prev("ab\ncd", "^", re, 0, true), Some((3, 3)));
    }

    #[test]
    fn find_in_a_range() {
        let text = "foo\nfoo foo\nfoo\nfoo";
        let m = Matcher::new("foo", PLAIN).unwrap();
        let r = rope(text);
        let got: Vec<_> = m
            .find_in(&r, ByteIdx(4)..ByteIdx(16))
            .iter()
            .map(|m| m.start.0)
            .collect();
        assert_eq!(got, vec![4, 8, 12]);
        // Partially covered matches overlap the range; lines are searched whole.
        let got: Vec<_> = m
            .find_in(&r, ByteIdx(6)..ByteIdx(9))
            .iter()
            .map(|m| m.start.0)
            .collect();
        assert_eq!(got, vec![4, 8]);
        // Whole-line search gives the same matches as `find_all` even mid-line.
        let m = Matcher::new("aa", PLAIN).unwrap();
        let r = rope("aaaa");
        assert_eq!(m.find_in(&r, ByteIdx(1)..ByteIdx(4)), m.find_all(&r));
    }

    #[test]
    fn multiline_patterns_span_lines() {
        // Plain text with a line break, e.g. Cmd+D on a selection over two lines.
        let m = Matcher::new("a\r\nb", PLAIN).unwrap();
        assert!(m.is_multiline());
        let text = "a\r\nb a\nb A\r\nB";
        assert_eq!(all(text, "a\r\nb", PLAIN), vec![(0, 4), (9, 13)]);
        assert_eq!(all(text, "a\nb", PLAIN), vec![(5, 8)]);
        let re = opts(true, true, false);
        assert_eq!(all("x\ny\nz", r"y\n", re), vec![(2, 4)]);
        assert_eq!(all("ab\r\ncd", r"^cd$", opts(true, true, false)), vec![(4, 6)]);
        assert_eq!(all("ab\r\ncd", r"b\r\nc", re), vec![(1, 5)]);
        assert_eq!(next("a\nb a\nb", "a\nb", PLAIN, 1, true), Some((4, 7)));
        assert_eq!(next("a\nb a\nb", "a\nb", PLAIN, 5, true), Some((0, 3)));
        assert_eq!(prev("a\nb a\nb", "a\nb", PLAIN, 6, true), Some((0, 3)));
        assert_eq!(prev("a\nb a\nb", "a\nb", PLAIN, 2, true), Some((4, 7)));
        // VS Code's rule: a line break in the text, or `\n`, `\r`, `\W` in a regex.
        assert!(Matcher::new("a\rb", PLAIN).unwrap().is_multiline());
        for pattern in ["a\nb", r"a\n", r"a\r", r"\W+"] {
            assert!(Matcher::new(pattern, re).unwrap().is_multiline(), "{pattern:?}");
        }
        assert_eq!(all("a b\nc", r"\W+", re), vec![(1, 2), (3, 4)]);
        assert_eq!(all("a\r\n\rb", r"\r", re), vec![(1, 2), (3, 4)]);
        // `^` / `$` still match at every line break, CRLF and lone CR included.
        assert_eq!(all("ab\r\ncd\ref", r"$\W+^", re), vec![(2, 4), (6, 7)]);
        // `\\n` is an escaped backslash, not a line break.
        assert!(!Matcher::new(r"\\n", re).unwrap().is_multiline());
        assert!(!Matcher::new(r"\s", re).unwrap().is_multiline());
    }

    /// Every match `grep_matcher`'s own iterator finds. A haystack has at most `len + 1`
    /// matches, so more means the matcher stopped advancing: fail instead of growing forever.
    fn grep_all(m: &Matcher, hay: &[u8]) -> Vec<(usize, usize)> {
        use grep_matcher::Matcher as _;
        let mut got = Vec::new();
        m.find_iter(hay, |g| {
            got.push((g.start(), g.end()));
            got.len() <= hay.len() + 1
        })
        .unwrap();
        assert!(
            got.len() <= hay.len() + 1,
            "grep_matcher isn't advancing: {:?}",
            &got[..10]
        );
        got
    }

    #[test]
    fn grep_matcher_agrees_with_rope_search() {
        let text = "foo Foo\r\nfoofoo foo_x\rfoo\n\nhé foo\n";
        let cases = [
            ("foo", PLAIN),
            ("foo", opts(false, true, true)),
            ("^foo", opts(true, false, false)),
            (r"o\b", opts(true, true, false)),
            ("o*", opts(true, true, false)),
            ("a\nb", PLAIN),
        ];
        for (pattern, o) in cases {
            let m = Matcher::new(pattern, o).unwrap();
            let want: Vec<_> = m.find_all(&rope(text)).iter().map(|m| (m.start.0, m.end.0)).collect();
            assert_eq!(grep_all(&m, text.as_bytes()), want, "{pattern:?} {o:?}");
            // `grep-searcher` may hand over many lines at once only when matches never span them.
            let terminator = grep_matcher::Matcher::line_terminator(&m).map(|t| t.as_bytes().to_vec());
            assert_eq!(terminator, (!m.is_multiline()).then(|| b"\n".to_vec()), "{pattern:?}");
        }
    }

    #[test]
    fn grep_searcher_finds_the_same_lines() {
        use grep_searcher::{Searcher, sinks};
        let text = "foo\nbar\nxfoo\nfoo_\n(foo)\n";
        let m = Matcher::new("foo", opts(false, true, true)).unwrap();
        let mut lines = Vec::new();
        Searcher::new()
            .search_slice(
                &m,
                text.as_bytes(),
                sinks::UTF8(|n, _| {
                    lines.push(n);
                    Ok(true)
                }),
            )
            .unwrap();
        assert_eq!(lines, vec![1, 5]);
    }

    #[test]
    fn single_line_matcher_never_spans_lines() {
        let re = opts(true, true, false);
        for pattern in [r"a\nb", r"\W", r"b\r"] {
            assert!(!Matcher::new_single_line(pattern, re).unwrap().is_multiline());
        }
        let text = "a b\nc\r\nd";
        let m = Matcher::new_single_line(r"\W", re).unwrap();
        let got: Vec<_> = m.find_all(&rope(text)).iter().map(|m| (m.start.0, m.end.0)).collect();
        assert_eq!(got, vec![(1, 2)]);
        assert_eq!(grep_all(&m, text.as_bytes()), vec![(1, 2)]);
        assert!(
            Matcher::new_single_line(r"a\nb", re)
                .unwrap()
                .find_all(&rope("a\nb"))
                .is_empty()
        );
        assert!(
            Matcher::new_single_line("a\nb", PLAIN)
                .unwrap()
                .find_all(&rope("a\nb"))
                .is_empty()
        );
        // `each_in_line` stops when asked, and stays linear on a long line of matches.
        let m = Matcher::new_single_line("ab", PLAIN).unwrap();
        let line = "ab".repeat(100_000);
        let mut n = 0;
        m.each_in_line(line.as_bytes(), |_, _| {
            n += 1;
            true
        });
        assert_eq!(n, 100_000);
        n = 0;
        m.each_in_line(line.as_bytes(), |_, _| {
            n += 1;
            n < 3
        });
        assert_eq!(n, 3);
    }

    #[test]
    fn long_lines_split_across_chunks() {
        // A line much longer than a rope chunk is searched whole (copied into scratch).
        let text = format!("{}needle{}\nneedle", "x".repeat(10_000), "é".repeat(5_000));
        let got = all(&text, "needle", opts(false, true, true));
        assert_eq!(got, vec![(20_007, 20_013)]);
        let got = all(&text, "needle", PLAIN);
        assert_eq!(got, vec![(10_000, 10_006), (20_007, 20_013)]);
        assert_eq!(next(&text, "é", PLAIN, 20_006, true), Some((10_006, 10_008)));
        assert_eq!(prev(&text, "x", PLAIN, 20_013, false), Some((9_999, 10_000)));
    }

    /// Every match by brute force, independent of the matcher's own line splitting, stepping and
    /// whole-word code: `str` regexes over each line (or the whole text), then VS Code's rule.
    fn reference_regex(pattern: &str, o: Options) -> regex::Regex {
        let source = if o.regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        regex::RegexBuilder::new(&source)
            .case_insensitive(!o.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .unwrap()
    }

    fn reference(text: &str, re: &regex::Regex, whole_word: bool, multiline: bool) -> Vec<(usize, usize)> {
        let mut lines = Vec::new();
        if multiline {
            lines.push((0, text));
        } else {
            let mut start = 0;
            let mut i = 0;
            let b = text.as_bytes();
            while i < b.len() {
                if b[i] == b'\n' || b[i] == b'\r' {
                    lines.push((start, &text[start..i]));
                    i += if b[i..].starts_with(b"\r\n") { 2 } else { 1 };
                    start = i;
                } else {
                    i += 1;
                }
            }
            lines.push((start, &text[start..]));
        }
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let whole = |line: &str, s: usize, e: usize| {
            let left = !word(line[..s].chars().next_back()) || (s < e && !word(line[s..].chars().next()));
            let right = !word(line[e..].chars().next()) || (s < e && !word(line[..e].chars().next_back()));
            left && right
        };
        let mut out = Vec::new();
        for (base, line) in lines {
            if !whole_word {
                out.extend(re.find_iter(line).map(|m| (base + m.start(), base + m.end())));
                continue;
            }
            // Leftmost match from `at`; a rejected one is retried one char after its start.
            let (mut at, mut last_end) = (0, None);
            while at <= line.len() {
                let Some(m) = re.find_at(line, at) else { break };
                let (s, e) = (m.start(), m.end());
                let next = s + line[s..].chars().next().map_or(1, char::len_utf8);
                if (s == e && last_end == Some(s)) || !whole(line, s, e) {
                    at = next;
                    continue;
                }
                out.push((base + s, base + e));
                (at, last_end) = if s == e { (next, None) } else { (e, Some(e)) };
            }
        }
        out
    }

    #[test]
    fn agrees_with_a_reference_on_random_text() {
        const PIECES: &[&str] = &[
            "a", "a", "aa", "A", "b", "x", "_", " ", ".", "é", "É", "界", "😀", "\n", "\r\n", "\r", "foo", "Foo", "𠀀",
        ];
        // (pattern, regex, multi-line)
        const PATTERNS: &[(&str, bool, bool)] = &[
            ("a", false, false),
            ("aa", false, false),
            ("A", false, false),
            ("é", false, false),
            ("a.", false, false),
            ("foo", false, false),
            ("a b", false, false),
            ("a.a", false, false),
            ("a\nb", false, true),
            ("a\r\nb", false, true),
            ("a", true, false),
            ("aa", true, false),
            (r"\b", true, false),
            ("^", true, false),
            ("$", true, false),
            ("^$", true, false),
            (".*", true, false),
            ("x*", true, false),
            ("a|aa", true, false),
            (r"\w+", true, false),
            (r"[^a]", true, false),
            (r"\.\w", true, false),
            (r"a\nb", true, true),
            (r"\n", true, true),
            (r"\W", true, true),
            (r"$\r?\n^", true, true),
            ("\r", false, true),
            ("a\nb", true, true),
            (r"a\r", true, true),
            (r"a?\n?", true, true),
            (r"\\n", true, false),
        ];
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut rand = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        // Compiling is the slow part in debug builds, so it's done once.
        let mut cases = Vec::new();
        for &(pattern, regex, multiline) in PATTERNS {
            for flags in 0..4 {
                let o = opts(regex, flags & 1 != 0, flags & 2 != 0);
                let m = Matcher::new(pattern, o).unwrap();
                // Stated here, not taken from the matcher, so the reference can't share a bug.
                assert_eq!(m.is_multiline(), multiline, "{pattern:?} {o:?}");
                cases.push((pattern, o, m, reference_regex(pattern, o), multiline));
            }
        }
        for round in 0..200 {
            // A few texts have lines longer than a rope chunk.
            let long = round % 50 == 0;
            let len = if long { 500 } else { rand(30) };
            let mut pieces: Vec<String> = (0..len).map(|_| PIECES[rand(PIECES.len())].to_string()).collect();
            if long {
                pieces.insert(rand(len), "é".repeat(2000));
            }
            let text = pieces.concat();
            let bounds: Vec<_> = (0..=text.len()).filter(|&i| text.is_char_boundary(i)).collect();
            let r = rope(&text);
            for (pattern, o, m, re, multiline) in &cases {
                let ctx = match long {
                    true => format!("{pattern:?} {o:?} in round {round}"),
                    false => format!("{pattern:?} {o:?} in {text:?}"),
                };
                let want = reference(&text, re, o.whole_word, *multiline);
                let found = m.find_all(&r);
                let got: Vec<_> = found.iter().map(|m| (m.start.0, m.end.0)).collect();
                assert_eq!(got, want, "find_all {ctx}");
                // Find next / previous from each match step through the same list, wrapping.
                // (Only around the ends of a long text: each step is a search of its own.)
                let steps = found
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| !long || i < 20 || i + 20 > found.len());
                for (i, m0) in steps {
                    let after = found[(i + 1) % found.len()];
                    assert_eq!(m.find_next(&r, m0.end, true), Some(after), "find_next {i} {ctx}");
                    let before = found[(i + found.len() - 1) % found.len()];
                    assert_eq!(m.find_prev(&r, m0.start, true), Some(before), "find_prev {i} {ctx}");
                }
                if found.is_empty() {
                    assert_eq!(m.find_next(&r, ByteIdx(0), true), None, "{ctx}");
                    assert_eq!(m.find_prev(&r, ByteIdx(text.len()), true), None, "{ctx}");
                }
                // A random range gets exactly the overlapping matches.
                let (a, b) = (bounds[rand(bounds.len())], bounds[rand(bounds.len())]);
                let range = ByteIdx(a.min(b))..ByteIdx(a.max(b));
                let overlapping: Vec<_> = found
                    .iter()
                    .copied()
                    .filter(|m| match m.start == m.end {
                        true => range.contains(&m.start) || m.start == range.end,
                        false => m.start < range.end && range.start < m.end,
                    })
                    .collect();
                assert_eq!(m.find_in(&r, range.clone()), overlapping, "find_in {range:?} {ctx}");
                // So does `grep-searcher`'s view of the same bytes.
                assert_eq!(grep_all(m, text.as_bytes()), want, "grep {ctx}");
                // And line by line, for a pattern that can't span lines.
                if !*multiline {
                    let mut lines = Vec::new();
                    pos::each_line(&r, 0, false, |start, line| {
                        m.each_in_line(line.as_bytes(), |s, e| {
                            lines.push((start.0 + s, start.0 + e));
                            true
                        });
                        ControlFlow::<()>::Continue(())
                    });
                    assert_eq!(lines, want, "each_in_line {ctx}");
                }
            }
        }
    }

    /// A 1 MB buffer, CRLF, with a token on every line (Cmd+Shift+L on a common word).
    /// Timings are asserted in release builds only: `cargo test --release search -- --nocapture`.
    #[test]
    fn find_all_in_one_mb_is_fast() {
        let mut text = String::new();
        let mut i = 0;
        while text.len() < 1_000_000 {
            text.push_str(&format!("\tlet item_{i} = héllo(item, 界, {i}); // item\r\n"));
            i += 1;
        }
        let r = rope(&text);
        let lines = i;
        for (pattern, o, per_line) in [
            ("item", PLAIN, 3),
            ("item", opts(false, true, true), 2),
            (r"item_\d+", opts(true, true, false), 1),
        ] {
            let m = Matcher::new(pattern, o).unwrap();
            let t = Instant::now();
            let found = m.find_all(&r);
            let took = t.elapsed();
            println!("find_all {pattern:?}: {} matches in {took:?}", found.len());
            assert_eq!(found.len(), lines * per_line);
            if !cfg!(debug_assertions) {
                assert!(
                    took < Duration::from_millis(50),
                    "find_all {pattern:?} too slow: {took:?}"
                );
            }
        }
        // Find next / previous when the only match is far away.
        let m = Matcher::new("not there", PLAIN).unwrap();
        let t = Instant::now();
        assert_eq!(m.find_next(&r, ByteIdx(text.len() / 2), true), None);
        assert_eq!(m.find_prev(&r, ByteIdx(text.len() / 2), true), None);
        let took = t.elapsed();
        println!("find_next + find_prev, no match: {took:?}");
        if !cfg!(debug_assertions) {
            assert!(
                took < Duration::from_millis(50),
                "find with no match too slow: {took:?}"
            );
        }
    }

    /// Every match's replacement, as (start, end, text).
    fn replaced(text: &str, pattern: &str, o: Options, with: &str) -> Vec<(usize, usize, String)> {
        let m = Matcher::new(pattern, o).unwrap();
        let r = rope(text);
        let mut out = Vec::new();
        let rep = Replacement::new(with, o.regex);
        m.replace_in(&r, ByteIdx(0)..ByteIdx(r.len()), &rep, |m, t| {
            out.push((m.start.0, m.end.0, t.to_string()))
        });
        out
    }

    #[test]
    fn replacement_groups_as_vs_code() {
        let re = opts(true, false, false);
        let one = |pattern: &str, with: &str, text: &str| -> String {
            replaced(text, pattern, re, with)
                .into_iter()
                .map(|(_, _, t)| t)
                .collect()
        };
        assert_eq!(one(r"(\w+)-(\w+)", "$2-$1", "ab-cd"), "cd-ab");
        assert_eq!(one(r"b+", "[$&]", "abbc"), "[bb]");
        assert_eq!(one(r"b+", "[$0]", "abbc"), "[bb]");
        assert_eq!(one(r"(b)", "$$1", "b"), "$1");
        // `$12` with one group: group 1, then "2"; a missing group stays as typed.
        assert_eq!(one(r"(b)", "$12", "b"), "b2");
        assert_eq!(one(r"(b)", "$5", "b"), "$5");
        assert_eq!(one(r"(b)", "$", "b"), "$");
        assert_eq!(one(r"(a)|(b)", "<$1>", "b"), "<>", "a group outside the match is empty");
        assert_eq!(one(r"b", r"x\ny\tz\\w\q", "b"), "x\ny\tz\\w\\q");
        // Groups see the line around the match (anchors, \b).
        assert_eq!(
            replaced("ab ab\nab", r"^(a)b", re, "$1!"),
            vec![(0, 2, "a!".to_string()), (6, 8, "a!".to_string())]
        );
        // Multi-line patterns expand from the whole text.
        assert_eq!(
            replaced("a\nb", r"(a)\n(b)", re, "$2$1"),
            vec![(0, 3, "ba".to_string())]
        );
        // Plain text: `$1` is literal, and whole word applies.
        assert_eq!(
            replaced("a $1 a", "a", opts(false, false, true), "$1"),
            vec![(0, 1, "$1".to_string()), (5, 6, "$1".to_string())]
        );
        assert_eq!(Replacement::new("$1", false).literal(), Some("$1"));
        assert_eq!(Replacement::new("x$$", true).literal(), Some("x$"));
        assert_eq!(Replacement::new("$1", true).literal(), None);
    }
}
