//! Review fixes, subtask a: the review's FILES list only the merge request's own files (the merge
//! base's tree against the MR head, else local HEAD), shown as the work tree has them: untracked
//! files and locally edited files outside the MR are hidden, an MR file edited locally shows with
//! the edit's counts, and a local HEAD behind the MR still lists the MR's files.

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
