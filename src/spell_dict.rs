//! The spell checker's user dictionaries: plain-text word lists the user accepts, in two files.
//!
//! * the user's `<config dir>/dictionary.txt`, for every workspace ([`user_path`]);
//! * the workspace's `<root>/.jed/dictionary.txt`, for a project's own terms ([`workspace_path`]).
//!
//! Decision (the task floated a `[spell_check] words` list in `.workspace` too): only the
//! `.jed/dictionary.txt` file. A word list is inert, so the trusted-workspace question doesn't
//! arise either way, but a file is what "Add Word to Workspace Dictionary" appends to atomically
//! (rewriting `.workspace` would mean editing a TOML file the user keeps by hand), it reloads
//! like the user file, and it can be committed so a team shares it: writing it also makes
//! `.jed/.gitignore` (which `history` creates as `*`) let `dictionary.txt` through. The
//! `spell_check_ignore_words` setting stays for a few words in the settings file itself.
//!
//! Format: one word per line, blank lines skipped, `#` starts a comment (a whole line, or after the
//! word following whitespace). A word is letters and apostrophes, at most [`MAX_WORD`] chars; any
//! other line is skipped, counted in [`UserWords::errors`] and logged once (target
//! `jed::spellcheck`).
//!
//! **Case rule: matching is case-insensitive, whatever the entry's case.** `Colour` accepts
//! `colour`, `COLOUR` and `Colour` (Code Spell Checker only does that for all-lower-case entries;
//! here one rule keeps the checker a lookup in one lower-case set, [`UserWords::extra_set`], which
//! is what `Checker::check` takes).
//!
//! Pure functions; the ones touching the disk are documented `Blocking: call from spawn_blocking`.
//! Watching and reloading are the editor's (`-c`): [`is_dictionary_path`] says whether a changed
//! path is one of the two files.

#![allow(dead_code)]

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The file's name in both places.
pub const FILE_NAME: &str = "dictionary.txt";
/// The workspace's directory for jed's own files (`history` keeps its snapshots there too).
const JED_DIR: &str = ".jed";
/// The longest word accepted (chars): longer lines are not words.
pub const MAX_WORD: usize = 64;

const HEADER: &str = "\
# jed spell-check dictionary: one word per line, case-insensitive.
# Lines starting with # are comments. Edits apply as soon as the file is saved.
";

/// Which of the two files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Which {
    User,
    Workspace,
}

/// `<config dir>/dictionary.txt`.
pub fn user_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

/// `<root>/.jed/dictionary.txt`.
pub fn workspace_path(root: &Path) -> PathBuf {
    root.join(JED_DIR).join(FILE_NAME)
}

/// Which dictionary file `path` is, if either (the workspace's only when there is a root).
pub fn is_dictionary_path(path: &Path, config_dir: &Path, root: Option<&Path>) -> Option<Which> {
    if path == user_path(config_dir) {
        return Some(Which::User);
    }
    (path == workspace_path(root?)).then_some(Which::Workspace)
}

/// The words of one dictionary (or several merged): lower-case, matched case-insensitively.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UserWords {
    words: HashSet<String>,
    /// Lines skipped by [`parse`] as not a word.
    pub errors: usize,
}

impl UserWords {
    /// The words as given (the settings' `spell_check_ignore_words`); a bad one counts as an error.
    pub fn from_words<I, S>(words: I) -> UserWords
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = UserWords::default();
        for w in words {
            match normalize(w.as_ref().trim()) {
                Some(w) => {
                    out.words.insert(w);
                }
                None => out.errors += 1,
            }
        }
        out
    }

    /// Whether `word` (any case) is in the list.
    pub fn accepts(&self, word: &str) -> bool {
        self.words.contains(&word.to_lowercase())
    }

    /// The lower-case words: `Checker::check`'s `extra`.
    pub fn extra_set(&self) -> &HashSet<String> {
        &self.words
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// This list plus `other`'s words (user + workspace + settings).
    pub fn merged(&self, other: &UserWords) -> UserWords {
        UserWords {
            words: self.words.union(&other.words).cloned().collect(),
            errors: self.errors + other.errors,
        }
    }
}

/// `word` lower-cased if it is a valid entry: letters and apostrophes (not first or last), at most
/// [`MAX_WORD`] chars.
fn normalize(word: &str) -> Option<String> {
    let n = word.chars().count();
    let ok = n > 0
        && n <= MAX_WORD
        && word.chars().all(|c| c.is_alphabetic() || c == '\'')
        && !word.starts_with('\'')
        && !word.ends_with('\'');
    ok.then(|| word.to_lowercase())
}

/// The word on a line (comment stripped, trimmed): `None` for a blank or comment-only line,
/// `Some(Err(()))` for text that isn't a word.
fn line_word(line: &str) -> Option<Result<&str, ()>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let word = match line.find(|c: char| c.is_whitespace()) {
        // `word # comment`: whitespace then `#`; any other text after the word is a bad line.
        Some(at) if line[at..].trim_start().starts_with('#') => &line[..at],
        _ => line,
    };
    Some(if normalize(word).is_some() { Ok(word) } else { Err(()) })
}

