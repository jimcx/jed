//! Milestone 4 end to end: copy, cut and paste through the real `App`, with an
//! in-memory clipboard. Bracketed paste and the Linux Ctrl+V both go through it.

use std::fs;
use std::path::Path;

use crossterm::event::{Event, KeyCode as K, KeyModifiers as M};

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::selection::{Selection, Selections};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn open_app(root: &Path, file: &str, contents: &str) -> App {
    fs::write(root.join(file), contents).unwrap();
    let buffer = Buffer::open(&root.join(file)).unwrap();
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.test_start();
    app
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn head(app: &App) -> usize {
    app.group().active_tab().unwrap().view.selections.primary().head.0
}

fn select(app: &mut App, anchor: usize, head: usize) {
    app.active_mut().unwrap().1.selections = Selections::single(Selection::new(ByteIdx(anchor), ByteIdx(head)));
}

/// What the terminal sends for Cmd+V (macOS) or Ctrl+Shift+V (Linux): the clipboard as a bracketed paste.
fn terminal_paste(app: &mut App) {
    let clip = app.clipboard.read().unwrap();
    app.handle_test_event(Event::Paste(clip));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_4_copy_cut_paste() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    let mut app = open_app(&root, "a.txt", "one\ntwo\nthree\n");

    // Copy a selection; paste it elsewhere through the terminal.
    select(&mut app, 0, 3);
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "one");
    assert_eq!(text(&app), "one\ntwo\nthree\n", "copy doesn't edit");
    select(&mut app, 7, 7);
    terminal_paste(&mut app);
    assert_eq!(text(&app), "one\ntwoone\nthree\n");

    // Text copied in another app pastes as plain text, even if it ends in a newline.
    app.clipboard.write("from elsewhere\n".into(), false);
    select(&mut app, 0, 0);
    terminal_paste(&mut app);
    assert_eq!(text(&app), "from elsewhere\none\ntwoone\nthree\n");
    app.press(K::Char('z'), cmd());

    // Empty selection: Cmd+C copies the whole line; pasting puts it above the cursor's line,
    // and the cursor stays where it was in its text.
    select(&mut app, 5, 5); // "t|woone"
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "twoone\n");
    select(&mut app, 13, 13); // "th|ree"
    terminal_paste(&mut app);
    assert_eq!(text(&app), "one\ntwoone\ntwoone\nthree\n");
    assert_eq!(head(&app), 20, "still between 'th' and 'ree'");

    // Empty selection: Cmd+X cuts the whole line in one undo step.
    app.press(K::Char('x'), cmd());
    assert_eq!(text(&app), "one\ntwoone\ntwoone\n");
    assert_eq!(app.clipboard.read().unwrap(), "three\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "one\ntwoone\ntwoone\nthree\n");

    // Cut then paste the line back above another: moves the line.
    select(&mut app, 0, 0);
    app.press(K::Char('x'), cmd());
    select(&mut app, 14, 14); // start of "three"
    terminal_paste(&mut app);
    assert_eq!(text(&app), "twoone\ntwoone\none\nthree\n");

    // A selection cut is one undo step and pastes inline.
    select(&mut app, 0, 3);
    app.press(K::Char('x'), cmd());
    assert_eq!(text(&app), "one\ntwoone\none\nthree\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "twoone\ntwoone\none\nthree\n");

    // Linux keymap: Ctrl+V reads the clipboard itself.
    if !cfg!(target_os = "macos") {
        app.clipboard.write("V".into(), false);
        select(&mut app, 0, 0);
        app.press(K::Char('v'), M::CONTROL);
        assert!(text(&app).starts_with("Vtwoone"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_4_crlf_and_terminal_newlines() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    let mut app = open_app(&root, "a.txt", "ab\r\ncd\r\n");

    // The clipboard gets `\n`; pasting back converts to the file's CRLF.
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "ab\n");
    // Some terminals send newlines as `\r` in a bracketed paste; still a whole-line paste.
    select(&mut app, 5, 5);
    app.handle_test_event(Event::Paste("ab\r".into()));
    assert_eq!(text(&app), "ab\r\nab\r\ncd\r\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_4_read_only_buffer_copies_but_never_cuts_or_pastes() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    let mut app = open_app(&root, "a.txt", "keep\n");
    app.active_mut().unwrap().0.read_only = Some("test".into());

    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "keep\n");
    app.press(K::Char('x'), cmd());
    app.handle_test_event(Event::Paste("x".into()));
    assert_eq!(text(&app), "keep\n");
    assert_eq!(
        app.clipboard.read().unwrap(),
        "keep\n",
        "a refused cut leaves the clipboard alone"
    );
}
