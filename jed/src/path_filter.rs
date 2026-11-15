//! The Search view's "files to include" / "files to exclude" fields as one matcher over
//! root-relative paths (no drawing, no I/O). Workspace search uses the same `PathFilter` to prune
//! the walk, to pick the files it reaches and to pick dirty buffers, so they can't disagree.
//!
//! Each field is a comma-separated list of globs (VS Code's spelling; a comma inside `{…}` or
//! escaped as `\,` doesn't split). A pattern:
//!
//! - without a `/` matches at any depth: `*.rs` is `**/*.rs`, `src` is any folder (or file)
//!   named `src` (VS Code's rule for such patterns);
//! - with a `/` is relative to the root: `src/app`, `src/**/*.rs`; a leading `./` or `/` also
//!   anchors a pattern without one (`./src`: only the root's `src`), and `**/` starts it at any
//!   depth. (VS Code puts every pattern not starting with `./` at any depth; jed anchors one with
//!   a `/` in it, as `.gitignore` does.)
//! - also matches everything under what it matches: `src` = `src/**` (a folder or path means its
//!   contents);
//! - `*` and `?` stop at `/`, `**` crosses folders, `[…]` and `{a,b}` as in globs; matching is
//!   case-sensitive; a trailing `/` is dropped before deciding (`src/` = `src`);
//! - with a leading `!` takes its matches back out of the field's others: `src, !src/gen` in
//!   include keeps `src` but not `src/gen`; in exclude, `target, !target/doc` excludes `target`
//!   but not `target/doc`. A field of only `!` patterns: include = everything but them, exclude
//!   = nothing.
//!
//! A path is searched when include selects it (an empty include selects everything) and exclude
//! doesn't: exclude beats include. Both apply on top of `.gitignore` and `.workspace` excludes,
//! which the walk handles. An invalid glob is a `SearchError` naming the field and pattern.

use std::path::Path;
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::search::SearchError;

/// Both fields compiled. `Default` is the empty filter (everything passes). Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct PathFilter {
    include: Arc<Field>,
    exclude: Arc<Field>,
}

/// One field's patterns: those without `!` and those with.
#[derive(Debug, Default)]
struct Field {
    /// `None`: no such pattern given.
    positive: Option<GlobSet>,
    negative: Option<GlobSet>,
}

impl Field {
    fn parse(text: &str, name: &str) -> Result<Field, SearchError> {
        let (mut positive, mut negative) = (GlobSetBuilder::new(), GlobSetBuilder::new());
        let (mut any_positive, mut any_negative) = (false, false);
        for pattern in split(text) {
            let Some((negated, globs)) = globs(pattern) else {
                continue;
            };
            let set = if negated { &mut negative } else { &mut positive };
            for g in globs {
                let glob = GlobBuilder::new(&g)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .build()
                    .map_err(|e| SearchError::new(format!("{name}: {pattern}: {}", e.kind())))?;
                set.add(glob);
            }
            *(if negated { &mut any_negative } else { &mut any_positive }) = true;
        }
        let build = |b: GlobSetBuilder, any: bool| -> Result<Option<GlobSet>, SearchError> {
            if !any {
                return Ok(None);
            }
            b.build()
                .map(Some)
                .map_err(|e| SearchError::new(format!("{name}: {}", e.kind())))
        };
        Ok(Field {
            positive: build(positive, any_positive)?,
            negative: build(negative, any_negative)?,
        })
    }

    fn positive(&self, rel: &Path) -> Option<bool> {
        self.positive.as_ref().map(|s| s.is_match(rel))
    }

    fn negative(&self, rel: &Path) -> bool {
        self.negative.as_ref().is_some_and(|s| s.is_match(rel))
    }

    fn is_empty(&self) -> bool {
        self.positive.is_none() && self.negative.is_none()
    }
}

impl PathFilter {
    /// The two fields' text: `Err` names the field and the bad pattern.
    pub fn new(include: &str, exclude: &str) -> Result<PathFilter, SearchError> {
        Ok(PathFilter {
            include: Arc::new(Field::parse(include, "files to include")?),
            exclude: Arc::new(Field::parse(exclude, "files to exclude")?),
        })
    }

