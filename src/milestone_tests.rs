//! Milestone 1 "done when": a 1 MB CRLF file opens, edits, undoes/redoes and
//! saves back byte-identical, with no lag.
//!
//! Timings are asserted in release builds only:
//! `cargo test --release milestone -- --nocapture`

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::file::Eol;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, Motion};
use crate::edit;
use crate::selection::{Selection, Selections};
use crate::ui;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

const KEYSTROKE_BUDGET: Duration = Duration::from_millis(2);
const FRAME_BUDGET: Duration = Duration::from_millis(16);

macro_rules! buf {
    ($app:expr) => {
        $app.active_mut().unwrap().0
    };
}

/// An app on an empty workspace in `dir`, with `buffer` open.
pub fn test_app(dir: &std::path::Path, buffer: Buffer) -> App {
    let ws = Workspace::load(crate::workspace::normalize(dir));
    let trust = TrustStore::load(dir.join("trust.toml"));
    App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true)
}

fn one_mb_crlf() -> Vec<u8> {
    let mut out = String::new();
    let mut i = 0;
    while out.len() < 1_000_000 {
        out.push_str(&format!("\tfn item_{i}() {{ let s = \"héllo 界 {i}\"; }}\r\n"));
        i += 1;
    }
    out.into_bytes()
}

fn report(what: &str, total: Duration, n: u32, budget: Duration) {
    let avg = total / n;
    println!("{what}: {n} in {total:?}, avg {avg:?}");
    if !cfg!(debug_assertions) {
        assert!(avg < budget, "{what} too slow: avg {avg:?} > {budget:?}");
    }
}

#[test]
fn milestone_1_round_trip_one_mb_crlf() {
    let original = one_mb_crlf();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    std::fs::write(&path, &original).unwrap();

    let t = Instant::now();
    let buf = Buffer::open(&path).unwrap();
    println!("open: {:?}", t.elapsed());
    assert_eq!(buf.eol, Eol::Crlf);
    let mut app = test_app(dir.path(), buf);
    let s = app.settings.clone();
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();

    // Edits spread through the file; each is followed by a frame, like real typing.
    let mut edit_time = Duration::ZERO;
    let mut frame_time = Duration::ZERO;
    let mut n = 0;
    let len = buf!(app).rope.len();
    for k in 0..50 {
        let at = buf!(app).rope.floor_char_boundary(len / 50 * k);
        app.active_mut().unwrap().1.selections = Selections::single(Selection::cursor(ByteIdx(at)));
        buf!(app).seal_undo();
        for cmd in [
            Command::Move {
                motion: Motion::LineEnd,
                select: false,
            },
            Command::InsertNewline,
            Command::Move {
                motion: Motion::Up,
                select: false,
            },
            Command::DeleteWordLeft,
        ] {
            let t = Instant::now();
            let (b, v) = app.active_mut().unwrap();
            edit::run(cmd, b, v, &s);
            edit_time += t.elapsed();
            n += 1;
        }
        for c in "x = 1;".chars() {
            let t = Instant::now();
            let (b, v) = app.active_mut().unwrap();
            edit::insert_char(b, v, c, &s);
            edit_time += t.elapsed();
            n += 1;
            let t = Instant::now();
            term.draw(|f| ui::render(f, &mut app)).unwrap();
            frame_time += t.elapsed();
        }
    }
    report("keystrokes", edit_time, n, KEYSTROKE_BUDGET);
    report("frames", frame_time, 50 * 6, FRAME_BUDGET);
    let edited = buf!(app).rope.to_string();
    assert_ne!(edited.as_bytes(), &original[..]);
    assert!(buf!(app).is_dirty());

    // Undo everything, redo everything, undo again.
    let t = Instant::now();
    while buf!(app).undo().is_some() {}
    assert_eq!(buf!(app).rope.to_string().as_bytes(), &original[..]);
    assert!(!buf!(app).is_dirty());
    while buf!(app).redo().is_some() {}
    assert_eq!(buf!(app).rope.to_string(), edited);
    while buf!(app).undo().is_some() {}
    println!("undo/redo all: {:?}", t.elapsed());

    // Save back through the real save path.
    let job = buf!(app).begin_save();
    let t = Instant::now();
    let written = job.run().unwrap();
    println!("save: {:?}", t.elapsed());
    buf!(app).finish_save(job.state, Some(written));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "saved file must be byte-identical"
    );
}

#[test]
fn renders_layout_text_gutter_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app(dir.path(), Buffer::from_str("hello\r\n\tworld\r\n"));
    let mut term = Terminal::new(TestBackend::new(100, 6)).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen: Vec<String> = (0..6)
        .map(|y| {
            (0..100)
                .map(|x| term.backend().buffer()[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    assert!(screen[0].contains("EXPLORER"), "{screen:?}");
    assert!(screen[0].contains(" test.txt ×"), "{screen:?}");
    assert!(screen[1].contains("   1 hello"), "{screen:?}");
    assert!(screen[2].contains("   2     world"), "{screen:?}");
    assert!(screen[5].contains("Ln 1, Col 1"), "{screen:?}");
    assert!(screen[5].contains("CRLF"), "{screen:?}");
}
