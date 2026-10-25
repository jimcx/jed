//! soft_wrap-a: the `word_wrap` setting (`off` / `on`, default `off`) and "View: Toggle Word
//! Wrap" (`view.toggleWordWrap`, Alt+Z; Cmd+Alt+Z on macOS), which flips the effective mode for
//! the session without writing it to a settings file.
//!
//! soft_wrap-b: the editor draws a wrapped line on screen rows — the line number only on its
//! first row, continuation rows at its indentation, the cursor on the row it lands on, the
//! selection / find match / squiggle on the rows their bytes are drawn on — and wrapping off
//! draws exactly as it did before.
//!
//! soft_wrap-c: the per-line overlays follow the rows: a bracket pair spans the rows its bytes
//! are on, a quick-diff gutter marker and the lightbulb sit on the line's first row (where the
//! line number is), and find matches, squiggles and selections stay on their rows.
//!
//! soft_wrap-d: scrolling over screen rows — the wheel and the PageUp / PageDown scroll move by
//! rows, the scroll bar's top, thumb and git marks are rows, and a split that narrows rewraps
//! with the cursor kept in view.
//!
//! soft_wrap-e: movement over screen rows — Up / Down / PageUp / PageDown move by rows keeping
//! the goal column, Home / End stop on the row's edge before the line's, and every selection
//! moves (the layouts are walked in order at 60k selections).
//!
//! soft_wrap-f: the mouse and popups on screen rows — a click, drag, double / triple click and
//! Alt+Click map the cell through the row drawn there, and the hover / completion / code action
//! popups anchor at the cursor's screen row.
//!
//! Sessions and the 1 MB release budget come in the task's last subtask. The "Done when" parts
//! covered here: the setting is read from `settings.toml` and `.workspace`, defaults to `off`,
//! the toggle flips `App::word_wrap()` and keeps doing so across a `.workspace` reload, the
//! wrapped rows are drawn as the plan says, scrolling / the scroll bar use screen rows, the
//! cursor moves over them and the mouse / popups land on them like VS Code's.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Position;
use ratatui::style::Modifier;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx};
use crate::command::Command;
use crate::config::settings;
use crate::diagnostics::{Diagnostic, Mark, Severity};
use crate::edit::WordWrap;
use crate::git::test_repo::{commit_all, init};
use crate::lsp::ServerId;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{open_app, open_synced, project, put_cursor};
use crate::milestone10_tests::{dirs, open_file_app, settle_markers};
use crate::scrollbar::Scroll;
use crate::selection::{Selection, Selections};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A workspace with `a.txt` open, a user `settings.toml` holding `user` and a `.workspace`
/// holding `workspace` (both as `[settings]` bodies, empty for none).
fn open(dir: &tempfile::TempDir, user: &str, workspace: &str) -> (App, PathBuf, PathBuf) {
    open_text(dir, user, workspace, "one two three\nfour\n")
}

/// `open`, with `a.txt` holding `text`.
fn open_text(dir: &tempfile::TempDir, user: &str, workspace: &str, text: &str) -> (App, PathBuf, PathBuf) {
    let base = crate::workspace::normalize(dir.path());
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    if !user.is_empty() {
        fs::write(config_dir.join(settings::FILE_NAME), format!("[settings]\n{user}")).unwrap();
    }
    let root = base.join("ws");
    fs::create_dir_all(&root).unwrap();
    if !workspace.is_empty() {
        fs::write(root.join(".workspace"), format!("[settings]\n{workspace}")).unwrap();
    }
    fs::write(root.join("a.txt"), text).unwrap();
    let loaded = settings::load(&config_dir.join(settings::FILE_NAME));
    let ws = Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let trust = TrustStore::load(base.join("trust.toml"));
    let initial = InitialFile {
        buffer: Buffer::open(&root.join("a.txt")).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config_dir.clone();
    app.report_errors(loaded.errors);
    app.test_start();
    (app, root, config_dir)
}

/// The app with no side panel (the editor takes the terminal's width) and whitespace drawn as
/// itself, so a drawn row's spaces can be compared.
fn plain_editor(app: &mut App) {
    app.ui.side_visible = false;
    app.test_plain_whitespace();
}

fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// Every drawn row's cells as a string (one char a cell here).
fn cells(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// Where `needle` is drawn.
fn find(rows: &[String], needle: &str) -> Vec<(u16, u16)> {
    let needle: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        for x in 0..=chars.len().saturating_sub(needle.len()) {
            if chars[x..x + needle.len()] == needle[..] {
                out.push((x as u16, y as u16));
            }
        }
    }
    out
}

/// VS Code's toggle: Alt+Z, Option+Command+Z on macOS.
fn toggle_mods() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER | KeyModifiers::ALT
    } else {
        KeyModifiers::ALT
    }
}

#[test]
fn soft_wrap_setting_defaults_off() {
    assert_eq!(settings::parse("").overrides.word_wrap, None);
    let none = settings::SettingsOverrides::default();
    assert_eq!(settings::layer(&none, &none).word_wrap, WordWrap::Off);
}

