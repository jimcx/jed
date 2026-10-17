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
    Rust,
    Markdown,
    Yaml,
    Json,
    /// JSON with comments (`tsconfig.json`, `.vscode/*.json`, …): JSON's grammar (which accepts
    /// comments) with a `//` line comment.
    Jsonc,
    Xml,
    Toml,
    /// Bash and other POSIX-like shells (VS Code's `shellscript`).
    ShellScript,
    CMake,
    Makefile,
    Dockerfile,
}

impl Language {
    pub const ALL: [Language; 15] = [
        Language::PlainText,
        Language::C,
        Language::Cpp,
        Language::Python,
        Language::Rust,
        Language::Markdown,
        Language::Yaml,
        Language::Json,
        Language::Jsonc,
        Language::Xml,
        Language::Toml,
        Language::ShellScript,
        Language::CMake,
        Language::Makefile,
        Language::Dockerfile,
    ];

    /// VS Code's language id (also LSP's `languageId`, and the value `.workspace` associations name).
    pub fn id(self) -> &'static str {
        match self {
            Language::PlainText => "plaintext",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Python => "python",
            Language::Rust => "rust",
            Language::Markdown => "markdown",
            Language::Yaml => "yaml",
            Language::Json => "json",
            Language::Jsonc => "jsonc",
            Language::Xml => "xml",
            Language::Toml => "toml",
            Language::ShellScript => "shellscript",
            Language::CMake => "cmake",
            Language::Makefile => "makefile",
            Language::Dockerfile => "dockerfile",
        }
    }

    /// The status bar's language mode (VS Code's names).
    pub fn name(self) -> &'static str {
        match self {
            Language::PlainText => "Plain Text",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Python => "Python",
            Language::Rust => "Rust",
            Language::Markdown => "Markdown",
            Language::Yaml => "YAML",
            Language::Json => "JSON",
            Language::Jsonc => "JSON with Comments",
            Language::Xml => "XML",
            Language::Toml => "TOML",
            Language::ShellScript => "Shell Script",
            Language::CMake => "CMake",
            Language::Makefile => "Makefile",
            Language::Dockerfile => "Dockerfile",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// The tree-sitter grammar, if the language has one.
    pub fn grammar(self) -> Option<tree_sitter::Language> {
        Some(match self {
            Language::PlainText => return None,
            Language::C => tree_sitter_c::LANGUAGE.into(),
            Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            // The block grammar only: inline text (emphasis, links, code spans) is one leaf.
            Language::Markdown => tree_sitter_md::LANGUAGE.into(),
            Language::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Language::Json | Language::Jsonc => tree_sitter_json::LANGUAGE.into(),
            Language::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            Language::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Language::ShellScript => tree_sitter_bash::LANGUAGE.into(),
            Language::CMake => tree_sitter_cmake::LANGUAGE.into(),
            Language::Makefile => tree_sitter_make::LANGUAGE.into(),
            Language::Dockerfile => tree_sitter_containerfile::LANGUAGE.into(),
        })
    }

    /// The highlight queries, in order (for the same node a later pattern wins): the grammar's
    /// `highlights.scm` (C++'s extends C's: C's first), then jed's additions (`queries/*.scm`:
    /// control keywords, built-in types, parameters, … for Dark+'s colours). Rust starts with
    /// `(identifier) @variable` (its grammar colours no plain identifiers, Dark+ does). CMake's
    /// and TOML's grammar queries are replaced by jed's own (CMake's relies on Neovim's
    /// `#lua-match?`; TOML's captures each whole `pair`).
    pub fn highlight_queries(self) -> &'static [&'static str] {
        const C: &str = include_str!("queries/c.scm");
        const CPP: &str = include_str!("queries/cpp.scm");
        const PYTHON: &str = include_str!("queries/python.scm");
        const RUST_FIRST: &str = "(identifier) @variable\n";
        const RUST: &str = include_str!("queries/rust.scm");
        const MARKDOWN: &str = include_str!("queries/markdown.scm");
        const YAML: &str = include_str!("queries/yaml.scm");
        const JSON: &str = include_str!("queries/json.scm");
        const XML: &str = include_str!("queries/xml.scm");
        const TOML: &str = include_str!("queries/toml.scm");
        const SHELL: &str = include_str!("queries/shell.scm");
        const CMAKE: &str = include_str!("queries/cmake.scm");
        const MAKE: &str = include_str!("queries/make.scm");
        const DOCKER: &str = include_str!("queries/dockerfile.scm");
        match self {
            Language::PlainText => &[],
            Language::C => &[tree_sitter_c::HIGHLIGHT_QUERY, C],
            Language::Cpp => &[tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY, C, CPP],
            Language::Python => &[tree_sitter_python::HIGHLIGHTS_QUERY, PYTHON],
            Language::Rust => &[RUST_FIRST, tree_sitter_rust::HIGHLIGHTS_QUERY, RUST],
            Language::Markdown => &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK, MARKDOWN],
            Language::Yaml => &[tree_sitter_yaml::HIGHLIGHTS_QUERY, YAML],
            Language::Json | Language::Jsonc => &[tree_sitter_json::HIGHLIGHTS_QUERY, JSON],
            Language::Xml => &[tree_sitter_xml::XML_HIGHLIGHT_QUERY, XML],
            Language::Toml => &[TOML],
            Language::ShellScript => &[tree_sitter_bash::HIGHLIGHT_QUERY, SHELL],
            Language::CMake => &[CMAKE],
            Language::Makefile => &[tree_sitter_make::HIGHLIGHTS_QUERY, MAKE],
            Language::Dockerfile => &[tree_sitter_containerfile::HIGHLIGHTS_QUERY, DOCKER],
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
    detect_path(path, root, associations).unwrap_or_else(|| detect_shebang(first_line))
}

