//! Every user action is a named command; keybindings and the palette go through these.

use ExplorerCommand as X;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// Toggles between first non-blank and line start.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

impl Motion {
    pub const ALL: [Motion; 12] = [
        Motion::Left,
        Motion::Right,
        Motion::Up,
        Motion::Down,
        Motion::WordLeft,
        Motion::WordRight,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocStart,
        Motion::DocEnd,
        Motion::PageUp,
        Motion::PageDown,
    ];
}

/// Which way column (box) select moves the box's moving corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnMotion {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
}

impl ColumnMotion {
    pub const ALL: [ColumnMotion; 6] = [
        ColumnMotion::Left,
        ColumnMotion::Right,
        ColumnMotion::Up,
        ColumnMotion::Down,
        ColumnMotion::PageUp,
        ColumnMotion::PageDown,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Move {
        motion: Motion,
        select: bool,
    },
    DeleteLeft,
    DeleteRight,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToLineStart,
    InsertNewline,
    InsertTab,
    SelectAll,
    /// Esc: drop extra cursors, then clear the selection.
    CancelSelection,
    /// Cmd+D: select the word under the cursor, then add a selection at the next match.
    AddNextMatch,
    /// Cmd+Shift+L: a selection at every match of the selection (or the word under the cursor).
    SelectAllMatches,
    /// Cmd+Alt+D: move the newest selection to the next match.
    SkipToNextMatch,
    /// Cmd+U: undo the last selection change (separate from text undo).
    CursorUndo,
    /// Cmd+Alt+Up / Down: a copy of every cursor on the line above / below, same column.
    AddCursorAbove,
    AddCursorBelow,
    /// Shift+Alt+I: a cursor at the end of every line each selection covers.
    CursorsAtLineEnds,
    /// Cmd+Shift+Alt+arrows / PageUp / PageDown: grow or shrink a box selection.
    ColumnSelect(ColumnMotion),
    /// Alt+Up / Down: move the lines of every selection.
    MoveLinesUp,
    MoveLinesDown,
    /// Shift+Alt+Up / Down: duplicate the lines of every selection; the selection goes with
    /// the lower copy when copying down.
    CopyLinesUp,
    CopyLinesDown,
    /// Cmd+/: toggle the line comment on the lines of every selection.
    CommentLine,
    /// Cmd+]: indent the lines of every selection by one level (Tab over lines does too).
    IndentLines,
    /// Cmd+[ / Shift+Tab: outdent the lines of every selection by one level.
    OutdentLines,
    Undo,
    Redo,
    /// Copy / cut the selections, or the whole line when they're all empty.
    Copy,
    Cut,
    /// Paste from the system clipboard. On macOS Cmd+V stays with the terminal (bracketed paste).
    Paste,
    Save,
    Quit,
    CloseTab,
    NextTab,
    PrevTab,
    ToggleSidebar,
    FocusExplorer,
    /// Focus editor group N (1-based, reading order); one past the last splits right.
    FocusGroup(u8),
    SplitRight,
    SplitDown,
    Explorer(ExplorerCommand),
    /// Command palette: every command, with its binding.
    ShowCommands,
    /// Quick open a file (fuzzy).
    QuickOpen,
    GoToLine,
    ManageWorkspaceTrust,
    OpenLogFile,
    SetLogLevel,
    /// Open `~/.config/jed/settings.toml` (created from a template if missing).
    OpenUserSettings,
    /// Keys inside the palette.
    Palette(PaletteCommand),
    /// Cmd+F: open the find widget (seeded from the selection or the word under the cursor).
    OpenFind,
    /// Cmd+Alt+F (Ctrl+H on Linux): open it with the replace input.
    OpenReplace,
    /// The find widget's commands.
    Find(FindCommand),
    /// Cmd+Shift+F: show the Search view and focus its query, seeded with the selection.
    FindInFiles,
    /// The Search view's commands.
    Search(SearchCommand),
    /// Cmd+J: show (and focus) or hide the bottom panel.
    TogglePanel,
    /// Cmd+Shift+M: show and focus the Problems list, or hide the panel when it has the keys.
    ToggleProblems,
    /// The Problems list's commands, and next / previous problem.
    Problems(ProblemsCommand),
    /// Show the language server's hover for the cursor's position (VS Code's Cmd+K Cmd+I).
    ShowHover,
    /// F12: go to the definition of the symbol at the cursor (several: the Locations list).
    RevealDefinition,
    /// The Locations list's commands (the bottom panel's "LOCATIONS" tab).
    Locations(LocationsCommand),
    /// Ctrl+Space: ask the language server for completions at the cursor.
    TriggerSuggest,
    /// The completion list's keys (only while it's shown).
    Suggest(SuggestCommand),
    /// Shift+F12: the references to the symbol at the cursor, in the Locations list.
    GoToReferences,
    /// F2 in the editor: rename the symbol at the cursor across the workspace (a prompt).
    RenameSymbol,
    /// Shift+Alt+F (Linux: Ctrl+Shift+I): format the whole file with the language server.
    FormatDocument,
    /// The active file as it is on disk replaces the buffer's text (one undo step; unsaved
    /// edits too): the changed-on-disk banner's "Reload".
    ReloadFromDisk,
    /// The banner's "Keep Mine": keep the buffer's edits over the file changed on disk; the next
    /// save overwrites it.
    KeepMine,
    /// Stages the change the gutter shows on the cursor's line (from the file on disk, after
    /// saving the buffer).
    StageHunk,
    /// Unstages the staged change (index against HEAD) at the cursor's line.
    UnstageHunk,
    /// In an unstaged diff view (Index ↔ Working Tree): the change at the cursor back to the
    /// index's text in the working tree (the open buffer, one undo step, or the file on disk).
    RevertHunk,
    /// Alt+F5 / Shift+Alt+F5 (F7 / Shift+F7 in a diff tab): the cursor to the next / previous
    /// change in the gutter, or in the diff view.
    NextChange,
    PreviousChange,
    /// The diff view of the active file on disk against its buffer (the changed-on-disk
    /// banner's Show Diff).
    CompareWithSaved,
    /// The diff view of the active file's unstaged changes (staged ones if it has only those).
    OpenChange,
    /// Stages / unstages the active file's changes (all of them; from disk after saving).
    StageFile,
    UnstageFile,
    /// Stages / unstages every changed file (the Source Control view's section buttons).
    StageAll,
    UnstageAll,
    /// Ctrl+Shift+G: shows the Source Control view and gives it the keys.
    FocusScm,
    /// The Source Control view's list (`Context::Scm`).
    Scm(ScmCommand),
    /// The Explorer view's "Git History" section (`Context::GitHistory`).
    GitHistory(GitHistoryCommand),
    /// A commit tab (`Context::Commit`): opening a stat row's diff.
    Commit(CommitCommand),
    /// The active file's history entries in the palette; picking one compares it with the
    /// buffer (a diff tab).
    ShowFileHistory,
    /// On a history diff tab: its entry replaces the buffer's text (one undo step, not saved).
    /// Elsewhere: the history picker, whose pick is restored.
    RestoreFileHistory,
    /// "Run Task": a picker of the task templates; picking one runs it in a new panel tab.
    RunTask,
    /// Kills the shown task tab's run (else the newest running one).
    KillTask,
    /// Closes the shown task tab, killing its run if it still runs.
    CloseTask,
    /// A task tab's keys (`Context::Task`): scrolling, back to the editor.
    Task(TaskCommand),
    /// "Open Recent Workspace" (Ctrl+R): a picker of recent workspaces; picking one opens it in
    /// another jed (jed never switches its own workspace).
    OpenRecentWorkspace,
    /// "View: Toggle Render Whitespace": whitespace drawn as `·` / `→` off, or back to the
    /// `render_whitespace` setting (`all` when that is `none`); for the session only.
    ToggleRenderWhitespace,
    /// "Git: Toggle Inline Blame": the primary cursor line's blame after its text (off at
    /// start; for the session only).
    ToggleBlame,
    /// "Go Back": the place before the last jump (the navigation history, `navigation.rs`).
    NavBack,
    /// "Go Forward": the place the last "Go Back" left.
    NavForward,
    /// "Tools: Open Log": a picker of the tools with a log; picking one opens (or shows) its
    /// read-only log tab in the active group.
    OpenToolLog,
    /// "Tools: Clear Log": empties the active log tab's tool's log.
    ClearToolLog,
    /// "Tools: Show" (Cmd+Shift+U, VS Code's Output): shows the Tools view and gives it the keys.
    ShowTools,
    /// The Tools view's list (`Context::Tools`).
    Tools(ToolsCommand),
    /// "Tools: Toggle Protocol Trace": the selected (or the active file's) language server's
    /// JSON-RPC messages into its log, or off again.
    ToggleToolTrace,
    /// "Tools: Restart Server": stops the selected (or the active file's) language server and
    /// starts it again, also one that failed.
    RestartServer,
}

/// The Tools view's list (`Context::Tools`, while it has the keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolsCommand {
    /// The selection: a row up / down, a page, the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Enter: the selected tool's log tab (a task run: its panel tab), focused.
    Open,
    /// Esc: back to the editor.
    FocusEditor,
}

