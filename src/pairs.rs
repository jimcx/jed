//! Auto-closing pairs at every cursor, VS Code's `typeWithInterceptors` / `deleteLeft` with the
//! default settings (`autoClosingBrackets` / `autoClosingQuotes` / `autoSurround`:
//! `languageDefined`, `autoClosingOvertype` / `autoClosingDelete`: `auto`), from the buffer's
//! `LanguageConfig`. Like VS Code each rule is all or nothing: it applies when every cursor
//! qualifies, else the char is typed plainly at all of them.
//!
//! - Type-over: typing a closing char right before a closer jed inserted itself (tracked in
//!   `AutoClosed`) steps over it (a quote after a backslash doesn't).
//! - Auto-close: an opening char at empty cursors inserts the pair, the cursor between, when the
//!   next char allows it (`LanguageConfig::closes_before`); `'` / `"` not after a word char
//!   (unless a Python string prefix letter is before every cursor), nor in a string / comment
//!   where the pair says so (`notIn`, read from the parse tree when there is a current one). A
//!   new undo step starts before it.
//! - Wrap: an opening char with every selection non-empty (and not whitespace only) wraps each
//!   one, keeping the inner text selected. Its own undo step.
//! - Delete: Backspace between an auto-inserted empty pair deletes both.
//!
//! Not done (VS Code has them): a bracket isn't held back by an unmatched closing bracket after
//! the cursor (`hasUnmatchedClosingBracketAfter`; `brackets::match_at` could answer it, but at a
//! tree descent per cursor, too slow at 60k cursors); no "neutral char"
//! re-tokenizing probe for quotes; without a current tree (a parse running, a file past the size
//! limit) `notIn` is skipped, where VS Code would not auto-close a line it can't tokenize cheaply.

use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx, Peek};
use crate::buffer::undo::EditKind;
use crate::edit::{Land, replace_each, replace_each_landing};
use crate::selection::{Selection, Selections};
use crate::syntax::{AutoPair, LanguageConfig, Scope, is_quote};
use crate::view::View;

/// The closers a view inserted by auto-closing that are still live (VS Code's
/// `AutoClosedAction`s): one action per auto-closing keystroke, one pair per cursor. An action
/// lives while every selection is an empty cursor strictly inside one of its pairs (after the
/// open char's start, before the closer's end: VS Code's `strictContainsRange`) and its pairs
/// stay on one line; its pairs follow the view's own edits, and any other change to the buffer
/// (undo, another view, line commands) drops them all.
#[derive(Debug, Clone, Default)]
pub struct AutoClosed {
    actions: Vec<Vec<Closed>>,
    /// The buffer revision the positions are for.
    revision: u64,
}

/// One auto-closed pair: the open char's start, the closer's start and end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Closed {
    open: ByteIdx,
    close: ByteIdx,
    end: ByteIdx,
}

