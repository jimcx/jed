//! Review drafts on the forge, subtask b (Add to Review), driven through the app against the
//! fake Gitea, GitHub and GitLab servers: with `review_drafts_on_forge` on, saving a comment adds
//! it to the forge's pending review, editing it there edits it and deleting it there deletes it;
//! a forge that fails or refuses leaves the draft local (`local only: why` in PENDING); a 404 on
//! a delete drops the draft; Claude's drafts send nothing until the user saves them; the writes
//! run in order (an edit waits for its add's ids); the save button reads `Add to Review` /
//! `Start Review` / `Save Draft` by the setting and the pending review; the setting off sends
//! nothing.
//!
//! Subtask c (reconcile, submit, discard): a draft made on the website appears in PENDING and is
//! edited, deleted and submitted from jed; one submitted elsewhere is dropped with a notice; each
//! verdict submits the pending review in one request on each fake; on-forge, local only and
//! Claude's drafts go in one submit; Discard Review; Open on Forge from PENDING; the done-when.

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

// ---- subtask c: reconcile, submit, discard ------------------------------------------------------

use crate::app::Prompt;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
enum F {
    Gitea,
    GitHub,
    GitLab,
}

struct Rig {
    app: App,
    server: FakeServer,
    f: F,
    _dir: tempfile::TempDir,
}

const LIST: &str = "/api/v1/repos/alice/project/pulls/7/reviews?page=1&limit=50";
const GH: &str = "/api/v3/repos/alice/project";

fn gitea_list(pending: bool) -> String {
    let bob = r#"{"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9}"#;
    if pending {
        format!(r#"[{bob},{{"id":5,"user":{{"login":"sam"}},"body":"","state":"PENDING"}}]"#)
    } else {
        format!("[{bob}]")
    }
}

/// A fake forge whose user has a pending review holding one comment made on the website
/// (`web`) or none; `post` answers a plain `POST …/reviews` (Gitea's old path / a new pending review).
async fn rig(f: F, web: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, server) = match f {
        F::Gitea => {
            let (root, head, main) = repo(dir.path());
            let comment = if web {
                threads::rc(99, "sam", "From the web", "mod.txt", 3, &head, "+m3", false)
            } else {
                threads::rc(100, "sam", "x", "mod.txt", 3, &head, "+m3", false)
            };
            let mut extra = gitea_comment_routes("");
            extra.retain(|(t, _)| !t.starts_with(&format!("{PULLS}/reviews/5/comments")));
            extra.extend([
                (
                    format!("{PULLS}/reviews/5/comments"),
                    Reply::json(&format!("[{comment}]")),
                ),
                (format!("POST {PULLS}/reviews/5"), Reply::json("{}")),
                (format!("DELETE {PULLS}/reviews/5"), Reply::status(204)),
            ]);
            let server = crate::mr_review_tests::write::server_reviews(
                &main,
                &head,
                &head,
                pending_review(),
                &gitea_list(web),
                extra,
            );
            let app = crate::mr_review_tests::write::boot(&root, &server, None).await;
            (app, server)
        }
        F::GitHub => {
            let (root, head, main) = repo(dir.path());
            let ok = |name: &str| {
                format!(
                    r#"{{"data":{{"{name}":{{"thread":{{"comments":{{"nodes":[{{"id":"PRRC_new"}}]}}}},"pullRequestReviewComment":{{"id":"PRRC_new"}}}}}}}}"#
                )
            };
            let mut extra = vec![
                (
                    format!("POST {GH}/pulls/7/reviews ~commit_id"),
                    Reply::json(r#"{"id":77,"node_id":"PRR_77","user":{"login":"sam"}}"#),
                ),
                (
                    "POST /api/graphql ~addPullRequestReviewThread(".into(),
                    Reply::json(&ok("addPullRequestReviewThread")),
                ),
                (
                    "POST /api/graphql ~updatePullRequestReviewComment".into(),
                    Reply::json(&ok("updatePullRequestReviewComment")),
                ),
                (
                    "POST /api/graphql ~deletePullRequestReviewComment".into(),
                    Reply::json(&ok("deletePullRequestReviewComment")),
                ),
                (format!("POST {GH}/pulls/7/reviews/77/events"), Reply::json("{}")),
                (format!("DELETE {GH}/pulls/7/reviews/77"), Reply::json("{}")),
                (
                    format!("GET {GH}/pulls/7/reviews/77/comments"),
                    Reply::json(&format!(
                        r#"[{{"id":1,"node_id":"PRRC_web","path":"mod.txt","body":"From the web","line":3,"side":"RIGHT","commit_id":"{head}"}}]"#
                    )),
                ),
            ];
            extra.push((
                format!("GET {GH}/pulls/7/reviews"),
                Reply::json(if web {
                    r#"[{"id":77,"node_id":"PRR_77","state":"PENDING","user":{"login":"sam"}}]"#
                } else {
                    "[]"
                }),
            ));
            let server = github::server_plus(&main, &head, true, false, extra);
            let mut app = github::boot_github(&root, &server).await;
            open_file(&mut app, "mod.txt").await;
            draw_wide(&mut app, 120);
            (app, server)
        }
        F::GitLab => {
            let (root, main) = make_repo(dir.path());
            let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
                .trim()
                .to_string();
            let notes = if web {
                format!(
                    r#"[{{"id":41,"note":"From the web","discussion_id":null,"resolve_discussion":false,
                    "position":{{"head_sha":"{head}","old_path":"mod.txt","new_path":"mod.txt","position_type":"text","new_line":3,"old_line":null}}}}]"#
                )
            } else {
                "[]".to_string()
            };
            let notes_path = format!("{}/merge_requests/7/draft_notes", gitlab::API);
            let extra = vec![
                (format!("PUT {notes_path}/41"), Reply::json("{}")),
                (format!("DELETE {notes_path}/41"), Reply::status(204)),
                (format!("DELETE {notes_path}/31"), Reply::status(204)),
            ];
            let server = gitlab::server_pending(&main, &head, Reply::status(204), ("bob", "Remark"), &notes, extra);
            let app = gitlab::boot(&root, &server, None).await;
            (app, server)
        }
    };
    app.settings.review_drafts_on_forge = true;
    let mut rig = Rig {
        app,
        server,
        f,
        _dir: dir,
    };
    refresh(&mut rig.app).await;
    rig
}

