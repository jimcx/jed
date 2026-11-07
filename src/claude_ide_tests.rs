//! End-to-end tests for the Claude Code integration's IPC queries (claude_ide-b): a blocking
//! client (`ipc::client::Connection`) against a headless app listening on the task socket gets
//! the context (file, language, cursor, selection and its capped text), editors, diagnostics
//! (errors first, capped), terminals (the caller flagged `self`), commands and their output, a
//! terminal's screen and the environment; many requests share one connection and old `open`
//! requests still work; log lines are acked, bad requests close the connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::app::App;
use crate::buffer::pos::ByteIdx;
use crate::diagnostics::{Mark, Severity, tests::diag};
use crate::ipc::client::Connection;
use crate::ipc::{LogLine, Query, Reply, Request};
use crate::lsp::ServerId;
use crate::selection::{Selection, Selections};
use crate::shell_tests::{dirs, listening_app, run_in_shell};

const SOURCE: &str = "fn main() {\n    let x = 1;\n}\n";

/// `requests` over one connection, answered while the app runs.
async fn ask(app: &mut App, socket: &Path, requests: Vec<Request>) -> Vec<Reply> {
    let socket = socket.to_path_buf();
    let client = std::thread::spawn(move || {
        let mut conn = Connection::connect(&socket, Some(Duration::from_secs(20))).unwrap();
        requests.iter().map(|r| conn.request(r).unwrap()).collect::<Vec<_>>()
    });
    assert!(app.settle(|_| client.is_finished()).await, "no answer");
    client.join().unwrap()
}

async fn query(app: &mut App, socket: &Path, q: Query) -> Reply {
    ask(app, socket, vec![Request::query(q, None)]).await.remove(0)
}

fn data(reply: &Reply) -> &Value {
    assert_eq!(reply.error, None, "an error answer");
    reply.data.as_ref().expect("a data answer")
}

/// An app with `a.rs` open (through an old-style `open` request).
async fn with_file(text: &str) -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let (d, root, state) = dirs();
    let (mut app, socket) = listening_app(&root, &state, &d).await;
    let path = root.join("a.rs");
    std::fs::write(&path, text).unwrap();
    let open = Request {
        open: path.clone(),
        ..Request::default()
    };
    let replies = ask(&mut app, &socket, vec![open]).await;
    assert_eq!(replies, vec![Reply::opened()], "an old request is answered as before");
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path == path)).await,
        "the file opens"
    );
    (d, app, socket, path)
}