/// Parses a dictionary file's text. Bad lines are skipped and counted (one summary log line).
pub fn parse(text: &str) -> UserWords {
    let mut out = UserWords::default();
    for line in text.lines() {
        match line_word(line) {
            None => {}
            Some(Ok(w)) => {
                out.words.extend(normalize(w));
            }
            Some(Err(())) => out.errors += 1,
        }
    }
    if out.errors > 0 {
        tracing::warn!(
            target: "jed::spellcheck",
            skipped = out.errors,
            words = out.words.len(),
            "dictionary lines skipped (not a word)"
        );
    }
    out
}

/// Blocking: call from spawn_blocking. The words in the file at `path`; a missing file is an empty
/// list, silently; an unreadable one is empty and logged.
pub fn load(path: &Path) -> UserWords {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UserWords::default(),
        Err(e) => {
            tracing::warn!(target: "jed::spellcheck", path = %path.display(), error = %e, "can't read dictionary");
            UserWords::default()
        }
    }
}

/// Blocking: call from spawn_blocking. Adds `word` to the file at `path` (created, with its
/// directory, if missing). Returns `false` and writes nothing if the word is already there (any
/// case); an invalid word is an `InvalidInput` error.
///
/// The file is rewritten, atomically (a temp file beside it, synced, renamed over it): its leading
/// comment block, then any other comment or bad lines as they were, then the entries sorted
/// (case-insensitively) and de-duplicated, each as its line was written.
pub fn add_word(path: &Path, word: &str) -> std::io::Result<bool> {
    let word = word.trim();
    let Some(lower) = normalize(word) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("\"{word}\" is not a word"),
        ));
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut head = Vec::new();
    let mut other = Vec::new();
    let mut entries: Vec<(String, &str)> = Vec::new();
    for line in text.lines() {
        match line_word(line) {
            None if entries.is_empty() && other.is_empty() => head.push(line),
            None | Some(Err(())) => other.push(line),
            Some(Ok(w)) => {
                let key = w.to_lowercase();
                if key == lower {
                    return Ok(false);
                }
                if !entries.iter().any(|(k, _)| *k == key) {
                    entries.push((key, line.trim()));
                }
            }
        }
    }
    if text.is_empty() {
        head.extend(HEADER.lines());
    }
    entries.push((lower, word));
    entries.sort();
    let mut out = String::new();
    for line in head.iter().chain(&other) {
        out.push_str(line);
        out.push('\n');
    }
    for (_, line) in &entries {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    keep_workspace_file_tracked(path);
    write_atomic(path, out.as_bytes())?;
    Ok(true)
}

