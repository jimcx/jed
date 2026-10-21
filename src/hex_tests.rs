//! Hex view (hex-b): the hex tab. A binary file opened through quick open (or the command line)
//! shows in a read-only hex tab whose drawn rows are `hex::row`'s (`hexdump -C`'s when the CLI is
//! there) at a width fitting 16 bytes but not 32, a power of two per row as the tab is resized
//! with the cursor kept in view; the movement / selection keys, Tab between the columns, clicks
//! and drags in either column, the wheel and the scroll bar, Ctrl+G as Go to Offset, the status
//! bar, Cmd+C as hex pairs or ASCII, every command leaving the bytes alone (typing and paste
//! too), splits sharing the bytes, one tab per path, Reopen as Hex / Text, the `(deleted)` mark
//! and files too big for the view; the explorer opening it, its rename followed, the tab moved to
//! another group, closed and quit without a prompt; an empty file and rows narrowed to 1 byte
//! and cut. Find on a hex tab (hex-c): ASCII and hex queries finding the same match, the `0x`
//! toggle (Alt+X / Cmd+Alt+X, or a click) converting the query both ways, case, the errors of
//! an invalid hex query, highlights in both columns (across a row break too), next / previous
//! wrapping, selecting and scrolling, the off-thread count against a naive one, its cap, stale
//! counts dropped, seeding from a selection, no replace / select all / Cmd+D / whole word /
//! regex there, and the text tab's widget as it was. Disk changes, sessions and navigation
//! (hex-d): the file rewritten re-read into every hex tab of it (cursor and top clamped, find's
//! count again), deleted and back, grown past `HEX_MAX_BYTES`; a hex tab kept by the session
//! (a missing file dropped, a file now text still hex, the command line's file merged); back /
//! forward to a hex tab, shown or reopened. `hex_done_when` walks the milestone's "Done when" in
//! order on one app.

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
    let (_d, _root, mut app) = hex_app().await;
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
    quick_open(&mut app, "bin.dat").await;
    assert_eq!(app.groups.active, third);
    assert_eq!(hex(&app).doc.bytes(), data(), "the open tab's bytes");
    assert_eq!(
        shared(&app, third),
        shared(&app, second),
        "the same bytes, not read again"
    );
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

// Find on a hex tab (hex-c).

/// The find widget's toggle keys: Cmd+Alt on macOS, Alt elsewhere.
fn toggle() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER | M::ALT
    } else {
        M::ALT
    }
}

/// Bytes with `ELF` in every case at known places, one across a 16-byte row break (and none on
/// the first two rows, which the find widget covers at the right):
/// `ELF` at 32 (row 2), `elf` at 46 (rows 2 and 3), `Elf` at 64, `ELF` at 80.
fn find_data() -> Vec<u8> {
    let mut b = b"\x7f\x00\x01\x02\x03\x04\x05\x06".to_vec();
    b.extend([0u8; 24]);
    b.extend(b"ELF");
    b.extend([0xffu8; 11]);
    b.extend(b"elf");
    b.extend([0u8; 15]);
    b.extend(b"Elf");
    b.extend([1u8; 13]);
    b.extend(b"ELF");
    b.extend(b" tail");
    b
}

/// An app with `find.bin` (`bytes`) open in a hex tab, and `notes.txt`.
async fn find_app(bytes: &[u8]) -> (tempfile::TempDir, App) {
    let (dir, root, state) = dirs();
    fs::write(root.join("find.bin"), bytes).unwrap();
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
    quick_open(&mut app, "find.bin").await;
    assert!(app.active_hex().is_some());
    (dir, app)
}

/// Cmd+F, then `query` typed over the seeded one.
fn find(app: &mut App, query: &str) {
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.ui.focus, Focus::Find);
    app.press(KeyCode::Backspace, M::NONE);
    app.type_text(query);
}

/// Waits for the current query's count.
async fn settle_count(app: &mut App) {
    assert!(app.settle(|a| a.find.hex.count().is_some()).await, "the count arrives");
}

/// The find widget's rows as drawn.
fn widget_rows(app: &App, term: &Terminal<TestBackend>) -> Vec<String> {
    let hit = app.hits.find.expect("the find widget is drawn");
    (0..hit.area.height)
        .map(|i| {
            line(term, hit.area.x, hit.area.y + i, hit.area.width)
                .trim_start()
                .to_string()
        })
        .collect()
}

/// The background of byte `at`'s hex digits and of its ASCII char, as drawn (on screen).
fn byte_bgs(app: &App, term: &Terminal<TestBackend>, at: usize) -> (ratatui::style::Color, ratatui::style::Color) {
    let h = hex(app);
    let rect = rows_rect(app);
    let n = h.layout.bytes_per_row();
    let y = rect.y + (at / n - h.top_row()) as u16;
    let buf = term.backend().buffer();
    let x = |col: usize| rect.x + col as u16;
    (
        buf[(x(h.layout.hex_col(at % n)), y)].bg,
        buf[(x(h.layout.ascii_col(at % n)), y)].bg,
    )
}

fn bg(app: &App, key: &str) -> ratatui::style::Color {
    app.theme.get(key).bg.expect(key)
}

