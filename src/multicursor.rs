//! Cmd+D, Cmd+Shift+L and Cmd+Alt+D: selections at matches of the selected text, as VS Code's
//! `MultiCursorSession` (`editor/contrib/multicursor/browser/multicursor.ts`).
//!
//! A `Session` is what is searched for. It starts at the first of these commands and lasts while
//! only these commands change the selections; the app ends it on any other selection change or
//! when the editor loses focus, as VS Code does. Starting one:
//! - one empty cursor: the word under it, **whole word and case-sensitive** ("word mode"); the
//!   first Cmd+D just selects that word
//! - otherwise: the primary selection's exact text, literally, with the find widget's case and
//!   whole-word options (`find`; VS Code's default `editor.selectedTextMatchMode: findOptions`).
//!   Both default to off, so selected text matches case-insensitively, as in VS Code
//!
//! Searching starts after the newest selection (the one added last) and wraps; matches that are
//! already selected are skipped.

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx};
use crate::search::{Match, Matcher, Options};
use crate::selection::{Selection, Selections};
use crate::view::View;

#[derive(Debug)]
pub struct Session {
    matcher: Matcher,
    /// Searching for the word under an empty cursor (whole word, case-sensitive), not the
    /// find widget's options. 6d's option toggles end only sessions that use them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub word_mode: bool,
    /// The word under the empty cursor, which the first Cmd+D / Cmd+Alt+D selects
    /// (VS Code's `currentMatch`).
    pending: Option<Selection>,
}

impl Session {
    /// VS Code's `MultiCursorSession.create`. `None` for an empty cursor not touching a word.
    pub fn start(rope: &Rope, sels: &Selections, find: Options) -> Option<Session> {
        let primary = sels.primary();
        let word_mode = sels.len() == 1 && primary.is_empty();
        let (text, pending) = if primary.is_empty() {
            let (start, end) = pos::word_at(rope, primary.head)?;
            (rope.slice(start.0..end.0).to_string(), Some(Selection::new(start, end)))
        } else {
            (rope.slice(primary.start().0..primary.end().0).to_string(), None)
        };
        let opts = Options {
            regex: false,
            case_sensitive: word_mode || find.case_sensitive,
            whole_word: word_mode || find.whole_word,
        };
        // Plain text always compiles.
        let matcher = Matcher::new(&text, opts).ok()?;
        Some(Session {
            matcher,
            word_mode,
            pending,
        })
    }

    /// The next match after the newest selection, wrapping, that isn't selected yet.
    fn next_match(&mut self, rope: &Rope, sels: &Selections) -> Option<Selection> {
        if let Some(word) = self.pending.take() {
            return Some(word);
        }
        let m = next_unselected(&self.matcher, rope, sels, sels.newest().end())?;
        Some(Selection::new(m.start, m.end))
    }
}

/// Which Cmd+D variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Cmd+D: add a selection at the next match.
    Add,
    /// Cmd+Alt+D: move the newest selection to the next match.
    Skip,
}

/// Cmd+D / Cmd+Alt+D. `session` is the running session, started here if there is none.
pub fn next_match(rope: &Rope, view: &mut View, session: &mut Option<Session>, find: Options, step: Step) {
    // Several selections that don't all hold the same text: Cmd+D first expands each empty
    // cursor to its word, outside any session (VS Code does this for Cmd+D only).
    if step == Step::Add
        && session.is_none()
        && view.selections.len() > 1
        && !same_text(rope, &view.selections, find.case_sensitive)
    {
        view.selections.transform(|s| {
            if !s.is_empty() {
                return *s;
            }
            pos::word_at(rope, s.head).map_or(*s, |(a, b)| Selection::new(a, b))
        });
        return;
    }
    let sels = &view.selections;
    if session.is_none() {
        *session = Session::start(rope, sels, find);
    }
    let Some(s) = session else {
        return;
    };
    let Some(found) = s.next_match(rope, sels) else {
        return;
    };
    let mut list = sels.as_slice().to_vec();
    let mut primary = sels.primary_index();
    match step {
        Step::Add => list.push(found),
        Step::Skip => {
            let newest = sels.newest_index();
            list.remove(newest);
            if primary == newest {
                primary = list.len();
            } else if primary > newest {
                primary -= 1;
            }
            list.push(found);
        }
    }
    let newest = list.len() - 1;
    view.selections = Selections::with_newest(list, primary.min(newest), newest);
    view.reveal_range = Some((found.start(), found.end()));
}