    /// No patterns in either field: everything passes.
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Whether the file at `rel` (relative to the root, `/`-separated) is searched.
    pub fn matches(&self, rel: &Path) -> bool {
        let included = self.include.positive(rel).unwrap_or(true) && !self.include.negative(rel);
        let excluded = self.exclude.positive(rel).unwrap_or(false) && !self.exclude.negative(rel);
        included && !excluded
    }

    /// Whether no file under the folder at `rel` can match, so the walk needn't enter it: the
    /// folder is taken out by an include `!` pattern, or excluded with no exclude `!` pattern
    /// that could take something under it back. (A pattern's matches take everything under them,
    /// so this only ever skips folders whose files `matches` refuses.)
    pub fn skips_folder(&self, rel: &Path) -> bool {
        self.include.negative(rel) || (self.exclude.negative.is_none() && self.exclude.positive(rel) == Some(true))
    }
}

/// The patterns of a field: split at commas outside `{…}` and not escaped, trimmed, empty ones
/// dropped.
fn split(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut escaped, mut start) = (0usize, false, 0);
    for (i, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out.into_iter().map(trim).filter(|p| !p.is_empty()).collect()
}

/// A pattern without the spaces around it, but keeping an escaped one at its end (`a\ ` is
/// `a` and a space: "Find in Folder…" escapes a folder name's trailing space).
fn trim(p: &str) -> &str {
    let p = p.trim_start();
    let end = p.trim_end().len();
    let slashes = p[..end].bytes().rev().take_while(|&b| b == b'\\').count();
    match p[end..].chars().next() {
        Some(c) if slashes % 2 == 1 => &p[..end + c.len_utf8()],
        _ => &p[..end],
    }
}

/// A pattern as globs: whether it's negated, then the glob and the glob for everything under it.
/// `None` for a bare `!`.
fn globs(pattern: &str) -> Option<(bool, Vec<String>)> {
    let (negated, mut p) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, pattern),
    };
    if negated && p.is_empty() {
        return None;
    }
    let mut anchored = false;
    loop {
        if let Some(rest) = p.strip_prefix("./") {
            p = rest;
        } else if let Some(rest) = p.strip_prefix('/') {
            p = rest;
        } else {
            break;
        }
        anchored = true;
    }
    if p == "." {
        p = "";
    }
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        // `./` or `/`: the root, so everything.
        return Some((negated, vec!["**".to_string()]));
    }
    let glob = if anchored || p.contains('/') {
        p.to_string()
    } else {
        format!("**/{p}")
    };
    let under = format!("{glob}/**");
    Some((negated, vec![glob, under]))
}