/// Overlapping matches of `needle` in `bytes`, ASCII letters folded unless `case_sensitive`.
fn naive_count(bytes: &[u8], needle: &[u8], case_sensitive: bool) -> usize {
    bytes
        .windows(needle.len())
        .filter(|w| {
            if case_sensitive {
                *w == needle
            } else {
                w.eq_ignore_ascii_case(needle)
            }
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_ascii_and_hex_find_the_same_match_and_the_toggle_converts() {
    let (_d, mut app) = find_app(&find_data()).await;
    draw(&mut app, 100, 20);
    find(&mut app, "ELF");
    assert!(!app.find_options.hex && !app.find_options.case_sensitive);
    assert_eq!(
        hex(&app).selection(),
        Some(32..35),
        "incremental: the first match from the cursor"
    );
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert_eq!(rows.len(), 1, "no replace row, no error: {rows:#?}");
    assert!(
        rows[0].contains("Aa ab .* 0x") && rows[0].ends_with("1 of 4"),
        "{rows:#?}"
    );

    // Alt+X: the same bytes as hex pairs, the same match; and back.
    app.press(KeyCode::Char('x'), toggle());
    assert!(app.find_options.hex);
    assert_eq!(app.find.query.text, "45 4c 46");
    assert_eq!(hex(&app).selection(), Some(32..35));
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("1 of 2"), "hex is exact");
    app.press(KeyCode::Char('x'), toggle());
    assert!(!app.find_options.hex);
    assert_eq!(app.find.query.text, "ELF");

    // A hex query typed (spaces ignored, either case) finds what the ASCII one does.
    app.press(KeyCode::Char('x'), toggle());
    find(&mut app, "454C46");
    assert_eq!(app.find.query.text, "454C46");
    assert_eq!(hex(&app).selection(), Some(32..35));
    // Bytes that aren't all printable stay as they are when going to ASCII (re-read as text).
    find(&mut app, "00 ff");
    app.press(KeyCode::Char('x'), toggle());
    assert!(!app.find_options.hex);
    assert_eq!(app.find.query.text, "00 ff", "kept, read as text");
    assert!(app.find.hex.error().is_none());

    // ASCII text always converts to hex: its bytes.
    app.press(KeyCode::Char('x'), toggle());
    assert_eq!(app.find.query.text, "30 30 20 66 66");

    // The mode is kept for the session: closing and reopening the widget keeps it (seeded
    // from the selection as pairs: the NUL byte `00` selected while typing `00 ff`).
    app.press(KeyCode::Esc, M::NONE);
    assert!(!app.find.visible);
    app.press(KeyCode::Char('f'), cmd());
    assert!(app.find_options.hex);
    assert_eq!(app.find.query.text, "00");
    // From the editor (widget closed) the toggle still flips the mode and converts the query
    // when it can (a NUL isn't printable: kept).
    app.press(KeyCode::Esc, M::NONE);
    find(&mut app, "45 4c 46");
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Char('x'), toggle());
    assert!(!app.find_options.hex && !app.find.visible);
    assert_eq!(app.find.query.text, "ELF");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_case_sensitivity_and_errors() {
    let data = find_data();
    let (_d, mut app) = find_app(&data).await;
    draw(&mut app, 100, 20);
    find(&mut app, "elf");
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), naive_count(&data, b"elf", false));
    assert_eq!(hex(&app).selection(), Some(32..35), "`ELF` matches `elf` without Aa");

    // Aa: ASCII letters must match exactly.
    app.press(KeyCode::Char('c'), toggle());
    assert!(app.find_options.case_sensitive);
    assert_eq!(hex(&app).selection(), Some(46..49), "researched from the selection");
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), 1);
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("1 of 1"));
    app.press(KeyCode::Char('c'), toggle());

    // Hex mode: an odd count of digits, or a non-hex char, is the widget's error.
    app.press(KeyCode::Char('x'), toggle());
    find(&mut app, "45 4");
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert_eq!(rows[1], "hex bytes come in pairs");
    assert!(
        !rows[0].contains(" of ") && !rows[0].contains("No results"),
        "no count: {rows:#?}"
    );
    // Drawn in the error style, as an invalid regex is.
    let hit = app.hits.find.unwrap();
    let cell = &term.backend().buffer()[(hit.area.x + 1, hit.area.y + 1)];
    assert_eq!(Some(cell.fg), app.theme.get("dialog.error").fg);
    find(&mut app, "4g");
    let term = draw(&mut app, 100, 20);
    assert_eq!(widget_rows(&app, &term)[1], "not a hex digit: g");
    let before = hex(&app).selection();
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).selection(), before, "an invalid query finds nothing");
    // Fixed: the error goes; a query with no match says so.
    find(&mut app, "aa bb");
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ends_with("No results"), "{rows:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_highlights_both_columns_across_a_row_break() {
    let (_d, mut app) = find_app(&find_data()).await;
    draw(&mut app, 100, 20);
    find(&mut app, "elf");
    assert_eq!(hex(&app).selection(), Some(32..35));
    let term = draw(&mut app, 100, 20);
    assert_eq!(hex(&app).layout.bytes_per_row(), 16);
    let current = bg(&app, "editor.findMatch");
    let other = bg(&app, "editor.findMatchHighlight");
    // The selected match (its last byte has the cursor: not checked).
    for at in [32, 33] {
        assert_eq!(byte_bgs(&app, &term, at), (current, current), "byte {at}");
    }
    // The others, in both columns; 46..49 runs over the break between rows 2 and 3.
    for at in [46, 47, 48, 64, 65, 66, 80, 82] {
        assert_eq!(byte_bgs(&app, &term, at), (other, other), "byte {at}");
    }
    for at in [31, 35, 45, 49, 63, 67] {
        let (h, a) = byte_bgs(&app, &term, at);
        assert!(
            ![current, other].contains(&h) && ![current, other].contains(&a),
            "byte {at}"
        );
    }

    // Enter: the next one is current, the first one isn't any more.
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).selection(), Some(46..49));
    let term = draw(&mut app, 100, 20);
    assert_eq!(byte_bgs(&app, &term, 46), (current, current));
    assert_eq!(byte_bgs(&app, &term, 32), (other, other));

    // Closed, nothing is highlighted.
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Esc, M::NONE);
    let term = draw(&mut app, 100, 20);
    for at in [32, 48, 64] {
        let (h, a) = byte_bgs(&app, &term, at);
        assert!(
            ![current, other].contains(&h) && ![current, other].contains(&a),
            "byte {at}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_next_and_previous_wrap_select_and_scroll() {
    // `ELF` at 32 and near the end of a file of 4096 rows.
    let mut data = find_data();
    data.resize(0x10000, 0);
    data.extend(b"ELF");
    data.extend([0u8; 20]);
    let (_d, mut app) = find_app(&data).await;
    draw(&mut app, 100, 20);
    find(&mut app, "ELF");
    app.press(KeyCode::Char('c'), toggle());
    assert_eq!(hex(&app).selection(), Some(32..35));
    let sel = |app: &App| (hex(app).selection(), hex(app).cursor);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(sel(&app), (Some(80..83), 82), "the cursor on the match's last byte");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(sel(&app), (Some(0x10000..0x10003), 0x10002));
    let term = draw(&mut app, 100, 20);
    let h = hex(&app);
    assert!(h.top_row() > 0, "scrolled to the match");
    assert!((h.top_row()..h.top_row() + h.height).contains(&h.layout.row_of(0x10000)));
    assert_eq!(byte_bgs(&app, &term, 0x10000).0, bg(&app, "editor.findMatch"));
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("3 of 3"));
    // Wrapping forward, and back.
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(sel(&app).0, Some(32..35));
    draw(&mut app, 100, 20);
    assert_eq!(hex(&app).top_row(), 2, "scrolled back up to the match's row");
    app.press(KeyCode::Enter, M::SHIFT);
    assert_eq!(sel(&app).0, Some(0x10000..0x10003), "Shift+Enter wraps back");
    app.press(KeyCode::Enter, M::SHIFT);
    assert_eq!(sel(&app).0, Some(80..83));

    // F3 / Shift+F3 from the editor (the widget closed: F3 shows it, keeping the focus).
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.press(KeyCode::F(3), M::NONE);
    assert!(app.find.visible && app.ui.focus == Focus::Editor);
    assert_eq!(sel(&app).0, Some(0x10000..0x10003));
    app.press(KeyCode::F(3), M::SHIFT);
    assert_eq!(sel(&app).0, Some(80..83));

    // Without a selection, next starts at the cursor's byte.
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(hex(&app).selection(), None);
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("32");
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::F(3), M::NONE);
    assert_eq!(sel(&app).0, Some(32..35));
    // Overlapping matches are each a step.
    app.press(KeyCode::Char('x'), toggle());
    find(&mut app, "00 00");
    let start = hex(&app).selection().unwrap().start;
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).selection(), Some(start + 1..start + 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_count_is_naive_and_off_thread_and_stale_counts_are_dropped() {
    // A few hundred KB of a small alphabet: many matches, many overlapping, mixed case.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut data = vec![0u8];
    data.extend((0..300_000).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        b"aAb\x00"[(seed % 4) as usize]
    }));
    let (_d, mut app) = find_app(&data).await;
    draw(&mut app, 100, 20);
    find(&mut app, "ab");
    // "…" until the count arrives (it runs off the UI thread and comes back as an event, which
    // nothing has handled yet).
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with('…'));
    settle_count(&mut app).await;
    let want = naive_count(&data, b"ab", false);
    assert!(want > 1000 && want < hex::HEX_MAX_COUNT);
    assert_eq!(app.find.hex.count().unwrap().found(), want);
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with(&format!("1 of {want}")));
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with(&format!("3 of {want}")));
    app.press(KeyCode::Char('c'), toggle());
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), naive_count(&data, b"ab", true));
    app.press(KeyCode::Char('x'), toggle());
    assert_eq!(app.find.query.text, "61 62");
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), naive_count(&data, b"ab", true));

    // A count for an older query, or another tab, is dropped.
    let tab = hex(&app).id;
    let generation = app.find.hex.generation();
    let fake = || hex::Count {
        starts: vec![1, 2, 3],
        capped: false,
    };
    app.test_hex_count(tab, generation - 1, fake());
    app.test_hex_count(tab + 1000, generation, fake());
    let want = naive_count(&data, b"ab", true);
    assert_eq!(app.find.hex.count().unwrap().found(), want, "stale counts dropped");
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with(&format!(" of {want}")));
    // Typing on starts a new generation: "…" again until its own count arrives.
    app.type_text(" 41");
    assert!(app.find.hex.generation() > generation);
    app.test_hex_count(tab, generation, fake());
    let term = draw(&mut app, 100, 20);
    assert!(
        widget_rows(&app, &term)[0].ends_with('…'),
        "the older count isn't shown"
    );
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), naive_count(&data, b"abA", true));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_count_is_capped() {
    let mut data = vec![0u8];
    data.extend(std::iter::repeat_n(b'a', hex::HEX_MAX_COUNT + 20_000));
    let (_d, mut app) = find_app(&data).await;
    draw(&mut app, 100, 20);
    find(&mut app, "a");
    assert_eq!(hex(&app).selection(), Some(1..2));
    settle_count(&mut app).await;
    let c = app.find.hex.count().unwrap();
    assert!(c.capped && c.found() == hex::HEX_MAX_COUNT);
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("1 of 100000+"));
    // Past the cap the index isn't known.
    app.press(KeyCode::Enter, M::SHIFT);
    assert_eq!(
        hex(&app).selection(),
        Some(data.len() - 1..data.len()),
        "wrapped to the last"
    );
    let term = draw(&mut app, 100, 20);
    assert!(
        widget_rows(&app, &term)[0].ends_with("? of 100000+"),
        "{:?}",
        widget_rows(&app, &term)
    );
    assert_cursor_shown_unfocused(&app);
}