#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_toggle_flips_the_effective_mode_for_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root, config_dir) = open(&dir, "word_wrap = \"off\"\n", "");
    assert_eq!(app.settings.word_wrap, WordWrap::Off);
    assert_eq!(app.word_wrap(), WordWrap::Off);

    app.press(KeyCode::Char('z'), toggle_mods());
    assert_eq!(app.last_command, Some(Command::ToggleWordWrap));
    assert_eq!(app.settings.word_wrap, WordWrap::Off, "the setting is untouched");
    assert_eq!(app.word_wrap(), WordWrap::On);

    // A `.workspace` reload changes the setting but keeps the session toggle: an `on` setting
    // with the toggle flipped is off.
    fs::write(root.join(".workspace"), "[settings]\nword_wrap = \"on\"\n").unwrap();
    assert!(app.settle(|a| a.settings.word_wrap == WordWrap::On).await);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    app.press(KeyCode::Char('z'), toggle_mods());
    assert_eq!(app.word_wrap(), WordWrap::On);

    // Never written to either file.
    assert_eq!(
        fs::read_to_string(config_dir.join(settings::FILE_NAME)).unwrap(),
        "[settings]\nword_wrap = \"off\"\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".workspace")).unwrap(),
        "[settings]\nword_wrap = \"on\"\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_setting_on_is_toggled_off_and_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open(&dir, "word_wrap = \"on\"\n", "word_wrap = \"on\"\n");
    assert!(app.settle(|a| a.settings.word_wrap == WordWrap::On).await);
    assert_eq!(app.word_wrap(), WordWrap::On);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::On);
}

/// The key that opens the find widget (Cmd+F on macOS, Ctrl+F elsewhere).
fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// A drawn cell's background (`None` for the terminal's default).
fn bg(term: &Terminal<TestBackend>, x: u16, y: u16) -> Option<ratatui::style::Color> {
    term.backend().buffer()[(x, y)].bg.into()
}

/// A wrapped line's rows: the text on the rows (the continuation at the line's indentation),
/// the line number on the first row only, and the cursor on the row it lands on.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_draws_text_on_rows_with_the_cursor_there() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open_text(&dir, "", "", "  abcd efgh ijkl mnop qrst\n");
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::On);
    // The cursor at the line's end lands on its continuation row.
    app.press(KeyCode::End, KeyModifiers::NONE);
    let term = draw(&mut app, 30, 8);
    let screen = cells(&term);
    let start = find(&screen, "abcd");
    let continuation = find(&screen, "qrst");
    assert_eq!(start.len(), 1, "the line's first row: {screen:?}");
    assert_eq!(continuation.len(), 1, "the line's continuation row: {screen:?}");
    let ((x0, y0), (x1, y1)) = (start[0], continuation[0]);
    assert_eq!(
        (x1, y1),
        (x0, y0 + 1),
        "the continuation row starts at the line's two-space indentation"
    );
    // The gutter is the five columns left of the text (the line's indentation starts the text).
    let chars: Vec<char> = screen[y0 as usize].chars().collect();
    let cont: Vec<char> = screen[y1 as usize].chars().collect();
    let gutter = (x0 - 2 - 5) as usize;
    assert_eq!(
        &chars[gutter..gutter + 5],
        &[' ', ' ', ' ', '1', ' '],
        "the line's number is on its first row: {screen:?}"
    );
    assert_eq!(
        &cont[gutter..gutter + 5],
        &[' '; 5],
        "the continuation row's gutter is blank"
    );
    assert_eq!(
        term.backend().cursor_position(),
        Position { x: x1 + 4, y: y1 },
        "the cursor at the line's end is on its continuation row"
    );
}

/// With wrapping off a long line is drawn clipped, as before: two frames off are identical, and
/// turning wrapping off again brings the same frame back.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_off_draws_the_long_line_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open_text(&dir, "", "", "  abcd efgh ijkl mnop qrst\n");
    plain_editor(&mut app);
    // A frame that measures the line first reserves the sideways bar's row: settle it.
    draw(&mut app, 30, 8);
    draw(&mut app, 30, 8);
    let off = cells(&draw(&mut app, 30, 8));
    assert!(
        find(&off, "qrst").is_empty(),
        "the line is cut at the right edge: {off:?}"
    );

    app.test_execute(Command::ToggleWordWrap);
    // While wrapping the line fits the view, so the rest of it is visible on the next row.
    let on = cells(&draw(&mut app, 30, 8));
    assert!(
        !find(&on, "qrst").is_empty(),
        "wrapping on shows the line's tail: {on:?}"
    );
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    let back = cells(&draw(&mut app, 30, 8));
    assert_eq!(back, off, "wrapping off draws exactly as it did");
}

