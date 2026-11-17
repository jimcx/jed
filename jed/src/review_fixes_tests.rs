//! Review fixes, subtask a: the review's FILES list only the merge request's own files (the merge
//! base's tree against the MR head, else local HEAD), shown as the work tree has them: untracked
//! files and locally edited files outside the MR are hidden, an MR file edited locally shows with
//! the edit's counts, and a local HEAD behind the MR still lists the MR's files.
//!
//! Subtask d: a thread out of its file's context (outdated, missing) opens in a thread tab, where Reply and
//! Resolve work (`mod thread_tab`). Subtask e: threads on a commit the clone lacks are placed when the file matches (`mod behind`).

use crate::git::Repo;
use crate::git::range::{FileChange, changes};
use crate::git::test_repo::{commit_all, git, init};

/// A repo: `base` on main with a.txt, b.txt, c.txt; branch `feature` changes a.txt and adds mr.txt.
/// Returns (dir, base sha, feature sha); `main` is checked out afterwards when `behind`.
fn setup(behind: bool) -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    for f in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(p.join(f), "one\ntwo\n").unwrap();
    }
    commit_all(p, "base");
    let base = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    git(p, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(p.join("mr.txt"), "m\n").unwrap();
    commit_all(p, "work");
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    if behind {
        git(p, &["checkout", "-q", "main"]);
    }
    (dir, base, head)
}

fn names(files: &[FileChange]) -> Vec<&str> {
    let mut v: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    v.sort();
    v
}

#[test]
fn review_fixes_untracked_and_unrelated_edits_are_hidden() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::write(p.join("untracked.txt"), "u\n").unwrap();
    std::fs::write(p.join("b.txt"), "one\ntwo\nlocal\n").unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(names(&files), ["a.txt", "mr.txt"]);
}

#[test]
fn review_fixes_a_local_edit_of_an_mr_file_shows_its_counts() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::write(p.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    let a = files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!(a.additions, 3);
    assert_eq!(a.status, 'M');
}

#[test]
fn review_fixes_an_mr_file_deleted_locally_shows_as_deleted() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::remove_file(p.join("a.txt")).unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(files.iter().find(|f| f.path == "a.txt").unwrap().status, 'D');
}

#[test]
fn review_fixes_local_head_behind_the_mr_lists_the_mrs_files() {
    let (dir, base, head) = setup(true);
    let repo = Repo::discover(dir.path()).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(names(&files), ["a.txt", "mr.txt"]);
    // Without the MR head in the clone, local HEAD (main, no changes) lists nothing.
    let none = changes(&repo, &base, Some(&"0".repeat(40))).unwrap();
    assert!(none.is_empty());
}

#[test]
fn review_fixes_an_appended_file_hashes_the_mrs_blob_not_the_work_tree() {
    let (dir, base, head) = setup(true);
    let repo = Repo::discover(dir.path()).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    let mr = files.iter().find(|f| f.path == "mr.txt").unwrap();
    let blob = git(dir.path(), &["rev-parse", &format!("{head}:mr.txt")]);
    assert_eq!(mr.hash, blob.trim());
}

/// Subtask b: FILES is a collapsing section like CONVERSATIONS.
mod files_section {
    use crate::command::{Command, ReviewCommand as C};
    use crate::mr_review_tests::threads::loaded_app;
    use crate::mr_review_tests::{idle, screen};
    use crossterm::event::{MouseButton, MouseEventKind};

