//! Hex view, large files (hex_more-b): a file over `RESIDENT_MAX` opens as a paged hex tab, whatever
//! its size. The first frame draws placeholders for the bytes that aren't read yet and queues
//! their pages; they arrive off the UI thread (`ViewEvent::HexPages`) and the next frame shows
//! them; Go to Offset far in loads that page; the status bar's size in units; find next /
//! previous / count on a paged doc run off the UI thread ("…" meanwhile, a newer query or a stale
//! result dropped); Cmd+C reads a paged selection off the UI thread and refuses one over
//! `HEX_COPY_MAX`; a file changed or truncated on disk drops the page cache and clamps the
//! cursor, one truncated without a disk check draws unreadable pages once; a paged tab kept by
//! the session; a 5 GiB sparse file (offsets of 9 digits, a marker past 4 GiB found by Go to
//! Offset and by find next / previous across the 4 GiB boundary, its count cancelled by a new
//! query). Files are sparse (`set_len`), so they cost no disk.

use std::fs;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus};
use crate::hex::{self, RESIDENT_MAX};
use crate::hex_tests::{
    cmd, drawn_rows, find, go_to_offset, hex, quick_open, quit, rows_rect, settle_count, state_app, status_bar,
};
use crate::milestone10_tests::dirs;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A bit over `RESIDENT_MAX`: paged.
const BIG: u64 = RESIDENT_MAX + (1 << 20) + 1;

