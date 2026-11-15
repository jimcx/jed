//! Searching a terminal's scrollback (`term_search`): Cmd+F (Ctrl+Shift+F on Linux) over a shell
//! tab opens a bar whose typed query is searched across the scrollback and the live screen, the
//! newest match first ("12 of 12"), Enter / Shift+Enter step with wrapping and scroll the view to
//! the match, the matches are highlighted, the keys never reach the program while the bar is
//! open, and Esc closes it. The matching itself is `tasks::search`'s unit tests.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::shell_tests::{COUNT_TO_200, dirs, draw, screen_text, scrollback, shell_app};

fn find_mods() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    }
}

fn label(app: &crate::app::App) -> String {
    app.term_search.as_ref().map(|s| s.label()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn term_search_finds_in_the_scrollback_and_steps() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    assert!(app.settle(|a| !screen_text(a).trim().is_empty()).await);
    drop(draw(&mut app, 100, 30));
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "199"))
            .await
    );
    assert_eq!(scrollback(&app), 0);

    app.press(KeyCode::Char('f'), find_mods());
    assert!(app.term_search.is_some(), "the bar opened");
    app.type_text("17");
    // 17, 117 and 170..179
    assert_eq!(label(&app), "12 of 12");
    // Typed keys went to the query, not the shell.
    assert!(!screen_text(&app).contains("$ 17"), "{}", screen_text(&app));

    // Wraps to the oldest match, scrolling back to it.
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(label(&app), "1 of 12");
    assert!(scrollback(&app) > 0, "the view went back to line 17");
    assert!(screen_text(&app).lines().any(|l| l.trim_end() == "17"));
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(label(&app), "2 of 12");
    assert!(screen_text(&app).lines().any(|l| l.trim_end() == "117"));
    app.press(KeyCode::Enter, KeyModifiers::SHIFT);
    app.press(KeyCode::Enter, KeyModifiers::SHIFT);
    assert_eq!(label(&app), "12 of 12", "previous wraps to the newest");
    assert!(
        screen_text(&app).lines().any(|l| l.trim_end() == "179"),
        "scrolled to 179"
    );

    // The current match is drawn in the find match colour.
    let term = draw(&mut app, 100, 30);
    let current = app.theme.get("editor.findMatch").bg.unwrap();
    let area = app.hits.group(app.groups.active).unwrap().editor;
    let text = screen_text(&app);
    let row = text.lines().position(|l| l.trim_end() == "179").unwrap() as u16;
    assert_eq!(term.backend().buffer()[(area.x, area.y + row)].bg, current);
    drop(term);

    // Case option and an empty result.
    app.type_text("zz");
    assert_eq!(label(&app), "No results");

    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.term_search.is_none(), "Esc closed the bar");
}

/// With `scrollback_compression` on, old lines are kept compressed (`scrollback_compressed`), and
/// searching still finds and scrolls to a line that has been compressed.
#[tokio::test(flavor = "multi_thread")]
async fn term_search_finds_lines_in_compressed_scrollback() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.settings.scrollback_compression = true;
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    assert!(app.settle(|a| !screen_text(a).trim().is_empty()).await);
    drop(draw(&mut app, 100, 30));
    app.group().active_shell().unwrap().run.write(b"seq 1 3000\n");
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "3000"))
            .await
    );
    {
        let run = &app.group().active_shell().unwrap().run;
        assert!(
            run.screen().screen().scrollback_compressed(),
            "old lines were compressed"
        );
    }

    app.press(KeyCode::Char('f'), find_mods());
    app.type_text("1234");
    assert_eq!(label(&app), "1 of 1");
    assert!(screen_text(&app).lines().any(|l| l.trim_end() == "1234"));
    assert!(scrollback(&app) > 0);
}