/// The cursor's row is on screen (the terminal cursor is in the find input).
fn assert_cursor_shown_unfocused(app: &App) {
    let h = hex(app);
    let row = h.layout.row_of(h.cursor);
    assert!((h.top_row()..h.top_row() + h.height).contains(&row));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_seeds_from_the_selection() {
    let (_d, mut app) = find_app(&find_data()).await;
    draw(&mut app, 100, 20);
    let go = |app: &mut App, offset: &str| {
        app.press(KeyCode::Char('g'), M::CONTROL);
        app.type_text(offset);
        app.press(KeyCode::Enter, M::NONE);
    };
    go(&mut app, "32");
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    assert_eq!(hex(&app).selection(), Some(32..35));
    // ASCII mode: the text.
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.find.query.text, "ELF");
    assert!(app.find.query.all_selected, "selected: typing replaces it");
    assert_eq!(hex(&app).selection(), Some(32..35), "the selection is the match");
    app.press(KeyCode::Esc, M::NONE);
    // Hex mode: the pairs.
    app.press(KeyCode::Char('x'), toggle());
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.find.query.text, "45 4c 46");
    app.press(KeyCode::Esc, M::NONE);
    go(&mut app, "0");
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.find.query.text, "7f 00");
    app.press(KeyCode::Esc, M::NONE);
    // ASCII mode, bytes that aren't printable: the last query stays.
    app.press(KeyCode::Char('x'), toggle());
    assert_eq!(app.find.query.text, "7f 00", "not printable: kept");
    find(&mut app, "tail");
    app.press(KeyCode::Esc, M::NONE);
    go(&mut app, "0");
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.find.query.text, "tail");
    // No selection: the last query too.
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Left, M::NONE);
    app.press(KeyCode::Char('f'), cmd());
    assert_eq!(app.find.query.text, "tail");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_has_no_replace_select_all_or_word_options() {
    let (_d, mut app) = find_app(&find_data()).await;
    draw(&mut app, 100, 20);
    // Cmd+Alt+F / Ctrl+H: the find input only.
    let open_replace = if cfg!(target_os = "macos") {
        (KeyCode::Char('f'), M::SUPER | M::ALT)
    } else {
        (KeyCode::Char('h'), M::CONTROL)
    };
    app.press(open_replace.0, open_replace.1);
    assert!(app.find.visible);
    app.type_text("ELF");
    let sel = hex(&app).selection();
    assert_eq!(sel, Some(32..35));
    let term = draw(&mut app, 100, 20);
    assert_eq!(widget_rows(&app, &term).len(), 1, "no replace row");
    // Tab doesn't go to a replace input; replace keys, Alt+Enter, Cmd+D do nothing.
    app.press(KeyCode::Tab, M::NONE);
    assert_eq!(app.find.field, crate::find::Field::Find);
    app.press(KeyCode::Char('1'), cmd() | M::SHIFT);
    app.press(KeyCode::Enter, cmd() | M::ALT);
    app.press(KeyCode::Enter, M::ALT);
    assert_eq!(app.ui.focus, Focus::Find);
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Char('d'), cmd());
    app.press(KeyCode::Char('l'), cmd() | M::SHIFT);
    assert_eq!(hex(&app).selection(), sel);
    assert_eq!(hex(&app).doc.bytes(), find_data().as_slice());
    // Whole word and regex are drawn disabled and their keys and clicks do nothing here.
    app.press(KeyCode::Char('f'), cmd());
    let options = app.find_options;
    app.press(KeyCode::Char('w'), toggle());
    app.press(KeyCode::Char('r'), toggle());
    let term = draw(&mut app, 100, 20);
    let hit = app.hits.find.unwrap();
    for r in &hit.toggles[1..3] {
        app.mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), r.x, r.y);
    }
    assert_eq!(app.find_options, options);
    let disabled = app.theme.get("find.option.disabled").fg;
    let buf = term.backend().buffer();
    for r in &hit.toggles[1..3] {
        assert_eq!(Some(buf[(r.x, r.y)].fg), disabled);
    }
    // A click on 0x toggles the mode.
    let x = hit.toggles[3];
    assert_eq!(line(&term, x.x, x.y, 2), "0x");
    app.mouse(MouseEventKind::Down(MouseButton::Left), x.x, x.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x.x, x.y);
    assert!(app.find_options.hex);
    assert_eq!(app.find.query.text, "45 4c 46");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_leaves_the_text_tabs_find_as_it_was() {
    let (_d, mut app) = find_app(&find_data()).await;
    quick_open(&mut app, "notes.txt").await;
    app.test_plain_whitespace();
    draw(&mut app, 100, 20);
    app.press(KeyCode::Char('f'), cmd());
    app.type_text("world");
    let sel = |app: &App| {
        let s = *app.group().active_tab().unwrap().view.selections.primary();
        (s.start().0, s.end().0)
    };
    assert_eq!(sel(&app), (6, 11));
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert!(rows[0].contains("Aa ab .*") && !rows[0].contains("0x"), "{rows:#?}");
    assert!(rows[0].ends_with("1 of 1"));
    assert_eq!(app.hits.find.unwrap().toggles[3].width, 0, "no 0x toggle");
    // Alt+X does nothing on a text tab.
    app.press(KeyCode::Char('x'), toggle());
    assert!(!app.find_options.hex);
    assert_eq!(app.find.query.text, "world");
    // Replace still shows there.
    let open_replace = if cfg!(target_os = "macos") {
        (KeyCode::Char('f'), M::SUPER | M::ALT)
    } else {
        (KeyCode::Char('h'), M::CONTROL)
    };
    app.press(open_replace.0, open_replace.1);
    let term = draw(&mut app, 100, 20);
    assert_eq!(widget_rows(&app, &term).len(), 2, "the replace row");

    // Back on the hex tab (Ctrl+PageUp): the same query, searched as bytes; no replace row.
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(app.active_hex().is_some());
    app.press(KeyCode::Char('f'), cmd());
    app.type_text("tail");
    assert_eq!(hex(&app).selection(), Some(84..88));
    let term = draw(&mut app, 100, 20);
    assert_eq!(widget_rows(&app, &term).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_switching_tabs_with_the_widget_open() {
    let (_d, mut app) = find_app(&find_data()).await;
    quick_open(&mut app, "notes.txt").await;
    app.test_plain_whitespace();
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(app.active_hex().is_some());
    draw(&mut app, 100, 20);
    // Hex mode on the hex tab, the widget left open.
    app.press(KeyCode::Char('x'), toggle());
    find(&mut app, "6c");
    assert_eq!(hex(&app).selection(), Some(47..48));
    let hex_tab = hex(&app).id;

    // To the text tab, the widget still open: a text search there (the hex mode doesn't apply),
    // and its replace row is back.
    app.press(KeyCode::PageDown, M::CONTROL);
    assert!(app.active_hex().is_none() && app.find.visible && app.find_options.hex);
    app.press(KeyCode::Char('f'), cmd());
    app.type_text("world");
    let s = *app.group().active_tab().unwrap().view.selections.primary();
    assert_eq!((s.start().0, s.end().0), (6, 11));
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert!(rows[0].ends_with("1 of 1") && !rows[0].contains("0x"), "{rows:#?}");
    app.find.replace_visible = true;
    let term = draw(&mut app, 100, 20);
    assert_eq!(widget_rows(&app, &term).len(), 2, "the replace row");
    // A count for the hex tab arriving now changes nothing drawn here.
    app.test_hex_count(hex_tab, app.find.hex.generation(), hex::Count::default());
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("1 of 1"));

    // Back to the hex tab with the widget open: the query read as hex pairs ("world" isn't
    // hex: the error), no replace row; a valid one counts afresh.
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(app.active_hex().is_some());
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert!(rows[0].starts_with("world"), "{rows:#?}");
    assert_eq!(rows[1], "not a hex digit: w");
    find(&mut app, "4c");
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    let rows = widget_rows(&app, &term);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert!(rows[0].ends_with(" of 2"), "{rows:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_find_empty_file_end_of_file_and_one_byte_rows() {
    // An empty file (reopened as hex): nothing found, nothing breaks.
    let (_d, mut app) = find_app(&find_data()).await;
    as_hex(&mut app, "empty", b"").await;
    draw(&mut app, 100, 20);
    find(&mut app, "a");
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Enter, M::SHIFT);
    assert_eq!(hex(&app).selection(), None);
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("No results"));

    // A query longer than the file finds nothing either.
    as_hex(&mut app, "short", b"ab").await;
    app.ui.focus = Focus::Editor;
    draw(&mut app, 100, 20);
    find(&mut app, "abc");
    assert_eq!(hex(&app).selection(), Some(0..2), "kept from `ab`, as typed");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).selection(), Some(0..2));
    settle_count(&mut app).await;
    let term = draw(&mut app, 100, 20);
    assert!(widget_rows(&app, &term)[0].ends_with("No results"));

    // `l` matches at 33, 47, 65, 81 and 87, the file's last byte: highlighted at every width,
    // down to one byte per row (where the widget no longer fits but the highlights stay).
    let (_d, mut app) = find_app(&find_data()).await;
    let last = find_data().len() - 1;
    assert_eq!(last, 87);
    draw(&mut app, 100, 20);
    find(&mut app, "l");
    assert_eq!(hex(&app).selection(), Some(33..34));
    app.ui.focus = Focus::Editor;
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text("80");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).cursor, 80);
    assert!(app.find.visible);
    app.ui.side_visible = true;
    app.ui.side_width = crate::layout::MIN_SIDE_WIDTH;
    let other = bg(&app, "editor.findMatchHighlight");
    let mut one_per_row = false;
    for width in (30..100).rev() {
        let term = draw(&mut app, width, 14);
        let h = hex(&app);
        let rect = rows_rect(&app);
        one_per_row |= h.layout.bytes_per_row() == 1;
        for at in [81, last] {
            let n = h.layout.bytes_per_row();
            let row = at / n;
            if !(h.top_row()..h.top_row() + h.height).contains(&row)
                || h.layout.ascii_col(at % n) >= rect.width as usize
            {
                continue;
            }
            assert_eq!(byte_bgs(&app, &term, at), (other, other), "width {width}, byte {at}");
        }
        for at in [80, 82, 86] {
            let n = h.layout.bytes_per_row();
            if (h.top_row()..h.top_row() + h.height).contains(&(at / n))
                && h.layout.ascii_col(at % n) < rect.width as usize
            {
                assert_ne!(byte_bgs(&app, &term, at).1, other, "width {width}, byte {at}");
            }
        }
    }
    assert!(one_per_row, "some width fits one byte per row");
}