/// Looks the merge request up again and waits for it.
async fn refresh(app: &mut App) {
    app.test_execute(Command::ReviewRefresh);
    assert!(app.settle(|a| !a.review.loading && !a.review.queued).await);
}

impl Rig {
    /// The forge's pending review is gone (submitted or deleted on the website).
    fn gone(&self) {
        match self.f {
            F::Gitea => self.server.set(&format!("GET {LIST}"), Reply::json(&gitea_list(false))),
            F::GitHub => self.server.set(&format!("GET {GH}/pulls/7/reviews"), Reply::json("[]")),
            F::GitLab => self.server.set(
                &format!("GET {}/merge_requests/7/draft_notes", gitlab::API),
                Reply::json("[]"),
            ),
        }
    }

    /// What the app sent that changes something (GitHub's GraphQL reads are POSTs too).
    fn writes(&self) -> Vec<Request> {
        self.server
            .requests()
            .into_iter()
            .filter(|r| r.method != "GET" && !(r.target == "/api/graphql" && !r.body.contains("mutation")))
            .collect()
    }

    fn label(&self) -> &'static str {
        match self.f {
            F::Gitea => "Gitea",
            F::GitHub => "GitHub",
            F::GitLab => "GitLab",
        }
    }
}

fn pick_first_draft(app: &mut App) {
    let len = app.review.draft_rows().len();
    app.review.view.pending.focused = true;
    app.review.view.pending.select(len, 0);
}

/// Submit Review with `verdict` and `summary`, through the summary box.
fn submit(app: &mut App, verdict: Command, summary: &str) {
    app.test_execute(verdict);
    if !summary.is_empty() {
        app.type_text(summary);
    }
    app.press_bound(Command::ReviewSaveComment);
}

async fn submitted(app: &mut App) {
    assert!(
        app.settle(|a| !a.review.writing && a.review.draft_rows().is_empty())
            .await
    );
    assert!(app.settle(|a| !a.review.loading).await);
}