/// Cmd+Shift+L: a selection at every match. The primary is the match touching the old primary
/// selection; the newest is the last one in VS Code's order (it swaps the primary to the front).
pub fn select_all(rope: &Rope, view: &mut View, session: &mut Option<Session>, find: Options) {
    if session.is_none() {
        *session = Session::start(rope, &view.selections, find);
    }
    let Some(s) = session else {
        return;
    };
    let matches = s.matcher.find_all(rope);
    if matches.is_empty() {
        return;
    }
    let p = view.selections.primary();
    let primary = matches
        .iter()
        .position(|m| m.start <= p.end() && p.start() <= m.end)
        .unwrap_or(0);
    let last = matches.len() - 1;
    let newest = if primary == last { 0 } else { last };
    let list = matches.iter().map(|m| Selection::new(m.start, m.end)).collect();
    view.selections = Selections::with_newest(list, primary, newest);
}

/// VS Code's `modelRangesContainSameText`: the primary's text in every selection (an empty one
/// other than the primary never counts).
fn same_text(rope: &Rope, sels: &Selections, case_sensitive: bool) -> bool {
    let text = |s: &Selection| {
        let t = rope.slice(s.start().0..s.end().0).to_string();
        if case_sensitive { t } else { t.to_lowercase() }
    };
    let first = text(sels.primary());
    sels.iter()
        .enumerate()
        .filter(|(i, _)| *i != sels.primary_index())
        .all(|(_, s)| !s.is_empty() && text(s) == first)
}