impl AutoClosed {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Follows an edit made through this view, before it is applied to the buffer (at
    /// `revision`); then `edited`. Positions not current before the edit are dropped. The open
    /// char's start moves with text typed before it and the closer's end stays put on text typed
    /// after it, so neither grows (VS Code's `NeverGrowsWhenTypingAtEdges`); a deleted closer is
    /// gone. A line break typed (or pasted) at the cursors, which are all inside every action's
    /// pairs, makes them span lines: VS Code stops tracking those, so all go.
    pub fn follow(&mut self, revision: u64, changes: &ChangeSet) {
        if self.revision != revision || (!self.actions.is_empty() && changes.inserts_line_break()) {
            self.actions.clear();
        }
        if self.actions.is_empty() {
            return;
        }
        let m = changes.mapper();
        for action in &mut self.actions {
            for c in action.iter_mut() {
                c.open = m.map(c.open, Assoc::After);
                c.close = m.map(c.close, Assoc::After);
                c.end = m.map(c.end, Assoc::Before);
            }
            action.retain(|c| c.close < c.end);
        }
        self.actions.retain(|a| !a.is_empty());
    }

    /// After `follow`: the edit is applied, the buffer is at `revision`, `sels` are the view's
    /// new selections.
    pub fn edited(&mut self, revision: u64, sels: &Selections) {
        self.revision = revision;
        self.validate(revision, sels);
    }

    /// Drops what isn't current for the buffer at `revision`, and the actions `sels` have left
    /// (VS Code's `_validateAutoClosedActions`, run on every selection change: the app calls it
    /// after each input event).
    pub fn validate(&mut self, revision: u64, sels: &Selections) {
        if self.revision != revision {
            self.actions.clear();
            self.revision = revision;
        }
        if self.actions.is_empty() {
            return;
        }
        if sels.iter().any(|s| !s.is_empty()) {
            self.actions.clear();
            return;
        }
        // Both sorted: one pass per action.
        self.actions.retain(|action| {
            let mut j = 0;
            sels.iter().all(|s| {
                while j < action.len() && action[j].end <= s.head {
                    j += 1;
                }
                action.get(j).is_some_and(|c| c.open < s.head)
            })
        });
    }

    /// Whether a live auto-inserted closer starts at each position, asked in increasing order
    /// (a sorted selection list): one forward pass per action instead of a search per cursor.
    fn closers(&self) -> impl FnMut(ByteIdx) -> bool + '_ {
        let mut at = vec![0; self.actions.len()];
        move |pos| {
            let mut found = false;
            for (action, i) in self.actions.iter().zip(at.iter_mut()) {
                while *i < action.len() && action[*i].close < pos {
                    *i += 1;
                }
                found |= action.get(*i).is_some_and(|c| c.close == pos);
            }
            found
        }
    }
}

/// Typing `c` at every selection with the language's pairs: type-over, auto-close or wrap, in
/// VS Code's order. False when none applies (the caller types `c` plainly).
pub fn type_char(buf: &mut Buffer, view: &mut View, c: char) -> bool {
    let config = buf.syntax.language().config();
    if !config.is_pair_char(c) {
        return false;
    }
    view.auto_closed.validate(buf.revision(), &view.selections);
    if config.closes(c) && can_type_over(buf, view, c) {
        let mut utf8 = [0; 4];
        let text: &str = c.encode_utf8(&mut utf8);
        let len = c.len_utf8();
        replace_each(buf, view, EditKind::Insert, |_, s| {
            (s.head, ByteIdx(s.head.0 + len), text)
        });
        return true;
    }
    if let Some(pair) = config.pair_opened_by(c)
        && can_auto_close(buf, view, config, pair)
    {
        auto_close(buf, view, pair);
        return true;
    }
    if let Some(close) = config.surround_for(c)
        && can_surround(buf, view, c)
    {
        surround(buf, view, c, close);
        return true;
    }
    false
}

/// Backspace between an auto-inserted empty pair at every cursor deletes both chars; false
/// otherwise (the caller deletes as usual).
pub fn delete_pair(buf: &mut Buffer, view: &mut View) -> bool {
    view.auto_closed.validate(buf.revision(), &view.selections);
    if view.auto_closed.is_empty() {
        return false;
    }
    let Some(ranges) = pair_ranges(buf, view) else {
        return false;
    };
    let mut range = ranges.iter();
    replace_each(buf, view, EditKind::Delete, |_, _| {
        let (a, b) = range.next().copied().unwrap_or_default();
        (a, b, "")
    });
    true
}

/// Each cursor's empty auto-closed pair, if every cursor is in one.
fn pair_ranges(buf: &Buffer, view: &View) -> Option<Vec<(ByteIdx, ByteIdx)>> {
    let config = buf.syntax.language().config();
    let mut peek = Peek::new(&buf.rope);
    let mut closer_at = view.auto_closed.closers();
    view.selections
        .iter()
        .map(|s| {
            if !s.is_empty() || !closer_at(s.head) {
                return None;
            }
            let (open, close) = (peek.before(s.head)?, peek.after(s.head)?);
            config
                .pairs
                .iter()
                .any(|p| p.open == open && p.close == close)
                .then(|| {
                    (
                        ByteIdx(s.head.0 - open.len_utf8()),
                        ByteIdx(s.head.0 + close.len_utf8()),
                    )
                })
        })
        .collect()
}