/// Writes `name` (`bytes`, text to `detect`) and opens it in a hex tab (Reopen as Hex).
async fn as_hex(app: &mut App, name: &str, bytes: &[u8]) {
    fs::write(app.workspace.root.join(name), bytes).unwrap();
    quick_open(app, name).await;
    app.test_execute(Command::ReopenAsHex);
    assert!(
        app.settle(|a| a.active_hex().is_some_and(|h| h.path.ends_with(name)))
            .await
    );
    assert_eq!(hex(app).len(), bytes.len());
}

// Disk changes, sessions and navigation (hex-d).

/// An app on `root` with sessions in `state` (as `main`'s), the side bar hidden, the session's
/// restore waited for.
async fn state_app(root: &Path, state: &Path) -> App {
    let mut app = App::new(
        Workspace::load(root.to_path_buf()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.state_dir = Some(state.to_path_buf());
    app.test_start();
    assert!(app.settle(|a| !a.session.loading()).await, "the session is read");
    app.ui.side_visible = false;
    app.ui.focus = Focus::Editor;
    app
}

/// Quits as `run` does: the command, then `shutdown` (which writes the session).
async fn quit(mut app: App) {
    app.test_execute(Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;
}

fn go_to_offset(app: &mut App, offset: &str) {
    app.press(KeyCode::Char('g'), M::CONTROL);
    app.type_text(offset);
    app.press(KeyCode::Enter, M::NONE);
}

/// The milestone's "Done when", in order on one app: hexdump's rows at a width fitting 16 bytes
/// but not 32, widening and narrowing through powers of two, Go to Offset and the status bar,
/// find in ASCII and hex (the toggle converting, a match across a row break highlighted in both
/// columns of both rows, the count a naive one), Cmd+C's hex pairs, the file rewritten on disk,
/// quit and reopen at the same offset.
#[tokio::test(flavor = "multi_thread")]
async fn hex_done_when() {
    let (_d, root, state) = dirs();
    let path = root.join("bin.dat");
    let bytes = data();
    fs::write(&path, &bytes).unwrap();
    let mut app = state_app(&root, &state).await;
    open_bin(&mut app).await;

    // `hexdump -C` row for row at 100 columns (96 for the rows: 16 bytes fit, 32 don't).
    let dump = hexdump(&path);
    let term = draw(&mut app, 100, 30);
    let rect = rows_rect(&app);
    assert_eq!(hex(&app).layout.bytes_per_row(), 16);
    let start = drawn_rows(&term, rect);
    assert_eq!(start.len(), rect.height as usize);
    app.press_file_end();
    let term = draw(&mut app, 100, 30);
    let end = drawn_rows(&term, rect);
    assert_eq!(
        end.last().unwrap(),
        &format!("{:08x}", bytes.len()),
        "the closing offset row"
    );
    // Every row the tab would draw, scrolled through: the first and last pages as drawn.
    let h = hex(&app);
    let all: Vec<String> = (0..h.rows())
        .map(|r| hex::row(&h.doc, &h.layout, h.layout.row_offset(r, h.len())))
        .collect();
    assert_eq!(all[..start.len()], start[..]);
    assert_eq!(all[all.len() - end.len()..], end[..]);
    if let Some(dump) = &dump {
        assert_eq!(&all, dump, "every row, as hexdump -C prints them");
    }

    // Widening: 32 bytes per row; narrowing: 8, then 4; the cursor's byte shown each time.
    go_to_offset(&mut app, "0x150");
    for (width, per_row) in [(160, 32), (70, 8), (45, 4), (100, 16)] {
        let term = draw(&mut app, width, 12);
        assert_eq!(hex(&app).layout.bytes_per_row(), per_row, "{width} columns");
        assert_eq!(hex(&app).cursor, 0x150);
        assert_cursor_shown(&app, &term);
    }

    // Ctrl+G `0x100`: the cursor there, and the status bar says so.
    go_to_offset(&mut app, "0x100");
    assert_eq!(hex(&app).cursor, 0x100);
    let term = draw(&mut app, 100, 30);
    assert_cursor_shown(&app, &term);
    let status = status_bar(&term);
    assert!(status.contains("Offset 0x100 (256)   0xf8 248"), "{status}");

    // Cmd+F `ELF` (ASCII) and `45 4c 46` (hex) find the same match; the toggle converts.
    app.press_file_start();
    find(&mut app, "ELF");
    assert!(!app.find_options.hex);
    assert_eq!(hex(&app).selection(), Some(1..4));
    settle_count(&mut app).await;
    assert_eq!(
        app.find.hex.count().unwrap().found(),
        naive_count(&bytes, b"ELF", false)
    );
    app.press(KeyCode::Char('x'), toggle());
    assert_eq!(app.find.query.text, "45 4c 46");
    assert_eq!(hex(&app).selection(), Some(1..4));
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), naive_count(&bytes, b"ELF", true));
    app.press(KeyCode::Char('x'), toggle());
    assert_eq!(app.find.query.text, "ELF");

    // `07 08` runs over a row break three times (15, 271, 527): with the third current, the
    // second is highlighted in both columns of both its rows; the count is the naive one.
    app.press(KeyCode::Char('x'), toggle());
    find(&mut app, "07 08");
    assert_eq!(hex(&app).selection(), Some(15..17));
    settle_count(&mut app).await;
    assert_eq!(
        app.find.hex.count().unwrap().found(),
        naive_count(&bytes, &[7, 8], true)
    );
    assert_eq!(naive_count(&bytes, &[7, 8], true), 3);
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(hex(&app).selection(), Some(527..529));
    let term = draw(&mut app, 100, 30);
    let (current, other) = (bg(&app, "editor.findMatch"), bg(&app, "editor.findMatchHighlight"));
    assert_eq!((271 % 16, 272 % 16), (15, 0), "across a row break");
    for at in [271, 272] {
        assert_eq!(byte_bgs(&app, &term, at), (other, other), "byte {at}");
    }
    assert_eq!(byte_bgs(&app, &term, 527), (current, current));
    assert!(
        widget_rows(&app, &term)[0].ends_with("3 of 3"),
        "{:?}",
        widget_rows(&app, &term)
    );

    // Cmd+C on a selection: its hex pairs.
    app.press(KeyCode::Esc, M::NONE);
    assert!(!app.find.visible);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    assert_eq!(hex(&app).selection(), Some(527..531));
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "07 08 09 0a");

    // The file rewritten on disk updates the tab: new bytes, the cursor clamped into them, the
    // find widget's count counted again over them.
    find(&mut app, "07 08");
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), 3);
    let mut new = bytes[..300].to_vec();
    new.extend_from_slice(b"the end");
    fs::write(&path, &new).unwrap();
    let updated = app
        .settle(|a| hex(a).doc.bytes() == new && a.find.hex.count().is_some_and(|c| c.found() == 2))
        .await;
    assert!(updated, "re-read and counted again");
    assert_eq!(hex(&app).cursor, new.len() - 1, "clamped to the last byte");
    app.press(KeyCode::Esc, M::NONE);
    let term = draw(&mut app, 100, 30);
    assert_cursor_shown(&app, &term);
    let rows = drawn_rows(&term, rows_rect(&app));
    assert_eq!(rows.last().unwrap(), &format!("{:08x}", new.len()));

    // Quit and reopen: the hex tab at the same offset, column and scroll.
    go_to_offset(&mut app, "0x123");
    app.press(KeyCode::Tab, M::NONE);
    let term = draw(&mut app, 100, 8);
    let before = drawn_rows(&term, rows_rect(&app));
    let h = hex(&app);
    let (cursor, column, top) = (h.cursor, h.column, h.top);
    assert!(top > 0);
    quit(app).await;
    let mut app = state_app(&root, &state).await;
    assert_eq!(app.session.restored, 1);
    let h = hex(&app);
    assert_eq!(
        (h.path.as_path(), h.cursor, h.column, h.top),
        (path.as_path(), cursor, column, top)
    );
    assert_eq!((cursor, column), (0x123, Column::Ascii));
    let term = draw(&mut app, 100, 8);
    assert_eq!(drawn_rows(&term, rows_rect(&app)), before);
}

