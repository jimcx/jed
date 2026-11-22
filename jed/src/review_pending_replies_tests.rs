//! Replies in the pending review, subtask a (replies as drafts, Comment Now for replies), driven
//! through the app against the fake Gitea, GitHub and GitLab servers: the reply box's main button
//! saves the reply as a draft (`[Add to Review]` / `[Start Review]` / `[Save Draft]` with the
//! header saying so) and `[Comment Now]` posts it at once without touching the pending review;
//! a saved reply is a pending reply on GitHub and GitLab, local only on Gitea (posted after the
//! submit), listed in PENDING, edited and deleted there, one per thread (a second Reply opens it,
//! Claude's too, and saving makes it the user's); an edit, a conversation comment and the review
//! summary still post at once and show no Comment Now; with the setting off nothing reaches the
//! forge before Submit Review.

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
    // A new line comment has no Comment Now yet; an edit posts at once and has none either.
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
