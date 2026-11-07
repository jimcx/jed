//! The answers to the Claude Code integration's IPC queries (`ipc::Query`; PLAN.md, Claude Code
//! integration): `claude_answer` runs on the UI thread from `ipc_request` and reads only loaded
//! state, one builder per query, each reading only what it needs and capping what it returns
//! (selection text, ranges, tabs, diagnostics, commands, output lines and bytes, the screen), so
//! a reply stays far below `ipc::MAX_REPLY`: never a scan of the selection list (the primary
//! selection and the first few ranges), never a file read or a `git2` call.
//!
//! Lines and columns in answers are 1-based, the column a display column (a tab is `tab_width`
//! wide), as the `open` request takes them. A truncated text has a `truncated: true` beside it.

use std::path::Path;

use serde_json::{Value, json};

use super::App;
use super::tab_tip::path_text;
use crate::buffer::{Buffer, pos};
use crate::diagnostics::Severity;
use crate::group::{Tab, TextTab};
use crate::ipc::{Query, Reply};
use crate::selection::Selection;
use crate::tasks::{Run, RunId, Status};

/// The primary selection's text, in bytes.
const MAX_SELECTION: usize = 8 * 1024;
/// Selection ranges listed (the primary first); `selection_count` has the rest's number.
const MAX_RANGES: usize = 8;
/// Tabs listed in all.
const MAX_TABS: usize = 200;
/// Diagnostics listed in all, and one's message in bytes.
const MAX_DIAGNOSTICS: usize = 200;
const MAX_MESSAGE: usize = 500;
/// Commands listed per terminal (the ring holds 50), terminals listed with commands, and one
/// command line's bytes.
const MAX_COMMANDS: usize = 50;
const MAX_TERMINALS: usize = 20;
const MAX_COMMAND_LINE: usize = 1000;
/// Output lines of one command (`command_output` holds the terminal's parser lock while it pages
/// through the scrollback), the default, and the reply's bytes.
const MAX_LINES: usize = 2000;
const DEFAULT_LINES: usize = 200;
const MAX_OUTPUT: usize = 64 * 1024;
/// A terminal's visible screen, in bytes.
const MAX_SCREEN: usize = 32 * 1024;
/// Entries of the environment's lists (open files, templates, ...).
const MAX_LIST: usize = 100;

/// `text` cut to at most `max` bytes at a char boundary, keeping the start.
fn clip_head(text: &str, max: usize) -> (&str, bool) {
    let end = text.floor_char_boundary(max);
    (&text[..end], end < text.len())
}

/// `text` cut to at most `max` bytes at a char boundary, keeping the end.
fn clip_tail(text: &str, max: usize) -> (&str, bool) {
    let start = text.ceil_char_boundary(text.len().saturating_sub(max));
    (&text[start..], start > 0)
}

/// What a listed run is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Shell,
    Task,
}

impl RunKind {
    fn name(self) -> &'static str {
        match self {
            RunKind::Shell => "shell",
            RunKind::Task => "task",
        }
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

fn state_name(status: &Status) -> &'static str {
    match status {
        Status::Starting => "starting",
        Status::Running => "running",
        Status::Exited(_) => "exited",
        Status::Failed(_) => "failed",
    }
}

impl App {
    /// The answer to `query` from the terminal numbered `caller` (its `JED_TERMINAL`). `Open`
    /// changes state, so `ipc_request` handles it before this.
    pub(super) fn claude_answer(&self, query: &Query, caller: Option<u64>) -> Reply {
        match query {
            Query::Context => Reply::data(self.claude_context()),
            Query::Editors => Reply::data(self.claude_editors()),
            Query::Diagnostics { path } => self.claude_diagnostics(path.as_deref()),
            Query::Terminals => Reply::data(self.claude_terminals(caller)),
            Query::Commands { terminal, n } => self.claude_commands(*terminal, *n, caller),
            Query::Output {
                terminal,
                command,
                max_lines,
            } => self.claude_output(*terminal, *command, *max_lines),
            Query::Screen { terminal } => self.claude_screen(*terminal),
            Query::Environment => Reply::data(self.claude_environment()),
            Query::Open { .. } => Reply::error("open is not a read"),
        }
    }

