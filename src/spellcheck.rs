//! The spell checker's model: word splitting, the bundled dictionaries, the checker and
//! suggestions. Pure: no I/O, no app state, no drawing (`spell_check-b` finds the ranges to
//! check, `-c` runs it in the editor, `-d` has the commands).
//!
//! **Splitting** ([`split_pieces`], [`split_words`]): a token is a run of letters, digits, `_` and
//! `'` (an apostrophe stays only between two letters: `don't`); it is split at `_`, at
//! lower→UPPER (`camelCase`), at an acronym's end (`HTTPServer` → `HTTP`, `Server`) and at
//! letter↔digit (`utf8Decode` → `utf`, `Decode`; digits are dropped, so a digit run never makes a
//! piece). Whole whitespace-separated chunks that are URLs, e-mail addresses or file paths, and
//! hex-looking tokens (`0xDEAD`, `3fa9b2c1`), give no pieces. [`split_words`] also drops what
//! isn't checked: pieces under `min_length` chars, ALL-CAPS pieces (acronyms) and pieces with a
//! letter outside the Latin scripts the dictionary covers (`日本語`).
//!
//! **Dictionaries**: Hunspell `.aff` + `.dic` pairs read by the `spellbook` crate (which does the
//! affix expansion, case rules and suggestions). Australian English (`en_AU`, [`DEFAULT_LANGUAGE`])
//! is compiled in with its licence (`assets/dictionaries/en_AU/`, fetched by
//! `scripts/fetch-dictionary`). The `spell_check_language` setting names another language: its
//! pair is read from `<config dir>/dictionaries/<lang>/<lang>.{aff,dic}` (same script, given that
//! directory), see [`Dictionary::load`]; one that is missing falls back to `en_AU`
//! ([`Dictionary::missing_language`] says so, the app tells the user once). Always bundled:
//! `code.txt` (programming terms, every language) and `lang/*.txt` (one small list per
//! [`Language`], read only by a [`Checker`] for that language, see [`language_terms`]), plain
//! lower-case word lists in a `HashSet`. The dictionary is built once and shared by every buffer
//! (`Arc`). Programming terms and user words (`extra`: sets checked in turn, so a buffer's ignored
//! words join the shared ones without a merged copy; they must already be lower-case) are matched
//! case-insensitively; the dictionary follows Hunspell's case rules (`colour`, `Colour` and
//! `COLOUR` pass; `australia` does not).
//!
//! **Suggestions** ([`Checker::suggest`]): spellbook's, at most [`MAX_SUGGESTIONS`]. Asked for one
//! word on demand, never per mark. Inside an identifier the caller passes just the piece
//! ([`Misspelling::word`]), so `recieveMessage` → `receive` replaces the piece only.

// A few helpers are only used by tests.
#![allow(dead_code)]

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use std::path::Path;

use crate::buffer::pos::ByteIdx;
use crate::syntax::Language;

/// The most suggestions [`Checker::suggest`] gives.
pub const MAX_SUGGESTIONS: usize = 8;
/// The language of the bundled dictionary, and the default `spell_check_language`.
pub const DEFAULT_LANGUAGE: &str = "en_AU";

const EN_AU_AFF: &str = include_str!("../assets/dictionaries/en_AU/en_AU.aff");
const EN_AU_DIC: &str = include_str!("../assets/dictionaries/en_AU/en_AU.dic");
const CODE: &str = include_str!("../assets/dictionaries/code.txt");
const RUST: &str = include_str!("../assets/dictionaries/lang/rust.txt");
const PYTHON: &str = include_str!("../assets/dictionaries/lang/python.txt");
const C: &str = include_str!("../assets/dictionaries/lang/c.txt");
const CPP: &str = include_str!("../assets/dictionaries/lang/cpp.txt");