/// A selection and a squiggle land on the rows their bytes are drawn on.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_selection_and_squiggle_follow_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open_text(
        &dir,
        "",
        "",
        "one\ntwo\nthree\nfour\nfive\n  abcd efgh ijkl mnop qrst\nlast\n",
    );
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    let (buf, view) = app.active_mut().unwrap();
    // "mnop qrst" is selected across the line's two rows; "mnop" has an error squiggle.
    view.selections = Selections::single(Selection::new(ByteIdx(41), ByteIdx(50)));
    buf.diagnostics.replace(
        ServerId::test(1),
        vec![Mark {
            start: ByteIdx(41),
            end: ByteIdx(45),
            server: ServerId::test(1),
            diag: std::sync::Arc::new(Diagnostic {
                severity: Severity::Error,
                message: "bad".into(),
                source: None,
                code: None,
                raw: serde_json::Value::Null,
            }),
        }],
    );
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let (xm, ym) = find(&screen, "mnop")[0];
    let (xq, yq) = find(&screen, "qrst")[0];
    assert_eq!(yq, ym + 1, "qrst is on the line's continuation row: {screen:?}");
    assert_eq!(
        xq,
        find(&screen, "abcd")[0].0,
        "the continuation row keeps the indentation"
    );
    let selection = app.theme.get("editor.selection").bg;
    for x in xm..xm + 4 {
        assert_eq!(bg(&term, x, ym), selection, "the selected 'mnop' is on the first row");
    }
    for x in xq..xq + 4 {
        assert_eq!(
            bg(&term, x, yq),
            selection,
            "the selected 'qrst' is on the continuation row"
        );
    }
    let squiggle = app.theme.get("editorError.foreground").underline_color;
    for x in xm..xm + 4 {
        let cell = &term.backend().buffer()[(x, ym)];
        assert!(
            cell.modifier.contains(Modifier::UNDERLINED),
            "the squiggle is under 'mnop'"
        );
        assert_eq!(Some(cell.underline_color), squiggle);
    }
    assert!(
        !term.backend().buffer()[(xq, yq)]
            .modifier
            .contains(Modifier::UNDERLINED),
        "the squiggle isn't on the continuation row"
    );
}

/// A find match on a continuation row is highlighted there.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_find_match_on_a_continuation_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open_text(
        &dir,
        "",
        "",
        "one\ntwo\nthree\nfour\nfive\n  abcd efgh ijkl mnop qrst\nlast\n",
    );
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    app.press(KeyCode::Char('f'), cmd());
    app.type_text("qrst");
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    assert_eq!(
        find(&screen, "qrst").len(),
        1,
        "the match on the continuation row: {screen:?}"
    );
    let (x, y) = find(&screen, "qrst")[0];
    // The find widget's current match is the primary selection, drawn as `editor.findMatch`.
    let highlight = app.theme.get("editor.findMatch").bg;
    for i in 0..4 {
        assert_eq!(
            bg(&term, x + i, y),
            highlight,
            "the match is highlighted on the continuation row"
        );
    }
}

/// A bracket pair whose halves are on different rows of one wrapped line: both halves take the
/// bracket highlight on the row their bytes are drawn on.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_bracket_pair_spans_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = open_text(
        &dir,
        "",
        "",
        "one\ntwo\nthree\nfour\nfive\n  call(LONGA aaaa bbbb cccc dddd eeee ffff gggg LONGB) tail\nlast\n",
    );
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    let (buf, view) = app.active_mut().unwrap();
    let at = buf.rope.to_string().find("call(").unwrap() + "call".len();
    view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let (open, close) = (find(&screen, "("), find(&screen, ")"));
    assert_eq!((open.len(), close.len()), (1, 1), "one pair: {screen:?}");
    let ((xo, yo), (xc, yc)) = (open[0], close[0]);
    assert!(yc > yo, "the pair spans the line's rows: {screen:?}");
    assert_eq!(
        yo,
        find(&screen, "LONGA")[0].1,
        "the opening bracket is on the line's first row"
    );
    let bracket = app.theme.get("editor.bracketMatch").bg;
    assert_eq!(bg(&term, xo, yo), bracket, "the opening bracket on its row");
    assert_eq!(
        bg(&term, xc, yc),
        bracket,
        "the closing bracket on its continuation row"
    );
}

/// The quick-diff gutter marker of a changed line that wraps: the glyph is on the line's first
/// row (with its number), never on a continuation row.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_quick_diff_marker_stays_on_the_first_row() {
    let (_d, root, state) = dirs();
    init(&root);
    fs::write(root.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    commit_all(&root, "one");
    fs::write(
        root.join("f.txt"),
        "one\ntwo  LONGA aaaa bbbb cccc dddd eeee ffff gggg LONGB hhhh\nthree\n",
    )
    .unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await, "the gutter markers are current");
    app.ui.side_visible = false;
    app.test_execute(Command::ToggleWordWrap);
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let (_, y0) = find(&screen, "LONGA")[0];
    let (_, y1) = find(&screen, "LONGB")[0];
    assert!(y1 > y0, "the changed line wraps: {screen:?}");
    let text = app.hits.group(app.groups.active).unwrap().text;
    let marker = &term.backend().buffer()[(text.x - 1, y0)];
    assert_eq!(
        marker.symbol(),
        "▎",
        "the marker is in the gutter on the line's first row"
    );
    assert_eq!(
        Some(marker.fg),
        app.theme.get("editorGutter.modifiedBackground").fg,
        "an unstaged modification's colour"
    );
    assert_eq!(
        term.backend().buffer()[(text.x - 1, y1)].symbol(),
        " ",
        "no marker on the continuation row"
    );
}

