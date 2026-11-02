//! Code review follow-ups, subtask a: a mouse move redraws only when something it shows changed.
//! Crossterm's mouse capture reports every pointer move, so 100 moves over plain editor text ask
//! for no frame at all; and each hover effect a move still has (Source Control row buttons, a
//! Search result's `×`, the scroll-bar thumb colour, closing the hover popup by moving away, the
//! tab tooltip) asks for one, with the drawn frame showing the change.

use crossterm::event::MouseEventKind as E;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::app::App;
use crate::buffer::Buffer;
use crate::layout::{GroupId, ScrollTarget};
use crate::milestone_tests::test_app;

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// An app on `n` numbered lines in `a.txt`, drawn once.
fn long_file_app(dir: &tempfile::TempDir, n: usize) -> App {
    let root = crate::workspace::normalize(dir.path());
    let text: String = (0..n).map(|i| format!("line number {i}\n")).collect();
    std::fs::write(root.join("a.txt"), text).unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    draw(&mut app);
    app
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moves_over_plain_text_ask_for_no_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 50);
    let text = app.hits.group(app.groups.active).unwrap().text;
    app.mouse(E::Moved, text.x + 3, text.y + 2);
    draw(&mut app);
    app.test_take_redraw();
    for i in 0..100u16 {
        app.mouse(E::Moved, text.x + 2 + i % 6, text.y + 2 + i % 3);
    }
    assert!(!app.test_take_redraw(), "100 moves over text: no frame");
}

