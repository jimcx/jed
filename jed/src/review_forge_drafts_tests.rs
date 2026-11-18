//! Review drafts on the forge, subtask b (Add to Review), driven through the app against the
//! fake Gitea, GitHub and GitLab servers: with `review_drafts_on_forge` on, saving a comment adds
//! it to the forge's pending review, editing it there edits it and deleting it there deletes it;
//! a forge that fails or refuses leaves the draft local (`local only: why` in PENDING); a 404 on
//! a delete drops the draft; Claude's drafts send nothing until the user saves them; the writes
//! run in order (an edit waits for its add's ids); the save button reads `Add to Review` /
//! `Start Review` / `Save Draft` by the setting and the pending review; the setting off sends
//! nothing.

use crate::app::App;
use crate::command::Command;
use crate::forge::fake::{FakeServer, Reply, Request};
use crate::mr_review_tests::write::{comment, repo, select, server_extra};
use crate::mr_review_tests::{draw_wide, github, gitlab, make_repo, open_file, screen, threads};
use crate::review_drafts::Draft;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

const PULLS: &str = "/api/v1/repos/alice/project/pulls/7";

fn drafts(app: &App) -> Vec<Draft> {
    let key = app.review.drafts_key().unwrap();
    app.review.drafts.store.as_ref().unwrap().get(&key).to_vec()
}

/// Waits until no draft write is queued or on its way.
async fn quiet(app: &mut App) {
    assert!(
        app.settle(|a| a.review.drafts.inflight.is_none() && a.review.drafts.queue.is_empty())
            .await
    );
}