/// The lightbulb on the cursor's line with the cursor on a continuation row: it sits in the
/// gutter on that line's first row, by the line number.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_lightbulb_on_the_cursor_lines_first_row() {
    let dir = tempfile::tempdir().unwrap();
    // Two earlier lines wrap, so the cursor line's first screen row is below its line index:
    // the bulb must go by the rows, not the line number.
    let source = "one\n  alpha alpha alpha alpha alpha alpha alpha\nbeta beta beta beta beta beta beta beta beta\nfour\nfive\n  call(LONGA aaaa bbbb cccc dddd LONGB eeee ffff)\nlast\n";
    let root = project(dir.path(), &[("a.cpp", source)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    fake.respond(
        "textDocument/codeAction",
        serde_json::json!([{ "title": "Change it", "kind": "quickfix" }]),
    );
    app.ui.side_visible = false;
    app.test_execute(Command::ToggleWordWrap);
    put_cursor(&mut app, source.find("LONGB").unwrap());
    app.press(KeyCode::Right, KeyModifiers::NONE);
    assert!(app.settle(|a| a.bulb_line().is_some()).await, "actions after the rest");
    assert_eq!(app.bulb_line().unwrap().1, 5);
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let (_, y0) = find(&screen, "LONGA")[0];
    let (_, y1) = find(&screen, "LONGB")[0];
    assert!(y1 > y0, "the cursor is on a continuation row: {screen:?}");
    let bulb = find(&screen, crate::code_action::BULB);
    assert_eq!(bulb.len(), 1, "one bulb: {screen:?}");
    let (bx, by) = bulb[0];
    assert_eq!(by, y0, "the bulb is on the cursor line's first row");
    assert_ne!(by, y1, "not on the continuation row the cursor is on");
    assert!(
        bx < app.hits.group(app.groups.active).unwrap().text.x,
        "in the gutter: {screen:?}"
    );
    app.shutdown_servers().await;
}

/// A line long enough to wrap to several rows at a 30-column terminal.
fn long_line(i: usize) -> String {
    format!("line {i} alpha bravo charlie delta echo foxtrot golf hotel\n")
}

/// An app on `text` with the side panel off and wrapping on, drawn at `w`×`h` (which sets the
/// view's width, rows and wrap cache scrolling reads).
fn wrapped_editor(dir: &tempfile::TempDir, text: &str, w: u16, h: u16) -> (App, Terminal<TestBackend>) {
    let (mut app, _, _) = open_text(dir, "", "", text);
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::On);
    let term = draw(&mut app, w, h);
    (app, term)
}

/// The active view's first shown row as one global screen row.
fn global_top(app: &mut App) -> usize {
    let (buf, view) = app.active_mut().unwrap();
    let (revision, width, tab) = (view.revision, view.width, view.tab_width);
    view.wrap.row_of_line(&buf.rope, revision, view.top_line(), width, tab) + view.top_row()
}

/// The active view's first shown (line, row).
fn top(app: &App) -> (usize, usize) {
    let view = &app.group().active_tab().unwrap().view;
    (view.top_line(), view.top_row())
}

/// The active view's primary cursor as a global screen row and its column within that row.
fn cursor_row_col(app: &mut App) -> (usize, usize) {
    let (buf, view) = app.active_mut().unwrap();
    let head = view.selections.primary().head;
    let line = pos::line_of(&buf.rope, head);
    let (revision, width, tab) = (view.revision, view.width, view.tab_width);
    let layout = view.wrap.layout(&buf.rope, revision, line, width, tab);
    let within = ByteIdx(head.0 - pos::line_start(&buf.rope, line).0);
    let (row, col) = layout.row_of(within);
    (
        view.wrap.row_of_line(&buf.rope, revision, line, width, tab) + row,
        col.0,
    )
}

/// The active view's row edges of line 0 as `(start, end)` byte offsets within the text, drawn
/// at the view's width.
fn line0_rows(app: &mut App) -> Vec<(usize, usize)> {
    let (buf, view) = app.active_mut().unwrap();
    let layout = view
        .wrap
        .layout(&buf.rope, view.revision, 0, view.width, view.tab_width);
    (0..layout.len())
        .map(|i| (layout.rows()[i].start.0, layout.row_end(i).0))
        .collect()
}

/// The active view's primary cursor.
fn head(app: &App) -> usize {
    app.group().active_tab().unwrap().view.selections.primary().head.0
}

/// Up / Down move between screen rows at the same column (the goal column), across line breaks
/// too, and Up undoes Down.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_arrows_move_by_rows_keeping_the_column() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..3).map(long_line).collect();
    let (mut app, _) = wrapped_editor(&dir, &text, 30, 10);
    let rows = line0_rows(&mut app);
    assert!(rows.len() >= 3, "the line wraps to several rows");

    // The second row's fifth column.
    put_cursor(&mut app, rows[1].0 + 5);
    let (row, col) = cursor_row_col(&mut app);
    assert_eq!(col, 5);
    app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        cursor_row_col(&mut app),
        (row + 1, 5),
        "one screen row down, same column"
    );
    app.press(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(cursor_row_col(&mut app), (row, 5), "and back up the row");

    // Down off the line's last row lands on the next line's first row.
    put_cursor(&mut app, rows[rows.len() - 1].0);
    app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        cursor_row_col(&mut app),
        (rows.len(), 0),
        "down crosses the line break to the next line's first row"
    );
    app.press(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(
        cursor_row_col(&mut app),
        (rows.len() - 1, 0),
        "up comes back to the line's last row"
    );
}

