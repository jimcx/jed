//! The keyboard cheat sheet's model ("Help: Keyboard Cheat Sheet", Shift+F1): the navigation
//! keys in fixed groups of commands, built from the live `Keymap` (so `keybindings.toml`
//! overrides show and unbound commands are left out), with the context it was opened from first;
//! laid out in columns by `columns`. `app/cheatsheet.rs` opens and drives it, `ui/cheatsheet.rs`
//! draws it.

use unicode_width::UnicodeWidthStr;

use crate::command::{ColumnMotion, Command, FindCommand, Motion, ProblemsCommand, SearchCommand};
use crate::config::keymap::{Context, Keymap};
use crate::scrollbar::{Scroll, Scrollable};

/// Columns between a row's key and its title.
pub const KEY_GAP: usize = 2;
/// Columns between two columns of sections.
pub const COLUMN_GAP: usize = 3;

/// One titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
}

/// A key label (the platform's, e.g. `Cmd+Shift+P`) and the command's title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub title: String,
}

/// A drawn line of a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line<'a> {
    Header(&'a str),
    /// A row, its title after `key_width` (its section's widest key) and `KEY_GAP`.
    Row {
        row: &'a Row,
        key_width: usize,
    },
    Blank,
}

/// The open overlay: where it opened and its scroll (`height` and `rows` are set while drawing).
#[derive(Debug, Clone)]
pub struct CheatSheet {
    /// The keymap context it was opened from: its keys are the first section.
    pub context: Context,
    pub top: usize,
    /// Rows on screen, and the tallest column's rows (render-time state).
    pub height: usize,
    pub rows: usize,
}

impl CheatSheet {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            top: 0,
            height: 0,
            rows: 0,
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.set_top(self.top.saturating_add_signed(delta));
    }
}

impl Scrollable for CheatSheet {
    fn scroll(&self) -> Scroll {
        Scroll::rows(self.rows, self.height, self.top)
    }

    fn set_top(&mut self, top: usize) {
        self.top = top.min(self.rows.saturating_sub(self.height.max(1)));
    }
}

/// The fixed groups: a title and its commands, in order.
fn groups() -> Vec<(&'static str, Vec<Command>)> {
    let mv = |motion, select| Command::Move { motion, select };
    let motions = [
        Motion::Left,
        Motion::Right,
        Motion::Up,
        Motion::Down,
        Motion::WordLeft,
        Motion::WordRight,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::PageUp,
        Motion::PageDown,
        Motion::DocStart,
        Motion::DocEnd,
    ];
    let mut selection: Vec<Command> = motions.iter().map(|m| mv(*m, true)).collect();
    selection.extend([
        Command::SelectAll,
        Command::ColumnSelect(ColumnMotion::Up),
        Command::ColumnSelect(ColumnMotion::Down),
        Command::ColumnSelect(ColumnMotion::Left),
        Command::ColumnSelect(ColumnMotion::Right),
        Command::AddCursorAbove,
        Command::AddCursorBelow,
        Command::CursorsAtLineEnds,
        Command::AddNextMatch,
        Command::SkipToNextMatch,
        Command::SelectAllMatches,
        Command::CursorUndo,
        Command::CancelSelection,
    ]);
    vec![
        ("Cursor Movement", motions.iter().map(|m| mv(*m, false)).collect()),
        ("Selection", selection),
        (
            "Going Places",
            vec![
                Command::QuickOpen,
                Command::GoToLine,
                Command::ShowCommands,
                Command::NavBack,
                Command::NavForward,
                Command::RevealDefinition,
                Command::GoToReferences,
                Command::Problems(ProblemsCommand::Next),
                Command::Problems(ProblemsCommand::Previous),
                Command::NextChange,
                Command::PreviousChange,
                Command::Find(FindCommand::Next),
                Command::Find(FindCommand::Previous),
                Command::Search(SearchCommand::NextResult),
                Command::Search(SearchCommand::PreviousResult),
            ],
        ),
        (
            "Tabs and Groups",
            vec![
                Command::NextTab,
                Command::PrevTab,
                Command::CloseTab,
                Command::FocusGroup(1),
                Command::FocusGroup(2),
                Command::FocusGroup(3),
                Command::SplitRight,
                Command::SplitDown,
                Command::MoveTabToNextGroup,
                Command::MoveTabToPreviousGroup,
                Command::MoveTabLeft,
                Command::MoveTabRight,
            ],
        ),
        (
            "Panels",
            vec![
                Command::ToggleSidebar,
                Command::FocusExplorer,
                Command::FindInFiles,
                Command::FocusScm,
                Command::TogglePanel,
                Command::ToggleProblems,
                Command::ShowTools,
                Command::CheatSheet,
            ],
        ),
    ]
}

/// The section heading for a context's own keys; `None` for the editor (the groups are its).
fn context_title(context: Context) -> Option<&'static str> {
    (context != Context::Editor).then(|| context.title())
}