fn can_type_over(buf: &Buffer, view: &View, c: char) -> bool {
    if view.auto_closed.is_empty() {
        return false;
    }
    let mut peek = Peek::new(&buf.rope);
    let mut closer_at = view.auto_closed.closers();
    view.selections.iter().all(|s| {
        s.is_empty()
            && closer_at(s.head)
            && peek.after(s.head) == Some(c)
            && !(is_quote(c) && peek.before(s.head) == Some('\\'))
    })
}

fn can_auto_close(buf: &Buffer, view: &View, config: &LanguageConfig, pair: &AutoPair) -> bool {
    let mut peek = Peek::new(&buf.rope);
    // Python's `f'`, `r"`, …: VS Code picks such a pair when a prefix letter is before every
    // cursor (only the letter is checked, so `rb'` and `elif'` count); it may follow a word char
    // and is `notIn` strings and comments.
    let prefixed = is_quote(pair.open)
        && !config.string_prefixes.is_empty()
        && view
            .selections
            .iter()
            .all(|s| peek.before(s.head).is_some_and(|c| config.string_prefixes.contains(c)));
    let (not_in_string, not_in_comment) = if prefixed {
        (true, true)
    } else {
        (pair.not_in_string, pair.not_in_comment)
    };
    view.selections.iter().all(|s| {
        if !s.is_empty() || !config.closes_before(pair.open, peek.after(s.head)) {
            return false;
        }
        // VS Code: `'` and `"` don't auto-close after a word char (`don't`).
        if !prefixed && matches!(pair.open, '\'' | '"') && peek.before(s.head).is_some_and(pos::is_word_char) {
            return false;
        }
        if not_in_string || not_in_comment {
            match buf.syntax.scope_before(s.head) {
                Some(Scope::String) if not_in_string => return false,
                Some(Scope::Comment) if not_in_comment => return false,
                _ => {}
            }
        }
        true
    })
}

fn auto_close(buf: &mut Buffer, view: &mut View, pair: &AutoPair) {
    let text: String = [pair.open, pair.close].into_iter().collect();
    let (open, close) = (pair.open.len_utf8(), pair.close.len_utf8());
    // VS Code starts a new undo step before an auto-close; what's typed next joins it.
    buf.seal_undo();
    replace_each_landing(buf, view, EditKind::Insert, |_, s| {
        (s.head, s.head, text.as_str(), Land::Inside(open))
    });
    let action = view
        .selections
        .iter()
        .map(|s| Closed {
            open: ByteIdx(s.head.0 - open),
            close: s.head,
            end: ByteIdx(s.head.0 + close),
        })
        .collect();
    view.auto_closed.actions.push(action);
}

/// VS Code's `_isSurroundSelectionType`: every selection non-empty and not only whitespace, and
/// a quote isn't typed over a selected quote.
fn can_surround(buf: &Buffer, view: &View, c: char) -> bool {
    let rope = &buf.rope;
    view.selections.iter().all(|s| {
        let (start, end) = (s.start().0, s.end().0);
        if start == end {
            return false;
        }
        if is_quote(c) && end - start == 1 && pos::bytes_in(rope, start, end).any(|b| is_quote(b[0] as char)) {
            return false;
        }
        pos::bytes_in(rope, start, end).any(|chunk| chunk.iter().any(|b| !b" \t\r\n".contains(b)))
    })
}