/// The thumb's cells' colours as drawn.
fn thumb_cells(app: &App, term: &Terminal<TestBackend>, g: GroupId) -> Vec<(Color, Color)> {
    let bar = app.hits.scrollbar(ScrollTarget::Group(g)).expect("a bar");
    let th = bar.thumb.expect("a thumb");
    let buf = term.backend().buffer();
    (th.start..th.start + th.len)
        .map(|d| {
            let c = &buf[(bar.track.x, bar.track.y + d)];
            (c.fg, c.bg)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_onto_the_scrollbar_thumb_redraws_with_the_hover_colour() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 200);
    let g = app.groups.active;
    let bar = app.hits.scrollbar(ScrollTarget::Group(g)).unwrap();
    let thumb = bar.thumb.unwrap();
    let text = app.hits.group(g).unwrap().text;
    app.mouse(E::Moved, text.x + 3, text.y + 1);
    let t = draw(&mut app);
    let before = thumb_cells(&app, &t, g);
    app.test_take_redraw();
    app.mouse(E::Moved, bar.track.x, bar.track.y + thumb.start);
    assert!(app.test_take_redraw(), "onto the thumb");
    let t = draw(&mut app);
    let over = thumb_cells(&app, &t, g);
    assert_ne!(before, over, "the thumb is drawn in its hover colour");
    app.mouse(E::Moved, text.x + 3, text.y + 1);
    assert!(app.test_take_redraw(), "off the thumb");
    let t = draw(&mut app);
    assert_eq!(thumb_cells(&app, &t, g), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_over_a_source_control_row_redraws_with_its_buttons() {
    use crate::milestone10_tests::{ctrl_shift_g, dirs, file, scm_row, settle_git};
    use crate::scm::Section;
    let (_d, root, state) = dirs();
    crate::milestone10_tests::scm_repo(&root);
    let ws = crate::workspace::Workspace::load(root.clone());
    let trust = crate::workspace::trust::TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app.test_plain_whitespace();
    assert!(settle_git(&mut app, |s| s.entries.len() == 5).await);
    ctrl_shift_g(&mut app);
    let key = file(Section::Changes, "a.txt");
    let (x, y) = scm_row(&mut app, &key);
    let has_button = |app: &App| app.hits.scm.as_ref().unwrap().buttons.iter().any(|(_, k, _)| *k == key);
    assert!(!has_button(&app), "no pointer: no button on an unselected row");
    app.test_take_redraw();
    app.mouse(E::Moved, x, y);
    assert!(app.test_take_redraw(), "onto the row");
    draw(&mut app);
    assert!(has_button(&app), "the + / − button is drawn");
    app.mouse(E::Moved, x, y);
    assert!(!app.test_take_redraw(), "same row again: nothing changed");
    app.mouse(E::Moved, 0, 0);
    assert!(app.test_take_redraw(), "off the row");
    draw(&mut app);
    assert!(!has_button(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_over_a_search_result_redraws_with_its_cross() {
    use crate::replace_tests::{draw as rdraw, key, open_app, search_for, term};
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut t = term();
    search_for(&mut app, "foo").await;
    rdraw(&mut t, &mut app);
    let list = app.hits.search.as_ref().unwrap().list;
    let cross = |app: &App| !app.hits.search.as_ref().unwrap().dismiss.is_empty();
    assert!(!cross(&app));
    app.test_take_redraw();
    app.mouse(E::Moved, list.x + 5, list.y);
    assert!(app.test_take_redraw(), "onto a result");
    rdraw(&mut t, &mut app);
    assert!(cross(&app), "its × is drawn");
    assert_eq!(app.search_view.hover, Some(key("a.txt", 0)));
    app.mouse(E::Moved, 0, 0);
    assert!(app.test_take_redraw(), "off it");
    rdraw(&mut t, &mut app);
    assert!(!cross(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_moving_off_a_hover_popup_target_closes_it_with_a_frame() {
    use crate::fixes_tests::{cell, fixes_app, rest, terminal};
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = fixes_app(dir.path(), true).await;
    let mut term = terminal();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let at = cell(&app, 6, 5);
    assert!(rest(&mut app, &mut term, at));
    assert!(app.hover.popup.is_some());
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.hover.is_some(), "the popup is drawn");
    app.test_take_redraw();
    let away = cell(&app, 6, 3);
    app.mouse(E::Moved, away.0, away.1);
    assert!(app.hover.popup.is_none());
    assert!(app.test_take_redraw(), "closing the popup asks for a frame");
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.hover.is_none(), "and the frame has no popup");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_the_tab_tooltip_shows_after_its_rest_and_goes_with_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = long_file_app(&dir, 5);
    let tab = app.hits.group(app.groups.active).unwrap().tab_bar;
    let (x, y) = (tab.x + 2, tab.y);
    let drawn = |app: &mut App| crate::milestone10_tests::find_text(&draw(app), "a.txt").len();
    let plain = drawn(&mut app);
    app.test_take_redraw();
    app.mouse(E::Moved, x, y);
    assert!(app.tab_tip.deadline().is_some(), "the timer runs");
    assert!(!app.test_take_redraw(), "starting the rest needs no frame");
    let deadline = app.tab_tip.deadline().unwrap();
    app.test_tab_tip_tick(deadline);
    assert!(app.test_take_redraw(), "the tooltip appears");
    assert!(app.tab_tip.shown.is_some());
    assert!(drawn(&mut app) > plain, "the frame shows the tooltip");
    app.mouse(E::Moved, x, y);
    assert!(!app.test_take_redraw(), "still on the tab");
    app.mouse(E::Moved, tab.x + 2, tab.y + 5);
    assert!(app.test_take_redraw(), "moving off drops it");
    assert_eq!(drawn(&mut app), plain);
}

// Subtask b: a long unwrapped line scrolled far right is drawn from the scroll column, not from
// column 0.

/// Minified-JS-like text of one line, `size` bytes; with `multibyte`, an "é世" every 1 KB so the
/// printable-ASCII fast paths of the column helpers don't apply to the whole line.
fn minified_line(size: usize, multibyte: bool) -> String {
    let mut s = String::with_capacity(size + 100);
    let mut i = 0;
    while s.len() < size {
        s.push_str(&format!(
            "function f{i}(a,b){{return a+b*{i}}};var v{i}=[1,2,3].map(x=>x*{i});"
        ));
        if multibyte && i % 12 == 0 {
            s.push_str("é世");
        }
        i += 1;
    }
    s
}

/// Frames and keystrokes with the cursor at column ~900,000 of a 1 MB unwrapped line.
async fn hscroll_budget(multibyte: bool) {
    use crate::buffer::pos::{self, ByteIdx};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::{Duration, Instant};
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let at = size * 9 / 10;
    let text = minified_line(size, multibyte);
    assert!(!text.contains('\n'));
    let (_d, root, state) = crate::milestone10_tests::dirs();
    std::fs::write(root.join("a.txt"), &text).unwrap();
    let mut app = crate::milestone10_tests::open_file_app(&root, &state, "a.txt");
    app.ui.side_visible = false;
    let mut term = Terminal::new(TestBackend::new(160, 50)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let at = text.floor_char_boundary(at);
    crate::milestone9_tests::put_cursor(&mut app, at);
    let (buf, view) = app.active_mut().unwrap();
    let col = pos::display_col(&buf.rope, ByteIdx(at), view.tab_width).0;
    view.left_col = col - 40;
    let left = view.left_col;
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert_eq!(
        app.groups
            .get(app.groups.active)
            .unwrap()
            .active_tab()
            .unwrap()
            .view
            .left_col,
        left
    );

    // Frames alone.
    let n = 5u32;
    let t = Instant::now();
    for _ in 0..n {
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let frame = t.elapsed() / n;
    // 50 typed characters, each with the frame after it.
    let t = Instant::now();
    for _ in 0..50 {
        app.press(KeyCode::Char('y'), KeyModifiers::NONE);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let key = t.elapsed() / 50;
    println!("1 MB line scrolled right (multibyte {multibyte}): frame {frame:?}, key+frame {key:?}");
    assert!(
        app.active_buffer().unwrap().rope.len() >= size + 50,
        "the keys were typed"
    );
    assert!(key < Duration::from_secs(2), "typing too slow: {key:?}");
    if !cfg!(debug_assertions) {
        assert!(key < Duration::from_millis(16), "typing too slow: {key:?}");
        assert!(frame < Duration::from_millis(16), "frame too slow: {frame:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_hscroll_budget_1mb_line_scrolled_right() {
    hscroll_budget(false).await;
    hscroll_budget(true).await;
}

/// A row drawn at `left_col` is the same line drawn from column 0 with its first `left_col`
/// columns cut off: tabs, a wide CJK char and a combining mark around the left edge.
#[tokio::test(flavor = "multi_thread")]
async fn review_followups_hscroll_row_matches_the_line_drawn_from_column_0() {
    let line = "ab\tcd日本語e\u{301}fg\t\th日i\u{301}\u{302}j日日kl\tmnop日qr";
    let text = format!("{line}\nsecond\n");
    let (_d, root, state) = crate::milestone10_tests::dirs();
    std::fs::write(root.join("a.txt"), &text).unwrap();
    let mut app = crate::milestone10_tests::open_file_app(&root, &state, "a.txt");
    app.ui.side_visible = false;
    let h = 10u16;
    // Selections that start left of, inside and past the visible region, one ending past the line
    // end, plus a secondary cursor at the line end and one on a wide char.
    {
        use crate::buffer::pos::ByteIdx;
        use crate::selection::{Selection, Selections};
        let wide = line.find('日').unwrap();
        let list = vec![
            Selection::new(ByteIdx(3), ByteIdx(wide + 3)),
            Selection::cursor(ByteIdx(wide + 3 * '日'.len_utf8())),
            Selection::new(ByteIdx(line.len() - 4), ByteIdx(line.len() + 3)),
        ];
        let (_, view) = app.active_mut().unwrap();
        view.selections = Selections::new(list, 1);
    }
    let row = |app: &mut App, w: u16| -> Vec<Vec<(String, String)>> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let t = app.hits.group(app.groups.active).unwrap().text;
        let buf = term.backend().buffer();
        // The long line and the short one under it (empty once scrolled past its end).
        (t.y..t.y + 2)
            .map(|y| {
                (t.x..t.x + t.width)
                    .map(|x| {
                        let cell = &buf[(x, y)];
                        let s = cell.symbol().to_string();
                        (
                            if s.is_empty() { " ".to_string() } else { s },
                            format!("{:?}", cell.style()),
                        )
                    })
                    .collect()
            })
            .collect()
    };
    let full = row(&mut app, 120);
    let width = crate::buffer::pos::col_after(line, crate::buffer::pos::DisplayCol(0), 4).0;
    assert!(width > 30, "the line is wide: {width}");
    let mut compared = 0;
    for left in 1..width {
        let (_, view) = app.active_mut().unwrap();
        view.left_col = left;
        let got = row(&mut app, 30);
        let applied = app
            .groups
            .get(app.groups.active)
            .unwrap()
            .active_tab()
            .unwrap()
            .view
            .left_col;
        if applied != left {
            // The scroll was clamped to the widest line.
            continue;
        }
        // Not the last cell: a wide char cut by the right edge is blank there.
        for (got, full) in got.iter().zip(&full) {
            let n = got.len() - 1;
            // The first cell may be the tail of a wide char cut by the left edge: drawn as a
            // blank (with the selection colour when selected), where the full row leaves it
            // untouched.
            let (got, want) = (&got[..n], &full[left..left + n]);
            assert_eq!(got[0].0, want[0].0, "left_col {left}");
            assert_eq!(got[1..], want[1..], "left_col {left}");
        }
        compared += 1;
    }
    assert!(compared > 20, "compared {compared} scroll columns");
}

/// Subtask c: the explorer caches its flattened rows, so a frame and a key press walk no tree.
#[tokio::test(flavor = "multi_thread")]
async fn review_followups_explorer_frames_and_keys_walk_no_tree() {
    use crate::app::Focus;
    use crate::explorer::Entry;
    use crossterm::event::{KeyCode, KeyModifiers};

    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    // 50,000 rows without touching the disk: 500 expanded folders of 99 files each.
    let entry = |dir: &std::path::Path, name: String, is_dir: bool| Entry {
        path: dir.join(&name),
        name,
        is_dir,
        expanded: is_dir,
        ignored: false,
        children: None,
    };
    let folders: Vec<Entry> = (0..500).map(|i| entry(&root, format!("d{i:03}"), true)).collect();
    app.explorer.set_children(&root, folders);
    for i in 0..500 {
        let d = root.join(format!("d{i:03}"));
        let files = (0..99).map(|j| entry(&d, format!("f{j:02}.txt"), false)).collect();
        app.explorer.set_children(&d, files);
    }
    assert_eq!(app.explorer.row_count(), 50_000);
    app.ui.focus = Focus::SidePanel;
    draw(&mut app);
    assert!(app.explorer.walks.get() > 0, "the walk counter counts the first build");
    app.explorer.walks.set(0);
    app.explorer.lookups.set(0);
    draw(&mut app);
    for _ in 0..10 {
        app.press(KeyCode::Down, KeyModifiers::NONE);
    }
    draw(&mut app);
    assert_eq!(app.explorer.walks.get(), 0, "no full tree walk after the first frame");
    assert_eq!(app.explorer.lookups.get(), 0, "no linear search for the selected row");
    assert_eq!(app.explorer.selected_index_for_test(), Some(9));
}

/// Subtask d: secondary cursors come from a walk over the selections' heads (no per-frame set):
/// the cells drawn match where the cursors are, on unwrapped and wrapped rows, at a row's last char,
/// at a line end and on an empty line; and the token style table is the theme's, not rebuilt.
fn underlined(app: &App, term: &Terminal<TestBackend>) -> Vec<(u16, u16)> {
    use ratatui::style::Modifier;
    let t = app.hits.group(app.groups.active).unwrap().text;
    let buf = term.backend().buffer();
    let mut out = Vec::new();
    for y in t.y..t.y + t.height {
        for x in t.x..t.x + t.width {
            if buf[(x, y)].modifier.contains(Modifier::UNDERLINED) {
                out.push((x - t.x, y - t.y));
            }
        }
    }
    out
}

fn set_cursors(app: &mut App, heads: &[usize]) {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let list = heads.iter().map(|h| Selection::cursor(ByteIdx(*h))).collect();
    app.active_mut().unwrap().1.selections = Selections::new(list, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_followups_secondary_cursors_are_drawn_where_the_heads_are() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    let w = 100usize;
    let long = "a".repeat(2 * w + 5);
    std::fs::write(root.join("a.txt"), format!("abcd\n\nxy\n{long}\n")).unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    app.ui.side_visible = false;
    // Heads: abcd 0 (primary), 3, 4 (line end); empty line 5; xy: 7 (on `y`); the long line starts at 9.
    set_cursors(&mut app, &[0, 3, 4, 5, 7]);
    let term = draw(&mut app);
    assert_eq!(underlined(&app, &term), vec![(3, 0), (4, 0), (0, 1), (1, 2)]);

    // Wrapped: the text width decides the rows; cursors on a row's last char, the next row's first,
    // and the line end (one past the last char).
    app.test_execute(crate::command::Command::ToggleWordWrap);
    draw(&mut app);
    let tw = app.hits.group(app.groups.active).unwrap().text.width as usize;
    let start = 9;
    let len = long.len();
    let last_of_first = start + tw - 1;
    set_cursors(&mut app, &[0, last_of_first, last_of_first + 1, start + len]);
    let term2 = draw(&mut app);
    let got = underlined(&app, &term2);
    let top = got.iter().map(|c| c.1).min().unwrap();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[0], ((tw - 1) as u16, top), "a row's last char");
    assert_eq!(got[1], (0, top + 1), "the next row's first char");
    assert_eq!(got[2], ((len % tw) as u16, top + (len / tw) as u16), "the line end");
}

#[test]
fn review_followups_token_styles_are_built_with_the_theme() {
    let theme = crate::theme::Theme::dark_plus(true);
    let a = theme.token_styles().as_ptr();
    assert_eq!(theme.token_styles().len(), crate::syntax::KEYS.len());
    assert_eq!(a, theme.token_styles().as_ptr(), "the same table every frame");
}

/// A cursor on every line, with selections (forward ones ending at the line end, backward ones headed
/// at their start) walk in order: the underlined cells are exactly the heads but the primary's.
#[tokio::test(flavor = "multi_thread")]
async fn review_followups_secondary_cursors_on_every_line_with_selections() {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    let n = 200usize;
    std::fs::write(root.join("a.txt"), "abc\n".repeat(n)).unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    app.ui.side_visible = false;
    let list = (0..n)
        .map(|i| {
            let s = 4 * i;
            if i % 2 == 0 {
                Selection::new(ByteIdx(s + 1), ByteIdx(s + 3))
            } else {
                Selection::new(ByteIdx(s + 3), ByteIdx(s + 1))
            }
        })
        .collect();
    app.active_mut().unwrap().1.selections = Selections::new(list, 0);
    let term = draw(&mut app);
    let rows = app.hits.group(app.groups.active).unwrap().text.height as usize;
    let want: Vec<(u16, u16)> = (1..rows).map(|i| (if i % 2 == 0 { 3 } else { 1 }, i as u16)).collect();
    assert_eq!(underlined(&app, &term), want);
    // The same with wrapping on (rows drawn through the wrapped path).
    app.test_execute(crate::command::Command::ToggleWordWrap);
    let term = draw(&mut app);
    assert_eq!(underlined(&app, &term), want);
    // A backward selection over the line break whose head is the line end: the end cell is the cursor.
    let list = (0..n)
        .filter(|i| i % 3 == 0)
        .map(|i| Selection::new(ByteIdx(4 * i + 4), ByteIdx(4 * i + 3)))
        .collect();
    app.active_mut().unwrap().1.selections = Selections::new(list, 0);
    app.test_execute(crate::command::Command::ToggleWordWrap);
    let term = draw(&mut app);
    let want: Vec<(u16, u16)> = (1..rows).filter(|i| i % 3 == 0).map(|i| (3, i as u16)).collect();
    assert_eq!(underlined(&app, &term), want);
}

/// The current-match colour goes only to a match that is exactly the primary selection: a selection
/// sharing just a match's start (here "abc" over the match "ab") is drawn as a selection, and the
/// other match gets the ordinary highlight.
#[tokio::test(flavor = "multi_thread")]
async fn review_followups_current_match_is_only_the_exact_primary_selection() {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    std::fs::write(root.join("a.txt"), "abc ab\n").unwrap();
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    app.ui.side_visible = false;
    app.test_execute(crate::command::Command::OpenFind);
    app.type_text("ab");
    assert_eq!(app.find.query.text, "ab");
    let set = |app: &mut App, a: usize, h: usize| {
        app.active_mut().unwrap().1.selections = Selections::single(Selection::new(ByteIdx(a), ByteIdx(h)));
    };
    let (current, other, selection) = (
        app.theme.get("editor.findMatch").bg,
        app.theme.get("editor.findMatchHighlight").bg,
        app.theme.get("editor.selection").bg,
    );
    assert!(current != selection && other != selection && current != other);
    let bgs = |app: &App, term: &Terminal<TestBackend>| -> Vec<Option<Color>> {
        let t = app.hits.group(app.groups.active).unwrap().text;
        (0..6)
            .map(|x| Some(term.backend().buffer()[(t.x + x, t.y)].bg))
            .collect::<Vec<_>>()
    };

    set(&mut app, 0, 3);
    let term = draw(&mut app);
    let b = bgs(&app, &term);
    assert_eq!(
        &b[0..3],
        &[selection; 3],
        "selection \"abc\" shares only the start with the match"
    );
    assert_eq!(&b[4..6], &[other; 2], "the other match is the ordinary highlight");

    set(&mut app, 4, 6);
    let term = draw(&mut app);
    let b = bgs(&app, &term);
    assert_eq!(&b[4..6], &[current; 2], "the exactly selected match is the current one");
    assert_eq!(&b[0..2], &[other; 2]);
}
