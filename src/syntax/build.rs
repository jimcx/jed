//! Building a syntax plugin's grammar from its `src/` (`parser.c`, an optional `scanner.c` or
//! `scanner.cc`, `tree_sitter/*.h`) into `parser.so` / `parser.dylib` beside it, and the
//! `build.toml` stamp that says whether that library is current.
//!
//! No app state: `build` is a plain blocking function returning its outcome and the lines for the
//! grammar's tool log, shared by the app and `--check-language`.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::plugin::LIBRARY;

/// The stamp's file name, beside the library.
pub const STAMP: &str = "build.toml";

/// The compilers to run: `$CC` (else `cc`), and for a `scanner.cc` `$CXX` (else `c++`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub cc: String,
    pub cxx: String,
}

impl Toolchain {
    pub fn from_env() -> Toolchain {
        let var = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| default.into())
        };
        Toolchain {
            cc: var("CC", "cc"),
            cxx: var("CXX", "c++"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No `src/parser.c`: nothing to build (a prebuilt library is loaded as is).
    NoSources,
    /// The stamp matches and the library exists: no compiler ran.
    Fresh,
    Built,
    /// Why not: the compiler's exit status and its first error line (or why it didn't run).
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub outcome: Outcome,
    /// For the grammar's tool log: the command lines run and the compilers' output.
    pub log: Vec<String>,
}

/// The sources the stamp covers, as (name relative to `src/`, path), sorted by name.
fn sources(src: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for (sub, prefix) in [(src.to_path_buf(), ""), (src.join("tree_sitter"), "tree_sitter/")] {
        let Ok(entries) = std::fs::read_dir(&sub) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
            if entry.path().is_file() && matches!(ext, "c" | "cc" | "h") {
                found.push((format!("{prefix}{name}"), entry.path()));
            }
        }
    }
    found.sort();
    found
}

/// The first line of `<compiler> --version` ("" when it can't run).
fn version(compiler: &str) -> String {
    Command::new(compiler)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(str::to_string))
        .unwrap_or_default()
}