fn wide(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..30)
        .map(|y| (0..140).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn sent(server: &FakeServer, method: &str, ends_with: &str) -> Vec<Request> {
    server
        .requests()
        .into_iter()
        .filter(|r| r.method == method && r.target.ends_with(ends_with))
        .collect()
}

/// Replaces the open box's text and saves it.
fn rewrite(app: &mut App, text: &str) {
    app.test_execute(Command::SelectAll);
    app.type_text(text);
    app.press_bound(Command::ReviewSaveComment);
}

fn pending_review() -> Reply {
    Reply::json(r#"{"id":5,"user":{"login":"sam"},"body":"","state":"PENDING"}"#)
}

async fn gitea(dir: &std::path::Path, review: Reply, extra: Vec<(String, Reply)>) -> (App, FakeServer) {
    let (root, head, main) = repo(dir);
    let server = server_extra(&main, &head, &head, review, extra);
    let mut app = crate::mr_review_tests::write::boot(&root, &server, None).await;
    app.settings.review_drafts_on_forge = true;
    (app, server)
}

fn gitea_comment_routes(head: &str) -> Vec<(String, Reply)> {
    let _ = head;
    vec![
        // A later add re-reads the pending review's body.
        (
            format!("{PULLS}/reviews/5"),
            Reply::json(r#"{"id":5,"body":"","state":"PENDING"}"#),
        ),
        (
            format!("{PULLS}/reviews/5/comments"),
            Reply::json(&format!(
                "[{}]",
                threads::rc(99, "sam", "x", "mod.txt", 3, &"a".repeat(40), "+m3", false)
            )),
        ),
        (
            "PATCH /api/v1/repos/alice/project/issues/comments/99".into(),
            Reply::json("{}"),
        ),
        (
            "DELETE /api/v1/repos/alice/project/issues/comments/99".into(),
            Reply::status(204),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_gitea_add_edit_and_delete_reach_the_pending_review() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    comment(&mut app, 1, 2, "First");
    quiet(&mut app).await;
    let post = sent(&server, "POST", "/pulls/7/reviews");
    assert_eq!(post.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&post[0].body).unwrap();
    assert_eq!(v["event"], "PENDING");
    assert!(v["comments"][0]["body"].as_str().unwrap().starts_with("First"));
    let d = &drafts(&app)[0];
    let f = d.forge.as_ref().unwrap();
    assert_eq!((f.review_id.as_str(), f.comment_id.as_str()), ("5", "99"));
    assert!(
        screen(&mut app).contains("PENDING (1, on Gitea)"),
        "{}",
        screen(&mut app)
    );

    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Second");
    quiet(&mut app).await;
    let patch = sent(&server, "PATCH", "/issues/comments/99");
    assert_eq!(patch.len(), 1);
    assert!(patch[0].body.contains("Second"));
    assert_eq!(drafts(&app)[0].body, "Second");

    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert_eq!(sent(&server, "DELETE", "/issues/comments/99").len(), 1);
    assert!(drafts(&app).is_empty());
    // Nothing was published.
    assert_eq!(sent(&server, "POST", "/pulls/7/reviews").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_github_add_edit_and_delete_reach_the_pending_review() {
    let dir = tempfile::tempdir().unwrap();
    let (root, head, main) = repo(dir.path());
    let extra = vec![
        (
            "POST /api/v3/repos/alice/project/pulls/7/reviews ~commit_id".to_string(),
            Reply::json(r#"{"id":77,"node_id":"PRR_77","user":{"login":"sam"}}"#),
        ),
        (
            "POST /api/graphql ~addPullRequestReviewThread(".into(),
            Reply::json(
                r#"{"data":{"addPullRequestReviewThread":{"thread":{"comments":{"nodes":[{"id":"PRRC_new"}]}}}}}"#,
            ),
        ),
        (
            "POST /api/graphql ~updatePullRequestReviewComment".into(),
            Reply::json(
                r#"{"data":{"updatePullRequestReviewComment":{"pullRequestReviewComment":{"id":"PRRC_new"}}}}"#,
            ),
        ),
        (
            "POST /api/graphql ~deletePullRequestReviewComment".into(),
            Reply::json(
                r#"{"data":{"deletePullRequestReviewComment":{"pullRequestReviewComment":{"id":"PRRC_new"}}}}"#,
            ),
        ),
    ];
    let server = github::server_plus(&main, &head, true, false, extra);
    let mut app = github::boot_github(&root, &server).await;
    app.settings.review_drafts_on_forge = true;
    open_file(&mut app, "mod.txt").await;
    draw_wide(&mut app, 120);
    comment(&mut app, 1, 2, "First");
    quiet(&mut app).await;
    let graphql = |needle: &str| {
        server
            .requests()
            .into_iter()
            .filter(|r| r.target == "/api/graphql" && r.body.contains(needle))
            .collect::<Vec<_>>()
    };
    let add = graphql("addPullRequestReviewThread(");
    assert_eq!(add.len(), 1);
    assert!(add[0].body.contains("First") && add[0].body.contains("PRR_77"));
    let f = drafts(&app)[0].forge.clone().unwrap();
    assert_eq!((f.review_id.as_str(), f.comment_id.as_str()), ("77:PRR_77", "PRRC_new"));
    assert!(screen(&mut app).contains("on GitHub"));

    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Second");
    quiet(&mut app).await;
    let edit = graphql("updatePullRequestReviewComment");
    assert_eq!(edit.len(), 1);
    assert!(edit[0].body.contains("Second") && edit[0].body.contains("PRRC_new"));

    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert_eq!(graphql("deletePullRequestReviewComment").len(), 1);
    assert!(drafts(&app).is_empty());
    // One pending review was started for all of it.
    assert_eq!(sent(&server, "POST", "/pulls/7/reviews").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_gitlab_add_edit_and_delete_reach_the_draft_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let notes = format!("{}/merge_requests/7/draft_notes", gitlab::API);
    let extra = vec![
        (format!("PUT {notes}/31"), Reply::json("{}")),
        (format!("DELETE {notes}/31"), Reply::status(204)),
    ];
    let server = gitlab::server_with_routes(&main, &head, Reply::status(204), "bob", "Remark", extra);
    let mut app = gitlab::boot(&root, &server, None).await;
    app.settings.review_drafts_on_forge = true;
    comment(&mut app, 1, 2, "First");
    quiet(&mut app).await;
    let post = sent(&server, "POST", "/draft_notes");
    assert_eq!(post.len(), 1);
    assert!(post[0].body.contains("First"));
    assert_eq!(drafts(&app)[0].forge.as_ref().unwrap().comment_id, "31");
    assert!(screen(&mut app).contains("on GitLab"));

    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Second");
    quiet(&mut app).await;
    let put = sent(&server, "PUT", "/draft_notes/31");
    assert_eq!(put.len(), 1);
    assert!(put[0].body.contains("Second"));

    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert_eq!(sent(&server, "DELETE", "/draft_notes/31").len(), 1);
    assert!(drafts(&app).is_empty());
    // Never published.
    assert!(sent(&server, "POST", "/bulk_publish").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_failed_add_keeps_the_draft_local_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let failing = Reply::status(500).with_body(r#"{"message":"boom"}"#);
    let (mut app, server) = gitea(dir.path(), failing, Vec::new()).await;
    comment(&mut app, 1, 2, "Keep me");
    quiet(&mut app).await;
    let d = drafts(&app);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].forge, None);
    let why = app.review.drafts.local_why.get(&d[0].id).cloned().unwrap();
    assert!(why.contains("500"), "{why}");
    assert!(app.review.drafts.pending.is_none());
    assert_eq!(sent(&server, "POST", "/pulls/7/reviews").len(), 1);
    assert!(!screen(&mut app).contains("on Gitea"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_pending_says_local_only_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let failing = Reply::status(500).with_body(r#"{"message":"boom"}"#);
    let (mut app, _server) = gitea(dir.path(), failing, Vec::new()).await;
    comment(&mut app, 1, 2, "Keep me");
    quiet(&mut app).await;
    app.review.view.pending.collapsed = false;
    app.ui.focus = crate::app::Focus::SidePanel;
    let text = wide(&mut app);
    assert!(text.contains("local only: "), "{text}");
    assert!(text.contains("500"), "{text}");
    assert!(!text.contains("on Gitea"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_delete_while_the_add_is_on_its_way_takes_it_off_the_forge() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    comment(&mut app, 1, 2, "Short lived");
    assert!(app.review.drafts.inflight.is_some());
    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert_eq!(sent(&server, "POST", "/pulls/7/reviews").len(), 1);
    assert_eq!(sent(&server, "DELETE", "/issues/comments/99").len(), 1);
    assert!(drafts(&app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_404_on_edit_drops_the_draft() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = gitea_comment_routes("");
    routes.retain(|(t, _)| !t.starts_with("PATCH"));
    let (mut app, server) = gitea(dir.path(), pending_review(), routes).await;
    comment(&mut app, 1, 2, "First");
    quiet(&mut app).await;
    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Second");
    quiet(&mut app).await;
    assert_eq!(sent(&server, "PATCH", "/issues/comments/99").len(), 1);
    assert!(drafts(&app).is_empty());
    assert!(
        app.message
            .as_ref()
            .is_some_and(|m| m.text.contains("already gone on Gitea"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_404_on_delete_drops_the_draft() {
    let dir = tempfile::tempdir().unwrap();
    // No DELETE route: the fake answers 404 (the draft was deleted on the website).
    let mut routes = gitea_comment_routes("");
    routes.retain(|(t, _)| !t.starts_with("DELETE"));
    let (mut app, server) = gitea(dir.path(), pending_review(), routes).await;
    comment(&mut app, 1, 2, "Gone");
    quiet(&mut app).await;
    assert!(drafts(&app)[0].forge.is_some());
    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert_eq!(sent(&server, "DELETE", "/issues/comments/99").len(), 1);
    assert!(drafts(&app).is_empty());
    assert!(
        app.message
            .as_ref()
            .is_some_and(|m| m.text.contains("already gone on Gitea"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_failed_delete_keeps_the_draft_with_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut routes = gitea_comment_routes("");
    routes.retain(|(t, _)| !t.starts_with("DELETE"));
    routes.push((
        "DELETE /api/v1/repos/alice/project/issues/comments/99".into(),
        Reply::status(500),
    ));
    let (mut app, _server) = gitea(dir.path(), pending_review(), routes).await;
    comment(&mut app, 1, 2, "Stays");
    quiet(&mut app).await;
    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    let d = drafts(&app);
    assert_eq!(d.len(), 1);
    assert!(app.review.drafts.errors.contains_key(&d[0].id));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_claude_draft_sends_nothing_until_the_user_saves_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    // A local draft turned into Claude's (the store is where the MCP tool puts them).
    app.settings.review_drafts_on_forge = false;
    comment(&mut app, 1, 2, "From Claude");
    app.settings.review_drafts_on_forge = true;
    let key = app.review.drafts_key().unwrap();
    let mut d = drafts(&app).remove(0);
    let store = app.review.drafts.store.as_mut().unwrap();
    store.remove(&key, &d.id);
    d.by_claude = true;
    store.add(&key, d);
    // The user's own draft elsewhere goes to the forge; Claude's does not follow it.
    comment(&mut app, 2, 2, "Mine");
    quiet(&mut app).await;
    let posts = sent(&server, "POST", "/pulls/7/reviews");
    assert_eq!(posts.len(), 1);
    assert!(posts[0].body.contains("Mine") && !posts[0].body.contains("From Claude"));
    let claude = drafts(&app).into_iter().find(|d| d.by_claude).unwrap();
    assert!(claude.forge.is_none());
    // The user edits and saves it: now it is theirs and goes to the forge.
    select(&mut app, true, 1, 1);
    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Edited by the user");
    quiet(&mut app).await;
    let d = drafts(&app).into_iter().find(|d| d.id == claude.id).unwrap();
    assert!(!d.by_claude && d.forge.is_some(), "{d:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_an_edit_right_after_the_add_runs_after_it_with_its_ids() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    comment(&mut app, 1, 2, "First");
    // No settling: the add is still on its way.
    assert!(app.review.drafts.inflight.is_some());
    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Second");
    quiet(&mut app).await;
    let order: Vec<(String, String)> = server
        .requests()
        .into_iter()
        .filter(|r| (r.method == "POST" && r.target.ends_with("/pulls/7/reviews")) || r.method == "PATCH")
        .map(|r| (r.method, r.target))
        .collect();
    assert_eq!(order.len(), 2, "{order:?}");
    assert_eq!(order[0].0, "POST");
    assert!(order[1].0 == "PATCH" && order[1].1.ends_with("/issues/comments/99"));
    assert_eq!(drafts(&app)[0].body, "Second");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_the_save_button_says_what_saving_does() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    select(&mut app, true, 1, 2);
    app.test_execute(Command::ReviewComment);
    assert!(wide(&mut app).contains("[Start Review Ctrl+Enter]"));
    app.test_execute(Command::ReviewCancelComment);
    comment(&mut app, 1, 2, "First");
    quiet(&mut app).await;
    select(&mut app, true, 2, 2);
    app.test_execute(Command::ReviewComment);
    let text = wide(&mut app);
    assert!(text.contains("[Add to Review Ctrl+Enter]"), "{text}");
    app.test_execute(Command::ReviewCancelComment);
    app.settings.review_drafts_on_forge = false;
    select(&mut app, true, 2, 2);
    app.test_execute(Command::ReviewComment);
    assert!(wide(&mut app).contains("[Save Draft Ctrl+Enter]"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_with_the_setting_off_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = gitea(dir.path(), pending_review(), gitea_comment_routes("")).await;
    app.settings.review_drafts_on_forge = false;
    comment(&mut app, 1, 2, "Local");
    quiet(&mut app).await;
    app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut app, "Still local");
    quiet(&mut app).await;
    app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut app).await;
    assert!(drafts(&app).is_empty());
    assert!(
        server.requests().iter().all(|r| r.method == "GET"),
        "{:?}",
        server.requests()
    );
}