/// Home / End stop on the wrapped row's edge first, then the line's (VS Code).
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_home_and_end_stop_on_the_row_first() {
    let dir = tempfile::tempdir().unwrap();
    let text = "    alpha bravo charlie delta echo foxtrot golf hotel\n";
    let (mut app, _) = wrapped_editor(&dir, text, 30, 10);
    let rows = line0_rows(&mut app);
    assert!(rows.len() >= 3, "the indented line wraps to several rows");
    let line_end = text.len() - 1;

    put_cursor(&mut app, 0);
    app.press(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(head(&app), rows[0].1, "end goes to the first row's end");
    app.press(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(head(&app), line_end, "then to the line's end, not the next row's");

    put_cursor(&mut app, line_end);
    app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(head(&app), rows[rows.len() - 1].0, "home goes to the last row's start");
    app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(head(&app), 4, "then to the line's first non-blank");
    app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(head(&app), 0, "then to the line's start");
}

/// PageUp / PageDown move the cursor a page of screen rows, and the view scrolls with it.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_page_keys_move_the_cursor_by_rows() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..12).map(long_line).collect();
    let (mut app, _) = wrapped_editor(&dir, &text, 30, 10);
    put_cursor(&mut app, 0);
    let page = app.group().active_tab().unwrap().view.height;
    assert!(page >= 2);
    assert_eq!(cursor_row_col(&mut app), (0, 0));

    app.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(cursor_row_col(&mut app), (page, 0), "a page of rows down");
    assert_eq!(global_top(&mut app), page, "and the view scrolled by the same rows");
    draw(&mut app, 30, 10);
    assert_eq!(
        cursor_row_col(&mut app),
        (page, 0),
        "the frame keeps the cursor on its row"
    );

    app.press(KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(cursor_row_col(&mut app), (0, 0), "a page of rows back up");
    assert_eq!(global_top(&mut app), 0);
}

/// The wheel and PageUp / PageDown scroll screen rows, and a frame keeps the sub-row the view was
/// scrolled to (the cursor's own move is `soft_wrap_page_keys_move_the_cursor_by_rows`).
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_wheel_and_page_keys_scroll_by_screen_rows() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..12).map(long_line).collect();
    let (mut app, _) = wrapped_editor(&dir, &text, 30, 10);
    let text_rect = app.hits.group(app.groups.active).unwrap().text;
    assert_eq!(global_top(&mut app), 0);

    app.mouse(MouseEventKind::ScrollDown, text_rect.x + 1, text_rect.y + 1);
    assert_eq!(
        global_top(&mut app),
        3,
        "the wheel scrolls rows, not lines (which would give line 3)"
    );
    app.mouse(MouseEventKind::ScrollUp, text_rect.x + 1, text_rect.y + 1);
    assert_eq!(global_top(&mut app), 0);

    // PageDown scrolls by the view's rows (the cursor's move is in the page-keys test).
    let page = app.group().active_tab().unwrap().view.height;
    app.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(global_top(&mut app), page, "PageDown scrolls by its rows");
    app.press(KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(global_top(&mut app), 0, "PageUp scrolls back by its rows");
    draw(&mut app, 30, 10); // the frame after PageUp reveals the cursor (to line 0)

    // A frame re-anchors the top line without dropping the row.
    app.mouse(MouseEventKind::ScrollDown, text_rect.x + 1, text_rect.y + 1);
    draw(&mut app, 30, 10);
    assert_eq!(global_top(&mut app), 3, "the frame keeps the first shown row");
}

/// The scroll bar's thumb and drag work over the whole text's rows: a press at the track's bottom
/// lands on the last line's last row, and a frame doesn't reveal the cursor back.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_scroll_bar_counts_rows_and_drags_to_the_last_row() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..12).map(long_line).collect();
    let (mut app, _) = wrapped_editor(&dir, &text, 30, 10);
    let (total, height) = {
        let (buf, view) = app.active_mut().unwrap();
        let total = view
            .wrap
            .total_rows(&buf.rope, view.revision, view.width, view.tab_width);
        (total, view.height)
    };
    assert!(total > height, "the wrapped text is taller than the view");
    let bar = app.hits.group(app.groups.active).unwrap().scrollbar.unwrap();
    let expected = Scroll::text(total, height, 0).thumb(bar.track.height).unwrap();
    let hit = bar.thumb.unwrap();
    assert_eq!(
        (hit.start, hit.len),
        (expected.start, expected.len),
        "the thumb's length is the screen's share of the rows"
    );

    // A press at the track's bottom scrolls to the last row.
    let track = bar.track;
    let (x, y) = (track.x, track.y + track.height - 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    let (line, row) = {
        let (buf, view) = app.active_mut().unwrap();
        view.wrap
            .line_at(&buf.rope, view.revision, total - 1, view.width, view.tab_width)
    };
    assert_eq!(top(&app), (line, row), "the drag's row, not its line");
    let lines = crate::buffer::pos::line_count(&app.active_buffer().unwrap().rope);
    assert_eq!(line, lines - 1, "the text's last line");

    // The next frame keeps the row (nothing reveals the cursor back up).
    draw(&mut app, 30, 10);
    let bar = app.hits.group(app.groups.active).unwrap().scrollbar.unwrap();
    assert!(bar.thumb.unwrap().start > hit.start, "the thumb moved down the track");
}

/// While wrapping, a change's scroll-bar marks are its rows: a line below a wrapping line
/// projects to the cell of its first row, not of its line number.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_scroll_bar_marks_are_projected_over_rows() {
    let (_d, root, state) = dirs();
    init(&root);
    let original = "one\n  alpha bravo charlie delta echo foxtrot\nthree\nfour\nfive\nsix\nseven\neight\n";
    fs::write(root.join("f.txt"), original).unwrap();
    commit_all(&root, "one");
    fs::write(root.join("f.txt"), original.replace("three", "THREE")).unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await, "the gutter markers are current");
    app.ui.side_visible = false;
    app.test_execute(Command::ToggleWordWrap);
    let term = draw(&mut app, 30, 14);

    // The changed line (2) starts below the wrapping line 1, so its row is past its line.
    let (total, start, height) = {
        let (buf, view) = app.active_mut().unwrap();
        let total = view
            .wrap
            .total_rows(&buf.rope, view.revision, view.width, view.tab_width);
        let start = view
            .wrap
            .row_of_line(&buf.rope, view.revision, 2, view.width, view.tab_width);
        (total, start, view.height)
    };
    assert!(start > 2, "line 2 is below the wrapped line 1: row {start}");
    let bar = app.hits.group(app.groups.active).unwrap().scrollbar.unwrap();
    let row_cell = Scroll::text(total, height, 0).cell(bar.track.height, start);
    let lines = crate::buffer::pos::line_count(&app.active_buffer().unwrap().rope);
    let line_cell = Scroll::text(lines, height, 0).cell(bar.track.height, 2);
    assert_ne!(row_cell, line_cell, "rows, not lines, put the mark on the track");
    let mark = app.theme.get("editorGutter.modifiedBackground").fg.unwrap();
    assert_eq!(
        bg(&term, bar.track.x, bar.track.y + row_cell),
        Some(mark),
        "the change's mark is on its first row's cell"
    );
    assert_ne!(
        bg(&term, bar.track.x, bar.track.y + line_cell),
        Some(mark),
        "not on the line-number cell"
    );
}

