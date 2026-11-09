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
