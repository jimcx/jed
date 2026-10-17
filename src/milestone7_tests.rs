//! Milestone 7 end to end (7a, the engine): searches started through `App::start_search` run in
//! the background and stream into `app.search` (files in path order, line text and match ranges,
//! the total); `.gitignore`, `.workspace` excludes, `.jed/` and binary files are skipped; an
//! unsaved edit typed into an open buffer shows up in the results (and the disk version
//! doesn't); a new search replaces a running one and its stale results are dropped; an empty
//! pattern finishes at once and an invalid regex returns its error.
//!
//! 7b (`mod view`): the Search view through its keys, mouse and drawn cells: Cmd+Shift+F seeds
//! from a one-line selection (regex-escaped) and searches at once; typing restarts the search
//! only after the debounce, dropping the replaced search's results; the case / regex toggles
//! (keys and buttons, separate from the find widget's) change the results; an invalid regex's
//! error under the input; the summary, file rows with counts, highlighted matches and the
//! truncated note; the wheel scrolls; the activity icon switches views; Esc returns to the
//! editor; keys and paste never reach the buffer; a `keybindings.toml` search-context binding;
//! an unsaved edit after searching again; and typing in the query on a big workspace stays
//! within a frame (asserted in release: `cargo test --release milestone -- --nocapture`).
//!
//! 7c (`mod navigate`): Down from the query enters the results list, Up from its first row
//! leaves it; Up/Down/PageUp/PageDown/Home/End move the selection (drawn active / inactive);
//! Enter opens the selected match (a file row: its first) in the focused group with it selected
//! and focuses the editor; a click opens a match keeping the focus in the list, a double click
//! focuses the editor; F4 / Shift+F4 from anywhere step through every match (several on a line
//! too) with wrapping, without moving the focus; results open in the last focused of two
//! groups; files not open yet are loaded and then selected (a later request while loading
//! decides the selection, focus is kept if any request asked); an opened match is centred;
//! results whose file changed since are clamped; results streaming in don't move the selection.
//!
//! Done when (`mod done_when`): a copy of this repo's `src/` and `docs/`, searched through the
//! Search view in all three modes, gives exactly the hits of an independent reference (what
//! `rg -F -i`, `rg -F -s` and `rg` report); an unsaved edit shows up in the results and opens
//! with its text selected; typing never stalls (`view::milestone_7_typing_in_the_query_never_stalls`,
//! plus a burst of keys during a big search within a frame, ending with the final query's
//! results only).

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
    write(&root, ".jed/history/x", "needle\n");
    write(&root, "img.bin", b"needle\0\x01\x02");
    let buffer = Buffer::open(&root.join("a.txt")).unwrap();
    let mut app = test_app(&root, buffer);
    app.test_start();
    app.test_plain_whitespace();
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

// 7b: the Search view.

mod view {
    use std::time::{Duration, Instant};