    fn click_files_header(app: &mut crate::app::App) {
        let h = app.hits.review.clone().unwrap().files_header;
        assert_eq!(h.height, 1);
        app.mouse(MouseEventKind::Down(MouseButton::Left), h.x + 4, h.y);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_files_header_click_collapses_and_conversations_gets_the_space() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        let text = screen(&mut app);
        assert!(text.contains("▾ FILES"), "{text}");
        let rows_before = app.hits.review.clone().unwrap().conv_list.height;
        assert!(!app.hits.review.clone().unwrap().rows.is_empty());

        click_files_header(&mut app);
        let text = screen(&mut app);
        assert!(app.review.view.files_collapsed);
        assert!(text.contains("▸ FILES"), "{text}");
        let hit = app.hits.review.clone().unwrap();
        assert!(hit.rows.is_empty(), "file rows are gone");
        assert!(hit.conv_list.height > rows_before, "CONVERSATIONS fills the space");
        assert_eq!(hit.conv_header.y, hit.files_header.y + 2, "divider, then the header");

        // A lookup does not reset it.
        app.test_execute(Command::ReviewRefresh);
        idle(&mut app).await;
        screen(&mut app);
        assert!(app.review.view.files_collapsed);

        click_files_header(&mut app);
        let text = screen(&mut app);
        assert!(!app.review.view.files_collapsed);
        assert!(text.contains("▾ FILES"), "{text}");
        assert!(!app.hits.review.clone().unwrap().rows.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_the_command_and_keys_toggle_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.test_execute(Command::ReviewToggleFiles);
        assert!(app.review.view.files_collapsed && app.review.view.files_header);
        // Collapsed: Down skips the hidden rows into CONVERSATIONS; Up comes back to the header.
        app.test_execute(Command::Review(C::Down));
        assert!(app.review.view.conv.focused);
        app.test_execute(Command::Review(C::Up));
        assert!(!app.review.view.conv.focused && app.review.view.files_collapsed);
        // Enter on the header expands, Space collapses again.
        app.test_execute(Command::Review(C::Open));
        assert!(!app.review.view.files_collapsed);
        app.test_execute(Command::Review(C::ToggleViewed));
        assert!(app.review.view.files_collapsed);
        app.test_execute(Command::Review(C::Open));
        // Expanded with the header selected: Down goes to the first file.
        app.test_execute(Command::Review(C::Down));
        assert!(!app.review.view.files_header);
        // Up from the first file selects the header again.
        app.test_execute(Command::Review(C::Up));
        assert!(app.review.view.files_header);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_the_divider_drags_while_files_is_collapsed() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.test_execute(Command::ReviewToggleFiles);
        screen(&mut app);
        let divider = app.hits.review.clone().unwrap().dividers[0].0;
        let height = app.hits.review.clone().unwrap().conv_list.height;
        // The section fills the panel (it took FILES' rows), so a drag can't move its divider; the
        // stored height is what it has once FILES expands again.
        app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 5, divider.y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 5, divider.y + 2);
        app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 5, divider.y + 2);
        screen(&mut app);
        let hit = app.hits.review.clone().unwrap();
        assert_eq!(hit.dividers[0].0.y, divider.y, "the section keeps filling the panel");
        assert_eq!(hit.conv_list.height, height);
        app.test_execute(Command::ReviewToggleFiles);
        screen(&mut app);
        let hit = app.hits.review.clone().unwrap();
        assert!(hit.conv_list.height < height, "expanded, it has only its stored height");
        assert!(hit.conv_list.height > 0);
    }
}

mod approvals {
    use crate::forge::{Approval, ApprovalState, Verdict, latest_approvals};
    use crate::mr_review_tests::threads::loaded_app_reviews;
    use crate::mr_review_tests::{idle, screen};

    fn v(who: &str, s: Option<ApprovalState>) -> (String, Verdict) {
        (who.to_string(), s.map_or(Verdict::Keep, Verdict::Set))
    }

    #[test]
    fn review_fixes_latest_verdict_per_author_wins() {
        use ApprovalState::*;
        let got = latest_approvals([
            v("sam", Some(Approved)),
            v("bob", Some(ChangesRequested)),
            v("sam", None),
            v("bob", Some(Approved)),
            v("sam", Some(ChangesRequested)),
            v("ann", None),
        ]);
        let want = [("bob", Approved), ("sam", ChangesRequested)]
            .map(|(n, s)| Approval {
                author: n.into(),
                state: s,
                id: None,
            })
            .to_vec();
        assert_eq!(got, want);
    }