/// The sections to show: `context`'s own keys first (and its parents' short of the editor's:
/// Replace shows Find's too), then the fixed groups; commands with no key are left out.
pub fn sections(keymap: &Keymap, context: Context) -> Vec<Section> {
    let mut out = Vec::new();
    if let Some(title) = context_title(context) {
        let mut rows: Vec<Row> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut ctx = Some(context);
        while let Some(c) = ctx
            && c != Context::Editor
        {
            for (combo, cmd) in keymap.context_bindings(c) {
                if seen.insert(cmd) {
                    rows.push(Row {
                        key: combo.label(),
                        title: short_title(cmd),
                    });
                }
            }
            ctx = c.parent();
        }
        if !rows.is_empty() {
            out.push(Section {
                title: title.to_string(),
                rows,
            });
        }
    }
    for (title, commands) in groups() {
        let rows: Vec<Row> = commands
            .into_iter()
            .filter_map(|cmd| {
                Some(Row {
                    key: keymap.binding_for(cmd)?.label(),
                    title: short_title(cmd),
                })
            })
            .collect();
        if !rows.is_empty() {
            out.push(Section {
                title: title.to_string(),
                rows,
            });
        }
    }
    out
}

/// One column of the layout: its lines and its width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column<'a> {
    pub lines: Vec<Line<'a>>,
    pub width: usize,
}

/// The sections in as many columns as fit in `width` cells (`COLUMN_GAP` between two; one
/// column however narrow), each column as wide as its own rows.
pub fn layout(sections: &[Section], width: usize) -> Vec<Column<'_>> {
    let mut count = sections.len().max(1);
    loop {
        let cols: Vec<Column> = columns(sections, count).into_iter().map(measure).collect();
        let total = cols.iter().map(|c| c.width).sum::<usize>() + cols.len().saturating_sub(1) * COLUMN_GAP;
        if total <= width || count == 1 {
            return cols;
        }
        count -= 1;
    }
}

fn measure(lines: Vec<Line<'_>>) -> Column<'_> {
    let width = lines
        .iter()
        .map(|l| match l {
            Line::Header(t) => t.width(),
            Line::Row { row, key_width } => key_width + KEY_GAP + row.title.width(),
            Line::Blank => 0,
        })
        .max()
        .unwrap_or(0);
    Column { lines, width }
}

/// The sections flowed down at most `count` columns, whole sections per column (a blank line
/// between two), as evenly as that allows.
fn columns(sections: &[Section], count: usize) -> Vec<Vec<Line<'_>>> {
    let total: usize = sections.iter().map(|s| 1 + s.rows.len()).sum::<usize>() + sections.len().saturating_sub(1);
    let count = count.max(1);
    let mut target = total.div_ceil(count);
    loop {
        let mut cols: Vec<Vec<Line>> = Vec::new();
        for s in sections {
            let h = 1 + s.rows.len();
            match cols.last_mut() {
                Some(col) if col.len() + 1 + h <= target => col.push(Line::Blank),
                _ => cols.push(Vec::new()),
            }
            let col = cols.last_mut().expect("pushed");
            col.push(Line::Header(&s.title));
            let key_width = s.rows.iter().map(|r| r.key.width()).max().unwrap_or(0);
            col.extend(s.rows.iter().map(|row| Line::Row { row, key_width }));
        }
        if cols.len() <= count || target >= total {
            return cols;
        }
        target += 1;
    }
}

