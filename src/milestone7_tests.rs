//! Milestone 7 end to end (7a, the engine): searches started through `App::start_search` run in
//! the background and stream into `app.search` (files in path order, line text and match ranges,
//! the total); `.gitignore`, `.workspace` excludes, `.red/` and binary files are skipped; an
//! unsaved edit typed into an open buffer shows up in the results (and the disk version
//! doesn't); a new search replaces a running one and its stale results are dropped; an empty
//! pattern finishes at once and an invalid regex returns its error.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::app::App;
use crate::buffer::Buffer;
use crate::milestone_tests::test_app;
use crate::search::Options;
use crate::workspace_search::{Query, Results};

fn write(root: &Path, rel: &str, text: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A small workspace, with `a.txt` open.
fn open_app(dir: &tempfile::TempDir) -> (App, PathBuf) {
    let root = crate::workspace::normalize(dir.path());
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root, ".workspace", "[files]\nexclude = [\"build/\"]\n");
    write(&root, ".gitignore", "target/\n");
    write(&root, "a.txt", "first line\nsecond needle\n");
    write(&root, "src/b.rs", "fn needle() {}\r\n// NEEDLE\r\n");
    write(&root, "src/c.rs", "nothing here\n");
    write(&root, "target/out.txt", "needle\n");
    write(&root, "build/gen.txt", "needle\n");
    write(&root, ".red/history/x", "needle\n");
    write(&root, "img.bin", b"needle\0\x01\x02");
    let buffer = Buffer::open(&root.join("a.txt")).unwrap();
    let mut app = test_app(&root, buffer);
    app.test_start();
    (app, root)
}

fn query(pattern: &str, regex: bool, case_sensitive: bool) -> Query {
    Query {
        pattern: pattern.to_string(),
        options: Options {
            regex,
            case_sensitive,
            whole_word: false,
        },
    }
}

async fn search(app: &mut App, q: Query) -> &Results {
    app.start_search(q).unwrap();
    assert!(app.settle(|a| a.search.as_ref().is_some_and(|r| r.done)).await);
    app.search.as_ref().unwrap()
}

/// `path:line:start-end text` per matching line, line 1-based.
fn lines(r: &Results) -> Vec<String> {
    let mut out = Vec::new();
    for f in &r.files {
        for l in &f.lines {
            let ranges: Vec<_> = l.ranges.iter().map(|m| format!("{}-{}", m.start, m.end)).collect();
            out.push(format!(
                "{}:{}:{} {}",
                f.path.display(),
                l.line + 1,
                ranges.join(","),
                l.text
            ));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_7_search_streams_results_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let r = search(&mut app, query("needle", false, false)).await;
    assert_eq!(
        lines(r),
        vec![
            "a.txt:2:7-13 second needle",
            "src/b.rs:1:3-9 fn needle() {}",
            "src/b.rs:2:3-9 // NEEDLE",
        ]
    );
    assert_eq!((r.total, r.truncated), (3, false));
    assert_eq!(r.files[1].count(), 2);

    // Plain text, case-insensitive by default.
    let plain = Query {
        pattern: "needle".into(),
        ..Query::default()
    };
    assert_eq!(search(&mut app, plain).await.total, 3);

    let r = search(&mut app, query("NEEDLE", false, true)).await;
    assert_eq!(lines(r), vec!["src/b.rs:2:3-9 // NEEDLE"]);

    let r = search(&mut app, query(r"fn \w+\(\)", true, true)).await;
    assert_eq!(lines(r), vec!["src/b.rs:1:0-11 fn needle() {}"]);

    // An empty pattern is done at once; an invalid regex is an error and clears the results.
    app.start_search(query("", false, false)).unwrap();
    let r = app.search.as_ref().unwrap();
    assert!(r.done && r.files.is_empty());
    let err = app.start_search(query("(needle", true, false)).unwrap_err();
    assert_eq!(err.to_string(), "unclosed group");
    assert!(app.search.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_7_unsaved_edit_shows_up_in_the_results() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir);
    // Type a match into the first line of the open a.txt, and remove the one on disk's line 2.
    app.press(K::End, M::NONE);
    app.type_text(" haystack");
    app.press(K::Down, M::NONE);
    app.press(K::End, M::NONE);
    for _ in 0.."needle".len() {
        app.press(K::Backspace, M::NONE);
    }
    assert!(app.active_buffer().unwrap().is_dirty());
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "first line\nsecond needle\n"
    );

    let r = search(&mut app, query("haystack", false, false)).await;
    assert_eq!(lines(r), vec!["a.txt:1:11-19 first line haystack"]);
    let r = search(&mut app, query("needle", false, false)).await;
    assert_eq!(
        lines(r),
        vec!["src/b.rs:1:3-9 fn needle() {}", "src/b.rs:2:3-9 // NEEDLE"],
        "the buffer's text, not the disk's"
    );

    // Saved, it's searched from the disk again: the same results.
    app.press(K::Char('s'), cmd());
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);
    let r = search(&mut app, query("haystack", false, false)).await;
    assert_eq!(lines(r), vec!["a.txt:1:11-19 first line haystack"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_7_a_new_search_replaces_the_running_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir);
    for i in 0..300 {
        write(&root, &format!("many/f{i:03}.txt"), "old old old\n");
    }
    app.start_search(query("old", false, false)).unwrap();
    let first = app.search.as_ref().unwrap().generation;
    let r = search(&mut app, query("needle", false, false)).await;
    assert!(r.generation > first);
    assert_eq!(r.query.pattern, "needle");
    assert_eq!(r.total, 3, "no results of the old search");
    assert!(r.files.iter().all(|f| !f.path.starts_with("many")));
}

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}
