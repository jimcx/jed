//! Language detection: `.workspace` associations, then file name / extension, then the shebang.

use std::collections::BTreeMap;
use std::path::Path;

use globset::{Glob, GlobMatcher};

/// A buffer's language mode. `id()` is the LSP `languageId` (VS Code's ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    PlainText,
    C,
    Cpp,
    Python,
}

impl Language {
    pub const ALL: [Language; 4] = [Language::PlainText, Language::C, Language::Cpp, Language::Python];

    /// VS Code's language id (also LSP's `languageId`, and the value `.workspace` associations name).
    pub fn id(self) -> &'static str {
        match self {
            Language::PlainText => "plaintext",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Python => "python",
        }
    }

    /// The status bar's language mode.
    pub fn name(self) -> &'static str {
        match self {
            Language::PlainText => "Plain Text",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Python => "Python",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// The tree-sitter grammar, if the language has one.
    pub fn grammar(self) -> Option<tree_sitter::Language> {
        match self {
            Language::PlainText => None,
            Language::C => Some(tree_sitter_c::LANGUAGE.into()),
            Language::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
            Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
        }
    }

    /// The highlight queries, in order (for the same node a later pattern wins): the grammar's
    /// `highlights.scm` (C++'s extends C's: C's first), then red's additions (`queries/*.scm`:
    /// control keywords, built-in types, parameters, … for Dark+'s colours).
    pub fn highlight_queries(self) -> &'static [&'static str] {
        const C: &str = include_str!("queries/c.scm");
        const CPP: &str = include_str!("queries/cpp.scm");
        const PYTHON: &str = include_str!("queries/python.scm");
        match self {
            Language::PlainText => &[],
            Language::C => &[tree_sitter_c::HIGHLIGHT_QUERY, C],
            Language::Cpp => &[tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY, C, CPP],
            Language::Python => &[tree_sitter_python::HIGHLIGHTS_QUERY, PYTHON],
        }
    }
}

/// `.workspace` `[files] associations`: glob → language id. A glob with a `/` matches the path
/// relative to the workspace root, else the file name (as VS Code's `files.associations`).
#[derive(Debug, Default)]
pub struct Associations {
    rules: Vec<(GlobMatcher, bool, Language)>,
}

impl Associations {
    /// Compiles the map; returns the entries it couldn't use (bad glob, unknown language id).
    pub fn new(map: &BTreeMap<String, String>) -> (Self, Vec<String>) {
        let mut rules = Vec::new();
        let mut errors = Vec::new();
        for (pattern, id) in map {
            let Some(lang) = Language::from_id(id) else {
                errors.push(format!("unknown language \"{id}\" for \"{pattern}\""));
                continue;
            };
            match Glob::new(pattern.trim_start_matches('/')) {
                Ok(g) => rules.push((g.compile_matcher(), pattern.contains('/'), lang)),
                Err(e) => errors.push(format!("bad pattern \"{pattern}\": {e}")),
            }
        }
        (Self { rules }, errors)
    }

    fn lookup(&self, path: &Path, root: &Path) -> Option<Language> {
        let name = path.file_name()?;
        let rel = path.strip_prefix(root).unwrap_or(path);
        self.rules
            .iter()
            .find(|(glob, whole, _)| {
                if *whole {
                    glob.is_match(rel)
                } else {
                    glob.is_match(name)
                }
            })
            .map(|r| r.2)
    }
}

/// The language of the file at `path` (absolute) whose text starts with `first_line`.
pub fn detect(path: &Path, root: &Path, first_line: &str, associations: &Associations) -> Language {
    associations
        .lookup(path, root)
        .or_else(|| by_name(path))
        .or_else(|| by_shebang(first_line))
        .unwrap_or_default()
}

/// File names and extensions, as VS Code's defaults (`.h` is C++, as there: the C++ grammar
/// parses C headers, the C grammar not C++ ones). SCons files are Python.
fn by_name(path: &Path) -> Option<Language> {
    let name = path.file_name()?.to_str()?;
    if matches!(name, "SConstruct" | "SConscript" | "Sconstruct" | "sconstruct") {
        return Some(Language::Python);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "c" | "i" => Language::C,
        "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ii" | "ipp" | "inl" | "tpp" | "txx"
        | "ixx" | "cppm" | "ino" => Language::Cpp,
        "py" | "pyi" | "pyw" | "ipy" | "gyp" | "gypi" | "scons" => Language::Python,
        _ => return None,
    })
}

