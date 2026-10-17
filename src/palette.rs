//! Command palette, quick open, go to line and small pickers: one input over a list of rows.
//! Fuzzy matching through `nucleo`. No drawing here.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Nucleo, Utf32Str, Utf32String};

use crate::command::Command;
use crate::field::TextField;
use crate::history::{self, Entry, Snapshot};
use crate::logs::Tool;
use crate::recent;
use crate::tasks::{Source, Template};

/// Quick open shows at most this many files (the best matches).
pub const MAX_FILE_ROWS: u32 = 200;

/// What the palette was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picker {
    /// Commands (`>`), files (no prefix) or go to line (`:`), switched by the input's first character, as VS Code.
    QuickOpen,
    /// "Set Log Level": a level, or a typed filter.
    LogLevel,
    /// "File History: Show" / "Restore": an entry of the active file's history (the entries are
    /// the app's: `app.file_history.picker`). Accepting compares, or with `restore` restores.
    History { restore: bool },
    /// "Run Task": a task template (`App::task_templates`); accepting runs it.
    Tasks,
    /// "Open Recent Workspace": a recent workspace (the entries are the app's:
    /// `app.recent.picker`); accepting opens it in another jed (`recent::how_to_open`).
    Recent,
    /// "Tools: Open Log": a tool with a log (`App::log_tools`); accepting opens its log tab.
    Logs,
}

/// What the rows are, from the picker and the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Commands,
    Files,
    GoToLine,
    LogLevel,
    History,
    Tasks,
    Recent,
    Logs,
}

pub const COMMANDS_PREFIX: char = '>';
pub const LINE_PREFIX: char = ':';

/// What accepting a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Run(Command),
    /// Workspace-relative path.
    Open(PathBuf),
    SetLogLevel(&'static str),
    /// A file history entry.
    History(Snapshot),
    /// A task template, by name.
    RunTask(String),
    /// A recent workspace's root.
    OpenWorkspace(PathBuf),
    /// A tool's log.
    OpenLog(Tool),
}

/// One line in the list. Hits are char indices to highlight.
#[derive(Debug, Clone)]
pub struct Row {
    pub label: String,
    pub label_hits: Vec<usize>,
    /// Dimmed after the label (a file's folder, a level's meaning).
    pub detail: String,
    pub detail_hits: Vec<usize>,
    /// Right-aligned key binding.
    pub keys: Option<String>,
    pub action: Action,
}

pub struct Palette {
    pub picker: Picker,
    pub field: TextField,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// List rows visible in the last render, for paging.
    pub height: usize,
    /// Shown above the rows: go to line help, "no matches", errors.
    pub hint: Option<String>,
}

pub const LOG_LEVELS: [(&str, &str); 6] = [
    ("trace", "everything, incl. every key press (input)"),
    ("debug", "commands and their timing"),
    ("info", "the default"),
    ("warn", "slow frames, recoverable problems"),
    ("error", "failures only"),
    ("off", "nothing"),
];

impl Palette {
    pub fn new(picker: Picker, text: &str) -> Self {
        Self {
            picker,
            field: TextField::new(text.to_string()),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            height: 1,
            hint: None,
        }
    }

    pub fn mode(&self) -> Mode {
        match self.picker {
            Picker::LogLevel => Mode::LogLevel,
            Picker::History { .. } => Mode::History,
            Picker::Tasks => Mode::Tasks,
            Picker::Recent => Mode::Recent,
            Picker::Logs => Mode::Logs,
            Picker::QuickOpen => match self.field.text.chars().next() {
                Some(COMMANDS_PREFIX) => Mode::Commands,
                Some(LINE_PREFIX) => Mode::GoToLine,
                _ => Mode::Files,
            },
        }
    }

    /// The input without its mode prefix.
    pub fn query(&self) -> &str {
        match self.mode() {
            Mode::Commands | Mode::GoToLine => &self.field.text[1..],
            Mode::Files | Mode::LogLevel | Mode::History | Mode::Tasks | Mode::Recent | Mode::Logs => &self.field.text,
        }
    }

    /// New rows; the selection goes back to the top.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Moves the selection by `delta`, clamped, keeping it in view.
    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.select(self.selected.saturating_add_signed(delta).min(self.rows.len() - 1));
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
        let height = self.height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.rows.len().saturating_sub(self.height.max(1));
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }
}

fn pattern(query: &str) -> Pattern {
    Pattern::parse(query, CaseMatching::Smart, Normalization::Smart)
}

