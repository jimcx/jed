//! spell_check-c's "Done when": in the editor, a typo in a Rust file's comment, string and
//! identifier is marked (only the misspelled piece), keywords and acronyms are left alone; the
//! marks follow edits and only the touched lines are checked again; a stale background result
//! is dropped; a half-typed word is not flagged until the caret leaves it or a separator is
//! typed; a dictionary file edited by hand takes effect without a restart and a workspace
//! dictionary stays in its workspace; the Problems list shows the marks only when
//! `spell_check_problems` is on; drawing reads the marks only; and the checker is off for a
//! scope of `off`, a toggled buffer and a file over `syntax_max_bytes`. The release budget test
//! (`milestone_spell_check_budget_*`) covers a 1 MB file.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::edit::SpellScope;
use crate::lsp::ServerId;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// The words the tests' dictionary knows (the programming terms come with the checker).
const WORDS: &str = "the comment main max size hello message server world a ok line text value total count \
                     example another here some words and one two";

struct Env {
    app: App,
    root: PathBuf,
    config: PathBuf,
}

/// An app on `name` with `text` in a fresh workspace, a config dir with the test dictionary.
fn open(dir: &tempfile::TempDir, name: &str, text: &str, tweak: impl FnOnce(&mut App)) -> Env {
    let base = crate::workspace::normalize(dir.path());
    let config = base.join("config");
    std::fs::create_dir_all(config.join("dictionaries")).unwrap();
    std::fs::write(config.join("dictionaries/en_AU.txt"), WORDS.replace(' ', "\n")).unwrap();
    let root = base.join("ws");
    std::fs::create_dir_all(&root).unwrap();
    open_in(&base, &root, &config, name, text, tweak)
}