/// Contractions the word list may lack (lower-case, apostrophe `'`).
const CONTRACTIONS: &[&str] = &[
    "ain't",
    "aren't",
    "can't",
    "couldn't",
    "didn't",
    "doesn't",
    "don't",
    "hadn't",
    "hasn't",
    "haven't",
    "he'd",
    "he'll",
    "he's",
    "i'd",
    "i'll",
    "i'm",
    "i've",
    "isn't",
    "it'd",
    "it'll",
    "it's",
    "let's",
    "mightn't",
    "mustn't",
    "shan't",
    "she'd",
    "she'll",
    "she's",
    "shouldn't",
    "that's",
    "there's",
    "they'd",
    "they'll",
    "they're",
    "they've",
    "wasn't",
    "we'd",
    "we'll",
    "we're",
    "we've",
    "weren't",
    "what's",
    "who's",
    "won't",
    "wouldn't",
    "you'd",
    "you'll",
    "you're",
    "you've",
    "o'clock",
];

/// The extra word lists for one language, besides `code.txt`: the keywords and standard library
/// names of that language only (empty for a language with none).
pub fn language_terms(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::Rust => &[RUST],
        Language::Python => &[PYTHON],
        Language::C | Language::ObjC => &[C],
        Language::Cpp | Language::ObjCpp => &[C, CPP],
        _ => &[],
    }
}

/// One word piece: its byte range in the text that was split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub range: Range<usize>,
}

impl Piece {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.range.clone()]
    }
}

/// A word not in any dictionary: `range` covers the piece only, `word` is its text (what
/// [`Checker::suggest`] and the add-word commands take).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    pub range: Range<ByteIdx>,
    pub word: String,
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// Every piece of `text` (see the module doc), before the `min_length`, acronym and script rules.
pub fn split_pieces(text: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    for_each_piece(text, &mut |r| out.push(Piece { range: r }));
    out
}

/// The pieces to check: [`split_pieces`] minus those under `min_length` chars, ALL-CAPS ones
/// and ones with a non-Latin letter.
pub fn split_words(text: &str, min_length: usize) -> Vec<Piece> {
    let mut out = Vec::new();
    for_each_piece(text, &mut |r| {
        if checkable(&text[r.clone()], min_length) {
            out.push(Piece { range: r });
        }
    });
    out
}

fn is_latin_letter(c: char) -> bool {
    let u = c as u32;
    c.is_alphabetic() && (u < 0x250 || (0x1E00..=0x1EFF).contains(&u))
}

fn checkable(piece: &str, min_length: usize) -> bool {
    if piece.chars().count() < min_length.max(1) {
        return false;
    }
    if !piece.chars().all(|c| is_apostrophe(c) || is_latin_letter(c)) {
        return false;
    }
    // ALL-CAPS: an acronym.
    piece.chars().any(char::is_lowercase)
}

fn for_each_piece(text: &str, f: &mut dyn FnMut(Range<usize>)) {
    pieces_into(text, &mut Vec::new(), f);
}

/// [`for_each_piece`] with the chunk's chars decoded into `chars`, a buffer reused across chunks
/// (and across calls: [`Checker::check`]), so splitting allocates nothing per word.
fn pieces_into(text: &str, chars: &mut Vec<(usize, char)>, f: &mut dyn FnMut(Range<usize>)) {
    let mut chunk_start = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = chunk_start.take() {
                chunk(text, s, i, chars, f);
            }
        } else if chunk_start.is_none() {
            chunk_start = Some(i);
        }
    }
    if let Some(s) = chunk_start {
        chunk(text, s, text.len(), chars, f);
    }
}