/// Rows for the commands matching `query` (by title or by name), best first; all of them,
/// alphabetically, for an empty query. `keys` gives each command's binding label.
pub fn command_rows(query: &str, commands: &[Command], keys: impl Fn(Command) -> Option<String>) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, Row)> = commands
        .iter()
        .filter_map(|&c| {
            let title = c.title();
            let mut hits = Vec::new();
            let by_title = pattern.indices(Utf32Str::new(&title, &mut buf), &mut matcher, &mut hits);
            let score = match by_title {
                Some(s) => s,
                None => {
                    hits.clear();
                    pattern.score(Utf32Str::new(&c.name(), &mut buf), &mut matcher)?
                }
            };
            Some((
                score,
                Row {
                    label: title,
                    label_hits: sorted(hits),
                    detail: String::new(),
                    detail_hits: Vec::new(),
                    keys: keys(c),
                    action: Action::Run(c),
                },
            ))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then_with(|| a.label.cmp(&b.label)));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// Rows for "Set Log Level", `current` marked.
pub fn log_level_rows(query: &str, current: Option<&str>) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    LOG_LEVELS
        .iter()
        .filter_map(|&(level, about)| {
            let mut hits = Vec::new();
            pattern.indices(Utf32Str::new(level, &mut buf), &mut matcher, &mut hits)?;
            let detail = if current == Some(level) {
                format!("{about} (current)")
            } else {
                about.to_string()
            };
            Some(Row {
                label: level.to_string(),
                label_hits: sorted(hits),
                detail,
                detail_hits: Vec::new(),
                keys: None,
                action: Action::SetLogLevel(level),
            })
        })
        .collect()
}

/// Rows for a file's history `entries` (newest first, kept in that order) matching `query`:
/// "3 min ago · 12 KB", the time (UTC) dimmed after it. `now` in ms since the epoch.
pub fn history_rows(query: &str, entries: &[Entry], now: u64) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    entries
        .iter()
        .filter_map(|e| {
            let label = format!("{} · {}", history::ago(e.time, now), history::size_label(e.size));
            let detail = history::utc_label(e.time);
            let (mut label_hits, mut detail_hits) = (Vec::new(), Vec::new());
            if !query.is_empty() {
                let by_label = pattern.indices(Utf32Str::new(&label, &mut buf), &mut matcher, &mut label_hits);
                if by_label.is_none() {
                    label_hits.clear();
                    pattern.indices(Utf32Str::new(&detail, &mut buf), &mut matcher, &mut detail_hits)?;
                }
            }
            Some(Row {
                label,
                label_hits: sorted(label_hits),
                detail,
                detail_hits: sorted(detail_hits),
                keys: None,
                action: Action::History(e.snapshot()),
            })
        })
        .collect()
}

/// Rows for "Run Task": each template matching `query` by name (else by command), best first
/// (all, by name, for an empty query): the name, the command dimmed, where it's defined at the
/// right (`.workspace` / `tasks.toml`).
pub fn task_rows(query: &str, templates: &[Template]) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, Row)> = templates
        .iter()
        .filter_map(|t| {
            let (mut label_hits, mut detail_hits) = (Vec::new(), Vec::new());
            let detail = t.task.command.replace(['\n', '\r', '\t'], " ");
            let score = match pattern.indices(Utf32Str::new(&t.name, &mut buf), &mut matcher, &mut label_hits) {
                Some(s) => s,
                None => {
                    label_hits.clear();
                    pattern.indices(Utf32Str::new(&detail, &mut buf), &mut matcher, &mut detail_hits)?
                }
            };
            let source = match t.source {
                Source::Workspace => crate::workspace::FILE_NAME,
                Source::User => crate::tasks::templates::FILE_NAME,
            };
            Some((
                score,
                Row {
                    label: t.name.clone(),
                    label_hits: sorted(label_hits),
                    detail,
                    detail_hits: sorted(detail_hits),
                    keys: Some(source.to_string()),
                    action: Action::RunTask(t.name.clone()),
                },
            ))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then_with(|| a.label.cmp(&b.label)));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// Rows for "Tools: Open Log": the `tools` (in their order) whose name matches `query`, best
/// first, each with what it is dimmed after it.
pub fn log_rows(query: &str, tools: &[Tool]) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, Row)> = tools
        .iter()
        .filter_map(|tool| {
            let mut hits = Vec::new();
            let score = pattern.indices(Utf32Str::new(tool.name(), &mut buf), &mut matcher, &mut hits)?;
            let detail = match tool {
                Tool::Jed => "jed's own log",
                Tool::Server(_) => "language server",
            };
            Some((
                score,
                Row {
                    label: tool.name().to_string(),
                    label_hits: sorted(hits),
                    detail: detail.into(),
                    detail_hits: Vec::new(),
                    keys: None,
                    action: Action::OpenLog(tool.clone()),
                },
            ))
        })
        .collect();
    // Stable: equal scores (an empty query) keep the tools' order.
    scored.sort_by(|(a, _), (b, _)| b.cmp(a));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// Rows for "Open Recent Workspace": each entry (most recent first) matching `query` by folder