/// `detect`'s first steps (associations, then the name), without the text: `None` when the
/// shebang decides (a diff tab of a file not open reads its text off the UI thread).
pub fn detect_path(path: &Path, root: &Path, associations: &Associations) -> Option<Language> {
    associations.lookup(path, root).or_else(|| by_name(path))
}

/// `detect`'s last step: the language a text's first line names (a shebang), else plain text.
pub fn detect_shebang(first_line: &str) -> Language {
    by_shebang(first_line).unwrap_or_default()
}

/// File names and extensions, as VS Code's defaults (`.h` is C++, as there: the C++ grammar
/// parses C headers, the C grammar not C++ ones). SCons files are Python; jed's `.workspace` is
/// TOML. Exact names first (`Makefile`, `CMakeLists.txt`, `Cargo.lock`, `.bashrc`, …), then name
/// patterns (`Dockerfile.*`, `tsconfig.*.json`, `.vscode/*.json`), then the extension (a
/// dotfile's name isn't one: `.c` is plain text), multi-dot ones (`.js.map`) included.
fn by_name(path: &Path) -> Option<Language> {
    use Language::*;
    let name = path.file_name()?.to_str()?;
    let exact = match name {
        "SConstruct" | "SConscript" | "Sconstruct" | "sconstruct" => Some(Python),
        "Makefile" | "makefile" | "GNUmakefile" | "OCamlMakefile" => Some(Makefile),
        "CMakeLists.txt" => Some(CMake),
        "Dockerfile" | "Containerfile" => Some(Dockerfile),
        "Cargo.lock" | "Pipfile" | "poetry.lock" | "uv.lock" | ".workspace" => Some(Toml),
        "composer.lock" | ".watchmanconfig" => Some(Json),
        "tsconfig.json" | "jsconfig.json" | "babel.config.json" | ".babelrc" | ".babelrc.json" | ".eslintrc"
        | ".eslintrc.json" | ".jshintrc" | ".jsfmtrc" | ".swcrc" | ".hintrc" | ".ember-cli" | "typedoc.json"
        | ".devcontainer.json" | "devcontainer.json" => Some(Jsonc),
        ".clang-format" | "_clang-format" | ".clang-tidy" | ".clangd" => Some(Yaml),
        ".bashrc"
        | ".bash_aliases"
        | ".bash_profile"
        | ".bash_login"
        | ".bash_logout"
        | ".profile"
        | ".xprofile"
        | ".xsession"
        | ".xsessionrc"
        | ".Xsession"
        | ".zshrc"
        | ".zprofile"
        | ".zlogin"
        | ".zlogout"
        | ".zshenv"
        | ".cshrc"
        | ".tcshrc"
        | ".kshrc"
        | ".yashrc"
        | ".yash_profile"
        | ".envrc"
        | ".hushlogin"
        | "zshrc"
        | "zshenv"
        | "zlogin"
        | "zprofile"
        | "zlogout"
        | "bashrc_Apple_Terminal"
        | "zshrc_Apple_Terminal"
        | "APKBUILD"
        | "PKGBUILD" => Some(ShellScript),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }
    if name.starts_with("Dockerfile.") || name.starts_with("Containerfile.") {
        return Some(Dockerfile);
    }
    let lower = name.to_ascii_lowercase();
    let config_json = ["tsconfig.", "jsconfig."].iter().any(|p| lower.starts_with(p)) && lower.ends_with(".json");
    let in_vscode = path.parent().and_then(Path::file_name).is_some_and(|d| d == ".vscode");
    if config_json || (in_vscode && lower.ends_with(".json")) {
        return Some(Jsonc);
    }
    // Multi-dot extensions VS Code lists (`.js.map`, `.csproj.user`).
    const SUFFIXES: &[(&str, Language)] = &[
        (".js.map", Json),
        (".css.map", Json),
        (".ts.map", Json),
        (".csproj.user", Xml),
        (".vbproj.user", Xml),
        (".pubxml.user", Xml),
        (".vcxproj.filters", Xml),
    ];
    if let Some(&(_, l)) = SUFFIXES
        .iter()
        .find(|(s, _)| lower.len() > s.len() && lower.ends_with(s))
    {
        return Some(l);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "c" | "i" => C,
        "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ii" | "ipp" | "inl" | "tpp" | "txx"
        | "ixx" | "cppm" | "ino" => Cpp,
        "py" | "pyi" | "pyw" | "ipy" | "gyp" | "gypi" | "scons" => Python,
        "rs" => Rust,
        "md" | "mkd" | "mdwn" | "mdown" | "markdown" | "markdn" | "mdtxt" | "mdtext" | "workbook" => Markdown,
        "yaml" | "yml" | "eyaml" | "eyml" | "cff" | "yaml-tmlanguage" | "yaml-tmpreferences" | "yaml-tmtheme"
        | "winget" => Yaml,
        "json" | "bowerrc" | "jscsrc" | "webmanifest" | "har" | "jslintrc" | "jsonld" | "geojson" | "ipynb"
        | "vuerc" => Json,
        "jsonc" | "code-workspace" | "code-snippets" | "code-profile" => Jsonc,
        "xml" | "xsd" | "ascx" | "atom" | "axml" | "axaml" | "bpmn" | "cpt" | "csl" | "csproj" | "dita" | "ditamap"
        | "dtd" | "ent" | "dtml" | "fsproj" | "fxml" | "iml" | "isml" | "jmx" | "launch" | "menu" | "mxml"
        | "nuspec" | "opml" | "owl" | "proj" | "props" | "pt" | "publishsettings" | "pubxml" | "rbxlx" | "rbxmx"
        | "rdf" | "rng" | "rss" | "shproj" | "slnx" | "storyboard" | "svg" | "targets" | "tld" | "tmx" | "vbproj"
        | "vcxproj" | "wsdl" | "wxi" | "wxl" | "wxs" | "xaml" | "xbl" | "xib" | "xlf" | "xliff" | "xpdl" | "xul"
        | "xoml" => Xml,
        "toml" => Toml,
        "sh" | "bash" | "zsh" | "ksh" | "csh" | "ebuild" | "eclass" | "zsh-theme" => ShellScript,
        "cmake" => CMake,
        "mk" | "mak" => Makefile,
        "dockerfile" | "containerfile" => Dockerfile,
        _ => return None,
    })
}