/// `#!/usr/bin/python3`, `#!/usr/bin/env python`, `#!/usr/bin/env -S python3 -u`, …
fn by_shebang(first_line: &str) -> Option<Language> {
    let rest = first_line.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut program = basename(words.next()?);
    if program == "env" {
        // Skip env's options and VAR=value assignments.
        program = basename(words.find(|w| !w.starts_with('-') && !w.contains('='))?);
    }
    let is = |stem: &str| {
        program
            .strip_prefix(stem)
            .is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
    };
    (is("python") || is("pypy")).then_some(Language::Python)
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_plain(path: &str, first: &str) -> Language {
        detect(Path::new(path), Path::new("/ws"), first, &Associations::default())
    }

    #[test]
    fn ids_round_trip() {
        for l in Language::ALL {
            assert_eq!(Language::from_id(l.id()), Some(l));
        }
        assert_eq!(Language::Cpp.name(), "C++");
        assert_eq!(Language::from_id("rust"), None);
    }

    #[test]
    fn by_extension() {
        assert_eq!(detect_plain("/ws/a.c", ""), Language::C);
        assert_eq!(detect_plain("/ws/a.h", ""), Language::Cpp);
        for ext in ["cc", "cpp", "cxx", "hpp", "hh", "hxx", "ipp", "inl", "CPP"] {
            assert_eq!(detect_plain(&format!("/ws/a.{ext}"), ""), Language::Cpp, "{ext}");
        }
        for ext in ["py", "pyi", "pyw", "scons"] {
            assert_eq!(detect_plain(&format!("/ws/a.{ext}"), ""), Language::Python, "{ext}");
        }
        assert_eq!(detect_plain("/ws/SConstruct", ""), Language::Python);
        assert_eq!(detect_plain("/ws/sub/SConscript", ""), Language::Python);
        assert_eq!(detect_plain("/ws/notes.txt", ""), Language::PlainText);
        assert_eq!(detect_plain("/ws/Makefile", ""), Language::PlainText);
        assert_eq!(detect_plain("/ws/.c", ""), Language::PlainText);
    }

    #[test]
    fn by_shebang_line() {
        for line in [
            "#!/usr/bin/python",
            "#!/usr/bin/python3",
            "#!/usr/local/bin/python3.12 -u",
            "#!/usr/bin/env python",
            "#! /usr/bin/env python3",
            "#!/usr/bin/env -S python3 -u",
            "#!/usr/bin/env -S PYTHONPATH=. python3",
            "#!/usr/bin/pypy3",
        ] {
            assert_eq!(detect_plain("/ws/tool", line), Language::Python, "{line}");
        }
        for line in [
            "#!/bin/sh",
            "#!/usr/bin/env bash",
            "#!/usr/bin/pythonista",
            "# python",
            "",
        ] {
            assert_eq!(detect_plain("/ws/tool", line), Language::PlainText, "{line}");
        }
        // The extension wins over the shebang.
        assert_eq!(detect_plain("/ws/tool.c", "#!/usr/bin/python"), Language::C);
    }

    #[test]
    fn associations_win() {
        let map = BTreeMap::from([
            ("*.scons".to_string(), "python".to_string()),
            ("BUILD".to_string(), "python".to_string()),
            ("*.h".to_string(), "c".to_string()),
            ("vendor/**/*.inc".to_string(), "cpp".to_string()),
            ("*.x".to_string(), "rust".to_string()),
            ("[".to_string(), "c".to_string()),
        ]);
        let (a, errors) = Associations::new(&map);
        assert_eq!(errors.len(), 2, "{errors:?}");
        let d = |p: &str| detect(Path::new(p), Path::new("/ws"), "", &a);
        assert_eq!(d("/ws/sub/BUILD"), Language::Python);
        assert_eq!(d("/ws/a.h"), Language::C);
        assert_eq!(d("/ws/vendor/x/y.inc"), Language::Cpp);
        assert_eq!(d("/ws/other/y.inc"), Language::PlainText);
        assert_eq!(d("/ws/a.x"), Language::PlainText);
    }
}