impl ToolsCommand {
    pub const ALL: [ToolsCommand; 8] = [
        ToolsCommand::Up,
        ToolsCommand::Down,
        ToolsCommand::PageUp,
        ToolsCommand::PageDown,
        ToolsCommand::First,
        ToolsCommand::Last,
        ToolsCommand::Open,
        ToolsCommand::FocusEditor,
    ];
}

/// A task tab in the bottom panel (`Context::Task`, while it has the keys); keys bound to
/// nothing go to the task's terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCommand {
    /// Through the scrollback: a line, a page, the oldest output, the live screen.
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// Esc: back to the editor; the panel stays.
    FocusEditor,
}

impl TaskCommand {
    pub const ALL: [TaskCommand; 7] = [
        TaskCommand::ScrollUp,
        TaskCommand::ScrollDown,
        TaskCommand::PageUp,
        TaskCommand::PageDown,
        TaskCommand::Top,
        TaskCommand::Bottom,
        TaskCommand::FocusEditor,
    ];
}

/// Focus-group commands listed in the palette and accepted in keybindings (as VS Code: 1 to 8).
const PALETTE_GROUPS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExplorerCommand {
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Collapse, or go to the parent.
    Left,
    /// Expand, or step into.
    Right,
    /// Open a file and focus it; toggle a folder.
    Open,
    /// Open a file but keep focus in the explorer.
    Preview,
    NewFile,
    NewFolder,
    Rename,
    Delete,
    /// "File History: Show" for the selected file (also a right-click on a file's row).
    ShowHistory,
    /// "Find in Folder…": the Search view with the selected folder as its files to include
    /// (also a right-click on a folder's row).
    FindInFolder,
}

impl ExplorerCommand {
    pub const ALL: [ExplorerCommand; 16] = [
        X::Up,
        X::Down,
        X::PageUp,
        X::PageDown,
        X::First,
        X::Last,
        X::Left,
        X::Right,
        X::Open,
        X::Preview,
        X::NewFile,
        X::NewFolder,
        X::Rename,
        X::Delete,
        X::ShowHistory,
        X::FindInFolder,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaletteCommand {
    Next,
    Previous,
    PageDown,
    PageUp,
    /// Run, open or pick the selected row.
    Accept,
    Close,
}

impl PaletteCommand {
    pub const ALL: [PaletteCommand; 6] = [
        PaletteCommand::Next,
        PaletteCommand::Previous,
        PaletteCommand::PageDown,
        PaletteCommand::PageUp,
        PaletteCommand::Accept,
        PaletteCommand::Close,
    ];
}

/// The find / replace widget. Next / previous and the toggles also work from the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FindCommand {
    /// Select the next / previous match, wrapping.
    Next,
    Previous,
    ToggleCase,
    ToggleWholeWord,
    ToggleRegex,
    /// Alt+Enter: a selection at every match.
    SelectAllMatches,
    /// Replace the selected match and go to the next one.
    ReplaceOne,
    /// Replace every match, as one undo step.
    ReplaceAll,
    /// Tab: between the find and replace inputs.
    SwitchField,
    /// Esc: close the widget, back to the editor.
    Close,
}

impl FindCommand {
    pub const ALL: [FindCommand; 10] = [
        FindCommand::Next,
        FindCommand::Previous,
        FindCommand::ToggleCase,
        FindCommand::ToggleWholeWord,
        FindCommand::ToggleRegex,
        FindCommand::SelectAllMatches,
        FindCommand::ReplaceOne,
        FindCommand::ReplaceAll,
        FindCommand::SwitchField,
        FindCommand::Close,
    ];
}

/// The Search view (M7): its query input, its results list, and stepping through results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchCommand {
    ToggleCase,
    ToggleRegex,
    /// Enter: search again now (no debounce), e.g. after editing a file.
    Refresh,
    /// Esc: back to the editor; the results stay.
    FocusEditor,
    /// Down in the query: the results list takes the keys.
    FocusResults,
    /// The list's selection: a row up (from the first row, back to the query) / down, a page,
    /// the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Enter in the list: open the selected match (a file row: its first) and focus the editor.
    Open,
    /// F4 / Shift+F4 from anywhere: open the next / previous match, focus where it was.
    NextResult,
    PreviousResult,
    /// The `…` button: show / hide the files to include / exclude inputs.
    ToggleDetails,
    /// Tab / Shift+Tab: the next / previous of the query, include and exclude inputs and the
    /// results list.
    NextInput,
    PreviousInput,
}