    use crossterm::event::{Event, KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Style;

    use super::{cmd, open_app, write};
    use crate::app::{App, Focus};
    use crate::buffer::Buffer;
    use crate::buffer::pos::ByteIdx;
    use crate::layout::SidePanel;
    use crate::search_view::DEBOUNCE;
    use crate::selection::{Selection, Selections};

    /// One keystroke in the query plus a frame, on the big workspace with a search running.
    pub(super) const KEYSTROKE_BUDGET: Duration = Duration::from_millis(16);
    /// The same in debug builds: only catches drawing or input doing work per result.
    pub(super) const KEYSTROKE_DEBUG_BOUND: Duration = Duration::from_millis(500);

    /// The toggle keys: Cmd+Alt on macOS, Alt on Linux.
    pub(super) fn toggle() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER | M::ALT
        } else {
            M::ALT
        }
    }

    pub(super) fn term() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(100, 30)).unwrap()
    }

    pub(super) fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
        term.draw(|f| crate::ui::render(f, app)).unwrap();
    }

    pub(super) fn row(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The side panel's text, row by row (the Search view lives there), trimmed.
    pub(super) fn panel(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                let r: String = (3..32).map(|x| buf[(x, y)].symbol()).collect();
                r.trim_end().to_string()
            })
            .collect()
    }

    pub(super) fn panel_has(term: &Terminal<TestBackend>, text: &str) -> bool {
        panel(term).iter().any(|r| r.contains(text))
    }

    /// The cell where `text` first appears in the side panel.
    pub(super) fn find(term: &Terminal<TestBackend>, text: &str) -> Option<(u16, u16)> {
        let p = panel(term);
        let y = p.iter().position(|r| r.contains(text))?;
        // Cells hold one char each here (ASCII rows).
        let x = p[y][..p[y].find(text)?].chars().count() as u16;
        Some((3 + x, y as u16))
    }

    pub(super) fn bg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Option<ratatui::style::Color> {
        Some(term.backend().buffer()[(x, y)].bg)
    }

    pub(super) fn theme_bg(app: &App, key: &str) -> Option<ratatui::style::Color> {
        let s: Style = app.theme.get(key);
        s.bg
    }

    pub(super) fn find_in_files(app: &mut App) {
        app.press(K::Char('f'), cmd() | M::SHIFT);
    }

    pub(super) async fn settled(app: &mut App) {
        assert!(
            app.settle(|a| a.search.as_ref().is_some_and(|r| r.done) && a.search_view.deadline().is_none())
                .await
        );
    }

    pub(super) fn select(app: &mut App, from: usize, to: usize) {
        let view = app.active_mut().unwrap().1;
        view.selections = Selections::single(Selection::new(ByteIdx(from), ByteIdx(to)));
    }

    pub(super) fn total(app: &App) -> usize {
        app.search.as_ref().unwrap().total
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_cmd_shift_f_seeds_from_the_selection_and_searches() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        let mut term = term();
        // "needle" on a.txt's second line.
        select(&mut app, 18, 24);
        find_in_files(&mut app);
        assert_eq!((app.ui.focus, app.ui.side_panel), (Focus::SidePanel, SidePanel::Search));
        assert_eq!(app.search_view.query.text, "needle");
        assert!(app.search_view.query.all_selected, "selected, so typing replaces it");
        assert!(app.search.is_some(), "searching at once, no debounce");
        settled(&mut app).await;
        draw(&mut term, &mut app);
        let p = panel(&term);
        assert_eq!(p[0].trim(), "SEARCH");
        assert!(
            p[1].contains("needle") && p[1].contains("Aa") && p[1].contains(".*"),
            "{p:?}"
        );
        assert_eq!(p[2].trim(), "3 results in 2 files");
        // Files in path order: name, dimmed folder, count; then each line with its match.
        assert!(p[4].contains("a.txt") && p[4].ends_with(" 1"), "{p:?}");
        assert_eq!(p[5].trim(), "second needle");
        assert!(p[6].contains("b.rs src") && p[6].ends_with(" 2"), "{p:?}");
        assert_eq!(p[7].trim(), "fn needle() {}");
        assert_eq!(p[8].trim(), "// NEEDLE");
        let at = find(&term, "second needle").unwrap();
        let matched = theme_bg(&app, "search.match");
        assert_ne!(bg(&term, at), matched, "\"second\" isn't highlighted");
        for i in 7..13 {
            assert_eq!(bg(&term, (at.0 + i, at.1)), matched, "needle is");
        }
        assert_ne!(bg(&term, (at.0 + 6, at.1)), matched);
        // The terminal cursor is in the query.
        assert_eq!(term.get_cursor_position().unwrap().y, 1);

        // With regex on, the seed is escaped so it finds itself.
        app.press(K::Char('r'), toggle());
        assert!(app.search_view.options.regex);
        app.test_open(Buffer::open(&root.join("src/b.rs")).unwrap());
        app.ui.focus = Focus::Editor;
        select(&mut app, 3, 11);
        find_in_files(&mut app);
        assert_eq!(app.search_view.query.text, r"needle\(\)");
        settled(&mut app).await;
        assert_eq!(total(&app), 1);

        // A selection over two lines (or none) keeps the last query, and searches again.
        app.ui.focus = Focus::Editor;
        select(&mut app, 0, 20);
        find_in_files(&mut app);
        assert_eq!(app.search_view.query.text, r"needle\(\)");
        settled(&mut app).await;
        assert_eq!(total(&app), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_typing_restarts_the_search_after_a_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let before = app.active_buffer().unwrap().rope.to_string();
        let mut term = term();
        find_in_files(&mut app);
        let first = app.search.as_ref().unwrap().generation;
        app.type_text("n");
        let due = app.search_view.deadline().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        app.type_text("ee");
        // Each key pushes the restart back; nothing restarts until the debounce passes.
        let pushed = app.search_view.deadline().unwrap();
        assert!(pushed > due, "the second key pushed the deadline");
        assert_eq!(app.search.as_ref().unwrap().generation, first);
        app.search_tick(due);
        assert_eq!(app.search.as_ref().unwrap().generation, first, "not due yet");
        app.search_tick(pushed - Duration::from_millis(1));
        assert_eq!(app.search.as_ref().unwrap().generation, first, "not due yet");
        app.search_tick(pushed);
        let nee = app.search.as_ref().unwrap().generation;
        assert!(nee > first);
        assert_eq!(app.search.as_ref().unwrap().query.pattern, "nee");
        // Typing on cancels "nee" when the next search starts; its batches are dropped.
        app.type_text("dle");
        app.search_tick(Instant::now() + DEBOUNCE);
        settled(&mut app).await;
        let r = app.search.as_ref().unwrap();
        assert_eq!((r.query.pattern.as_str(), r.total), ("needle", 3));
        assert!(
            r.files
                .iter()
                .flat_map(|f| &f.lines)
                .all(|l| l.ranges.iter().all(|m| m.len() == 6))
        );
        draw(&mut term, &mut app);
        assert!(panel_has(&term, "3 results in 2 files"));

        // The event loop's own timer (here `settle`) runs it too, and shows "Searching…" until then.
        app.type_text("x");
        draw(&mut term, &mut app);
        assert!(panel_has(&term, "Searching…"));
        settled(&mut app).await;
        draw(&mut term, &mut app);
        assert!(panel_has(&term, "No results found"));

        // Emptying the query clears the results at once.
        for _ in 0.."needlex".len() {
            app.press(K::Backspace, M::NONE);
        }
        assert_eq!(app.search_view.deadline(), None);
        let r = app.search.as_ref().unwrap();
        assert!(r.done && r.files.is_empty());
        draw(&mut term, &mut app);
        assert_eq!(panel(&term)[2], "", "no summary for an empty query");

        // None of it reached the buffer; paste goes to the query too.
        app.handle_test_event(Event::Paste("pasted\n".into()));
        assert_eq!(app.search_view.query.text, "pasted");
        assert_eq!(app.active_buffer().unwrap().rope.to_string(), before);
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_toggles_change_the_results() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("NEEDLE");
        settled(&mut app).await;
        assert_eq!(total(&app), 3, "case-insensitive by default");
        draw(&mut term, &mut app);
        let case_at = find(&term, "Aa").unwrap();
        assert_ne!(bg(&term, case_at), theme_bg(&app, "find.option.active"));

        app.press(K::Char('c'), toggle());
        assert!(app.search_view.options.case_sensitive);
        assert!(
            !app.find_options.case_sensitive,
            "the find widget's options are separate"
        );
        settled(&mut app).await;
        draw(&mut term, &mut app);
        assert!(panel_has(&term, "1 result in 1 file"), "{:?}", panel(&term));
        assert_eq!(bg(&term, case_at), theme_bg(&app, "find.option.active"));

        // Regex: `NE..LE` is literal text until the toggle.
        for _ in 0..4 {
            app.press(K::Backspace, M::NONE);
        }
        app.type_text("..LE");
        settled(&mut app).await;
        assert_eq!(total(&app), 0);
        app.press(K::Char('r'), toggle());
        settled(&mut app).await;
        assert_eq!(total(&app), 1);
        draw(&mut term, &mut app);
        assert_eq!(
            bg(&term, find(&term, ".*").unwrap()),
            theme_bg(&app, "find.option.active")
        );
        assert!(panel_has(&term, "// NEEDLE"));

        // The buttons click: case off again finds all three.
        app.mouse(MouseEventKind::Down(MouseButton::Left), case_at.0, case_at.1);
        assert!(!app.search_view.options.case_sensitive);
        settled(&mut app).await;
        assert_eq!(total(&app), 3);
        assert_eq!(app.ui.focus, Focus::SidePanel);
        // Whole word isn't a Search view option (v1): its key does nothing there.
        app.press(K::Char('w'), toggle());
        assert!(!app.search_view.options.whole_word && !app.find_options.whole_word);
        assert_eq!(app.search_view.query.text, "NE..LE");
    }

    /// The graphemes of row `y` from `x` on, with whether each has the `search.match` background.
    /// The cell a wide character covers is skipped (the test backend keeps stale text there).
    fn cells_from(term: &Terminal<TestBackend>, app: &App, mut x: u16, y: u16) -> Vec<(String, bool)> {
        use unicode_width::UnicodeWidthStr;
        let buf = term.backend().buffer();
        let matched = theme_bg(app, "search.match");
        let mut out = Vec::new();
        while x < 32 {
            let cell = &buf[(x, y)];
            out.push((cell.symbol().to_string(), Some(cell.bg) == matched));
            x += cell.symbol().width().max(1) as u16;
        }
        out
    }

    /// The highlighted text in row `y` of the panel.
    fn highlighted(term: &Terminal<TestBackend>, app: &App, y: u16) -> String {
        cells_from(term, app, 3, y)
            .into_iter()
            .filter(|(_, m)| *m)
            .map(|(s, _)| s)
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_lines_are_cut_to_show_the_match_and_empty_matches_dont_hide_others() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        write(&root, "wide.txt", format!("{}needle end\n", "日本語".repeat(20)));
        let mut term = term();
        find_in_files(&mut app);
        app.press(K::Char('r'), toggle());
        // `^` matches (empty) at every line start, before "needle" on the same line.
        app.type_text("^|needle");
        settled(&mut app).await;
        draw(&mut term, &mut app);
        let p = panel(&term);
        let y = p.iter().position(|r| r.trim() == "second needle").unwrap() as u16;
        assert_eq!(highlighted(&term, &app, y), "needle", "{p:?}");

        // A long line of wide characters: cut with "…" before a little context, the match whole.
        for _ in 0.."^|needle".len() {
            app.press(K::Backspace, M::NONE);
        }
        app.type_text("needle");
        settled(&mut app).await;
        draw(&mut term, &mut app);
        let p = panel(&term);
        let y = p.iter().position(|r| r.contains("needle") && r.contains('語')).unwrap() as u16;
        let cells = cells_from(&term, &app, 6, y);
        assert_eq!(cells[0].0, "…", "{p:?}");
        assert_eq!(highlighted(&term, &app, y), "needle", "{p:?}");
        let text: String = cells.iter().map(|(s, _)| s.as_str()).collect();
        assert!(text.contains("語needle"), "{text:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_invalid_regex_shows_its_error_and_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        find_in_files(&mut app);
        app.press(K::Char('r'), toggle());
        app.type_text("(needle");
        app.search_tick(Instant::now() + DEBOUNCE);
        assert!(app.search.is_none());
        draw(&mut term, &mut app);
        let p = panel(&term);
        assert_eq!(p[2].trim(), "unclosed group");
        let at = find(&term, "unclosed").unwrap();
        assert_eq!(
            term.backend().buffer()[at].fg,
            app.theme.get("search.error").fg.unwrap()
        );
        assert!(!panel_has(&term, "result"));
        // Fixing it searches again and drops the error.
        app.type_text(")");
        settled(&mut app).await;
        draw(&mut term, &mut app);
        assert!(app.search_view.error.is_none());
        assert_eq!(panel(&term)[2].trim(), "3 results in 2 files");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_truncated_results_say_so_and_scroll() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        write(&root, "many.txt", "q\n".repeat(25_000));
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("q");
        settled(&mut app).await;
        let r = app.search.as_ref().unwrap();
        assert!(r.truncated);
        draw(&mut term, &mut app);
        let p = panel(&term);
        assert_eq!(p[2].trim(), "20,000 results in 1 file");
        assert!(p[3].contains("Results truncated"), "{p:?}");
        assert!(p[5].contains("many.txt") && p[5].ends_with("20,000"), "{p:?}");
        // The wheel scrolls the list (only the visible rows are drawn).
        let (x, y) = (10, 10);
        app.mouse(MouseEventKind::ScrollDown, x, y);
        draw(&mut term, &mut app);
        assert!(!panel_has(&term, "many.txt"), "the file row scrolled off");
        for _ in 0..10_000 {
            app.mouse(MouseEventKind::ScrollDown, x, y);
        }
        draw(&mut term, &mut app);
        assert_eq!(panel(&term)[28].trim(), "q", "the last row reaches the bottom");
        assert!(!row(&term, 29).trim().is_empty(), "the status bar is below it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_activity_icon_switches_views_and_esc_returns_to_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        draw(&mut term, &mut app);
        let icon = |app: &App, panel: SidePanel| app.hits.icons.iter().find(|(_, p)| *p == panel).unwrap().0;
        let y = icon(&app, SidePanel::Search);
        app.mouse(MouseEventKind::Down(MouseButton::Left), 1, y);
        assert_eq!((app.ui.side_panel, app.ui.focus), (SidePanel::Search, Focus::SidePanel));
        draw(&mut term, &mut app);
        assert_eq!(panel(&term)[0].trim(), "SEARCH");
        assert!(panel(&term)[1].contains("Search"), "the placeholder");
        // Keys type into the query, not the buffer or the explorer.
        app.type_text("needle");
        app.press(K::Enter, M::NONE);
        assert_eq!(app.search_view.deadline(), None, "Enter searches now");
        assert_eq!(app.search.as_ref().unwrap().query.pattern, "needle");
        settled(&mut app).await;
        assert_eq!(total(&app), 3);
        assert!(!app.active_buffer().unwrap().is_dirty());

        // Esc: back to the editor; results and query stay, the buffer untouched.
        app.press(K::Esc, M::NONE);
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(app.ui.side_panel, SidePanel::Search);
        draw(&mut term, &mut app);
        assert!(panel_has(&term, "3 results in 2 files"));
        assert_eq!(
            app.active_buffer().unwrap().rope.to_string(),
            "first line\nsecond needle\n"
        );

        // The explorer icon (and Cmd+Shift+E) shows the explorer again.
        let y = icon(&app, SidePanel::Explorer);
        app.mouse(MouseEventKind::Down(MouseButton::Left), 1, y);
        draw(&mut term, &mut app);
        assert_eq!(panel(&term)[0].trim(), "EXPLORER");
        find_in_files(&mut app);
        assert_eq!(app.search_view.query.text, "needle");
        app.press(K::Char('e'), cmd() | M::SHIFT);
        assert_eq!(app.ui.side_panel, SidePanel::Explorer);
        // With no editor open, Esc leaves the focus in the query.
        app.press(K::Char('w'), cmd());
        assert!(app.group().active_tab().is_none());
        find_in_files(&mut app);
        app.press(K::Esc, M::NONE);
        assert_eq!(app.ui.focus, Focus::SidePanel);
        // The palette lists the Search commands.
        let cmds = crate::command::Command::all();
        assert!(cmds.iter().any(|c| c.title() == "Search: Find in Files"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_unsaved_edit_shows_up_after_searching_again() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("needle");
        settled(&mut app).await;
        assert_eq!(total(&app), 3);
        app.press(K::Esc, M::NONE);
        app.type_text("needle ");
        find_in_files(&mut app);
        settled(&mut app).await;
        assert_eq!(total(&app), 4);
        draw(&mut term, &mut app);
        let p = panel(&term);
        assert!(p[4].contains("a.txt") && p[4].ends_with(" 2"), "{p:?}");
        assert_eq!(p[5].trim(), "needle first line");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_keybindings_toml_in_the_search_context() {
        use crate::config::keymap::{self, Keymap, Platform};
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let config = dir.path().join(keymap::FILE_NAME);
        std::fs::write(
            &config,
            "[[binding]]\nkey = \"ctrl+t\"\ncommand = \"search.toggleCase\"\ncontext = \"search\"\n",
        )
        .unwrap();
        let (km, errors) = Keymap::load(&config, Platform::current());
        assert!(errors.is_empty(), "{errors:?}");
        app.set_keymap(km, errors);
        app.press(K::Char('t'), M::CONTROL);
        assert!(!app.search_view.options.case_sensitive, "only in the search context");
        find_in_files(&mut app);
        app.press(K::Char('t'), M::CONTROL);
        assert!(app.search_view.options.case_sensitive);
        // The default key lost the command.
        app.press(K::Char('c'), toggle());
        assert!(app.search_view.options.case_sensitive);
        assert_eq!(app.search_view.query.text, "");
    }

    /// 100 files under `big/` (10 MB in release builds, 1 MB in debug), each line
    /// `let needle_{file}_{i} = haystack({i}); …`. Returns the size.
    pub(super) fn big_workspace(root: &std::path::Path) -> usize {
        let size = if cfg!(debug_assertions) { 1_000_000 } else { 10_000_000 };
        let files = 100;
        for f in 0..files {
            let mut text = String::new();
            let mut i = 0;
            while text.len() < size / files {
                text.push_str(&format!("let needle_{f}_{i} = haystack({i}); // filler text here\n"));
                i += 1;
            }
            write(root, &format!("big/f{f:03}.rs"), text);
        }
        size
    }

    /// The "typing never stalls" check: each key in the query plus a frame stays within a frame
    /// on a big workspace while searches run (restarted every few keys), since the search runs
    /// off the UI thread and drawing looks only at the visible rows.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_typing_in_the_query_never_stalls() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        let size = big_workspace(&root);
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("needle");
        settled(&mut app).await;
        assert!(total(&app) > 15_000);

        let keys = "_4_12";
        let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
        let mut n = 0;
        for _ in 0..4 {
            for c in keys.chars() {
                let t = Instant::now();
                app.press(K::Char(c), M::NONE);
                // Every other key the debounce runs out: a search restarts mid-typing.
                if n % 2 == 1 {
                    app.search_tick(Instant::now() + DEBOUNCE);
                }
                app.pump();
                draw(&mut term, &mut app);
                let e = t.elapsed();
                total += e;
                worst = worst.max(e);
                n += 1;
            }
            for _ in keys.chars() {
                app.press(K::Backspace, M::NONE);
            }
        }
        let avg = total / n;
        println!("search view keystroke + frame ({size} bytes): {n} keys, avg {avg:?}, worst {worst:?}");
        if cfg!(debug_assertions) {
            assert!(worst < KEYSTROKE_DEBUG_BOUND, "worst {worst:?}");
        } else {
            assert!(worst < KEYSTROKE_BUDGET, "worst {worst:?} > {KEYSTROKE_BUDGET:?}");
        }
        settled(&mut app).await;
    }
}

// 7c: navigating results, and the milestone's "done when".

mod navigate {
    use std::time::Instant;

    use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};

    use super::view::{draw, find, find_in_files, panel, settled, term, theme_bg};
    use super::{cmd, open_app, write};
    use crate::app::{App, Focus};
    use crate::search_view::DEBOUNCE;
    use crate::workspace_search::{Batch, FileMatches, LineMatch};

    /// The active tab's file, relative to `root`, and its primary selection (anchor, head).
    fn at(app: &mut App) -> (String, (usize, usize)) {
        let root = app.workspace.root.clone();
        let path = app
            .active_buffer()
            .unwrap()
            .path
            .strip_prefix(&root)
            .unwrap()
            .display()
            .to_string();
        let s = *app.active_mut().unwrap().1.selections.primary();
        (path, (s.anchor.0, s.head.0))
    }

    fn selected_text(app: &mut App) -> String {
        let (buf, view) = app.active_mut().unwrap();
        let s = view.selections.primary();
        buf.rope.slice(s.start().0..s.end().0).to_string()
    }

    fn selected_row(app: &App) -> Option<usize> {
        app.search_view.selected_row(app.search.as_ref().unwrap())
    }

    /// Waits until the active tab shows `path` (opening a file is asynchronous).
    async fn showing(app: &mut App, path: &str) {
        let path = app.workspace.root.join(path);
        assert!(
            app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await,
            "{} never opened",
            path.display()
        );
    }

    /// Searches `needle` in `open_app`'s workspace. Rows: 0 a.txt, 1 its line 2 (bytes 18..24),
    /// 2 src/b.rs, 3 its line 1 (3..9), 4 its line 2 (19..25; the file has CRLF line ends).
    async fn needle(app: &mut App) {
        find_in_files(app);
        app.type_text("needle");
        settled(app).await;
        assert_eq!(app.search.as_ref().unwrap().total, 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_keys_move_through_the_list_and_enter_opens() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        needle(&mut app).await;

        // Down from the query: the list takes the keys, its first row selected.
        app.press(K::Down, M::NONE);
        assert!(app.search_view.list_focused);
        assert_eq!(selected_row(&app), Some(0));
        draw(&mut term, &mut app);
        assert!(term.get_cursor_position().unwrap().y != 1, "no cursor in the query");
        let (x, y) = find(&term, "a.txt").unwrap();
        let active = theme_bg(&app, "list.active");
        assert_eq!(term.backend().buffer()[(x, y)].bg, active.unwrap());
        // Up from the first row: back to the query (Enter there searches again).
        app.press(K::Up, M::NONE);
        assert!(!app.search_view.list_focused);
        assert_eq!(app.ui.focus, Focus::SidePanel);
        draw(&mut term, &mut app);
        assert_eq!(term.get_cursor_position().unwrap().y, 1, "the cursor is in the query");
        assert_ne!(
            term.backend().buffer()[(x, y)].bg,
            active.unwrap(),
            "inactive selection"
        );
        assert_eq!(
            term.backend().buffer()[(x, y)].bg,
            theme_bg(&app, "list.inactive").unwrap()
        );

        // Down again keeps the selection; Down, PageDown, End, Home, PageUp move it, clamped.
        app.press(K::Down, M::NONE);
        assert_eq!(selected_row(&app), Some(0));
        app.press(K::Down, M::NONE);
        assert_eq!(selected_row(&app), Some(1));
        app.press(K::PageDown, M::NONE);
        assert_eq!(selected_row(&app), Some(4), "a page is more than the list");
        app.press(K::Down, M::NONE);
        assert_eq!(selected_row(&app), Some(4));
        app.press(K::Home, M::NONE);
        assert_eq!(selected_row(&app), Some(0));
        app.press(K::End, M::NONE);
        assert_eq!(selected_row(&app), Some(4));
        app.press(K::PageUp, M::NONE);
        assert_eq!(selected_row(&app), Some(0));
        app.press(K::End, M::NONE);

        // Enter opens src/b.rs (not open yet: loaded, then selected) and focuses the editor.
        app.press(K::Enter, M::NONE);
        showing(&mut app, "src/b.rs").await;
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)));
        assert_eq!(selected_text(&mut app), "NEEDLE");
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(app.search.as_ref().unwrap().total, 3, "the results stay");

        // Enter on a file row opens its first match; a.txt is open: selected at once.
        app.ui.focus = Focus::SidePanel;
        app.search_view.list_focused = true;
        app.press(K::Home, M::NONE);
        app.press(K::Enter, M::NONE);
        assert_eq!(at(&mut app), ("a.txt".into(), (18, 24)));
        assert_eq!(selected_row(&app), Some(1), "the match row");
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(app.group().tabs.len(), 2, "a.txt's own tab, not a new one");

        // Typing in the list goes back to the query and types there; other keys do nothing.
        app.ui.focus = Focus::SidePanel;
        app.search_view.list_focused = true;
        app.press(K::Left, M::NONE);
        assert!(app.search_view.list_focused);
        app.type_text("x");
        assert!(!app.search_view.list_focused);
        assert_eq!(app.search_view.query.text, "needlex");
        // So does a paste.
        app.press(K::Down, M::NONE);
        assert!(app.search_view.list_focused);
        app.handle_test_event(crossterm::event::Event::Paste("y".into()));
        assert!(!app.search_view.list_focused);
        assert_eq!(app.search_view.query.text, "needlexy");
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_clicking_a_result_opens_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        needle(&mut app).await;
        draw(&mut term, &mut app);
        let click = |app: &mut App, (x, y): (u16, u16)| app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);

        // A file row: selected, nothing opens.
        click(&mut app, find(&term, "b.rs").unwrap());
        assert_eq!(selected_row(&app), Some(2));
        assert!(app.search_view.list_focused);
        assert_eq!(at(&mut app).0, "a.txt");

        // A match: opens it, the keys stay in the list (VS Code's preview click).
        click(&mut app, find(&term, "// NEEDLE").unwrap());
        showing(&mut app, "src/b.rs").await;
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)));
        assert_eq!((app.ui.focus, app.search_view.list_focused), (Focus::SidePanel, true));
        assert_eq!(selected_row(&app), Some(4));

        // A double click: opens and focuses the editor.
        draw(&mut term, &mut app);
        let line = find(&term, "second needle").unwrap();
        click(&mut app, line);
        assert_eq!(at(&mut app), ("a.txt".into(), (18, 24)));
        assert_eq!(app.ui.focus, Focus::SidePanel);
        click(&mut app, line);
        assert_eq!(app.ui.focus, Focus::Editor);

        // Clicking the query gives it the keys back.
        draw(&mut term, &mut app);
        click(&mut app, (6, 1));
        assert_eq!((app.ui.focus, app.search_view.list_focused), (Focus::SidePanel, false));

        // A double click on a file row opens its first match.
        let file_row = find(&term, "b.rs").unwrap();
        click(&mut app, file_row);
        assert_eq!(at(&mut app).0, "a.txt");
        click(&mut app, file_row);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)));
        assert_eq!(app.ui.focus, Focus::Editor);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_f4_steps_through_results_from_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        // Nothing searched: F4 does nothing.
        app.press(K::F(4), M::NONE);
        app.press(K::F(4), M::SHIFT);
        assert_eq!(at(&mut app), ("a.txt".into(), (0, 0)));

        needle(&mut app).await;
        // From the query, the focus stays put.
        app.press(K::F(4), M::NONE);
        assert_eq!(at(&mut app), ("a.txt".into(), (18, 24)));
        assert_eq!((app.ui.focus, app.search_view.list_focused), (Focus::SidePanel, false));
        assert_eq!(selected_row(&app), Some(1));
        // Down then enters the list on that row.
        app.press(K::Down, M::NONE);
        assert!(app.search_view.list_focused);
        assert_eq!(selected_row(&app), Some(1));

        // From the editor too, opening files as needed; it wraps both ways.
        app.press(K::Esc, M::NONE);
        assert_eq!(app.ui.focus, Focus::Editor);
        app.press(K::F(4), M::NONE);
        showing(&mut app, "src/b.rs").await;
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)));
        assert_eq!(app.ui.focus, Focus::Editor);
        app.press(K::F(4), M::NONE);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)));
        assert_eq!(selected_row(&app), Some(4));
        app.press(K::F(4), M::NONE);
        assert_eq!(at(&mut app), ("a.txt".into(), (18, 24)), "wrapped");
        app.press(K::F(4), M::SHIFT);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)), "wrapped back");
        app.press(K::F(4), M::SHIFT);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)));
        assert_eq!(app.group().tabs.len(), 2);
        assert_eq!(app.ui.focus, Focus::Editor);

        // Several matches on a line are visited one by one.
        app.press(K::End, M::NONE);
        app.type_text(" needle needle");
        find_in_files(&mut app);
        app.press(K::Esc, M::NONE);
        settled(&mut app).await;
        let mut seen = Vec::new();
        for _ in 0..6 {
            app.press(K::F(4), M::NONE);
            seen.push(at(&mut app));
        }
        let b = |s, e| ("src/b.rs".to_string(), (s, e));
        let a = |s, e| ("a.txt".to_string(), (s, e));
        assert_eq!(seen, [a(18, 24), b(3, 9), b(15, 21), b(22, 28), b(33, 39), a(18, 24)]);

        // Emptied results: nothing to go to.
        find_in_files(&mut app);
        app.press(K::Backspace, M::NONE);
        app.press(K::Esc, M::NONE);
        app.press(K::F(4), M::NONE);
        assert_eq!(at(&mut app), a(18, 24));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_results_open_in_the_focused_group() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        app.press(K::Char('\\'), cmd());
        let ids = app.groups.ids();
        assert_eq!(ids.len(), 2);
        app.press(K::Char('1'), cmd());
        needle(&mut app).await;
        assert_eq!(app.groups.active, ids[0], "the last focused group stays the target");
        app.press(K::Down, M::NONE);
        for _ in 0..3 {
            app.press(K::Down, M::NONE);
        }
        app.press(K::Enter, M::NONE);
        showing(&mut app, "src/b.rs").await;
        assert_eq!(app.groups.active, ids[0]);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)));
        let tabs = |app: &App, g| app.groups.get(g).unwrap().tabs.len();
        assert_eq!((tabs(&app, ids[0]), tabs(&app, ids[1])), (2, 1));

        // Group 2 focused: the next result opens there, in a tab of its own.
        app.press(K::Char('2'), cmd());
        app.press(K::F(4), M::NONE);
        assert_eq!(app.groups.active, ids[1]);
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)));
        assert_eq!((tabs(&app, ids[0]), tabs(&app, ids[1])), (2, 2));
        app.press(K::Char('1'), cmd());
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)), "group 1's view untouched");
    }

    /// While a file loads, a later request for it replaces where its selection goes (the last F4
    /// wins) and keeps the focus an earlier Enter asked for.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_the_last_request_wins_while_a_file_loads() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        needle(&mut app).await;
        app.press(K::Down, M::NONE);
        for _ in 0..3 {
            app.press(K::Down, M::NONE);
        }
        // Enter on b.rs's first match (b.rs not open: loading), then F4 to its second before
        // the load is taken in (`press` handles no background results).
        app.press(K::Enter, M::NONE);
        app.press(K::F(4), M::NONE);
        assert_eq!(at(&mut app).0, "a.txt", "still loading");
        showing(&mut app, "src/b.rs").await;
        assert_eq!(at(&mut app), ("src/b.rs".into(), (19, 25)));
        assert_eq!(app.ui.focus, Focus::Editor, "Enter's focus kept");
        assert_eq!(app.group().tabs.len(), 2, "loaded once");

        // The other way round: F4 (no focus) to the second match, then Enter on the first.
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        needle(&mut app).await;
        app.press(K::Down, M::NONE);
        for _ in 0..3 {
            app.press(K::Down, M::NONE);
        }
        app.press(K::F(4), M::NONE);
        assert_eq!(selected_row(&app), Some(4));
        app.press(K::Up, M::NONE);
        app.press(K::Enter, M::NONE);
        showing(&mut app, "src/b.rs").await;
        assert_eq!(at(&mut app), ("src/b.rs".into(), (3, 9)));
        assert_eq!(app.ui.focus, Focus::Editor, "Enter's focus added");
    }

    /// A match far down a file is centred when opened, whether the file is loading or open.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_an_opened_match_is_centred() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        let mut text: String = (0..300).map(|i| format!("line {i}\n")).collect();
        text.push_str("the needle far down\n");
        text.extend((0..300).map(|i| format!("more {i}\n")));
        write(&root, "zlong.txt", text);
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("needle");
        settled(&mut app).await;
        app.press(K::Esc, M::NONE);
        let shown = |term: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            // Right of the side panel (whose results list shows the line too).
            let buf = term.backend().buffer();
            (0..30).find(|&y| {
                (33..100)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains("the needle far down")
            })
        };
        // Rows 1..=28 are the editor: centred is around row 14, not at an edge.
        let centred = |term: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            let y = shown(term).expect("the match is on screen");
            assert!((10..=18).contains(&y), "row {y}");
        };
        // zlong.txt is last in the list: Shift+F4 from nothing goes there (loading it).
        app.press(K::F(4), M::SHIFT);
        showing(&mut app, "zlong.txt").await;
        assert_eq!(selected_text(&mut app), "needle");
        draw(&mut term, &mut app);
        centred(&term);

        // Scrolled away, then back to it with the file open.
        app.press(K::Home, cmd());
        draw(&mut term, &mut app);
        assert_eq!(shown(&term), None);
        app.press(K::F(4), M::NONE); // wraps to a.txt
        assert_eq!(at(&mut app).0, "a.txt");
        app.press(K::F(4), M::SHIFT);
        assert_eq!(at(&mut app).0, "zlong.txt");
        draw(&mut term, &mut app);
        centred(&term);
    }

    /// A result whose file changed on disk since the search opens clamped: past the last line,
    /// past the line's end, inside a multibyte character.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_stale_results_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        write(&root, "stale.txt", "a\nb\nxneedle\n");
        write(&root, "wide.txt", "zzzneedle\n");
        find_in_files(&mut app);
        app.type_text("needle");
        settled(&mut app).await;
        write(&root, "stale.txt", "é\n");
        write(&root, "wide.txt", "éé\n");
        app.press(K::Esc, M::NONE);
        // The last match is wide.txt's; Shift+F4 from nothing goes there.
        app.press(K::F(4), M::SHIFT);
        showing(&mut app, "wide.txt").await;
        assert_eq!(
            at(&mut app),
            ("wide.txt".into(), (2, 4)),
            "3..9: the start inside the second é moves back to its start, the end cut to the line"
        );
        app.press(K::F(4), M::SHIFT);
        showing(&mut app, "stale.txt").await;
        assert_eq!(
            at(&mut app),
            ("stale.txt".into(), (3, 3)),
            "line 3 is now the last (empty) line"
        );

        // An open buffer edited since: the same clamping, from the buffer's text.
        app.press(K::Char('a'), cmd());
        app.type_text("x");
        app.press(K::F(4), M::NONE);
        app.press(K::F(4), M::SHIFT);
        assert_eq!(at(&mut app), ("stale.txt".into(), (1, 1)));
    }

    /// Results streaming in above the selected row don't move the selection to another result.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::single_range_in_vec_init)] // one match
    async fn milestone_7_streaming_results_keep_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _root) = open_app(&dir);
        let mut term = term();
        find_in_files(&mut app);
        app.type_text("needle");
        // The search starts; its batches wait in the channel while a first batch is taken in.
        app.search_tick(Instant::now() + DEBOUNCE);
        let generation = app.search.as_ref().unwrap().generation;
        app.test_search_batch(Batch {
            generation,
            files: vec![FileMatches {
                path: "m.txt".into(),
                lines: vec![LineMatch {
                    line: 4,
                    ranges: vec![0..6],
                    text: "needle in m".into(),
                    text_start: 0,
                }],
            }],
            done: false,
            truncated: false,
        });
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        assert_eq!(selected_row(&app), Some(1));
        settled(&mut app).await;
        assert_eq!(app.search.as_ref().unwrap().total, 4);
        assert_eq!(selected_row(&app), Some(3), "a.txt's rows came in above");
        let r = app.search.as_ref().unwrap();
        let t = app.search_view.target(r).unwrap();
        assert_eq!((t.path.to_str(), t.line, t.range), (Some("m.txt"), 4, 0..6));
        draw(&mut term, &mut app);
        let (x, y) = find(&term, " in m").unwrap();
        assert_eq!(
            Some(term.backend().buffer()[(x, y)].bg),
            theme_bg(&app, "list.active"),
            "{:?}",
            panel(&term)
        );
    }
}