    /// The active group's text tab, else the first group's that has one (a shell moved into the
    /// editor area can be the active tab while `claude` runs in it).
    fn claude_editor(&self) -> Option<(&TextTab, &Buffer)> {
        let tab = self.groups.active_group().active_tab().or_else(|| {
            self.groups
                .ids()
                .into_iter()
                .find_map(|id| self.groups.get(id)?.active_tab())
        })?;
        Some((tab, self.buffers.get(tab.buffer)?))
    }

    /// `{line, col}` of `at` in `buf`.
    fn claude_place(&self, buf: &Buffer, at: pos::ByteIdx) -> Value {
        let tab_width = self.settings.for_buffer(buf).tab_width;
        let line = pos::line_of(&buf.rope, at);
        json!({"line": line + 1, "col": pos::display_col(&buf.rope, at, tab_width).0 + 1})
    }

    fn claude_range(&self, buf: &Buffer, sel: &Selection) -> Value {
        let (lo, hi) = (sel.anchor.min(sel.head), sel.anchor.max(sel.head));
        json!({"start": self.claude_place(buf, lo), "end": self.claude_place(buf, hi), "head": self.claude_place(buf, sel.head)})
    }

    fn claude_context(&self) -> Value {
        let Some((tab, buf)) = self.claude_editor() else {
            return json!({"editor": null});
        };
        let sels = &tab.view.selections;
        let primary = sels.primary();
        let (lo, hi) = (primary.anchor.min(primary.head), primary.anchor.max(primary.head));
        let end = pos::ByteIdx(
            buf.rope
                .floor_char_boundary(hi.0.min(lo.0.saturating_add(MAX_SELECTION))),
        );
        let text = buf.rope.slice(lo.0..end.0).to_string();
        let ranges: Vec<Value> = sels
            .iter()
            .take(MAX_RANGES)
            .map(|s| self.claude_range(buf, s))
            .collect();
        json!({"editor": {
            "path": buf.has_file().then(|| buf.path.display().to_string()),
            "relative": buf.has_file().then(|| path_text(&self.workspace.root, &buf.path)),
            "name": buf.name(),
            "language": buf.syntax.language().name(),
            "dirty": buf.is_dirty(),
            "read_only": buf.read_only.is_some(),
            "cursor": self.claude_place(buf, primary.head),
            "selection": {
                "empty": lo == hi,
                "text": text,
                "truncated": end < hi,
            },
            "selection_count": sels.len(),
            "ranges": ranges,
        }})
    }

    fn claude_tab(&self, tab: &Tab, active: bool) -> Value {
        let mut v = match tab {
            Tab::Text(t) => {
                let buf = self.buffers.get(t.buffer);
                json!({
                    "kind": "text",
                    "name": buf.map(Buffer::name),
                    "path": buf.filter(|b| b.has_file()).map(|b| b.path.display().to_string()),
                    "dirty": buf.is_some_and(Buffer::is_dirty),
                })
            }
            Tab::Diff(d) => json!({"kind": "diff", "path": d.path.display().to_string()}),
            Tab::Hex(h) => json!({"kind": "hex", "path": h.path.display().to_string()}),
            Tab::Markdown(m) => json!({
                "kind": "markdown",
                "name": m.name,
                "path": m.path.as_ref().map(|p| p.display().to_string()),
            }),
            Tab::Shell(s) => json!({"kind": "shell", "terminal": s.run.id.0, "name": s.run.name}),
        };
        v["active"] = json!(active);
        v
    }

    fn claude_editors(&self) -> Value {
        let mut listed = 0;
        let mut groups = Vec::new();
        for (area, set) in [("editor", &self.groups), ("terminal", &self.terminals)] {
            for id in set.ids() {
                let Some(group) = set.get(id) else { continue };
                let tabs: Vec<Value> = group
                    .tabs
                    .iter()
                    .enumerate()
                    .take(MAX_TABS.saturating_sub(listed))
                    .map(|(i, t)| self.claude_tab(t, i == group.active))
                    .collect();
                listed += tabs.len();
                groups.push(json!({
                    "group": id,
                    "area": area,
                    "active": id == set.active,
                    "tabs": tabs,
                    "tab_count": group.tabs.len(),
                }));
            }
        }
        json!({"groups": groups, "truncated": listed >= MAX_TABS})
    }