    /// An approval with an empty body is no conversation entry, but the panel names the approver.
    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_the_summary_shows_who_approved_and_who_asked_for_changes() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = r#"[
            {"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9,"submitted_at":"2026-01-02T10:00:00Z","html_url":"u"},
            {"id":2,"user":{"login":"sam"},"body":"","state":"APPROVED","submitted_at":"2026-01-02T11:00:00Z","html_url":"u"},
            {"id":3,"user":{"login":"alex"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-02T12:00:00Z","html_url":"u"},
            {"id":4,"user":{"login":"alex"},"body":"","state":"APPROVED","submitted_at":"2026-01-02T13:00:00Z","html_url":"u"},
            {"id":5,"user":{"login":"zed"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-02T14:00:00Z","html_url":"u"}]"#;
        let (mut app, _server) = loaded_app_reviews(dir.path(), None, Some(reviews)).await;
        idle(&mut app).await;
        let r = app.review.review().unwrap();
        assert_eq!(
            r.conversation.len(),
            1,
            "the empty-bodied reviews add no conversation entries"
        );
        let text = screen(&mut app);
        assert!(text.contains("✓ approved by sam, alex"), "{text}");
        assert!(text.contains("✗ changes requested by zed"), "{text}");
        assert!(!text.contains("by bob"), "{text}");
    }

    /// Reviews without any verdict draw no approval lines.
    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_no_verdicts_no_approval_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app_reviews(dir.path(), None, None).await;
        let text = screen(&mut app);
        assert!(
            !text.contains("approved by") && !text.contains("changes requested"),
            "{text}"
        );
    }
}

/// Subtask d: a thread out of its file's context opens in a thread tab (its code, its comments;
/// Reply and Resolve work there), reused per thread id.
mod thread_tab {
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use crate::app::App;
    use crate::command::Command;
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::idle;
    use crate::mr_review_tests::threads::{loaded_app, loaded_app_with};

    const PULLS: &str = "/api/v1/repos/alice/project/pulls";
    /// mod.txt as the work tree has it after an edit that outdates thread 11 (line 3).
    const EDITED: (&str, &str) = ("mod.txt", "m1\nm2 changed\nm3 edited\n");

    fn wide(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..30)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Enter on CONVERSATIONS row `i` (by path and line: 0 is thread 13, 1 is 12, 2 is 11).
    async fn open_row(app: &mut App, i: usize) {
        let len = app.review.review().unwrap().conv_len();
        app.review.view.conv.focused = true;
        app.review.view.conv.select(len, i);
        app.ui.focus = crate::app::Focus::SidePanel;
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.diffs_settled()).await);
    }

    fn thread_tabs(app: &App) -> Vec<String> {
        app.group()
            .tabs
            .iter()
            .filter_map(|t| t.diff().and_then(|d| d.thread.as_ref()).map(|t| t.id.clone()))
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_an_outdated_thread_opens_in_a_thread_tab_with_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), Some(EDITED)).await;
        open_row(&mut app, 2).await;
        assert_eq!(thread_tabs(&app), ["11"]);
        assert_eq!(app.group().active_diff().unwrap().title(), "Thread mod.txt:3");
        let text = wide(&mut app);
        // The code at the anchor commit, then the thread's block.
        assert!(text.contains("m2 changed"), "{text}");
        assert!(text.contains("Can this overflow"), "{text}");
        assert!(text.contains("[Reply] [Resolve]"), "{text}");
        assert!(!text.contains("Apply"), "{text}");
        assert!(!text.contains("m3 edited"), "the work tree's text isn't shown: {text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_the_thread_tab_is_reused_and_open_thread_works_on_placed_threads() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        // A placed thread's row opens the file's diff...
        open_row(&mut app, 2).await;
        assert!(thread_tabs(&app).is_empty());
        // ...and Open Thread opens its tab, once.
        app.review.view.conv.focused = true;
        app.test_execute(Command::ReviewOpenThread);
        app.test_execute(Command::ReviewOpenThread);
        assert_eq!(thread_tabs(&app), ["11"]);
        open_row(&mut app, 0).await;
        assert_eq!(thread_tabs(&app), ["11", "13"]);
        open_row(&mut app, 0).await;
        assert_eq!(thread_tabs(&app), ["11", "13"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_a_missing_commit_shows_the_hunk_or_the_block_alone() {
        // Thread 13 has a hunk; the other has none; both anchor a commit the clone doesn't have.
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        open_row(&mut app, 0).await;
        let text = wide(&mut app);
        assert!(text.contains("Old commit"), "{text}");
        assert!(text.contains("m1"), "the hunk's line: {text}");

        let dir = tempfile::tempdir().unwrap();
        let bare = format!(
            r#"[{{"id":21,"user":{{"login":"cy"}},"body":"No hunk here","path":"mod.txt","position":1,
            "original_position":1,"commit_id":"{d}","original_commit_id":"{d}","diff_hunk":"","resolver":null}}]"#,
            d = "d".repeat(40)
        );
        let (mut app, _server) = loaded_app_with(dir.path(), None, None, Some(&bare), Vec::new()).await;
        open_row(&mut app, 0).await;
        let text = wide(&mut app);
        assert!(text.contains("No hunk here"), "{text}");
        assert!(text.contains("the code isn't available"), "{text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_edit_and_delete_work_in_the_thread_tab() {
        let dir = tempfile::tempdir().unwrap();
        let extra = vec![
            ("/api/v1/user".to_string(), Reply::json(r#"{"login":"sam"}"#)),
            (
                "PATCH /api/v1/repos/alice/project/issues/comments/11".to_string(),
                Reply::json("{}"),
            ),
            (
                "DELETE /api/v1/repos/alice/project/issues/comments/11".to_string(),
                Reply::status(204),
            ),
        ];
        let (mut app, server) = loaded_app_with(dir.path(), Some(EDITED), None, None, extra).await;
        open_row(&mut app, 2).await;
        app.review.view.conv.focused = false;
        app.test_execute(Command::ReviewEditComment);
        assert_eq!(
            app.test_composer_text().as_deref(),
            Some("Can this overflow on a narrow terminal")
        );
        app.type_text("?");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        let patch = server.requests().into_iter().find(|r| r.method == "PATCH").unwrap();
        assert_eq!(patch.body, r#"{"body":"Can this overflow on a narrow terminal?"}"#);
        idle(&mut app).await;
        app.test_execute(Command::ReviewDeleteComment);
        assert!(matches!(
            app.prompt,
            Some(crate::app::Prompt::ConfirmDeleteComment { .. })
        ));
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            app.settle(|_| server.requests().iter().any(|r| r.method == "DELETE"))
                .await
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_a_thread_tab_never_stands_in_for_the_files_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), Some(EDITED)).await;
        open_row(&mut app, 2).await;
        let path = app.group().active_diff().unwrap().path.clone();
        // Thread 12 is placed: its row opens the file's review diff beside the thread tab.
        open_row(&mut app, 1).await;
        assert!(app.group().active_diff().unwrap().pair.is_review());
        let n = app
            .group()
            .tabs
            .iter()
            .filter(|t| t.diff().is_some_and(|d| d.path == path))
            .count();
        assert_eq!(n, 2, "the thread tab and the file's diff");
        // Another thread of the same file is a tab of its own.
        open_row(&mut app, 0).await;
        assert_eq!(thread_tabs(&app), ["11", "13"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_reply_and_resolve_work_in_the_thread_tab() {
        let dir = tempfile::tempdir().unwrap();
        let extra = vec![
            (format!("POST {PULLS}/7/comments/11/replies"), Reply::json("{}")),
            (format!("POST {PULLS}/comments/11/resolve"), Reply::status(204)),
        ];
        let (mut app, server) = loaded_app_with(dir.path(), Some(EDITED), None, None, extra).await;
        open_row(&mut app, 2).await;
        // The keys are the editor's now: the tab's own thread is the target.
        app.review.view.conv.focused = false;
        app.test_execute(Command::ReviewReply);
        assert!(app.test_composer_text().is_some());
        app.type_text("Fixed it");
        let text = wide(&mut app);
        assert!(text.contains("Reply (posted at once)"), "{text}");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        let reqs = server.requests();
        let reply = reqs
            .iter()
            .find(|r| r.target.ends_with("/comments/11/replies"))
            .unwrap();
        assert_eq!(reply.body, r#"{"body":"Fixed it"}"#);
        idle(&mut app).await;
        assert_eq!(thread_tabs(&app), ["11"]);
        app.test_execute(Command::ReviewResolve);
        assert!(app.settle(|a| !a.review.writing).await);
        assert!(
            server
                .requests()
                .iter()
                .any(|r| r.target.ends_with("/comments/11/resolve"))
        );
    }
}

/// Subtask e: threads on a commit the clone lacks are placed when the host's blob id of the file
/// there equals the work tree's (`mod behind`).
mod behind {
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use crate::app::App;
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::screen;
    use crate::mr_review_tests::threads::loaded_app_with;
    use crate::review_anchor::Placement;

    const CONTENTS: &str = "/api/v1/repos/alice/project/contents/mod.txt";
    /// The work tree's mod.txt.
    const TEXT: &str = "m1\nm2 changed\nm3\n";

    fn blob(text: &str) -> String {
        git2::Oid::hash_object(git2::ObjectType::Blob, text.as_bytes())
            .unwrap()
            .to_string()
    }

    fn comments() -> String {
        let d = "d".repeat(40);
        let one = |id: u32, line: u32, body: &str| {
            format!(
                r#"{{"id":{id},"user":{{"login":"cy"}},"body":"{body}","path":"mod.txt","position":{line},
                "original_position":{line},"commit_id":"{d}","original_commit_id":"{d}","diff_hunk":"","resolver":null}}"#
            )
        };
        format!("[{},{}]", one(21, 1, "First behind"), one(22, 3, "Second behind"))
    }

    fn extra(sha: &str) -> Vec<(String, Reply)> {
        vec![(
            format!("{CONTENTS}?ref={}", "d".repeat(40)),
            Reply::json(&format!(r#"{{"name":"mod.txt","sha":"{sha}"}}"#)),
        )]
    }

    fn places(app: &App) -> Vec<(String, Placement, bool)> {
        let r = app.review.review().unwrap();
        r.threads
            .iter()
            .map(|t| (t.thread.id.clone(), t.place.clone(), t.behind))
            .collect()
    }

    fn wide(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..30)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Contents requests, and lookups (the panel looks up when it loads and again when shown, and
    /// each looks up the merge request once): the blob ids are kept, so one request in all.
    fn contents_requests(server: &crate::forge::fake::FakeServer) -> (usize, usize) {
        let reqs = server.requests();
        let n = |p: &str| reqs.iter().filter(|r| r.target.split('?').next() == Some(p)).count();
        (n(CONTENTS), n("/api/v1/repos/alice/project/pulls/7"))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_a_thread_on_a_newer_commit_is_placed_when_the_file_matches() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, server) = loaded_app_with(dir.path(), None, None, Some(&comments()), extra(&blob(TEXT))).await;
        let p = places(&app);
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|(_, _, behind)| *behind), "{p:?}");
        let right = crate::forge::Side::Right;
        assert_eq!(
            p[0].1,
            Placement::Placed {
                side: right,
                lines: 0..1
            }
        );
        assert_eq!(
            p[1].1,
            Placement::Placed {
                side: right,
                lines: 2..3
            }
        );
        // Two threads, one file and commit: one request.
        let (asked, lookups) = contents_requests(&server);
        assert!(lookups >= 2 && asked == 1, "{asked} requests in {lookups} lookups");
        let text = screen(&mut app);
        assert!(text.contains("newer commit"), "{text}");
        assert!(!text.contains("no commit"), "{text}");
        // Its row opens the file's diff (not a thread tab), which draws the thread.
        let len = app.review.review().unwrap().conv_len();
        app.review.view.conv.focused = true;
        app.review.view.conv.select(len, 0);
        app.ui.focus = crate::app::Focus::SidePanel;
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.diffs_settled()).await);
        assert!(app.group().active_diff().unwrap().pair.is_review());
        let text = wide(&mut app);
        assert!(text.contains("First behind"), "{text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_a_thread_on_a_newer_commit_stays_missing_when_the_file_differs() {
        let dir = tempfile::tempdir().unwrap();
        let extra = extra(&blob("something else\n"));
        let (app, server) = loaded_app_with(dir.path(), None, None, Some(&comments()), extra).await;
        for (id, place, behind) in places(&app) {
            assert_eq!((place, behind), (Placement::Missing, false), "{id}");
        }
        let (asked, lookups) = contents_requests(&server);
        assert!(lookups >= 2 && asked == 1, "{asked} requests in {lookups} lookups");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_fixes_a_thread_on_a_newer_commit_stays_missing_when_the_host_has_no_file() {
        // No contents route: the fake answers 404.
        let dir = tempfile::tempdir().unwrap();
        let (app, server) = loaded_app_with(dir.path(), None, None, Some(&comments()), Vec::new()).await;
        assert!(places(&app).iter().all(|(_, p, b)| *p == Placement::Missing && !b));
        // A 404 is an answer too: asked once.
        assert_eq!(contents_requests(&server).0, 1);
    }
}