/// name, else by path, best first (ties keep the recency order): the folder name, its path with
/// `~` for `home` dimmed, and "open" at the right for one already open in a terminal window.
pub fn recent_rows(query: &str, entries: &[recent::Entry], home: &std::path::Path) -> Vec<Row> {
    let pattern = pattern(query);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, Row)> = entries
        .iter()
        .filter_map(|e| {
            let label = recent::folder_name(&e.path);
            let detail = recent::display(&e.path, home);
            let (mut label_hits, mut detail_hits) = (Vec::new(), Vec::new());
            let score = match pattern.indices(Utf32Str::new(&label, &mut buf), &mut matcher, &mut label_hits) {
                Some(s) => s,
                None => {
                    label_hits.clear();
                    pattern.indices(Utf32Str::new(&detail, &mut buf), &mut matcher, &mut detail_hits)?
                }
            };
            Some((
                score,
                Row {
                    label,
                    label_hits: sorted(label_hits),
                    detail,
                    detail_hits: sorted(detail_hits),
                    keys: e.window.map(|_| "open".to_string()),
                    action: Action::OpenWorkspace(e.path.clone()),
                },
            ))
        })
        .collect();
    // Stable: equal scores (all of them for an empty query) stay most recent first.
    scored.sort_by(|(sa, _), (sb, _)| sb.cmp(sa));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// A go to line query: `12`, `12:5` or `12,5` (1-based). `None` if it isn't one.
pub fn parse_line(query: &str) -> Option<(usize, Option<usize>)> {
    let query = query.trim();
    let num = |s: &str| s.trim().parse::<usize>().ok().filter(|n| *n > 0);
    match query.split_once([':', ',']) {
        Some((line, col)) => Some((num(line)?, Some(num(col)?))),
        None => Some((num(query)?, None)),
    }
}

/// The file part of a quick open query: drops a `:line[:col]` suffix, also while it's being
/// typed (`main.rs:`, `main.rs:12:`), so the matches don't empty and come back on each `:`.
pub fn file_query(query: &str) -> &str {
    let mut name = query;
    for _ in 0..2 {
        match name.rsplit_once(':') {
            Some((rest, n)) if !rest.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => name = rest,
            _ => break,
        }
    }
    name
}

fn sorted(mut hits: Vec<u32>) -> Vec<usize> {
    hits.sort_unstable();
    hits.dedup();
    hits.into_iter().map(|i| i as usize).collect()
}

/// Quick open's file list: the walk pushes paths in from a blocking task; `nucleo` matches
/// on its own threads. `notify` (given at creation) is called when there's something new to `tick`.
pub struct FileMatcher {
    nucleo: Nucleo<PathBuf>,
    matcher: Matcher,
    query: String,
    /// Set to stop the walk feeding the current list.
    pub cancel: Arc<AtomicBool>,
    /// The walk for the current list has finished.
    pub walked: bool,
    /// Counts walks, so a finished old walk isn't taken for the current one.
    pub generation: u64,
}

impl FileMatcher {
    pub fn new(notify: Arc<dyn Fn() + Sync + Send>) -> Self {
        Self {
            nucleo: Nucleo::new(Config::DEFAULT.match_paths(), notify, None, 1),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            query: String::new(),
            cancel: Arc::new(AtomicBool::new(true)),
            walked: false,
            generation: 0,
        }
    }