fn open_in(
    base: &std::path::Path,
    root: &std::path::Path,
    config: &std::path::Path,
    name: &str,
    text: &str,
    tweak: impl FnOnce(&mut App),
) -> Env {
    let path = root.join(name);
    std::fs::write(&path, text).unwrap();
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(base.join("trust.toml"));
    let initial = InitialFile {
        buffer: Buffer::open(&path).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config.to_path_buf();
    app.settings.spell_check_scope = SpellScope::All;
    tweak(&mut app);
    app.test_start();
    Env {
        app,
        root: root.to_path_buf(),
        config: config.to_path_buf(),
    }
}

/// Waits until every buffer is checked under the current settings and dictionaries.
async fn idle(app: &mut App) {
    assert!(app.settle(|a| a.test_spell_idle()).await, "the spell check finished");
}

/// The text under each spell mark of the active buffer, in order.
fn spelled(app: &App) -> Vec<String> {
    let b = app.active_buffer().unwrap();
    b.diagnostics
        .spelling()
        .iter()
        .map(|m| b.rope.slice(m.start.0..m.end.0).to_string())
        .collect()
}

const RUST: &str = "// the recieve comment\nfn main() {\n    let max_buffr_size = \"hello wrold\";\n    let recieveMessage = HTTPServer;\n}\n";

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_marks_the_misspelled_pieces_of_a_rust_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.rs", RUST, |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "buffr", "wrold", "recieve"]);
    let m = &e.app.active_buffer().unwrap().diagnostics.spelling()[0];
    assert_eq!(m.diag.message, "Unknown word \"recieve\"");
    assert_eq!(m.diag.source.as_deref(), Some("Spelling"));
    assert_eq!(m.diag.severity, crate::diagnostics::Severity::Hint);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_comments_and_strings_scope_skips_identifiers() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.rs", RUST, |a| {
        a.settings.spell_check_scope = SpellScope::CommentsAndStrings;
    });
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "wrold"]);
    // Changing the setting checks again without a restart.
    e.app.settings.spell_check_scope = SpellScope::Off;
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty());
    e.app.settings.spell_check_scope = SpellScope::All;
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_marks_follow_edits_and_only_the_touched_lines_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let text = "// recieve one\n// wrold two\n// ok line\n";
    let mut e = open(&dir, "a.rs", text, |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "wrold"]);
    let before = e.app.spell.scanned;
    e.app.press(K::Home, M::CONTROL);
    e.app.type_text("a ");
    // Right away the old marks have moved with the text.
    let b = e.app.active_buffer().unwrap();
    let wrold = b.diagnostics.spelling()[1].start.0;
    assert_eq!(b.rope.slice(wrold..wrold + 5).to_string(), "wrold");
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "wrold"]);
    let scanned = e.app.spell.scanned - before;
    assert!(
        scanned <= "a // recieve one\n".len(),
        "only line 1 was scanned: {scanned} bytes"
    );
    assert!(scanned > 0);

    // A fixed word loses its mark; the rest stay.
    e.app.press(K::Down, M::NONE);
    e.app.press(K::Home, M::NONE);
    e.app.press(K::Right, M::NONE);
    e.app.press(K::Right, M::NONE);
    e.app.press(K::Right, M::NONE);
    for _ in 0..5 {
        e.app.press(K::Delete, M::NONE);
    }
    e.app.type_text("world");
    e.app.press(K::Char(' '), M::NONE);
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_a_stale_background_result_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut text = String::from("recieve\n");
    text.push_str(&"hello world some words here\n".repeat(1000));
    let mut e = open(&dir, "a.txt", &text, |_| {});
    // Run the app by hand until the full check is in the background.
    while e.app.spell.background == 0 {
        e.app.test_handle_one().await;
        e.app.sync_spell();
    }
    // The text changes before its result is handled: the typo is gone.
    e.app.press(K::Home, M::CONTROL);
    for _ in 0..7 {
        e.app.press(K::Delete, M::NONE);
    }
    while e.app.active_buffer().unwrap().spell.running.is_some() {
        e.app.test_handle_one().await;
    }
    assert!(spelled(&e.app).is_empty(), "the old result was not applied");
    assert!(e.app.active_buffer().unwrap().spell.full, "the buffer is checked again");
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty());
    assert!(e.app.spell.background >= 2, "a second check ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_a_half_typed_word_is_not_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "ok\n", |_| {});
    idle(&mut e.app).await;
    e.app.press(K::End, M::CONTROL);
    e.app.type_text("recieveMessage");
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty(), "no mark while the caret is in the word");
    // The held word stays held while the caret is in it: turns of the loop scan nothing.
    let scanned = e.app.spell.scanned;
    for _ in 0..3 {
        e.app.sync_spell();
        idle(&mut e.app).await;
    }
    assert_eq!(
        e.app.spell.scanned, scanned,
        "a held word is not checked again until the caret leaves"
    );
    assert!(spelled(&e.app).is_empty());
    // A separator typed: the word is done.
    e.app.type_text(" ");
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve"]);

    // Held until the caret leaves, too.
    e.app.type_text("wrold");
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve"]);
    e.app.press(K::Up, M::NONE);
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "wrold"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_editing_the_user_dictionary_applies_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve hello wrold\n", |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve", "wrold"]);
    let file = e.config.join("dictionary.txt");
    std::fs::write(&file, "# mine\nrecieve\n").unwrap();
    e.app.test_fs_changed(vec![file.clone()]);
    assert!(
        e.app.settle(|a| spelled(a) == ["wrold"]).await,
        "the word is accepted: {:?}",
        spelled(&e.app)
    );
    std::fs::write(&file, "").unwrap();
    e.app.test_fs_changed(vec![file]);
    assert!(e.app.settle(|a| spelled(a).len() == 2).await, "and unaccepted");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_a_workspace_dictionary_stays_in_its_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::workspace::normalize(dir.path());
    let config = base.join("config");
    std::fs::create_dir_all(config.join("dictionaries")).unwrap();
    std::fs::write(config.join("dictionaries/en_AU.txt"), WORDS.replace(' ', "\n")).unwrap();
    let (one, two) = (base.join("one"), base.join("two"));
    std::fs::create_dir_all(one.join(".jed")).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    std::fs::write(one.join(".jed/dictionary.txt"), "recieve\n").unwrap();
    let mut a = open_in(&base, &one, &config, "a.txt", "recieve here\n", |_| {});
    let mut b = open_in(&base, &two, &config, "a.txt", "recieve here\n", |_| {});
    idle(&mut a.app).await;
    idle(&mut b.app).await;
    assert!(spelled(&a.app).is_empty(), "accepted in its own workspace");
    assert_eq!(spelled(&b.app), ["recieve"], "not in the other");

    // Edited by hand: the watcher's event for it gets through (the rest of `.jed` is dropped).
    let file = a.root.join(".jed/dictionary.txt");
    std::fs::write(&file, "").unwrap();
    a.app.test_fs_changed(vec![file]);
    assert!(a.app.settle(|x| spelled(x) == ["recieve"]).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_ignore_words_setting_and_off_languages() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.rs", RUST, |a| {
        a.settings.spell_check_ignore_words = vec!["Recieve".into(), "wrold".into()];
    });
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["buffr"]);
    e.app.settings.spell_check_off_languages = vec!["rust".into()];
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty());
    assert!(!e.app.active_buffer().unwrap().diagnostics.has(ServerId::SPELL));
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_is_off_over_the_size_limit_and_when_toggled() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\n", |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 1);
    e.app.settings.syntax_max_bytes = 4;
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty(), "over syntax_max_bytes");
    e.app.settings.syntax_max_bytes = 1 << 20;
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 1);
    e.app.active_mut().unwrap().0.spell.enabled = false;
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty(), "toggled off for the buffer");
    e.app.active_mut().unwrap().0.spell.enabled = true;
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_problems_list_them_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\nwrold\n", |_| {});
    idle(&mut e.app).await;
    let count = |a: &App| {
        crate::problems::files(&a.lsp.diagnostics, &a.buffers)
            .iter()
            .map(|f| f.count())
            .sum::<usize>()
    };
    assert_eq!(count(&e.app), 0, "off by default");
    e.app.settings.spell_check_problems = true;
    idle(&mut e.app).await;
    assert_eq!(count(&e.app), 2);
    let files = crate::problems::files(&e.app.lsp.diagnostics, &e.app.buffers);
    let first = files[0].problem(0).unwrap();
    assert_eq!(first.diag.source.as_deref(), Some("Spelling"));
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_draws_its_own_squiggle_and_hovers_the_word() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "hello recieve\n", |_| {});
    idle(&mut e.app).await;
    let (before, bg) = (e.app.spell.scanned, e.app.spell.background);
    let term = draw(&mut e.app);
    let want = e.app.theme.get("editorSpell.foreground").underline_color;
    assert!(want.is_some());
    let (x, y) = crate::milestone9_tests::find_cells(&term, "recieve").expect("drawn");
    for i in 0..7 {
        assert_eq!(crate::milestone9_tests::squiggle(&term, x + i, y), want);
    }
    assert_eq!(
        crate::milestone9_tests::squiggle(&term, x - 2, y),
        None,
        "not under hello"
    );
    // A frame with no edit scans nothing.
    for _ in 0..3 {
        draw(&mut e.app);
    }
    assert_eq!((e.app.spell.scanned, e.app.spell.background), (before, bg));

    // The hover rows come from the mark like any diagnostic.
    let b = e.app.active_buffer().unwrap();
    let at = crate::buffer::pos::ByteIdx(8);
    let marks = b.diagnostics.touching(&b.rope, at);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].diag.message, "Unknown word \"recieve\"");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_marks_never_count_as_a_servers() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\n", |_| {});
    idle(&mut e.app).await;
    let b = e.app.active_buffer().unwrap();
    assert_eq!(b.diagnostics.spelling().len(), 1, "there is a spell mark");
    // The servers' view of the marks (`all`, the status bar's totals and the stores) has none.
    assert!(b.diagnostics.all().is_empty());
    let counts = e.app.lsp.diagnostics.counts();
    assert_eq!(crate::diagnostics::Severity::ALL.map(|s| counts.of(s)), [0; 4]);
    // The status bar's line message stays a server's.
    let (start, end, _) = crate::buffer::pos::line_bounds(&b.rope, 0);
    assert!(b.diagnostics.for_line(start, end, start).is_none());
    assert_ne!(ServerId::SPELL, ServerId::none());
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_show_hover_lists_the_word() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "hello recieve\n", |_| {});
    idle(&mut e.app).await;
    for _ in 0..8 {
        e.app.press(K::Right, M::NONE);
    }
    e.app.test_execute(crate::command::Command::ShowHover);
    let popup = e.app.hover.popup.as_ref().expect("the hover opened on the mark");
    assert!(
        popup.lines.iter().any(|l| l.text.contains("Unknown word \"recieve\"")),
        "{:?}",
        popup.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_an_edit_with_many_selections_checks_again_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", &"wrold here\n".repeat(100), |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 100);
    e.app.press(K::Home, M::CONTROL);
    e.app.press(K::Right, M::NONE);
    e.app.test_execute(crate::command::Command::SelectAllMatches);
    e.app.type_text("hello");
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty(), "every selection's word was replaced");
    e.app.test_execute(crate::command::Command::SelectAllMatches);
    e.app.type_text("recieve");
    idle(&mut e.app).await;
    let marks = spelled(&e.app);
    assert_eq!(marks.len(), 100);
    assert!(marks.iter().all(|w| w == "recieve"));
}

