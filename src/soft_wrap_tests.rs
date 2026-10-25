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
//! Scrolling, movement and the mouse on wrapped rows come in the task's later subtasks. The
//! "Done when" parts covered here: the setting is read from `settings.toml` and `.workspace`,
//! defaults to `off`, the toggle flips `App::word_wrap()` and keeps doing so across a
//! `.workspace` reload, and the wrapped rows are drawn as the plan says.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Position;
use ratatui::style::Modifier;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::config::settings;
use crate::diagnostics::{Diagnostic, Mark, Severity};
use crate::edit::WordWrap;
use crate::git::test_repo::{commit_all, init};
use crate::lsp::ServerId;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{open_app, open_synced, project, put_cursor};
use crate::milestone10_tests::{dirs, open_file_app, settle_markers};
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
