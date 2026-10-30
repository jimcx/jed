//! Per-language editing rules (VS Code's `language-configuration.json`): the line comment
//! token for comment line, the pairs for auto-close, type-over, pair delete and wrap, and the
//! bracket pairs for auto-indent and bracket matching.

use super::Language;

/// One auto-closing pair (single chars: jed has no multi-char pairs such as `/**`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPair {
    pub open: char,
    pub close: char,
    /// VS Code's `notIn`: not auto-closed when the char before the cursor is in a string /
    /// comment (needs a current parse tree; without one the rule is skipped).
    pub not_in_string: bool,
    pub not_in_comment: bool,
}

#[derive(Debug, PartialEq)]
pub struct LanguageConfig {
    /// Comment line's token (`//`, `#`); `None`: comment line does nothing.
    pub line_comment: Option<&'static str>,
    pub pairs: &'static [AutoPair],
    /// The bracket pairs (VS Code's `brackets`, single chars only): auto-indent and bracket
    /// matching.
    pub brackets: &'static [(char, char)],
    /// Whether the parse tree's leaves are the language's tokens, so a bracket counts only as a
    /// token of its own (not in a string or comment). False for Markdown, whose block grammar
    /// keeps a paragraph's text in one leaf: its brackets are found in the text.
    pub tree_tokens: bool,
    /// VS Code's language-specific `editor.insertSpaces` default, over the user's setting
    /// (Makefiles indent recipes with tabs).
    pub insert_spaces: Option<bool>,
    /// A language's default `rulers` (VS Code's language-specific `editor.rulers`), replacing the
    /// user's setting (a commit message wraps at 72).
    pub rulers: Option<&'static [usize]>,
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

pub(super) const BRACKETS: [AutoPair; 3] = [
    pair('{', '}', false, false),
    pair('[', ']', false, false),
    pair('(', ')', false, false),
];

/// `{}` `[]` `()`: most languages' `brackets`.
pub(super) const BRACKET_PAIRS: &[(char, char)] = &[('{', '}'), ('[', ']'), ('(', ')')];
/// Brackets and both quotes: most languages' `surroundingPairs`.
pub(super) const SURROUND: &[(char, char)] = &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\'')];

/// VS Code's `DEFAULT_AUTOCLOSE_BEFORE_LANGUAGE_DEFINED_QUOTES` / `_BRACKETS`.
const DEFAULT_BEFORE_QUOTES: &str = ";:.,=}])> \n\t";
const DEFAULT_BEFORE_BRACKETS: &str = "'\"`;:.,=}])> \n\t";

/// VS Code's built-in plaintext configuration (`modesRegistry.ts`): its brackets auto-close,
/// quotes don't; a selection can be wrapped in brackets, `<>`, quotes or backticks.
static PLAIN_TEXT: LanguageConfig = LanguageConfig {
    line_comment: None,
    pairs: &BRACKETS,
    brackets: BRACKET_PAIRS,
    tree_tokens: true,
    insert_spaces: None,
    rulers: None,
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
    brackets: BRACKET_PAIRS,
    tree_tokens: true,
    insert_spaces: None,
    rulers: None,
    surround: &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('<', '>')],
    close_before_quotes: DEFAULT_BEFORE_QUOTES,
    close_before_brackets: DEFAULT_BEFORE_BRACKETS,
    string_prefixes: "",
};

/// VS Code's `swift` configuration: `//`, brackets, and `"` (not in strings; `"""""` and `\(`
/// have no pairs of their own, so `(` closes in a string as anywhere). No `'` (a Swift string
/// is `"`-quoted; there is no character literal).
static SWIFT: LanguageConfig = LanguageConfig {
    pairs: &[BRACKETS[0], BRACKETS[1], BRACKETS[2], pair('"', '"', true, false)],
    surround: &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('<', '>')],
    ..C_LIKE
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
    brackets: BRACKET_PAIRS,
    tree_tokens: true,
    insert_spaces: None,
    rulers: None,
    surround: &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('`', '`')],
    close_before_quotes: DEFAULT_BEFORE_QUOTES,
    close_before_brackets: DEFAULT_BEFORE_BRACKETS,
    string_prefixes: "rRuUfFbB",
};

/// A configuration with VS Code's default `autoCloseBefore`, no string prefixes, the parse
/// tree's tokens and the user's indentation.
pub(super) const fn config(
    line_comment: Option<&'static str>,
    pairs: &'static [AutoPair],
    brackets: &'static [(char, char)],
    surround: &'static [(char, char)],
) -> LanguageConfig {
    LanguageConfig {
        line_comment,
        pairs,
        brackets,
        tree_tokens: true,
        insert_spaces: None,
        rulers: None,
        surround,
        close_before_quotes: DEFAULT_BEFORE_QUOTES,
        close_before_brackets: DEFAULT_BEFORE_BRACKETS,
        string_prefixes: "",
    }
}