/// A plain-text file of about 1 MB with a typo on every 200th line.
fn big_text() -> String {
    let mut out = String::new();
    let mut i = 0;
    while out.len() < 1_000_000 {
        if i % 200 == 0 {
            out.push_str("some words here recieve another example\n");
        } else {
            out.push_str("some words here and another example text\n");
        }
        i += 1;
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_spell_check_budget_one_mb() {
    let dir = tempfile::tempdir().unwrap();
    let text = big_text();
    let typos = text.matches("recieve").count();
    let t = Instant::now();
    let mut e = open(&dir, "big.txt", &text, |_| {});
    // The UI keeps turning while the full scan runs on another thread.
    let mut turns = 0;
    let started = Instant::now();
    while !e.app.test_spell_idle() {
        e.app.pump();
        turns += 1;
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert!(started.elapsed() < Duration::from_secs(60), "scan finished");
    }
    let scan = t.elapsed();
    println!("full scan of {} bytes: {scan:?} ({turns} turns)", text.len());
    assert_eq!(spelled(&e.app).len(), typos);
    assert!(e.app.spell.background >= 1, "the full scan ran in the background");
    let bound = if cfg!(debug_assertions) { 60 } else { 1 };
    assert!(scan < Duration::from_secs(bound), "full scan too slow: {scan:?}");

    // A one-line edit re-checks in a few ms and scans one line.
    let before = e.app.spell.scanned;
    e.app.press(K::Home, M::CONTROL);
    e.app.type_text("x ");
    let edited = Instant::now();
    e.app.active_mut().unwrap().0.spell.edited_at = Some(Instant::now() - Duration::from_secs(1));
    e.app.sync_spell();
    let recheck = edited.elapsed();
    println!("one-line re-check: {recheck:?}");
    assert!(e.app.test_spell_idle());
    assert_eq!(e.app.spell.background, 1, "no second background scan");
    assert!(e.app.spell.scanned - before < 200);
    let bound = if cfg!(debug_assertions) { 100 } else { 5 };
    assert!(recheck < Duration::from_millis(bound), "re-check too slow: {recheck:?}");
    assert_eq!(spelled(&e.app).len(), typos);

    // Frames read only the marks on screen.
    let (scanned, bg) = (e.app.spell.scanned, e.app.spell.background);
    let t = Instant::now();
    for _ in 0..20 {
        draw(&mut e.app);
    }
    println!("frame: {:?}", t.elapsed() / 20);
    assert_eq!((e.app.spell.scanned, e.app.spell.background), (scanned, bg));
}