/// The interpreter a shebang names: `#!/usr/bin/python3`, `#!/usr/bin/env python`,
/// `#!/usr/bin/env -S python3 -u`, `#!/bin/sh`, `#!/usr/bin/make -f`, …
fn by_shebang(first_line: &str) -> Option<Language> {
    let rest = first_line.strip_prefix("#!")?;
    let mut words = rest.split_whitespace();
    let mut program = basename(words.next()?);
    if program == "env" {
        // Skip env's options and VAR=value assignments.
        program = basename(words.find(|w| !w.starts_with('-') && !w.contains('='))?);
    }
    // A name followed only by a version (`python3.12`, `bash5`).
    let is = |stem: &str| {
        program
            .strip_prefix(stem)
            .is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
    };
    const SHELLS: &[&str] = &[
        "sh", "bash", "zsh", "ksh", "dash", "ash", "mksh", "pdksh", "dtksh", "yash", "csh", "tcsh", "jcsh", "itcsh",
    ];
    if is("python") || is("pypy") {
        Some(Language::Python)
    } else if SHELLS.iter().any(|s| is(s)) {
        Some(Language::ShellScript)
    } else if is("make") || is("gmake") {
        Some(Language::Makefile)
    } else {
        None
    }
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
        assert_eq!(Language::from_id("rust"), Some(Language::Rust));
        assert_eq!(Language::from_id("shellscript"), Some(Language::ShellScript));
        assert_eq!(Language::from_id("bash"), None);
        // Every language but plain text has a grammar and a query.
        for l in Language::ALL {
            assert_eq!(l.grammar().is_some(), l != Language::PlainText, "{l:?}");
            assert_eq!(l.highlight_queries().is_empty(), l == Language::PlainText, "{l:?}");
        }
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
        assert_eq!(detect_plain("/ws/.c", ""), Language::PlainText);
        assert_eq!(detect_plain("/ws/Makefile.txt", ""), Language::PlainText);
    }

    /// The new languages' names and extensions (VS Code's lists).
    #[test]
    fn by_name_table() {
        use Language::*;
        let table = [
            ("main.rs", Rust),
            ("README.md", Markdown),
            ("notes.markdown", Markdown),
            ("ci.yml", Yaml),
            ("compose.yaml", Yaml),
            (".clang-format", Yaml),
            ("CITATION.cff", Yaml),
            ("package.json", Json),
            ("composer.lock", Json),
            ("app.js.map", Json),
            ("tsconfig.json", Jsonc),
            ("tsconfig.base.json", Jsonc),
            ("a.jsonc", Jsonc),
            ("jed.code-workspace", Jsonc),
            (".eslintrc", Jsonc),
            ("pom.xml", Xml),
            ("icon.svg", Xml),
            ("App.csproj", Xml),
            ("App.csproj.user", Xml),
            ("Cargo.toml", Toml),
            ("Cargo.lock", Toml),
            ("Pipfile", Toml),
            (".workspace", Toml),
            ("build.sh", ShellScript),
            ("x.bash", ShellScript),
            (".bashrc", ShellScript),
            (".zshrc", ShellScript),
            (".profile", ShellScript),
            ("PKGBUILD", ShellScript),
            ("CMakeLists.txt", CMake),
            ("toolchain.cmake", CMake),
            ("Makefile", Makefile),
            ("makefile", Makefile),
            ("GNUmakefile", Makefile),
            ("rules.mk", Makefile),
            ("Dockerfile", Dockerfile),
            ("Containerfile", Dockerfile),
            ("Dockerfile.dev", Dockerfile),
            ("web.dockerfile", Dockerfile),
            ("app.Dockerfile", Dockerfile),
        ];
        for (name, want) in table {
            assert_eq!(detect_plain(&format!("/ws/sub/{name}"), ""), want, "{name}");
        }
        assert_eq!(detect_plain("/ws/.vscode/settings.json", ""), Jsonc);
        assert_eq!(detect_plain("/ws/vscode/settings.json", ""), Json);
        // Exact names are case-sensitive (as VS Code's `filenames`); extensions aren't.
        assert_eq!(detect_plain("/ws/cmakelists.txt", ""), PlainText);
        assert_eq!(detect_plain("/ws/A.RS", ""), Rust);
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
            "#!/bin/bash -e",
            "#!/usr/bin/env bash",
            "#!/usr/bin/env -S bash -eu",
            "#!/usr/local/bin/zsh",
            "#!/bin/dash",
            "#!/bin/ksh93",
        ] {
            assert_eq!(detect_plain("/ws/tool", line), Language::ShellScript, "{line}");
        }
        assert_eq!(detect_plain("/ws/tool", "#!/usr/bin/make -f"), Language::Makefile);
        for line in [
            "#!/usr/bin/pythonista",
            "#!/usr/bin/shellcheck",
            "#!/usr/bin/env node",
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
            ("*.x".to_string(), "cobol".to_string()),
            ("*.conf".to_string(), "shellscript".to_string()),
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
        assert_eq!(d("/ws/a.conf"), Language::ShellScript);
    }
}