/// VS Code's `rust`: only `"` auto-closes among the quotes (`'` starts lifetimes), not in strings;
/// wraps in `<>` too.
static RUST: LanguageConfig = config(
    Some("//"),
    &[BRACKETS[0], BRACKETS[1], BRACKETS[2], pair('"', '"', true, false)],
    BRACKET_PAIRS,
    &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('<', '>')],
);

/// VS Code's `markdown`: block comments only (none here), `<` auto-closes outside strings, and a
/// selection wraps in emphasis / code marks as well.
static MARKDOWN: LanguageConfig = LanguageConfig {
    tree_tokens: false,
    ..config(
        None,
        &[BRACKETS[0], BRACKETS[1], BRACKETS[2], pair('<', '>', true, false)],
        BRACKET_PAIRS,
        &[
            ('(', ')'),
            ('[', ']'),
            ('`', '`'),
            ('_', '_'),
            ('*', '*'),
            ('{', '}'),
            ('\'', '\''),
            ('"', '"'),
            ('<', '>'),
            ('~', '~'),
            ('$', '$'),
        ],
    )
};

/// VS Code's `yaml`: `#`, both quotes auto-close anywhere.
static YAML: LanguageConfig = config(
    Some("#"),
    &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('"', '"', false, false),
        pair('\'', '\'', false, false),
    ],
    BRACKET_PAIRS,
    SURROUND,
);

/// VS Code's `json` / `jsonc` pairs: nothing auto-closes in a string, `"` and `` ` `` not in a
/// comment either; brackets are `{}` and `[]`.
const JSON_PAIRS: &[AutoPair] = &[
    pair('{', '}', true, false),
    pair('[', ']', true, false),
    pair('(', ')', true, false),
    pair('\'', '\'', true, false),
    pair('"', '"', true, true),
    pair('`', '`', true, true),
];
const JSON_BRACKETS: &[(char, char)] = &[('{', '}'), ('[', ']')];
const JSON_SURROUND: &[(char, char)] = &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('`', '`')];

/// JSON has no comments (VS Code's shared configuration gives it `//`, which then shows as an
/// error).
static JSON: LanguageConfig = config(None, JSON_PAIRS, JSON_BRACKETS, JSON_SURROUND);
static JSONC: LanguageConfig = config(Some("//"), JSON_PAIRS, JSON_BRACKETS, JSON_SURROUND);

/// VS Code's `xml`: block comments only (none here); `'` not in strings or comments, `"` not in
/// strings; brackets `<>` `{}` `()`; wraps in `<>` too.
static XML: LanguageConfig = config(
    None,
    &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('\'', '\'', true, true),
        pair('"', '"', true, false),
    ],
    &[('<', '>'), ('{', '}'), ('(', ')')],
    &[('\'', '\''), ('"', '"'), ('{', '}'), ('[', ']'), ('(', ')'), ('<', '>')],
);

/// TOML (no VS Code built-in; Even Better TOML's): `#`, brackets `{}` `[]`, quotes not in strings.
static TOML: LanguageConfig = config(
    Some("#"),
    &[
        BRACKETS[0],
        BRACKETS[1],
        pair('"', '"', true, false),
        pair('\'', '\'', true, false),
    ],
    &[('{', '}'), ('[', ']')],
    &[('{', '}'), ('[', ']'), ('"', '"'), ('\'', '\'')],
);

/// VS Code's `shellscript`: `#`; quotes and backticks not in strings.
static SHELL: LanguageConfig = config(
    Some("#"),
    &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('"', '"', true, false),
        pair('\'', '\'', true, false),
        pair('`', '`', true, false),
    ],
    BRACKET_PAIRS,
    &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('`', '`')],
);

/// CMake (no VS Code built-in; the CMake extension's): `#`, `"` not in strings.
static CMAKE: LanguageConfig = config(
    Some("#"),
    &[BRACKETS[0], BRACKETS[1], BRACKETS[2], pair('"', '"', true, false)],
    BRACKET_PAIRS,
    &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"')],
);

/// VS Code's `makefile`: `#`, `'` not in strings or comments, `"` not in strings; indented with
/// tabs (its `[makefile] editor.insertSpaces: false` default: a recipe line must start with one).
static MAKEFILE: LanguageConfig = LanguageConfig {
    insert_spaces: Some(false),
    ..config(
        Some("#"),
        &[
            BRACKETS[0],
            BRACKETS[1],
            BRACKETS[2],
            pair('\'', '\'', true, true),
            pair('"', '"', true, false),
        ],
        BRACKET_PAIRS,
        &[('{', '}'), ('[', ']'), ('(', ')'), ('"', '"'), ('\'', '\''), ('`', '`')],
    )
};

