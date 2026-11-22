//! Replies in the pending review, subtask a (replies as drafts, Comment Now for replies), driven
//! through the app against the fake Gitea, GitHub and GitLab servers: the reply box's main button
//! saves the reply as a draft (`[Add to Review]` / `[Start Review]` / `[Save Draft]` with the
//! header saying so) and `[Comment Now]` posts it at once without touching the pending review;
//! a saved reply is a pending reply on GitHub and GitLab, local only on Gitea (posted after the
//! submit), listed in PENDING, edited and deleted there, one per thread (a second Reply opens it,
//! Claude's too, and saving makes it the user's); an edit, a conversation comment and the review
//! summary still post at once and show no Comment Now; with the setting off nothing reaches the
//! forge before Submit Review.
//!
//! Subtask b (`review_pending_replies_b_*`): `[Comment Now]` on a new line comment posts one
//! comment (GitLab: a discussion; GitHub and Gitea without a pending review) and leaves the
//! pending review alone; with one, GitHub and Gitea show no button and refuse the command; a
//! refusal keeps the box; GitHub's lines outside the diff refuse before any request; an edited
//! line draft is dropped after the post.

use crate::app::App;
use crate::command::Command;
use crate::forge::fake::Reply;
use crate::mr_review_tests::write::select;
use crate::review_drafts::Draft;
use crate::review_forge_drafts_tests::*;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn screen_lines(app: &mut App) -> Vec<String> {
    screen_at(app, 140)
}

fn screen_at(app: &mut App, width: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(width, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..40)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect()
}

fn text(app: &mut App) -> String {
    screen_lines(app).join("\n")
}

/// Clicks the button whose label starts with `label` in the open box.
fn click(app: &mut App, label: &str) {
    let lines = screen_lines(app);
    let (row, line) = lines
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains(label))
        .unwrap_or_else(|| panic!("no {label} on screen:\n{}", lines.join("\n")));
    let col = line[..line.find(label).unwrap()].chars().count() as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), col + 1, row as u16);
}

/// The caret on the thread that every fake has at right line 3 (GitLab's `aa11`, GitHub's
/// `PRRT_1`, Gitea's 11).
fn on_thread(app: &mut App) {
    select(app, true, 2, 2);
}

fn thread_id(f: F) -> &'static str {
    match f {
        F::Gitea => "11",
        F::GitHub => "PRRT_1",
        F::GitLab => "7:aa11",
    }
}

fn reply_routes(r: &Rig) {
    if r.f == F::GitHub {
        r.server.set(
            "POST /api/graphql ~addPullRequestReviewThreadReply",
            Reply::json(r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"PRRC_reply"}}}}"#),
        );
    }
}

/// Opens the reply box on the thread, types `body` and clicks the main button.
async fn reply_saved(r: &mut Rig, body: &str) {
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    assert!(r.app.test_composer_text().is_some(), "{:?}", r.f);
    r.app.type_text(body);
    click(&mut r.app, "Ctrl+Enter]");
    assert!(r.app.test_composer_text().is_none(), "{:?}", r.f);
    quiet(&mut r.app).await;
}