    fn claude_diagnostics(&self, path: Option<&Path>) -> Reply {
        let mut files: Vec<&Buffer> = match path {
            Some(path) => {
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.workspace.root.join(path)
                };
                match self.buffers.find_path(&path).and_then(|id| self.buffers.get(id)) {
                    Some(buf) => vec![buf],
                    None => return Reply::error(format!("{} is not open", path.display())),
                }
            }
            None => self
                .buffers
                .iter()
                .map(|(_, b)| b)
                .filter(|b| b.has_file() && !b.diagnostics.is_empty())
                .collect(),
        };
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let total: usize = files.iter().map(|b| b.diagnostics.all().len()).sum();
        let mut items = Vec::new();
        // Errors first, so the cap drops hints before errors.
        'fill: for severity in Severity::ALL {
            for buf in &files {
                for mark in buf.diagnostics.all().iter().filter(|m| m.diag.severity == severity) {
                    if items.len() == MAX_DIAGNOSTICS {
                        break 'fill;
                    }
                    let (message, _) = clip_head(&mark.diag.message, MAX_MESSAGE);
                    let (start, end) = (self.claude_place(buf, mark.start), self.claude_place(buf, mark.end));
                    items.push(json!({
                        "path": buf.path.display().to_string(),
                        "severity": severity_name(severity),
                        "line": start["line"],
                        "col": start["col"],
                        "end_line": end["line"],
                        "end_col": end["col"],
                        "message": message,
                        "source": mark.diag.source,
                        "code": mark.diag.code,
                    }));
                }
            }
        }
        Reply::data(json!({"count": total, "diagnostics": items, "truncated": total > items.len()}))
    }

    /// Every shell tab's run (editor groups, then the terminal area's), then every task run.
    fn claude_runs(&self) -> Vec<(&Run, RunKind)> {
        let shells = self
            .groups
            .shell_tabs()
            .chain(self.terminals.shell_tabs())
            .map(|t| (&t.run, RunKind::Shell));
        let mut runs: Vec<_> = shells.collect();
        runs.sort_by_key(|(r, _)| r.id);
        runs.extend(self.tasks.runs().iter().map(|r| (r, RunKind::Task)));
        runs
    }

    fn claude_run(&self, id: u64) -> Option<&Run> {
        self.claude_runs()
            .into_iter()
            .map(|(r, _)| r)
            .find(|r| r.id == RunId(id))
    }

    fn claude_terminals(&self, caller: Option<u64>) -> Value {
        let idle = |id: RunId| {
            self.groups
                .shell_tabs()
                .chain(self.terminals.shell_tabs())
                .find(|t| t.run.id == id)
                .map(|t| t.idle)
        };
        let list: Vec<Value> = self
            .claude_runs()
            .into_iter()
            .map(|(run, kind)| {
                let status = run.status();
                let commands = run.commands();
                let last = commands.iter().rev().find(|c| c.finished.is_some());
                json!({
                    "id": run.id.0,
                    "kind": kind.name(),
                    "name": run.name,
                    "cwd": run.cwd.display().to_string(),
                    "state": state_name(&status),
                    "exit": match &status { Status::Exited(e) => Some(e.code), _ => None },
                    "idle": if kind == RunKind::Shell { idle(run.id) } else { None },
                    "self": caller == Some(run.id.0),
                    "last_command": last.map(|c| json!({
                        "id": c.id,
                        "command": c.command.as_deref().map(|s| clip_head(s, MAX_COMMAND_LINE).0),
                        "exit": c.exit,
                    })),
                })
            })
            .collect();
        json!({"terminals": list})
    }

    fn claude_commands(&self, terminal: Option<u64>, n: Option<usize>, caller: Option<u64>) -> Reply {
        let n = n.unwrap_or(10).clamp(1, MAX_COMMANDS);
        let runs: Vec<&Run> = match terminal {
            Some(id) => match self.claude_run(id) {
                Some(run) => vec![run],
                None => return Reply::error(format!("no terminal {id}")),
            },
            None => self
                .claude_runs()
                .into_iter()
                .filter(|(r, kind)| *kind == RunKind::Shell && caller != Some(r.id.0))
                .map(|(r, _)| r)
                .take(MAX_TERMINALS)
                .collect(),
        };
        let list: Vec<Value> = runs
            .into_iter()
            .map(|run| {
                let commands: Vec<Value> = run
                    .commands()
                    .iter()
                    .rev()
                    .take(n)
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "command": c.command.as_deref().map(|s| clip_head(s, MAX_COMMAND_LINE).0),
                            "cwd": c.cwd.as_ref().map(|p| p.display().to_string()),
                            "running": c.finished.is_none(),
                            "exit": c.exit,
                            "seconds_ago": c.started.elapsed().as_secs(),
                            "seconds": c.finished.map(|f| f.duration_since(c.started).as_secs_f64()),
                        })
                    })
                    .collect();
                json!({"terminal": run.id.0, "name": run.name, "commands": commands})
            })
            .collect();
        Reply::data(json!({"terminals": list}))
    }

    fn claude_output(&self, terminal: u64, command: u64, max_lines: Option<usize>) -> Reply {
        let Some(run) = self.claude_run(terminal) else {
            return Reply::error(format!("no terminal {terminal}"));
        };
        let lines = max_lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);
        let Some(out) = run.command_output(command, lines) else {
            return Reply::error(format!(
                "terminal {terminal} has no command {command} (only the last 50 are kept)"
            ));
        };
        let (text, cut) = clip_tail(&out.text, MAX_OUTPUT);
        Reply::data(json!({
            "terminal": terminal,
            "command": command,
            "text": text,
            "lines": text.lines().count(),
            "truncated": out.truncated || cut,
            "running": out.running,
        }))
    }

    fn claude_screen(&self, terminal: u64) -> Reply {
        let Some(run) = self.claude_run(terminal) else {
            return Reply::error(format!("no terminal {terminal}"));
        };
        let (contents, rows, cols) = {
            let parser = run.screen();
            let screen = parser.screen();
            let (rows, cols) = screen.size();
            (screen.contents(), rows.get(), cols.get())
        };
        let (text, truncated) = clip_head(contents.trim_end(), MAX_SCREEN);
        Reply::data(json!({"terminal": terminal, "rows": rows, "cols": cols, "text": text, "truncated": truncated}))
    }

    fn claude_environment(&self) -> Value {
        let git = self.git.status.as_ref().map(|s| {
            json!({
                "branch": s.head.name(),
                "dirty": s.entries.len(),
                "loaded": s.loaded,
            })
        });
        let open_files: Vec<String> = {
            let mut files: Vec<String> = self
                .buffers
                .iter()
                .filter(|(_, b)| b.has_file())
                .map(|(_, b)| path_text(&self.workspace.root, &b.path))
                .collect();
            files.sort();
            files.truncate(MAX_LIST);
            files
        };
        let servers: Vec<Value> = self
            .lsp
            .servers()
            .iter()
            .map(|s| {
                let state = match s.state {
                    crate::lsp::State::Starting => "starting",
                    crate::lsp::State::Running { .. } => "running",
                    crate::lsp::State::Failed => "failed",
                };
                json!({
                    "name": s.config.name,
                    "state": state,
                    "languages": s.languages.iter().map(|l| l.name()).collect::<Vec<_>>(),
                })
            })
            .collect();
        let templates: Vec<String> = self
            .task_templates()
            .into_iter()
            .map(|t| t.name)
            .take(MAX_LIST)
            .collect();
        // Names only: the values may be secrets.
        let variables: Vec<String> = self.environment_vars().keys().take(MAX_LIST).cloned().collect();
        let shells: Vec<Value> = self
            .claude_runs()
            .into_iter()
            .filter(|(_, kind)| *kind == RunKind::Shell)
            .map(|(r, _)| json!({"id": r.id.0, "name": r.name, "cwd": r.cwd.display().to_string()}))
            .collect();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "workspace": [self.workspace.root.display().to_string()],
            "git": git,
            "open_files": open_files,
            "servers": servers,
            "task_templates": templates,
            "task_environment": {"name": self.environment, "variables": variables},
            "shells": shells,
        })
    }
}