impl SearchCommand {
    pub const ALL: [SearchCommand; 17] = [
        SearchCommand::ToggleCase,
        SearchCommand::ToggleRegex,
        SearchCommand::Refresh,
        SearchCommand::FocusEditor,
        SearchCommand::FocusResults,
        SearchCommand::Up,
        SearchCommand::Down,
        SearchCommand::PageUp,
        SearchCommand::PageDown,
        SearchCommand::First,
        SearchCommand::Last,
        SearchCommand::Open,
        SearchCommand::NextResult,
        SearchCommand::PreviousResult,
        SearchCommand::ToggleDetails,
        SearchCommand::NextInput,
        SearchCommand::PreviousInput,
    ];
}

/// The Problems list in the bottom panel (M9), and stepping through problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemsCommand {
    /// The list's selection: a row up / down, a page, the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Enter: open the selected problem (a file row: its first) and focus the editor.
    Open,
    /// Esc: back to the editor; the panel stays.
    FocusEditor,
    /// F8 / Shift+F8 from anywhere: the next / previous problem after the cursor, on through the
    /// files with problems (VS Code's "in files").
    Next,
    Previous,
}

impl ProblemsCommand {
    pub const ALL: [ProblemsCommand; 10] = [
        ProblemsCommand::Up,
        ProblemsCommand::Down,
        ProblemsCommand::PageUp,
        ProblemsCommand::PageDown,
        ProblemsCommand::First,
        ProblemsCommand::Last,
        ProblemsCommand::Open,
        ProblemsCommand::FocusEditor,
        ProblemsCommand::Next,
        ProblemsCommand::Previous,
    ];
}

/// The Locations list in the bottom panel (definitions; 9g's references).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocationsCommand {
    /// The list's selection: a row up / down, a page, the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Enter: open the selected place (a file row: its first) and focus the editor.
    Open,
    /// Esc: back to the editor; the panel stays.
    FocusEditor,
}

impl LocationsCommand {
    pub const ALL: [LocationsCommand; 8] = [
        LocationsCommand::Up,
        LocationsCommand::Down,
        LocationsCommand::PageUp,
        LocationsCommand::PageDown,
        LocationsCommand::First,
        LocationsCommand::Last,
        LocationsCommand::Open,
        LocationsCommand::FocusEditor,
    ];
}

/// The Source Control view's list (`Context::Scm`, while it has the keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScmCommand {
    /// The selection: a row up / down, a page, the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Left: collapse a section (from a file: select its section's header); Right: expand it.
    Collapse,
    Expand,
    /// Enter: open the selected file's diff and focus it (a header: collapse / expand).
    Open,
    /// Space: open the diff, keeping the keys in the list.
    Preview,
    /// Esc: back to the editor.
    FocusEditor,
}

impl ScmCommand {
    pub const ALL: [ScmCommand; 11] = [
        ScmCommand::Up,
        ScmCommand::Down,
        ScmCommand::PageUp,
        ScmCommand::PageDown,
        ScmCommand::First,
        ScmCommand::Last,
        ScmCommand::Collapse,
        ScmCommand::Expand,
        ScmCommand::Open,
        ScmCommand::Preview,
        ScmCommand::FocusEditor,
    ];
}

/// The Explorer view's "Git History" section (`Context::GitHistory`, while it has the keys):
/// moving the selection through the loaded commits, back to the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitHistoryCommand {
    /// The selection: a row up / down, a page, the first / last row.
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Enter (or a click): the selected commit's tab (reusing one already showing it).
    Open,
    /// Left / Esc: back to the tree (`focused = false`); the side panel keeps the keys.
    FocusTree,
}

impl GitHistoryCommand {
    pub const ALL: [GitHistoryCommand; 8] = [
        GitHistoryCommand::Up,
        GitHistoryCommand::Down,
        GitHistoryCommand::PageUp,
        GitHistoryCommand::PageDown,
        GitHistoryCommand::First,
        GitHistoryCommand::Last,
        GitHistoryCommand::Open,
        GitHistoryCommand::FocusTree,
    ];
}

/// A commit tab's keys (`Context::Commit`, which falls back to the editor's movement, copy and
/// select keys): opening the file's diff at the cursor's stat row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitCommand {
    /// Enter: the diff of the file whose stat line the cursor is on, in the group to the right
    /// (splitting one when there is none). Not on a stat line: a message.
    Open,
}

impl CommitCommand {
    pub const ALL: [CommitCommand; 1] = [CommitCommand::Open];
}

/// The completion list (`Context::Suggest`, while it's shown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestCommand {
    /// Up / Down: the previous / next item, wrapping round.
    Up,
    Down,
    PageUp,
    PageDown,
    /// Enter / Tab: insert the selected item at every cursor.
    Accept,
    /// Esc: close the list.
    Close,
}

impl SuggestCommand {
    pub const ALL: [SuggestCommand; 6] = [
        SuggestCommand::Up,
        SuggestCommand::Down,
        SuggestCommand::PageUp,
        SuggestCommand::PageDown,
        SuggestCommand::Accept,
        SuggestCommand::Close,
    ];
}

impl Command {
    /// Every command, in a stable order: what the palette lists and keybindings can name.
    pub fn all() -> Vec<Command> {
        let mut all = Vec::new();
        for motion in Motion::ALL {
            for select in [false, true] {
                all.push(Command::Move { motion, select });
            }
        }
        all.extend([
            Command::DeleteLeft,
            Command::DeleteRight,
            Command::DeleteWordLeft,
            Command::DeleteWordRight,
            Command::DeleteToLineStart,
            Command::InsertNewline,
            Command::InsertTab,
            Command::SelectAll,
            Command::CancelSelection,
            Command::AddNextMatch,
            Command::SelectAllMatches,
            Command::SkipToNextMatch,
            Command::CursorUndo,
            Command::AddCursorAbove,
            Command::AddCursorBelow,
            Command::CursorsAtLineEnds,
        ]);
        all.extend(ColumnMotion::ALL.map(Command::ColumnSelect));
        all.extend([
            Command::MoveLinesUp,
            Command::MoveLinesDown,
            Command::CopyLinesUp,
            Command::CopyLinesDown,
            Command::CommentLine,
            Command::IndentLines,
            Command::OutdentLines,
            Command::Undo,
            Command::Redo,
            Command::Copy,
            Command::Cut,
            Command::Paste,
            Command::Save,
            Command::Quit,
            Command::CloseTab,
            Command::NextTab,
            Command::PrevTab,
            Command::ToggleSidebar,
            Command::FocusExplorer,
        ]);
        all.extend((1..=PALETTE_GROUPS).map(Command::FocusGroup));
        all.extend([Command::SplitRight, Command::SplitDown]);
        all.extend(ExplorerCommand::ALL.map(Command::Explorer));
        all.extend([
            Command::ShowCommands,
            Command::QuickOpen,
            Command::GoToLine,
            Command::ManageWorkspaceTrust,
            Command::OpenLogFile,
            Command::SetLogLevel,
            Command::OpenUserSettings,
        ]);
        all.extend(PaletteCommand::ALL.map(Command::Palette));
        all.extend([Command::OpenFind, Command::OpenReplace]);
        all.extend(FindCommand::ALL.map(Command::Find));
        all.push(Command::FindInFiles);
        all.extend(SearchCommand::ALL.map(Command::Search));
        all.extend([Command::TogglePanel, Command::ToggleProblems]);
        all.extend(ProblemsCommand::ALL.map(Command::Problems));
        all.extend([Command::ShowHover, Command::RevealDefinition]);
        all.extend(LocationsCommand::ALL.map(Command::Locations));
        all.push(Command::TriggerSuggest);
        all.extend(SuggestCommand::ALL.map(Command::Suggest));
        all.extend([Command::GoToReferences, Command::RenameSymbol, Command::FormatDocument]);
        all.extend([Command::ReloadFromDisk, Command::KeepMine]);
        all.extend([
            Command::StageHunk,
            Command::UnstageHunk,
            Command::RevertHunk,
            Command::NextChange,
            Command::PreviousChange,
            Command::CompareWithSaved,
            Command::OpenChange,
            Command::StageFile,
            Command::UnstageFile,
            Command::StageAll,
            Command::UnstageAll,
            Command::FocusScm,
        ]);
        all.extend(ScmCommand::ALL.map(Command::Scm));
        all.extend(GitHistoryCommand::ALL.map(Command::GitHistory));
        all.extend(CommitCommand::ALL.map(Command::Commit));
        all.extend([Command::ShowFileHistory, Command::RestoreFileHistory]);
        all.extend([Command::RunTask, Command::KillTask, Command::CloseTask]);
        all.extend(TaskCommand::ALL.map(Command::Task));
        all.push(Command::OpenRecentWorkspace);
        all.push(Command::ToggleRenderWhitespace);
        all.push(Command::ToggleBlame);
        all.extend([Command::NavBack, Command::NavForward]);
        all.extend([
            Command::OpenToolLog,
            Command::ClearToolLog,
            Command::ShowTools,
            Command::ToggleToolTrace,
            Command::RestartServer,
        ]);
        all.extend(ToolsCommand::ALL.map(Command::Tools));
        all
    }