/// Narrowing a split rewraps the lines: the cursor's line takes more rows, and the frame keeps
/// the cursor visible.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_narrowing_a_split_rewraps_with_the_cursor_in_view() {
    let dir = tempfile::tempdir().unwrap();
    let text: String = (0..40)
        .map(|i| format!("line {i} with several more words to wrap around\n"))
        .collect();
    let (mut app, _, _) = open_text(&dir, "", "", &text);
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    let end = app.active_buffer().unwrap().rope.len();
    put_cursor(&mut app, end);
    app.active_mut().unwrap().1.reveal_cursor = true;

    let term = draw(&mut app, 100, 12);
    let area = app.hits.group(app.groups.active).unwrap().text;
    let cursor = term.backend().cursor_position();
    assert!(
        (area.y..area.y + area.height).contains(&cursor.y),
        "visible when wide: {cursor:?} in {area:?}"
    );

    // At 40 columns the lines wrap to more rows: the cursor's row falls past the view's bottom
    // unless the frame re-reveals it.
    let term = draw(&mut app, 40, 12);
    let area = app.hits.group(app.groups.active).unwrap().text;
    let cursor = term.backend().cursor_position();
    assert!(
        (area.y..area.y + area.height).contains(&cursor.y),
        "the cursor is still on screen after the split narrowed: {cursor:?} in {area:?}"
    );
}

/// The active view's rows of `line`: `(start, end, indent)` — the bytes the row covers within
/// the line, and the display column it is drawn from (a continuation row's indentation).
fn rows_of_line(app: &mut App, line: usize) -> Vec<(usize, usize, usize)> {
    let (buf, view) = app.active_mut().unwrap();
    let layout = view
        .wrap
        .layout(&buf.rope, view.revision, line, view.width, view.tab_width);
    (0..layout.len())
        .map(|i| (layout.rows()[i].start.0, layout.row_end(i).0, layout.rows()[i].col.0))
        .collect()
}

