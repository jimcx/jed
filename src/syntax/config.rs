//! Per-language editing rules (VS Code's `language-configuration.json`): the line comment
//! token for comment line, and the pairs for auto-close, type-over, pair delete and wrap.

use super::Language;

/// One auto-closing pair (single chars: red has no multi-char pairs such as `/**`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPair {
    pub open: char,
    pub close: char,
    /// VS Code's `notIn`: not auto-closed when the char before the cursor is in a string /
    /// comment (needs a current parse tree; without one the rule is skipped).
    pub not_in_string: bool,
    pub not_in_comment: bool,
}

#[derive(Debug)]
pub struct LanguageConfig {
    /// Comment line's token (`//`, `#`); `None`: comment line does nothing.
    pub line_comment: Option<&'static str>,
    pub pairs: &'static [AutoPair],
    /// What typing an opening char wraps a selection in (VS Code's `surroundingPairs`).
    pub surround: &'static [(char, char)],
    /// A pair auto-closes only before one of these, the end of the line, or a closing char of a
    /// bracket pair (VS Code's `autoCloseBefore`; its defaults differ for quotes and brackets).
    pub close_before_quotes: &'static str,
    pub close_before_brackets: &'static str,
    /// Python's string prefixes (VS Code lists `r"`, `f'`, … as pairs of their own, `notIn`
    /// strings and comments): a quote right after one of these letters auto-closes although it
    /// follows a word char (as in VS Code, whatever comes before the letter: `rb'`, `elif'`).
    pub string_prefixes: &'static str,
}

const fn pair(open: char, close: char, not_in_string: bool, not_in_comment: bool) -> AutoPair {
    AutoPair {
        open,
        close,
        not_in_string,
        not_in_comment,
    }
}

const BRACKETS: [AutoPair; 3] = [
    pair('{', '}', false, false),
    pair('[', ']', false, false),
    pair('(', ')', false, false),
];

/// VS Code's `DEFAULT_AUTOCLOSE_BEFORE_LANGUAGE_DEFINED_QUOTES` / `_BRACKETS`.
const DEFAULT_BEFORE_QUOTES: &str = ";:.,=}])> \n\t";
const DEFAULT_BEFORE_BRACKETS: &str = "'\"`;:.,=}])> \n\t";

/// VS Code's built-in plaintext configuration (`modesRegistry.ts`): its brackets auto-close,
/// quotes don't; a selection can be wrapped in brackets, `<>`, quotes or backticks.
static PLAIN_TEXT: LanguageConfig = LanguageConfig {
    line_comment: None,
    pairs: &BRACKETS,
    surround: &[
        ('{', '}'),
        ('[', ']'),
        ('(', ')'),
        ('<', '>'),
        ('"', '"'),
        ('\'', '\''),
        ('`', '`'),
    ],
    close_before_quotes: DEFAULT_BEFORE_QUOTES,
    close_before_brackets: DEFAULT_BEFORE_BRACKETS,
    string_prefixes: "",
};

/// VS Code's `cpp` / `c` configuration (its `/*` and `/**` pairs left out).
static C_LIKE: LanguageConfig = LanguageConfig {
    line_comment: Some("//"),
    pairs: &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('\'', '\'', true, true),
        pair('"', '"', true, false),
    ],
    surround: &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('<', '>')],
    close_before_quotes: DEFAULT_BEFORE_QUOTES,
    close_before_brackets: DEFAULT_BEFORE_BRACKETS,
    string_prefixes: "",
};

/// VS Code's `python` configuration: `'` not in strings or comments, `"` and `` ` `` not in
/// strings, the default `autoCloseBefore`, and the `r` / `u` / `f` / `b` string prefixes.
static PYTHON: LanguageConfig = LanguageConfig {
    line_comment: Some("#"),
    pairs: &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('"', '"', true, false),
        pair('\'', '\'', true, true),
        pair('`', '`', true, false),
    ],
    surround: &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('`', '`')],
    close_before_quotes: DEFAULT_BEFORE_QUOTES,
    close_before_brackets: DEFAULT_BEFORE_BRACKETS,
    string_prefixes: "rRuUfFbB",
};

impl Language {
    pub fn config(self) -> &'static LanguageConfig {
        match self {
            Language::PlainText => &PLAIN_TEXT,
            Language::C | Language::Cpp => &C_LIKE,
            Language::Python => &PYTHON,
        }
    }
}

impl LanguageConfig {
    /// Whether typing `c` can do more than insert it: open or close a pair, or wrap a selection.
    /// Everything else takes the plain typing path.
    pub fn is_pair_char(&self, c: char) -> bool {
        self.pairs.iter().any(|p| p.open == c || p.close == c) || self.surround.iter().any(|s| s.0 == c)
    }

    pub fn pair_opened_by(&self, c: char) -> Option<&AutoPair> {
        self.pairs.iter().find(|p| p.open == c)
    }

    pub fn closes(&self, c: char) -> bool {
        self.pairs.iter().any(|p| p.close == c)
    }

    pub fn surround_for(&self, c: char) -> Option<char> {
        self.surround.iter().find(|s| s.0 == c).map(|s| s.1)
    }

    /// Whether a pair opened by `open` auto-closes before `next` (`None`: the end of the line or
    /// text). VS Code's `autoCloseBefore`, or a closing char of a bracket (`_isBeforeClosingBrace`:
    /// a close that isn't also an open, so not a quote).
    pub fn closes_before(&self, open: char, next: Option<char>) -> bool {
        let Some(next) = next else { return true };
        if next == '\r' || next == '\n' {
            return true;
        }
        let list = if is_quote(open) {
            self.close_before_quotes
        } else {
            self.close_before_brackets
        };
        list.contains(next) || (self.closes(next) && self.pair_opened_by(next).is_none())
    }
}

pub fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}