    /// Empties the list (stopping any walk) and returns what the next walk should push into.
    pub fn restart(&mut self) -> (FileInjector, Arc<AtomicBool>) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.nucleo.restart(true);
        self.walked = false;
        self.generation += 1;
        (FileInjector(self.nucleo.injector()), self.cancel.clone())
    }

    /// Stops the walk and drops the list (until the next `restart`).
    pub fn stop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.nucleo.restart(true);
        self.walked = false;
    }

    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        let append = query.starts_with(&self.query);
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.query = query.to_string();
    }

    /// Takes in finished matching without waiting. True while matching is still running.
    pub fn tick(&mut self) -> bool {
        self.nucleo.tick(0).running
    }

    /// Files matched so far, of how many walked.
    pub fn counts(&self) -> (u32, u32) {
        let s = self.nucleo.snapshot();
        (s.matched_item_count(), s.item_count())
    }

    /// The best matches as rows: file name, then its folder.
    pub fn rows(&mut self) -> Vec<Row> {
        let snapshot = self.nucleo.snapshot();
        let pattern = snapshot.pattern().column_pattern(0);
        let n = snapshot.matched_item_count().min(MAX_FILE_ROWS);
        let mut hits = Vec::new();
        snapshot
            .matched_items(0..n)
            .map(|item| {
                let rel = item.data;
                hits.clear();
                pattern.indices(item.matcher_columns[0].slice(..), &mut self.matcher, &mut hits);
                let hits = sorted(std::mem::take(&mut hits));
                let label = rel
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let detail = rel
                    .parent()
                    .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
                // The haystack is `detail/label`: split the hits between them.
                let label_start = item.matcher_columns[0].len() - label.chars().count();
                Row {
                    label_hits: hits
                        .iter()
                        .filter(|&&i| i >= label_start)
                        .map(|i| i - label_start)
                        .collect(),
                    detail_hits: hits.iter().copied().filter(|&i| i < detail.chars().count()).collect(),
                    label,
                    detail,
                    keys: None,
                    action: Action::Open(rel.clone()),
                }
            })
            .collect()
    }
}

/// Where a walk pushes the files it finds.
pub struct FileInjector(nucleo::Injector<PathBuf>);