/// A URL, e-mail address or file path (the chunk with surrounding punctuation trimmed).
fn is_opaque(chunk: &str) -> bool {
    let t = chunk.trim_start_matches(['(', '[', '{', '<', '"', '\'', '`']);
    let t = t.trim_end_matches([')', ']', '}', '>', '"', '\'', '`', '.', ',', ';', ':', '!', '?']);
    let lower = |p: &str| t.len() >= p.len() && t.is_char_boundary(p.len()) && t[..p.len()].eq_ignore_ascii_case(p);
    if t.contains("://") || lower("www.") || lower("mailto:") {
        return true;
    }
    if let Some(at) = t.find('@') {
        let (user, host) = (&t[..at], &t[at + 1..]);
        if !user.is_empty() && host.contains('.') && !host.ends_with('.') && !host.starts_with('.') {
            return true;
        }
    }
    let b = t.as_bytes();
    let path_char = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'~' | b'-');
    if b.len() > 1 && b[0] == b'/' && path_char(b[1]) {
        return true;
    }
    if t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with("~/")
        || t.starts_with(".\\")
        || t.starts_with("..\\")
    {
        return true;
    }
    if b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        return true;
    }
    t.matches('/').count() >= 2 || t.matches('\\').count() >= 2
}

fn chunk(text: &str, s: usize, e: usize, chars: &mut Vec<(usize, char)>, f: &mut dyn FnMut(Range<usize>)) {
    if is_opaque(&text[s..e]) {
        return;
    }
    chars.clear();
    chars.extend(text[s..e].char_indices().map(|(i, c)| (s + i, c)));
    let chars = &chars[..];
    let mut i = 0;
    while i < chars.len() {
        let token_char = |k: usize| {
            let c = chars[k].1;
            c.is_alphanumeric()
                || c == '_'
                || (is_apostrophe(c)
                    && k > 0
                    && chars[k - 1].1.is_alphabetic()
                    && chars.get(k + 1).is_some_and(|n| n.1.is_alphabetic()))
        };
        if !token_char(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && token_char(j) {
            j += 1;
        }
        let end = chars.get(j).map_or(e, |c| c.0);
        token(text, &chars[i..j], chars[i].0, end, f);
        i = j;
    }
}

fn hex_looking(t: &str) -> bool {
    if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    t.len() >= 6 && t.chars().all(|c| c.is_ascii_hexdigit()) && t.chars().any(|c| c.is_ascii_digit())
}

fn token(text: &str, chars: &[(usize, char)], start: usize, end: usize, f: &mut dyn FnMut(Range<usize>)) {
    if hex_looking(&text[start..end]) {
        return;
    }
    let mut piece_start: Option<usize> = None;
    let emit = |from: usize, to: usize, f: &mut dyn FnMut(Range<usize>)| f(from..to);
    for (k, &(pos, c)) in chars.iter().enumerate() {
        if c == '_' || c.is_numeric() {
            if let Some(s) = piece_start.take() {
                emit(s, pos, f);
            }
            continue;
        }
        if let Some(s) = piece_start {
            let prev = chars[k - 1].1;
            let next = chars.get(k + 1).map(|n| n.1);
            let boundary = c.is_uppercase()
                && (prev.is_lowercase() || (prev.is_uppercase() && next.is_some_and(|n| n.is_lowercase())));
            if boundary {
                emit(s, pos, f);
                piece_start = Some(pos);
            }
        } else {
            piece_start = Some(pos);
        }
    }
    if let Some(s) = piece_start {
        emit(s, end, f);
    }
}

/// Whether `name` can name a dictionary: a Hunspell language name such as `en_AU` or `de_DE`
/// (it becomes a directory and file name, so letters, digits and `_` / `-` only).
pub fn valid_language(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A Hunspell dictionary: the installed one for the configured language, or the bundled `en_AU`.
pub struct Dictionary {
    inner: spellbook::Dictionary,
    /// The configured language when its files could not be used (so this is `en_AU` instead).
    missing: Option<String>,
}

impl Dictionary {
    /// Builds a dictionary from the text of an `.aff` and a `.dic` file.
    pub fn from_hunspell(aff: &str, dic: &str) -> Result<Dictionary, String> {
        match spellbook::Dictionary::new(aff, dic) {
            Ok(inner) => Ok(Dictionary { inner, missing: None }),
            Err(e) => Err(e.to_string()),
        }
    }

    /// A dictionary of exactly these words (one per line, no affixes). Tests.
    #[cfg(test)]
    pub fn from_list(list: &str) -> Dictionary {
        let words: Vec<&str> = list_words(list).collect();
        let dic = format!("{}\n{}\n", words.len(), words.join("\n"));
        Dictionary::from_hunspell("SET UTF-8\n", &dic).unwrap()
    }

    /// The compiled-in `en_AU`, built on first use and shared.
    /// Blocking: call from spawn_blocking (parses ~550 KB).
    pub fn bundled() -> Arc<Dictionary> {
        static DICT: OnceLock<Arc<Dictionary>> = OnceLock::new();
        DICT.get_or_init(|| Arc::new(Dictionary::from_hunspell(EN_AU_AFF, EN_AU_DIC).expect("bundled en_AU parses")))
            .clone()
    }

    /// The dictionary for `language`: `<dir>/<language>/<language>.aff` and `.dic` (`dir` =
    /// `<config dir>/dictionaries`, filled by `scripts/fetch-dictionary`), else the bundled
    /// `en_AU` for the default language. Any other language whose files are missing or don't
    /// parse is logged and answered by `en_AU` with [`Dictionary::missing_language`] set.
    /// Blocking: call from spawn_blocking (reads and parses the files).
    pub fn load(dir: &Path, language: &str) -> Arc<Dictionary> {
        let base = dir.join(language);
        let read = |ext: &str| std::fs::read_to_string(base.join(format!("{language}.{ext}")));
        let installed = valid_language(language).then(|| (read("aff"), read("dic")));
        match installed {
            Some((Ok(aff), Ok(dic))) => match Dictionary::from_hunspell(&aff, &dic) {
                Ok(d) => return Arc::new(d),
                Err(e) => tracing::warn!(target: "jed::spellcheck", "dictionary {} is unusable: {e}", base.display()),
            },
            Some((Err(e), _) | (_, Err(e))) if language != DEFAULT_LANGUAGE => {
                tracing::info!(target: "jed::spellcheck", "no dictionary in {} ({e})", base.display());
            }
            _ => {}
        }
        let bundled = Dictionary::bundled();
        if language == DEFAULT_LANGUAGE {
            return bundled;
        }
        // The shared bundled one can't carry the name, so this is a handle with its own flag.
        Arc::new(Dictionary {
            inner: bundled.inner.clone(),
            missing: Some(language.to_string()),
        })
    }

    /// The configured language whose dictionary could not be loaded (`en_AU` is used instead).
    pub fn missing_language(&self) -> Option<&str> {
        self.missing.as_deref()
    }

    /// Whether the word (any case, `'` or `’`) is spelled correctly.
    pub fn contains(&self, word: &str) -> bool {
        if word.contains('\u{2019}') {
            return self.inner.check(&word.replace('\u{2019}', "'"));
        }
        self.inner.check(word)
    }

    /// Spellbook's suggestions for `word`.
    fn suggest(&self, word: &str, out: &mut Vec<String>) {
        self.inner.suggest(&word.replace('\u{2019}', "'"), out);
    }
}

fn list_words(list: &str) -> impl Iterator<Item = &str> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

fn lower(word: &str) -> String {
    let mut s = String::with_capacity(word.len());
    lower_into(word, &mut s);
    s
}

fn lower_into(word: &str, buf: &mut String) {
    buf.clear();
    for c in word.chars() {
        if c == '\u{2019}' {
            buf.push('\'');
        } else {
            buf.extend(c.to_lowercase());
        }
    }
}

/// Checks text against a dictionary plus the programming terms of one language.
pub struct Checker {
    dict: Arc<Dictionary>,
    terms: HashSet<&'static str>,
    min_length: usize,
}

impl Checker {
    /// `min_length`: pieces shorter than this many chars are not checked (default 3).
    pub fn new(dict: Arc<Dictionary>, lang: Language, min_length: usize) -> Checker {
        let terms = list_words(CODE)
            .chain(language_terms(lang).iter().flat_map(|l| list_words(l)))
            .collect();
        Checker {
            dict,
            terms,
            min_length,
        }
    }

    /// Whether the word is accepted: the programming terms, `extra` (the user's words,
    /// lower-case) or a possessive of one of those, a contraction, or the dictionary. `buf` is
    /// scratch.
    fn accepts(&self, piece: &str, extra: &[&HashSet<String>], buf: &mut String) -> bool {
        lower_into(piece, buf);
        let known = |w: &str| self.terms.contains(w) || extra.iter().any(|e| e.contains(w));
        if known(buf) || CONTRACTIONS.contains(&buf.as_str()) || buf.strip_suffix("'s").is_some_and(known) {
            return true;
        }
        self.dict.contains(piece)
    }

    /// Whether `word` (a piece, any case) is accepted.
    pub fn is_known(&self, word: &str, extra: &[&HashSet<String>]) -> bool {
        self.accepts(word, extra, &mut String::new())
    }

    /// The misspelled pieces of `text` inside `spans` (byte ranges into `text`, from the parse
    /// tree; out-of-range or mid-char spans are skipped), in order. `extra`: sets of accepted
    /// words, lower-case.
    pub fn check(&self, text: &str, spans: &[Range<ByteIdx>], extra: &[&HashSet<String>]) -> Vec<Misspelling> {
        let mut out = Vec::new();
        let mut buf = String::new();
        let mut chars = Vec::new();
        for span in spans {
            let Some(slice) = text.get(span.start.0..span.end.0) else {
                continue;
            };
            pieces_into(slice, &mut chars, &mut |r| {
                let piece = &slice[r.clone()];
                if checkable(piece, self.min_length) && !self.accepts(piece, extra, &mut buf) {
                    out.push(Misspelling {
                        range: ByteIdx(span.start.0 + r.start)..ByteIdx(span.start.0 + r.end),
                        word: piece.to_string(),
                    });
                }
            });
        }
        out
    }

    /// Up to [`MAX_SUGGESTIONS`] corrections for one misspelled word (a piece: pass
    /// [`Misspelling::word`]), best first. Ask for one word on demand, never per mark.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        if !word.is_empty() {
            self.dict.suggest(word, &mut out);
        }
        out.retain(|w| !w.eq_ignore_ascii_case(word));
        out.dedup();
        out.truncate(MAX_SUGGESTIONS);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(text: &str) -> Vec<&str> {
        split_pieces(text).iter().map(|p| p.text(text)).collect()
    }

    fn checker() -> Checker {
        Checker::new(Dictionary::bundled(), Language::Rust, 3)
    }

    fn misses(text: &str) -> Vec<String> {
        let span = ByteIdx(0)..ByteIdx(text.len());
        checker()
            .check(text, &[span], &[])
            .into_iter()
            .map(|m| m.word)
            .collect()
    }

    #[test]
    fn spell_check_split_cases() {
        assert_eq!(pieces("camelCase"), ["camel", "Case"]);
        assert_eq!(pieces("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(pieces("parseXMLFile"), ["parse", "XML", "File"]);
        assert_eq!(pieces("utf8Decode"), ["utf", "Decode"]);
        assert_eq!(pieces("snake_case"), ["snake", "case"]);
        assert_eq!(pieces("SCREAMING_SNAKE"), ["SCREAMING", "SNAKE"]);
        assert_eq!(pieces("kebab-case"), ["kebab", "case"]);
        assert_eq!(pieces("ID3Tag"), ["ID", "Tag"]);
        assert_eq!(pieces("a_b"), ["a", "b"]);
        assert_eq!(pieces("__init__"), ["init"]);
        assert_eq!(pieces("_leading and trailing_"), ["leading", "and", "trailing"]);
        assert_eq!(pieces("--flag-name--"), ["flag", "name"]);
        assert_eq!(pieces("x1y2"), ["x", "y"]);
        assert_eq!(pieces("12345"), Vec::<&str>::new());
    }

    #[test]
    fn spell_check_split_apostrophes_and_unicode() {
        assert_eq!(pieces("don't stop"), ["don't", "stop"]);
        assert_eq!(pieces("the dogs' bowl"), ["the", "dogs", "bowl"]);
        assert_eq!(pieces("'quoted'"), ["quoted"]);
        assert_eq!(pieces("it\u{2019}s"), ["it\u{2019}s"]);
        assert_eq!(pieces("naïve"), ["naïve"]);
        assert_eq!(pieces("naïveCafé"), ["naïve", "Café"]);
        // Non-Latin scripts split as pieces but are never checked.
        let t = "日本語 text";
        assert_eq!(pieces(t), ["日本語", "text"]);
        let w = split_words(t, 3);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].text(t), "text");
        assert_eq!(w[0].range, 10..14, "byte offsets after 3-byte chars");
        assert_eq!(misses("日本語"), Vec::<String>::new());
        let t = "xé recieve";
        assert_eq!(split_pieces(t)[1].range, 4..11, "ranges are bytes");
    }

    #[test]
    fn spell_check_split_words_skips() {
        let words = |t: &str, n| {
            split_words(t, n)
                .iter()
                .map(|p| p.text(t).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(words("x id io word", 3), ["word"]);
        assert_eq!(words("x id io", 2), ["id", "io"]);
        assert_eq!(words("HTTP XML word", 3), ["word"]);
        assert_eq!(words("parseXMLFile", 3), ["parse", "File"]);
        assert_eq!(words("0xDEADbeef 3fa9b2c1 word", 3), ["word"]);
        assert_eq!(words("see https://example.com/recieve now", 3), ["see", "now"]);
        assert_eq!(words("(http://example.com/a_b).", 3), Vec::<String>::new());
        assert_eq!(words("mail bob@example.com please", 3), ["mail", "please"]);
        assert_eq!(words("at /usr/lib/recieve.so and ./src/main.rs", 3), ["and"]);
        assert_eq!(words("C:\\Users\\recieve", 3), Vec::<String>::new());
        assert_eq!(words("and/or", 3), ["and"]);
        assert_eq!(words("a1b2c3 12345", 3), Vec::<String>::new());
        assert_eq!(
            words("recieve2 word3 utf8Decode", 3),
            ["recieve", "word", "utf", "Decode"]
        );
        // URL forms with no `//` (so no path rule applies) are skipped; the same word outside is not.
        assert_eq!(words("www.recieve.org", 3), Vec::<String>::new());
        assert_eq!(words("WWW.recieve.org (mailto:recieve)", 3), Vec::<String>::new());
        assert_eq!(words("https://recieve.example/recieve", 3), Vec::<String>::new());
        assert_eq!(words("recieve.org", 3), ["recieve", "org"]);
        assert_eq!(words("mailto recieve", 3), ["mailto", "recieve"]);
    }

    #[test]
    fn spell_check_check_marks_only_the_piece() {
        let text = "let recieveMessage = 1; // max_buffr_size";
        let span = ByteIdx(0)..ByteIdx(text.len());
        let got = checker().check(text, &[span], &[]);
        let words: Vec<_> = got.iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, ["recieve", "buffr"]);
        assert_eq!(&text[got[0].range.start.0..got[0].range.end.0], "recieve");
        assert_eq!(&text[got[1].range.start.0..got[1].range.end.0], "buffr");
    }

    #[test]
    fn spell_check_check_respects_spans() {
        let text = "recieve ok recieve";
        let spans = [
            ByteIdx(11)..ByteIdx(18),
            ByteIdx(2)..ByteIdx(100),
            ByteIdx(1)..ByteIdx(0),
        ];
        let got = checker().check(text, &spans, &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].range, ByteIdx(11)..ByteIdx(18));
    }

    #[test]
    fn spell_check_dictionary_accepts_and_misses() {
        assert!(misses("parsing tokens Parsing TOKENS").is_empty());
        assert_eq!(misses("unparsed"), ["unparsed"]);
        assert!(misses("colour centre organise").is_empty());
        assert_eq!(misses("color"), ["color"]);
        assert!(misses("don't it's can't").is_empty());
        assert!(misses("struct enum impl stdin mutex regex async tokio").is_empty());
        assert!(Dictionary::bundled().contains("Parsing"));
    }

    #[test]
    fn spell_check_possessive_and_language_terms() {
        assert!(misses("server's user's").is_empty());
        assert_eq!(misses("zzyx's"), ["zzyx's"]);
        // `deref` is a Rust term, not a C one.
        assert!(misses("deref").is_empty());
        let c = Checker::new(Dictionary::bundled(), Language::C, 3);
        let t = "deref sizeof";
        let got = c.check(t, &[ByteIdx(0)..ByteIdx(t.len())], &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].word, "deref");
    }

    #[test]
    fn spell_check_extra_words_and_min_length() {
        let extra: HashSet<String> = ["recieve".to_string()].into();
        let t = "Recieve RECIEVE recieve xq";
        let c = checker();
        assert!(c.check(t, &[ByteIdx(0)..ByteIdx(t.len())], &[&extra]).is_empty());
        let one = Checker::new(Dictionary::bundled(), Language::Rust, 1);
        assert_eq!(one.check(t, &[ByteIdx(0)..ByteIdx(t.len())], &[&extra]).len(), 1);
    }

    #[test]
    fn spell_check_suggestions() {
        let c = checker();
        assert_eq!(c.suggest("recieve")[0], "receive");
        assert_eq!(c.suggest("Recieve")[0], "Receive");
        assert_eq!(c.suggest("RECIEVE")[0], "RECEIVE");
        // Inside an identifier only the piece is replaced.
        let t = "recieveMessage";
        let m = &c.check(t, &[ByteIdx(0)..ByteIdx(t.len())], &[])[0];
        let fixed = format!(
            "{}{}{}",
            &t[..m.range.start.0],
            c.suggest(&m.word)[0],
            &t[m.range.end.0..]
        );
        assert_eq!(fixed, "receiveMessage");
        assert!(c.suggest("color").contains(&"colour".to_string()));
    }

    #[test]
    fn spell_check_suggestions_limit() {
        let c = checker();
        let s = c.suggest("tet");
        assert!(!s.is_empty() && s.len() <= MAX_SUGGESTIONS);
        assert!(c.suggest("qqqqqqqqqq").is_empty());
        assert!(c.suggest("").is_empty());
        assert!(!c.suggest("tet").contains(&"tet".to_string()));
    }

    #[test]
    fn spell_check_check_byte_ranges_after_multibyte() {
        // Span starts mid-text after multibyte chars; ranges are into the whole text.
        let text = "日本 naïve é recieve and recieve2 x";
        let start = text.find("é ").unwrap();
        let span = ByteIdx(start)..ByteIdx(text.len());
        let got = checker().check(text, &[span], &[]);
        let words: Vec<_> = got.iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, ["recieve", "recieve"]);
        for m in &got {
            assert_eq!(&text[m.range.start.0..m.range.end.0], "recieve");
        }
        assert_eq!(got[0].range.start.0, text.find("recieve").unwrap());
        assert_eq!(got[1].range.start.0, text.rfind("recieve").unwrap());
        // A span that cuts a multibyte char is skipped, not a panic.
        let cut = ByteIdx(1)..ByteIdx(text.len());
        assert!(checker().check(text, &[cut], &[]).is_empty());
    }

    #[test]
    fn spell_check_check_skips_opaque_and_acronyms() {
        let t = "see https://x.org/recieve or bob@recieve.com or /tmp/recieve and HTTP 0xBADf00d recieve";
        let got = misses(t);
        assert_eq!(got, ["recieve"]);
        assert!(misses("www.recieve.org mailto:recieve https://recieve.example/recieve").is_empty());
        assert_eq!(misses("recieve.org"), ["recieve"]);
        assert!(misses("it\u{2019}s server\u{2019}s").is_empty());
    }

    #[test]
    fn spell_check_language_terms_per_language() {
        let known = |lang, w: &str| Checker::new(Dictionary::bundled(), lang, 3).is_known(w, &[]);
        assert!(known(Language::Rust, "deref") && !known(Language::Python, "deref"));
        assert!(known(Language::Python, "elif") && !known(Language::Rust, "elif"));
        assert!(known(Language::C, "malloc") && known(Language::Cpp, "malloc") && !known(Language::Rust, "malloc"));
        assert!(known(Language::Cpp, "constexpr") && !known(Language::C, "constexpr"));
        assert!(!known(Language::C, "deref") && !known(Language::C, "elif") && !known(Language::Python, "malloc"));
        assert!(!known(Language::Cpp, "elif") && !known(Language::Cpp, "deref"));
        assert!(language_terms(Language::PlainText).is_empty());
        assert!(!known(Language::PlainText, "deref"));
        // Programming terms common to every language.
        assert!(known(Language::PlainText, "struct"));
    }

    #[test]
    fn spell_check_word_list_dictionary() {
        let list = "# c\nzzyx\n\nfoo\n";
        let c = Checker::new(Arc::new(Dictionary::from_list(list)), Language::PlainText, 3);
        assert!(c.is_known("zzyx", &[]) && c.is_known("foo", &[]) && !c.is_known("bar", &[]));
    }

    /// A language dir under `dir` holding a tiny dictionary.
    fn write_language(dir: &Path, lang: &str, words: &[&str]) {
        let d = dir.join(lang);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{lang}.aff")), "SET UTF-8\n").unwrap();
        std::fs::write(
            d.join(format!("{lang}.dic")),
            format!("{}\n{}\n", words.len(), words.join("\n")),
        )
        .unwrap();
    }

    #[test]
    fn spell_check_load_picks_the_configured_language() {
        let dir = tempfile::tempdir().unwrap();
        // The default language needs no files; an installed one wins over the bundled.
        let d = Dictionary::load(dir.path(), "en_AU");
        assert!(d.missing_language().is_none() && d.contains("colour"));
        write_language(dir.path(), "de_DE", &["Haus", "Maus"]);
        let de = Dictionary::load(dir.path(), "de_DE");
        assert!(de.missing_language().is_none());
        assert!(de.contains("Haus") && !de.contains("colour"));
        write_language(dir.path(), "en_AU", &["zzyx"]);
        assert!(
            Dictionary::load(dir.path(), "en_AU").contains("zzyx"),
            "installed en_AU overrides"
        );
    }

    #[test]
    fn spell_check_load_falls_back_to_en_au() {
        let dir = tempfile::tempdir().unwrap();
        let d = Dictionary::load(dir.path(), "fr_FR");
        assert_eq!(d.missing_language(), Some("fr_FR"));
        assert!(d.contains("colour"));
        // Unparseable files and unsafe names fall back too.
        std::fs::create_dir_all(dir.path().join("xx")).unwrap();
        std::fs::write(dir.path().join("xx/xx.aff"), "FLAG nonsense\n").unwrap();
        std::fs::write(dir.path().join("xx/xx.dic"), "x\n").unwrap();
        assert_eq!(Dictionary::load(dir.path(), "xx").missing_language(), Some("xx"));
        assert_eq!(Dictionary::load(dir.path(), "../xx").missing_language(), Some("../xx"));
        assert!(
            !valid_language("../xx") && !valid_language("") && valid_language("en_AU") && valid_language("sr-Latn")
        );
    }

    #[test]
    fn spell_check_real_dictionary() {
        let c = checker();
        for w in [
            "colour",
            "centre",
            "organise",
            "parsing",
            "tokens",
            "receive",
            "don't",
            "Australia",
        ] {
            assert!(c.is_known(w, &[]), "{w} should be known");
        }
        for w in ["color", "organize", "recieve", "unparsed"] {
            assert!(!c.is_known(w, &[]), "{w} should be unknown");
        }
        assert!(c.suggest("recieve").contains(&"receive".to_string()));
    }
}