/// A click, a drag, a double / triple click and an Alt+Click on a continuation row land on its
/// bytes: its first byte in the indentation, the text's bytes where they are drawn, the word /
/// line for the extra clicks and a new cursor for Alt.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_clicks_land_on_the_wrapped_rows() {
    let dir = tempfile::tempdir().unwrap();
    let text = "    alpha bravo charlie delta echo foxtrot\nsecond line\n";
    let (mut app, _) = wrapped_editor(&dir, text, 30, 10);
    let rect = app.hits.group(app.groups.active).unwrap().text;
    let rows = rows_of_line(&mut app, 0);
    assert!(rows.len() >= 2, "the line wraps: {rows:?}");
    let (start, end, indent) = rows[1];
    assert!(end - start > 8 && indent > 1, "a continuation row with text: {rows:?}");
    let at = |within: usize| start + within;

    // A click in the continuation row's indentation is its first byte.
    let (x, y) = (rect.x + 1, rect.y + 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(head(&app), start, "the row's first byte, not the line's");

    // A click on the row's text is the byte drawn there (the indent is not a line column).
    let (x, y) = (rect.x + indent as u16 + 2, rect.y + 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(head(&app), at(2));

    // A drag across the row selects from the press to the pointer.
    let (x0, x1, y) = (rect.x + indent as u16 + 3, rect.x + indent as u16 + 7, rect.y + 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x0, y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x1, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x1, y);
    let sel = app.group().active_tab().unwrap().view.selections.primary();
    assert_eq!((sel.start().0, sel.end().0), (at(3), at(7)));

    // A double click selects the word on the row.
    let (x, y) = (rect.x + indent as u16 + 4, rect.y + 1);
    for _ in 0..2 {
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }
    let (word_start, word_end) = {
        let rope = &app.active_buffer().unwrap().rope;
        pos::word_range(rope, ByteIdx(at(4)))
    };
    assert_ne!(word_start.0, word_end.0, "the click is in a word");
    let sel = app.group().active_tab().unwrap().view.selections.primary();
    assert_eq!(
        (sel.start().0, sel.end().0),
        (word_start.0, word_end.0),
        "the word drawn on the continuation row"
    );

    // A triple click selects the whole line, not the row (line commands stay per line).
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    let sel = app.group().active_tab().unwrap().view.selections.primary();
    let line_end = text.find('\n').unwrap();
    assert_eq!((sel.start().0, sel.end().0), (0, line_end + 1));

    // Alt+Click adds a cursor at the row's byte.
    put_cursor(&mut app, 0);
    let (x, y) = (rect.x + indent as u16 + 5, rect.y + 1);
    app.mouse_with(MouseEventKind::Down(MouseButton::Left), x, y, KeyModifiers::ALT);
    let view = &app.group().active_tab().unwrap().view;
    assert_eq!(view.selections.len(), 2, "a cursor added with the click");
    assert_eq!(view.selections.newest().head.0, at(5));

    // Shift+Alt+Click (box select) keeps the line-relative column: from the line's start to
    // the clicked byte, though it is drawn on a continuation row.
    put_cursor(&mut app, 0);
    app.mouse_with(
        MouseEventKind::Down(MouseButton::Left),
        rect.x + indent as u16 + 6,
        rect.y + 1,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    );
    let view = &app.group().active_tab().unwrap().view;
    assert!(view.column.is_some());
    let sel = view.selections.primary();
    assert_eq!((sel.start().0, sel.end().0), (0, at(6)));
}

/// The mouse resting on a continuation row, or on a line below one that wraps, targets that
/// row's byte (a mouse hover is a hit like a click), and nothing past a row's text.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_mouse_rest_targets_the_wrapped_rows_text() {
    let dir = tempfile::tempdir().unwrap();
    let text = "    alpha bravo charlie delta echo\n    second line words here\n";
    let (mut app, _) = wrapped_editor(&dir, text, 30, 10);
    let rect = app.hits.group(app.groups.active).unwrap().text;
    let rows0 = rows_of_line(&mut app, 0);
    assert!(rows0.len() >= 3, "line 0 wraps: {rows0:?}");
    let (start, _, indent) = rows0[1];

    // A word on line 0's second row: its own byte, not the line's column at that screen row.
    let (x, y) = (rect.x + indent as u16 + 2, rect.y + 1);
    app.mouse(MouseEventKind::Moved, x, y);
    let at = ByteIdx(start + 2);
    let rope = &app.active_buffer().unwrap().rope;
    let rest = app.hover.rest.as_ref().expect("resting over the row's text");
    assert_eq!(rest.at, at);
    assert_eq!(rest.word, pos::word_range(rope, at));

    // Line 1's first row is below line 0's rows: the screen row counts them.
    let line1 = text.find("second").unwrap();
    let y = rect.y + rows0.len() as u16;
    app.mouse(MouseEventKind::Moved, rect.x + 4 + 2, y);
    let rest = app.hover.rest.as_ref().expect("resting on line 1's first row");
    assert_eq!(rest.at, ByteIdx(line1 + 2));

    // Past the last row's text (the line breaks early): nothing to rest on.
    let rows1 = rows_of_line(&mut app, 1);
    let last = rows1.last().unwrap();
    let x = (rect.x + last.2 as u16 + (last.1 - last.0) as u16 + 1).min(rect.x + rect.width - 1);
    let y = rect.y + rows0.len() as u16 + rows1.len() as u16 - 1;
    app.mouse(MouseEventKind::Moved, x, y);
    assert!(app.hover.rest.is_none(), "past the row's text");
}

