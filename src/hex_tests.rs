//! Hex view (hex-b): the hex tab. A binary file opened through quick open (or the command line)
//! shows in a read-only hex tab whose drawn rows are `hex::row`'s (`hexdump -C`'s when the CLI is
//! there) at a width fitting 16 bytes but not 32, a power of two per row as the tab is resized
//! with the cursor kept in view; the movement / selection keys, Tab between the columns, clicks
//! and drags in either column, the wheel and the scroll bar, Ctrl+G as Go to Offset, the status
//! bar, Cmd+C as hex pairs or ASCII, every command leaving the bytes alone (typing and paste
//! too), splits sharing the bytes, one tab per path, Reopen as Hex / Text, the `(deleted)` mark
//! and files too big for the view; the explorer opening it, its rename followed, the tab moved to
//! another group, closed and quit without a prompt; an empty file and rows narrowed to 1 byte
//! and cut. (Find on a hex tab is hex-c; disk changes, sessions and
//! navigation entries hex-d.)

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{Event, KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::command::Command;
use crate::group::Tab;
use crate::hex::{self, Column, HexDoc, HexTab};
use crate::milestone10_tests::dirs;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// `\x7fELF` and a NUL early (binary), then every byte value three times, then some text.
fn data() -> Vec<u8> {
    let mut b = b"\x7fELF\x02\x01\x01\x00".to_vec();
    for _ in 0..3 {
        b.extend(0..=255u8);
    }
    b.extend_from_slice(b"tail text");
    b
}

/// An app on a workspace with `bin.dat` (`data()`) and `notes.txt`, no file open, the side bar
/// hidden (so the editor's width is the terminal's less the activity bar and the scroll bar).
async fn hex_app() -> (tempfile::TempDir, PathBuf, App) {
    let (dir, root, state) = dirs();
    fs::write(root.join("bin.dat"), data()).unwrap();
    fs::write(root.join("notes.txt"), "hello\nworld\n").unwrap();
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.test_start();
    app.ui.side_visible = false;
    app.ui.focus = Focus::Editor;
    (dir, root, app)
}

/// Quick open (Cmd+P): types `name`, Enter, waits for its tab (hex or text).
async fn quick_open(app: &mut App, name: &str) {
    app.press(KeyCode::Char('p'), cmd());
    app.type_text(name);
    let found = app
        .settle(|a| {
            a.palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
        })
        .await;
    assert!(found, "quick open lists {name}");
    app.press(KeyCode::Enter, M::NONE);
    let opened = app
        .settle(|a| {
            a.active_hex().is_some_and(|h| h.name() == name) || a.active_buffer().is_some_and(|b| b.name() == name)
        })
        .await;
    assert!(opened, "{name} opened");
}

async fn open_bin(app: &mut App) {
    quick_open(app, "bin.dat").await;
    assert!(app.active_hex().is_some(), "a binary file opens as hex");
}

fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// The active group's hex rows' rect, as drawn last.
fn rows_rect(app: &App) -> Rect {
    app.hits.group(app.groups.active).unwrap().text
}

fn line(term: &Terminal<TestBackend>, x: u16, y: u16, width: u16) -> String {
    let buf = term.backend().buffer();
    let s: String = (x..x + width).map(|x| buf[(x, y)].symbol()).collect();
    s.trim_end().to_string()
}

/// Each drawn row of the active hex tab's rect.
fn drawn_rows(term: &Terminal<TestBackend>, rect: Rect) -> Vec<String> {
    (0..rect.height)
        .map(|i| line(term, rect.x, rect.y + i, rect.width))
        .filter(|l| !l.is_empty())
        .collect()
}

fn status_bar(term: &Terminal<TestBackend>) -> String {
    let area = term.backend().buffer().area;
    line(term, 0, area.height - 1, area.width)
}

fn hex(app: &App) -> &HexTab {
    app.active_hex().expect("a hex tab")
}

/// The real `hexdump -C -v` of `path`, `None` (with a note) when it isn't installed.
fn hexdump(path: &Path) -> Option<Vec<String>> {
    match std::process::Command::new("hexdump")
        .args(["-C", "-v"])
        .arg(path)
        .env("LC_ALL", "C")
        .output()
    {
        Ok(out) if out.status.success() => Some(
            String::from_utf8(out.stdout)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect(),
        ),
        _ => {
            eprintln!("note: no hexdump CLI, rows checked against hex::row only");
            None
        }
    }
}

/// The screen row drawing the cursor's row, which the drawn rows must include.
fn assert_cursor_shown(app: &App, term: &Terminal<TestBackend>) {
    let h = hex(app);
    let rect = rows_rect(app);
    let row = h.layout.row_of(h.cursor);
    assert!(
        (h.top_row()..h.top_row() + rect.height as usize).contains(&row),
        "cursor row {row} not in view from {}",
        h.top_row()
    );
    let y = rect.y + (row - h.top_row()) as u16;
    let start = row * h.layout.bytes_per_row();
    assert!(line(term, rect.x, y, rect.width).starts_with(&format!("{start:08x}  ")));
    // The terminal cursor on the cursor's hex digits.
    let x = rect.x + h.layout.hex_col(h.cursor - start) as u16;
    assert_eq!(term.backend().cursor_position(), (x, y).into());
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_rows_are_hexdumps_and_resizing_keeps_a_power_of_two() {
    let (_d, root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    assert_eq!(app.group().tabs.len(), 1);
    assert!(app.buffers.find_path(&root.join("bin.dat")).is_none(), "no text buffer");

    // 100 columns: the rows get 96, which fits 16 bytes (78) but not 32 (144).
    let term = draw(&mut app, 100, 30);
    let rect = rows_rect(&app);
    assert_eq!(rect.width, 96);
    let h = hex(&app);
    assert_eq!(h.layout.bytes_per_row(), 16);
    let rows = drawn_rows(&term, rect);
    assert_eq!(rows.len(), rect.height as usize);
    assert_eq!(
        rows[0],
        "00000000  7f 45 4c 46 02 01 01 00  00 01 02 03 04 05 06 07  |.ELF............|"
    );
    for (i, drawn) in rows.iter().enumerate() {
        assert_eq!(drawn, &hex::row(&h.doc, &h.layout, i * 16), "row {i}");
    }
    if let Some(dump) = hexdump(&root.join("bin.dat")) {
        assert_eq!(rows[..], dump[..rows.len()]);
    }
    // The end of the file: its last rows and the end offset, as hexdump ends.
    app.press_file_end();
    let term = draw(&mut app, 100, 30);
    let rows = drawn_rows(&term, rect);
    assert_eq!(rows.last().unwrap(), &format!("{:08x}", data().len()));
    assert_eq!(rows[rows.len() - 2], hex::row(&hex(&app).doc, &hex(&app).layout, 784));
    if let Some(dump) = hexdump(&root.join("bin.dat")) {
        assert_eq!(rows[..], dump[dump.len() - rows.len()..]);
    }

    // Colours: the offsets, a `.` stand-in and its hex, the bars.
    let buf = term.backend().buffer();
    let offset = app.theme.get("hexEditor.offset").fg;
    let dim = app.theme.get("hexEditor.nonPrintable").fg;
    let border = app.theme.get("hexEditor.border").fg;
    let y = rect.y;
    let l = hex(&app).layout;
    assert_eq!(buf[(rect.x, y)].fg, offset.unwrap());
    assert_eq!(buf[(rect.x + l.bar_col() as u16, y)].fg, border.unwrap());

    // Widening gives 32 bytes per row, narrowing 8 then 4 — never another count — the cursor
    // (somewhere in the middle) shown each time.
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("0x200");
    app.press(KeyCode::Enter, M::NONE);
    for (width, per_row) in [
        (160, 32),
        (130, 16),
        (100, 16),
        (70, 8),
        (60, 8),
        (45, 4),
        (100, 16),
        (200, 32),
    ] {
        let term = draw(&mut app, width, 12);
        assert_eq!(hex(&app).layout.bytes_per_row(), per_row, "{width} columns");
        assert_eq!(hex(&app).cursor, 0x200);
        assert_cursor_shown(&app, &term);
        let rect = rows_rect(&app);
        let h = hex(&app);
        for (i, drawn) in drawn_rows(&term, rect).iter().enumerate() {
            let start = (h.top_row() + i) * per_row;
            assert_eq!(
                drawn,
                &hex::row(&h.doc, &h.layout, start.min(h.len())),
                "{width}: row {i}"
            );
        }
    }
    // A non-printable byte (0x00 at 7) dimmed in both columns.
    app.press_file_start();
    let term = draw(&mut app, 100, 12);
    let buf = term.backend().buffer();
    let rect = rows_rect(&app);
    let l = hex(&app).layout;
    assert_eq!(buf[(rect.x + l.hex_col(7) as u16, rect.y)].fg, dim.unwrap());
    assert_eq!(buf[(rect.x + l.ascii_col(7) as u16, rect.y)].fg, dim.unwrap());
    assert_ne!(
        buf[(rect.x + l.ascii_col(1) as u16, rect.y)].fg,
        dim.unwrap(),
        "`E` isn't"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_keys_move_select_and_switch_columns() {
    let (_d, _root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    draw(&mut app, 100, 12);
    let at = |app: &App| (hex(app).cursor, hex(app).selection());
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(at(&app), (17, None));
    app.press(KeyCode::End, M::NONE);
    assert_eq!(at(&app), (31, None));
    app.press(KeyCode::Home, M::NONE);
    assert_eq!(at(&app), (16, None));
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    assert_eq!(at(&app), (18, Some(16..19)));
    app.press(KeyCode::Down, M::SHIFT);
    assert_eq!(at(&app), (34, Some(16..35)));
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(at(&app), (34, None));
    // A page is the rows on screen less one; the view scrolls with it.
    let height = rows_rect(&app).height as usize;
    app.press(KeyCode::PageDown, M::NONE);
    assert_eq!(hex(&app).cursor, 34 + (height - 1) * 16);
    assert_eq!(hex(&app).top_row(), height - 1);
    app.press_file_end();
    assert_eq!(at(&app), (data().len() - 1, None));
    draw(&mut app, 100, 12);
    assert!(hex(&app).top_row() > 0);
    app.press_file_start();
    assert_eq!(at(&app), (0, None));
    app.press(KeyCode::Char('a'), cmd());
    assert_eq!(at(&app), (data().len() - 1, Some(0..data().len())));
    let term = draw(&mut app, 100, 12);
    assert!(status_bar(&term).contains(&format!("{} bytes selected", data().len())));

    // Tab: the other column (where the terminal cursor goes); Shift+Tab back.
    app.press_file_start();
    app.press(KeyCode::Right, M::NONE);
    assert_eq!(hex(&app).column, Column::Hex);
    app.press(KeyCode::Tab, M::NONE);
    assert_eq!(hex(&app).column, Column::Ascii);
    let term = draw(&mut app, 100, 12);
    let rect = rows_rect(&app);
    let l = hex(&app).layout;
    assert_eq!(
        term.backend().cursor_position(),
        (rect.x + l.ascii_col(1) as u16, rect.y).into()
    );
    // The other column's cell is highlighted as a secondary cursor.
    let other = app.theme.get("editor.cursor.secondary").add_modifier;
    let buf = term.backend().buffer();
    assert!(buf[(rect.x + l.hex_col(1) as u16, rect.y)].modifier.contains(other));
    app.press(KeyCode::BackTab, M::SHIFT);
    assert_eq!(hex(&app).column, Column::Hex);
    assert_eq!(hex(&app).cursor, 1, "Tab never moves the cursor");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_copy_takes_the_cursors_column() {
    let (_d, _root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "7f", "the cursor's byte");
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "45 4c 46");
    app.press(KeyCode::Tab, M::NONE);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "ELF");
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "ELF..", "`.` for the rest");
    // Cut doesn't take anything away.
    app.press(KeyCode::Char('x'), cmd());
    assert_eq!(hex(&app).doc.bytes(), data());
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_mouse_clicks_drags_wheel_and_scroll_bar() {
    let (_d, _root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    draw(&mut app, 100, 12);
    let rect = rows_rect(&app);
    let l = hex(&app).layout;
    let cell = |row: u16, col: usize| (rect.x + col as u16, rect.y + row);

    // A click on a byte's second hex digit, then on a byte in the ASCII column.
    let (x, y) = cell(1, l.hex_col(5) + 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!((hex(&app).cursor, hex(&app).column), (21, Column::Hex));
    draw(&mut app, 100, 12);
    let (x, y) = cell(2, l.ascii_col(3));
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(
        (hex(&app).cursor, hex(&app).column, hex(&app).selection()),
        (35, Column::Ascii, None)
    );
    // Shift+Click extends from the cursor.
    draw(&mut app, 100, 12);
    let (x, y) = cell(2, l.ascii_col(8));
    app.mouse_with(MouseEventKind::Down(MouseButton::Left), x, y, M::SHIFT);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(hex(&app).selection(), Some(35..41));
    // A drag from a hex cell to another row selects between them; past the right edge of the
    // rows, to the row's last byte.
    draw(&mut app, 100, 12);
    let (x, y) = cell(0, l.hex_col(2));
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    let (x, y) = cell(3, l.hex_col(4));
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    assert_eq!(hex(&app).selection(), Some(2..53));
    app.mouse(
        MouseEventKind::Drag(MouseButton::Left),
        rect.x + rect.width + 5,
        rect.y + 1,
    );
    app.mouse(
        MouseEventKind::Up(MouseButton::Left),
        rect.x + rect.width + 5,
        rect.y + 1,
    );
    assert_eq!((hex(&app).cursor, hex(&app).selection()), (31, Some(2..32)));
    // Selected cells drawn as the editor's selection, in both columns.
    let term = draw(&mut app, 100, 12);
    let sel = app.theme.get("editor.selection").bg.unwrap();
    let buf = term.backend().buffer();
    assert_eq!(buf[(rect.x + l.hex_col(3) as u16, rect.y)].bg, sel);
    assert_eq!(
        buf[(rect.x + l.hex_col(3) as u16 - 1, rect.y)].bg,
        sel,
        "the gap between bytes too"
    );
    assert_eq!(buf[(rect.x + l.ascii_col(3) as u16, rect.y)].bg, sel);
    assert_ne!(buf[(rect.x + l.hex_col(1) as u16, rect.y)].bg, sel);

    // The wheel scrolls three rows, the cursor staying put.
    app.mouse(MouseEventKind::ScrollDown, rect.x + 5, rect.y + 2);
    assert_eq!((hex(&app).top_row(), hex(&app).cursor), (3, 31));
    app.mouse(MouseEventKind::ScrollUp, rect.x + 5, rect.y + 2);
    assert_eq!(hex(&app).top_row(), 0);

    // The scroll bar: a click low on the track jumps down, dragging the thumb back up.
    draw(&mut app, 100, 12);
    let bar = app.hits.group(app.groups.active).unwrap().scrollbar.unwrap();
    assert!(bar.thumb.is_some(), "more rows than fit");
    let bottom = bar.track.y + bar.track.height - 1;
    app.mouse(MouseEventKind::Down(MouseButton::Left), bar.track.x, bottom);
    let low = hex(&app).top_row();
    assert!(low > 20, "{low}");
    app.mouse(MouseEventKind::Drag(MouseButton::Left), bar.track.x, bar.track.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), bar.track.x, bar.track.y);
    assert_eq!(hex(&app).top_row(), 0);
    assert_eq!(hex(&app).cursor, 31, "the scroll bar moves no cursor");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_go_to_offset_and_the_status_bar() {
    let (_d, _root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    let len = data().len();
    let term = draw(&mut app, 100, 12);
    let status = status_bar(&term);
    assert!(status.contains("bin.dat [read-only]"), "{status}");
    assert!(
        status.ends_with(&format!("Offset 0x0 (0)   0x7f 127   {len} bytes")),
        "{status}"
    );
    assert!(!status.contains("Ln "), "no line, encoding or language: {status}");

    // Ctrl+G is Go to Offset: its hint, hex then decimal, past the end the last byte.
    app.press(KeyCode::Char('g'), M::CONTROL);
    let term = draw(&mut app, 100, 12);
    let screen: Vec<String> = (0..12).map(|y| line(&term, 0, y, 100)).collect();
    assert!(
        screen.iter().any(|l| l.contains("Current offset: 0x0 (0)")),
        "{screen:#?}"
    );
    app.type_text("0x100");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(hex(&app).cursor, 0x100);
    let term = draw(&mut app, 100, 12);
    assert_cursor_shown(&app, &term);
    assert!(status_bar(&term).ends_with(&format!("Offset 0x100 (256)   0xf8 248   {len} bytes")));
    for (query, want) in [("300", 300), ("1f0h", 0x1f0), ("0x99999", len - 1)] {
        app.press(KeyCode::Char('g'), M::CONTROL);
        app.type_text(query);
        app.press(KeyCode::Enter, M::NONE);
        assert_eq!(hex(&app).cursor, want, "{query}");
    }
    // Not an offset: the input stays.
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("zz");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_some());
    app.press(KeyCode::Esc, M::NONE);

    app.press(KeyCode::Left, M::SHIFT);
    let term = draw(&mut app, 100, 12);
    assert!(status_bar(&term).contains("2 bytes selected"), "{}", status_bar(&term));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_editor_commands_leave_the_bytes_alone() {
    let (_d, root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    app.log_dir = root.join("../state/logs");
    app.config_dir = root.join("../state/config");
    let skip = |c: &Command| {
        matches!(
            c,
            Command::Quit
                | Command::CloseTab
                | Command::NextTab
                | Command::PrevTab
                | Command::MoveTabToNextGroup
                | Command::MoveTabToPreviousGroup
                | Command::MoveTabLeft
                | Command::MoveTabRight
                | Command::OpenUserSettings
                | Command::OpenLogFile
                | Command::Tools(_)
        )
    };
    for c in Command::all().into_iter().filter(|c| !skip(c)) {
        app.test_execute(c);
        app.prompt = None;
        app.palette = None;
        while app.groups.len() > 1 {
            app.test_execute(Command::FocusGroup(2));
            app.test_execute(Command::CloseTab);
        }
        let i = app.group().find_hex(&root.join("bin.dat")).expect("still there");
        app.group_mut().active = i;
        app.test_execute(Command::FocusGroup(1));
        draw(&mut app, 150, 24);
        draw(&mut app, 50, 24);
    }
    // Typed text, Enter, Backspace, paste: nothing anywhere.
    app.type_text("abc");
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Backspace, M::NONE);
    app.handle_test_event(Event::Paste("pasted".into()));
    // The pointer resting on the rows: no hover, nothing.
    draw(&mut app, 100, 24);
    let rect = rows_rect(&app);
    app.mouse(MouseEventKind::Moved, rect.x + 20, rect.y + 2);
    app.test_hover_tick(std::time::Instant::now() + crate::hover::DELAY);
    app.mouse(MouseEventKind::Down(MouseButton::Right), rect.x + 20, rect.y + 2);
    app.mouse(MouseEventKind::Down(MouseButton::Middle), rect.x + 20, rect.y + 2);
    assert!(app.hover.popup.is_none());
    assert!(app.settle(|a| !a.buffers.iter().any(|(_, b)| b.save_in_flight())).await);
    assert_eq!(hex(&app).doc.bytes(), data());
    assert_eq!(fs::read(root.join("bin.dat")).unwrap(), data(), "never written");
    assert!(app.buffers.find_path(&root.join("bin.dat")).is_none());
    assert!(app.buffers.dirty().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_splits_share_the_bytes_and_one_tab_per_path() {
    let (_d, root, mut app) = hex_app().await;
    quick_open(&mut app, "notes.txt").await;
    open_bin(&mut app).await;
    app.press(KeyCode::Right, M::NONE);
    // Opening it again shows the same tab.
    quick_open(&mut app, "bin.dat").await;
    assert_eq!(app.group().tabs.len(), 2);
    assert_eq!(hex(&app).cursor, 1);

    // A split copies the tab, cursor and all, sharing the bytes; each moves on its own.
    let first = app.groups.active;
    app.test_execute(Command::SplitRight);
    let second = app.groups.active;
    assert_ne!(second, first);
    assert_eq!(hex(&app).cursor, 1);
    let shared = |app: &App, g| {
        let h = app.groups.get(g).unwrap().active_hex().unwrap();
        h.doc.bytes().as_ptr()
    };
    assert_eq!(shared(&app, first), shared(&app, app.groups.active));
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(hex(&app).cursor, 17);
    assert_eq!(app.groups.get(first).unwrap().active_hex().unwrap().cursor, 1);
    let term = draw(&mut app, 200, 12);
    assert_eq!(app.hits.groups.len(), 2);
    assert_eq!(
        drawn_rows(&term, app.hits.group(first).unwrap().text)[0],
        drawn_rows(&term, rows_rect(&app))[0]
    );

    // Quick open in a group without it: a new tab with the other group's bytes (not read again).
    app.test_execute(Command::FocusGroup(1));
    app.group_mut().active = 0;
    app.test_execute(Command::SplitDown);
    let third = app.groups.active;
    assert!(app.active_buffer().is_some_and(|b| b.name() == "notes.txt"));
    fs::write(root.join("bin.dat"), b"\0changed").unwrap();
    quick_open(&mut app, "bin.dat").await;
    assert_eq!(app.groups.active, third);
    assert_eq!(hex(&app).doc.bytes(), data(), "the open tab's bytes");
    assert_eq!(shared(&app, third), shared(&app, second));
    assert_eq!(hex(&app).cursor, 0, "a new tab");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_reopen_as_hex_and_as_text() {
    let (_d, root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    // A binary file can't be text.
    app.test_execute(Command::ReopenAsText);
    assert!(app.active_hex().is_some());
    assert!(app.message.as_ref().unwrap().text.contains("binary"));

    quick_open(&mut app, "notes.txt").await;
    let notes = root.join("notes.txt");
    let index = app.group().active;
    // Unsaved edits: refused.
    app.type_text("x");
    app.test_execute(Command::ReopenAsHex);
    assert!(app.active_hex().is_none());
    assert!(app.message.as_ref().unwrap().text.contains("unsaved changes"));
    app.press(KeyCode::Char('z'), cmd());
    assert!(!app.active_buffer().unwrap().is_dirty());

    // In place of the text tab, its buffer dropped; the bytes on disk.
    app.test_execute(Command::ReopenAsHex);
    assert!(app.settle(|a| a.active_hex().is_some()).await);
    assert_eq!(app.group().active, index);
    assert_eq!(app.group().tabs.len(), 2);
    assert!(app.buffers.find_path(&notes).is_none());
    let term = draw(&mut app, 100, 12);
    assert_eq!(
        drawn_rows(&term, rows_rect(&app))[0],
        "00000000  68 65 6c 6c 6f 0a 77 6f  72 6c 64 0a              |hello.world.|"
    );
    // Opening it again shows that tab.
    quick_open(&mut app, "notes.txt").await;
    assert!(app.active_hex().is_some());
    assert_eq!(app.group().tabs.len(), 2);

    // And back, in place again.
    app.test_execute(Command::ReopenAsText);
    assert!(app.settle(|a| a.active_buffer().is_some_and(|b| b.path == notes)).await);
    assert_eq!(app.group().active, index);
    assert_eq!(app.group().tabs.len(), 2);
    assert!(app.group().find_hex(&notes).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_tab_title_deleted_mark_and_files_too_big() {
    let (_d, root, mut app) = hex_app().await;
    open_bin(&mut app).await;
    let term = draw(&mut app, 100, 12);
    assert!(
        line(&term, 3, 0, 97).contains(" bin.dat ×"),
        "{}",
        line(&term, 3, 0, 97)
    );
    for h in app.groups.hex_tabs_mut() {
        h.deleted = true;
    }
    let term = draw(&mut app, 100, 12);
    assert!(line(&term, 3, 0, 97).contains("bin.dat (deleted)"));
    assert!(status_bar(&term).contains("bin.dat [deleted]"));

    // Over `HEX_MAX_BYTES` (a sparse file: only its start is read): a message, no tab.
    let big = fs::File::create(root.join("big.bin")).unwrap();
    big.set_len(hex::HEX_MAX_BYTES as u64 + 1).unwrap();
    drop(big);
    app.press(KeyCode::Char('p'), cmd());
    app.type_text("big.bin");
    assert!(
        app.settle(|a| a
            .palette
            .as_ref()
            .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == "big.bin")))
            .await
    );
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text.contains("too big for the hex view")))
            .await
    );
    assert_eq!(app.group().tabs.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_from_the_command_line() {
    let (_d, root, state) = dirs();
    let path = root.join("bin.dat");
    fs::write(&path, data()).unwrap();
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    // As `main` does: the load hands back the bytes it read.
    let bytes = match crate::buffer::Buffer::open(&path) {
        Err(crate::buffer::file::LoadError::Binary(bytes)) => bytes,
        other => panic!("not binary: {:?}", other.map(|b| b.name())),
    };
    assert_eq!(bytes, data());
    app.open_initial_hex(path.clone(), HexDoc::new(bytes));
    app.test_start();
    assert_eq!(app.ui.focus, Focus::Editor);
    let term = draw(&mut app, 130, 12);
    assert_eq!(hex(&app).path, path);
    assert!(drawn_rows(&term, rows_rect(&app))[0].starts_with("00000000  7f 45 4c 46"));
    app.press(KeyCode::Right, M::NONE);
    assert_eq!(hex(&app).cursor, 1);
    assert!(matches!(app.group().tabs[0], Tab::Hex(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_explorer_opens_renames_moves_and_closes_the_tab() {
    let (_d, root, mut app) = hex_app().await;
    let bin = root.join("bin.dat");
    // The explorer: Enter on the file opens it as hex.
    app.press(KeyCode::Char('e'), cmd() | M::SHIFT);
    assert!(app.settle(|a| a.explorer.find(&bin).is_some()).await, "root listed");
    app.explorer.selected = Some(bin.clone());
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.active_hex().is_some()).await, "opened as hex");
    assert_eq!(hex(&app).path, bin);
    assert!(app.buffers.find_path(&bin).is_none());

    // F2 in the explorer: the tab follows the file to its new name.
    let renamed = root.join("renamed.dat");
    app.press(KeyCode::Char('e'), cmd() | M::SHIFT);
    app.explorer.selected = Some(bin.clone());
    app.press(KeyCode::F(2), M::NONE);
    for _ in 0.."bin.dat".len() {
        app.press(KeyCode::Backspace, M::NONE);
    }
    app.type_text("renamed.dat");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a.group().active_hex().is_some_and(|h| h.path == renamed))
            .await
    );
    assert!(renamed.is_file() && !bin.exists());
    let term = draw(&mut app, 120, 12);
    assert!(
        line(&term, 0, 0, 120).contains("renamed.dat"),
        "{}",
        line(&term, 0, 0, 120)
    );
    app.ui.focus = Focus::Editor;
    quick_open(&mut app, "renamed.dat").await;
    assert_eq!(app.group().tabs.len(), 1, "the same tab, not a second");

    // Moved into a new group with its cursor; closed there without asking (never dirty), the
    // group with it; quitting asks nothing.
    quick_open(&mut app, "notes.txt").await;
    quick_open(&mut app, "renamed.dat").await;
    app.press(KeyCode::Right, M::NONE);
    let first = app.groups.active;
    app.test_execute(Command::MoveTabToNextGroup);
    assert_eq!(app.groups.len(), 2);
    assert_ne!(app.groups.active, first);
    assert_eq!(hex(&app).cursor, 1);
    assert!(app.groups.get(first).unwrap().find_hex(&renamed).is_none());
    app.test_execute(Command::CloseTab);
    assert!(app.prompt.is_none());
    assert_eq!(app.groups.len(), 1);
    assert!(app.groups.hex_tabs_mut().next().is_none());
    quick_open(&mut app, "renamed.dat").await;
    app.test_execute(Command::Quit);
    assert!(app.prompt.is_none() && app.quitting());
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_empty_file_and_narrow_rows() {
    let (_d, root, mut app) = hex_app().await;
    fs::write(root.join("empty.txt"), "").unwrap();
    quick_open(&mut app, "empty.txt").await;
    app.test_execute(Command::ReopenAsHex);
    assert!(app.settle(|a| a.active_hex().is_some()).await);
    assert_eq!(hex(&app).len(), 0);
    let term = draw(&mut app, 100, 12);
    assert!(drawn_rows(&term, rows_rect(&app)).is_empty(), "no rows, as hexdump");
    let status = status_bar(&term);
    assert!(status.ends_with("0 bytes") && !status.contains("Offset"), "{status}");
    // Keys, copy, the mouse and Go to Offset find no byte to move to.
    for (key, mods) in [
        (KeyCode::Right, M::NONE),
        (KeyCode::End, M::NONE),
        (KeyCode::PageDown, M::NONE),
        (KeyCode::Down, M::SHIFT),
        (KeyCode::Char('a'), cmd()),
        (KeyCode::Char('c'), cmd()),
        (KeyCode::Tab, M::NONE),
    ] {
        app.press(key, mods);
    }
    assert_eq!((hex(&app).cursor, hex(&app).selection()), (0, None));
    assert_eq!(app.clipboard.read().unwrap(), "", "nothing copied");
    let rect = rows_rect(&app);
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 12, rect.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), rect.x + 30, rect.y + 2);
    app.mouse(MouseEventKind::Up(MouseButton::Left), rect.x + 30, rect.y + 2);
    app.mouse(MouseEventKind::ScrollDown, rect.x + 12, rect.y);
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("5");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!((hex(&app).cursor, hex(&app).top), (0, 0));
    draw(&mut app, 100, 12);
    // Not binary: back to text.
    app.test_execute(Command::ReopenAsText);
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.name() == "empty.txt"))
            .await
    );

    // Narrow tabs (the side bar shown, the terminal narrowed): 2 then 1 byte per row, then rows
    // cut; never a panic, the cursor kept and shown.
    quick_open(&mut app, "bin.dat").await;
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("0x20");
    app.press(KeyCode::Enter, M::NONE);
    app.ui.side_visible = true;
    app.ui.side_width = crate::layout::MIN_SIDE_WIDTH;
    let mut seen = Vec::new();
    let mut one_per_row = None;
    for width in (30..60).rev() {
        let term = draw(&mut app, width, 12);
        let rect = rows_rect(&app);
        let h = hex(&app);
        let per_row = h.layout.bytes_per_row();
        assert_eq!(h.layout, hex::Layout::fit(rect.width as usize, 32, h.len()), "{width}");
        assert_eq!(h.cursor, 0x20);
        let rows = drawn_rows(&term, rect);
        assert_eq!(rows.len(), rect.height as usize, "{width}");
        for (i, drawn) in rows.iter().enumerate() {
            let start = ((h.top_row() + i) * per_row).min(h.len());
            let full = hex::row(&h.doc, &h.layout, start);
            assert!(
                full.starts_with(drawn.as_str()),
                "{width}: row {i}: {drawn:?} of {full:?}"
            );
            if full.len() <= rect.width as usize {
                assert_eq!(drawn, &full, "{width}: row {i}");
            }
        }
        assert_cursor_shown(&app, &term);
        if !seen.contains(&per_row) {
            seen.push(per_row);
        }
        if per_row == 1 && rect.width as usize > h.layout.ascii_col(1) {
            one_per_row = Some(width);
        }
    }
    assert!(seen.contains(&2) && seen.contains(&1), "{seen:?}");
    assert!(
        hex::row(&hex(&app).doc, &hex(&app).layout, 0).len() > rows_rect(&app).width as usize,
        "the narrowest rows are cut"
    );
    // At one byte per row: Down is the next byte, a click on the third row its byte.
    let width = one_per_row.expect("a width fitting one byte per row");
    draw(&mut app, width, 12);
    assert_eq!(hex(&app).layout.bytes_per_row(), 1);
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(hex(&app).cursor, 0x21);
    draw(&mut app, width, 12);
    let rect = rows_rect(&app);
    let top = hex(&app).top;
    let x = rect.x + hex(&app).layout.ascii_col(0) as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, rect.y + 2);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, rect.y + 2);
    assert_eq!((hex(&app).cursor, hex(&app).column), (top + 2, Column::Ascii));
}
