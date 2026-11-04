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
/// An installed `en_AU` Hunspell pair (no affixes) of just these space-separated words.
fn write_dictionary(config: &std::path::Path, words: &str) {
    let dir = config.join("dictionaries/en_AU");
    std::fs::create_dir_all(&dir).unwrap();
    let words: Vec<&str> = words.split_whitespace().collect();
    std::fs::write(dir.join("en_AU.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(
        dir.join("en_AU.dic"),
        format!("{}\n{}\n", words.len(), words.join("\n")),
    )
    .unwrap();
}

fn open(dir: &tempfile::TempDir, name: &str, text: &str, tweak: impl FnOnce(&mut App)) -> Env {
    let base = crate::workspace::normalize(dir.path());
    let config = base.join("config");
    write_dictionary(&config, WORDS);
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
    b.spell
        .marks
        .all()
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
    let m = &e.app.active_buffer().unwrap().spell.marks.all()[0];
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
    let wrold = b.spell.marks.all()[1].start.0;
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
    e.app.press(K::Down, M::NONE); // onto the empty last line (Ctrl+End is not bound on macOS)
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
    write_dictionary(&config, WORDS);
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
    assert!(e.app.active_buffer().unwrap().spell.marks.is_empty());
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

    // Show Hover on the word: its row comes from the mark like any diagnostic's.
    let at = crate::buffer::pos::ByteIdx(8);
    e.app.active_mut().unwrap().1.selections =
        crate::selection::Selections::single(crate::selection::Selection::cursor(at));
    e.app.test_execute(crate::command::Command::ShowHover);
    let popup = e.app.hover.popup.as_ref().expect("a hover");
    assert!(
        popup.lines.iter().any(|l| l.text.contains("Unknown word \"recieve\"")),
        "{:?}",
        popup.lines.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_marks_never_count_as_a_servers() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\n", |_| {});
    idle(&mut e.app).await;
    let b = e.app.active_buffer().unwrap();
    assert_eq!(b.spell.marks.all().len(), 1, "there is a spell mark");
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

// spell_check-d: the commands and quick fixes.

use crate::buffer::pos::ByteIdx;
use crate::command::{Command, SpellCommand};
use crate::selection::{Selection, Selections};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn spell(app: &mut App, c: SpellCommand) {
    app.test_execute(Command::Spell(c));
}

/// The active editor's caret at byte `at`.
fn caret(app: &mut App, at: usize) {
    let (_, view) = app.active_mut().unwrap();
    view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
}

fn head(app: &App) -> usize {
    app.group().active_tab().unwrap().view.selections.primary().head.0
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

/// The quick fix list's titles (Cmd+. at the caret).
fn fixes(app: &mut App) -> Vec<String> {
    app.code_actions.popup = None;
    app.test_execute(Command::QuickFix);
    app.code_actions
        .popup
        .as_ref()
        .map(|p| p.actions.iter().map(|a| a.title.clone()).collect())
        .unwrap_or_default()
}

/// Also teaches the dictionary `receive`, so there is something to suggest.
fn with_receive(a: &mut App) {
    write_dictionary(&a.config_dir, &format!("{WORDS} receive"));
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_next_and_previous_go_round_the_misspellings() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve ok wrold\nok teh\n", |_| {});
    idle(&mut e.app).await;
    caret(&mut e.app, 0);
    spell(&mut e.app, SpellCommand::Next);
    assert_eq!(head(&e.app), 11, "wrold");
    spell(&mut e.app, SpellCommand::Next);
    assert_eq!(head(&e.app), 20, "teh");
    spell(&mut e.app, SpellCommand::Next);
    assert_eq!(head(&e.app), 0, "wraps to recieve");
    spell(&mut e.app, SpellCommand::Previous);
    assert_eq!(head(&e.app), 20, "and back round");
    spell(&mut e.app, SpellCommand::Previous);
    assert_eq!(head(&e.app), 11);
    assert_eq!(e.app.group().active_tab().unwrap().view.selections.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_next_with_no_misspelling_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "ok line\n", |_| {});
    idle(&mut e.app).await;
    spell(&mut e.app, SpellCommand::Next);
    assert_eq!(message(&e.app), "No misspelled words");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_add_to_user_dictionary_unmarks_the_word_in_every_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve hello wrold\n", |_| {});
    std::fs::write(e.root.join("b.txt"), "another recieve here\n").unwrap();
    idle(&mut e.app).await;
    e.app.press(K::Char('p'), cmd());
    e.app.type_text("b.txt");
    assert!(
        e.app
            .settle(|a| a
                .palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == "b.txt")))
            .await
    );
    e.app.press(K::Enter, M::NONE);
    assert!(
        e.app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "b.txt"))
            .await
    );
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve"]);
    // Back on a.txt, the caret inside the word: mid-word, the mark under it.
    e.app.press(K::Char('p'), cmd());
    e.app.type_text("a.txt");
    assert!(
        e.app
            .settle(|a| a
                .palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == "a.txt")))
            .await
    );
    e.app.press(K::Enter, M::NONE);
    assert!(
        e.app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "a.txt"))
            .await
    );
    caret(&mut e.app, 3);
    spell(&mut e.app, SpellCommand::AddToUser);
    let ok = e
        .app
        .settle(|a| {
            a.buffers.iter().all(|(_, b)| {
                b.spell
                    .marks
                    .all()
                    .iter()
                    .all(|m| b.rope.slice(m.start.0..m.end.0) != "recieve")
            })
        })
        .await;
    assert!(ok, "gone from both buffers");
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["wrold"]);
    let file = std::fs::read_to_string(e.config.join("dictionary.txt")).unwrap();
    assert!(file.lines().any(|l| l == "recieve"), "{file}");
    assert!(!e.root.join(".jed/dictionary.txt").exists());
    // Again: written once.
    spell(&mut e.app, SpellCommand::AddToUser);
    caret(&mut e.app, 0);
    assert!(message(&e.app).contains("Added \"recieve\"") || message(&e.app).contains("No word"));
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_add_to_workspace_dictionary_stays_in_its_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::workspace::normalize(dir.path());
    let config = base.join("config");
    write_dictionary(&config, WORDS);
    let (one, two) = (base.join("one"), base.join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let mut a = open_in(&base, &one, &config, "a.txt", "recieve here\n", |_| {});
    let mut b = open_in(&base, &two, &config, "a.txt", "recieve here\n", |_| {});
    idle(&mut a.app).await;
    idle(&mut b.app).await;
    spell(&mut a.app, SpellCommand::Next);
    spell(&mut a.app, SpellCommand::AddToWorkspace);
    assert!(a.app.settle(|x| spelled(x).is_empty()).await);
    assert!(
        std::fs::read_to_string(one.join(".jed/dictionary.txt"))
            .unwrap()
            .contains("recieve")
    );
    assert!(!config.join("dictionary.txt").exists(), "not the user's file");
    assert!(!two.join(".jed/dictionary.txt").exists());
    idle(&mut b.app).await;
    assert_eq!(spelled(&b.app), ["recieve"], "the other workspace still marks it");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_ignore_is_for_this_buffer_and_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve hello wrold\n", |_| {});
    idle(&mut e.app).await;
    caret(&mut e.app, 2);
    spell(&mut e.app, SpellCommand::Ignore);
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["wrold"]);
    // A new occurrence typed later is ignored too.
    e.app.press(K::End, M::CONTROL);
    e.app.type_text(" recieve ");
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["wrold"]);
    assert!(!e.config.join("dictionary.txt").exists());
    assert!(!e.root.join(".jed/dictionary.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_the_word_at_the_caret_is_used_when_no_mark_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "hello wrold\n", |_| {});
    idle(&mut e.app).await;
    caret(&mut e.app, 2);
    spell(&mut e.app, SpellCommand::AddToUser);
    assert!(e.app.settle(|_| e.config.join("dictionary.txt").exists()).await);
    let file = std::fs::read_to_string(e.config.join("dictionary.txt")).unwrap();
    assert!(file.lines().any(|l| l == "hello"), "{file}");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_toggle_turns_this_buffer_off_and_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\n", |_| {});
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app).len(), 1);
    spell(&mut e.app, SpellCommand::Toggle);
    assert!(message(&e.app).contains("off"));
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty());
    spell(&mut e.app, SpellCommand::Toggle);
    idle(&mut e.app).await;
    assert_eq!(spelled(&e.app), ["recieve"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_open_user_dictionary_creates_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "ok\n", |_| {});
    idle(&mut e.app).await;
    let file = e.config.join("dictionary.txt");
    assert!(!file.exists());
    spell(&mut e.app, SpellCommand::OpenUserDictionary);
    assert!(
        e.app
            .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "dictionary.txt"))
            .await
    );
    assert!(file.exists());
    // An existing file is not overwritten.
    std::fs::write(&file, "mine\n").unwrap();
    spell(&mut e.app, SpellCommand::OpenUserDictionary);
    e.app.settle(|_| true).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_quick_fix_lists_suggestions_in_the_words_case() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve Recieve recieveMessage\n", with_receive);
    idle(&mut e.app).await;
    let tail = [
        "Add \"recieve\" to user dictionary",
        "Add \"recieve\" to workspace dictionary",
        "Ignore in this file",
    ];
    for (at, first) in [(2, "receive"), (10, "Receive"), (18, "receive")] {
        caret(&mut e.app, at);
        let titles = fixes(&mut e.app);
        assert_eq!(titles[0], format!("Replace with \"{first}\""), "{titles:?}");
        assert_eq!(titles[titles.len() - 3..], tail, "{titles:?}");
    }
    // Not on a clean word.
    caret(&mut e.app, 0);
    e.app.press(K::Esc, M::NONE);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_quick_fix_replaces_just_the_piece_in_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "ok recieveMessage ok\n", with_receive);
    idle(&mut e.app).await;
    caret(&mut e.app, 6);
    assert_eq!(fixes(&mut e.app)[0], "Replace with \"receive\"");
    e.app.press(K::Enter, M::NONE);
    assert_eq!(text(&e.app), "ok receiveMessage ok\n");
    assert!(e.app.code_actions.popup.is_none());
    idle(&mut e.app).await;
    assert!(spelled(&e.app).is_empty());
    e.app.test_execute(Command::Undo);
    assert_eq!(text(&e.app), "ok recieveMessage ok\n", "one step");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_quick_fix_handles_several_selections() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve ok recieve ok recieve wrold\n", |a| {
        with_receive(a)
    });
    idle(&mut e.app).await;
    // Three carets in the three `recieve`s (primary the last) and one in the other word.
    let (_, view) = e.app.active_mut().unwrap();
    let c = |n: usize| Selection::cursor(ByteIdx(n));
    view.selections = Selections::new(vec![c(2), c(13), c(23), c(32)], 2);
    let titles = fixes(&mut e.app);
    assert_eq!(titles[0], "Replace with \"receive\"");
    e.app.press(K::Enter, M::NONE);
    assert_eq!(text(&e.app), "receive ok receive ok receive wrold\n");
    e.app.test_execute(Command::Undo);
    assert_eq!(text(&e.app), "recieve ok recieve ok recieve wrold\n", "one undo step");
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_quick_fix_add_and_ignore_rows_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve wrold\n", |_| {});
    idle(&mut e.app).await;
    caret(&mut e.app, 2);
    let titles = fixes(&mut e.app);
    let row = titles.iter().position(|t| t.ends_with("to user dictionary")).unwrap();
    for _ in 0..row {
        e.app.press(K::Down, M::NONE);
    }
    e.app.press(K::Enter, M::NONE);
    assert!(e.app.settle(|a| spelled(a) == ["wrold"]).await, "{:?}", spelled(&e.app));
    caret(&mut e.app, 9);
    let titles = fixes(&mut e.app);
    let row = titles.iter().position(|t| t == "Ignore in this file").unwrap();
    for _ in 0..row {
        e.app.press(K::Down, M::NONE);
    }
    e.app.press(K::Enter, M::NONE);
    assert!(e.app.settle(|a| spelled(a).is_empty()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_the_lightbulb_shows_on_a_spelling_mark() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "ok recieve here\n", |_| {});
    idle(&mut e.app).await;
    caret(&mut e.app, 0);
    assert!(e.app.bulb_line().is_none());
    caret(&mut e.app, 5);
    assert_eq!(e.app.bulb_line().map(|(_, l)| l), Some(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_commands_refuse_on_read_only_and_hex_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve here\n", with_receive);
    idle(&mut e.app).await;
    caret(&mut e.app, 2);
    let id = e.app.group().active_tab().unwrap().buffer;
    e.app.buffers.get_mut(id).unwrap().read_only = Some("locked".into());
    for c in [
        SpellCommand::AddToUser,
        SpellCommand::AddToWorkspace,
        SpellCommand::Ignore,
        SpellCommand::Toggle,
    ] {
        spell(&mut e.app, c);
        assert!(message(&e.app).contains("Read-only"), "{c:?}: {}", message(&e.app));
    }
    assert!(fixes(&mut e.app).is_empty(), "no quick fixes in a read-only buffer");
    e.app.settle(|_| true).await;
    assert!(!e.config.join("dictionary.txt").exists());
    e.app.buffers.get_mut(id).unwrap().read_only = None;

    // A hex tab.
    std::fs::write(e.root.join("b.bin"), [0u8, 1, 2, 0xff, 0, 0, 7]).unwrap();
    e.app.press(K::Char('p'), cmd());
    e.app.type_text("b.bin");
    assert!(
        e.app
            .settle(|a| a
                .palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == "b.bin")))
            .await
    );
    e.app.press(K::Enter, M::NONE);
    assert!(e.app.settle(|a| a.active_hex().is_some()).await);
    for c in SpellCommand::ALL
        .into_iter()
        .filter(|c| *c != SpellCommand::OpenUserDictionary)
    {
        e.app.message = None;
        spell(&mut e.app, c);
        assert!(message(&e.app).contains("text editor"), "{c:?}: {}", message(&e.app));
    }
    e.app.settle(|_| true).await;
    assert!(!e.config.join("dictionary.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_commands_are_in_the_palette_and_unicode_words_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "caf\u{e9} wrold ok\n", |_| {});
    idle(&mut e.app).await;
    // Byte offsets after a two-byte char: wrold starts at 6.
    caret(&mut e.app, 0);
    spell(&mut e.app, SpellCommand::Next);
    assert_eq!(head(&e.app), 6);
    spell(&mut e.app, SpellCommand::AddToUser);
    assert!(
        e.app.settle(|a| !spelled(a).contains(&"wrold".to_string())).await,
        "{:?}",
        spelled(&e.app)
    );
    assert_eq!(
        std::fs::read_to_string(e.config.join("dictionary.txt"))
            .unwrap()
            .lines()
            .last(),
        Some("wrold")
    );
    // The palette lists all seven.
    e.app.press(K::Char('p'), cmd() | M::SHIFT);
    e.app.type_text("Spelling:");
    assert!(
        e.app
            .settle(|a| a
                .palette
                .as_ref()
                .is_some_and(|p| p.rows.len() == SpellCommand::ALL.len()))
            .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spell_check_a_quick_fix_list_closes_when_the_caret_moves_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = open(&dir, "a.txt", "recieve ok recieve\n", with_receive);
    idle(&mut e.app).await;
    caret(&mut e.app, 2);
    assert!(!fixes(&mut e.app).is_empty());
    caret(&mut e.app, 11);
    e.app.press(K::Esc, M::NONE);
    assert!(e.app.code_actions.popup.is_none());
    assert_eq!(text(&e.app), "recieve ok recieve\n");
}