/// The SHA-256 (hex) of everything a build depends on: the sources (names and contents), the
/// compilers and their versions, the platform.
fn inputs_hash(sources: &[(String, PathBuf)], compilers: &[&str]) -> std::io::Result<String> {
    let mut h = Sha256::new();
    for (name, path) in sources {
        let text = std::fs::read(path)?;
        h.update(format!("file {name} {}\n", text.len()));
        h.update(&text);
    }
    for c in compilers {
        h.update(format!("\ncompiler {c} {}\n", version(c)));
    }
    h.update(format!(
        "\nplatform {}-{}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn stamp_matches(dir: &Path, hash: &str) -> bool {
    std::fs::read_to_string(dir.join(STAMP))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("inputs").and_then(|v| v.as_str()).map(|s| s == hash))
        .unwrap_or(false)
}

/// A command line as the log shows it.
fn shown(program: &str, args: &[String]) -> String {
    let mut s = program.to_string();
    for a in args {
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// Runs `program args` in `dir`, appending its command line and output to `log`. `Err`: the
/// failure in words.
fn run(program: &str, args: &[String], dir: &Path, log: &mut Vec<String>) -> Result<(), String> {
    log.push(format!("$ {}", shown(program, args)));
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run `{program}`: {e} (set $CC / $CXX to a compiler)"))?;
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    log.extend(stdout.lines().chain(stderr.lines()).map(str::to_string));
    if out.status.success() {
        return Ok(());
    }
    let first = stderr
        .lines()
        .chain(stdout.lines())
        .find(|l| l.contains("error"))
        .or_else(|| stderr.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    Err(format!("`{program}` failed ({}): {}", out.status, first.trim()))
}

/// Builds `dir`'s library from `dir/src` unless the stamp says it is current (or `force`).
/// The library is written to a temporary file and renamed over the old one, so a library already
/// loaded by this process is never overwritten in place. The stamp is written only after a
/// successful build; a failure leaves the old library and stamp alone.
/// Blocking: call from spawn_blocking.
pub fn build(dir: &Path, tools: &Toolchain, force: bool) -> Build {
    let src = dir.join("src");
    let mut log = Vec::new();
    if !src.join("parser.c").is_file() {
        return Build {
            outcome: Outcome::NoSources,
            log: vec!["no src/parser.c: nothing to build".into()],
        };
    }
    let files = sources(&src);
    let cxx = src.join("scanner.cc").is_file();
    let mut compilers = vec![tools.cc.as_str()];
    if cxx {
        compilers.push(tools.cxx.as_str());
    }
    let hash = match inputs_hash(&files, &compilers) {
        Ok(h) => h,
        Err(e) => {
            return Build {
                outcome: Outcome::Failed(format!("cannot read the sources: {e}")),
                log,
            };
        }
    };
    let library = dir.join(LIBRARY);
    if !force && library.is_file() && stamp_matches(dir, &hash) {
        return Build {
            outcome: Outcome::Fresh,
            log: vec!["up to date".into()],
        };
    }
    let result = compile(dir, tools, cxx, &mut log).and_then(|tmp| {
        std::fs::rename(&tmp, &library).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot install {}: {e}", library.display())
        })
    });
    let outcome = match result {
        Ok(()) => {
            let stamp = format!(
                "# Written by jed after a successful build; the library is rebuilt when `inputs` changes.\n\
                 inputs = \"{hash}\"\ncompiler = \"{}\"\nplatform = \"{}-{}\"\n",
                version(&tools.cc).replace(['"', '\\'], ""),
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            match std::fs::write(dir.join(STAMP), stamp) {
                Ok(()) => Outcome::Built,
                Err(e) => Outcome::Failed(format!("built, but cannot write {STAMP}: {e}")),
            }
        }
        Err(e) => Outcome::Failed(e),
    };
    Build { outcome, log }
}

/// Compiles and links into a temporary file in `dir`; returns its path.
fn compile(dir: &Path, tools: &Toolchain, cxx: bool, log: &mut Vec<String>) -> Result<PathBuf, String> {
    let tmp_name = format!(".{LIBRARY}.{}.tmp", std::process::id());
    let tmp = dir.join(&tmp_name);
    let shared = if cfg!(target_os = "macos") {
        "-dynamiclib"
    } else {
        "-shared"
    };
    let strings = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let result = if cxx {
        let objects = dir.join(format!(".objects-{}", std::process::id()));
        let made = std::fs::create_dir_all(&objects).map_err(|e| format!("cannot create {}: {e}", objects.display()));
        let result = made.and_then(|()| {
            let o = |name: &str| objects.join(name).display().to_string();
            let (parser_o, scanner_o) = (o("parser.o"), o("scanner.o"));
            let cc_args = |src: &str, out: &str| strings(&["-c", "-fPIC", "-O2", "-I", "src", src, "-o", out]);
            run(&tools.cc, &cc_args("src/parser.c", &parser_o), dir, log)?;
            run(&tools.cxx, &cc_args("src/scanner.cc", &scanner_o), dir, log)?;
            run(
                &tools.cxx,
                &strings(&[shared, "-fPIC", &parser_o, &scanner_o, "-o", &tmp_name]),
                dir,
                log,
            )
        });
        let _ = std::fs::remove_dir_all(&objects);
        result
    } else {
        let mut args = strings(&[shared, "-fPIC", "-O2", "-I", "src", "src/parser.c"]);
        if dir.join("src/scanner.c").is_file() {
            args.push("src/scanner.c".into());
        }
        args.extend(strings(&["-o", &tmp_name]));
        run(&tools.cc, &args, dir, log)
    };
    match result {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin directory holding a copy of the vendored CMake grammar's `src/`.
    fn copy_of_cmake() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/languages/cmake/src");
        std::fs::create_dir_all(dir.path().join("src/tree_sitter")).unwrap();
        for f in [
            "parser.c",
            "scanner.c",
            "tree_sitter/alloc.h",
            "tree_sitter/array.h",
            "tree_sitter/parser.h",
        ] {
            std::fs::copy(from.join(f), dir.path().join("src").join(f)).unwrap();
        }
        dir
    }

    /// Whether a C compiler is there (a note and a skip without).
    fn have_cc() -> bool {
        let ok = Command::new(Toolchain::from_env().cc).arg("--version").output().is_ok();
        if !ok {
            eprintln!("note: no C compiler; the build test is skipped");
        }
        ok
    }

    fn inode(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).unwrap().ino()
    }

    #[test]
    fn syntax_plugins_build_then_fresh_then_rebuilt_on_change() {
        if !have_cc() {
            return;
        }
        let dir = copy_of_cmake();
        let (p, tools) = (dir.path(), Toolchain::from_env());
        let first = build(p, &tools, false);
        assert_eq!(first.outcome, Outcome::Built, "{:?}", first.log);
        assert!(first.log[0].starts_with("$ "), "{:?}", first.log);
        assert!(first.log[0].contains("-shared") || first.log[0].contains("-dynamiclib"));
        assert!(first.log[0].contains("-fPIC -O2 -I src src/parser.c src/scanner.c"));
        assert!(p.join(LIBRARY).is_file() && p.join(STAMP).is_file());
        let ino = inode(&p.join(LIBRARY));

        let second = build(p, &tools, false);
        assert_eq!(second.outcome, Outcome::Fresh);
        assert_eq!(second.log, ["up to date"]);
        assert_eq!(inode(&p.join(LIBRARY)), ino, "no compiler ran");

        // A changed source (stamps, not file times).
        let scanner = p.join("src/scanner.c");
        let mut text = std::fs::read_to_string(&scanner).unwrap();
        text.push_str("\n/* changed */\n");
        std::fs::write(&scanner, text).unwrap();
        let third = build(p, &tools, false);
        assert_eq!(third.outcome, Outcome::Built);
        assert_ne!(inode(&p.join(LIBRARY)), ino, "renamed over, never written in place");
        assert_eq!(build(p, &tools, false).outcome, Outcome::Fresh);

        // A changed header too; files that aren't sources (grammar repos ship JSON) don't count.
        let header = p.join("src/tree_sitter/parser.h");
        let mut text = std::fs::read_to_string(&header).unwrap();
        text.push_str("\n/* changed */\n");
        std::fs::write(&header, text).unwrap();
        assert_eq!(build(p, &tools, false).outcome, Outcome::Built, "a header");
        std::fs::write(p.join("src/grammar.json"), "{}").unwrap();
        std::fs::write(p.join("src/node-types.json"), "[]").unwrap();
        assert_eq!(build(p, &tools, false).outcome, Outcome::Fresh, "JSON isn't a source");

        // A different compiler is a different build (this one can't run: the old library and
        // stamp stay, and are current again for the old compiler).
        let ino = inode(&p.join(LIBRARY));
        let other = Toolchain {
            cc: "jed-no-such-compiler".into(),
            ..tools.clone()
        };
        assert!(matches!(build(p, &other, false).outcome, Outcome::Failed(_)));
        assert_eq!(build(p, &tools, false).outcome, Outcome::Fresh);
        assert_eq!(inode(&p.join(LIBRARY)), ino);

        // A stamp without its library is not fresh.
        std::fs::remove_file(p.join(LIBRARY)).unwrap();
        assert_eq!(build(p, &tools, false).outcome, Outcome::Built, "library gone");
        assert_eq!(build(p, &tools, true).outcome, Outcome::Built, "force");
        // No temporary files left behind.
        let left: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|n| !n.ends_with(".tmp")), "{left:?}");
    }

    #[test]
    fn syntax_plugins_build_failure_keeps_the_old_library() {
        if !have_cc() {
            return;
        }
        let dir = copy_of_cmake();
        let (p, tools) = (dir.path(), Toolchain::from_env());
        assert_eq!(build(p, &tools, false).outcome, Outcome::Built);
        let (ino, stamp) = (inode(&p.join(LIBRARY)), std::fs::read_to_string(p.join(STAMP)).unwrap());
        let parser = p.join("src/parser.c");
        let mut text = std::fs::read_to_string(&parser).unwrap();
        text.push_str("\nthis is not C;\n");
        std::fs::write(&parser, text).unwrap();
        let b = build(p, &tools, false);
        let Outcome::Failed(why) = &b.outcome else {
            panic!("{:?}", b.outcome)
        };
        assert!(why.contains("failed") && why.contains("error"), "{why}");
        assert!(b.log[0].starts_with("$ "));
        assert!(b.log.len() > 1, "the compiler's output is logged: {:?}", b.log);
        assert_eq!(inode(&p.join(LIBRARY)), ino);
        assert_eq!(
            std::fs::read_to_string(p.join(STAMP)).unwrap(),
            stamp,
            "stamp untouched"
        );
        let again = build(p, &tools, false);
        assert!(
            matches!(again.outcome, Outcome::Failed(_)),
            "a failure is not remembered as fresh"
        );
    }

    #[test]
    fn syntax_plugins_build_missing_compiler_and_no_sources() {
        let dir = copy_of_cmake();
        let tools = Toolchain {
            cc: "jed-no-such-compiler".into(),
            cxx: "jed-no-such-compiler".into(),
        };
        let b = build(dir.path(), &tools, false);
        let Outcome::Failed(why) = &b.outcome else {
            panic!("{:?}", b.outcome)
        };
        assert!(why.contains("cannot run `jed-no-such-compiler`"), "{why}");
        assert!(!dir.path().join(LIBRARY).exists());
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(build(empty.path(), &tools, false).outcome, Outcome::NoSources);
    }

    /// A `scanner.cc` is compiled with the C++ compiler, `parser.c` with the C one.
    #[test]
    fn syntax_plugins_build_cpp_scanner_command_lines() {
        if !have_cc() {
            return;
        }
        let dir = copy_of_cmake();
        let p = dir.path();
        std::fs::rename(p.join("src/scanner.c"), p.join("src/scanner.cc")).unwrap();
        // The scanner is C, so a C++ compiler may refuse it; the command lines are what is checked.
        let tools = Toolchain {
            cc: Toolchain::from_env().cc,
            cxx: "jed-no-such-cxx".into(),
        };
        let b = build(p, &tools, false);
        assert!(b.log[0].contains("-c -fPIC -O2 -I src src/parser.c -o"), "{:?}", b.log);
        assert!(b.log[1].starts_with("$ jed-no-such-cxx -c"), "{:?}", b.log);
        assert!(matches!(b.outcome, Outcome::Failed(_)));
        // The objects directory and the temp file are cleaned up after the failure.
        let left: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            left.iter().all(|n| !n.starts_with(".objects") && !n.ends_with(".tmp")),
            "{left:?}"
        );
        // With both a `scanner.c` and a `scanner.cc`, the C++ one is built.
        std::fs::copy(p.join("src/scanner.cc"), p.join("src/scanner.c")).unwrap();
        let both = build(p, &tools, false);
        assert!(both.log[1].contains("src/scanner.cc"), "{:?}", both.log);
    }
}