/// A root-relative folder path as an include pattern meaning exactly that folder ("Find in
/// Folder…"): `./` + the path, glob characters, commas and a trailing space escaped.
pub fn folder_pattern(rel: &Path) -> String {
    let mut out = String::from("./");
    let rel = rel.to_string_lossy();
    let last = rel.chars().count().saturating_sub(1);
    for (i, c) in rel.chars().enumerate() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | ',' | '\\' | '!') || (i == last && c.is_whitespace()) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(include: &str, exclude: &str) -> PathFilter {
        PathFilter::new(include, exclude).unwrap()
    }

    /// The paths of `all` that `f` passes.
    fn passed(f: &PathFilter, all: &[&str]) -> Vec<String> {
        all.iter()
            .filter(|p| f.matches(Path::new(p)))
            .map(|p| p.to_string())
            .collect()
    }

    const TREE: [&str; 10] = [
        "a.rs",
        "README.md",
        "src/main.rs",
        "src/app/mod.rs",
        "src/app/notes.md",
        "src/gen/out.rs",
        "lib/src/x.rs",
        "tests/t.rs",
        "target/doc/i.html",
        "target/debug/jed",
    ];

    #[test]
    fn empty_fields_pass_everything() {
        let f = filter("", " , ,");
        assert!(f.is_empty());
        assert_eq!(passed(&f, &TREE).len(), TREE.len());
        assert!(!f.skips_folder(Path::new("src")));
    }

    #[test]
    fn a_pattern_without_a_slash_matches_at_any_depth() {
        assert_eq!(
            passed(&filter("*.rs", ""), &TREE),
            [
                "a.rs",
                "src/main.rs",
                "src/app/mod.rs",
                "src/gen/out.rs",
                "lib/src/x.rs",
                "tests/t.rs"
            ]
        );
        // A bare folder name: every folder of that name, everything under it.
        assert_eq!(
            passed(&filter("src", ""), &TREE),
            [
                "src/main.rs",
                "src/app/mod.rs",
                "src/app/notes.md",
                "src/gen/out.rs",
                "lib/src/x.rs"
            ]
        );
        assert_eq!(
            passed(&filter("app", ""), &TREE),
            ["src/app/mod.rs", "src/app/notes.md"]
        );
        // A file name too.
        assert_eq!(passed(&filter("mod.rs", ""), &TREE), ["src/app/mod.rs"]);
        // `*` stops at `/`.
        assert!(passed(&filter("s*", ""), &TREE).iter().all(|p| p.contains("src/")));
    }

    #[test]
    fn a_path_is_relative_to_the_root_and_means_everything_under_it() {
        let src = ["src/main.rs", "src/app/mod.rs", "src/app/notes.md", "src/gen/out.rs"];
        for p in ["./src", "/src", "./src/", "src/**", "./src/**"] {
            assert_eq!(passed(&filter(p, ""), &TREE), src, "{p}");
        }
        // A trailing `/` alone doesn't anchor (as `.gitignore`).
        assert_eq!(passed(&filter("src/", ""), &TREE), passed(&filter("src", ""), &TREE));
        assert_eq!(
            passed(&filter("src/app", ""), &TREE),
            ["src/app/mod.rs", "src/app/notes.md"]
        );
        assert!(
            passed(&filter("app/mod.rs", ""), &TREE).is_empty(),
            "anchored at the root"
        );
        assert_eq!(
            passed(&filter("**/src/*.rs", ""), &TREE),
            ["src/main.rs", "lib/src/x.rs"]
        );
        assert_eq!(
            passed(&filter("src/**/*.rs", ""), &TREE),
            ["src/main.rs", "src/app/mod.rs", "src/gen/out.rs"]
        );
        assert_eq!(passed(&filter("src/*.rs", ""), &TREE), ["src/main.rs"]);
        assert_eq!(passed(&filter("./", ""), &TREE).len(), TREE.len(), "the root");
        assert_eq!(passed(&filter(".", ""), &TREE).len(), TREE.len());
    }

    #[test]
    fn commas_split_except_in_braces_or_escaped() {
        assert_eq!(
            passed(&filter("tests, README.md", ""), &TREE),
            ["README.md", "tests/t.rs"]
        );
        assert_eq!(
            passed(&filter("*.{md,html}", ""), &TREE),
            ["README.md", "src/app/notes.md", "target/doc/i.html"]
        );
        assert_eq!(split(r"a\,b, c"), [r"a\,b", "c"]);
        assert!(filter(r"a\,b", "").matches(Path::new("a,b")));
        assert_eq!(split(" , {a,b} ,"), ["{a,b}"]);
        // An escaped space at the end stays (a folder name's, from "Find in Folder…").
        assert_eq!(split(r"a\ , b ,c\\ "), [r"a\ ", "b", r"c\\"]);
    }

    #[test]
    fn exclude_beats_include() {
        let f = filter("src", "app");
        assert_eq!(passed(&f, &TREE), ["src/main.rs", "src/gen/out.rs", "lib/src/x.rs"]);
        let f = filter("*.rs", "src/**");
        assert_eq!(passed(&f, &TREE), ["a.rs", "lib/src/x.rs", "tests/t.rs"]);
        assert_eq!(passed(&filter("tests", "tests"), &TREE), Vec::<String>::new());
        assert_eq!(passed(&filter("", "target, *.md"), &TREE).len(), 6);
    }

    #[test]
    fn negation_works_within_a_field() {
        // Include: `!` takes matches out of the others.
        assert_eq!(
            passed(&filter("src, !src/gen, !*.md", ""), &TREE),
            ["src/main.rs", "src/app/mod.rs", "lib/src/x.rs"]
        );
        // Only negations: everything else.
        assert_eq!(
            passed(&filter("!src, !target", ""), &TREE),
            ["a.rs", "README.md", "tests/t.rs"]
        );
        // Exclude: `!` takes matches back.
        assert_eq!(
            passed(&filter("", "target, !target/doc"), &TREE).last().unwrap(),
            "target/doc/i.html"
        );
        assert!(!filter("", "target, !target/doc").matches(Path::new("target/debug/jed")));
        // An exclude of only negations excludes nothing; `!` doesn't beat the other field.
        assert_eq!(passed(&filter("", "!src"), &TREE).len(), TREE.len());
        assert_eq!(passed(&filter("!src", "!src"), &TREE).len(), 5);
        assert!(!filter("src", "!src").is_empty());
        // A bare `!` is nothing; `! x` is `!x`.
        assert!(filter("!", "").is_empty());
        assert_eq!(passed(&filter("! src", ""), &TREE).len(), 5);
    }

    #[test]
    fn skipping_a_folder_agrees_with_its_files() {
        let fields = [
            ("", ""),
            ("src", ""),
            ("!src/gen", ""),
            ("*.rs, !app", "target"),
            ("", "target, !target/doc"),
            ("", "src/app"),
            ("", "*.md"),
            ("lib", "src"),
        ];
        let folders = [
            "src",
            "src/app",
            "src/gen",
            "lib",
            "lib/src",
            "tests",
            "target",
            "target/doc",
        ];
        for (include, exclude) in fields {
            let f = filter(include, exclude);
            for dir in folders {
                if f.skips_folder(Path::new(dir)) {
                    for file in TREE.iter().filter(|p| p.starts_with(&format!("{dir}/"))) {
                        assert!(
                            !f.matches(Path::new(file)),
                            "{include:?} {exclude:?}: {dir} skipped, {file} matches"
                        );
                    }
                }
            }
        }
        assert!(filter("!src/gen", "").skips_folder(Path::new("src/gen")));
        assert!(filter("", "target").skips_folder(Path::new("target")));
        assert!(!filter("", "target, !target/doc").skips_folder(Path::new("target")));
        assert!(
            !filter("src", "").skips_folder(Path::new("lib")),
            "include never skips a folder"
        );
    }

    #[test]
    fn an_invalid_glob_names_its_field_and_pattern() {
        let e = PathFilter::new("src, {a,b", "").unwrap_err().to_string();
        assert!(e.starts_with("files to include: {a,b: "), "{e}");
        let e = PathFilter::new("", "[z-a]").unwrap_err().to_string();
        assert!(e.starts_with("files to exclude: [z-a]: "), "{e}");
        assert!(!e.contains('\n'));
    }

    #[test]
    fn a_folder_pattern_means_exactly_that_folder() {
        let tree = [
            "src/a.rs",
            "lib/src/b.rs",
            "we*ird,{x}/c.rs",
            "we*ird,{x}y/d.rs",
            "!bang/e.rs",
            "x[1]?/f.rs",
            "x1?/g.rs",
            "sp ace /h.rs",
            "sp ace/i.rs",
            " lead/j.rs",
            "back\\slash/k.rs",
        ];
        for (dir, want) in [
            ("src", "src/a.rs"),
            ("lib/src", "lib/src/b.rs"),
            ("we*ird,{x}", "we*ird,{x}/c.rs"),
            ("!bang", "!bang/e.rs"),
            ("x[1]?", "x[1]?/f.rs"),
            ("sp ace ", "sp ace /h.rs"),
            ("sp ace", "sp ace/i.rs"),
            (" lead", " lead/j.rs"),
            ("back\\slash", "back\\slash/k.rs"),
        ] {
            let p = folder_pattern(Path::new(dir));
            assert_eq!(passed(&filter(&p, ""), &tree), [want], "{p}");
        }
        assert_eq!(folder_pattern(Path::new("src/app")), "./src/app");
    }
}