    /// The command with this stable name (as used in `keybindings.toml`).
    pub fn from_name(name: &str) -> Option<Command> {
        Command::all().into_iter().find(|c| c.name() == name)
    }

    /// Whether the palette lists it: everything except the palette's own keys.
    pub fn in_palette(&self) -> bool {
        !matches!(self, Command::Palette(_))
    }

    /// Stable name, used in logs, keybinding config and the palette.
    pub fn name(&self) -> String {
        match self {
            Command::Move { motion, select } => {
                let m = match motion {
                    Motion::Left => "left",
                    Motion::Right => "right",
                    Motion::Up => "up",
                    Motion::Down => "down",
                    Motion::WordLeft => "wordLeft",
                    Motion::WordRight => "wordRight",
                    Motion::LineStart => "home",
                    Motion::LineEnd => "end",
                    Motion::DocStart => "top",
                    Motion::DocEnd => "bottom",
                    Motion::PageUp => "pageUp",
                    Motion::PageDown => "pageDown",
                };
                format!("cursor.{m}{}", if *select { ".select" } else { "" })
            }
            Command::DeleteLeft => "edit.deleteLeft".into(),
            Command::DeleteRight => "edit.deleteRight".into(),
            Command::DeleteWordLeft => "edit.deleteWordLeft".into(),
            Command::DeleteWordRight => "edit.deleteWordRight".into(),
            Command::DeleteToLineStart => "edit.deleteToLineStart".into(),
            Command::InsertNewline => "edit.newline".into(),
            Command::InsertTab => "edit.tab".into(),
            Command::SelectAll => "selection.all".into(),
            Command::CancelSelection => "selection.cancel".into(),
            Command::AddNextMatch => "selection.addNextMatch".into(),
            Command::SelectAllMatches => "selection.selectAllMatches".into(),
            Command::SkipToNextMatch => "selection.moveToNextMatch".into(),
            Command::CursorUndo => "cursor.undo".into(),
            Command::AddCursorAbove => "selection.addCursorAbove".into(),
            Command::AddCursorBelow => "selection.addCursorBelow".into(),
            Command::CursorsAtLineEnds => "selection.addCursorsToLineEnds".into(),
            Command::ColumnSelect(m) => format!("selection.columnSelect{}", column_motion_name(*m)),
            Command::MoveLinesUp => "edit.moveLinesUp".into(),
            Command::MoveLinesDown => "edit.moveLinesDown".into(),
            Command::CopyLinesUp => "edit.copyLinesUp".into(),
            Command::CopyLinesDown => "edit.copyLinesDown".into(),
            Command::CommentLine => "edit.commentLine".into(),
            Command::IndentLines => "edit.indentLines".into(),
            Command::OutdentLines => "edit.outdentLines".into(),
            Command::Undo => "edit.undo".into(),
            Command::Redo => "edit.redo".into(),
            Command::Copy => "edit.copy".into(),
            Command::Cut => "edit.cut".into(),
            Command::Paste => "edit.paste".into(),
            Command::Save => "file.save".into(),
            Command::Quit => "app.quit".into(),
            Command::CloseTab => "tab.close".into(),
            Command::NextTab => "tab.next".into(),
            Command::PrevTab => "tab.previous".into(),
            Command::ToggleSidebar => "view.toggleSidebar".into(),
            Command::FocusExplorer => "view.focusExplorer".into(),
            Command::FocusGroup(n) => format!("view.focusGroup{n}"),
            Command::SplitRight => "view.splitRight".into(),
            Command::SplitDown => "view.splitDown".into(),
            Command::Explorer(c) => format!(
                "explorer.{}",
                match c {
                    X::Up => "up",
                    X::Down => "down",
                    X::PageUp => "pageUp",
                    X::PageDown => "pageDown",
                    X::First => "first",
                    X::Last => "last",
                    X::Left => "collapse",
                    X::Right => "expand",
                    X::Open => "open",
                    X::Preview => "preview",
                    X::NewFile => "newFile",
                    X::NewFolder => "newFolder",
                    X::Rename => "rename",
                    X::Delete => "delete",
                    X::ShowHistory => "showFileHistory",
                    X::FindInFolder => "findInFolder",
                }
            ),
            Command::ShowCommands => "palette.showCommands".into(),
            Command::QuickOpen => "palette.quickOpen".into(),
            Command::GoToLine => "palette.goToLine".into(),
            Command::ManageWorkspaceTrust => "workspace.manageTrust".into(),
            Command::OpenLogFile => "log.open".into(),
            Command::SetLogLevel => "log.setLevel".into(),
            Command::OpenUserSettings => "preferences.openUserSettings".into(),
            Command::Palette(c) => format!(
                "palette.{}",
                match c {
                    PaletteCommand::Next => "next",
                    PaletteCommand::Previous => "previous",
                    PaletteCommand::PageDown => "pageDown",
                    PaletteCommand::PageUp => "pageUp",
                    PaletteCommand::Accept => "accept",
                    PaletteCommand::Close => "close",
                }
            ),
            Command::OpenFind => "find.open".into(),
            Command::OpenReplace => "find.openReplace".into(),
            Command::Find(c) => format!(
                "find.{}",
                match c {
                    FindCommand::Next => "next",
                    FindCommand::Previous => "previous",
                    FindCommand::ToggleCase => "toggleCase",
                    FindCommand::ToggleWholeWord => "toggleWholeWord",
                    FindCommand::ToggleRegex => "toggleRegex",
                    FindCommand::SelectAllMatches => "selectAllMatches",
                    FindCommand::ReplaceOne => "replace",
                    FindCommand::ReplaceAll => "replaceAll",
                    FindCommand::SwitchField => "switchField",
                    FindCommand::Close => "close",
                }
            ),
            Command::FindInFiles => "search.findInFiles".into(),
            Command::Search(c) => format!(
                "search.{}",
                match c {
                    SearchCommand::ToggleCase => "toggleCase",
                    SearchCommand::ToggleRegex => "toggleRegex",
                    SearchCommand::Refresh => "refresh",
                    SearchCommand::FocusEditor => "focusEditor",
                    SearchCommand::FocusResults => "focusResults",
                    SearchCommand::Up => "up",
                    SearchCommand::Down => "down",
                    SearchCommand::PageUp => "pageUp",
                    SearchCommand::PageDown => "pageDown",
                    SearchCommand::First => "first",
                    SearchCommand::Last => "last",
                    SearchCommand::Open => "open",
                    SearchCommand::NextResult => "next",
                    SearchCommand::PreviousResult => "previous",
                    SearchCommand::ToggleDetails => "toggleDetails",
                    SearchCommand::NextInput => "nextInput",
                    SearchCommand::PreviousInput => "previousInput",
                }
            ),
            Command::TogglePanel => "view.togglePanel".into(),
            Command::ToggleProblems => "view.toggleProblems".into(),
            Command::Problems(c) => format!(
                "problems.{}",
                match c {
                    ProblemsCommand::Up => "up",
                    ProblemsCommand::Down => "down",
                    ProblemsCommand::PageUp => "pageUp",
                    ProblemsCommand::PageDown => "pageDown",
                    ProblemsCommand::First => "first",
                    ProblemsCommand::Last => "last",
                    ProblemsCommand::Open => "open",
                    ProblemsCommand::FocusEditor => "focusEditor",
                    ProblemsCommand::Next => "next",
                    ProblemsCommand::Previous => "previous",
                }
            ),
            Command::ShowHover => "editor.showHover".into(),
            Command::RevealDefinition => "editor.revealDefinition".into(),
            Command::Locations(c) => format!(
                "locations.{}",
                match c {
                    LocationsCommand::Up => "up",
                    LocationsCommand::Down => "down",
                    LocationsCommand::PageUp => "pageUp",
                    LocationsCommand::PageDown => "pageDown",
                    LocationsCommand::First => "first",
                    LocationsCommand::Last => "last",
                    LocationsCommand::Open => "open",
                    LocationsCommand::FocusEditor => "focusEditor",
                }
            ),
            Command::TriggerSuggest => "editor.triggerSuggest".into(),
            Command::Suggest(c) => format!(
                "suggest.{}",
                match c {
                    SuggestCommand::Up => "up",
                    SuggestCommand::Down => "down",
                    SuggestCommand::PageUp => "pageUp",
                    SuggestCommand::PageDown => "pageDown",
                    SuggestCommand::Accept => "accept",
                    SuggestCommand::Close => "close",
                }
            ),
            Command::GoToReferences => "editor.goToReferences".into(),
            Command::RenameSymbol => "editor.rename".into(),
            Command::FormatDocument => "editor.formatDocument".into(),
            Command::ReloadFromDisk => "file.reloadFromDisk".into(),
            Command::KeepMine => "file.keepMine".into(),
            Command::StageHunk => "git.stageHunk".into(),
            Command::UnstageHunk => "git.unstageHunk".into(),
            Command::RevertHunk => "git.revertHunk".into(),
            Command::NextChange => "editor.nextChange".into(),
            Command::CompareWithSaved => "file.compareWithSaved".into(),
            Command::ShowFileHistory => "fileHistory.show".into(),
            Command::RestoreFileHistory => "fileHistory.restore".into(),
            Command::OpenChange => "git.openChange".into(),
            Command::StageFile => "git.stageFile".into(),
            Command::UnstageFile => "git.unstageFile".into(),
            Command::StageAll => "git.stageAll".into(),
            Command::UnstageAll => "git.unstageAll".into(),
            Command::FocusScm => "view.focusSourceControl".into(),
            Command::Scm(c) => format!(
                "scm.{}",
                match c {
                    ScmCommand::Up => "up",
                    ScmCommand::Down => "down",
                    ScmCommand::PageUp => "pageUp",
                    ScmCommand::PageDown => "pageDown",
                    ScmCommand::First => "first",
                    ScmCommand::Last => "last",
                    ScmCommand::Collapse => "collapse",
                    ScmCommand::Expand => "expand",
                    ScmCommand::Open => "open",
                    ScmCommand::Preview => "preview",
                    ScmCommand::FocusEditor => "focusEditor",
                }
            ),
            Command::GitHistory(c) => format!(
                "gitHistory.{}",
                match c {
                    GitHistoryCommand::Up => "up",
                    GitHistoryCommand::Down => "down",
                    GitHistoryCommand::PageUp => "pageUp",
                    GitHistoryCommand::PageDown => "pageDown",
                    GitHistoryCommand::First => "first",
                    GitHistoryCommand::Last => "last",
                    GitHistoryCommand::Open => "open",
                    GitHistoryCommand::FocusTree => "focusTree",
                }
            ),
            Command::Commit(c) => format!(
                "commit.{}",
                match c {
                    CommitCommand::Open => "open",
                }
            ),
            Command::PreviousChange => "editor.previousChange".into(),
            Command::RunTask => "task.run".into(),
            Command::KillTask => "task.kill".into(),
            Command::CloseTask => "task.close".into(),
            Command::OpenRecentWorkspace => "workspace.openRecent".into(),
            Command::ToggleRenderWhitespace => "view.toggleRenderWhitespace".into(),
            Command::ToggleBlame => "git.toggleBlame".into(),
            Command::NavBack => "nav.back".into(),
            Command::NavForward => "nav.forward".into(),
            Command::OpenToolLog => "tools.openLog".into(),
            Command::ClearToolLog => "tools.clearLog".into(),
            Command::ShowTools => "tools.show".into(),
            Command::ToggleToolTrace => "tools.toggleTrace".into(),
            Command::RestartServer => "tools.restartServer".into(),
            Command::Tools(c) => format!(
                "toolsView.{}",
                match c {
                    ToolsCommand::Up => "up",
                    ToolsCommand::Down => "down",
                    ToolsCommand::PageUp => "pageUp",
                    ToolsCommand::PageDown => "pageDown",
                    ToolsCommand::First => "first",
                    ToolsCommand::Last => "last",
                    ToolsCommand::Open => "open",
                    ToolsCommand::FocusEditor => "focusEditor",
                }
            ),
            Command::Task(c) => format!(
                "task.{}",
                match c {
                    TaskCommand::ScrollUp => "scrollUp",
                    TaskCommand::ScrollDown => "scrollDown",
                    TaskCommand::PageUp => "scrollPageUp",
                    TaskCommand::PageDown => "scrollPageDown",
                    TaskCommand::Top => "scrollToTop",
                    TaskCommand::Bottom => "scrollToBottom",
                    TaskCommand::FocusEditor => "focusEditor",
                }
            ),
        }
    }