fn select(app: &mut App, anchor: usize, head: usize) {
    let tab = app.groups.active_group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::single(Selection::new(ByteIdx(anchor), ByteIdx(head)));
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_context_reports_file_cursor_and_selection() {
    let (_d, mut app, socket, path) = with_file(SOURCE).await;
    // "let x" on line 2 (bytes 16..21), the head at its end.
    select(&mut app, 16, 21);
    let reply = query(&mut app, &socket, Query::Context).await;
    let editor = &data(&reply)["editor"];
    assert_eq!(editor["path"], path.display().to_string());
    assert_eq!(editor["relative"], "a.rs");
    assert_eq!(editor["language"], "Rust");
    assert_eq!(editor["dirty"], false);
    assert_eq!(editor["cursor"]["line"], 2);
    assert_eq!(editor["cursor"]["col"], 10);
    assert_eq!(editor["selection"]["text"], "let x");
    assert_eq!(editor["selection"]["truncated"], false);
    assert_eq!(editor["selection_count"], 1);
    assert_eq!(editor["ranges"][0]["start"]["col"], 5);
    assert_eq!(editor["ranges"][0]["end"]["col"], 10);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_context_caps_a_huge_selection_and_its_ranges() {
    let text = "é".repeat(20_000);
    let (_d, mut app, socket, _path) = with_file(&text).await;
    select(&mut app, 0, text.len());
    let reply = query(&mut app, &socket, Query::Context).await;
    let selection = &data(&reply)["editor"]["selection"];
    let shown = selection["text"].as_str().unwrap();
    assert!(
        shown.len() <= 8 * 1024 && shown.len() > 8 * 1024 - 4,
        "{} bytes",
        shown.len()
    );
    assert_eq!(selection["truncated"], true);
    // Many cursors: only a handful of ranges are listed, the count is exact.
    let list: Vec<Selection> = (0..500)
        .map(|i| Selection::new(ByteIdx(i * 2), ByteIdx(i * 2)))
        .collect();
    let tab = app.groups.active_group_mut().active_tab_mut().unwrap();
    tab.view.selections = Selections::new(list, 0);
    let reply = query(&mut app, &socket, Query::Context).await;
    let editor = &data(&reply)["editor"];
    assert_eq!(editor["selection_count"], 500);
    assert_eq!(editor["ranges"].as_array().unwrap().len(), 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_context_without_an_editor_is_null() {
    let (d, root, state) = dirs();
    let (mut app, socket) = listening_app(&root, &state, &d).await;
    let reply = query(&mut app, &socket, Query::Context).await;
    assert!(data(&reply)["editor"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_editors_lists_every_group_tab() {
    let (_d, mut app, socket, path) = with_file(SOURCE).await;
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    let reply = query(&mut app, &socket, Query::Editors).await;
    let groups = data(&reply)["groups"].as_array().unwrap();
    let tabs: Vec<&Value> = groups.iter().flat_map(|g| g["tabs"].as_array().unwrap()).collect();
    let text = tabs.iter().find(|t| t["kind"] == "text").expect("the file's tab");
    assert_eq!(text["path"], path.display().to_string());
    assert_eq!(text["name"], "a.rs");
    assert!(tabs.iter().any(|t| t["kind"] == "shell"), "the shell's tab: {tabs:?}");
    assert_eq!(
        groups
            .iter()
            .filter(|g| g["area"] == "editor" && g["active"] == true)
            .count(),
        1
    );
}

fn marks(app: &mut App, count: usize) {
    let buf = app.active_mut().unwrap().0;
    let list = (0..count)
        .map(|i| {
            let severity = if i == count - 1 {
                Severity::Error
            } else {
                Severity::Hint
            };
            Mark {
                start: ByteIdx(16),
                end: ByteIdx(17),
                server: ServerId::test(1),
                diag: diag(severity, &format!("problem {i}")),
            }
        })
        .collect();
    buf.diagnostics.replace(ServerId::test(1), list);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_diagnostics_by_path_and_capped_errors_first() {
    let (_d, mut app, socket, path) = with_file(SOURCE).await;
    marks(&mut app, 3);
    let reply = query(&mut app, &socket, Query::Diagnostics { path: None }).await;
    let list = data(&reply)["diagnostics"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0]["severity"], "error");
    assert_eq!(list[0]["message"], "problem 2");
    assert_eq!((list[0]["line"].clone(), list[0]["col"].clone()), (2.into(), 5.into()));
    assert_eq!(list[0]["path"], path.display().to_string());

    // A relative path names the same file; an unopened one is an error answer.
    let by_path = query(
        &mut app,
        &socket,
        Query::Diagnostics {
            path: Some("a.rs".into()),
        },
    )
    .await;
    assert_eq!(data(&by_path)["count"], 3);
    let missing = query(
        &mut app,
        &socket,
        Query::Diagnostics {
            path: Some("nope.rs".into()),
        },
    )
    .await;
    assert!(missing.error.unwrap().contains("not open"));

    marks(&mut app, 300);
    let reply = query(&mut app, &socket, Query::Diagnostics { path: None }).await;
    let list = data(&reply)["diagnostics"].as_array().unwrap();
    assert_eq!(list.len(), 200);
    assert_eq!(data(&reply)["count"], 300);
    assert_eq!(data(&reply)["truncated"], true);
    assert_eq!(list[0]["severity"], "error", "the cap drops hints first");
}

/// A command in the shell as the integration reports it: `133;B`, `633;E`, output, `133;D`.
fn run_recorded(app: &mut App, command: &str, output: &str, exit: i32) {
    run_in_shell(
        app,
        &format!("printf '\\033]133;B\\007\\033]633;E;{command}\\007{output}\\n\\033]133;D;{exit}\\007'"),
    );
}

fn shell_id(app: &App) -> u64 {
    app.group().active_shell().unwrap().run.id.0
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_terminals_commands_and_output() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    app.new_shell_in_editor();
    let id = shell_id(&app);
    run_recorded(&mut app, "make test", "boom", 2);
    assert!(
        app.settle(|a| {
            let t = a.group().active_shell().unwrap();
            t.run.commands().iter().any(|c| c.exit == Some(2))
        })
        .await,
        "the command is recorded"
    );

    // Many requests on one connection, answered in order; the caller is another terminal (99).
    let replies = ask(
        &mut app,
        &socket,
        vec![
            Request::query(Query::Terminals, Some(99)),
            Request::query(Query::Terminals, Some(id)),
            Request::query(
                Query::Commands {
                    terminal: None,
                    n: None,
                },
                Some(99),
            ),
            Request::query(
                Query::Commands {
                    terminal: None,
                    n: None,
                },
                Some(id),
            ),
            Request::query(
                Query::Commands {
                    terminal: Some(id),
                    n: Some(1),
                },
                Some(id),
            ),
            Request::query(
                Query::Output {
                    terminal: id,
                    command: 1,
                    max_lines: Some(5),
                },
                None,
            ),
            Request::query(
                Query::Output {
                    terminal: id,
                    command: 77,
                    max_lines: None,
                },
                None,
            ),
            Request::query(
                Query::Commands {
                    terminal: Some(4242),
                    n: None,
                },
                None,
            ),
        ],
    )
    .await;
    let terminal = |r: &Reply, i: usize| data(r)["terminals"][i].clone();
    assert_eq!(terminal(&replies[0], 0)["id"], id);
    assert_eq!(terminal(&replies[0], 0)["kind"], "shell");
    assert_eq!(terminal(&replies[0], 0)["self"], false);
    assert_eq!(terminal(&replies[0], 0)["last_command"]["command"], "make test");
    assert_eq!(terminal(&replies[0], 0)["last_command"]["exit"], 2);
    assert_eq!(terminal(&replies[1], 0)["self"], true);

    let first = &data(&replies[2])["terminals"][0]["commands"][0];
    assert_eq!(first["command"], "make test");
    assert_eq!(first["exit"], 2);
    assert_eq!(first["running"], false);
    assert!(
        data(&replies[3])["terminals"].as_array().unwrap().is_empty(),
        "the caller's own shell is left out"
    );
    assert_eq!(
        data(&replies[4])["terminals"][0]["commands"].as_array().unwrap().len(),
        1
    );

    let out = data(&replies[5]);
    assert!(out["text"].as_str().unwrap().contains("boom"), "{out}");
    assert_eq!(out["running"], false);
    assert!(replies[6].error.as_ref().unwrap().contains("no command 77"));
    assert!(replies[7].error.as_ref().unwrap().contains("no terminal 4242"));
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_screen_shows_a_terminals_text() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    app.new_shell_in_editor();
    let id = shell_id(&app);
    run_in_shell(&mut app, "echo marker-$((6*7))");
    assert!(
        app.settle(|a| {
            let t = a.group().active_shell().unwrap();
            t.run.screen().screen().contents().contains("marker-42")
        })
        .await
    );
    let reply = query(&mut app, &socket, Query::Screen { terminal: id }).await;
    assert!(data(&reply)["text"].as_str().unwrap().contains("marker-42"));
    assert!(data(&reply)["rows"].as_u64().unwrap() > 0);
    let gone = query(&mut app, &socket, Query::Screen { terminal: 4242 }).await;
    assert!(gone.error.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_environment_lists_workspace_files_and_shells() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    app.new_shell_in_editor();
    let reply = query(&mut app, &socket, Query::Environment).await;
    let env = data(&reply);
    assert_eq!(env["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(env["workspace"][0], app.workspace.root.display().to_string());
    assert_eq!(env["open_files"][0], "a.rs");
    assert_eq!(env["shells"].as_array().unwrap().len(), 1);
    assert!(env["task_environment"]["name"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_open_query_opens_a_file_and_log_lines_are_acked() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    let other = app.workspace.root.join("b.rs");
    std::fs::write(&other, "// b\nfn b() {}\n").unwrap();
    let replies = ask(
        &mut app,
        &socket,
        vec![
            Request::query(
                Query::Open {
                    path: other.clone(),
                    line: Some(2),
                    col: Some(4),
                },
                None,
            ),
            Request {
                log: Some(LogLine {
                    dir: "in".into(),
                    msg: serde_json::json!({"method": "initialize"}),
                }),
                ..Request::default()
            },
            Request::query(
                Query::Open {
                    path: "relative.rs".into(),
                    line: None,
                    col: None,
                },
                None,
            ),
        ],
    )
    .await;
    assert_eq!(replies[0], Reply::opened());
    assert_eq!(replies[1], Reply::default());
    assert!(replies[2].error.is_some());
    assert!(app.settle(|a| a.active_buffer().is_some_and(|b| b.path == other)).await);
    let cursor = app.active_buffer().map(|_| {
        app.groups
            .active_group()
            .active_tab()
            .unwrap()
            .view
            .selections
            .primary()
            .head
    });
    assert_eq!(cursor, Some(ByteIdx(8)), "line 2, column 4");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_a_request_asking_for_nothing_closes_the_connection() {
    let (d, root, state) = dirs();
    let (mut app, socket) = listening_app(&root, &state, &d).await;
    let client = std::thread::spawn(move || {
        let mut conn = Connection::connect(&socket, Some(Duration::from_secs(20))).unwrap();
        conn.request(&Request {
            wait: true,
            ..Request::default()
        })
    });
    assert!(app.settle(|_| client.is_finished()).await);
    assert!(client.join().unwrap().is_err());
}

/// Raw lines over one connection: reads every reply line until the server closes it.
async fn raw(app: &mut App, socket: &Path, lines: Vec<Vec<u8>>) -> Vec<String> {
    use std::io::{BufRead, BufReader, Write};
    let socket = socket.to_path_buf();
    let client = std::thread::spawn(move || {
        let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        // A write may fail once the server has closed the connection on a bad line.
        for l in &lines {
            let _ = writer.write_all(l);
        }
        // Hang up the write side, as a client that is done does.
        let _ = writer.shutdown(std::net::Shutdown::Write);
        BufReader::new(stream).lines().map_while(Result::ok).collect::<Vec<_>>()
    });
    assert!(
        app.settle(|_| client.is_finished()).await,
        "the server never closed the connection"
    );
    client.join().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_bad_lines_end_the_connection_after_the_good_ones() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    let ctx = b"{\"query\":{\"type\":\"context\"}}\n".to_vec();
    // Malformed JSON mid-connection: the first request is answered, the rest is dropped.
    let got = raw(
        &mut app,
        &socket,
        vec![ctx.clone(), b"not json\n".to_vec(), ctx.clone()],
    )
    .await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("\"editor\""));
    // An unknown query type does not decode: closed with no answer, nothing hangs.
    let got = raw(&mut app, &socket, vec![b"{\"query\":{\"type\":\"nope\"}}\n".to_vec()]).await;
    assert!(got.is_empty(), "{got:?}");
    // A line over the 64 KB cap (no newline within it) ends the connection.
    let mut big = b"{\"log\":{\"dir\":\"in\",\"msg\":\"".to_vec();
    big.extend(std::iter::repeat_n(b'x', 70 * 1024));
    big.extend_from_slice(b"\"}}\n");
    let got = raw(&mut app, &socket, vec![big]).await;
    assert!(got.is_empty(), "{got:?}");
    // A client that sends half a line and hangs up is just dropped, and the server still serves.
    let got = raw(&mut app, &socket, vec![b"{\"query\":".to_vec()]).await;
    assert!(got.is_empty());
    let reply = query(&mut app, &socket, Query::Context).await;
    assert!(data(&reply)["editor"].is_object());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_diagnostics_path_filters_to_one_file() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    marks(&mut app, 2);
    let other = app.workspace.root.join("b.rs");
    std::fs::write(&other, SOURCE).unwrap();
    let open = Request {
        open: other.clone(),
        ..Request::default()
    };
    ask(&mut app, &socket, vec![open]).await;
    assert!(app.settle(|a| a.active_buffer().is_some_and(|b| b.path == other)).await);
    marks(&mut app, 1);
    let all = query(&mut app, &socket, Query::Diagnostics { path: None }).await;
    assert_eq!(data(&all)["count"], 3);
    let one = query(
        &mut app,
        &socket,
        Query::Diagnostics {
            path: Some(other.clone()),
        },
    )
    .await;
    assert_eq!(data(&one)["count"], 1);
    assert_eq!(data(&one)["diagnostics"][0]["path"], other.display().to_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_output_of_a_running_command_and_line_caps() {
    let (_d, mut app, socket, _path) = with_file(SOURCE).await;
    app.new_shell_in_editor();
    let id = shell_id(&app);
    // A finished command with 30 lines of output, then one that has started and not ended.
    let lines = (1..=30).map(|i| format!("L{i}")).collect::<Vec<_>>().join("\\n");
    run_recorded(&mut app, "seq 30", &lines, 0);
    assert!(
        app.settle(|a| a
            .group()
            .active_shell()
            .unwrap()
            .run
            .commands()
            .iter()
            .any(|c| c.exit == Some(0)))
            .await
    );
    run_in_shell(&mut app, "printf '\\033]133;B\\007\\033]633;E;sleep 99\\007partial\\n'");
    assert!(
        app.settle(|a| a
            .group()
            .active_shell()
            .unwrap()
            .run
            .commands()
            .iter()
            .any(|c| c.exit.is_none()))
            .await
    );
    let ask_output = |command, max_lines| {
        Request::query(
            Query::Output {
                terminal: id,
                command,
                max_lines,
            },
            None,
        )
    };
    let replies = ask(
        &mut app,
        &socket,
        vec![
            ask_output(1, Some(5)),
            ask_output(1, Some(0)),
            ask_output(1, Some(1_000_000)),
            ask_output(2, None),
        ],
    )
    .await;
    let five = data(&replies[0]);
    assert_eq!(five["truncated"], true);
    let text = five["text"].as_str().unwrap();
    assert!(text.contains("L30") && !text.contains("L20"), "{text}");
    assert!(text.lines().count() <= 5);
    // 0 clamps up to one line; a huge request clamps down but still returns everything here.
    assert_eq!(data(&replies[1])["text"].as_str().unwrap().lines().count(), 1);
    let all = data(&replies[2]);
    assert!(all["text"].as_str().unwrap().contains("L1\n"), "{all}");
    assert_eq!(all["truncated"], false);
    let running = data(&replies[3]);
    assert_eq!(running["running"], true);
    assert!(running["text"].as_str().unwrap().contains("partial"));
}