fn check_website_draft(r: &Rig) {
    let d = drafts(&r.app);
    assert_eq!(d.len(), 1, "{d:?}");
    let d = &d[0];
    assert_eq!(
        (d.body.as_str(), d.path.as_str(), d.start, d.end, d.by_claude, d.right),
        ("From the web", "mod.txt", 3, 3, false, true)
    );
    assert_eq!(d.lines, vec!["m3".to_string()], "its lines are read at its commit");
    assert!(d.forge.is_some() && d.reply_to.is_none());
    assert!(r.app.review.drafts.pending.is_some());
    assert!(r.app.review.draft_rows()[0].held.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_gitea_a_website_draft_appears_is_edited_and_submitted() {
    let mut r = rig(F::Gitea, true).await;
    check_website_draft(&r);
    assert!(
        screen(&mut r.app).contains("PENDING (1, on Gitea)"),
        "{}",
        screen(&mut r.app)
    );
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut r.app, "Edited here");
    quiet(&mut r.app).await;
    let patch = sent(&r.server, "PATCH", "/issues/comments/99");
    assert_eq!(patch.len(), 1);
    assert!(patch[0].body.contains("Edited here"));
    r.gone();
    submit(&mut r.app, Command::ReviewSubmitRequestChanges, "Please fix");
    submitted(&mut r.app).await;
    let w = r.writes();
    let posts: Vec<_> = w.iter().filter(|q| q.method == "POST").collect();
    assert_eq!(posts.len(), 1, "{w:?}");
    assert_eq!(posts[0].target, format!("{PULLS}/reviews/5"));
    let v: serde_json::Value = serde_json::from_str(&posts[0].body).unwrap();
    assert_eq!(v["event"], "REQUEST_CHANGES");
    assert!(v["body"].as_str().unwrap().contains("Please fix"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_github_a_website_draft_appears_is_edited_and_submitted() {
    let mut r = rig(F::GitHub, true).await;
    check_website_draft(&r);
    assert_eq!(drafts(&r.app)[0].forge.as_ref().unwrap().review_id, "77:PRR_77");
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut r.app, "Edited here");
    quiet(&mut r.app).await;
    let edits: Vec<_> = r
        .writes()
        .into_iter()
        .filter(|q| q.body.contains("updatePullRequestReviewComment"))
        .collect();
    assert_eq!(edits.len(), 1);
    assert!(edits[0].body.contains("Edited here") && edits[0].body.contains("PRRC_web"));
    r.gone();
    submit(&mut r.app, Command::ReviewSubmitApprove, "");
    submitted(&mut r.app).await;
    let w = r.writes();
    let last = w.last().unwrap();
    assert_eq!(last.target, format!("{GH}/pulls/7/reviews/77/events"));
    assert!(last.body.contains("APPROVE"));
    assert_eq!(w.iter().filter(|q| q.target.ends_with("/events")).count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_gitlab_a_website_draft_appears_is_edited_deleted_and_submitted() {
    let mut r = rig(F::GitLab, true).await;
    check_website_draft(&r);
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut r.app, "Edited here");
    quiet(&mut r.app).await;
    let put = sent(&r.server, "PUT", "/draft_notes/41");
    assert_eq!(put.len(), 1);
    assert!(put[0].body.contains("Edited here"));
    // Deleted from jed: gone on the forge too.
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut r.app).await;
    assert_eq!(sent(&r.server, "DELETE", "/draft_notes/41").len(), 1);
    assert!(drafts(&r.app).is_empty());
    // Another one made on the website appears at the next refresh and is submitted.
    let mut r = rig(F::GitLab, true).await;
    submit(&mut r.app, Command::ReviewSubmitComment, "Nice");
    submitted(&mut r.app).await;
    let w = r.writes();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].target.ends_with("/draft_notes/bulk_publish") && w[0].body.contains("Nice"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_each_verdict_submits_the_pending_review_in_one_request() {
    let verdicts = [
        (Command::ReviewSubmitComment, "", "COMMENT"),
        (Command::ReviewSubmitComment, "Thanks", "COMMENT"),
        (Command::ReviewSubmitApprove, "", "APPROV"),
        (Command::ReviewSubmitApprove, "LGTM", "APPROV"),
        (Command::ReviewSubmitRequestChanges, "", "REQUEST_CHANGES"),
        (Command::ReviewSubmitRequestChanges, "Fix it", "REQUEST_CHANGES"),
    ];
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        for (cmd, summary, word) in verdicts {
            let mut r = rig(f, true).await;
            r.gone();
            submit(&mut r.app, cmd, summary);
            submitted(&mut r.app).await;
            let w = r.writes();
            let what = format!("{f:?} {cmd:?} {summary:?}: {w:?}");
            match f {
                F::Gitea | F::GitHub => {
                    assert_eq!(w.len(), 1, "{what}");
                    assert_eq!(w[0].method, "POST", "{what}");
                    assert!(w[0].target.contains("/reviews/"), "{what}");
                    assert!(w[0].body.contains(word), "{what}");
                    assert!(summary.is_empty() || w[0].body.contains(summary), "{what}");
                }
                F::GitLab => {
                    // One publish (the verdict and summary with it); Approve also approves.
                    let publish = &w[0];
                    assert!(publish.target.ends_with("/bulk_publish"), "{what}");
                    assert_eq!(w.len(), if word == "APPROV" { 2 } else { 1 }, "{what}");
                    assert!(summary.is_empty() || publish.body.contains(summary), "{what}");
                    assert_eq!(
                        publish.body.contains("requested_changes"),
                        word == "REQUEST_CHANGES",
                        "{what}"
                    );
                    if word == "APPROV" {
                        assert!(w[1].target.ends_with("/approve"), "{what}");
                    }
                }
            }
            assert!(drafts(&r.app).is_empty(), "{what}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_draft_submitted_elsewhere_is_dropped_with_one_notice() {
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        let mut r = rig(f, true).await;
        assert_eq!(drafts(&r.app).len(), 1);
        // A draft that was never on the forge is not touched.
        let key = r.app.review.drafts_key().unwrap();
        let local = crate::review_drafts::Draft::new(
            "mod.txt".into(),
            true,
            2,
            2,
            "c0".into(),
            vec!["m2 changed".into()],
            "mine".into(),
        );
        r.app.review.drafts.store.as_mut().unwrap().add(&key, local);
        r.gone();
        r.app.message = None;
        refresh(&mut r.app).await;
        let d = drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert_eq!(d[0].body, "mine");
        let text = r.app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
        assert_eq!(
            text,
            format!("1 draft was submitted or deleted on {}", r.label()),
            "{f:?}"
        );
        assert!(r.app.review.drafts.pending.is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_body_changed_on_the_forge_wins_and_a_missing_commit_holds_the_draft_back() {
    let mut r = rig(F::Gitea, true).await;
    let comment = threads::rc(
        99,
        "sam",
        "Changed on the web",
        "mod.txt",
        3,
        &"e".repeat(40),
        "+m3",
        false,
    );
    r.server.set(
        &format!("{PULLS}/reviews/5/comments"),
        Reply::json(&format!(
            "[{comment},{}]",
            threads::rc(98, "sam", "Brand new", "mod.txt", 3, &"e".repeat(40), "+m3", false)
        )),
    );
    refresh(&mut r.app).await;
    let d = drafts(&r.app);
    assert_eq!(d.len(), 2);
    let by = |b: &str| d.iter().find(|d| d.body == b).unwrap();
    assert_eq!(by("Changed on the web").forge.as_ref().unwrap().comment_id, "99");
    let new = by("Brand new");
    assert!(new.lines.is_empty(), "its commit isn't in the clone");
    assert!(assert_held(&mut r.app, &new.id).await.contains("fetch"));
}

async fn assert_held(app: &mut App, id: &str) -> String {
    assert!(
        app.settle(|a| a.review.drafts.drift.get(id).is_some_and(|h| h.is_some()))
            .await
    );
    app.review.drafts.drift[id].clone().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_mix_of_on_forge_and_local_only_drafts_goes_in_one_submit() {
    // Gitea: the reply can't join a pending review, so it is posted after the submit.
    let mut r = rig(F::Gitea, true).await;
    let key = r.app.review.drafts_key().unwrap();
    let store = r.app.review.drafts.store.as_mut().unwrap();
    let line = crate::review_drafts::Draft::new(
        "mod.txt".into(),
        true,
        2,
        2,
        "c0".into(),
        vec!["m2 changed".into()],
        "local line".into(),
    );
    store.add(&key, line);
    store.add(
        &key,
        crate::review_drafts::Draft::reply("mod.txt".into(), true, (3, 3), "11".into(), "local reply".into(), false),
    );
    refresh(&mut r.app).await;
    assert!(r.app.settle(|a| a.review.drafts.drift.len() == 3).await);
    r.gone();
    let before = r.writes().len();
    submit(&mut r.app, Command::ReviewSubmitApprove, "");
    submitted(&mut r.app).await;
    let w: Vec<_> = r.writes().into_iter().skip(before).collect();
    let targets: Vec<_> = w.iter().map(|q| (q.method.as_str(), q.target.as_str())).collect();
    assert_eq!(
        targets,
        vec![
            ("POST", format!("{PULLS}/reviews").as_str()),
            ("POST", format!("{PULLS}/reviews/5").as_str()),
            ("POST", format!("{PULLS}/comments/11/replies").as_str()),
        ]
    );
    assert!(w[0].body.contains("PENDING") && w[0].body.contains("local line"));
    assert!(drafts(&r.app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_claude_drafts_reach_the_forge_at_the_users_submit() {
    let mut r = rig(F::GitLab, true).await;
    let key = r.app.review.drafts_key().unwrap();
    let mut claude = crate::review_drafts::Draft::new(
        "mod.txt".into(),
        true,
        2,
        2,
        "c0".into(),
        vec!["m2 changed".into()],
        "claude says".into(),
    );
    claude.by_claude = true;
    r.app.review.drafts.store.as_mut().unwrap().add(&key, claude);
    let mine = crate::review_drafts::Draft::new(
        "mod.txt".into(),
        true,
        1,
        1,
        "c0".into(),
        vec!["m1".into()],
        "mine".into(),
    );
    r.app.review.drafts.store.as_mut().unwrap().add(&key, mine);
    refresh(&mut r.app).await;
    assert!(r.app.settle(|a| a.review.drafts.drift.len() == 3).await);
    assert!(r.writes().is_empty(), "nothing is pushed by itself");
    r.gone();
    submit(&mut r.app, Command::ReviewSubmitComment, "");
    submitted(&mut r.app).await;
    let w = r.writes();
    let adds: Vec<_> = w
        .iter()
        .filter(|q| q.method == "POST" && q.target.ends_with("/draft_notes"))
        .collect();
    assert_eq!(adds.len(), 2, "{w:?}");
    assert!(adds.iter().any(|q| q.body.contains("claude says")));
    assert!(w.last().unwrap().target.ends_with("/bulk_publish"));
    assert_eq!(w.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_forge_that_cant_hold_a_draft_gets_it_the_old_way() {
    // Nothing pending on Gitea and the pending review refuses (500): the draft stays local only
    // and Submit Review posts it as before, in one review.
    let dir = tempfile::tempdir().unwrap();
    let refuse = vec![(
        format!("POST {PULLS}/reviews ~PENDING"),
        Reply::status(500).with_body(r#"{"message":"no"}"#),
    )];
    let (mut app, server) = gitea(dir.path(), Reply::json("{}"), refuse).await;
    comment(&mut app, 1, 2, "Old way");
    quiet(&mut app).await;
    assert_eq!(drafts(&app)[0].forge, None);
    submit(&mut app, Command::ReviewSubmitApprove, "");
    submitted(&mut app).await;
    let posts = sent(&server, "POST", "/pulls/7/reviews");
    let last: serde_json::Value = serde_json::from_str(&posts.last().unwrap().body).unwrap();
    assert_eq!(last["event"], "APPROVED");
    assert!(last["comments"][0]["body"].as_str().unwrap().starts_with("Old way"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_discard_review_deletes_the_pending_review_and_asks_first() {
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        let mut r = rig(f, true).await;
        let key = r.app.review.drafts_key().unwrap();
        let local = crate::review_drafts::Draft::new(
            "mod.txt".into(),
            true,
            2,
            2,
            "c0".into(),
            vec!["m2 changed".into()],
            "local only".into(),
        );
        r.app.review.drafts.store.as_mut().unwrap().add(&key, local);
        // Declined: nothing is sent.
        r.app.test_execute(Command::ReviewDiscardPending);
        assert!(matches!(r.app.prompt, Some(Prompt::ConfirmDiscardReview { .. })));
        r.app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(r.app.prompt.is_none() && r.writes().is_empty());
        // Discard keeps the local only draft.
        r.app.test_execute(Command::ReviewDiscardPending);
        r.app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(r.app.settle(|a| !a.review.writing).await);
        // The forge no longer has it, so the lookup that follows finds nothing there.
        r.gone();
        refresh(&mut r.app).await;
        assert!(r.app.settle(|a| a.review.draft_rows().len() == 1).await);
        assert_eq!(drafts(&r.app)[0].body, "local only", "{f:?}");
        let w = r.writes();
        assert!(w.iter().all(|q| q.method == "DELETE"), "{f:?} {w:?}");
        assert!(!w.is_empty(), "{f:?}");
        assert!(r.app.review.drafts.pending.is_none());
    }
    // "and local drafts" deletes those too.
    let mut r = rig(F::Gitea, true).await;
    let key = r.app.review.drafts_key().unwrap();
    let local = crate::review_drafts::Draft::new(
        "mod.txt".into(),
        true,
        2,
        2,
        "c0".into(),
        vec!["m2 changed".into()],
        "local only".into(),
    );
    r.app.review.drafts.store.as_mut().unwrap().add(&key, local);
    r.app.test_execute(Command::ReviewDiscardPending);
    r.app.press(KeyCode::Char('l'), KeyModifiers::NONE);
    assert!(r.app.settle(|a| !a.review.writing).await);
    r.gone();
    refresh(&mut r.app).await;
    assert!(r.app.settle(|a| a.review.draft_rows().is_empty()).await);
    assert!(drafts(&r.app).is_empty());
    assert_eq!(sent(&r.server, "DELETE", "/pulls/7/reviews/5").len(), 1);
    // Nothing pending: it says so and asks nothing.
    let mut r = rig(F::Gitea, false).await;
    r.app.test_execute(Command::ReviewDiscardPending);
    assert!(r.app.prompt.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_open_on_forge_from_pending_opens_the_files_page() {
    let mut r = rig(F::Gitea, true).await;
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewOpenOnForge);
    let url = "https://git.example.com/alice/project/pulls/7/files";
    assert!(r.app.settle(|_| crate::url::opened(url)).await, "{url} opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_with_the_setting_off_the_lookup_reads_no_pending_review() {
    let mut r = rig(F::Gitea, true).await;
    r.app.settings.review_drafts_on_forge = false;
    let (key, id) = (r.app.review.drafts_key().unwrap(), drafts(&r.app)[0].id.clone());
    r.app.review.drafts.store.as_mut().unwrap().remove(&key, &id);
    let before = r.server.requests().len();
    refresh(&mut r.app).await;
    let reads: Vec<_> = r.server.requests().into_iter().skip(before).collect();
    assert!(reads.iter().all(|q| !q.target.contains("/reviews/5")), "{reads:?}");
    assert!(drafts(&r.app).is_empty(), "the website's draft isn't added");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_done_when_a_saved_comment_is_listed_as_pending_and_not_doubled() {
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        let mut r = rig(f, false).await;
        comment(&mut r.app, 1, 2, "First");
        quiet(&mut r.app).await;
        let id = drafts(&r.app)[0].forge.clone().expect("the forge took it");
        // The forge now lists it in the user's pending review: the lookup keeps the one draft.
        match f {
            F::Gitea => r.server.set(&format!("GET {LIST}"), Reply::json(&gitea_list(true))),
            F::GitHub => {
                r.server.set(
                    &format!("GET {GH}/pulls/7/reviews"),
                    Reply::json(r#"[{"id":77,"node_id":"PRR_77","state":"PENDING","user":{"login":"sam"}}]"#),
                );
                r.server.set(
                    &format!("GET {GH}/pulls/7/reviews/77/comments"),
                    Reply::json(r#"[{"id":1,"node_id":"PRRC_new","path":"mod.txt","body":"First","line":3,"side":"RIGHT","commit_id":"h"}]"#),
                );
            }
            F::GitLab => r.server.set(
                &format!("GET {}/merge_requests/7/draft_notes", gitlab::API),
                Reply::json(r#"[{"id":31,"note":"First","discussion_id":null,"position":{"head_sha":"h","new_path":"mod.txt","position_type":"text","new_line":3}}]"#),
            ),
        }
        refresh(&mut r.app).await;
        let d = drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}: {d:?}");
        assert_eq!(d[0].forge.as_ref().unwrap().comment_id, id.comment_id, "{f:?}");
        assert!(r.app.review.drafts.pending.is_some(), "{f:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_website_draft_is_deleted_from_jed_on_gitea_and_github() {
    for f in [F::Gitea, F::GitHub] {
        let mut r = rig(f, true).await;
        check_website_draft(&r);
        pick_first_draft(&mut r.app);
        r.app.test_execute(Command::ReviewDeleteDraft);
        quiet(&mut r.app).await;
        assert!(drafts(&r.app).is_empty(), "{f:?}");
        let w = r.writes();
        assert_eq!(w.len(), 1, "{f:?}: {w:?}");
        match f {
            F::Gitea => {
                assert_eq!(w[0].method, "DELETE", "{w:?}");
                assert!(w[0].target.ends_with("/issues/comments/99"), "{w:?}");
            }
            _ => assert!(
                w[0].body.contains("deletePullRequestReviewComment") && w[0].body.contains("PRRC_web"),
                "{w:?}"
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_lookup_that_overlaps_a_write_does_not_drop_the_draft() {
    let mut r = rig(F::Gitea, true).await;
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    rewrite(&mut r.app, "Edited here");
    // The forge's answer to the lookup below no longer lists the draft (a stale read as far as
    // the edit on its way is concerned): that lookup must not drop it.
    r.gone();
    r.app.test_execute(Command::ReviewRefresh);
    quiet(&mut r.app).await;
    assert!(r.app.settle(|a| !a.review.loading && !a.review.queued).await);
    assert_eq!(drafts(&r.app).len(), 1, "kept: a write finished or was on its way");
    assert_eq!(drafts(&r.app)[0].body, "Edited here");
    // Nothing overlapping this time: the lookup drops it, once.
    r.app.message = None;
    refresh(&mut r.app).await;
    assert!(drafts(&r.app).is_empty());
    assert_eq!(
        r.app.message.as_ref().map(|m| m.text.as_str()),
        Some("1 draft was submitted or deleted on Gitea")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_failed_pending_read_drops_nothing() {
    let mut r = rig(F::GitHub, true).await;
    r.server.set(&format!("GET {GH}/pulls/7/reviews"), Reply::status(500));
    refresh(&mut r.app).await;
    assert_eq!(drafts(&r.app).len(), 1);
    assert!(r.app.review.review().unwrap().pending_error.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_left_side_range_from_the_website_reads_the_old_file() {
    let mut r = rig(F::GitHub, true).await;
    let (key, old) = (r.app.review.drafts_key().unwrap(), drafts(&r.app)[0].clone());
    r.app.review.drafts.store.as_mut().unwrap().remove(&key, &old.id);
    r.server.set(
        &format!("GET {GH}/pulls/7/reviews/77/comments"),
        Reply::json(&format!(
            r#"[{{"id":1,"node_id":"PRRC_left","path":"mod.txt","body":"Old lines","start_line":1,"line":2,"side":"LEFT","commit_id":"{}"}}]"#,
            old.commit
        )),
    );
    refresh(&mut r.app).await;
    let d = drafts(&r.app);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(
        (d[0].right, d[0].start, d[0].end, d[0].lines.clone()),
        (false, 1, 2, vec!["m1".to_string(), "m2".to_string()]),
        "numbered in the merge base's file, not the head's `m2 changed`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_failed_discard_keeps_every_draft() {
    let mut r = rig(F::Gitea, true).await;
    r.server.set(&format!("DELETE {PULLS}/reviews/5"), Reply::status(500));
    r.app.test_execute(Command::ReviewDiscardPending);
    r.app.press(KeyCode::Char('l'), KeyModifiers::NONE);
    assert!(r.app.settle(|a| !a.review.writing).await);
    assert_eq!(drafts(&r.app).len(), 1, "the forge still has it");
    assert!(r.app.review.drafts.pending.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_forge_drafts_a_draft_leaves_the_store_only_when_the_submit_confirms() {
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        let publish = match f {
            F::Gitea => format!("POST {PULLS}/reviews/5"),
            F::GitHub => format!("POST {GH}/pulls/7/reviews/77/events"),
            F::GitLab => format!("POST {}/merge_requests/7/draft_notes/bulk_publish", gitlab::API),
        };
        // The forge refuses: every draft stays, still on the forge, and the reason is said.
        let mut r = rig(f, true).await;
        r.server
            .set(&publish, Reply::status(500).with_body(r#"{"message":"boom"}"#));
        r.app.message = None;
        submit(&mut r.app, Command::ReviewSubmitComment, "");
        assert!(r.app.settle(|a| !a.review.writing).await);
        let d = drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}: {d:?}");
        assert!(d[0].forge.is_some(), "{f:?}");
        assert!(r.app.message.is_some(), "{f:?} says why");
        // The forge takes it: the draft is gone as soon as the submit is confirmed (checked before
        // any lookup can drop it).
        let mut r = rig(f, true).await;
        submit(&mut r.app, Command::ReviewSubmitComment, "");
        assert!(r.app.settle(|a| !a.review.writing).await);
        assert!(drafts(&r.app).is_empty(), "{f:?}");
    }
}