fn sparse(path: &Path, len: u64, marks: &[(u64, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    f.set_len(len).unwrap();
    for (at, bytes) in marks {
        f.write_all_at(bytes, *at).unwrap();
    }
}

/// An app on a workspace with `big.bin` (a sparse file of `len` bytes with `marks` written), no
/// tab open, the side bar hidden.
async fn big_app(len: u64, marks: &[(u64, &[u8])], watch: bool) -> (tempfile::TempDir, PathBuf, App) {
    let (dir, root, state) = dirs();
    sparse(&root.join("big.bin"), len, marks);
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    if watch {
        app.enable_file_watching();
    }
    app.test_start();
    // The data inspector (`hex_more_c_*` tests turn it on) would narrow the rows.
    app.hex_inspector = false;
    app.ui.side_visible = false;
    app.ui.focus = Focus::Editor;
    (dir, root, app)
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    crate::hex_tests::draw(app, 100, 12)
}

fn rows(app: &App, term: &Terminal<TestBackend>) -> Vec<String> {
    drawn_rows(term, rows_rect(app))
}

/// Draws until no placeholder is left (each frame asks for its pages, which the settle reads).
async fn draw_loaded(app: &mut App) -> Terminal<TestBackend> {
    for _ in 0..100 {
        let term = draw(app);
        if !rows(app, &term).iter().any(|r| r.contains('\u{b7}')) {
            return term;
        }
        app.needs_redraw = false;
        assert!(app.settle(|a| a.needs_redraw).await, "the pages arrive");
    }
    panic!("pages never loaded");
}

/// Waits for the clipboard to hold something other than `previous` (a paged copy is read off the UI thread).
async fn wait_copied(app: &mut App, previous: &str) -> String {
    for _ in 0..200 {
        if let Ok(t) = app.clipboard.read()
            && t != previous
        {
            return t;
        }
        app.needs_redraw = false;
        assert!(app.settle(|a| a.needs_redraw).await);
    }
    panic!("nothing copied");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_a_big_file_opens_paged_with_placeholders_then_its_bytes() {
    let (_d, _root, mut app) = big_app(BIG, &[(1, b"JEDHEX!")], false).await;
    quick_open(&mut app, "big.bin").await;
    let h = hex(&app);
    assert!(h.doc.resident_bytes().is_none(), "paged");
    assert_eq!((h.len() as u64, h.doc.cached_bytes()), (BIG, 0), "nothing read at open");

    // The first frame: placeholders, nothing read on the UI thread.
    let term = draw(&mut app);
    let first = rows(&app, &term);
    assert!(
        first[0].starts_with("00000000  \u{b7}\u{b7} \u{b7}\u{b7} "),
        "{first:?}"
    );
    assert!(first[0].ends_with(
        "|\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}|"
    ));
    assert!(
        !status_bar(&term).contains("Offset"),
        "the byte isn't known yet: {}",
        status_bar(&term)
    );
    assert_eq!(hex(&app).doc.cached_bytes(), 0);

    // Then the bytes.
    let term = draw_loaded(&mut app).await;
    let loaded = rows(&app, &term);
    assert!(loaded[0].starts_with("00000000  00 4a 45 44 48 45 58 21"), "{loaded:?}");
    assert!(loaded[0].ends_with("|.JEDHEX!........|"), "{loaded:?}");
    let cached = hex(&app).doc.cached_bytes();
    assert!(cached > 0 && cached <= hex::CACHE_MAX);
    let bar = status_bar(&term);
    assert!(
        bar.contains("Offset 0x0 (0)") && bar.contains("9.0 MB (9437185 bytes)"),
        "{bar}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_go_to_offset_far_in_loads_that_page() {
    let (_d, _root, mut app) = big_app(BIG, &[(0x70_0020, b"FARAWAY")], false).await;
    quick_open(&mut app, "big.bin").await;
    draw_loaded(&mut app).await;
    go_to_offset(&mut app, "0x700020");
    assert_eq!(hex(&app).cursor, 0x70_0020);
    let term = draw(&mut app);
    let pending = rows(&app, &term);
    assert!(
        pending.iter().any(|r| r.starts_with("00700020  \u{b7}\u{b7}")),
        "{pending:?}"
    );
    let term = draw_loaded(&mut app).await;
    let loaded = rows(&app, &term);
    assert!(
        loaded
            .iter()
            .any(|r| r.starts_with("00700020  46 41 52 41 57 41 59") && r.contains("|FARAWAY")),
        "{loaded:?}"
    );
    assert!(status_bar(&term).contains("Offset 0x700020 (7340064)  ") || status_bar(&term).contains("0x46 70"));
    // Memory stays within the cache's cap however far it scrolls.
    assert!(hex(&app).doc.cached_bytes() <= hex::CACHE_MAX);
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_find_next_previous_and_count_run_off_the_ui_thread() {
    let marks: [(u64, &[u8]); 3] = [(0x10, b"NEEDLE"), (0x50_0000, b"NEEDLE"), (0x86_0000, b"NEEDLE")];
    let (_d, _root, mut app) = big_app(BIG, &marks, false).await;
    quick_open(&mut app, "big.bin").await;
    draw_loaded(&mut app).await;
    find(&mut app, "NEEDLE");
    // Nothing has come back yet: the count is "…", the cursor hasn't moved.
    assert_eq!(app.find.hex.count_label(None), "…");
    assert!(app.settle(|a| hex(a).selection() == Some(0x10..0x16)).await);
    settle_count(&mut app).await;
    let c = app.find.hex.count().unwrap();
    assert_eq!((c.found(), c.capped), (3, false));
    assert_eq!(app.find.hex.count_label(hex(&app).selection()), "1 of 3");

    let select = |want: std::ops::Range<usize>| move |a: &App| hex(a).selection() == Some(want.clone());
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(app.find.hex.count_label(None), "…", "while the search runs");
    assert!(app.settle(select(0x50_0000..0x50_0006)).await);
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(select(0x86_0000..0x86_0006)).await);
    assert_eq!(hex(&app).cursor, 0x86_0005);
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(select(0x10..0x16)).await, "wraps");
    app.press(KeyCode::Enter, M::SHIFT);
    assert!(app.settle(select(0x86_0000..0x86_0006)).await, "previous wraps");
    app.press(KeyCode::Enter, M::SHIFT);
    assert!(app.settle(select(0x50_0000..0x50_0006)).await);
    assert_eq!(app.find.hex.count_label(hex(&app).selection()), "2 of 3");

    // The match is highlighted once its page is loaded.
    let term = draw_loaded(&mut app).await;
    assert!(rows(&app, &term).iter().any(|r| r.contains("|NEEDLE")));

    // A result for an older search is dropped.
    let tab = hex(&app).id;
    let (_, generation) = app.find.hex.begin_step();
    app.test_hex_found(tab, generation - 1, Some(0x10..0x16));
    assert_eq!(hex(&app).selection(), Some(0x50_0000..0x50_0006));
    app.test_hex_found(tab, generation, Some(0x10..0x16));
    assert_eq!(hex(&app).selection(), Some(0x10..0x16));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_copy_reads_off_the_ui_thread_and_refuses_over_the_cap() {
    let len = 20 << 20;
    let (_d, _root, mut app) = big_app(len, &[(1, b"COPYME")], false).await;
    quick_open(&mut app, "big.bin").await;
    app.press(KeyCode::Right, M::NONE);
    for _ in 0..5 {
        app.press(KeyCode::Right, M::SHIFT);
    }
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(wait_copied(&mut app, "").await, "43 4f 50 59 4d 45");

    // Over the cap: a message, nothing copied.
    app.press(KeyCode::Char('a'), cmd());
    app.press(KeyCode::Char('c'), cmd());
    let said = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert_eq!(said, "Selection too big to copy (20 MB, at most 16 MB)");
    assert_eq!(app.clipboard.read().unwrap(), "43 4f 50 59 4d 45", "unchanged");

    // Exactly the cap: copied (the ASCII column, to keep the text short).
    app.press(KeyCode::Tab, M::NONE);
    {
        let h = app.groups.active_group_mut().active_hex_mut().unwrap();
        h.move_to(0, false);
        h.move_to(hex::HEX_COPY_MAX as usize - 1, true);
    }
    app.message = None;
    app.press(KeyCode::Char('c'), cmd());
    assert!(app.message.is_none());
    let text = wait_copied(&mut app, "43 4f 50 59 4d 45").await;
    assert_eq!(text.len() as u64, hex::HEX_COPY_MAX);
    assert!(
        text.starts_with(".COPYME.") || text.starts_with("\u{0}COPYME"),
        "{:?}",
        &text[..8]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_a_changed_file_drops_the_cache_and_a_truncated_one_clamps() {
    let (_d, root, mut app) = big_app(BIG, &[(0x80_0010, b"TAILV1")], true).await;
    quick_open(&mut app, "big.bin").await;
    go_to_offset(&mut app, "0x800010");
    let term = draw_loaded(&mut app).await;
    assert!(rows(&app, &term).iter().any(|r| r.contains("|TAILV1")));
    assert!(hex(&app).doc.cached_bytes() > 0);
    let old = hex(&app).doc.version();

    // Rewritten in place and cut to a bit over the resident limit: still paged, a new version
    // with an empty cache, and no stale byte drawn.
    let f = fs::File::options().write(true).open(root.join("big.bin")).unwrap();
    f.write_all_at(b"TAILV2", 0x80_0010).unwrap();
    f.set_len(RESIDENT_MAX + 0x100).unwrap();
    drop(f);
    assert!(app.settle(|a| hex(a).len() as u64 == RESIDENT_MAX + 0x100).await);
    assert_ne!(hex(&app).doc.version(), old);
    assert_eq!(hex(&app).doc.cached_bytes(), 0, "the cache went with the old doc");
    assert_eq!(hex(&app).cursor, 0x80_0010, "the cursor stays");
    let term = draw_loaded(&mut app).await;
    let shown = rows(&app, &term);
    assert!(shown.iter().any(|r| r.contains("|TAILV2")), "{shown:?}");
    assert!(!shown.iter().any(|r| r.contains("TAILV1")));

    // Cut to 1 MiB: resident now, the cursor on the last byte, nothing of the old file drawn.
    let f = fs::File::options().write(true).open(root.join("big.bin")).unwrap();
    f.set_len(1 << 20).unwrap();
    drop(f);
    assert!(app.settle(|a| hex(a).len() == 1 << 20).await);
    assert!(hex(&app).doc.resident_bytes().is_some());
    assert_eq!(hex(&app).cursor, (1 << 20) - 1);
    let term = draw(&mut app);
    assert!(
        !rows(&app, &term)
            .iter()
            .any(|r| r.contains("TAIL") || r.contains('\u{b7}'))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_a_page_that_cannot_be_read_draws_unreadable_and_is_not_asked_for_again() {
    let (_d, root, mut app) = big_app(BIG, &[(0x80_0010, b"GONE")], false).await;
    quick_open(&mut app, "big.bin").await;
    draw_loaded(&mut app).await;
    // Cut under the open tab with no disk check: reads past the new end fail.
    fs::File::options()
        .write(true)
        .open(root.join("big.bin"))
        .unwrap()
        .set_len(1 << 20)
        .unwrap();
    go_to_offset(&mut app, "0x800010");
    let term = draw_loaded(&mut app).await;
    let shown = rows(&app, &term);
    assert!(shown.iter().any(|r| r.starts_with("00800010  ?? ?? ??")), "{shown:?}");
    assert!(!shown.iter().any(|r| r.contains("GONE")));
    assert!(hex(&app).doc.plan_reads().is_empty(), "not retried every frame");
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_sessions_keep_a_paged_tab() {
    let (_d, root, state) = dirs();
    sparse(&root.join("big.bin"), BIG, &[(0x80_0020, b"KEPT")]);
    let mut app = state_app(&root, &state).await;
    quick_open(&mut app, "big.bin").await;
    go_to_offset(&mut app, "0x800020");
    draw_loaded(&mut app).await;
    quit(app).await;

    let mut app = state_app(&root, &state).await;
    assert!(app.settle(|a| a.active_hex().is_some()).await, "restored");
    let h = hex(&app);
    assert!(h.doc.resident_bytes().is_none());
    assert_eq!((h.cursor, h.len() as u64), (0x80_0020, BIG));
    let term = draw_loaded(&mut app).await;
    assert!(rows(&app, &term).iter().any(|r| r.contains("|KEPT")));
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_a_file_past_4_gib_has_wide_offsets_and_find_crosses_the_boundary() {
    let (a, b) = ((4u64 << 30) - 8, (4u64 << 30) + 8);
    let (_d, _root, mut app) = big_app(5 << 30, &[(a, b"MARKA"), (b, b"MARKB")], false).await;
    quick_open(&mut app, "big.bin").await;
    assert_eq!(hex(&app).len() as u64, 5 << 30);
    go_to_offset(&mut app, "0x100000008");
    assert_eq!(hex(&app).cursor as u64, b);
    let term = draw_loaded(&mut app).await;
    let shown = rows(&app, &term);
    assert!(
        shown
            .iter()
            .any(|r| r.starts_with("100000000  ") && r.contains("4d 41 52 4b 42")),
        "{shown:?}"
    );
    assert!(status_bar(&term).contains("5.0 GB (5368709120 bytes)"));

    find(&mut app, "MARK");
    let at = |want: u64| move |x: &App| hex(x).selection().is_some_and(|s| s.start as u64 == want);
    assert!(app.settle(at(b)).await, "from the cursor, at or after it");
    app.press(KeyCode::Enter, M::SHIFT);
    assert!(app.settle(at(a)).await, "previous crosses 4 GiB");
    let term = draw_loaded(&mut app).await;
    assert!(
        rows(&app, &term)
            .iter()
            .any(|r| r.starts_with("0fffffff0  ") && r.contains("MARKA"))
    );
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(at(b)).await, "next crosses it back");
    // A new query ends the whole-file count of the old one (it would read 5 GiB).
    app.type_text("x");
    assert_eq!(app.find.hex.count_label(None), "…");
    assert!(hex(&app).doc.cached_bytes() <= hex::CACHE_MAX);
    // Emptying the query leaves no count running (the last one would read all 5 GiB).
    for _ in 0..5 {
        app.press(KeyCode::Backspace, M::NONE);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hex_more_closing_the_widget_or_the_tab_ends_the_running_search() {
    let (_d, root, mut app) = big_app(BIG, &[(0x10, b"NEEDLE")], false).await;
    fs::write(root.join("t.txt"), "text\n").unwrap();
    quick_open(&mut app, "big.bin").await;
    find(&mut app, "NEEDLE");
    settle_count(&mut app).await;
    assert!(app.find.hex.query().is_some());
    // Closing the widget forgets the query (and so cancels a count still scanning).
    app.press(KeyCode::Esc, M::NONE);
    assert!(!app.find.visible);
    assert!(app.find.hex.query().is_none() && app.find.hex.count().is_none());
    // Opening it again counts again.
    app.press(KeyCode::Char('f'), cmd());
    settle_count(&mut app).await;
    assert_eq!(app.find.hex.count().unwrap().found(), 1);
    // Another tab in front: the hex search is dropped, not left running.
    quick_open(&mut app, "t.txt").await;
    assert!(app.active_hex().is_none());
    assert!(app.find.hex.query().is_none(), "no hex search behind a text tab");
}

// hex_more-c: the data inspector and byte order.

mod inspector {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::layout::Rect;

    use super::*;
    use crate::command::Command;
    use crate::hex::{ByteOrder, HexDoc, RowKind};
    use crate::hex_tests::{bg, byte_bgs, hex_app, line};

    /// An app with the inspector on and a hex tab (`x.bin`) of `bytes`.
    async fn ins_app(bytes: &[u8]) -> (tempfile::TempDir, App) {
        let (dir, root, mut app) = hex_app().await;
        app.hex_inspector = true;
        app.open_initial_hex(root.join("x.bin"), HexDoc::new(bytes.to_vec()));
        (dir, app)
    }

    fn hit(app: &App) -> crate::layout::HexHit {
        app.hits
            .group(app.groups.active)
            .unwrap()
            .hex
            .clone()
            .expect("the panel is drawn")
    }

    /// The panel's rows as drawn, whitespace collapsed: `u16 513`, `i32 int 67305985`.
    fn panel(app: &App, term: &Terminal<TestBackend>) -> Vec<String> {
        hit(app)
            .rows
            .iter()
            .map(|(r, _)| {
                line(term, r.x, r.y, r.width)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    /// The value shown for `label` (`u16`, `i32 int`).
    fn shown(app: &App, term: &Terminal<TestBackend>, label: &str) -> String {
        let prefix = format!("{label} ");
        panel(app, term)
            .into_iter()
            .find_map(|r| r.strip_prefix(&prefix).map(str::to_owned))
            .unwrap_or_else(|| panic!("no row {label}"))
    }

    fn wide(app: &mut App) -> Terminal<TestBackend> {
        crate::hex_tests::draw(app, 120, 30)
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    fn line_has(term: &Terminal<TestBackend>, r: Rect, text: &str) -> bool {
        line(term, r.x, r.y, r.width).contains(text)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_the_panel_is_beside_the_rows_when_wide_and_under_them_at_80_columns() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(300).collect();
        let (_d, mut app) = ins_app(&bytes).await;
        let term = wide(&mut app);
        let g = app.hits.group(app.groups.active).unwrap();
        let (rows, h) = (g.text, hit(&app));
        assert!(h.rows[0].0.x >= rows.x + rows.width, "beside");
        assert_eq!(h.rows.len(), 14);
        assert_eq!(h.rows[0].0.y, rows.y + 1, "the header is above the first row");
        assert!(line_has(&term, h.toggle, "LE"));
        assert_eq!(rows.height, g.editor.height, "the rows keep the whole height");

        // At 80 columns 8-byte rows and the panel don't fit side by side: it goes under them.
        let term = crate::hex_tests::draw(&mut app, 80, 30);
        let g = app.hits.group(app.groups.active).unwrap();
        let (rows, h) = (g.text, hit(&app));
        assert!(h.rows[0].0.y >= rows.y + rows.height, "under");
        assert!(rows.height < g.editor.height);
        assert_eq!(shown(&app, &term, "u8"), "0");

        // Off: no panel, the whole area for the rows.
        app.test_execute(Command::HexToggleInspector);
        crate::hex_tests::draw(&mut app, 80, 30);
        let g = app.hits.group(app.groups.active).unwrap();
        assert!(g.hex.is_none());
        assert_eq!(g.text.height, g.editor.height);
    }

    /// With the panel on (the default users get), byte clicks still map to the right offsets, the
    /// rows are not overdrawn by the panel and the scroll bar stays on the editor's right edge.
    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_with_the_panel_on_byte_clicks_rows_and_scroll_bar_stay_right() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(2000).collect();
        let (_d, mut app) = ins_app(&bytes).await;
        for (w, beside) in [(120u16, true), (80, false)] {
            let term = crate::hex_tests::draw(&mut app, w, 30);
            let g = app.hits.group(app.groups.active).unwrap();
            let (rows, editor, bar) = (g.text, g.editor, g.scrollbar.expect("scroll bar"));
            let ph = hit(&app);
            if beside {
                assert!(rows.x + rows.width <= ph.toggle.x, "{w}: rows end before the panel");
            } else {
                assert!(rows.y + rows.height <= ph.toggle.y, "{w}: rows end above the panel");
            }
            assert!(
                bar.track.x >= rows.x + rows.width - 1 && bar.track.x < w,
                "{w}: bar at the right"
            );
            assert!(
                bar.track.x + bar.track.width <= editor.x + editor.width + 1,
                "{w}: bar in the editor"
            );
            let l = hex(&app).layout;
            let n = l.bytes_per_row();
            assert!(n >= 8, "{w}: at least 8-byte rows");
            assert!(
                l.ascii_col(n - 1) < rows.width as usize,
                "{w}: the last ASCII cell is inside"
            );
            // The first row shows its last byte's hex digits uncut.
            let first = line(&term, rows.x, rows.y, rows.width);
            let last = format!("{:02x}", n - 1);
            assert!(first.contains(&last), "{w}: row 0 `{first}` has byte {last}");
            // Clicking bytes of rows 1 and 2 selects the matching offsets.
            click(&mut app, rows.x + l.hex_col(5) as u16 + 1, rows.y + 1);
            assert_eq!(hex(&app).cursor, n + 5, "{w}");
            click(&mut app, rows.x + l.ascii_col(n - 1) as u16, rows.y + 2);
            assert_eq!(hex(&app).cursor, 3 * n - 1, "{w}: last byte of row 2");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_values_in_both_byte_orders_by_command_key_and_header_click() {
        let (_d, mut app) = ins_app(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]).await;
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u16"), "513");
        assert_eq!(shown(&app, &term, "u32"), "67305985");
        assert_eq!(shown(&app, &term, "u64"), "578437695752307201");
        assert_eq!(shown(&app, &term, "binary"), "00000001");

        app.test_execute(Command::HexToggleByteOrder);
        let term = wide(&mut app);
        assert_eq!(app.hex_order(), ByteOrder::Big);
        assert_eq!(shown(&app, &term, "u16"), "258");
        assert_eq!(shown(&app, &term, "u32"), "16909060");
        assert_eq!(shown(&app, &term, "u64"), "72623859790382856");
        let toggle = hit(&app).toggle;
        assert!(line_has(&term, toggle, "BE"));

        // Alt+E (bound on a hex tab) and a click on the header flip it back and forth.
        app.press(KeyCode::Char('e'), M::ALT);
        assert_eq!(app.hex_order(), ByteOrder::Little);
        wide(&mut app);
        click(&mut app, toggle.x + 1, toggle.y);
        assert_eq!(app.hex_order(), ByteOrder::Big);
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u16"), "258");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_floats_and_text_rows() {
        let (_d, mut app) = ins_app(&[0, 0, 0x80, 0x3f, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f]).await;
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "f32 float"), "1");
        assert_eq!(shown(&app, &term, "char"), "\\0");
        // 8 bytes on from 4: 00 00 00 00 00 00 f0 3f.
        go_to_offset(&mut app, "4");
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "f64 double"), "1");
        assert_eq!(shown(&app, &term, "f32 float"), "0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_a_selection_reads_from_its_start_highlights_its_width_and_outlines_its_bytes() {
        let bytes: Vec<u8> = (1..=40).collect();
        let (_d, mut app) = ins_app(&bytes).await;
        go_to_offset(&mut app, "4");
        for _ in 0..3 {
            app.press(KeyCode::Right, M::SHIFT);
        }
        let term = wide(&mut app);
        assert_eq!(hex(&app).selection(), Some(4..8));
        assert_eq!(
            shown(&app, &term, "u32"),
            "134678021",
            "read from the selection's start"
        );
        let active = bg(&app, "hexEditor.inspector.active");
        let lit: Vec<RowKind> = hit(&app)
            .rows
            .iter()
            .filter(|(r, _)| term.backend().buffer()[(r.x, r.y)].bg == active)
            .map(|&(_, k)| k)
            .collect();
        assert_eq!(lit, [RowKind::I32, RowKind::U32, RowKind::F32]);
        // Those four bytes (and only they) are outlined, in both columns.
        let inspected = bg(&app, "hexEditor.inspected");
        for at in 0..16 {
            let (h, a) = byte_bgs(&app, &term, at);
            let on = (4..8).contains(&at);
            assert_eq!((h == inspected, a == inspected), (on, on), "byte {at}");
        }
        // A selection of 3 bytes marks nothing.
        app.press(KeyCode::Left, M::SHIFT);
        let term = wide(&mut app);
        assert!(
            hit(&app)
                .rows
                .iter()
                .all(|(r, _)| term.backend().buffer()[(r.x, r.y)].bg != active)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_values_past_the_end_are_blank() {
        let (_d, mut app) = ins_app(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]).await;
        go_to_offset(&mut app, "9");
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u16"), "2826");
        for label in ["i32 int", "u32", "i64 long", "u64", "f32 float", "f64 double"] {
            assert_eq!(shown(&app, &term, label), "—", "{label}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_copy_value_copies_the_clicked_or_highlighted_row() {
        let (_d, mut app) = ins_app(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]).await;
        wide(&mut app);
        let (u32_row, _) = *hit(&app).rows.iter().find(|(_, k)| *k == RowKind::U32).unwrap();
        click(&mut app, u32_row.x + 2, u32_row.y);
        let term = wide(&mut app);
        assert_eq!(hex(&app).cursor, 0, "a row click doesn't move the cursor");
        let active = bg(&app, "hexEditor.inspector.active");
        assert_eq!(term.backend().buffer()[(u32_row.x, u32_row.y)].bg, active);
        // Its four bytes are outlined.
        let inspected = bg(&app, "hexEditor.inspected");
        assert_eq!(byte_bgs(&app, &term, 3).0, inspected);
        assert_ne!(byte_bgs(&app, &term, 4).0, inspected);
        app.test_execute(Command::HexCopyValue);
        assert_eq!(app.clipboard.read().unwrap(), "67305985");
        // Moving the cursor drops the clicked row; with nothing marked nothing is copied.
        app.press(KeyCode::Right, M::NONE);
        wide(&mut app);
        app.clipboard.write(crate::clipboard::Copied {
            text: "keep".into(),
            whole_line: false,
            pieces: Vec::new(),
        });
        app.test_execute(Command::HexCopyValue);
        assert_eq!(app.clipboard.read().unwrap(), "keep");
        // A 2-byte selection highlights `i16` first: Copy Value copies it.
        app.press(KeyCode::Right, M::SHIFT);
        wide(&mut app);
        app.test_execute(Command::HexCopyValue);
        assert_eq!(app.clipboard.read().unwrap(), "770");
        // A 1-byte selection copies `i8` (the first integer row, not `binary`).
        app.groups.active_group_mut().active_hex_mut().unwrap().select(5..6);
        wide(&mut app);
        app.test_execute(Command::HexCopyValue);
        assert_eq!(app.clipboard.read().unwrap(), "6");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_the_toggles_and_the_byte_order_survive_quit_and_reopen() {
        let (_d, root, state) = dirs();
        fs::write(root.join("a.bin"), [0u8, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let mut app = state_app(&root, &state).await;
        app.hex_inspector = true;
        quick_open(&mut app, "a.bin").await;
        app.test_execute(Command::HexToggleByteOrder);
        app.test_execute(Command::HexToggleInspector);
        assert!(!app.hex_inspector);
        quit(app).await;
        let mut app = state_app(&root, &state).await;
        assert!(app.settle(|a| a.active_hex().is_some()).await, "restored");
        assert!(!app.hex_inspector);
        assert_eq!(app.hex_byte_order, Some(ByteOrder::Big));
        app.test_execute(Command::HexToggleInspector);
        quit(app).await;
        let mut app = state_app(&root, &state).await;
        assert!(app.settle(|a| a.active_hex().is_some()).await, "restored");
        assert!(app.hex_inspector);
    }

    #[test]
    fn hex_more_c_an_older_session_without_the_fields_loads_with_the_defaults() {
        let none: crate::session::HexState = serde_json::from_str(r#"{"inspector":false}"#).unwrap();
        assert_eq!((none.inspector, none.byte_order), (false, None));
        let only: crate::session::HexState = serde_json::from_str(r#"{"byte_order":"big"}"#).unwrap();
        assert_eq!((only.inspector, only.byte_order), (true, Some(ByteOrder::Big)));
        let both: crate::session::HexState = serde_json::from_str(r#"{"inspector":true,"byte_order":"big"}"#).unwrap();
        assert_eq!(both.byte_order, Some(ByteOrder::Big));
        let d = crate::session::HexState::default();
        assert_eq!((d.inspector, d.byte_order), (true, None));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_the_setting_sets_the_starting_order_and_a_chosen_one_wins() {
        let (_d, mut app) = ins_app(&[1, 2, 3, 4]).await;
        app.settings.hex_byte_order = ByteOrder::Big;
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u16"), "258");
        app.test_execute(Command::HexToggleByteOrder);
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u16"), "513", "toggling flips the setting's order");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_c_a_paged_doc_shows_pending_values_until_its_page_loads() {
        let (_d, _root, mut app) = big_app(BIG, &[(0x10, &[1, 2, 3, 4, 5, 6, 7, 8])], false).await;
        app.hex_inspector = true;
        quick_open(&mut app, "big.bin").await;
        go_to_offset(&mut app, "0x10");
        let term = wide(&mut app);
        assert_eq!(shown(&app, &term, "u32"), "…");
        assert_eq!(shown(&app, &term, "u8"), "…");
        for _ in 0..100 {
            let term = wide(&mut app);
            if shown(&app, &term, "u32") != "…" {
                assert_eq!(shown(&app, &term, "u32"), "67305985");
                return;
            }
            app.needs_redraw = false;
            assert!(app.settle(|a| a.needs_redraw).await, "the page arrives");
        }
        panic!("the page never loaded");
    }

    #[test]
    fn hex_more_c_commands_are_named() {
        for (c, name) in [
            (Command::HexToggleInspector, "hex.toggleInspector"),
            (Command::HexToggleByteOrder, "hex.toggleEndianness"),
            (Command::HexCopyValue, "hex.copyValue"),
        ] {
            assert_eq!(c.name(), name);
            assert_eq!(Command::from_name(name), Some(c));
        }
    }
}

/// Byte grouping (hex_more-d): "Hex: Cycle Byte Grouping" and the `hex_group_bytes` setting group
/// the hex column (`xxd -g`), reversed per group in little-endian; rows stay aligned, clicks hit
/// bytes inside a group, Cmd+C copies the groups as shown, the grouping is kept in the session.
mod hex_more_d {
    use super::*;
    use crate::command::Command;
    use crate::hex::{ByteOrder, HexDoc};
    use crate::hex_tests::{bg, byte_bgs, hex_app};
    use crossterm::event::{MouseButton, MouseEventKind};

    async fn grp_app(len: u8) -> (tempfile::TempDir, App) {
        let (dir, root, mut app) = hex_app().await;
        app.open_initial_hex(root.join("x.bin"), HexDoc::new((1..=len).collect::<Vec<_>>()));
        (dir, app)
    }

    fn first_row(app: &mut App) -> String {
        let term = crate::hex_tests::draw(app, 100, 12);
        rows(app, &term).remove(0)
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_cycling_regroups_the_rows_in_both_byte_orders() {
        let (_d, mut app) = grp_app(20).await;
        assert!(first_row(&mut app).starts_with("00000000  01 02 03 04 05 06 07 08  09"));
        app.test_execute(Command::HexCycleGrouping);
        assert!(first_row(&mut app).starts_with("00000000  0201 0403 0605 0807  0a09"));
        app.test_execute(Command::HexCycleGrouping);
        assert!(first_row(&mut app).starts_with("00000000  04030201 08070605  0c0b0a09"));
        app.test_execute(Command::HexToggleByteOrder);
        assert_eq!(app.hex_order(), ByteOrder::Big);
        assert!(first_row(&mut app).starts_with("00000000  01020304 05060708  090a0b0c"));
        app.test_execute(Command::HexCycleGrouping);
        assert!(first_row(&mut app).starts_with("00000000  0102030405060708  090a0b0c0d0e0f10"));
        app.test_execute(Command::HexCycleGrouping);
        assert!(first_row(&mut app).starts_with("00000000  01 02 03 04"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_a_click_on_the_02_digits_puts_the_cursor_on_offset_1() {
        let (_d, mut app) = grp_app(20).await;
        app.hex_group = Some(4);
        let row = first_row(&mut app);
        assert!(row.contains("04030201"));
        let rect = rows_rect(&app);
        // "04030201": the 02 is the third pair.
        let col = row.find("04030201").unwrap() + 4;
        for dx in [0, 1] {
            click(&mut app, rect.x + 11, rect.y + 1);
            click(&mut app, rect.x + (col + dx) as u16, rect.y);
            assert_eq!(hex(&app).cursor, 1, "digit {dx}");
        }
        app.test_execute(Command::HexToggleByteOrder);
        let row = first_row(&mut app);
        let col = row.find("01020304").unwrap() + 4;
        click(&mut app, rect.x + col as u16, rect.y);
        assert_eq!(hex(&app).cursor, 2, "big-endian: the third pair is offset 2");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_rows_stay_aligned_with_a_short_last_row() {
        let (_d, mut app) = grp_app(37).await;
        for g in [2, 4, 8, 1] {
            app.hex_group = Some(g);
            for order in [ByteOrder::Little, ByteOrder::Big] {
                app.hex_byte_order = Some(order);
                let term = crate::hex_tests::draw(&mut app, 100, 12);
                let rs = rows(&app, &term);
                let bars: Vec<_> = rs.iter().filter(|r| r.contains('|')).map(|r| r.find('|')).collect();
                assert!(bars.len() >= 3, "{rs:?}");
                assert!(bars.windows(2).all(|w| w[0] == w[1]), "{g} {order:?}: {rs:?}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_a_selection_inside_a_reversed_group_paints_exactly_its_bytes() {
        let (_d, mut app) = grp_app(20).await;
        app.hex_group = Some(4);
        crate::hex_tests::draw(&mut app, 100, 12);
        {
            let h = app.group_mut().active_hex_mut().unwrap();
            h.anchor = Some(1);
            h.cursor = 2;
        }
        let term = crate::hex_tests::draw(&mut app, 100, 12);
        let sel = bg(&app, "editor.selection");
        let painted: Vec<_> = (0..5).map(|i| byte_bgs(&app, &term, i).0 == sel).collect();
        assert_eq!(painted, [false, true, true, false, false]);
        // Only the two bytes' four digits are lit in the hex part.
        let rect = rows_rect(&app);
        let buf = term.backend().buffer();
        let l = hex(&app).layout;
        let lit = (0..l.bar_col())
            .filter(|c| buf[(rect.x + *c as u16, rect.y)].bg == sel)
            .count();
        assert_eq!(lit, 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_narrow_windows_and_inspector_keep_whole_groups() {
        let (_d, mut app) = grp_app(40).await;
        app.hex_group = Some(8);
        // 80 columns: 16 bytes in groups of 8 fit.
        let term = crate::hex_tests::draw(&mut app, 80, 12);
        let first = rows(&app, &term)[0].clone();
        assert!(
            first.starts_with("00000000  0807060504030201  100f0e0d0c0b0a09"),
            "{first:?}"
        );
        // Too narrow even for one group: the row is cut, nothing panics.
        for w in [40u16, 30, 20, 12] {
            let term = crate::hex_tests::draw(&mut app, w, 12);
            let rect = rows_rect(&app);
            if rect.x + rect.width <= w {
                for r in rows(&app, &term) {
                    assert!(r.chars().count() <= w as usize, "{r:?}");
                }
            }
        }
        click(&mut app, 3, 1);
    }

    /// Both digits' backgrounds of byte `at` of the first row, as drawn.
    fn digit_bgs(app: &App, term: &Terminal<TestBackend>, at: usize) -> [ratatui::style::Color; 2] {
        let rect = rows_rect(app);
        let c = hex(app).layout.hex_col(at);
        let buf = term.backend().buffer();
        [0, 1].map(|d| buf[(rect.x + (c + d) as u16, rect.y)].bg)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_find_matches_paint_inside_reversed_groups_across_a_boundary() {
        let (_d, root, mut app) = hex_app().await;
        // Two "cde" matches, each starting two bytes before a group (of 4) boundary.
        app.open_initial_hex(root.join("x.bin"), HexDoc::new(b"xxcdexxxxxcdexxxxxxx".to_vec()));
        app.hex_group = Some(4);
        crate::hex_tests::draw(&mut app, 100, 12);
        crate::hex_tests::find(&mut app, "cde");
        assert_eq!(hex(&app).selection(), Some(2..5));
        let term = crate::hex_tests::draw(&mut app, 100, 12);
        assert_eq!(hex(&app).layout.bytes_per_row(), 16);
        let current = bg(&app, "editor.findMatch");
        let other = bg(&app, "editor.findMatchHighlight");
        // The selected match: bytes 2, 3 (4 has the cursor), digits and ASCII.
        for at in [2, 3] {
            assert_eq!(digit_bgs(&app, &term, at), [current; 2], "byte {at}");
            assert_eq!(byte_bgs(&app, &term, at).1, current, "byte {at}");
        }
        // The other match, 10..13: the end of one group and the start of the next.
        for at in [10, 11, 12] {
            assert_eq!(digit_bgs(&app, &term, at), [other; 2], "byte {at}");
            assert_eq!(byte_bgs(&app, &term, at).1, other, "byte {at}");
        }
        for at in [0, 1, 5, 6, 8, 9, 13, 14] {
            let [a, b] = digit_bgs(&app, &term, at);
            let asc = byte_bgs(&app, &term, at).1;
            assert!(![a, b, asc].iter().any(|c| *c == current || *c == other), "byte {at}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_selection_across_a_group_boundary_paints_its_digits_and_the_gap_between() {
        let (_d, mut app) = grp_app(20).await;
        app.hex_group = Some(4);
        crate::hex_tests::draw(&mut app, 100, 12);
        let sel = bg(&app, "editor.selection");
        // The space after group 0 (its 8 digits start at `offset_digits + 2`).
        let gap_painted = |app: &App, term: &Terminal<TestBackend>| {
            let rect = rows_rect(app);
            let col = hex(app).layout.offset_digits() + 2 + 8;
            term.backend().buffer()[(rect.x + col as u16, rect.y)].bg == sel
        };
        // Little-endian groups run right to left: no gap is bridged. Big-endian: only a gap with
        // both neighbours (bytes 3 and 4) selected.
        for (order, anchor, cursor, gap) in [
            (ByteOrder::Little, 2, 5, false),
            (ByteOrder::Little, 0, 8, false),
            (ByteOrder::Little, 0, 3, false),
            (ByteOrder::Big, 2, 5, true),
            (ByteOrder::Big, 0, 8, true),
            (ByteOrder::Big, 0, 3, false),
            (ByteOrder::Big, 4, 8, false),
        ] {
            app.hex_byte_order = Some(order);
            {
                let h = app.group_mut().active_hex_mut().unwrap();
                h.anchor = Some(anchor);
                h.cursor = cursor;
            }
            let term = crate::hex_tests::draw(&mut app, 100, 12);
            let want = hex(&app).selection().unwrap();
            for at in 0..12 {
                let d = digit_bgs(&app, &term, at);
                assert_eq!(
                    d == [sel; 2],
                    want.contains(&at),
                    "{order:?} {anchor}..{cursor} byte {at}"
                );
                assert!(
                    d == [sel; 2] || d.iter().all(|c| *c != sel),
                    "{order:?} {anchor}..{cursor} byte {at}"
                );
            }
            assert_eq!(
                gap_painted(&app, &term),
                gap,
                "{order:?} {anchor}..{cursor}: gap after group 0"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_a_paged_doc_copies_grouped_hex_and_unchanged_ascii() {
        let (_d, _root, mut app) = big_app(20 << 20, &[(1, b"COPYME!")], false).await;
        app.hex_group = Some(4);
        quick_open(&mut app, "big.bin").await;
        assert!(hex(&app).doc.resident_bytes().is_none(), "paged");
        draw(&mut app);
        // Bytes 1..=6 ("COPYME"): file-aligned groups of 4, the partial ones as selected.
        app.press(KeyCode::Right, M::NONE);
        for _ in 0..5 {
            app.press(KeyCode::Right, M::SHIFT);
        }
        app.press(KeyCode::Char('c'), cmd());
        let le = wait_copied(&mut app, "").await;
        assert_eq!(le, "504f43 454d59", "little-endian: each group reversed");
        app.test_execute(Command::HexToggleByteOrder);
        draw(&mut app);
        app.press(KeyCode::Char('c'), cmd());
        let be = wait_copied(&mut app, &le).await;
        assert_eq!(be, "434f50 594d45");
        // The ASCII column's copy is the text, whatever the grouping.
        app.press(KeyCode::Tab, M::NONE);
        draw(&mut app);
        app.press(KeyCode::Char('c'), cmd());
        assert_eq!(wait_copied(&mut app, &be).await, "COPYME");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_copy_follows_the_grouping_as_shown() {
        let (_d, mut app) = grp_app(20).await;
        app.hex_group = Some(4);
        crate::hex_tests::draw(&mut app, 100, 12);
        {
            let h = app.group_mut().active_hex_mut().unwrap();
            h.anchor = Some(1);
            h.cursor = 6;
        }
        app.press(KeyCode::Char('c'), cmd());
        assert_eq!(app.clipboard.read().unwrap(), "040302 070605");
        app.test_execute(Command::HexToggleByteOrder);
        crate::hex_tests::draw(&mut app, 100, 12);
        app.press(KeyCode::Char('c'), cmd());
        assert_eq!(app.clipboard.read().unwrap(), "020304 050607");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_the_setting_starts_the_grouping_and_a_chosen_one_wins() {
        let (_d, mut app) = grp_app(20).await;
        app.settings.hex_group_bytes = 2;
        assert!(first_row(&mut app).starts_with("00000000  0201 0403"));
        app.test_execute(Command::HexCycleGrouping);
        assert!(
            first_row(&mut app).starts_with("00000000  04030201"),
            "cycles from the setting's"
        );
        assert_eq!(
            crate::config::settings::parse("[settings]\nhex_group_bytes = 3\n")
                .errors
                .len(),
            1
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hex_more_d_the_grouping_survives_quit_and_reopen() {
        let (_d, root, state) = dirs();
        fs::write(root.join("a.bin"), [0u8, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let mut app = state_app(&root, &state).await;
        quick_open(&mut app, "a.bin").await;
        app.test_execute(Command::HexCycleGrouping);
        app.test_execute(Command::HexCycleGrouping);
        quit(app).await;
        let mut app = state_app(&root, &state).await;
        assert!(app.settle(|a| a.active_hex().is_some()).await, "restored");
        assert_eq!(app.hex_group, Some(4));
        let none: crate::session::HexState = serde_json::from_str(r#"{"inspector":false}"#).unwrap();
        assert_eq!(none.group, None);
    }

    #[test]
    fn hex_more_d_the_command_is_named() {
        assert_eq!(Command::HexCycleGrouping.name(), "hex.cycleGrouping");
        assert_eq!(Command::from_name("hex.cycleGrouping"), Some(Command::HexCycleGrouping));
        assert_eq!(Command::HexCycleGrouping.title(), "Hex: Cycle Byte Grouping");
    }
}