/// VS Code's `dockerfile`: `#`, quotes not in strings.
static DOCKERFILE: LanguageConfig = config(
    Some("#"),
    &[
        BRACKETS[0],
        BRACKETS[1],
        BRACKETS[2],
        pair('"', '"', true, false),
        pair('\'', '\'', true, false),
    ],
    BRACKET_PAIRS,
    SURROUND,
);

/// `git-commit`: `#` comments, brackets `[` `(` auto-close, no quotes (apostrophes in prose).
static GIT_COMMIT: LanguageConfig = LanguageConfig {
    rulers: Some(&[72]),
    ..config(
        Some("#"),
        &[BRACKETS[1], BRACKETS[2]],
        &[('[', ']'), ('(', ')')],
        SURROUND,
    )
};

/// `git-rebase-todo`: `#` comments only.
static GIT_REBASE: LanguageConfig = config(Some("#"), &[], &[], SURROUND);

impl Language {
    pub fn config(self) -> &'static LanguageConfig {
        match self {
            Language::PlainText => &PLAIN_TEXT,
            Language::C | Language::Cpp | Language::ObjC | Language::ObjCpp => &C_LIKE,
            Language::Swift => &SWIFT,
            Language::Python => &PYTHON,
            Language::Rust => &RUST,
            Language::Markdown => &MARKDOWN,
            Language::Yaml => &YAML,
            Language::Json => &JSON,
            Language::Jsonc => &JSONC,
            Language::Xml => &XML,
            Language::Toml => &TOML,
            Language::ShellScript => &SHELL,
            Language::CMake => &CMAKE,
            Language::Makefile => &MAKEFILE,
            Language::Dockerfile => &DOCKERFILE,
            Language::GitCommit => &GIT_COMMIT,
            Language::GitRebase => &GIT_REBASE,
            Language::Plugin(p) => &p.get().config,
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

    /// The bracket pairs: what auto-indent and bracket matching use (VS Code's `brackets`).
    pub fn brackets(&self) -> impl Iterator<Item = (char, char)> + '_ {
        self.brackets.iter().copied()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_configs() {
        use Language::*;
        let comment = |l: Language| l.config().line_comment;
        assert_eq!(comment(Rust), Some("//"));
        // Objective-C / ++ are VS Code's C++ data: `//`, brackets, and `"` closes (so `@"` does).
        for l in [ObjC, ObjCpp] {
            assert_eq!(comment(l), Some("//"), "{l:?}");
            assert!(std::ptr::eq(l.config(), Cpp.config()), "{l:?}");
            assert!(
                l.config().pairs.iter().any(|p| p.open == '"' && p.close == '"'),
                "{l:?}"
            );
        }
        // Swift: `//`, brackets and `"` (not in strings), no `'`.
        assert_eq!(comment(Swift), Some("//"));
        let swift = Swift.config();
        assert!(
            swift
                .pairs
                .iter()
                .any(|p| p.open == '"' && p.not_in_string && !p.not_in_comment)
        );
        assert!(!swift.pairs.iter().any(|p| p.open == '\''));
        assert_eq!(swift.brackets, Cpp.config().brackets);
        assert!(swift.tree_tokens);
        assert_eq!(comment(Jsonc), Some("//"));
        for l in [
            Python,
            Yaml,
            Toml,
            ShellScript,
            CMake,
            Makefile,
            Dockerfile,
            GitCommit,
            GitRebase,
        ] {
            assert_eq!(comment(l), Some("#"), "{l:?}");
        }
        for l in [PlainText, Json, Markdown, Xml] {
            assert_eq!(comment(l), None, "{l:?}");
        }
        let brackets = |l: Language| l.config().brackets().collect::<Vec<_>>();
        assert_eq!(brackets(Json), [('{', '}'), ('[', ']')]);
        assert_eq!(brackets(Xml), [('<', '>'), ('{', '}'), ('(', ')')]);
        assert_eq!(
            brackets(Markdown),
            [('{', '}'), ('[', ']'), ('(', ')')],
            "`<` auto-closes but isn't one"
        );
        for l in Language::BUILT_IN {
            let c = l.config();
            // Every bracket's opener is typed through the pairs path (auto-close or wrap).
            assert!(c.brackets().all(|(o, _)| c.is_pair_char(o)), "{l:?}");
            assert_eq!(c.tree_tokens, l != Markdown, "{l:?}");
            assert_eq!(c.insert_spaces, (l == Makefile).then_some(false), "{l:?}");
        }
        assert!(Rust.config().pair_opened_by('\'').is_none());
        assert!(
            Json.config()
                .pair_opened_by('"')
                .is_some_and(|p| p.not_in_string && p.not_in_comment)
        );
    }
}