/// The first match starting at or after `from` (wrapping) that isn't exactly a selection.
fn next_unselected(matcher: &Matcher, rope: &Rope, sels: &Selections, from: ByteIdx) -> Option<Match> {
    let selected = |m: &Match| sels.contains_range(m.start, m.end);
    if matcher.is_multiline() {
        // Each `find_next` would copy the text; find them all once instead.
        let all = matcher.find_all(rope);
        let i = all.partition_point(|m| m.start < from);
        return all[i..].iter().chain(&all[..i]).find(|m| !selected(m)).copied();
    }
    let mut at = from;
    let mut first = None;
    loop {
        let m = matcher.find_next(rope, at, true)?;
        if first == Some(m) {
            return None; // every match is selected
        }
        first.get_or_insert(m);
        if !selected(&m) {
            return Some(m);
        }
        // Plain text is never empty, so this always moves on.
        at = m.end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(a: usize, h: usize) -> Selection {
        Selection::new(ByteIdx(a), ByteIdx(h))
    }

    fn view(sels: Vec<Selection>) -> View {
        let mut v = View::new();
        v.selections = Selections::new(sels, 0);
        v
    }

    fn ranges(v: &View) -> Vec<(usize, usize)> {
        v.selections.iter().map(|s| (s.start().0, s.end().0)).collect()
    }

    /// Runs `steps` of Cmd+D (`d`), Cmd+Alt+D (`k`) and Cmd+Shift+L (`l`) in one session.
    fn run(text: &str, v: &mut View, steps: &str) {
        let rope = Rope::from_str(text);
        let mut session = None;
        for c in steps.chars() {
            match c {
                'd' => next_match(&rope, v, &mut session, Options::default(), Step::Add),
                'k' => next_match(&rope, v, &mut session, Options::default(), Step::Skip),
                'l' => select_all(&rope, v, &mut session, Options::default()),
                _ => unreachable!(),
            };
        }
    }

    #[test]
    fn empty_cursor_selects_word_then_whole_words_case_sensitive() {
        let text = "foo Foo foobar foo_x foo";
        let mut v = view(vec![sel(1, 1)]);
        run(text, &mut v, "d");
        assert_eq!(ranges(&v), vec![(0, 3)]);
        let mut v = view(vec![sel(1, 1)]);
        run(text, &mut v, "dd");
        assert_eq!(ranges(&v), vec![(0, 3), (21, 24)], "skips Foo, foobar and foo_x");
    }

    #[test]
    fn selected_text_matches_literally_case_insensitively_anywhere() {
        let text = "a.b A.B xa.bx a+b";
        let mut v = view(vec![sel(0, 3)]);
        let rope = Rope::from_str(text);
        let mut session = None;
        for _ in 0..3 {
            next_match(&rope, &mut v, &mut session, Options::default(), Step::Add);
        }
        assert_eq!(ranges(&v), vec![(0, 3), (4, 7), (9, 12)], "`.` is literal; not a+b");
        assert!(!session.unwrap().word_mode);
        // With the find widget's Match Case on, only the exact case.
        let mut v = view(vec![sel(0, 3)]);
        let case = Options {
            case_sensitive: true,
            ..Options::default()
        };
        let mut session = None;
        next_match(&rope, &mut v, &mut session, case, Step::Add);
        assert_eq!(ranges(&v), vec![(0, 3), (9, 12)]);
        // With Match Whole Word on: not inside xa.bx (still any case).
        let mut v = view(vec![sel(0, 3)]);
        let whole = Options {
            whole_word: true,
            ..Options::default()
        };
        let mut session = None;
        for _ in 0..2 {
            next_match(&rope, &mut v, &mut session, whole, Step::Add);
        }
        assert_eq!(ranges(&v), vec![(0, 3), (4, 7)]);
    }

    /// Several empty cursors aren't word mode (VS Code: only a single collapsed selection is):
    /// Cmd+Shift+L searches for the primary's word with the find options.
    #[test]
    fn several_cursors_select_all_uses_find_options() {
        let text = "foo foo fooFOO";
        let mut v = view(vec![sel(3, 3), sel(7, 7)]);
        run(text, &mut v, "l");
        assert_eq!(ranges(&v), vec![(0, 3), (4, 7), (8, 11), (11, 14)]);
    }

    /// Selections holding the same text in different case count as the same text when the
    /// search ignores case: Cmd+D adds the next match instead of expanding words.
    #[test]
    fn same_text_ignores_case_unless_match_case() {
        let text = "ab AB xx ab";
        let rope = Rope::from_str(text);
        let mut v = view(vec![sel(0, 2), sel(3, 5)]);
        let mut session = None;
        next_match(&rope, &mut v, &mut session, Options::default(), Step::Add);
        assert_eq!(ranges(&v), vec![(0, 2), (3, 5), (9, 11)]);
        // Match Case: different texts; nothing is empty, so there's nothing to expand.
        let mut v = view(vec![sel(0, 2), sel(3, 5)]);
        let case = Options {
            case_sensitive: true,
            ..Options::default()
        };
        let mut session = None;
        next_match(&rope, &mut v, &mut session, case, Step::Add);
        assert_eq!(ranges(&v), vec![(0, 2), (3, 5)]);
        assert!(session.is_none());
    }

    #[test]
    fn search_starts_after_newest_and_wraps() {
        let text = "x x x x";
        let mut v = view(vec![sel(4, 4)]);
        run(text, &mut v, "ddd");
        assert_eq!(ranges(&v), vec![(0, 1), (4, 5), (6, 7)]);
        assert_eq!(v.selections.primary().start().0, 4, "primary stays the first selection");
        assert_eq!(v.selections.newest().start().0, 0, "wrapped");
        assert_eq!(v.reveal_range, Some((ByteIdx(0), ByteIdx(1))));
    }

    #[test]
    fn already_selected_matches_are_skipped() {
        let text = "x x x";
        // Cmd+D, Cmd+D, Cmd+Alt+D: [x0, x2]; the next Cmd+D wraps past x0 to x1.
        let mut v = view(vec![sel(0, 0)]);
        run(text, &mut v, "ddkd");
        assert_eq!(ranges(&v), vec![(0, 1), (2, 3), (4, 5)]);
        // All selected: Cmd+D does nothing.
        let before = v.selections.clone();
        run(text, &mut v, "d");
        assert_eq!(v.selections, before);
    }

    #[test]
    fn touching_matches_stay_separate() {
        let mut v = view(vec![sel(0, 1)]);
        run("aaaa", &mut v, "ddd");
        assert_eq!(ranges(&v), vec![(0, 1), (1, 2), (2, 3), (3, 4)]);
        let mut v = view(vec![sel(0, 1)]);
        run("aaaa", &mut v, "l");
        assert_eq!(v.selections.len(), 4);
    }

    #[test]
    fn skip_moves_the_newest_selection() {
        let text = "ab ab ab ab";
        let mut v = view(vec![sel(0, 0)]);
        run(text, &mut v, "k");
        assert_eq!(ranges(&v), vec![(0, 2)], "first press selects the word");
        let mut v = view(vec![sel(0, 0)]);
        run(text, &mut v, "ddkk");
        assert_eq!(ranges(&v), vec![(0, 2), (9, 11)]);
        assert_eq!(v.selections.primary().start().0, 0);
        // A lone selection: skipping moves it (and the primary with it).
        let mut v = view(vec![sel(3, 5)]);
        run(text, &mut v, "k");
        assert_eq!(ranges(&v), vec![(6, 8)]);
        // The newest sorts before the primary: Cmd+D from the last word wraps to the first,
        // then skipping it keeps the primary on the last word.
        let mut v = view(vec![sel(9, 9)]);
        run(text, &mut v, "ddk");
        assert_eq!(ranges(&v), vec![(3, 5), (9, 11)]);
        assert_eq!(v.selections.primary().start().0, 9);
        assert_eq!(v.selections.newest().start().0, 3);
    }

    #[test]
    fn multi_byte_words() {
        let text = "héllo wörld héllo héllox";
        let mut v = view(vec![sel(2, 2)]);
        run(text, &mut v, "dd");
        assert_eq!(ranges(&v), vec![(0, 6), (14, 20)]);
    }

    #[test]
    fn select_all_keeps_primary_at_the_invoking_match() {
        let text = "ab x ab x ab";
        let mut v = view(vec![sel(6, 6)]);
        run(text, &mut v, "l");
        assert_eq!(ranges(&v), vec![(0, 2), (5, 7), (10, 12)]);
        assert_eq!(v.selections.primary().start().0, 5);
        assert_eq!(v.selections.newest().start().0, 10);
        // From the last match: VS Code swaps it to the front, so the first match is newest.
        let mut v = view(vec![sel(11, 11)]);
        run(text, &mut v, "l");
        assert_eq!(v.selections.primary().start().0, 10);
        assert_eq!(v.selections.newest().start().0, 0);
        // A cursor on no word: nothing happens.
        let mut v = view(vec![sel(3, 3)]);
        run("ab  ab", &mut v, "l");
        assert_eq!(ranges(&v), vec![(3, 3)]);
    }

    #[test]
    fn different_texts_expand_cursors_to_words_first() {
        let text = "one two three";
        let mut v = view(vec![sel(1, 1), sel(5, 5), sel(8, 13)]);
        run(text, &mut v, "d");
        assert_eq!(ranges(&v), vec![(0, 3), (4, 7), (8, 13)]);
    }

    #[test]
    fn multi_line_selection_matches_across_lines() {
        let text = "a\nb\na\nb\n";
        let mut v = view(vec![sel(0, 3)]);
        run(text, &mut v, "dd");
        assert_eq!(ranges(&v), vec![(0, 3), (4, 7)]);
        // From the middle match: after it first, then wrapping, then nothing is left.
        let text = "a\nb a\nb a\nb";
        let after = |steps: &str| {
            let mut v = view(vec![sel(4, 7)]);
            run(text, &mut v, steps);
            v
        };
        assert_eq!(ranges(&after("d")), vec![(4, 7), (8, 11)]);
        assert_eq!(ranges(&after("dd")), vec![(0, 3), (4, 7), (8, 11)]);
        assert_eq!(after("dd").selections.newest().start().0, 0);
        assert_eq!(
            after("ddd").selections,
            after("dd").selections,
            "all selected: the newest stays the same too"
        );
    }
}