/// Every hex tab of `path` (any group).
fn tabs_of<'a>(app: &'a App, path: &'a Path) -> Vec<&'a HexTab> {
    app.groups.hex_tabs().filter(|h| h.path == path).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_disk_changes_reach_every_tab_of_the_file() {
    let (_d, root, mut app) = hex_app().await;
    let path = root.join("bin.dat");
    open_bin(&mut app).await;
    app.test_execute(Command::SplitRight);
    app.press_file_end();
    draw(&mut app, 200, 12);
    assert_eq!(tabs_of(&app, &path).len(), 2);

    // Rewritten: both tabs take the same new bytes; the cursor at the end is clamped.
    // (One check for both: `app::disk`'s `one_hex_check_per_file_for_all_its_tabs`.)
    fs::write(&path, b"\0short").unwrap();
    assert!(
        app.settle(|a| tabs_of(a, &path).iter().all(|h| h.doc.bytes() == b"\0short"))
            .await
    );
    let tabs = tabs_of(&app, &path);
    assert_eq!(tabs[0].doc.bytes().as_ptr(), tabs[1].doc.bytes().as_ptr(), "shared");
    assert_eq!(hex(&app).cursor, 5);
    let term = draw(&mut app, 200, 12);
    assert_cursor_shown(&app, &term);

    // Deleted: `(deleted)`; back with the same bytes: cleared.
    fs::remove_file(&path).unwrap();
    assert!(app.settle(|a| tabs_of(a, &path).iter().all(|h| h.deleted)).await);
    let term = draw(&mut app, 200, 12);
    assert!(line(&term, 0, 0, 200).contains("bin.dat (deleted)"));
    fs::write(&path, b"\0short").unwrap();
    assert!(app.settle(|a| tabs_of(a, &path).iter().all(|h| !h.deleted)).await);
    assert_eq!(hex(&app).doc.bytes(), b"\0short");

    // Text now: still a hex tab (it was asked for), showing the text's bytes.
    fs::write(&path, "plain text\n").unwrap();
    assert!(app.settle(|a| hex(a).doc.bytes() == b"plain text\n").await);
    assert!(app.buffers.find_path(&path).is_none(), "no text buffer");

    // Grown past `HEX_MAX_BYTES` (sparse: never read): a message, the bytes kept.
    let f = fs::File::options().write(true).open(&path).unwrap();
    f.set_len(hex::HEX_MAX_BYTES as u64 + 1).unwrap();
    drop(f);
    let said = app
        .settle(|a| {
            a.message
                .as_ref()
                .is_some_and(|m| m.text.contains("too big for the hex view") && m.text.contains("not reloaded"))
        })
        .await;
    assert!(said, "{:?}", app.message.as_ref().map(|m| &m.text));
    assert!(
        tabs_of(&app, &path)
            .iter()
            .all(|h| h.doc.bytes() == b"plain text\n" && !h.deleted)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_sessions_keep_hex_tabs() {
    let (_d, root, state) = dirs();
    for name in ["a.bin", "b.bin"] {
        fs::write(root.join(name), data()).unwrap();
    }
    let mut app = state_app(&root, &state).await;
    quick_open(&mut app, "a.bin").await;
    go_to_offset(&mut app, "0x300");
    quick_open(&mut app, "b.bin").await;
    as_hex(&mut app, "notes.txt", b"hello\n").await;
    app.press(KeyCode::Right, M::NONE);
    quit(app).await;
    let text = fs::read_to_string(crate::session::file(&state, &root)).unwrap();
    assert_eq!(text.matches("\"kind\": \"hex\"").count(), 3, "{text}");

    // b.bin gone: its tab dropped; a.bin text now: a hex tab still, its cursor clamped.
    fs::remove_file(root.join("b.bin")).unwrap();
    fs::write(root.join("a.bin"), "now text\n").unwrap();
    let app = state_app(&root, &state).await;
    let names: Vec<String> = app.group().tabs.iter().filter_map(|t| Some(t.hex()?.name())).collect();
    assert_eq!(names, ["a.bin", "notes.txt"]);
    assert_eq!(app.group().tabs.len(), 2);
    assert_eq!((hex(&app).name(), hex(&app).cursor), ("notes.txt".to_string(), 1));
    let a = app.group().tabs[0].hex().unwrap();
    assert_eq!((a.doc.bytes(), a.cursor), (&b"now text\n"[..], 8));
    quit(app).await;

    // The command line's binary file, which the session has a hex tab of: one tab, the
    // session's, active.
    fs::write(root.join("a.bin"), data()).unwrap();
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.state_dir = Some(state.clone());
    app.open_initial_hex(root.join("a.bin"), HexDoc::new(data()));
    app.test_start();
    assert!(app.settle(|a| a.session.restored == 1).await);
    let names: Vec<String> = app.group().tabs.iter().filter_map(|t| Some(t.hex()?.name())).collect();
    assert_eq!(names, ["a.bin", "notes.txt"]);
    assert_eq!(hex(&app).name(), "a.bin");
    assert_eq!(hex(&app).cursor, 8, "the session's place (clamped then)");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_back_and_forward_to_a_hex_tab() {
    let (_d, root, mut app) = hex_app().await;
    let path = root.join("bin.dat");
    quick_open(&mut app, "notes.txt").await;
    open_bin(&mut app).await;
    go_to_offset(&mut app, "0x40");
    quick_open(&mut app, "notes.txt").await;
    let entries = app.nav.history.entries();
    let last = entries.last().unwrap();
    assert_eq!((last.path.as_path(), last.pos), (path.as_path(), None), "path only");

    // Back: the hex tab shown, as it was; forward: the text again.
    app.test_execute(Command::NavBack);
    assert_eq!(hex(&app).path, path);
    assert_eq!(hex(&app).cursor, 0x40);
    app.test_execute(Command::NavForward);
    assert!(app.active_buffer().is_some_and(|b| b.name() == "notes.txt"));

    // Closed, back reopens it (read again, as hex).
    app.test_execute(Command::NavBack);
    app.test_execute(Command::CloseTab);
    assert!(app.groups.hex_tabs().next().is_none());
    app.test_execute(Command::NavBack);
    assert!(app.settle(|a| a.active_hex().is_some()).await, "reopened");
    assert_eq!(hex(&app).path, path);
    assert_eq!(hex(&app).doc.bytes(), data());
    app.test_execute(Command::NavForward);
    assert!(app.active_buffer().is_some_and(|b| b.name() == "notes.txt"));
}

/// Hex tabs in two groups, the active group's active tab a hex one: each comes back in its group
/// with its own place, the bytes read once and shared, the same group and tab active.
#[tokio::test(flavor = "multi_thread")]
async fn hex_sessions_restore_hex_tabs_in_splits() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.bin"), data()).unwrap();
    fs::write(root.join("notes.txt"), "hello\n").unwrap();
    let mut app = state_app(&root, &state).await;
    quick_open(&mut app, "a.bin").await;
    go_to_offset(&mut app, "0x10");
    quick_open(&mut app, "notes.txt").await;
    app.test_execute(Command::SplitRight);
    quick_open(&mut app, "a.bin").await;
    go_to_offset(&mut app, "0x200");
    app.press(KeyCode::Tab, M::NONE);
    assert_eq!(app.groups.len(), 2);
    quit(app).await;

    let app = state_app(&root, &state).await;
    assert_eq!(app.session.restored, 1);
    assert_eq!(app.groups.len(), 2);
    let h = hex(&app);
    assert_eq!(
        (h.cursor, h.column),
        (0x200, Column::Ascii),
        "the active group's hex tab"
    );
    let other = app.groups.ids().into_iter().find(|&g| g != app.groups.active).unwrap();
    let g = app.groups.get(other).unwrap();
    assert!(g.active_hex().is_none(), "notes.txt stays active there");
    let first = g.tabs[g.find_hex(&root.join("a.bin")).unwrap()].hex().unwrap();
    assert_eq!((first.cursor, first.column), (0x10, Column::Hex));
    assert_eq!(first.doc.bytes(), data());
    assert_eq!(first.doc.bytes().as_ptr(), h.doc.bytes().as_ptr(), "read once, shared");
}