/// The milestone's "done when": the three modes agree with an independent reference (what `rg
/// -F -i` / `rg -F -s` / `rg` report), an unsaved edit shows up and opens, and typing never
/// stalls (with `view::milestone_7_typing_in_the_query_never_stalls`).
mod done_when {
    use std::path::Path;
    use std::time::Instant;

    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::view::{KEYSTROKE_BUDGET, KEYSTROKE_DEBUG_BOUND, big_workspace, draw, find_in_files, settled, toggle};
    use super::{open_app, write};
    use crate::app::{App, Focus};
    use crate::buffer::Buffer;
    use crate::milestone_tests::test_app;
    use crate::search::Options;
    use crate::workspace_search::reference::{hits, reference};

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// Types `pattern` into the Search view with the toggles set to `o`, and waits for the
    /// results.
    async fn search(app: &mut App, pattern: &str, o: Options) {
        find_in_files(app);
        if app.search_view.options.case_sensitive != o.case_sensitive {
            app.press(K::Char('c'), toggle());
        }
        if app.search_view.options.regex != o.regex {
            app.press(K::Char('r'), toggle());
        }
        app.type_text(pattern);
        settled(app).await;
        assert_eq!(app.search_view.query.text, pattern);
    }

    /// This repo's `src/` and `docs/`, copied so edits made while the test runs can't flake it,
    /// searched through the Search view in each mode, give exactly the reference's hits.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_three_modes_agree_with_the_reference() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        copy_dir(&repo.join("src"), &root.join("src"));
        copy_dir(&repo.join("docs"), &root.join("docs"));
        let mut app = test_app(&root, Buffer::open(&root.join("src/main.rs")).unwrap());
        app.test_start();
        app.test_plain_whitespace();
        let o = |regex, case_sensitive| Options {
            regex,
            case_sensitive,
            whole_word: false,
        };
        let cases = [
            // Plain text, case-insensitive (the default): `rg -F -i`.
            ("fn", o(false, false)),
            ("selections.", o(false, false)),
            // Plain, case-sensitive: `rg -F -s`.
            ("Matcher", o(false, true)),
            ("(x)", o(false, true)),
            // Regex: `rg`, `rg -i`.
            (r"fn \w+\(", o(true, true)),
            (r"[A-Z][a-z]+Error\b", o(true, true)),
            (r"^\s*//!.*search", o(true, false)),
        ];
        for (pattern, options) in cases {
            search(&mut app, pattern, options).await;
            let r = app.search.as_ref().unwrap();
            let want = reference(&root, pattern, options);
            assert!(!want.is_empty() && !r.truncated, "{pattern:?}: {}", want.len());
            assert_eq!(hits(r), want, "{pattern:?} {options:?}");
        }
    }

    /// An unsaved edit is searched from the buffer, and opening it selects the unsaved text.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_an_unsaved_edit_shows_up_and_opens() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        app.press(K::End, M::NONE);
        app.type_text(" haystack");
        assert!(app.active_buffer().unwrap().is_dirty());
        // Another tab in front, so opening the result has to switch back.
        app.test_open(Buffer::open(&root.join("src/c.rs")).unwrap());
        search(&mut app, "HAYSTACK", Options::default()).await;
        let r = app.search.as_ref().unwrap();
        assert_eq!(hits(r), ["a.txt:1:11-19"]);
        assert_eq!(r.files[0].lines[0].text, "first line haystack");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "first line\nsecond needle\n"
        );
        app.press(K::Down, M::NONE);
        app.press(K::Down, M::NONE);
        app.press(K::Enter, M::NONE);
        assert_eq!(app.ui.focus, Focus::Editor);
        let (buf, view) = app.active_mut().unwrap();
        assert_eq!(buf.path, root.join("a.txt"));
        let s = view.selections.primary();
        assert_eq!((s.anchor.0, s.head.0), (11, 19));
        assert_eq!(buf.rope.slice(s.start().0..s.end().0).to_string(), "haystack");
    }

    /// A burst of keys typed while a big search runs is handled within a frame (release builds),
    /// and once it settles the results are the final query's alone: every hit the reference's.
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_7_a_burst_of_keys_during_a_search_never_stalls() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir);
        let size = big_workspace(&root);
        write(&root, "big/other.rs", "needle_4_12 here too\n");
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        find_in_files(&mut app);
        app.type_text("needle");
        settled(&mut app).await;
        // Enter searches again now: the burst lands while that search runs.
        app.press(K::Enter, M::NONE);
        assert!(!app.search.as_ref().unwrap().done);
        let t = Instant::now();
        app.type_text("_4_12");
        app.pump();
        draw(&mut term, &mut app);
        let burst = t.elapsed();
        println!("search view: 5-key burst + frame during a search ({size} bytes): {burst:?}");
        if cfg!(debug_assertions) {
            assert!(burst < KEYSTROKE_DEBUG_BOUND, "{burst:?}");
        } else {
            assert!(burst < KEYSTROKE_BUDGET, "{burst:?} > {KEYSTROKE_BUDGET:?}");
        }
        settled(&mut app).await;
        let r = app.search.as_ref().unwrap();
        assert_eq!(r.query.pattern, "needle_4_12");
        let want = reference(&root, "needle_4_12", Options::default());
        assert!(want.len() > 10 && want.iter().any(|h| h.starts_with("big/other.rs")));
        assert_eq!(hits(r), want, "no hit of an earlier query");
    }
}
