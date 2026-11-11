//! claude_ide-e, the code review tools: the `Review*` IPC queries against a headless app with a
//! loaded merge request on the fake forge. Threads read back with their live lines and parsed
//! suggestions; apply saves the file and refuses an unsaved or out-of-date one; a reply draft is
//! tagged `claude`, posts nothing, is replaced by a second call and goes out (reply, then
//! resolve) only with the user's Submit Review; a failed reply stays; `mcp_read_only` refuses the
//! write tools; no query or tool reaches the forge with a write on its own.

use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::ipc::client::Connection;
use crate::ipc::{Query, Reply as IpcReply, Request};

const REVIEWS: &str = "/api/v1/repos/alice/project/pulls/7/reviews";
const ONE: &str = "Use this\\n```suggestion\\nm3 fixed\\nm4\\n```";
const FIXED: &str = "m1\nm2 changed\nm3 fixed\nm4\n";
const ORIGINAL: &str = "m1\nm2 changed\nm3\n";

struct World {
    app: App,
    root: PathBuf,
    server: FakeServer,
    socket: PathBuf,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// `with_reply_route`: the fake takes a reply (and the resolve) on thread 11.
async fn boot(read_only: bool, with_reply_route: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let sockets = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let pr = pr_json(7, "Add review panel", &main).replace(
        r#""ref":"feature","sha":"h0""#,
        &format!(r#""ref":"feature","sha":"{head}""#),
    );
    let comment = threads::rc(11, "bob", ONE, "mod.txt", 3, &head, "+m3", false);
    let mut routes = vec![
        (
            format!("{PULLS}?state=open&page=1&limit=50"),
            Reply::json(&format!("[{pr}]")),
        ),
        (format!("{PULLS}/7"), Reply::json(&pr)),
        (
            format!("{PULLS}/7/reviews?page=1&limit=50"),
            Reply::json(r#"[{"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9}]"#),
        ),
        (
            format!("{PULLS}/7/reviews/1/comments"),
            Reply::json(&format!("[{comment}]")),
        ),
        (
            "/api/v1/repos/alice/project/issues/7/comments?page=1&limit=50".into(),
            Reply::json("[]"),
        ),
        ("/api/v1/user".into(), Reply::json(r#"{"login":"sam"}"#)),
        (format!("POST {REVIEWS}"), Reply::json("{}")),
    ];
    if with_reply_route {
        routes.push((format!("POST {PULLS}/7/comments/11/replies"), Reply::json("{}")));
        routes.push((format!("POST {PULLS}/comments/11/resolve"), Reply::status(204)));
    }
    let server = FakeServer::start(routes);
    let mut config = config_for(&server);
    config.entries[0].mcp_read_only = read_only;
    let base = workspace::normalize(&root);
    let trust = TrustStore::load(base.join("trust.toml"));
    let mut app = App::new(Workspace::load(base), trust, None, true);
    let sock_dir = workspace::normalize(sockets.path()).join("s");
    let socket = crate::ipc::socket_path(&sock_dir, std::process::id());
    app.ipc_dir = Some(sock_dir);
    app.review.config = Some(Arc::new(config));
    app.review.env = Arc::new(|n| (n == "FAKE_TOKEN").then(|| "secret".to_string()));
    app.review.auto = true;
    app.test_start();
    assert!(app.settle(|a| a.tasks.ipc.is_some()).await, "the socket is listening");
    assert!(app.settle(loaded).await);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
    World {
        app,
        root,
        server,
        socket,
        _dirs: (dir, sockets),
    }
}

impl World {
    async fn ask(&mut self, q: Query) -> IpcReply {
        let socket = self.socket.clone();
        let client = std::thread::spawn(move || {
            let mut conn = Connection::connect(&socket, Some(Duration::from_secs(20))).unwrap();
            conn.request(&Request::query(q, None)).unwrap()
        });
        assert!(self.app.settle(|_| client.is_finished()).await, "no answer");
        client.join().unwrap()
    }

    async fn data(&mut self, q: Query) -> Value {
        let r = self.ask(q).await;
        assert_eq!(r.error, None);
        r.data.unwrap()
    }

    async fn error(&mut self, q: Query) -> String {
        let r = self.ask(q).await;
        assert_eq!(r.data, None, "an error answer");
        r.error.expect("an error")
    }

    fn writes(&self) -> Vec<String> {
        self.server
            .requests()
            .iter()
            .filter(|r| r.method != "GET")
            .map(|r| format!("{} {}", r.method, r.target))
            .collect()
    }

    fn disk(&self) -> String {
        fs::read_to_string(self.root.join("mod.txt")).unwrap()
    }
}

fn reply(thread: &str, body: &str, resolve: bool) -> Query {
    Query::ReviewReply {
        thread: thread.into(),
        body: body.into(),
        resolve,
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_review_reads_back_the_merge_request_and_its_threads() {
    let mut w = boot(false, false).await;
    let r = w.data(Query::Review).await;
    let r = &r["review"];
    assert_eq!(
        (r["number"].as_u64(), r["title"].as_str()),
        (Some(7), Some("Add review panel"))
    );
    assert_eq!(r["unresolved"], 1);
    assert_eq!(r["read_only"], false);
    assert_eq!(r["target_branch"], "main");
    assert!(
        r["files"].as_array().unwrap().iter().any(|f| f["path"] == "mod.txt"),
        "{r}"
    );
    assert!(r["fetched_at"].as_u64().is_some());
    let t = w
        .data(Query::ReviewThreads {
            path: None,
            resolved: false,
        })
        .await;
    let t = &t["threads"][0];
    assert_eq!((t["id"].as_str(), t["path"].as_str()), (Some("11"), Some("mod.txt")));
    assert_eq!(t["placement"]["status"], "placed");
    assert_eq!(
        (
            t["placement"]["start_line"].as_u64(),
            t["placement"]["end_line"].as_u64()
        ),
        (Some(3), Some(3))
    );
    assert_eq!(t["comments"][0]["author"], "bob");
    assert!(t["comments"][0]["body"].as_str().unwrap().contains("Use this"));
    assert_eq!(t["suggestions"][0]["new_lines"], json!(["m3 fixed", "m4"]));
    assert_eq!(t["own_last"], false);
    let other = w
        .data(Query::ReviewThreads {
            path: Some("other.txt".into()),
            resolved: false,
        })
        .await;
    assert!(other["threads"].as_array().unwrap().is_empty());
    // The prompt hook's counts ride on the context.
    let ctx = w.data(Query::Context).await;
    assert_eq!(ctx["review"]["unresolved"], 1);
    assert_eq!(ctx["review"]["number"], 7);
    assert!(w.writes().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_apply_changes_and_saves_the_file_in_one_undo_step() {
    let mut w = boot(false, false).await;
    let a = w.data(Query::ReviewApply { thread: "11".into() }).await;
    assert_eq!((a["applied"].as_bool(), a["saved"].as_bool()), (Some(true), Some(true)));
    assert_eq!(w.disk(), FIXED, "saved to disk");
    let id = w.app.buffers.find_path(&w.root.join("mod.txt")).unwrap();
    assert!(!w.app.buffers.get(id).unwrap().is_dirty());
    assert!(w.app.buffers.get_mut(id).unwrap().undo().is_some());
    assert_eq!(
        w.app.buffers.get(id).unwrap().rope.to_string(),
        ORIGINAL,
        "one undo step"
    );
    // The second apply finds the lines changed (the buffer was undone, so it is now unsaved).
    let why = w.error(Query::ReviewApply { thread: "11".into() }).await;
    assert!(why.contains("unsaved changes in jed"), "{why}");
    assert!(w.writes().is_empty(), "{:?}", w.writes());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_apply_is_refused_for_unsaved_changes_and_out_of_date_suggestions() {
    let mut w = boot(false, false).await;
    // Unsaved changes in jed.
    w.app.test_open_file(w.root.join("mod.txt"));
    let file = w.root.join("mod.txt");
    assert!(w.app.settle(|a| a.buffers.find_path(&file).is_some()).await);
    let id = w.app.buffers.find_path(&w.root.join("mod.txt")).unwrap();
    let mut view = crate::view::View::new();
    crate::edit::paste(w.app.buffers.get_mut(id).unwrap(), &mut view, "X", false, &[]);
    let why = w.error(Query::ReviewApply { thread: "11".into() }).await;
    assert!(why.contains("unsaved changes in jed"), "{why}");
    assert_eq!(w.disk(), ORIGINAL);
    // Saved, the suggestion applies once; the second finds the lines changed.
    assert!(w.app.buffers.get_mut(id).unwrap().undo().is_some());
    assert!(!w.app.buffers.get(id).unwrap().is_dirty());
    w.data(Query::ReviewApply { thread: "11".into() }).await;
    assert_eq!(w.disk(), FIXED);
    let why = w.error(Query::ReviewApply { thread: "11".into() }).await;
    assert!(why.contains("out of date"), "{why}");
    assert_eq!(w.disk(), FIXED);
    let why = w.error(Query::ReviewApply { thread: "nope".into() }).await;
    assert!(why.contains("no thread"), "{why}");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_a_reply_draft_is_tagged_claude_replaced_not_posted_and_submitted_by_the_user() {
    let mut w = boot(false, true).await;
    let d = w.data(reply("11", "First try", false)).await;
    assert_eq!(
        (d["drafted"].as_bool(), d["replaced"].as_bool()),
        (Some(true), Some(false))
    );
    let d = w.data(reply("11", "Fixed in the next commit", true)).await;
    assert_eq!(d["replaced"], true, "a second call replaces");
    assert_eq!(w.app.review.draft_rows().len(), 1);
    let row = &w.app.review.draft_rows()[0];
    assert_eq!(row.draft.body, "Fixed in the next commit");
    assert!(row.draft.by_claude && row.draft.resolve && row.draft.reply_to.as_deref() == Some("11"));
    assert_eq!(row.held, None, "a reply draft is never held back");
    let screen = wide(&mut w.app);
    assert!(screen.contains("[claude"), "{screen}");
    let review = w.data(Query::Review).await;
    assert_eq!(review["review"]["pending_drafts"][0]["by_claude"], true);
    let threads = w
        .data(Query::ReviewThreads {
            path: None,
            resolved: false,
        })
        .await;
    assert_eq!(threads["threads"][0]["reply_draft"]["resolve"], true);
    assert!(w.writes().is_empty(), "nothing is posted by the tool: {:?}", w.writes());
    // The user's Submit Review posts the reply, then resolves the thread.
    w.app.test_execute(Command::ReviewSubmitComment);
    assert!(
        w.app
            .settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    let writes = w.writes();
    assert!(
        writes
            .iter()
            .any(|x| x == &format!("POST {PULLS}/7/comments/11/replies")),
        "{writes:?}"
    );
    assert!(
        writes.iter().any(|x| x == &format!("POST {PULLS}/comments/11/resolve")),
        "{writes:?}"
    );
    assert!(
        !writes.iter().any(|x| x.ends_with("/reviews")),
        "no review without line comments: {writes:?}"
    );
    let body = w
        .server
        .requests()
        .into_iter()
        .find(|r| r.target.ends_with("/replies"))
        .unwrap()
        .body;
    assert_eq!(body, r#"{"body":"Fixed in the next commit"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_a_failed_reply_stays_pending_with_its_error() {
    let mut w = boot(false, false).await;
    w.data(reply("11", "Will not post", false)).await;
    w.app.test_execute(Command::ReviewSubmitComment);
    assert!(
        w.app
            .settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
            .await
    );
    assert_eq!(w.app.review.draft_rows().len(), 1, "kept");
    let screen = wide(&mut w.app);
    assert!(screen.contains("failed:"), "{screen}");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_read_only_hosts_refuse_the_write_tools_and_still_read() {
    let mut w = boot(true, true).await;
    assert_eq!(w.data(Query::Review).await["review"]["read_only"], true);
    let t = w
        .data(Query::ReviewThreads {
            path: None,
            resolved: false,
        })
        .await;
    assert_eq!(t["threads"].as_array().unwrap().len(), 1);
    let why = w.error(reply("11", "Hi", false)).await;
    assert!(
        why.contains("read-only for Claude (mcp_read_only in forges.toml)"),
        "{why}"
    );
    let why = w.error(Query::ReviewApply { thread: "11".into() }).await;
    assert!(why.contains("mcp_read_only"), "{why}");
    assert!(w.app.review.draft_rows().is_empty(), "no draft");
    assert_eq!(w.disk(), ORIGINAL, "no edit, no save");
    assert!(
        w.app.buffers.find_path(&w.root.join("mod.txt")).is_none(),
        "the file was not even opened"
    );
    // A refresh is a read: it works.
    w.data(Query::ReviewRefresh).await;
    assert!(w.writes().is_empty());
}

/// Every query and every tool: none writes to the forge, and the tool list has no submit.
#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_no_query_or_tool_writes_to_the_forge() {
    // Exhaustive on purpose: a new query must be classified here.
    fn classified(q: &Query) {
        match q {
            Query::Context
            | Query::Editors
            | Query::Lsp { .. }
            | Query::Diagnostics { .. }
            | Query::Terminals
            | Query::Commands { .. }
            | Query::Output { .. }
            | Query::Screen { .. }
            | Query::Environment
            | Query::Open { .. }
            | Query::Review
            | Query::ReviewThreads { .. }
            | Query::ReviewRefresh
            | Query::ReviewApply { .. }
            | Query::ReviewReply { .. } => {}
        }
    }
    let mut w = boot(false, true).await;
    let path = w.root.join("mod.txt");
    let mut queries = Vec::new();
    for tool in crate::claude::tools() {
        let schema = &tool["inputSchema"];
        let mut args = serde_json::Map::new();
        for r in schema["required"].as_array().unwrap() {
            let name = r.as_str().unwrap();
            let kind = schema["properties"][name]["type"].as_str().unwrap();
            args.insert(
                name.to_string(),
                match (name, kind) {
                    ("path", _) => json!(path.display().to_string()),
                    ("body", _) => json!("A reply"),
                    ("thread", _) => json!("11"),
                    (_, "integer") => json!(1),
                    _ => json!("x"),
                },
            );
        }
        let name = tool["name"].as_str().unwrap();
        let q = crate::claude::tool_query(name, &Value::Object(args), &w.root)
            .unwrap()
            .unwrap();
        classified(&q);
        queries.push((name.to_string(), q));
    }
    assert!(queries.len() >= 14);
    for (name, q) in queries {
        w.ask(q).await;
        assert!(w.writes().is_empty(), "{name} wrote to the forge: {:?}", w.writes());
    }
    // The apply saved the file and the reply is a draft, still unsent.
    assert_eq!(w.disk(), FIXED);
    assert_eq!(w.app.review.draft_rows().len(), 1);
    assert!(w.app.review.draft_rows()[0].draft.by_claude);
}