/// A command's title, without the palette's `Cursor: ` / `Selection: ` category (`Cursor: Move
/// Left` → `Move Left`) in the sections that say it already.
fn short_title(cmd: Command) -> String {
    let title = cmd.title();
    match title.split_once(": ") {
        Some(("Cursor" | "Selection", rest)) => rest.to_string(),
        _ => title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keymap::{Binding, KeyCombo, Platform, Source};

    fn row<'a>(sections: &'a [Section], title: &str) -> Option<&'a Row> {
        sections.iter().flat_map(|s| &s.rows).find(|r| r.title == title)
    }

    #[test]
    fn groups_show_each_platforms_keys() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let mac = platform == Platform::MacOs;
            let s = sections(&Keymap::default_for(platform), Context::Editor);
            let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
            assert_eq!(
                titles,
                [
                    "Cursor Movement",
                    "Selection",
                    "Going Places",
                    "Tabs and Groups",
                    "Panels"
                ]
            );
            let key = |cmd: Command| row(&s, &short_title(cmd)).map(|r| r.key.as_str());
            assert_eq!(key(Command::QuickOpen), Some(if mac { "Cmd+P" } else { "Ctrl+P" }));
            assert_eq!(key(Command::GoToLine), Some("Ctrl+G"));
            assert_eq!(key(Command::CheatSheet), Some("Shift+F1"));
            assert_eq!(
                key(Command::Move {
                    motion: Motion::WordLeft,
                    select: false
                }),
                Some(if mac { "Alt+Left" } else { "Ctrl+Left" })
            );
            assert_eq!(
                key(Command::Move {
                    motion: Motion::DocEnd,
                    select: true
                }),
                Some(if mac { "Cmd+Shift+Down" } else { "Ctrl+Shift+End" })
            );
            assert_eq!(key(Command::SplitRight), Some(if mac { "Cmd+\\" } else { "Ctrl+\\" }));
            assert_eq!(key(Command::AddNextMatch), Some(if mac { "Cmd+D" } else { "Ctrl+D" }));
            // Every command in the groups is bound by default on both platforms.
            let listed: usize = s.iter().map(|s| s.rows.len()).sum();
            let fixed: usize = groups().iter().map(|(_, c)| c.len()).sum();
            assert_eq!(listed, fixed, "{platform:?}");
        }
    }

    #[test]
    fn a_rebound_command_shows_its_new_key_and_an_unbound_one_is_left_out() {
        let user = [Binding {
            combo: KeyCombo::parse("ctrl+alt+o").unwrap(),
            context: None,
            command: Command::QuickOpen,
            source: Source::User,
        }];
        let km = Keymap::with_user(Platform::Linux, &user);
        let s = sections(&km, Context::Editor);
        assert_eq!(row(&s, &short_title(Command::QuickOpen)).unwrap().key, "Ctrl+Alt+O");
        // Ctrl+G → nothing: the user bound it in no context (only taking over the key).
        let user = [Binding {
            combo: KeyCombo::parse("ctrl+g").unwrap(),
            context: None,
            command: Command::QuickOpen,
            source: Source::User,
        }];
        let s = sections(&Keymap::with_user(Platform::Linux, &user), Context::Editor);
        assert!(row(&s, &short_title(Command::GoToLine)).is_none());
        assert_eq!(row(&s, &short_title(Command::QuickOpen)).unwrap().key, "Ctrl+G");
    }

    #[test]
    fn the_context_it_opens_from_comes_first() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let km = Keymap::default_for(platform);
            let s = sections(&km, Context::Explorer);
            assert_eq!(s[0].title, "Explorer");
            assert!(s[0].rows.iter().any(|r| r.key == "F2"), "{:?}", s[0]);
            assert_eq!(s[1].title, "Cursor Movement");
            // Replace has its own Enter and the find input's keys.
            let s = sections(&km, Context::Replace);
            assert_eq!(s[0].title, "Replace");
            assert!(s[0].rows.iter().any(|r| r.key == "Esc"));
            assert!(s[0].rows.iter().any(|r| r.key == "Enter"));
            let s = sections(&km, Context::Diff);
            assert_eq!(s[0].title, "Diff View");
            assert!(s[0].rows.iter().any(|r| r.key == "F7"));
            // No keys of its own (a hex tab takes the editor's): no section.
            assert_eq!(sections(&km, Context::Hex)[0].title, "Cursor Movement");
            // A shell tab's own keys are its scrollback, under "Terminal".
            let s = sections(&km, Context::Shell);
            assert_eq!(s[0].title, "Terminal");
            assert!(
                s[0].rows
                    .iter()
                    .any(|r| r.title == short_title(Command::Shell(crate::command::ShellCommand::PageUp))),
                "{:?}",
                s[0]
            );
            // The editor's own keys are the groups.
            assert_eq!(sections(&km, Context::Editor)[0].title, "Cursor Movement");
        }
    }

    #[test]
    fn columns_keep_sections_whole_and_balance_them() {
        let sec = |title: &str, n: usize| Section {
            title: title.into(),
            rows: (0..n)
                .map(|i| Row {
                    key: format!("K{i}"),
                    title: format!("T{i}"),
                })
                .collect(),
        };
        let s = [sec("a", 3), sec("b", 3), sec("c", 5)];
        let one = columns(&s, 1);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].len(), 4 + 1 + 4 + 1 + 6);
        let two = columns(&s, 2);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].len(), 9);
        assert_eq!(two[0][4], Line::Blank);
        assert_eq!(two[1][0], Line::Header("c"));
        assert_eq!(columns(&s, 3).len(), 3);
        assert_eq!(columns(&s, 9).len(), 3);
        assert!(columns(&[], 2).is_empty());

        // Each column as wide as its rows ("K0  T0"), three side by side when they fit.
        let w = 2 + KEY_GAP + 2;
        let cols = layout(&s, 3 * w + 2 * COLUMN_GAP);
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0].width, w);
        assert_eq!(layout(&s, 3 * w + 2 * COLUMN_GAP - 1).len(), 2);
        assert_eq!(layout(&s, 1).len(), 1);
    }

    #[test]
    fn scrolling_stays_in_range() {
        let mut c = CheatSheet::new(Context::Editor);
        (c.rows, c.height) = (30, 10);
        c.scroll_by(5);
        assert_eq!(c.top, 5);
        c.scroll_by(100);
        assert_eq!(c.top, 20);
        c.scroll_by(-100);
        assert_eq!(c.top, 0);
        assert!(c.scroll().shown);
    }
}
