//! Text steps on one line of a `&str`: grapheme and word boundaries (VS Code's word classes), shared by
//! the text field and the editor's rope versions.

use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Space,
    Word,
    Punct,
}

/// Word characters for word movement, double-click and whole-word search: alphanumeric or `_`.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn class_of(g: &str) -> CharClass {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        CharClass::Space
    } else if is_word_char(c) {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// `word_right` within one line of text (no line break), from byte offset `off` (a grapheme
/// boundary): the text fields' word moves. The end of `text` stays there.
pub fn word_right_in(text: &str, off: usize) -> usize {
    let mut gs = text.grapheme_indices(true).skip_while(|&(i, _)| i < off).peekable();
    while gs.next_if(|&(_, g)| class_of(g) == CharClass::Space).is_some() {}
    if let Some(&(_, g)) = gs.peek() {
        let class = class_of(g);
        while gs.next_if(|&(_, g)| class_of(g) == class).is_some() {}
    }
    gs.peek().map_or(text.len(), |&(i, _)| i)
}

/// `word_left` within one line of text (no line break), from byte offset `off`: 0 stays there.
pub fn word_left_in(text: &str, off: usize) -> usize {
    let off = off.min(text.len());
    let mut gs = text
        .grapheme_indices(true)
        .rev()
        .skip_while(|&(i, _)| i >= off)
        .peekable();
    while gs.next_if(|&(_, g)| class_of(g) == CharClass::Space).is_some() {}
    let mut result = gs.peek().map_or(0, |&(i, g)| i + g.len());
    if let Some(&(_, g)) = gs.peek() {
        let class = class_of(g);
        while let Some((i, _)) = gs.next_if(|&(_, g)| class_of(g) == class) {
            result = i;
        }
    }
    result
}

/// The grapheme boundary after byte offset `off` in `text` (`text.len()` at the end).
pub fn next_grapheme_in(text: &str, off: usize) -> usize {
    text[off..].graphemes(true).next().map_or(off, |g| off + g.len())
}

/// The grapheme boundary before byte offset `off` in `text` (0 at the start).
pub fn prev_grapheme_in(text: &str, off: usize) -> usize {
    text[..off].graphemes(true).next_back().map_or(0, |g| off - g.len())
}

/// The run of same-class characters around byte offset `off` of one line (`word_range` without
/// the rope; the text fields' double click uses it).
/// At a class boundary, the run to the right wins; at the text's end, the run to the left.
pub fn word_range_in(text: &str, off: usize) -> (usize, usize) {
    let gs: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    let Some(i) = gs
        .iter()
        .position(|&(o, g)| off < o + g.len())
        .or(gs.len().checked_sub(1))
    else {
        return (off, off);
    };
    let class = class_of(gs[i].1);
    let mut a = i;
    while a > 0 && class_of(gs[a - 1].1) == class {
        a -= 1;
    }
    let mut b = i + 1;
    while b < gs.len() && class_of(gs[b].1) == class {
        b += 1;
    }
    let end = gs.get(b).map_or(text.len(), |&(o, _)| o);
    (gs[a].0, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_chars() {
        assert!(is_word_char('a') && is_word_char('_') && is_word_char('é') && is_word_char('9'));
        assert!(!is_word_char('.') && !is_word_char(' ') && !is_word_char('-'));
    }

    #[test]
    fn word_steps_on_a_line() {
        assert_eq!(word_right_in("foo  bar", 0), 3);
        assert_eq!(word_left_in("foo  bar", 8), 5);
        assert_eq!(word_range_in("foo.bar baz", 3), (3, 4));
        assert_eq!(next_grapheme_in("e\u{301}x", 0), 3);
        assert_eq!(prev_grapheme_in("e\u{301}x", 3), 0);
    }
}