impl FileInjector {
    pub fn push(&self, rel: PathBuf) {
        self.0.push(rel, |p, cols| {
            cols[0] = Utf32String::from(p.to_string_lossy().as_ref());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Motion;

    #[test]
    fn prefixes_pick_the_mode() {
        let mut p = Palette::new(Picker::QuickOpen, ">sav");
        assert_eq!((p.mode(), p.query()), (Mode::Commands, "sav"));
        p.field = TextField::new(":12".into());
        assert_eq!((p.mode(), p.query()), (Mode::GoToLine, "12"));
        p.field = TextField::new("main.rs".into());
        assert_eq!((p.mode(), p.query()), (Mode::Files, "main.rs"));
        assert_eq!(Palette::new(Picker::LogLevel, ">x").mode(), Mode::LogLevel);
    }

    #[test]
    fn task_rows_show_name_command_and_source() {
        let t = |name: &str, command: &str, source| Template {
            name: name.into(),
            task: crate::tasks::Task {
                command: command.into(),
                cwd: None,
                env: Default::default(),
            },
            source,
        };
        let templates = [
            t("build", "cargo build", Source::Workspace),
            t("test", "cargo test\n--all", Source::User),
        ];
        let rows = task_rows("", &templates);
        let shown: Vec<_> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.detail.as_str(), r.keys.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("build", "cargo build", Some(".workspace")),
                ("test", "cargo test --all", Some("tasks.toml"))
            ]
        );
        assert_eq!(rows[0].action, Action::RunTask("build".into()));
        let rows = task_rows("tst", &templates);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].label.as_str(), rows[0].label_hits.as_slice()),
            ("test", &[0, 2, 3][..])
        );
        // By command when the name doesn't match.
        let rows = task_rows("--all", &templates);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].label_hits.is_empty() && !rows[0].detail_hits.is_empty());
        assert_eq!(Palette::new(Picker::Tasks, ">x").mode(), Mode::Tasks);
    }

    #[test]
    fn log_rows_list_tools_in_order_and_filter_by_name() {
        let tools = [
            Tool::Jed,
            Tool::Server("clangd".into()),
            Tool::Server("rust-analyzer".into()),
        ];
        let rows = log_rows("", &tools);
        let shown: Vec<_> = rows.iter().map(|r| (r.label.as_str(), r.detail.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("jed", "jed's own log"),
                ("clangd", "language server"),
                ("rust-analyzer", "language server")
            ]
        );
        let rows = log_rows("rust", &tools);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, Action::OpenLog(tools[2].clone()));
        assert_eq!(rows[0].label_hits, [0, 1, 2, 3]);
    }

    #[test]
    fn recent_rows_show_name_and_tilde_path_in_recency_order() {
        let home = std::path::Path::new("/home/me");
        let mut entries = vec![
            recent::Entry::new("/home/me/src/jed".into()),
            recent::Entry::new("/srv/www".into()),
            recent::Entry::new("/home/me/notes".into()),
        ];
        entries[1].window = Some(7);
        let rows = recent_rows("", &entries, home);
        let shown: Vec<_> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.detail.as_str(), r.keys.as_deref()))
            .collect();
        assert_eq!(
            shown,
            [
                ("jed", "~/src/jed", None),
                ("www", "/srv/www", Some("open")),
                ("notes", "~/notes", None)
            ]
        );
        assert_eq!(rows[2].action, Action::OpenWorkspace("/home/me/notes".into()));
        let rows = recent_rows("nts", &entries, home);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].label.as_str(), rows[0].label_hits.as_slice()),
            ("notes", &[0, 2, 4][..])
        );
        // By path when the name doesn't match.
        let rows = recent_rows("srv", &entries, home);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].label_hits.is_empty() && rows[0].detail_hits == [1, 2, 3]);
        assert_eq!(Palette::new(Picker::Recent, ">x").mode(), Mode::Recent);
        assert_eq!(Palette::new(Picker::Logs, ">x").mode(), Mode::Logs);
    }

    #[test]
    fn commands_match_titles_and_names() {
        let cmds = [
            Command::Save,
            Command::SplitRight,
            Command::Move {
                motion: Motion::Left,
                select: false,
            },
        ];
        let rows = command_rows("save", &cmds, |_| None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "File: Save");
        assert_eq!(rows[0].label_hits, vec![6, 7, 8, 9]);
        let rows = command_rows("view.splitR", &cmds, |_| Some("K".into()));
        assert_eq!(rows[0].action, Action::Run(Command::SplitRight), "matched by name");
        assert_eq!(rows[0].keys.as_deref(), Some("K"));
        let all = command_rows("", &cmds, |_| None);
        let labels: Vec<&str> = all.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Cursor: Move Left", "File: Save", "View: Split Editor Right"]);
    }

    #[test]
    fn line_queries() {
        assert_eq!(parse_line("12"), Some((12, None)));
        assert_eq!(parse_line(" 12:5 "), Some((12, Some(5))));
        assert_eq!(parse_line("12,5"), Some((12, Some(5))));
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("0"), None);
        assert_eq!(parse_line("x"), None);
        assert_eq!(parse_line("3:"), None);
    }

    #[test]
    fn file_queries_drop_the_position() {
        assert_eq!(file_query("main.rs"), "main.rs");
        assert_eq!(file_query("main.rs:"), "main.rs");
        assert_eq!(file_query("main.rs:12"), "main.rs");
        assert_eq!(file_query("main.rs:12:"), "main.rs");
        assert_eq!(file_query("main.rs:12:5"), "main.rs");
        assert_eq!(file_query("a:b.rs:3"), "a:b.rs");
        assert_eq!(file_query("a:b.rs"), "a:b.rs");
        assert_eq!(file_query(":12"), ":12");
    }

    #[test]
    fn selection_scrolls_and_clamps() {
        let mut p = Palette::new(Picker::LogLevel, "");
        p.set_rows(log_level_rows("", Some("info")));
        assert_eq!(p.rows.len(), 6);
        assert!(p.rows[2].detail.ends_with("(current)"));
        p.height = 2;
        p.move_selection(3);
        assert_eq!((p.selected, p.scroll), (3, 2));
        p.move_selection(10);
        assert_eq!(p.selected, 5);
        p.move_selection(-5);
        assert_eq!((p.selected, p.scroll), (0, 0));
        assert_eq!(log_level_rows("dbg", None)[0].label, "debug");
    }

    #[test]
    fn history_rows_keep_the_order_and_filter() {
        let entry = |time: u64, size: u64, hash: &str| Entry {
            id: format!("{time}"),
            time,
            size,
            hash: hash.repeat(64),
        };
        let now = 1_792_245_789_000;
        let entries = [entry(now - 180_000, 12 * 1024, "a"), entry(now - 7_200_000, 300, "b")];
        let rows = history_rows("", &entries, now);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["3 min ago · 12 KB", "2 hr ago · 300 B"]);
        assert_eq!(rows[0].detail, "2026-10-17 14:00:09 UTC");
        assert_eq!(rows[1].action, Action::History(entries[1].snapshot()));
        let rows = history_rows("300", &entries, now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label_hits, [11, 12, 13]);
        // The time matches too.
        let rows = history_rows("12:03", &entries, now);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].detail_hits.is_empty());
    }
}