/// Wraps every selection in `open` … `close`, the inner text staying selected (forwards, as
/// VS Code's `SurroundSelectionCommand`). One undo step.
fn surround(buf: &mut Buffer, view: &mut View, open: char, close: char) {
    let before = view.selections.clone();
    let (mut o, mut c) = ([0; 4], [0; 4]);
    let (open, close) = (&*open.encode_utf8(&mut o), &*close.encode_utf8(&mut c));
    let mut changes = ChangeSet::with_capacity(before.len() * 2, before.len() * 2);
    let mut out = Vec::with_capacity(before.len());
    let mut delta = 0;
    for s in before.iter() {
        changes.push(s.start(), s.start(), open);
        changes.push(s.end(), s.end(), close);
        delta += open.len();
        out.push(Selection::new(ByteIdx(s.start().0 + delta), ByteIdx(s.end().0 + delta)));
        delta += close.len();
    }
    let after = Selections::with_newest(out, before.primary_index(), before.newest_index());
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::command::{Command, Motion};
    use crate::edit::{self, Settings};
    use crate::syntax::Language;

    /// A buffer in `language` from `marked`: `|` is a cursor, `«…»` a selection (anchor, head).
    /// With `parse`, its tree is made current.
    pub(crate) fn setup(language: Language, marked: &str, parse: bool) -> (Buffer, View) {
        let (mut text, mut list, mut anchor) = (String::new(), Vec::new(), None);
        for c in marked.chars() {
            match c {
                '|' => list.push(Selection::cursor(ByteIdx(text.len()))),
                '«' => anchor = Some(text.len()),
                '»' => list.push(Selection::new(ByteIdx(anchor.take().unwrap()), ByteIdx(text.len()))),
                c => text.push(c),
            }
        }
        let mut buf = Buffer::from_str(&text);
        buf.syntax.set_language(language);
        if parse {
            reparse(&mut buf);
        }
        let mut view = View::new();
        view.selections = Selections::new(list, 0);
        (buf, view)
    }

    pub(crate) fn reparse(buf: &mut Buffer) {
        while let Some(job) = buf.syntax.update(&buf.rope, crate::syntax::DEFAULT_MAX_BYTES) {
            buf.syntax.parsed(job.run());
        }
        assert!(buf.syntax.tree().is_some());
    }

    /// The text with the selections marked as `setup` reads them.
    pub(crate) fn show(buf: &Buffer, view: &View) -> String {
        let text = buf.rope.to_string();
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in view.selections.iter() {
            if s.is_empty() {
                marks.push((s.head.0, '|'));
            } else {
                marks.push((s.anchor.0, '«'));
                marks.push((s.head.0, '»'));
            }
        }
        marks.sort_by_key(|&(p, c)| (p, c != '»'));
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

    fn typed(buf: &mut Buffer, view: &mut View, keys: &str) {
        let s = Settings::default();
        for c in keys.chars() {
            edit::insert_char(buf, view, c, &s);
            view.auto_closed.validate(buf.revision(), &view.selections);
        }
    }

    fn run(buf: &mut Buffer, view: &mut View, cmd: Command) {
        edit::run(cmd, buf, view, &Settings::default());
        view.auto_closed.validate(buf.revision(), &view.selections);
    }

    fn after(language: Language, marked: &str, keys: &str) -> String {
        let (mut b, mut v) = setup(language, marked, false);
        typed(&mut b, &mut v, keys);
        show(&b, &v)
    }

    #[test]
    fn brackets_auto_close_before_space_end_or_closer_only() {
        let cpp = Language::Cpp;
        assert_eq!(after(cpp, "foo|", "("), "foo(|)");
        assert_eq!(after(cpp, "foo| x", "["), "foo[|] x");
        assert_eq!(after(cpp, "f(|)", "{"), "f({|})", "before a closing bracket");
        assert_eq!(after(cpp, "f|;", "("), "f(|);");
        assert_eq!(
            after(cpp, "f|\"", "("),
            "f(|)\"",
            "C's default allows a quote after a bracket"
        );
        assert_eq!(after(cpp, "|bar", "("), "(|bar", "not before a word");
        assert_eq!(after(cpp, "|-1", "("), "(|-1");
        assert_eq!(after(cpp, "a|\r\nb", "("), "a(|)\r\nb", "before CRLF");
        // Plain text closes brackets (VS Code's plaintext brackets), not quotes.
        assert_eq!(after(Language::PlainText, "x |", "(\""), "x (\"|)");
        // Python uses VS Code's default `autoCloseBefore` too: a bracket closes before a quote.
        assert_eq!(after(Language::Python, "f|\"x\"", "("), "f(|)\"x\"");
        assert_eq!(
            after(Language::Python, "x = |`", "\""),
            "x = \"|`",
            "a quote not before `"
        );
    }

    #[test]
    fn quotes_not_after_a_word_except_python_prefixes() {
        assert_eq!(after(Language::Cpp, "x = |;", "\""), "x = \"|\";");
        assert_eq!(after(Language::Cpp, "it|", "'"), "it'|");
        assert_eq!(after(Language::Cpp, "f(|)", "'"), "f('|')");
        assert_eq!(after(Language::Python, "x = f|", "\""), "x = f\"|\"");
        // Only the letter before is checked (VS Code's `b'` pair): `rb'` closes, as does `elif'`.
        assert_eq!(after(Language::Python, "x = rb|", "'"), "x = rb'|'");
        assert_eq!(
            after(Language::Python, "x = az|", "'"),
            "x = az'|",
            "not a prefix letter"
        );
        // A prefix before only some cursors: the plain quote's rules, so none closes.
        assert_eq!(after(Language::Python, "f|\nx = |", "'"), "f'|\nx = '|");
        // A backtick isn't held back by a word before it (VS Code checks only ' and ").
        assert_eq!(after(Language::Python, "a|", "`"), "a`|`");
        assert_eq!(after(Language::Python, "x = |", "'"), "x = '|'");
        assert_eq!(after(Language::Python, "x = |a", "'"), "x = '|a", "not before a word");
    }

    #[test]
    fn quotes_not_in_strings_or_comments_with_a_tree() {
        let (mut b, mut v) = setup(Language::Cpp, "s = \"ab|\";", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "s = \"ab'|\";");
        let (mut b, mut v) = setup(Language::Cpp, "int a; // it |\n", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "int a; // it '|\n", "C's ' isn't closed in a comment");
        let (mut b, mut v) = setup(Language::Cpp, "int a; // it |\n", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "int a; // it \"|\"\n", "but \" is");
        // Python: ' isn't closed in a comment, " is (VS Code: `notIn: ["string"]`), and a
        // prefixed quote isn't (`f"` is `notIn` both).
        let (mut b, mut v) = setup(Language::Python, "x = 1  # it |\n", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "x = 1  # it '|\n");
        let (mut b, mut v) = setup(Language::Python, "x = 1  # it |\n", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "x = 1  # it \"|\"\n");
        let (mut b, mut v) = setup(Language::Python, "x = 1  # f|\n", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "x = 1  # f\"|\n");
        // Without a tree the rule is skipped.
        assert_eq!(after(Language::Python, "x = 1  # it |\n", "'"), "x = 1  # it '|'\n");
        // After a closing quote: still the string's last char.
        let (mut b, mut v) = setup(Language::Python, "x = 'a'|\n", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "x = 'a''|\n");
    }

    #[test]
    fn types_over_only_auto_inserted_closers() {
        assert_eq!(after(Language::Cpp, "foo|", "(a)"), "foo(a)|");
        assert_eq!(
            after(Language::Cpp, "f(|)", ")"),
            "f()|)",
            "typed by hand: no type-over"
        );
        assert_eq!(after(Language::Cpp, "|", "([x])"), "([x])|", "nested");
        assert_eq!(after(Language::Cpp, "x = |;", "\"a\""), "x = \"a\"|;");
        assert_eq!(
            after(Language::Cpp, "x = |;", "\"\\\""),
            "x = \"\\\"|\";",
            "not after a backslash"
        );
        // Enter inside makes the pair span lines (auto-indent puts the closer on a line of its
        // own): VS Code stops tracking it, so `}` is typed (and outdented), not typed over.
        let (mut b, mut v) = setup(Language::Cpp, "f|", false);
        typed(&mut b, &mut v, "{");
        run(&mut b, &mut v, Command::InsertNewline);
        assert_eq!(show(&b, &v), "f{\n    |\n}");
        typed(&mut b, &mut v, "}");
        assert_eq!(show(&b, &v), "f{\n}|\n}");
        // Typing inside (also a space) keeps it.
        assert_eq!(after(Language::Cpp, "f|", "{ x }"), "f{ x }|");
    }

    #[test]
    fn leaving_the_pair_ends_type_over() {
        let (mut b, mut v) = setup(Language::Cpp, "x|", false);
        typed(&mut b, &mut v, "(");
        let mv = |motion| Command::Move { motion, select: false };
        run(&mut b, &mut v, mv(Motion::Left));
        run(&mut b, &mut v, mv(Motion::Right));
        typed(&mut b, &mut v, ")");
        assert_eq!(
            show(&b, &v),
            "x()|)",
            "before the open char is outside (VS Code: strictly inside)"
        );
        let (mut b, mut v) = setup(Language::Cpp, "x|", false);
        typed(&mut b, &mut v, "(");
        run(&mut b, &mut v, mv(Motion::Right));
        run(&mut b, &mut v, mv(Motion::Left));
        typed(&mut b, &mut v, ")");
        assert_eq!(show(&b, &v), "x()|)", "after the closer is outside too");
        let (mut b, mut v) = setup(Language::Cpp, "x|", false);
        typed(&mut b, &mut v, "(");
        run(&mut b, &mut v, mv(Motion::LineStart));
        run(&mut b, &mut v, mv(Motion::LineEnd));
        run(&mut b, &mut v, mv(Motion::Left));
        typed(&mut b, &mut v, ")");
        assert_eq!(show(&b, &v), "x()|)", "left and came back");
        // Undo drops them too.
        let (mut b, mut v) = setup(Language::Cpp, "x|", false);
        typed(&mut b, &mut v, "(a");
        run(&mut b, &mut v, Command::Undo);
        run(&mut b, &mut v, Command::Redo);
        assert_eq!(show(&b, &v), "x(a|)");
        assert!(v.auto_closed.is_empty());
        typed(&mut b, &mut v, ")");
        assert_eq!(show(&b, &v), "x(a)|)");
        // Also when nothing validates in between: the next edit through the view drops them.
        let (mut b, mut v) = setup(Language::Cpp, "x|", false);
        typed(&mut b, &mut v, "(");
        edit::run(Command::Undo, &mut b, &mut v, &Settings::default());
        edit::run(Command::Redo, &mut b, &mut v, &Settings::default());
        typed(&mut b, &mut v, "a)");
        assert_eq!(show(&b, &v), "x(a)|)");
        // So does an edit made through another view.
        let (mut b, mut v) = setup(Language::Cpp, "x|\ny", false);
        typed(&mut b, &mut v, "(");
        b.take_applied();
        let mut other = View::new();
        other.selections = Selections::single(Selection::cursor(ByteIdx(b.rope.len())));
        edit::insert_char(&mut b, &mut other, 'z', &Settings::default());
        v.map_through(&b.take_applied(), &b.rope);
        typed(&mut b, &mut v, ")");
        assert_eq!(show(&b, &v), "x()|)\nyz");
    }

    #[test]
    fn backspace_deletes_an_auto_inserted_empty_pair() {
        let (mut b, mut v) = setup(Language::Cpp, "f|", false);
        typed(&mut b, &mut v, "([");
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(show(&b, &v), "f(|)");
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(show(&b, &v), "f|");
        // Typed by hand, or not empty: only the char before goes.
        let (mut b, mut v) = setup(Language::Cpp, "f(|)", false);
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(show(&b, &v), "f|)");
        let (mut b, mut v) = setup(Language::Cpp, "f|", false);
        typed(&mut b, &mut v, "(a");
        run(&mut b, &mut v, Command::DeleteLeft);
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(
            show(&b, &v),
            "f|",
            "emptied again, the pair is still the auto-inserted one"
        );
    }

    #[test]
    fn wraps_selections_keeping_the_inner_text_selected() {
        assert_eq!(after(Language::Cpp, "«abc» «de»", "("), "(«abc») («de»)");
        assert_eq!(after(Language::Cpp, "«abc»", "\""), "\"«abc»\"");
        assert_eq!(after(Language::Cpp, "«abc»", "[{"), "[{«abc»}]");
        // A backwards selection comes out forwards (VS Code).
        let (mut b, mut v) = setup(Language::Cpp, "|abc", false);
        v.selections = Selections::single(Selection::new(ByteIdx(3), ByteIdx(0)));
        typed(&mut b, &mut v, "(");
        assert_eq!(show(&b, &v), "(«abc»)");
        // Whitespace only, or a quote over a quote: typed over the selection.
        assert_eq!(after(Language::Cpp, "a« \t»b", "("), "a(|b");
        assert_eq!(after(Language::Cpp, "«\"»", "'"), "'|");
        assert_eq!(
            after(Language::PlainText, "«a»", "<"),
            "<«a»>",
            "plain text wraps in <>"
        );
        assert_eq!(after(Language::Cpp, "«a»", "<"), "<«a»>", "C/C++ too");
        assert_eq!(after(Language::Python, "«a»", "<"), "<|", "not Python");
        assert_eq!(after(Language::Python, "«a»", "`"), "`«a»`");
        // A selection over lines is wrapped as a whole.
        assert_eq!(after(Language::Cpp, "«a\nb»", "("), "(«a\nb»)");
        // Any selection empty: no wrap, and no auto-close either.
        assert_eq!(after(Language::Cpp, "«a» |", "("), "(| (|");
    }

    #[test]
    fn rules_apply_at_every_cursor_or_none() {
        assert_eq!(after(Language::Cpp, "a|\nb|\nc|", "("), "a(|)\nb(|)\nc(|)");
        assert_eq!(after(Language::Cpp, "a|\nb|\nc|", "(x)"), "a(x)|\nb(x)|\nc(x)|");
        assert_eq!(
            after(Language::Cpp, "a|\n|b\nc|", "("),
            "a(|\n(|b\nc(|",
            "one before a word"
        );
        let (mut b, mut v) = setup(Language::Cpp, "a|\nb|", false);
        typed(&mut b, &mut v, "(");
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(show(&b, &v), "a|\nb|");
        // Nested pairs at every cursor, typed over one action after the other.
        assert_eq!(after(Language::Cpp, "a|\nb|", "([])"), "a([])|\nb([])|");
        // One cursor's closer typed by hand (the second cursor is inside the action's outer
        // pair, not before its closer): `)` is typed at every cursor, Backspace deletes one char
        // at each.
        let hand = || {
            let (b, mut v) = setup(Language::Cpp, "a(|)\nb((|))", false);
            let at = |open, close, end| Closed {
                open: ByteIdx(open),
                close: ByteIdx(close),
                end: ByteIdx(end),
            };
            v.auto_closed.actions = vec![vec![at(1, 2, 3), at(5, 8, 9)]];
            (b, v)
        };
        let (mut b, mut v) = hand();
        typed(&mut b, &mut v, ")");
        assert_eq!(show(&b, &v), "a()|)\nb(()|))");
        let (mut b, mut v) = hand();
        run(&mut b, &mut v, Command::DeleteLeft);
        assert_eq!(show(&b, &v), "a|)\nb(|))");
    }

    #[test]
    fn undo_steps() {
        let (mut b, mut v) = setup(Language::Cpp, "|", false);
        typed(&mut b, &mut v, "foo(bar)");
        assert_eq!(show(&b, &v), "foo(bar)|");
        run(&mut b, &mut v, Command::Undo);
        assert_eq!(
            show(&b, &v),
            "foo|",
            "a new step starts at the auto-close; type-over joins it"
        );
        run(&mut b, &mut v, Command::Undo);
        assert_eq!(show(&b, &v), "|");
        let (mut b, mut v) = setup(Language::Cpp, "x«ab»", false);
        typed(&mut b, &mut v, "(");
        typed(&mut b, &mut v, "[");
        run(&mut b, &mut v, Command::Undo);
        assert_eq!(show(&b, &v), "x(«ab»)", "each wrap is a step");
        run(&mut b, &mut v, Command::Undo);
        assert_eq!(show(&b, &v), "x«ab»");
    }

    #[test]
    fn off_by_setting() {
        let s = Settings {
            auto_close_pairs: false,
            ..Settings::default()
        };
        let (mut b, mut v) = setup(Language::Cpp, "f|", false);
        for c in "(".chars() {
            edit::insert_char(&mut b, &mut v, c, &s);
        }
        assert_eq!(show(&b, &v), "f(|");
        let (mut b, mut v) = setup(Language::Cpp, "«a»", false);
        edit::insert_char(&mut b, &mut v, '(', &s);
        assert_eq!(show(&b, &v), "(|");
        // Pairs made while it was on aren't typed over or deleted while it's off.
        let (mut b, mut v) = setup(Language::Cpp, "f|", false);
        typed(&mut b, &mut v, "(");
        edit::run(Command::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(show(&b, &v), "f|)");
    }

    /// The new languages' pairs (VS Code's configurations).
    #[test]
    fn pairs_in_other_languages() {
        use Language::*;
        // Rust: `'` starts lifetimes, so it doesn't auto-close; `"` does, `<>` only wraps.
        assert_eq!(after(Rust, "fn f<|", "'"), "fn f<'|");
        assert_eq!(after(Rust, "x = |;", "\""), "x = \"|\";");
        assert_eq!(after(Rust, "«T»", "<"), "<«T»>");
        assert_eq!(after(Rust, "x|", "<"), "x<|");
        // JSON: brackets and quotes auto-close, but not in a string (with the tree).
        assert_eq!(after(Json, "|", "{\""), "{\"|\"}");
        assert_eq!(after(Json, "|", "["), "[|]");
        let (mut b, mut v) = setup(Json, "{\"a |\": 1}", true);
        typed(&mut b, &mut v, "[");
        assert_eq!(show(&b, &v), "{\"a [|\": 1}", "not in a string");
        let (mut b, mut v) = setup(Json, "{\"a | b\": 1}", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "{\"a \"| b\": 1}", "no quote pair in a string");
        let (mut b, mut v) = setup(Jsonc, "// a |\n{}", true);
        typed(&mut b, &mut v, "\"(");
        assert_eq!(
            show(&b, &v),
            "// a \"(|)\n{}",
            "no quote pair in a comment, brackets yes"
        );
        // YAML: both quotes, also in comments.
        assert_eq!(after(Yaml, "a: |", "'"), "a: '|'");
        let (mut b, mut v) = setup(Yaml, "# it |\n", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "# it \"|\"\n");
        // Markdown: `<` auto-closes; `*` / `_` / backticks wrap a selection.
        assert_eq!(after(Markdown, "see |", "<"), "see <|>");
        assert_eq!(after(Markdown, "a «b» c", "*"), "a *«b»* c");
        assert_eq!(after(Markdown, "a «b» c", "`"), "a `«b»` c");
        assert_eq!(after(Markdown, "a |", "*"), "a *|", "a cursor types it");
        assert_eq!(after(Markdown, "a |", "\""), "a \"|", "quotes don't auto-close");
        // XML: `"` not in an attribute value; `<` wraps only.
        assert_eq!(after(Xml, "<a x=|", "\""), "<a x=\"|\"");
        let (mut b, mut v) = setup(Xml, "<a x=\"y |\"/>", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "<a x=\"y '|\"/>", "' not in a string");
        assert_eq!(after(Xml, "«a»", "<"), "<«a»>");
        // Shell: backticks too, not in strings.
        assert_eq!(after(ShellScript, "x=|", "`"), "x=`|`");
        let (mut b, mut v) = setup(ShellScript, "echo 'a |'\n", true);
        typed(&mut b, &mut v, "\"");
        assert_eq!(show(&b, &v), "echo 'a \"|'\n", "in a raw string");
        let (mut b, mut v) = setup(ShellScript, "echo \"$(f |)\"\n", true);
        typed(&mut b, &mut v, "'");
        assert_eq!(show(&b, &v), "echo \"$(f '|')\"\n", "a command substitution is code");
        // TOML: no `(` pair, quotes not in strings.
        assert_eq!(after(Toml, "x = |", "("), "x = (|");
        assert_eq!(after(Toml, "x = |", "'"), "x = '|'");
        // Makefile, CMake, Dockerfile.
        assert_eq!(after(Makefile, "x = $|", "("), "x = $(|)");
        assert_eq!(after(CMake, "set|", "("), "set(|)");
        assert_eq!(after(Dockerfile, "CMD [|]", "\""), "CMD [\"|\"]");
    }
}