/// Blocking: call from spawn_blocking. Creates the file at `path` with a header comment unless it
/// exists (never overwrites, even if it appears meanwhile): "Spelling: Open User Dictionary".
pub fn ensure_file(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    keep_workspace_file_tracked(path);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => f.write_all(HEADER.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// `.jed/.gitignore` is `*` (the history snapshots): a workspace dictionary should be committable,
/// so it is let through. Best effort: a failure only means the file stays ignored.
fn keep_workspace_file_tracked(path: &Path) {
    let Some(dir) = path.parent().filter(|d| d.file_name().is_some_and(|n| n == JED_DIR)) else {
        return;
    };
    let ignore = dir.join(".gitignore");
    let rule = format!("!{FILE_NAME}\n");
    let result = match std::fs::read_to_string(&ignore) {
        Ok(text) if text.lines().any(|l| l.trim() == rule.trim()) => return,
        Ok(text) if text.lines().any(|l| l.trim() == "*") => {
            let sep = if text.ends_with('\n') { "" } else { "\n" };
            std::fs::write(&ignore, format!("{text}{sep}{rule}"))
        }
        Ok(_) => return,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::write(&ignore, format!("*\n{rule}")),
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        tracing::debug!(target: "jed::spellcheck", error = %e, "can't update .jed/.gitignore");
    }
}

/// A temp file beside `path`, synced, then renamed over it; removed again on failure.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn spell_check_dict_parse_comments_blanks_and_case() {
        let u = parse("# header\n\nTokio\ncolour  # trailing\n\tZed\t#x\nDon't\n   \n# end\n");
        assert_eq!(u.errors, 0);
        assert_eq!(u.len(), 4);
        for w in ["tokio", "Tokio", "TOKIO", "colour", "Colour", "zed", "don't"] {
            assert!(u.accepts(w), "{w}");
        }
        assert!(!u.accepts("toki"));
        assert!(u.extra_set().iter().all(|w| *w == w.to_lowercase()));
    }

    #[test]
    fn spell_check_dict_bad_lines_are_skipped_and_counted() {
        let long = "a".repeat(MAX_WORD + 1);
        let text = format!("two words\nfoo#bar\nok\nfoo2\n'quote\n{long}\n日本語\nfine # fine\n");
        let u = parse(&text);
        // `日本語` is alphabetic, so a valid entry; the checker never asks about it.
        assert_eq!(u.errors, 5);
        assert!(u.accepts("ok") && u.accepts("fine") && u.accepts("日本語"));
        assert_eq!(u.len(), 3);
    }

    #[test]
    fn spell_check_dict_from_words_and_merge() {
        let a = UserWords::from_words(["Foo", " bar ", "bad word"]);
        assert_eq!((a.len(), a.errors), (2, 1));
        let b = parse("baz\nfoo\n");
        let m = a.merged(&b);
        assert_eq!(m.len(), 3);
        assert!(m.accepts("BAZ") && m.accepts("bar"));
    }

    #[test]
    fn spell_check_dict_load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let u = load(&dir.path().join("nope").join(FILE_NAME));
        assert!(u.is_empty() && u.errors == 0);
        std::fs::write(dir.path().join(FILE_NAME), "alpha\n").unwrap();
        assert!(load(&dir.path().join(FILE_NAME)).accepts("alpha"));
    }

    #[test]
    fn spell_check_dict_add_word_writes_once_sorted_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join(FILE_NAME);
        assert!(add_word(&path, "zebra").unwrap());
        assert!(add_word(&path, "Apple").unwrap());
        assert!(add_word(&path, "mango").unwrap());
        assert!(!add_word(&path, "APPLE").unwrap(), "already there, any case");
        assert!(!add_word(&path, "zebra").unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let entries: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(entries, ["Apple", "mango", "zebra"]);
        assert!(text.starts_with("# jed spell-check dictionary"), "{text}");
        let u = load(&path);
        assert!(u.accepts("apple") && u.accepts("ZEBRA") && u.errors == 0);
        assert_eq!(names(path.parent().unwrap()), [FILE_NAME], "no temp file left behind");
    }

    #[test]
    fn spell_check_dict_add_word_keeps_comments_and_removes_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "# mine\n\nzeta # last\nalpha\nALPHA\nnot a word\n# note\n").unwrap();
        assert!(add_word(&path, "mid").unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# mine\n\nnot a word\n# note\nalpha\nmid\nzeta # last\n"
        );
        assert!(add_word(&path, "no space").is_err());
        assert!(add_word(&path, "").is_err());
        assert!(add_word(&path, "x1").is_err());
        assert_eq!(names(dir.path()), [FILE_NAME]);
    }

    #[test]
    fn spell_check_dict_workspace_word_does_not_leak() {
        let cfg = tempfile::tempdir().unwrap();
        let ws_a = tempfile::tempdir().unwrap();
        let ws_b = tempfile::tempdir().unwrap();
        assert!(add_word(&workspace_path(ws_a.path()), "frobnicate").unwrap());
        assert!(add_word(&user_path(cfg.path()), "tokio").unwrap());
        assert!(load(&workspace_path(ws_a.path())).accepts("frobnicate"));
        assert!(!load(&workspace_path(ws_b.path())).accepts("frobnicate"));
        assert!(!load(&user_path(cfg.path())).accepts("frobnicate"));
        assert!(!load(&workspace_path(ws_a.path())).accepts("tokio"));
    }

    #[test]
    fn spell_check_dict_workspace_file_is_let_through_the_jed_gitignore() {
        let ws = tempfile::tempdir().unwrap();
        let jed = ws.path().join(".jed");
        std::fs::create_dir_all(&jed).unwrap();
        std::fs::write(jed.join(".gitignore"), "*\n").unwrap();
        add_word(&workspace_path(ws.path()), "foo").unwrap();
        add_word(&workspace_path(ws.path()), "bar").unwrap();
        assert_eq!(
            std::fs::read_to_string(jed.join(".gitignore")).unwrap(),
            "*\n!dictionary.txt\n"
        );
        let fresh = tempfile::tempdir().unwrap();
        ensure_file(&workspace_path(fresh.path())).unwrap();
        let ignore = std::fs::read_to_string(fresh.path().join(".jed/.gitignore")).unwrap();
        assert_eq!(ignore, "*\n!dictionary.txt\n");
        // The user's file gets no .gitignore.
        let cfg = tempfile::tempdir().unwrap();
        add_word(&user_path(cfg.path()), "foo").unwrap();
        assert_eq!(names(cfg.path()), [FILE_NAME]);
    }

    #[test]
    fn spell_check_dict_ensure_file_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        ensure_file(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('#') && load(&path).is_empty());
        std::fs::write(&path, "mine\n").unwrap();
        ensure_file(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
        assert_eq!(names(path.parent().unwrap()), [FILE_NAME]);
    }

    #[test]
    fn spell_check_dict_paths() {
        let cfg = Path::new("/home/u/.config/jed");
        let root = Path::new("/work/proj");
        assert_eq!(is_dictionary_path(&user_path(cfg), cfg, Some(root)), Some(Which::User));
        assert_eq!(
            is_dictionary_path(&workspace_path(root), cfg, Some(root)),
            Some(Which::Workspace)
        );
        assert_eq!(is_dictionary_path(&workspace_path(root), cfg, None), None);
        assert_eq!(is_dictionary_path(&root.join("dictionary.txt"), cfg, Some(root)), None);
        assert_eq!(is_dictionary_path(&cfg.join("settings.toml"), cfg, Some(root)), None);
    }
}