    /// What the palette shows, "Category: Action" as VS Code does.
    pub fn title(&self) -> String {
        let s = match self {
            Command::Move { motion, select } => {
                let m = match motion {
                    Motion::Left => "Left",
                    Motion::Right => "Right",
                    Motion::Up => "Up",
                    Motion::Down => "Down",
                    Motion::WordLeft => "Word Left",
                    Motion::WordRight => "Word Right",
                    Motion::LineStart => "Line Start",
                    Motion::LineEnd => "Line End",
                    Motion::DocStart => "File Start",
                    Motion::DocEnd => "File End",
                    Motion::PageUp => "Page Up",
                    Motion::PageDown => "Page Down",
                };
                let verb = if *select { "Select" } else { "Move" };
                let to = matches!(
                    motion,
                    Motion::LineStart | Motion::LineEnd | Motion::DocStart | Motion::DocEnd
                );
                return format!("Cursor: {verb} {}{m}", if to { "to " } else { "" });
            }
            Command::DeleteLeft => "Edit: Delete Left",
            Command::DeleteRight => "Edit: Delete Right",
            Command::DeleteWordLeft => "Edit: Delete Word Left",
            Command::DeleteWordRight => "Edit: Delete Word Right",
            Command::DeleteToLineStart => "Edit: Delete to Line Start",
            Command::InsertNewline => "Edit: Insert Line Break",
            Command::InsertTab => "Edit: Insert Tab",
            Command::SelectAll => "Selection: Select All",
            Command::CancelSelection => "Selection: Cancel Selection",
            Command::AddNextMatch => "Selection: Add Selection to Next Find Match",
            Command::SelectAllMatches => "Selection: Select All Occurrences of Find Match",
            Command::SkipToNextMatch => "Selection: Move Last Selection to Next Find Match",
            Command::CursorUndo => "Cursor: Cursor Undo",
            Command::AddCursorAbove => "Selection: Add Cursor Above",
            Command::AddCursorBelow => "Selection: Add Cursor Below",
            Command::CursorsAtLineEnds => "Selection: Add Cursors to Line Ends",
            Command::ColumnSelect(m) => {
                let m = match m {
                    ColumnMotion::PageUp => "Page Up",
                    ColumnMotion::PageDown => "Page Down",
                    m => column_motion_name(*m),
                };
                return format!("Selection: Column Select {m}");
            }
            Command::MoveLinesUp => "Edit: Move Line Up",
            Command::MoveLinesDown => "Edit: Move Line Down",
            Command::CopyLinesUp => "Edit: Copy Line Up",
            Command::CopyLinesDown => "Edit: Copy Line Down",
            Command::CommentLine => "Edit: Toggle Line Comment",
            Command::IndentLines => "Edit: Indent Line",
            Command::OutdentLines => "Edit: Outdent Line",
            Command::Undo => "Edit: Undo",
            Command::Redo => "Edit: Redo",
            Command::Copy => "Edit: Copy",
            Command::Cut => "Edit: Cut",
            Command::Paste => "Edit: Paste",
            Command::Save => "File: Save",
            Command::Quit => "File: Quit",
            Command::CloseTab => "View: Close Editor",
            Command::NextTab => "View: Open Next Editor",
            Command::PrevTab => "View: Open Previous Editor",
            Command::ToggleSidebar => "View: Toggle Side Bar Visibility",
            Command::FocusExplorer => "View: Show Explorer",
            Command::FocusGroup(n) => return format!("View: Focus Editor Group {n}"),
            Command::SplitRight => "View: Split Editor Right",
            Command::SplitDown => "View: Split Editor Down",
            Command::Explorer(c) => match c {
                X::Up => "Explorer: Select Previous",
                X::Down => "Explorer: Select Next",
                X::PageUp => "Explorer: Page Up",
                X::PageDown => "Explorer: Page Down",
                X::First => "Explorer: Select First",
                X::Last => "Explorer: Select Last",
                X::Left => "Explorer: Collapse",
                X::Right => "Explorer: Expand",
                X::Open => "Explorer: Open",
                X::Preview => "Explorer: Open, Keep Focus",
                X::NewFile => "Explorer: New File",
                X::NewFolder => "Explorer: New Folder",
                X::Rename => "Explorer: Rename",
                X::Delete => "Explorer: Delete",
                X::ShowHistory => "Explorer: Show File History",
                X::FindInFolder => "Explorer: Find in Folder…",
            },
            Command::ShowCommands => "Show All Commands",
            Command::QuickOpen => "Go to File…",
            Command::GoToLine => "Go to Line…",
            Command::ManageWorkspaceTrust => "Workspaces: Manage Workspace Trust",
            Command::OpenLogFile => "Developer: Open Log File",
            Command::SetLogLevel => "Developer: Set Log Level…",
            Command::OpenUserSettings => "Preferences: Open User Settings",
            Command::Palette(c) => match c {
                PaletteCommand::Next => "Palette: Select Next",
                PaletteCommand::Previous => "Palette: Select Previous",
                PaletteCommand::PageDown => "Palette: Page Down",
                PaletteCommand::PageUp => "Palette: Page Up",
                PaletteCommand::Accept => "Palette: Accept",
                PaletteCommand::Close => "Palette: Close",
            },
            Command::OpenFind => "Find: Find",
            Command::OpenReplace => "Find: Replace",
            Command::Find(c) => match c {
                FindCommand::Next => "Find: Find Next",
                FindCommand::Previous => "Find: Find Previous",
                FindCommand::ToggleCase => "Find: Toggle Match Case",
                FindCommand::ToggleWholeWord => "Find: Toggle Match Whole Word",
                FindCommand::ToggleRegex => "Find: Toggle Use Regular Expression",
                FindCommand::SelectAllMatches => "Find: Select Every Match",
                FindCommand::ReplaceOne => "Find: Replace One",
                FindCommand::ReplaceAll => "Find: Replace All",
                FindCommand::SwitchField => "Find: Switch Between Find and Replace",
                FindCommand::Close => "Find: Close",
            },
            Command::FindInFiles => "Search: Find in Files",
            Command::Search(c) => match c {
                SearchCommand::ToggleCase => "Search: Toggle Match Case",
                SearchCommand::ToggleRegex => "Search: Toggle Use Regular Expression",
                SearchCommand::Refresh => "Search: Refresh",
                SearchCommand::FocusEditor => "Search: Back to Editor",
                SearchCommand::FocusResults => "Search: Focus Results",
                SearchCommand::Up => "Search: Select Previous Row",
                SearchCommand::Down => "Search: Select Next Row",
                SearchCommand::PageUp => "Search: Select Row a Page Up",
                SearchCommand::PageDown => "Search: Select Row a Page Down",
                SearchCommand::First => "Search: Select First Row",
                SearchCommand::Last => "Search: Select Last Row",
                SearchCommand::Open => "Search: Open Selected Result",
                SearchCommand::NextResult => "Search: Go to Next Result",
                SearchCommand::PreviousResult => "Search: Go to Previous Result",
                SearchCommand::ToggleDetails => "Search: Toggle Search Details",
                SearchCommand::NextInput => "Search: Focus Next Input",
                SearchCommand::PreviousInput => "Search: Focus Previous Input",
            },
            Command::TogglePanel => "View: Toggle Panel Visibility",
            Command::ToggleProblems => "View: Toggle Problems",
            Command::Problems(c) => match c {
                ProblemsCommand::Up => "Problems: Select Previous Row",
                ProblemsCommand::Down => "Problems: Select Next Row",
                ProblemsCommand::PageUp => "Problems: Select Row a Page Up",
                ProblemsCommand::PageDown => "Problems: Select Row a Page Down",
                ProblemsCommand::First => "Problems: Select First Row",
                ProblemsCommand::Last => "Problems: Select Last Row",
                ProblemsCommand::Open => "Problems: Open Selected Problem",
                ProblemsCommand::FocusEditor => "Problems: Back to Editor",
                ProblemsCommand::Next => "Go to Next Problem in Files (Error, Warning, Info)",
                ProblemsCommand::Previous => "Go to Previous Problem in Files (Error, Warning, Info)",
            },
            Command::ShowHover => "Show or Focus Hover",
            Command::RevealDefinition => "Go to Definition",
            Command::Locations(c) => match c {
                LocationsCommand::Up => "Locations: Select Previous Row",
                LocationsCommand::Down => "Locations: Select Next Row",
                LocationsCommand::PageUp => "Locations: Select Row a Page Up",
                LocationsCommand::PageDown => "Locations: Select Row a Page Down",
                LocationsCommand::First => "Locations: Select First Row",
                LocationsCommand::Last => "Locations: Select Last Row",
                LocationsCommand::Open => "Locations: Open Selected Location",
                LocationsCommand::FocusEditor => "Locations: Back to Editor",
            },
            Command::TriggerSuggest => "Trigger Suggest",
            Command::Suggest(c) => match c {
                SuggestCommand::Up => "Suggest: Select Previous Suggestion",
                SuggestCommand::Down => "Suggest: Select Next Suggestion",
                SuggestCommand::PageUp => "Suggest: Select Previous Page of Suggestions",
                SuggestCommand::PageDown => "Suggest: Select Next Page of Suggestions",
                SuggestCommand::Accept => "Suggest: Accept Selected Suggestion",
                SuggestCommand::Close => "Suggest: Hide Suggestions",
            },
            Command::GoToReferences => "Go to References",
            Command::RenameSymbol => "Rename Symbol",
            Command::FormatDocument => "Format Document",
            Command::ReloadFromDisk => "File: Reload from Disk",
            Command::KeepMine => "File: Keep Mine (Overwrite the Changed File)",
            Command::StageHunk => "Git: Stage Change at Cursor",
            Command::UnstageHunk => "Git: Unstage Change at Cursor",
            Command::RevertHunk => "Git: Revert Change at Cursor",
            Command::NextChange => "Go to Next Change",
            Command::CompareWithSaved => "File: Compare with Disk",
            Command::ShowFileHistory => "File History: Show",
            Command::RestoreFileHistory => "File History: Restore",
            Command::OpenChange => "Git: Open Changes",
            Command::StageFile => "Git: Stage File",
            Command::UnstageFile => "Git: Unstage File",
            Command::StageAll => "Git: Stage All Changes",
            Command::UnstageAll => "Git: Unstage All Changes",
            Command::FocusScm => "View: Show Source Control",
            Command::Scm(c) => match c {
                ScmCommand::Up => "Source Control: Select Previous Row",
                ScmCommand::Down => "Source Control: Select Next Row",
                ScmCommand::PageUp => "Source Control: Select Row a Page Up",
                ScmCommand::PageDown => "Source Control: Select Row a Page Down",
                ScmCommand::First => "Source Control: Select First Row",
                ScmCommand::Last => "Source Control: Select Last Row",
                ScmCommand::Collapse => "Source Control: Collapse",
                ScmCommand::Expand => "Source Control: Expand",
                ScmCommand::Open => "Source Control: Open Changes",
                ScmCommand::Preview => "Source Control: Open Changes, Keep Focus",
                ScmCommand::FocusEditor => "Source Control: Back to Editor",
            },
            Command::GitHistory(c) => match c {
                GitHistoryCommand::Up => "Git History: Select Previous Commit",
                GitHistoryCommand::Down => "Git History: Select Next Commit",
                GitHistoryCommand::PageUp => "Git History: Select Commit a Page Up",
                GitHistoryCommand::PageDown => "Git History: Select Commit a Page Down",
                GitHistoryCommand::First => "Git History: Select First Commit",
                GitHistoryCommand::Last => "Git History: Select Last Commit",
                GitHistoryCommand::Open => "Git History: Open Commit",
                GitHistoryCommand::FocusTree => "Git History: Back to the Explorer",
            },
            Command::Commit(c) => match c {
                CommitCommand::Open => "Commit: Open the File's Changes",
            },
            Command::PreviousChange => "Go to Previous Change",
            Command::RunTask => "Tasks: Run Task",
            Command::KillTask => "Tasks: Kill Task",
            Command::CloseTask => "Tasks: Close Task Tab",
            Command::OpenRecentWorkspace => "Workspaces: Open Recent Workspace",
            Command::ToggleRenderWhitespace => "View: Toggle Render Whitespace",
            Command::ToggleBlame => "Git: Toggle Inline Blame",
            Command::NavBack => "Go Back",
            Command::NavForward => "Go Forward",
            Command::OpenToolLog => "Tools: Open Log",
            Command::ClearToolLog => "Tools: Clear Log",
            Command::ShowTools => "Tools: Show",
            Command::ToggleToolTrace => "Tools: Toggle Protocol Trace",
            Command::RestartServer => "Tools: Restart Server",
            Command::Tools(c) => match c {
                ToolsCommand::Up => "Tools: Select Previous Row",
                ToolsCommand::Down => "Tools: Select Next Row",
                ToolsCommand::PageUp => "Tools: Select Row a Page Up",
                ToolsCommand::PageDown => "Tools: Select Row a Page Down",
                ToolsCommand::First => "Tools: Select First Row",
                ToolsCommand::Last => "Tools: Select Last Row",
                ToolsCommand::Open => "Tools: Open the Selected Tool's Log",
                ToolsCommand::FocusEditor => "Tools: Back to Editor",
            },
            Command::Task(c) => match c {
                TaskCommand::ScrollUp => "Tasks: Scroll Up",
                TaskCommand::ScrollDown => "Tasks: Scroll Down",
                TaskCommand::PageUp => "Tasks: Scroll Page Up",
                TaskCommand::PageDown => "Tasks: Scroll Page Down",
                TaskCommand::Top => "Tasks: Scroll to Top",
                TaskCommand::Bottom => "Tasks: Scroll to Bottom",
                TaskCommand::FocusEditor => "Tasks: Back to Editor",
            },
        };
        s.into()
    }
}