/// The code action list anchors at the cursor's screen cell on a continuation row: the list's
/// first row is the row below it, over the cursor's drawn column.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_popup_anchors_at_the_wrapped_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let source =
        "one\n  alpha alpha alpha alpha alpha alpha alpha\n  call(LONGA aaaa bbbb cccc dddd LONGB eeee ffff)\nlast\n";
    let root = project(dir.path(), &[("a.cpp", source)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    fake.respond(
        "textDocument/codeAction",
        serde_json::json!([{ "title": "Change it", "kind": "quickfix" }]),
    );
    app.ui.side_visible = false;
    app.test_execute(Command::ToggleWordWrap);
    // The cursor a word into LONGB, which wraps to a continuation row.
    let cursor = source.find("LONGB").unwrap() + 1;
    put_cursor(&mut app, cursor);
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let start = find(&screen, "LONGB");
    assert_eq!(start.len(), 1, "LONGB drawn once: {screen:?}");
    let (x, y) = start[0];
    let cursor_cell = (x + 1, y);
    let (_, above) = find(&screen, "LONGA")[0];
    assert!(y > above, "LONGB is on a continuation row: {screen:?}");

    app.test_execute(Command::QuickFix);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    draw(&mut app, 30, 14);
    let rect = app.hits.code_actions.expect("list drawn").rect;
    assert_eq!(rect.y, cursor_cell.1 + 1, "the row below the wrapped cursor's");
    assert!(
        rect.x <= cursor_cell.0 && cursor_cell.0 < rect.x + rect.width,
        "over the cursor's drawn column: {rect:?} at {cursor_cell:?}"
    );
    app.shutdown_servers().await;
}

/// Clicking the gutter marker of a changed line below a wrapping line opens that line's peek:
/// the click maps through the rows, so a line whose first row is below wrapping lines is not
/// mistaken for a later line (the marker is only on the line's first row).
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_gutter_click_opens_the_change_below_the_wrapped_rows() {
    let (_d, root, state) = dirs();
    init(&root);
    fs::write(root.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    commit_all(&root, "one");
    // Line 0 wraps (a change), and line 1 changed too: its marker is on line 1's first row,
    // several screen rows below the top.
    fs::write(
        root.join("f.txt"),
        "alpha alpha alpha alpha alpha alpha alpha alpha\ntwo changed\nthree\n",
    )
    .unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await, "the gutter markers are current");
    app.ui.side_visible = false;
    app.test_execute(Command::ToggleWordWrap);
    let term = draw(&mut app, 30, 14);
    let screen = cells(&term);
    let (_, y0) = find(&screen, "alpha")[0];
    let (_, y1) = find(&screen, "two changed")[0];
    assert!(y1 > y0 + 1, "line 0 wraps and the changed line is below it: {screen:?}");
    let text = app.hits.group(app.groups.active).unwrap().text;
    assert_eq!(
        term.backend().buffer()[(text.x - 1, y1)].symbol(),
        "▎",
        "the marker is in the gutter on line 1's first row"
    );
    assert!(app.quick_diff.peek.is_none(), "no peek before the click");

    app.mouse(MouseEventKind::Down(MouseButton::Left), text.x - 1, y1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), text.x - 1, y1);
    let peek = app.quick_diff.peek.as_ref().expect("the click opened the peek");
    assert!(
        peek.lines.contains(&1),
        "the peek is line 1's change, not a later line: {:?}",
        peek.lines
    );
}

/// A row scroll within a wrapped line closes the hover popup: `check_hover` compares the view's
/// `(top_line, top_row, left_col)` with the scroll recorded when the popup opened, so scrolling
/// into a continuation row (top_row moves, top_line stays) closes it as a line scroll does.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrap_row_scroll_closes_the_hover() {
    let dir = tempfile::tempdir().unwrap();
    let text = "  abcd efgh ijkl mnop qrst uvwx\nsecond line\n";
    let (mut app, _, _) = open_text(&dir, "", "", text);
    plain_editor(&mut app);
    app.test_execute(Command::ToggleWordWrap);
    let start = ByteIdx(text.find("mnop").unwrap());
    let (buf, _) = app.active_mut().unwrap();
    buf.diagnostics.replace(
        ServerId::test(1),
        vec![Mark {
            start,
            end: ByteIdx(start.0 + 4),
            server: ServerId::test(1),
            diag: std::sync::Arc::new(Diagnostic {
                severity: Severity::Error,
                message: "bad".into(),
                source: None,
                code: None,
                raw: serde_json::Value::Null,
            }),
        }],
    );
    put_cursor(&mut app, start.0 + 1);
    draw(&mut app, 30, 14);

    app.test_execute(Command::ShowHover);
    assert!(app.hover.popup.is_some(), "the hover opened");

    // One screen row down, still inside the wrapped first line: top_row moves, top_line stays.
    let (buf, view) = app.active_mut().unwrap();
    let before = (view.top_line(), view.top_row());
    view.scroll_lines(&buf.rope, 1);
    let after = (view.top_line(), view.top_row());
    assert_eq!(before.0, after.0, "still on the first line");
    assert!(
        after.1 > before.1,
        "scrolled into its continuation row: {before:?} -> {after:?}"
    );

    // The next event's `check_hover` sees the row scroll and closes the popup.
    app.test_execute(Command::Copy);
    assert!(app.hover.popup.is_none(), "the row scroll closed the hover");
}
