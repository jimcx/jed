//! Milestone 1 "done when": a 1 MB CRLF file opens, edits, undoes/redoes and
//! saves back byte-identical, with no lag.
//!
//! Timings are asserted in release builds only:
//! `cargo test --release milestone -- --nocapture`

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::buffer::file::Eol;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, Motion};
use crate::edit;
use crate::selection::{Selection, Selections};
use crate::ui;

const KEYSTROKE_BUDGET: Duration = Duration::from_millis(2);
const FRAME_BUDGET: Duration = Duration::from_millis(16);

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
    let mut app = App::new(buf, true);
    let s = app.settings.clone();
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();

    // Edits spread through the file; each is followed by a frame, like real typing.
    let mut edit_time = Duration::ZERO;
    let mut frame_time = Duration::ZERO;
    let mut n = 0;
    let len = app.buffer.rope.len();
    for k in 0..50 {
        let at = app.buffer.rope.floor_char_boundary(len / 50 * k);
        app.view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
        app.buffer.seal_undo();
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
            edit::run(cmd, &mut app.buffer, &mut app.view, &s);
            edit_time += t.elapsed();
            n += 1;
        }
        for c in "x = 1;".chars() {
            let t = Instant::now();
            edit::insert_char(&mut app.buffer, &mut app.view, c);
            edit_time += t.elapsed();
            n += 1;
            let t = Instant::now();
            term.draw(|f| ui::render(f, &mut app)).unwrap();
            frame_time += t.elapsed();
        }
    }
    report("keystrokes", edit_time, n, KEYSTROKE_BUDGET);
    report("frames", frame_time, 50 * 6, FRAME_BUDGET);
    let edited = app.buffer.rope.to_string();
    assert_ne!(edited.as_bytes(), &original[..]);
    assert!(app.buffer.is_dirty());

    // Undo everything, redo everything, undo again.
    let t = Instant::now();
    while app.buffer.undo().is_some() {}
    assert_eq!(app.buffer.rope.to_string().as_bytes(), &original[..]);
    assert!(!app.buffer.is_dirty());
    while app.buffer.redo().is_some() {}
    assert_eq!(app.buffer.rope.to_string(), edited);
    while app.buffer.undo().is_some() {}
    println!("undo/redo all: {:?}", t.elapsed());

    // Save back through the real save path.
    let job = app.buffer.begin_save();
    let t = Instant::now();
    job.run().unwrap();
    println!("save: {:?}", t.elapsed());
    app.buffer.finish_save(job.state, true);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "saved file must be byte-identical"
    );
}

#[test]
fn renders_text_gutter_and_status() {
    let mut app = App::new(Buffer::from_str("hello\r\n\tworld\r\n"), true);
    let mut term = Terminal::new(TestBackend::new(60, 5)).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let screen: Vec<String> = (0..5)
        .map(|y| {
            (0..60)
                .map(|x| term.backend().buffer()[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    assert!(screen[0].starts_with("   1 hello"), "{screen:?}");
    assert!(screen[1].starts_with("   2     world"), "{screen:?}");
    assert!(screen[4].contains("Ln 1, Col 1"), "{screen:?}");
    assert!(screen[4].contains("CRLF"), "{screen:?}");
}