fn column_motion_name(m: ColumnMotion) -> &'static str {
    match m {
        ColumnMotion::Left => "Left",
        ColumnMotion::Right => "Right",
        ColumnMotion::Up => "Up",
        ColumnMotion::Down => "Down",
        ColumnMotion::PageUp => "PageUp",
        ColumnMotion::PageDown => "PageDown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_round_trip() {
        let all = Command::all();
        let names: std::collections::HashSet<String> = all.iter().map(Command::name).collect();
        assert_eq!(names.len(), all.len(), "duplicate command names");
        for c in &all {
            assert_eq!(Command::from_name(&c.name()), Some(*c));
        }
        assert_eq!(Command::from_name("file.nope"), None);
    }

    /// Which variant a command is. No wildcard arm: a new variant won't compile until it's
    /// added here, and then `all_lists_every_variant` fails until it's in `Command::all()`.
    fn variant(c: Command) -> u8 {
        match c {
            Command::Move { .. } => 0,
            Command::DeleteLeft => 1,
            Command::DeleteRight => 2,
            Command::DeleteWordLeft => 3,
            Command::DeleteWordRight => 4,
            Command::DeleteToLineStart => 5,
            Command::InsertNewline => 6,
            Command::InsertTab => 7,
            Command::SelectAll => 8,
            Command::CancelSelection => 9,
            Command::Undo => 10,
            Command::Redo => 11,
            Command::Copy => 12,
            Command::Cut => 13,
            Command::Paste => 14,
            Command::Save => 15,
            Command::Quit => 16,
            Command::CloseTab => 17,
            Command::NextTab => 18,
            Command::PrevTab => 19,
            Command::ToggleSidebar => 20,
            Command::FocusExplorer => 21,
            Command::FocusGroup(_) => 22,
            Command::SplitRight => 23,
            Command::SplitDown => 24,
            Command::Explorer(_) => 25,
            Command::ShowCommands => 26,
            Command::QuickOpen => 27,
            Command::GoToLine => 28,
            Command::ManageWorkspaceTrust => 29,
            Command::OpenLogFile => 30,
            Command::SetLogLevel => 31,
            Command::Palette(_) => 32,
            Command::AddNextMatch => 33,
            Command::SelectAllMatches => 34,
            Command::SkipToNextMatch => 35,
            Command::CursorUndo => 36,
            Command::AddCursorAbove => 37,
            Command::AddCursorBelow => 38,
            Command::CursorsAtLineEnds => 39,
            Command::ColumnSelect(_) => 40,
            Command::MoveLinesUp => 41,
            Command::MoveLinesDown => 42,
            Command::CopyLinesUp => 43,
            Command::CopyLinesDown => 44,
            Command::OpenFind => 45,
            Command::OpenReplace => 46,
            Command::Find(_) => 47,
            Command::FindInFiles => 48,
            Command::Search(_) => 49,
            Command::CommentLine => 50,
            Command::OpenUserSettings => 51,
            Command::TogglePanel => 52,
            Command::ToggleProblems => 53,
            Command::Problems(_) => 54,
            Command::ShowHover => 55,
            Command::RevealDefinition => 56,
            Command::Locations(_) => 57,
            Command::TriggerSuggest => 58,
            Command::Suggest(_) => 59,
            Command::GoToReferences => 60,
            Command::RenameSymbol => 61,
            Command::FormatDocument => 62,
            Command::ReloadFromDisk => 63,
            Command::KeepMine => 64,
            Command::StageHunk => 65,
            Command::UnstageHunk => 66,
            Command::RevertHunk => 85,
            Command::NextChange => 67,
            Command::PreviousChange => 68,
            Command::CompareWithSaved => 69,
            Command::OpenChange => 70,
            Command::StageFile => 71,
            Command::UnstageFile => 72,
            Command::StageAll => 73,
            Command::UnstageAll => 74,
            Command::FocusScm => 75,
            Command::Scm(_) => 76,
            Command::ShowFileHistory => 77,
            Command::RestoreFileHistory => 78,
            Command::RunTask => 79,
            Command::KillTask => 80,
            Command::CloseTask => 81,
            Command::Task(_) => 82,
            Command::OpenRecentWorkspace => 83,
            Command::ToggleRenderWhitespace => 84,
            Command::ToggleBlame => 85,
            Command::NavBack => 86,
            Command::NavForward => 87,
            Command::OpenToolLog => 88,
            Command::ClearToolLog => 89,
            Command::ShowTools => 90,
            Command::Tools(_) => 91,
            Command::ToggleToolTrace => 92,
            Command::RestartServer => 93,
            Command::GitHistory(_) => 94,
            Command::Commit(_) => 95,
            Command::IndentLines => 96,
            Command::OutdentLines => 97,
        }
    }
    const VARIANTS: u8 = 98;

    #[test]
    fn all_lists_every_variant() {
        let all = Command::all();
        let seen: std::collections::BTreeSet<u8> = all.iter().map(|c| variant(*c)).collect();
        let missing: Vec<u8> = (0..VARIANTS).filter(|v| !seen.contains(v)).collect();
        assert!(missing.is_empty(), "Command::all() lacks variants {missing:?}");
        // And every payload: each motion both ways, each explorer and palette key.
        let count = |v: u8| all.iter().filter(|c| variant(**c) == v).count();
        assert_eq!(count(0), Motion::ALL.len() * 2);
        assert_eq!(count(25), ExplorerCommand::ALL.len());
        assert_eq!(count(32), PaletteCommand::ALL.len());
        assert_eq!(count(40), ColumnMotion::ALL.len());
        assert_eq!(count(47), FindCommand::ALL.len());
        assert_eq!(count(49), SearchCommand::ALL.len());
        assert_eq!(count(54), ProblemsCommand::ALL.len());
        assert_eq!(count(57), LocationsCommand::ALL.len());
        assert_eq!(count(59), SuggestCommand::ALL.len());
        assert_eq!(count(76), ScmCommand::ALL.len());
        assert_eq!(count(94), GitHistoryCommand::ALL.len());
        assert_eq!(count(95), CommitCommand::ALL.len());
        assert_eq!(count(82), TaskCommand::ALL.len());
        assert_eq!(count(91), ToolsCommand::ALL.len());
    }

    #[test]
    fn titles_are_unique() {
        let all = Command::all();
        let titles: std::collections::HashSet<String> = all.iter().map(Command::title).collect();
        assert_eq!(titles.len(), all.len(), "two commands share a palette title");
    }
}