fn reply_drafts(app: &App) -> Vec<Draft> {
    drafts(app).into_iter().filter(|d| d.reply_to.is_some()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_the_box_labels_follow_the_setting_and_the_pending_review() {
    // No pending review yet: Start Review.
    let mut r = rig(F::Gitea, false).await;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    let t = text(&mut r.app);
    assert!(t.contains("Reply (starts your review) on line 3"), "{t}");
    assert!(
        t.contains("[Start Review Ctrl+Enter] [Comment Now] [Cancel Esc] [Insert Suggestion]"),
        "{t}"
    );
    r.app.test_execute(Command::ReviewCancelComment);
    // A pending review exists (made on the website): Add to Review.
    let mut r = rig(F::Gitea, true).await;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    let t = text(&mut r.app);
    assert!(t.contains("Reply (added to your review) on line 3"), "{t}");
    assert!(
        t.contains("[Add to Review Ctrl+Enter] [Comment Now] [Cancel Esc]"),
        "{t}"
    );
    r.app.test_execute(Command::ReviewCancelComment);
    // The setting off: a local draft.
    r.app.settings.review_drafts_on_forge = false;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    let t = text(&mut r.app);
    assert!(t.contains("Reply (saved as a draft) on line 3"), "{t}");
    assert!(t.contains("[Save Draft Ctrl+Enter] [Comment Now] [Cancel Esc]"), "{t}");
    r.app.test_execute(Command::ReviewCancelComment);
    // A new line comment has Comment Now unless Gitea has a pending review (it would publish
    // it: see `review_pending_replies_b_*`); an edit posts at once and has none.
    select(&mut r.app, true, 1, 1);
    r.app.test_execute(Command::ReviewComment);
    let t = text(&mut r.app);
    assert!(t.contains("[Save Draft Ctrl+Enter] [Cancel Esc]"), "{t}");
    assert!(!t.contains("[Comment Now]"), "{t}");
    r.app.test_execute(Command::ReviewCancelComment);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_an_edit_still_posts_at_once_with_no_comment_now() {
    let mut r = rig(F::Gitea, true).await;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewEditComment);
    assert!(
        r.app.test_composer_text().is_some(),
        "sam's own comment opens for editing"
    );
    let t = text(&mut r.app);
    assert!(t.contains("Edit your comment (saved at once)"), "{t}");
    assert!(t.contains("[Comment Ctrl+Enter] [Cancel Esc]"), "{t}");
    assert!(!t.contains("[Comment Now]"), "{t}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_a_saved_reply_is_a_pending_reply_on_github_and_gitlab() {
    for f in [F::GitHub, F::GitLab] {
        let mut r = rig(f, false).await;
        reply_routes(&r);
        reply_saved(&mut r, "Fixed it").await;
        let d = reply_drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert_eq!(d[0].reply_to.as_deref(), Some(thread_id(f)));
        assert!(!d[0].by_claude && !d[0].resolve, "{f:?}");
        assert!(
            d[0].forge.is_some(),
            "{f:?}: it is in the pending review: {:?}",
            r.app.review.drafts.local_why
        );
        let w = r.writes();
        match f {
            F::GitHub => assert!(
                w.iter()
                    .any(|q| q.body.contains("addPullRequestReviewThreadReply") && q.body.contains("Fixed it")),
                "{w:?}"
            ),
            _ => assert!(
                w.iter()
                    .any(|q| q.target.ends_with("/draft_notes") && q.body.contains("in_reply_to_discussion_id")),
                "{w:?}"
            ),
        }
        // Listed in PENDING.
        let t = text(&mut r.app);
        assert!(t.contains("PENDING"), "{f:?}\n{t}");
        assert!(t.contains("Fixed it"), "{f:?}\n{t}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_on_gitea_a_reply_is_local_only_and_posted_after_the_submit() {
    let mut r = rig(F::Gitea, true).await;
    reply_saved(&mut r, "Fixed it").await;
    let d = reply_drafts(&r.app);
    assert_eq!(d.len(), 1);
    assert!(d[0].forge.is_none());
    let why = r.app.review.drafts.local_why.get(&d[0].id).cloned().unwrap_or_default();
    assert!(why.contains("reply"), "{why}");
    assert!(sent(&r.server, "POST", "/comments/11/replies").is_empty());
    r.gone();
    submit(&mut r.app, Command::ReviewSubmitComment, "");
    submitted(&mut r.app).await;
    let replies = sent(&r.server, "POST", "/comments/11/replies");
    assert_eq!(replies.len(), 1);
    assert!(replies[0].body.contains("Fixed it"));
    assert!(drafts(&r.app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_a_reply_draft_is_edited_deleted_and_one_per_thread() {
    let mut r = rig(F::GitHub, false).await;
    reply_routes(&r);
    reply_saved(&mut r, "First").await;
    let id = reply_drafts(&r.app)[0].id.clone();
    // A second Reply opens the draft, not an empty box.
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("First"));
    r.app.type_text(" and more");
    click(&mut r.app, "Ctrl+Enter]");
    quiet(&mut r.app).await;
    let d = reply_drafts(&r.app);
    assert_eq!(
        (d.len(), d[0].id.as_str(), d[0].body.as_str()),
        (1, id.as_str(), "First and more")
    );
    assert!(
        r.writes()
            .iter()
            .any(|q| q.body.contains("updatePullRequestReviewComment") && q.body.contains("First and more"))
    );
    // PENDING's Edit Draft opens the same box; Delete Draft removes it on the forge.
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("First and more"));
    let t = text(&mut r.app);
    assert!(t.contains("[Comment Now]"), "{t}");
    r.app.test_execute(Command::ReviewCancelComment);
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewDeleteDraft);
    quiet(&mut r.app).await;
    assert!(drafts(&r.app).is_empty());
    assert!(
        r.writes()
            .iter()
            .any(|q| q.body.contains("deletePullRequestReviewComment"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_replying_to_a_claude_draft_opens_it_and_saving_makes_it_the_users() {
    let mut r = rig(F::GitHub, false).await;
    reply_routes(&r);
    let key = r.app.review.drafts_key().unwrap();
    let mut claude = Draft::reply(
        "mod.txt".into(),
        true,
        (2, 3),
        "PRRT_1".into(),
        "Claude's idea".into(),
        false,
    );
    claude.by_claude = true;
    r.app.review.drafts.store.as_mut().unwrap().add(&key, claude);
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Claude's idea"));
    assert!(r.writes().is_empty(), "Claude's draft stays local");
    r.app.type_text("!");
    click(&mut r.app, "Ctrl+Enter]");
    quiet(&mut r.app).await;
    let d = reply_drafts(&r.app);
    assert_eq!(d.len(), 1);
    assert!(!d[0].by_claude);
    assert_eq!(d[0].body, "Claude's idea!");
    assert!(d[0].forge.is_some(), "{:?}", r.app.review.drafts.local_why);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_comment_now_posts_the_reply_and_leaves_the_pending_review_alone() {
    let mut r = rig(F::Gitea, true).await;
    let before = drafts(&r.app).len();
    let writes = r.writes().len();
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    r.app.type_text("Right now");
    click(&mut r.app, "[Comment Now]");
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    let replies = sent(&r.server, "POST", "/comments/11/replies");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].body, r#"{"body":"Right now"}"#);
    assert_eq!(drafts(&r.app).len(), before, "no draft was made");
    let new: Vec<_> = r.writes().into_iter().skip(writes).collect();
    assert_eq!(new.len(), 1, "{new:?}");
    assert!(
        sent(&r.server, "POST", "/reviews/5").is_empty(),
        "the review wasn't published"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_comment_now_command_is_for_a_reply_box_only() {
    let mut r = rig(F::Gitea, true).await;
    r.app.test_execute(Command::ReviewCommentNow);
    select(&mut r.app, true, 1, 1);
    r.app.test_execute(Command::ReviewComment);
    r.app.type_text("a line comment");
    r.app.test_execute(Command::ReviewCommentNow);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("a line comment"));
    assert!(r.writes().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_with_the_setting_off_nothing_reaches_the_forge_before_submit() {
    for f in [F::Gitea, F::GitHub, F::GitLab] {
        let mut r = rig(f, false).await;
        r.app.settings.review_drafts_on_forge = false;
        on_thread(&mut r.app);
        r.app.test_execute(Command::ReviewReply);
        r.app.type_text("Later");
        click(&mut r.app, "Ctrl+Enter]");
        quiet(&mut r.app).await;
        let d = reply_drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert!(d[0].forge.is_none() && !d[0].by_claude, "{f:?}");
        assert!(r.writes().is_empty(), "{f:?}: {:?}", r.writes());
    }
    // Submit Review posts the reply (Gitea: its own endpoint).
    let mut r = rig(F::Gitea, false).await;
    r.app.settings.review_drafts_on_forge = false;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    r.app.type_text("Later");
    click(&mut r.app, "Ctrl+Enter]");
    submit(&mut r.app, Command::ReviewSubmitComment, "");
    submitted(&mut r.app).await;
    let replies = sent(&r.server, "POST", "/comments/11/replies");
    assert_eq!(replies.len(), 1);
    assert!(replies[0].body.contains("Later"));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_a_narrow_box_drops_insert_suggestion_and_keeps_comment_now_clickable() {
    let mut r = rig(F::Gitea, true).await;
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    r.app.type_text("Narrow");
    // Find a terminal width where the row has lost Insert Suggestion but kept the two buttons.
    let width = (60..140u16)
        .find(|w| {
            let t = screen_at(&mut r.app, *w).join("\n");
            t.contains("[Comment Now]") && !t.contains("[Insert Suggestion]")
        })
        .expect("some width drops only Insert Suggestion");
    let lines = screen_at(&mut r.app, width);
    let (row, line) = lines
        .iter()
        .enumerate()
        .find(|(_, l)| l.contains("[Comment Now]"))
        .unwrap();
    assert!(line.contains("[Add to Review Ctrl+Enter]"), "{line}");
    let col = line[..line.find("[Comment Now]").unwrap()].chars().count() as u16;
    r.app
        .mouse(MouseEventKind::Down(MouseButton::Left), col + 1, row as u16);
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    assert_eq!(sent(&r.server, "POST", "/comments/11/replies").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_saving_a_reply_keeps_the_drafts_resolve_choice() {
    let mut r = rig(F::Gitea, false).await;
    let key = r.app.review.drafts_key().unwrap();
    let draft = Draft::reply("mod.txt".into(), true, (2, 3), "11".into(), "Old".into(), true);
    r.app.review.drafts.store.as_mut().unwrap().add(&key, draft);
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Old"));
    r.app.type_text(" new");
    click(&mut r.app, "Ctrl+Enter]");
    quiet(&mut r.app).await;
    let d = reply_drafts(&r.app);
    assert_eq!((d.len(), d[0].body.as_str(), d[0].resolve), (1, "Old new", true));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_comment_now_on_a_github_reply_uses_the_reply_route_and_no_review_write() {
    let mut r = rig(F::GitHub, false).await;
    reply_routes(&r);
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    r.app.type_text("Now on GitHub");
    click(&mut r.app, "[Comment Now]");
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    let w = r.writes();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].body.contains("addPullRequestReviewThreadReply") && w[0].body.contains("Now on GitHub"),
        "{w:?}"
    );
    assert!(reply_drafts(&r.app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_comment_now_from_an_on_forge_reply_draft_deletes_it_after_the_post() {
    let mut r = rig(F::GitHub, false).await;
    reply_routes(&r);
    reply_saved(&mut r, "Draft text").await;
    assert!(reply_drafts(&r.app)[0].forge.is_some());
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Draft text"));
    click(&mut r.app, "[Comment Now]");
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    quiet(&mut r.app).await;
    let w = r.writes();
    assert!(
        w.iter().any(|q| q.body.contains("addPullRequestReviewThreadReply")),
        "{w:?}"
    );
    assert!(
        w.iter().any(|q| q.body.contains("deletePullRequestReviewComment")),
        "{w:?}"
    );
    assert!(drafts(&r.app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_comment_now_from_a_local_reply_draft_does_not_post_it_again_at_submit() {
    let mut r = rig(F::Gitea, true).await;
    reply_saved(&mut r, "Once").await;
    assert_eq!(reply_drafts(&r.app).len(), 1);
    on_thread(&mut r.app);
    r.app.test_execute(Command::ReviewReply);
    click(&mut r.app, "[Comment Now]");
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    assert!(reply_drafts(&r.app).is_empty());
    assert_eq!(sent(&r.server, "POST", "/comments/11/replies").len(), 1);
    r.gone();
    submit(&mut r.app, Command::ReviewSubmitComment, "");
    submitted(&mut r.app).await;
    assert_eq!(
        sent(&r.server, "POST", "/comments/11/replies").len(),
        1,
        "not posted twice"
    );
}

// Subtask b: Comment Now for a new line comment.

/// Opens a line comment box on `mod.txt` line 1 and types `body`.
fn line_box(app: &mut App, body: &str) {
    select(app, true, 1, 1);
    app.test_execute(Command::ReviewComment);
    assert!(app.test_composer_text().is_some());
    app.type_text(body);
}

async fn posted(r: &mut Rig) {
    assert!(
        r.app
            .settle(|a| a.test_composer_text().is_none() && !a.review.writing)
            .await
    );
    quiet(&mut r.app).await;
}

fn discussions_route(r: &Rig) {
    match r.f {
        F::GitLab => r.server.set(
            &format!(
                "POST {}/merge_requests/7/discussions",
                crate::mr_review_tests::gitlab::API
            ),
            Reply::json(r#"{"id":"abc"}"#),
        ),
        F::GitHub => r
            .server
            .set(&format!("POST {GH}/pulls/7/comments"), Reply::json(r#"{"id":5}"#)),
        F::Gitea => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_gitlab_posts_a_discussion_beside_its_pending_review() {
    let mut r = rig(F::GitLab, true).await;
    discussions_route(&r);
    let before = drafts(&r.app).len();
    let writes = r.writes().len();
    line_box(&mut r.app, "Right now");
    let t = text(&mut r.app);
    assert!(t.contains("[Comment Now]"), "{t}");
    click(&mut r.app, "[Comment Now]");
    posted(&mut r).await;
    let d = sent(&r.server, "POST", "/merge_requests/7/discussions");
    assert_eq!(d.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&d[0].body).unwrap();
    assert_eq!(v["body"], "Right now");
    assert_eq!(v["position"]["new_path"], "mod.txt");
    assert!(v["position"]["new_line"].is_u64(), "{v}");
    assert!(
        sent(&r.server, "POST", "/draft_notes").is_empty(),
        "no draft note added"
    );
    assert!(sent(&r.server, "POST", "/bulk_publish").is_empty(), "nothing published");
    assert_eq!(drafts(&r.app).len(), before, "no draft was made");
    let new: Vec<_> = r.writes().into_iter().skip(writes).collect();
    assert_eq!(new.len(), 1, "{new:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_gitea_and_github_post_at_once_without_a_pending_review() {
    let mut r = rig(F::Gitea, false).await;
    let before = r.writes().len();
    line_box(&mut r.app, "Now please");
    click(&mut r.app, "[Comment Now]");
    posted(&mut r).await;
    let reviews = sent(&r.server, "POST", "/pulls/7/reviews");
    assert_eq!(reviews.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&reviews[0].body).unwrap();
    assert_eq!(v["event"], "COMMENT");
    assert_eq!(v["comments"][0]["body"], "Now please");
    assert!(v["comments"][0]["new_position"].as_u64().unwrap() > 0, "{v}");
    assert_eq!(r.writes().len(), before + 1);
    assert!(drafts(&r.app).is_empty());

    let mut r = rig(F::GitHub, false).await;
    discussions_route(&r);
    line_box(&mut r.app, "Now please");
    click(&mut r.app, "[Comment Now]");
    posted(&mut r).await;
    let c = sent(&r.server, "POST", "/pulls/7/comments");
    assert_eq!(c.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&c[0].body).unwrap();
    assert_eq!(v["path"], "mod.txt");
    assert_eq!(v["side"], "RIGHT");
    assert_eq!(v["body"], "Now please");
    assert!(sent(&r.server, "POST", "/reviews").is_empty());
    assert!(
        r.writes().iter().all(|q| !q.body.contains("mutation")),
        "no review write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_no_button_and_a_refusal_where_a_pending_review_blocks_it() {
    for f in [F::Gitea, F::GitHub] {
        let mut r = rig(f, true).await;
        discussions_route(&r);
        let before = r.writes().len();
        line_box(&mut r.app, "Not now");
        let t = text(&mut r.app);
        assert!(!t.contains("[Comment Now]"), "{f:?}: {t}");
        r.app.test_execute(Command::ReviewCommentNow);
        assert_eq!(r.app.test_composer_text().as_deref(), Some("Not now"));
        let m = r.app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
        assert!(m.contains("pending review"), "{f:?}: {m}");
        assert_eq!(r.writes().len(), before, "{f:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_a_refusal_keeps_the_box_and_shows_the_error() {
    let mut r = rig(F::GitLab, false).await;
    r.server.set(
        &format!(
            "POST {}/merge_requests/7/discussions",
            crate::mr_review_tests::gitlab::API
        ),
        Reply::status(400).with_body(r#"{"message":"line_code can't be blank"}"#),
    );
    line_box(&mut r.app, "Keep me");
    click(&mut r.app, "[Comment Now]");
    assert!(r.app.settle(|a| !a.review.writing && a.review.error.is_some()).await);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Keep me"));
    let e = r.app.review.error.clone().unwrap_or_default();
    assert!(e.contains("line_code can't be blank"), "{e}");
    assert!(drafts(&r.app).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_github_refuses_a_line_outside_the_diff_before_any_request() {
    use crate::mr_review_tests::{draw_wide, github, make_repo, open_file};
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let server = github::server(&main, &head);
    let mut app = github::boot_github(&root, &server).await;
    open_file(&mut app, "renamed.txt").await;
    draw_wide(&mut app, 120);
    app.group_mut().active_diff_mut().unwrap().go_to_side_line(true, 1);
    draw_wide(&mut app, 120);
    app.group_mut().active_diff_mut().unwrap().caret = crate::diff_caret::Caret {
        right: true,
        anchor: (0, 0),
        head: (0, 1),
        col: None,
    };
    app.test_execute(Command::ReviewComment);
    app.type_text("outside");
    let before = server.requests().len();
    app.test_execute(Command::ReviewCommentNow);
    assert_eq!(app.test_composer_text().as_deref(), Some("outside"));
    let m = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(m.contains("not in the merge request's diff"), "{m}");
    assert_eq!(server.requests().len(), before, "no request");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_comment_now_from_an_edited_line_draft_drops_it_after_the_post() {
    let mut r = rig(F::GitLab, false).await;
    discussions_route(&r);
    r.app.settings.review_drafts_on_forge = false;
    line_box(&mut r.app, "Draft first");
    click(&mut r.app, "Ctrl+Enter]");
    quiet(&mut r.app).await;
    assert_eq!(drafts(&r.app).len(), 1);
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Draft first"));
    click(&mut r.app, "[Comment Now]");
    posted(&mut r).await;
    assert_eq!(sent(&r.server, "POST", "/merge_requests/7/discussions").len(), 1);
    assert!(drafts(&r.app).is_empty(), "the draft would post it again at Submit");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_comment_now_is_refused_when_local_is_not_at_the_head() {
    let mut r = rig(F::GitLab, false).await;
    discussions_route(&r);
    line_box(&mut r.app, "Too early");
    r.app.git.status.as_mut().expect("git status").head_id = Some("0000000".into());
    let before = r.writes().len();
    r.app.test_execute(Command::ReviewCommentNow);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Too early"));
    let m = r.app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(m.contains("Can't post") && m.contains("local"), "{m}");
    assert_eq!(r.writes().len(), before, "no forge write");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_b_comment_now_is_refused_for_a_held_back_draft() {
    let mut r = rig(F::GitLab, false).await;
    discussions_route(&r);
    r.app.settings.review_drafts_on_forge = false;
    line_box(&mut r.app, "Held one");
    click(&mut r.app, "Ctrl+Enter]");
    quiet(&mut r.app).await;
    pick_first_draft(&mut r.app);
    r.app.test_execute(Command::ReviewEditDraft);
    let id = r.app.review.draft_rows()[0].draft.id.clone();
    r.app.review.drafts.live.insert(id, Some("the lines changed".into()));
    let before = r.writes().len();
    r.app.test_execute(Command::ReviewCommentNow);
    assert_eq!(r.app.test_composer_text().as_deref(), Some("Held one"));
    let m = r.app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(m.contains("Can't post it") && m.contains("the lines changed"), "{m}");
    assert_eq!(r.writes().len(), before, "no forge write");
    assert_eq!(drafts(&r.app).len(), 1, "the draft is kept");
}

// Subtask c (`review_pending_replies_c_*`): Claude's drafts (line comments and replies) go to
// the pending review when made, are listed there tagged `claude`, and can be replaced and deleted
// by Claude; nothing Claude does submits, discards or posts.

use crate::forge::fake::Request;
use crate::ipc::Query;

/// Asks `q` through the Claude IPC path; the answer.
async fn ask(app: &mut App, q: Query) -> crate::ipc::Reply {
    let rx = std::cell::RefCell::new(app.test_claude_ask(q));
    let out = std::cell::RefCell::new(None);
    assert!(
        app.settle(|_| {
            if let Ok(r) = rx.borrow_mut().try_recv() {
                *out.borrow_mut() = Some(r);
            }
            out.borrow().is_some()
        })
        .await,
        "no answer"
    );
    out.into_inner().unwrap()
}

fn comment_q(body: &str, start: u32, end: u32, draft: Option<&str>) -> Query {
    Query::ReviewComment {
        path: Some("mod.txt".into()),
        right: Some(true),
        start: Some(start),
        end: Some(end),
        body: body.into(),
        draft: draft.map(String::from),
    }
}

/// A Claude line draft: its id.
async fn claude_comment(r: &mut Rig, body: &str, start: u32, end: u32) -> String {
    let a = ask(&mut r.app, comment_q(body, start, end, None)).await;
    assert_eq!(a.error, None, "{:?}", r.f);
    quiet(&mut r.app).await;
    a.data.unwrap()["draft_id"].as_str().unwrap().to_string()
}

fn claude_drafts(app: &App) -> Vec<Draft> {
    drafts(app).into_iter().filter(|d| d.by_claude).collect()
}

/// Whether `w` is a request that only touches the pending review's drafts.
fn draft_write(f: F, w: &Request) -> bool {
    match f {
        F::GitHub => {
            let review = w.target.ends_with("/pulls/7/reviews") && w.method == "POST" && !w.body.contains("\"event\"");
            let gql = w.target == "/api/graphql"
                && (w.body.contains("addPullRequestReviewThread(")
                    || w.body.contains("updatePullRequestReviewComment")
                    || w.body.contains("deletePullRequestReviewComment")
                    || (w.body.contains("addPullRequestReviewThreadReply") && w.body.contains("pullRequestReviewId")));
            review || gql
        }
        F::GitLab => w.target.contains("/draft_notes"),
        F::Gitea => {
            (w.method == "POST" && w.target.ends_with("/reviews") && w.body.contains("PENDING"))
                || (w.target.contains("/reviews/") && w.target.contains("/comments") && w.method != "GET")
                // A pending comment is edited and deleted by its id (`EditDraft` / `DeleteDraft`).
                || (w.target.contains("/issues/comments/") && matches!(w.method.as_str(), "PATCH" | "DELETE"))
        }
    }
}

fn only_draft_writes(r: &Rig) {
    for w in r.writes() {
        assert!(
            draft_write(r.f, &w),
            "{:?}: not a pending-review draft request: {} {} {}",
            r.f,
            w.method,
            w.target,
            w.body
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_a_claude_line_draft_is_in_the_pending_review_on_each_fake() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        let id = claude_comment(&mut r, "Claude: why change this?", 2, 2).await;
        let d = claude_drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert_eq!(d[0].id, id);
        assert_eq!(d[0].lines, vec!["m2 changed".to_string()], "{f:?}: the text for drift");
        assert!(
            d[0].forge.is_some(),
            "{f:?}: on the forge: {:?}",
            r.app.review.drafts.local_why
        );
        assert!(!r.writes().is_empty(), "{f:?}");
        only_draft_writes(&r);
        let t = text(&mut r.app);
        assert!(
            t.contains("claude") && t.contains("Claude: why change this?"),
            "{f:?}\n{t}"
        );
        // get_review says who wrote it and where it is.
        let a = ask(&mut r.app, Query::Review).await;
        let rows = a.data.unwrap()["review"]["pending_drafts"].clone();
        assert_eq!(rows[0]["author"], "claude", "{f:?}");
        assert_eq!(rows[0]["state"], "on_forge", "{f:?}");
        assert_eq!(rows[0]["id"], id.as_str());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_the_first_claude_draft_starts_the_review_and_later_ones_join_it() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        claude_comment(&mut r, "First", 2, 2).await;
        claude_comment(&mut r, "Second", 3, 3).await;
        let d = claude_drafts(&r.app);
        assert_eq!(d.len(), 2, "{f:?}");
        assert!(
            d.iter().all(|d| d.forge.is_some()),
            "{f:?}: {:?}",
            r.app.review.drafts.local_why
        );
        // GitLab's draft notes need no review start. The Gitea fake lists no pending review between
        // the two drafts (a real Gitea does), so there only the drafts' presence is checked.
        if f == F::GitHub {
            let starts = sent(&r.server, "POST", "/pulls/7/reviews").len();
            assert_eq!(starts, 1, "{f:?}: one pending review: {:?}", r.writes());
        }
        if f == F::GitLab {
            assert_eq!(sent(&r.server, "POST", "/draft_notes").len(), 2);
        } else if f == F::GitHub {
            let ids: std::collections::HashSet<_> =
                d.iter().map(|d| d.forge.as_ref().unwrap().review_id.clone()).collect();
            assert_eq!(ids.len(), 1, "{f:?}: the same review");
        }
        only_draft_writes(&r);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_claude_replaces_and_deletes_its_own_draft_on_the_forge() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        if f == F::GitLab {
            r.server.set(
                &format!(
                    "PUT {}/merge_requests/7/draft_notes/31",
                    crate::mr_review_tests::gitlab::API
                ),
                Reply::json("{}"),
            );
        }
        let id = claude_comment(&mut r, "Old", 2, 2).await;
        if f == F::Gitea {
            let cid = claude_drafts(&r.app)[0].forge.as_ref().unwrap().comment_id.clone();
            let url = format!("/api/v1/repos/alice/project/issues/comments/{cid}");
            r.server.set(&format!("PATCH {url}"), Reply::json("{}"));
            r.server.set(&format!("DELETE {url}"), Reply::status(204));
        }
        let n = r.writes().len();
        let mut q = comment_q("New text", 2, 2, Some(&id));
        if let Query::ReviewComment {
            path,
            right,
            start,
            end,
            ..
        } = &mut q
        {
            (*path, *right, *start, *end) = (None, None, None, None); // only the body and id are needed
        }
        let a = ask(&mut r.app, q).await;
        assert_eq!(a.error, None, "{f:?}");
        quiet(&mut r.app).await;
        let d = claude_drafts(&r.app);
        assert_eq!(
            d.len(),
            1,
            "{f:?}: {:?} {:?}",
            r.app.review.drafts.local_why,
            r.app.review.drafts.errors
        );
        assert_eq!(d[0].body, "New text", "{f:?}");
        assert!(r.writes().len() > n, "{f:?}: the edit reached the forge");
        assert!(r.writes().iter().skip(n).any(|w| w.body.contains("New text")), "{f:?}");
        let n = r.writes().len();
        let a = ask(&mut r.app, Query::ReviewDeleteDraft { draft: id.clone() }).await;
        assert_eq!(a.error, None, "{f:?}");
        quiet(&mut r.app).await;
        assert!(claude_drafts(&r.app).is_empty(), "{f:?}");
        assert!(r.writes().len() > n, "{f:?}: the delete reached the forge");
        only_draft_writes(&r);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_claude_cannot_replace_or_delete_the_users_draft() {
    let mut r = rig(F::GitHub, false).await;
    select(&mut r.app, true, 1, 1);
    r.app.test_execute(Command::ReviewComment);
    r.app.type_text("Mine");
    r.app.press_bound(Command::ReviewSaveComment);
    quiet(&mut r.app).await;
    let id = drafts(&r.app)[0].id.clone();
    let n = r.writes().len();
    let a = ask(&mut r.app, Query::ReviewDeleteDraft { draft: id.clone() }).await;
    assert!(a.error.unwrap_or_default().contains("only your own drafts"));
    let a = ask(&mut r.app, comment_q("Hijack", 2, 2, Some(&id))).await;
    assert!(a.error.is_some());
    let a = ask(&mut r.app, Query::ReviewDeleteDraft { draft: "nope".into() }).await;
    assert!(a.error.is_some());
    quiet(&mut r.app).await;
    assert_eq!(drafts(&r.app)[0].body, "Mine");
    assert_eq!(r.writes().len(), n, "nothing sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_a_claude_reply_is_pending_on_github_and_gitlab_and_local_on_gitea() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        reply_routes(&r);
        let q = Query::ReviewReply {
            thread: thread_id(f).into(),
            body: "Claude fixed it".into(),
            resolve: false,
        };
        let a = ask(&mut r.app, q).await;
        assert_eq!(a.error, None, "{f:?}");
        quiet(&mut r.app).await;
        let d = claude_drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert!(d[0].reply_to.is_some());
        if f == F::Gitea {
            assert!(d[0].forge.is_none(), "Gitea: local only");
            assert!(r.writes().is_empty(), "{:?}", r.writes());
            let a = ask(&mut r.app, Query::Review).await;
            let rows = a.data.unwrap()["review"]["pending_drafts"].clone();
            assert_eq!(rows[0]["state"], "local_only");
        } else {
            assert!(d[0].forge.is_some(), "{f:?}: {:?}", r.app.review.drafts.local_why);
            only_draft_writes(&r);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_with_the_setting_off_claudes_drafts_stay_local() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        r.app.settings.review_drafts_on_forge = false;
        let id = claude_comment(&mut r, "Local", 2, 2).await;
        ask(
            &mut r.app,
            Query::ReviewReply {
                thread: thread_id(f).into(),
                body: "Local reply".into(),
                resolve: false,
            },
        )
        .await;
        let d = claude_drafts(&r.app);
        assert_eq!(d.len(), 2, "{f:?}");
        assert!(d.iter().all(|d| d.forge.is_none()));
        let a = ask(&mut r.app, Query::ReviewDeleteDraft { draft: id }).await;
        assert_eq!(a.error, None);
        assert_eq!(claude_drafts(&r.app).len(), 1);
        assert!(r.writes().is_empty(), "{f:?}: {:?}", r.writes());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_a_comment_must_be_on_lines_of_that_side() {
    let mut r = rig(F::Gitea, false).await;
    let a = ask(&mut r.app, comment_q("x", 50, 50, None)).await;
    assert!(a.error.unwrap_or_default().contains("do not exist"), "past the end");
    let a = ask(&mut r.app, comment_q("", 2, 2, None)).await;
    assert!(a.error.is_some(), "empty");
    let q = Query::ReviewComment {
        path: Some("nope.txt".into()),
        right: Some(true),
        start: Some(1),
        end: Some(1),
        body: "x".into(),
        draft: None,
    };
    assert!(
        ask(&mut r.app, q).await.error.is_some(),
        "not a file of the merge request"
    );
    let q = Query::ReviewComment {
        path: None,
        right: None,
        start: None,
        end: None,
        body: "x".into(),
        draft: None,
    };
    let why = ask(&mut r.app, q).await.error.unwrap_or_default();
    assert!(why.contains("required for a new comment"), "{why}");
    assert!(claude_drafts(&r.app).is_empty());
    assert!(r.writes().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_a_users_reply_and_claudes_comment_are_submitted_with_the_review_in_one_step() {
    for f in [F::GitHub, F::GitLab] {
        let mut r = rig(f, false).await;
        reply_routes(&r);
        reply_saved(&mut r, "User reply").await;
        claude_comment(&mut r, "Claude comment", 2, 2).await;
        assert!(drafts(&r.app).iter().all(|d| d.forge.is_some()), "{f:?}");
        let before = r.writes().len();
        r.gone();
        submit(&mut r.app, Command::ReviewSubmitComment, "");
        submitted(&mut r.app).await;
        let after: Vec<_> = r.writes().into_iter().skip(before).collect();
        match f {
            F::GitHub => {
                assert!(
                    after.iter().any(|w| w.target.ends_with("/reviews/77/events")),
                    "{after:?}"
                );
                assert!(
                    after.iter().all(|w| !w.body.contains("addPullRequestReview")),
                    "no draft is added again"
                );
            }
            _ => assert!(after.iter().any(|w| w.target.contains("publish")), "{after:?}"),
        }
        assert!(drafts(&r.app).is_empty(), "{f:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_claude_cannot_overwrite_the_users_reply_draft() {
    for f in [F::GitHub, F::GitLab, F::Gitea] {
        let mut r = rig(f, false).await;
        reply_routes(&r);
        reply_saved(&mut r, "My own reply").await;
        let n = r.writes().len();
        let a = ask(
            &mut r.app,
            Query::ReviewReply {
                thread: thread_id(f).into(),
                body: "Claude's take".into(),
                resolve: true,
            },
        )
        .await;
        assert!(a.error.unwrap_or_default().contains("own reply draft"), "{f:?}");
        quiet(&mut r.app).await;
        let d = reply_drafts(&r.app);
        assert_eq!(d.len(), 1, "{f:?}");
        assert_eq!(
            (d[0].body.as_str(), d[0].by_claude, d[0].resolve),
            ("My own reply", false, false)
        );
        assert_eq!(r.writes().len(), n, "{f:?}: nothing sent");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_pending_replies_c_github_refuses_a_claude_comment_outside_the_diff() {
    let mut r = rig(F::GitHub, false).await;
    let n = r.writes().len();
    // Line 1 of renamed.txt is outside every hunk of the GitHub fake's diff.
    let q = Query::ReviewComment {
        path: Some("renamed.txt".into()),
        right: Some(true),
        start: Some(1),
        end: Some(1),
        body: "outside".into(),
        draft: None,
    };
    let why = ask(&mut r.app, q).await.error.unwrap_or_default();
    assert!(why.contains(crate::review_drafts::NOT_IN_DIFF), "{why}");
    quiet(&mut r.app).await;
    assert!(drafts(&r.app).is_empty(), "no draft stored");
    assert_eq!(r.writes().len(), n, "no write: {:?}", r.writes());
}
