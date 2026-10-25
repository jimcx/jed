//! Movement and editing commands, applied at every selection.

use ropey::Rope;

use crate::buffer::Buffer;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::file::Eol;
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::undo::EditKind;
use crate::clipboard::Copied;
use crate::command::{Command, MergeCommand, Motion};
use crate::conflict::{self, Choice, Conflict};
use crate::indentation::Indentation;
use crate::selection::{Selection, Selections};
use crate::syntax::Language;
use crate::view::View;
use crate::wrap::{Layout, Wrap};
use crate::{column, comment, indent, lines, pairs, shift};

#[derive(Debug, Clone)]
pub struct Settings {
    /// Columns per tab stop: how wide a tab is drawn and counts in column maths (and, unless
    /// `indent_size` says otherwise, the indent size).
    pub tab_width: usize,
    /// Spaces per indent level when it differs from `tab_width` (not a user setting:
    /// `for_buffer` sets it when "Change Tab Display Size" changed a space-indented buffer's
    /// drawn tab width only, VS Code's `indentSize`). Read it through `indent_size()`.
    pub indent_size: Option<usize>,
    pub insert_spaces: bool,
    /// A buffer's guessed indentation (`indentation::guess`, kept on the `Buffer`) overrides
    /// `insert_spaces` / `tab_width` (`for_buffer`): `[settings] detect_indentation`.
    pub detect_indentation: bool,
    /// Buffers bigger than this get no parse tree (no highlighting): `[settings] syntax_max_bytes`.
    pub syntax_max_bytes: usize,
    /// Auto-close pairs, type over their closers, delete them in pairs and wrap selections
    /// (`pairs`): `[settings] auto_close_pairs`.
    pub auto_close_pairs: bool,
    /// Enter indents (keeps the line's indentation, adds a level after an opening bracket or a
    /// Python block, …) and a closing bracket typed first on a line outdents it (`indent`):
    /// `[settings] auto_indent`. Off, Enter is a plain line break.
    pub auto_indent: bool,
    /// Highlight the bracket next to each cursor and its partner (`brackets`):
    /// `[settings] match_brackets`.
    pub match_brackets: bool,
    /// Typing a word char asks the language server for completions (outside strings and
    /// comments): `[settings] quick_suggestions`.
    pub quick_suggestions: bool,
    /// Which spaces and tabs the editor draws as `·` / `→` (`ui::editor`, drawing only):
    /// `[settings] render_whitespace`. "View: Toggle Render Whitespace" overrides it for the
    /// session (`App::render_whitespace`).
    pub render_whitespace: RenderWhitespace,
    /// Whether lines wider than the editor wrap onto screen rows (`wrap`): `[settings]
    /// word_wrap`. "View: Toggle Word Wrap" overrides it for the session (`App::word_wrap`).
    pub word_wrap: WordWrap,
    /// The terminal cursor's shape (DECSCUSR, sent by `terminal::enter` and again when it
    /// changes) and secondary cursors' look (`ui::editor`): `[settings] cursor_style`.
    pub cursor_style: CursorStyle,
    /// The terminal cursor blinks: `[settings] cursor_blinking`.
    pub cursor_blinking: bool,
    /// A hex tab's widest row, in bytes (a power of two from 1 to 64: `hex::Layout::fit` takes
    /// the largest power of two up to this that fits): `[settings] hex_bytes_per_line`.
    pub hex_bytes_per_line: usize,
    /// Columns drawn as a thin vertical line behind the text (VS Code's `editor.rulers`):
    /// `[settings] rulers`. `None` (not set) takes the language's `LanguageConfig::rulers`
    /// (`for_buffer`; `git-commit`: 72); a set list, even `[]`, wins over it: jed has no
    /// language-specific user settings, so this is the only way to change or drop that 72.
    pub rulers: Option<Vec<usize>>,
    /// Lines kept above the screen for a shell tab or a task run (VS Code's
    /// `terminal.integrated.scrollback`), read when the run is created: `[settings] scrollback`.
    pub scrollback: usize,
}

/// VS Code's `editor.cursorStyle`, the shapes a terminal can draw: a bar before the char, a
/// block over it, a line under it. Secondary cursors (drawn by jed) are an underlined cell for
/// `line` / `underline`, a reversed one for `block`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Line,
    Block,
    Underline,
}

/// VS Code's `editor.wordWrap`, whether long lines wrap at the editor's width. Only `off` and
/// `on` (VS Code's `wordWrapColumn` / `bounded` have no setting here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WordWrap {
    /// No wrapping: long lines scroll sideways as before.
    #[default]
    Off,
    /// Long lines wrap onto screen rows at the editor's width.
    On,
}

/// VS Code's `editor.renderWhitespace`: which whitespace the editor draws visibly. Only U+0020
/// (as `·`) and tabs (as `→`) count, as in VS Code; other Unicode spaces (NBSP, …) are drawn as
/// themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderWhitespace {
    /// None of it.
    None,
    /// All but a single space between two non-whitespace chars: leading and trailing whitespace,
    /// tabs, runs of two or more spaces.
    Boundary,
    /// Only whitespace inside a selection.
    Selection,
    /// Only trailing whitespace (all of a blank line's).
    Trailing,
    /// All of it.
    #[default]
    All,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            indent_size: None,
            insert_spaces: true,
            detect_indentation: true,
            syntax_max_bytes: crate::syntax::DEFAULT_MAX_BYTES,
            auto_close_pairs: true,
            auto_indent: true,
            match_brackets: true,
            quick_suggestions: true,
            render_whitespace: RenderWhitespace::All,
            word_wrap: WordWrap::Off,
            cursor_style: CursorStyle::Line,
            cursor_blinking: true,
            hex_bytes_per_line: 32,
            rulers: None,
            scrollback: crate::tasks::run::SCROLLBACK,
        }
    }
}

impl Settings {
    /// The settings for a buffer in `language`: its `LanguageConfig::insert_spaces` (VS Code's
    /// language-specific default, which wins over the user's general setting) and, when the user
    /// set none, its `rulers`.
    fn for_language(&self, language: Language) -> Settings {
        let mut s = self.clone();
        if let Some(spaces) = language.config().insert_spaces {
            s.insert_spaces = spaces;
        }
        if s.rulers.is_none() {
            s.rulers = language.config().rulers.map(<[usize]>::to_vec);
        }
        s
    }

    /// The settings for `buf`, what everything reading `insert_spaces` / `tab_width` for a
    /// buffer's text uses: the language's (`for_language`), then the buffer's own indentation —
    /// the one chosen for it (`Buffer::indentation`), else, with `detect_indentation`, the
    /// guess from its text (`Buffer::guess`; not for a language that fixes `insert_spaces`, as
    /// a Makefile's tabs) — and last its tab display size (`Buffer::tab_display`: the drawn
    /// width of a tab; with spaces the indent size stays as it was, in `indent_size`).
    pub fn for_buffer(&self, buf: &Buffer) -> Settings {
        let language = buf.syntax.language();
        let mut s = self.for_language(language);
        let chosen = buf.indentation.or_else(|| {
            (s.detect_indentation && language.config().insert_spaces.is_none())
                .then(|| buf.guess().apply(s.indentation()))
        });
        if let Some(i) = chosen {
            s.insert_spaces = i.spaces;
            s.tab_width = i.size.max(1);
            s.indent_size = None;
        }
        if let Some(width) = buf.tab_display {
            if s.insert_spaces {
                s.indent_size = Some(s.indent_size());
            }
            s.tab_width = width.max(1);
        }
        s
    }

    /// Spaces per indent level (with `insert_spaces`; with tabs a level is one tab).
    pub fn indent_size(&self) -> usize {
        self.indent_size.unwrap_or(self.tab_width).max(1)
    }

    /// The indentation these settings give: what a guess falls back to (the indent size with
    /// spaces, the tab width with tabs).
    pub fn indentation(&self) -> Indentation {
        Indentation {
            spaces: self.insert_spaces,
            size: if self.insert_spaces {
                self.indent_size()
            } else {
                self.tab_width
            },
        }
    }
}

/// Runs a movement or editing command. Returns false if `cmd` isn't one.
/// The caller checks the buffer is writable before calling with an edit.
pub fn run(cmd: Command, buf: &mut Buffer, view: &mut View, s: &Settings) -> bool {
    let s = &s.for_buffer(buf);
    match cmd {
        Command::Move { motion, select } => move_cursors(buf, view, motion, select, s),
        Command::DeleteLeft => {
            if !(s.auto_close_pairs && pairs::delete_pair(buf, view)) {
                delete(buf, view, pos::prev_grapheme);
            }
        }
        Command::DeleteRight => delete(buf, view, pos::next_grapheme),
        Command::DeleteWordLeft => delete(buf, view, pos::word_left),
        Command::DeleteWordRight => delete(buf, view, pos::word_right),
        Command::DeleteToLineStart => delete(buf, view, |r, p| {
            let start = pos::line_start(r, pos::line_of(r, p));
            if p == start { pos::prev_grapheme(r, p) } else { start }
        }),
        Command::InsertNewline => indent::newline(buf, view, s),
        Command::InsertTab => {
            if !shift::tab(buf, view, s) {
                insert_tab(buf, view, s);
            }
        }
        Command::IndentLines => shift::indent(buf, view, s),
        Command::OutdentLines => shift::outdent(buf, view, s),
        Command::SelectAll => {
            view.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(buf.rope.len())));
            buf.seal_undo();
        }
        Command::CancelSelection => {
            if view.selections.len() > 1 {
                view.selections.keep_primary();
            } else {
                view.selections.transform(|s| Selection::cursor(s.head));
            }
            buf.seal_undo();
        }
        Command::Undo => restore(view, buf.undo()),
        Command::Redo => restore(view, buf.redo()),
        Command::AddCursorAbove | Command::AddCursorBelow => {
            column::add_cursor(&buf.rope, view, cmd == Command::AddCursorAbove, s.tab_width);
            buf.seal_undo();
        }
        Command::CursorsAtLineEnds => {
            column::cursors_at_line_ends(&buf.rope, view);
            buf.seal_undo();
        }
        Command::ColumnSelect(m) => {
            column::step(&buf.rope, view, m, s.tab_width);
            buf.seal_undo();
        }
        Command::MoveLinesUp => lines::move_lines(buf, view, true),
        Command::MoveLinesDown => lines::move_lines(buf, view, false),
        Command::CopyLinesUp => lines::copy_lines(buf, view, false),
        Command::CopyLinesDown => lines::copy_lines(buf, view, true),
        Command::CommentLine => comment::toggle(buf, view, s.tab_width),
        Command::Merge(m) => merge_command(buf, view, m),
        _ => return false,
    }
    true
}

pub fn is_edit(cmd: Command) -> bool {
    matches!(
        cmd,
        Command::DeleteLeft
            | Command::DeleteRight
            | Command::DeleteWordLeft
            | Command::DeleteWordRight
            | Command::DeleteToLineStart
            | Command::InsertNewline
            | Command::InsertTab
            | Command::Undo
            | Command::Redo
            | Command::MoveLinesUp
            | Command::MoveLinesDown
            | Command::CopyLinesUp
            | Command::CopyLinesDown
            | Command::CommentLine
            | Command::IndentLines
            | Command::OutdentLines
            | Command::Merge(
                MergeCommand::AcceptCurrent
                    | MergeCommand::AcceptIncoming
                    | MergeCommand::AcceptBoth
                    | MergeCommand::AcceptNone
                    | MergeCommand::AcceptAllCurrent
                    | MergeCommand::AcceptAllIncoming
                    | MergeCommand::AcceptAllBoth,
            )
    )
}

/// The conflicts `selections` touch (each selection's `start..=end` against the sorted list,
/// visited in order, each conflict once).
pub(crate) fn conflicts_touching(conflicts: &[Conflict], selections: &Selections) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = Vec::new();
    let mut ci = 0;
    for sel in selections.iter() {
        while ci < conflicts.len() && conflicts[ci].end <= sel.start() {
            ci += 1;
        }
        for conflict in &conflicts[ci..] {
            if conflict.start > sel.end() {
                break;
            }
            if out.last().is_none_or(|last| last.start != conflict.start) {
                out.push(conflict.clone());
            }
        }
    }
    out
}

/// A merge conflict command: accept `choice` at the conflicts the selections touch, or at every
/// conflict in the file for `AcceptAll*`.
fn merge_command(buf: &mut Buffer, view: &mut View, cmd: MergeCommand) {
    let choice = match cmd {
        MergeCommand::AcceptCurrent | MergeCommand::AcceptAllCurrent => Choice::Current,
        MergeCommand::AcceptIncoming | MergeCommand::AcceptAllIncoming => Choice::Incoming,
        MergeCommand::AcceptBoth | MergeCommand::AcceptAllBoth => Choice::Both,
        MergeCommand::AcceptNone => Choice::Base,
        // Not edits (`execute` runs them elsewhere).
        MergeCommand::Next | MergeCommand::Previous | MergeCommand::Compare => return,
    };
    let every = matches!(
        cmd,
        MergeCommand::AcceptAllCurrent | MergeCommand::AcceptAllIncoming | MergeCommand::AcceptAllBoth
    );
    let revision = buf.revision();
    let scanned = buf.conflicts.scan(&buf.rope, revision);
    let selected = if every {
        scanned.to_vec()
    } else {
        conflicts_touching(scanned, &view.selections)
    };
    accept_merge(buf, view, choice, &selected);
}

/// Accepts `choice` of `conflicts` as one `EditKind::Other` step: every cursor collapses onto
/// its mapped head.
pub(crate) fn accept_merge(buf: &mut Buffer, view: &mut View, choice: Choice, conflicts: &[Conflict]) {
    if conflicts.is_empty() {
        return;
    }
    let changes = conflict::accept_all(&buf.rope, conflicts, choice);
    let before = view.selections.clone();
    let mapper = changes.mapper();
    let cursors: Vec<Selection> = before
        .iter()
        .map(|sel| Selection::cursor(mapper.map(sel.head, Assoc::After)))
        .collect();
    let after = Selections::with_newest(cursors, before.primary_index(), before.newest_index());
    buf.edit(changes, before, after.clone(), EditKind::Other);
    view.selections = after;
    view.reveal_cursor = true;
}

pub fn insert_char(buf: &mut Buffer, view: &mut View, c: char, s: &Settings) {
    let s = &s.for_buffer(buf);
    if s.auto_indent && indent::type_char(buf, view, c, s) {
        return;
    }
    if s.auto_close_pairs && pairs::type_char(buf, view, c) {
        return;
    }
    let mut utf8 = [0; 4];
    let text: &str = c.encode_utf8(&mut utf8);
    replace_each(buf, view, EditKind::Insert, |_, sel| (sel.start(), sel.end(), text));
}

/// Pasted text: line endings converted to the buffer's. `whole_line` (from a copy of one
/// empty selection) puts it above each empty cursor's line, the cursor staying on its text.
/// At several cursors the text is spread, one part per cursor in order, when it splits into as
/// many parts: `pieces` (each selection's text from jed's own copy), else its lines (VS Code's
/// `multiCursorPaste: spread`). Otherwise every cursor gets all of it.
pub fn paste(buf: &mut Buffer, view: &mut View, text: &str, whole_line: bool, pieces: &[String]) {
    let crlf = buf.eol == Eol::Crlf;
    let convert = |t: &str| {
        let t = t.replace("\r\n", "\n").replace('\r', "\n");
        if crlf { t.replace('\n', "\r\n") } else { t }
    };
    let lf = text.replace("\r\n", "\n").replace('\r', "\n");
    if let Some(parts) = spread(&lf, view.selections.len(), whole_line, pieces) {
        let parts: Vec<String> = parts.into_iter().map(convert).collect();
        let mut part = parts.iter();
        replace_each(buf, view, EditKind::Other, |_, sel| {
            (sel.start(), sel.end(), part.next().map_or("", String::as_str))
        });
        return;
    }
    let text = convert(&lf);
    replace_each_landing(buf, view, EditKind::Other, |rope, sel| {
        if whole_line && sel.is_empty() {
            let start = pos::line_start(rope, pos::line_of(rope, sel.head));
            (start, start, text.as_str(), Land::Keep(sel.head))
        } else {
            (sel.start(), sel.end(), text.as_str(), Land::AfterText)
        }
    });
}

/// VS Code's `_distributePasteToCursors`: one part of `text` (newlines as `\n`) per cursor, or
/// `None`. Never for a whole-line copy or one cursor; jed's own pieces if there are `n`; else the
/// lines, ignoring one final line break, if there are `n`.
fn spread<'a>(text: &'a str, n: usize, whole_line: bool, pieces: &'a [String]) -> Option<Vec<&'a str>> {
    if whole_line || n == 1 {
        return None;
    }
    if pieces.len() == n {
        return Some(pieces.iter().map(String::as_str).collect());
    }
    let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(text).split('\n').collect();
    (lines.len() == n).then_some(lines)
}

/// What copy puts on the clipboard, newlines as `\n`. Selections are joined with newlines, each
/// one's text kept as a piece when there are several; if they're all empty, each cursor's line
/// is taken whole with its line break, once per line. Only one empty cursor's copy is marked
/// whole-line (VS Code: several cursors' lines paste like any text).
pub fn copy_text(buf: &Buffer, view: &View) -> Copied {
    let rope = &buf.rope;
    if view.selections.iter().all(|s| s.is_empty()) {
        let mut text = String::new();
        let mut last = None;
        for sel in view.selections.iter() {
            let line = pos::line_of(rope, sel.head);
            if last != Some(line) {
                last = Some(line);
                text.push_str(&pos::line_text(rope, line));
                text.push('\n');
            }
        }
        Copied {
            text,
            whole_line: view.selections.len() == 1,
            pieces: Vec::new(),
        }
    } else {
        let parts: Vec<String> = view
            .selections
            .iter()
            .map(|s| rope.slice(s.start().0..s.end().0).to_string().replace("\r\n", "\n"))
            .collect();
        Copied {
            text: parts.join("\n"),
            whole_line: false,
            pieces: if parts.len() > 1 { parts } else { Vec::new() },
        }
    }
}

/// Deletes what `copy_text` takes: the selections, or if they're all empty, each cursor's
/// whole line (the last line takes the line break before it). One undo step.
pub fn cut(buf: &mut Buffer, view: &mut View) {
    let whole_lines = view.selections.iter().all(|s| s.is_empty());
    replace_each(buf, view, EditKind::Other, |rope, sel| {
        if !whole_lines {
            return (sel.start(), sel.end(), "");
        }
        let line = pos::line_of(rope, sel.head);
        let (start, end) = if line + 1 < pos::line_count(rope) {
            (pos::line_start(rope, line), pos::line_start(rope, line + 1))
        } else if line > 0 {
            (pos::line_end(rope, line - 1), pos::line_end(rope, line))
        } else {
            (ByteIdx(0), pos::line_end(rope, line))
        };
        (start, end, "")
    });
}

fn restore(view: &mut View, sel: Option<Selections>) {
    if let Some(sel) = sel {
        view.selections = sel;
        view.reveal_cursor = true;
    }
}

fn move_cursors(buf: &mut Buffer, view: &mut View, motion: Motion, select: bool, s: &Settings) {
    let rope = &buf.rope;
    let page = view.height.max(1) as isize;
    // While the last frame wrapped, Up / Down / PageUp / PageDown move over screen rows and
    // Home / End stop on the row's edge first; the layouts are cached per line, so this stays
    // cheap at 60k selections (the lines are walked in order).
    let mut rows = view.wrapping.then_some(Rows {
        wrap: &mut view.wrap,
        revision: view.revision,
        width: view.width,
        tab_width: view.tab_width,
    });
    let mut walker = pos::LineWalker::new(rope);
    view.selections.transform(|sel| {
        if !select && !sel.is_empty() {
            match motion {
                Motion::Left => return Selection::cursor(sel.start()),
                Motion::Right => return Selection::cursor(sel.end()),
                _ => {}
            }
        }
        let (head, goal) = match motion {
            Motion::Left => (pos::prev_grapheme(rope, sel.head), None),
            Motion::Right => (pos::next_grapheme(rope, sel.head), None),
            Motion::Up => vertical(rope, sel, -1, s.tab_width, rows.as_mut(), &mut walker),
            Motion::Down => vertical(rope, sel, 1, s.tab_width, rows.as_mut(), &mut walker),
            Motion::PageUp => vertical(rope, sel, -page, s.tab_width, rows.as_mut(), &mut walker),
            Motion::PageDown => vertical(rope, sel, page, s.tab_width, rows.as_mut(), &mut walker),
            Motion::WordLeft => (pos::word_left(rope, sel.head), None),
            Motion::WordRight => (pos::word_right(rope, sel.head), None),
            Motion::LineStart => match rows.as_mut() {
                Some(rows) => line_start(rope, sel, rows, &mut walker),
                None => {
                    let line = pos::line_of(rope, sel.head);
                    let first = pos::first_non_blank(rope, line);
                    (
                        if sel.head == first {
                            pos::line_start(rope, line)
                        } else {
                            first
                        },
                        None,
                    )
                }
            },
            Motion::LineEnd => match rows.as_mut() {
                Some(rows) => line_end(rope, sel, rows, &mut walker),
                None => (pos::line_end(rope, pos::line_of(rope, sel.head)), None),
            },
            Motion::DocStart => (ByteIdx(0), None),
            Motion::DocEnd => (ByteIdx(rope.len()), None),
        };
        let anchor = if select { sel.anchor } else { head };
        Selection { anchor, head, goal }
    });
    match motion {
        Motion::PageUp => view.scroll_lines(rope, -page),
        Motion::PageDown => view.scroll_lines(rope, page),
        _ => {}
    }
    view.reveal_cursor = true;
    buf.seal_undo();
}

/// The view's wrap inputs while the last frame wrapped: `Wrap` and the revision / width / tab
/// width its layouts are cached for (drawing writes them).
struct Rows<'a> {
    wrap: &'a mut Wrap,
    revision: u64,
    width: usize,
    tab_width: usize,
}

impl Rows<'_> {
    fn layout(&mut self, rope: &Rope, line: usize) -> &Layout {
        self.wrap.layout(rope, self.revision, line, self.width, self.tab_width)
    }
}

/// `head`'s line index, its start and the head's byte offset within it. Heads arrive in order
/// (selections are sorted), so the walker steps forward instead of looking the line up.
fn line_here(walker: &mut pos::LineWalker<'_>, head: ByteIdx) -> (usize, ByteIdx, ByteIdx) {
    let line = walker.line_at(head);
    let start = line.start();
    (line.index, start, ByteIdx(head.0 - start.0))
}

/// Up/down by `delta` screen rows while the view wraps (`rows`), else by `delta` lines, keeping
/// the remembered column. Past the first row goes to the text's start, past the last to its end.
fn vertical(
    rope: &Rope,
    sel: &Selection,
    delta: isize,
    tab_width: usize,
    rows: Option<&mut Rows<'_>>,
    walker: &mut pos::LineWalker<'_>,
) -> (ByteIdx, Option<pos::DisplayCol>) {
    match rows {
        None => vertical_lines(rope, sel, delta, tab_width),
        Some(rows) => {
            let (line, start, within) = line_here(walker, sel.head);
            let (mut target, mut row, goal) = {
                let layout = rows.layout(rope, line);
                let (row, col) = layout.row_of(within);
                (line, row, sel.goal.unwrap_or(col))
            };
            let last = pos::line_count(rope) - 1;
            // Roll over the rows crossed, one line's layout at a time (a page is a few hundred
            // rows at most).
            let mut remaining = delta;
            while remaining != 0 {
                if remaining > 0 {
                    if row + 1 < rows.layout(rope, target).len() {
                        row += 1;
                        remaining -= 1;
                    } else if target < last {
                        target += 1;
                        row = 0;
                        remaining -= 1;
                    } else {
                        return (ByteIdx(rope.len()), None);
                    }
                } else if row > 0 {
                    row -= 1;
                    remaining += 1;
                } else if target > 0 {
                    target -= 1;
                    row = rows.layout(rope, target).len() - 1;
                    remaining += 1;
                } else {
                    return (ByteIdx(0), None);
                }
            }
            let target_start = if target == line {
                start
            } else {
                pos::line_start(rope, target)
            };
            let at = rows.layout(rope, target).byte_at(row, goal);
            (ByteIdx(target_start.0 + at.0), Some(goal))
        }
    }
}

/// `vertical` over lines: the behaviour without wrapping (rows off).
fn vertical_lines(rope: &Rope, sel: &Selection, delta: isize, tab_width: usize) -> (ByteIdx, Option<pos::DisplayCol>) {
    let goal = sel.goal.unwrap_or_else(|| pos::display_col(rope, sel.head, tab_width));
    let line = pos::line_of(rope, sel.head);
    let last = pos::line_count(rope) - 1;
    match line.checked_add_signed(delta) {
        None if line == 0 => (ByteIdx(0), None),
        Some(t) if t > last && line == last => (ByteIdx(rope.len()), None),
        target => {
            let target = target.unwrap_or(0).min(last);
            (pos::byte_at_col(rope, target, goal, tab_width), Some(goal))
        }
    }
}

/// Home while wrapping: the row's start first, then the line's first non-blank, then its start
/// (VS Code).
fn line_start(
    rope: &Rope,
    sel: &Selection,
    rows: &mut Rows<'_>,
    walker: &mut pos::LineWalker<'_>,
) -> (ByteIdx, Option<pos::DisplayCol>) {
    let (line, start, within) = line_here(walker, sel.head);
    let row_start = {
        let layout = rows.layout(rope, line);
        let (row, _) = layout.row_of(within);
        ByteIdx(start.0 + layout.rows()[row].start.0)
    };
    // On a continuation row, Home goes to the row's start first; on the line's first row (where
    // the row's start is the line's) and once at a row's start, it toggles the line's first
    // non-blank and start (VS Code's smart home).
    if row_start > start && sel.head != row_start {
        return (row_start, None);
    }
    let first = pos::first_non_blank(rope, line);
    (if sel.head == first { start } else { first }, None)
}

/// End while wrapping: the row's end first, then the line's end.
fn line_end(
    rope: &Rope,
    sel: &Selection,
    rows: &mut Rows<'_>,
    walker: &mut pos::LineWalker<'_>,
) -> (ByteIdx, Option<pos::DisplayCol>) {
    let (line, start, within) = line_here(walker, sel.head);
    let layout = rows.layout(rope, line);
    let (row, _) = layout.row_of(within);
    let line_end = pos::line_end(rope, line);
    // The cursor is at a row's end when it sits on a row boundary (`row_of` gives a boundary
    // byte the next row) or at the line's end; then End moves to the line's end, else to this
    // row's end (VS Code: the row's end first, then the line's).
    let at_row_end = sel.head == line_end || (row > 0 && within.0 == layout.rows()[row].start.0);
    (
        if at_row_end {
            line_end
        } else {
            ByteIdx(start.0 + layout.row_end(row).0)
        },
        None,
    )
}

/// Deletes each selection, or for an empty one, the range from `to(cursor)` to the cursor.
fn delete(buf: &mut Buffer, view: &mut View, to: impl Fn(&Rope, ByteIdx) -> ByteIdx) {
    replace_each(buf, view, EditKind::Delete, |rope, sel| {
        if sel.is_empty() {
            let other = to(rope, sel.head);
            (sel.head.min(other), sel.head.max(other), "")
        } else {
            (sel.start(), sel.end(), "")
        }
    });
}

fn insert_tab(buf: &mut Buffer, view: &mut View, s: &Settings) {
    replace_each(buf, view, EditKind::Insert, |rope, sel| {
        let text = if s.insert_spaces {
            let (col, size) = (pos::display_col(rope, sel.start(), s.tab_width).0, s.indent_size());
            " ".repeat(size - col % size)
        } else {
            "\t".to_string()
        };
        (sel.start(), sel.end(), text)
    });
}

/// Where a cursor ends up after its selection's change.
pub(crate) enum Land {
    AfterText,
    /// This many bytes into the new text (between an auto-closed pair).
    Inside(usize),
    /// Stays on the same text: a position at or after the change, before the edit.
    Keep(ByteIdx),
}

/// One transaction: replace a range per selection, cursors land after the new text.
pub(crate) fn replace_each<S: AsRef<str>>(
    buf: &mut Buffer,
    view: &mut View,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> (ByteIdx, ByteIdx, S),
) {
    replace_each_landing(buf, view, kind, |rope, sel| {
        let (start, end, text) = f(rope, sel);
        (start, end, text, Land::AfterText)
    });
}

/// `replace_each`, with each cursor's landing chosen per selection. The text is borrowed where
/// it can be (typing at 60k cursors copies one char into the set 60k times, allocating nothing).
pub(crate) fn replace_each_landing<S: AsRef<str>>(
    buf: &mut Buffer,
    view: &mut View,
    kind: EditKind,
    mut f: impl FnMut(&Rope, &Selection) -> (ByteIdx, ByteIdx, S, Land),
) {
    let before = view.selections.clone();
    // Room for a typed char per selection; longer text grows the set's string as it goes.
    let mut changes = ChangeSet::with_capacity(before.len(), before.len());
    let mut cursors = Vec::with_capacity(before.len());
    let mut prev_end = ByteIdx(0);
    let mut delta: isize = 0;
    let mut keeps = Vec::new();
    for sel in before.iter() {
        let (start, end, text, land) = f(&buf.rope, sel);
        let text = text.as_ref();
        // Expanded ranges (e.g. backspace next to a selection) can overlap; clip them.
        let start = start.max(prev_end);
        let end = end.max(start);
        prev_end = end;
        changes.push(start, end, text);
        let new_start = (start.0 as isize + delta) as usize;
        delta += text.len() as isize - (end.0 - start.0) as isize;
        cursors.push(Selection::cursor(match land {
            Land::AfterText => ByteIdx((end.0 as isize + delta) as usize),
            Land::Inside(n) => ByteIdx(new_start + n),
            Land::Keep(p) => {
                keeps.push((cursors.len(), p.max(end)));
                ByteIdx(0)
            }
        }));
    }
    // A kept position can sit after later selections' changes too (two cursors on one line
    // both paste a whole line at its start), so it goes through the whole set.
    if !keeps.is_empty() {
        let m = changes.mapper();
        for (i, p) in keeps {
            cursors[i] = Selection::cursor(m.map(p, Assoc::After));
        }
    }
    let after = Selections::with_newest(cursors, before.primary_index(), before.newest_index());
    // Auto-closed pairs follow the edit (positions only: mapped before it is applied).
    view.auto_closed.follow(buf.revision(), &changes);
    buf.edit(changes, before, after.clone(), kind);
    view.auto_closed.edited(buf.revision(), &after);
    view.selections = after;
    view.reveal_cursor = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command as C;

    const CONFLICTS: &str = "a\n\
<<<<<<< HEAD\n\
b\n\
=======\n\
c\n\
>>>>>>> main\n\
d\n\
<<<<<<< HEAD\n\
e\n\
=======\n\
f\n\
>>>>>>> main\n\
g\n";

    fn at(text: &str, needle: &str) -> ByteIdx {
        ByteIdx(text.find(needle).unwrap())
    }

    #[test]
    fn merge_commands_touch_the_conflicts_under_each_selection() {
        let mut buf = Buffer::from_str(CONFLICTS);
        let mut view = View::new();
        let revision = buf.revision();
        let conflicts = buf.conflicts.scan(&buf.rope, revision).to_vec();
        assert_eq!(conflicts.len(), 2);
        let (first, second) = (conflicts[0].start, conflicts[1].start);

        // Outside every conflict: nothing.
        view.selections = Selections::single(Selection::cursor(ByteIdx(0)));
        assert!(conflicts_touching(&conflicts, &view.selections).is_empty());
        // The byte right after a conflict's `>>>>>>>` line is outside it too (a conflict's
        // span is `[start, end)`): the cursor on the line after the markers must not accept.
        view.selections = Selections::single(Selection::cursor(conflicts[0].end));
        assert!(conflicts_touching(&conflicts, &view.selections).is_empty());
        // In the first conflict's current block, then its incoming one.
        view.selections = Selections::single(Selection::cursor(at(CONFLICTS, "b\n")));
        let got = conflicts_touching(&conflicts, &view.selections);
        assert_eq!(got.iter().map(|c| c.start).collect::<Vec<_>>(), vec![first]);
        view.selections = Selections::single(Selection::cursor(at(CONFLICTS, "c\n")));
        assert_eq!(conflicts_touching(&conflicts, &view.selections)[0].start, first);
        // A selection over both, and one cursor in each: both once, sorted.
        view.selections = Selections::single(Selection::new(at(CONFLICTS, "b\n"), at(CONFLICTS, "f\n")));
        assert_eq!(conflicts_touching(&conflicts, &view.selections).len(), 2);
        view.selections = Selections::new(
            vec![
                Selection::cursor(at(CONFLICTS, "b\n")),
                Selection::cursor(at(CONFLICTS, "f\n")),
            ],
            0,
        );
        let got = conflicts_touching(&conflicts, &view.selections);
        assert_eq!(got.iter().map(|c| c.start).collect::<Vec<_>>(), vec![first, second]);
    }

    #[test]
    fn accepting_a_merge_choice_is_one_undo_step() {
        let mut buf = Buffer::from_str(CONFLICTS);
        let mut view = View::new();
        view.selections = Selections::single(Selection::cursor(at(CONFLICTS, "b\n")));
        let before = buf.rope.to_string();
        assert!(run(
            C::Merge(MergeCommand::AcceptIncoming),
            &mut buf,
            &mut view,
            &Settings::default()
        ));
        assert_eq!(
            buf.rope.to_string(),
            "a\nc\nd\n<<<<<<< HEAD\ne\n=======\nf\n>>>>>>> main\ng\n"
        );
        assert!(buf.has_edits());
        assert!(buf.undo().is_some(), "one step");
        assert_eq!(buf.rope.to_string(), before);
        assert!(buf.undo().is_none(), "a single undo step");
        // Every conflict: both sides chosen.
        assert!(run(
            C::Merge(MergeCommand::AcceptAllBoth),
            &mut buf,
            &mut view,
            &Settings::default()
        ));
        assert_eq!(buf.rope.to_string(), "a\nb\nc\nd\ne\nf\ng\n");
        // A cursor not touching a conflict does nothing.
        view.selections = Selections::single(Selection::cursor(ByteIdx(0)));
        assert!(run(
            C::Merge(MergeCommand::AcceptCurrent),
            &mut buf,
            &mut view,
            &Settings::default()
        ));
        assert_eq!(buf.rope.to_string(), "a\nb\nc\nd\ne\nf\ng\n");
        assert!(buf.undo().is_some(), "the Accept All step");
        assert_eq!(buf.rope.to_string(), before);
    }

    #[test]
    fn a_language_default_for_rulers_applies_unless_the_user_set_some() {
        let d = Settings::default();
        assert_eq!(d.rulers, None);
        assert_eq!(d.for_language(Language::Rust).rulers, None);
        assert_eq!(d.for_language(Language::GitCommit).rulers, Some(vec![72]));
        let user = |r: Vec<usize>| Settings {
            rulers: Some(r),
            ..Settings::default()
        };
        assert_eq!(
            user(vec![80, 100]).for_language(Language::Rust).rulers,
            Some(vec![80, 100])
        );
        assert_eq!(user(vec![80]).for_language(Language::GitCommit).rulers, Some(vec![80]));
        // An explicit empty list turns git-commit's ruler off.
        assert_eq!(user(vec![]).for_language(Language::GitCommit).rulers, Some(vec![]));
    }

    fn setup(text: &str, cursor: usize) -> (Buffer, View, Settings) {
        let buf = Buffer::from_str(text);
        let mut view = View::new();
        view.selections = Selections::single(Selection::cursor(ByteIdx(cursor)));
        (buf, view, Settings::default())
    }

    fn head(view: &View) -> usize {
        view.selections.primary().head.0
    }

    fn mv(motion: Motion) -> Command {
        C::Move { motion, select: false }
    }

    #[test]
    fn a_buffers_tab_display_size_changes_only_the_drawn_width_with_spaces() {
        let mut b = Buffer::from_str("a\n");
        let s = Settings::default();
        let get = |b: &Buffer| {
            let r = s.for_buffer(b);
            (r.insert_spaces, r.indent_size(), r.tab_width, r.indentation())
        };
        let spaces = |size| Indentation { spaces: true, size };
        assert_eq!(get(&b), (true, 4, 4, spaces(4)));
        b.tab_display = Some(8);
        assert_eq!(get(&b), (true, 4, 8, spaces(4)), "spaces: the indent size stays");
        b.choose_indentation(spaces(2));
        assert_eq!(get(&b), (true, 2, 2, spaces(2)), "choosing drops the display size");
        b.tab_display = Some(6);
        assert_eq!(get(&b), (true, 2, 6, spaces(2)));
        let tabs = Indentation { spaces: false, size: 4 };
        b.choose_indentation(tabs);
        b.tab_display = Some(3);
        assert_eq!(
            get(&b),
            (false, 3, 3, Indentation { spaces: false, size: 3 }),
            "tabs: it is the tab size"
        );
        b.redetect(crate::indentation::Guess::default());
        assert_eq!((b.indentation, b.tab_display), (None, None));
        assert_eq!(get(&b), (true, 4, 4, spaces(4)));
    }

    #[test]
    fn tab_with_spaces_goes_to_the_next_indent_stop() {
        let (mut b, mut v, mut s) = setup("\tx", 1);
        s.tab_width = 8;
        s.indent_size = Some(3);
        s.detect_indentation = false; // else the (empty) guess resolves both from the settings
        run(C::InsertTab, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "\t x", "the tab reaches column 8: one space to 9");
    }

    #[test]
    fn typing_groups_into_one_undo_step() {
        let (mut b, mut v, s) = setup("", 0);
        for c in "hello".chars() {
            insert_char(&mut b, &mut v, c, &Settings::default());
        }
        assert_eq!(b.rope.to_string(), "hello");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "");
        assert!(!b.is_dirty());
        run(C::Redo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "hello");
        assert_eq!(head(&v), 5);
    }

    #[test]
    fn cursor_move_and_kind_switch_start_new_steps() {
        let (mut b, mut v, s) = setup("", 0);
        insert_char(&mut b, &mut v, 'a', &Settings::default());
        insert_char(&mut b, &mut v, 'b', &Settings::default());
        run(C::DeleteLeft, &mut b, &mut v, &s); // kind switch
        run(mv(Motion::Left), &mut b, &mut v, &s); // cursor move
        insert_char(&mut b, &mut v, 'c', &Settings::default());
        assert_eq!(b.rope.to_string(), "ca");

        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "");
    }

    #[test]
    fn paste_is_its_own_step_and_converts_eol() {
        let (mut b, mut v, s) = setup("x\r\n", 0);
        insert_char(&mut b, &mut v, 'a', &Settings::default());
        paste(&mut b, &mut v, "1\n2", false, &[]);
        insert_char(&mut b, &mut v, 'b', &Settings::default());
        assert_eq!(b.rope.to_string(), "a1\r\n2bx\r\n");
        run(C::Undo, &mut b, &mut v, &s);
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ax\r\n");
    }

    #[test]
    fn newline_uses_file_eol_and_keeps_indent() {
        let (mut b, mut v, s) = setup("    foo\r\n", 7);
        run(C::InsertNewline, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "    foo\r\n    \r\n");
        assert_eq!(head(&v), 13);
    }

    #[test]
    fn up_down_keep_goal_column() {
        let (mut b, mut v, s) = setup("abcdef\nab\nabcdef", 5);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 9);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 15);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 16);
        run(mv(Motion::Up), &mut b, &mut v, &s);
        assert_eq!(head(&v), 9, "goal column was reset by moving to the end");
    }

    /// A view as drawing leaves it while wrapping: rows laid out at `width`, a two-row page.
    fn wrapped(text: &str, cursor: usize, width: usize) -> (Buffer, View, Settings) {
        let buf = Buffer::from_str(text);
        let mut view = View::new();
        view.selections = Selections::single(Selection::cursor(ByteIdx(cursor)));
        view.wrapping = true;
        view.width = width;
        view.height = 2;
        view.revision = buf.revision();
        view.tab_width = 4;
        (buf, view, Settings::default())
    }

    #[test]
    fn wrapped_up_down_move_over_rows_keeping_the_row_column() {
        // "abcdefghij" at 5 columns: rows 0..5 and 5..10; byte 7 is row 1, column 2.
        let (mut b, mut v, s) = wrapped("abcdefghij", 7, 5);
        run(mv(Motion::Up), &mut b, &mut v, &s);
        assert_eq!(head(&v), 2, "up to the row above, at its column 2");
        assert_eq!(v.selections.primary().goal, Some(pos::DisplayCol(2)));
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 7, "down keeps the goal column");
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 10, "past the last row goes to the text's end");
        assert_eq!(v.selections.primary().goal, None);
    }

    #[test]
    fn wrapped_vertical_crosses_lines_and_clamps_at_the_edges() {
        let (mut b, mut v, s) = wrapped("abcde\nfghij", 0, 5);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 6, "the next line's first row");
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 11, "past the last row goes to the text's end");
        assert_eq!(v.selections.primary().goal, None);
        run(mv(Motion::Up), &mut b, &mut v, &s);
        assert_eq!(head(&v), 5, "up from the end enters the line above, at its end");
        run(mv(Motion::Up), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0, "past the first row goes to the text's start");
        assert_eq!(v.selections.primary().goal, None);
    }

    #[test]
    fn wrapped_page_moves_by_rows() {
        // Four rows of 5 columns; a page is the view's two rows.
        let (mut b, mut v, s) = wrapped("abcdefghijklmnopqrst", 2, 5);
        run(mv(Motion::PageDown), &mut b, &mut v, &s);
        assert_eq!(head(&v), 12, "two rows down, still at column 2");
        run(mv(Motion::PageUp), &mut b, &mut v, &s);
        assert_eq!(head(&v), 2, "two rows up");
    }

    #[test]
    fn wrapped_home_and_end_stop_on_the_row_first() {
        // "    aaaa bbbb cccc" at 10 columns breaks after the spaces: rows [0, 9), [9, 14)
        // and [14, 18), each continuation row at the indentation's column 4.
        let (mut b, mut v, s) = wrapped("    aaaa bbbb cccc", 18, 10);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 14, "home goes to the row's start first");
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 4, "then to the line's first non-blank");
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0, "then to the line's start");
        run(mv(Motion::LineEnd), &mut b, &mut v, &s);
        assert_eq!(head(&v), 9, "end goes to the row's end first");
        run(mv(Motion::LineEnd), &mut b, &mut v, &s);
        assert_eq!(head(&v), 18, "then to the line's end, not the next row's");
        run(mv(Motion::LineEnd), &mut b, &mut v, &s);
        assert_eq!(head(&v), 18, "and stays there");
    }

    #[test]
    fn wrapped_home_on_the_first_row_toggles_first_non_blank() {
        // A line that doesn't wrap (a single row) still gets VS Code's smart home: first
        // non-blank, then the start — not straight to the start as if it were a continuation row.
        let (mut b, mut v, s) = wrapped("    foo", 6, 20);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 4, "home goes to the first non-blank, not the line start");
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0, "then to the line start");
    }

    #[test]
    fn wrapped_end_from_a_continuation_row_stops_at_that_rows_end() {
        // End in the middle of a continuation row ends that row; only a second End reaches the
        // line's end (VS Code's "End twice").
        let (mut b, mut v, s) = wrapped("    aaaa bbbb cccc", 11, 10);
        run(mv(Motion::LineEnd), &mut b, &mut v, &s);
        assert_eq!(head(&v), 14, "end stops at this row's end");
        run(mv(Motion::LineEnd), &mut b, &mut v, &s);
        assert_eq!(head(&v), 18, "then at the line's end");
    }

    #[test]
    fn wrapped_down_clamps_to_a_short_rows_end_keeping_the_goal() {
        // "abcdefghij" wraps to rows [0,5) and [5,10); "ab" below is one short row.
        let (mut b, mut v, s) = wrapped("abcdefghij\nab", 7, 5);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 13, "down to the short line clamps to its end");
        assert_eq!(
            v.selections.primary().goal,
            Some(pos::DisplayCol(2)),
            "goal column kept"
        );
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(head(&v), 13, "past the last row is still the text's end");
        assert_eq!(v.selections.primary().goal, None);
    }

    #[test]
    fn wrapped_movement_applies_at_every_selection() {
        // Two cursors on the first line (the walker returns the same line for both) and one on
        // the second: every selection moves one row down at its own column.
        let (mut b, mut v, s) = wrapped("abcdefghij\nabcdefghij\n", 1, 5);
        let at = |p| Selection::cursor(ByteIdx(p));
        v.selections = Selections::new(vec![at(1), at(3), at(12)], 0);
        run(mv(Motion::Down), &mut b, &mut v, &s);
        assert_eq!(cursors(&v), vec![6, 8, 17], "each cursor moved one row down");
    }

    #[test]
    fn backspace_deletes_whole_crlf_and_grapheme() {
        let (mut b, mut v, s) = setup("a\r\ne\u{301}", 6);
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a\r\n");
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a");
    }

    #[test]
    fn edits_apply_at_every_cursor() {
        let (mut b, mut v, s) = setup("ab\ncd\nef", 0);
        v.selections = Selections::new((0..3).map(|l| Selection::cursor(ByteIdx(l * 3 + 1))).collect(), 0);
        insert_char(&mut b, &mut v, 'X', &Settings::default());
        assert_eq!(b.rope.to_string(), "aXb\ncXd\neXf");
        run(C::DeleteLeft, &mut b, &mut v, &s);
        run(C::DeleteLeft, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "b\nd\nf");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "aXb\ncXd\neXf");
        assert_eq!(v.selections.len(), 3);
    }

    #[test]
    fn copy_joins_selections_and_takes_whole_lines_when_empty() {
        let (b, mut v, _) = setup("ab\r\ncd\r\nef", 0);
        v.selections = Selections::new(
            vec![
                Selection::new(ByteIdx(1), ByteIdx(5)),
                Selection::new(ByteIdx(8), ByteIdx(9)),
            ],
            0,
        );
        let c = copy_text(&b, &v);
        assert_eq!((c.text.as_str(), c.whole_line), ("b\nc\ne", false));
        assert_eq!(c.pieces, vec!["b\nc", "e"], "each selection's text, CRLF as LF");
        // Two cursors on line 0 copy it once; the last line gets a line break added.
        v.selections = Selections::new([0, 1, 9].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        let c = copy_text(&b, &v);
        assert_eq!(
            (c.text.as_str(), c.whole_line),
            ("ab\nef\n", false),
            "whole line only for one cursor"
        );
        assert!(c.pieces.is_empty());
        v.selections = Selections::single(Selection::cursor(ByteIdx(9)));
        let c = copy_text(&b, &v);
        assert_eq!((c.text.as_str(), c.whole_line), ("ef\n", true));
    }

    #[test]
    fn cut_whole_lines_like_vs_code() {
        // Middle line: the line and its break go, cursor to the start of the next line.
        let (mut b, mut v, s) = setup("ab\ncd\nef", 4);
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "ab\nef");
        assert_eq!(head(&v), 3);
        // Last line: takes the break before it, cursor to the end of the line above.
        v.selections = Selections::single(Selection::cursor(ByteIdx(4)));
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "ab");
        assert_eq!(head(&v), 2);
        // Only line: emptied.
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "");
        // Each cut is one undo step.
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab");
        run(C::Undo, &mut b, &mut v, &s);
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab\ncd\nef");
        assert_eq!(head(&v), 4);
    }

    #[test]
    fn cut_selection_leaves_empty_cursors_alone() {
        let (mut b, mut v, _) = setup("abc\ndef", 0);
        v.selections = Selections::new(
            vec![Selection::new(ByteIdx(0), ByteIdx(2)), Selection::cursor(ByteIdx(5))],
            0,
        );
        cut(&mut b, &mut v);
        assert_eq!(b.rope.to_string(), "c\ndef");
    }

    #[test]
    fn whole_line_paste_goes_above_and_keeps_cursor_on_its_text() {
        let (mut b, mut v, s) = setup("ab\r\ncd", 6);
        paste(&mut b, &mut v, "x\n", true, &[]);
        assert_eq!(b.rope.to_string(), "ab\r\nx\r\ncd");
        assert_eq!(head(&v), 9, "still after 'c'");
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "ab\r\ncd");
        // With a selection, a whole-line paste just replaces it.
        v.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(2)));
        paste(&mut b, &mut v, "x\n", true, &[]);
        assert_eq!(b.rope.to_string(), "x\r\n\r\ncd");
    }

    /// Two cursors on one line both paste the line above it: each stays on its own text, past
    /// both pasted lines (it used to count only its own, landing inside the `界`).
    #[test]
    fn whole_line_paste_with_two_cursors_on_a_line_keeps_both() {
        let (mut b, mut v, _) = setup("a界b界", 0);
        v.selections = Selections::new(vec![Selection::cursor(ByteIdx(1)), Selection::cursor(ByteIdx(5))], 0);
        paste(&mut b, &mut v, "z\n", true, &[]);
        assert_eq!(b.rope.to_string(), "z\nz\na界b界");
        let heads: Vec<usize> = v.selections.iter().map(|s| s.head.0).collect();
        assert_eq!(heads, vec![5, 9], "still after 'a' and after 'b'");
    }

    fn cursors(v: &View) -> Vec<usize> {
        v.selections.iter().map(|s| s.head.0).collect()
    }

    #[test]
    fn paste_spreads_one_line_per_cursor_when_counts_match() {
        let (mut b, mut v, s) = setup("a\r\nb\r\nc", 0);
        v.selections = Selections::new([1, 4, 7].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        // A final line break doesn't count as another line; terminal newlines may be CRs.
        paste(&mut b, &mut v, "1\r2\r\n3\n", false, &[]);
        assert_eq!(b.rope.to_string(), "a1\r\nb2\r\nc3");
        assert_eq!(cursors(&v), vec![2, 6, 10]);
        run(C::Undo, &mut b, &mut v, &s);
        assert_eq!(b.rope.to_string(), "a\r\nb\r\nc", "one undo step");
        // Two lines, three cursors: all of it at each.
        paste(&mut b, &mut v, "x\ny", false, &[]);
        assert_eq!(b.rope.to_string(), "ax\r\ny\r\nbx\r\ny\r\ncx\r\ny");
    }

    #[test]
    fn paste_spreads_copied_pieces_even_with_line_breaks_inside() {
        let (mut b, mut v, _) = setup("ab", 0);
        v.selections = Selections::new([0, 2].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        let pieces = ["x\ny".to_string(), "z".to_string()];
        paste(&mut b, &mut v, "x\ny\nz", false, &pieces);
        assert_eq!(b.rope.to_string(), "x\nyabz");
        assert_eq!(cursors(&v), vec![3, 6]);
        // Pieces for another count are ignored: three lines at two cursors don't spread.
        let (mut b, mut v, _) = setup("ab", 0);
        v.selections = Selections::new([0, 2].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        paste(&mut b, &mut v, "1\n2\n3", false, &pieces[..1]);
        assert_eq!(b.rope.to_string(), "1\n2\n3ab1\n2\n3");
    }

    #[test]
    fn whole_line_and_single_cursor_pastes_never_spread() {
        let (mut b, mut v, _) = setup("a\nb", 0);
        v.selections = Selections::new([0, 2].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
        paste(&mut b, &mut v, "x\n", true, &[]);
        assert_eq!(b.rope.to_string(), "x\na\nx\nb", "above each line");
        let (mut b, mut v, _) = setup("a", 1);
        paste(&mut b, &mut v, "x\n", false, &[]);
        assert_eq!(b.rope.to_string(), "ax\n");
    }

    #[test]
    fn home_toggles_first_non_blank() {
        let (mut b, mut v, s) = setup("    foo", 6);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 4);
        run(mv(Motion::LineStart), &mut b, &mut v, &s);
        assert_eq!(head(&v), 0);
    }

    /// Seeded xorshift: `next(n)` is in `0..n`.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Random sorted selections on grapheme boundaries of `text`: cursors (often at 0, so some
    /// deletes are empty changes), forward and backward ranges, some touching.
    fn random_selections(rng: &mut Rng, text: &str, n: usize) -> Selections {
        use unicode_segmentation::UnicodeSegmentation;
        let bounds: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        let mut list = Vec::new();
        let mut at = 0;
        for _ in 0..n {
            if at >= bounds.len() {
                break;
            }
            let end = (at + rng.next(3)).min(bounds.len() - 1);
            let (a, h) = (ByteIdx(bounds[at]), ByteIdx(bounds[end]));
            list.push(if rng.next(2) == 0 {
                Selection::new(a, h)
            } else {
                Selection::new(h, a)
            });
            at = end + rng.next(4);
        }
        let primary = rng.next(list.len());
        Selections::new(list, primary)
    }

    /// The grapheme boundary before / after `pos` in `text` (or `pos` at either end).
    fn grapheme_step(text: &str, pos: usize, left: bool) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let mut bounds = text.grapheme_indices(true).map(|(i, _)| i).chain([text.len()]);
        if left {
            bounds.take_while(|&b| b < pos).last().unwrap_or(pos)
        } else {
            bounds.find(|&b| b > pos).unwrap_or(pos)
        }
    }

    /// Multi-cursor typing, pasting, deleting and cutting on multibyte text (CJK, emoji,
    /// combining marks, CRLF) against a `String` model: the text and the cursors after each
    /// edit, a second view mapped through every edit staying on char boundaries, then undo all
    /// the way back to the original and redo all the way forward through the same texts. Small
    /// rounds use tree edits; the 300-cursor rounds the one-pass rebuild.
    #[test]
    fn random_multi_cursor_edits_undo_and_redo_match_a_model() {
        const PIECES: &[&str] = &["a", "bc", "é", "e\u{301}", "界", "😀", "\n", "\r\n", " "];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let s = Settings::default();
        for round in 0..60 {
            let (len, cursors) = if round % 10 == 9 { (3000, 300) } else { (120, 6) };
            let mut text = String::new();
            while text.len() < len {
                text.push_str(PIECES[rng.next(PIECES.len())]);
            }
            let mut b = Buffer::from_str(&text);
            let mut v = View::new();
            let mut other = View::new();
            other.selections = random_selections(&mut rng, &text, cursors);
            let mut snapshots = vec![text.clone()];
            for step in 0..8 {
                if step == 0 || rng.next(3) > 0 {
                    v.selections = random_selections(&mut rng, &text, cursors);
                }
                // Expected (start, end, new text) per selection, or None: checked by snapshot only.
                let sels: Vec<Selection> = v.selections.iter().copied().collect();
                let op = rng.next(6);
                let expect: Option<Vec<(usize, usize, &str)>> = match op {
                    0 => {
                        let c = ['x', 'é', '\u{301}', '界', '😀'][rng.next(5)];
                        insert_char(&mut b, &mut v, c, &Settings::default());
                        let t: &'static str = ["x", "é", "\u{301}", "界", "😀"]
                            [['x', 'é', '\u{301}', '界', '😀'].iter().position(|&k| k == c).unwrap()];
                        Some(sels.iter().map(|x| (x.start().0, x.end().0, t)).collect())
                    }
                    1 => {
                        let t = ["ab", "é界", "😀", "e\u{301}"][rng.next(4)];
                        paste(&mut b, &mut v, t, false, &[]);
                        Some(sels.iter().map(|x| (x.start().0, x.end().0, t)).collect())
                    }
                    2 | 3 => {
                        let left = op == 2;
                        run(if left { C::DeleteLeft } else { C::DeleteRight }, &mut b, &mut v, &s);
                        Some(
                            sels.iter()
                                .map(|x| {
                                    if !x.is_empty() {
                                        (x.start().0, x.end().0, "")
                                    } else if left {
                                        (grapheme_step(&text, x.head.0, true), x.head.0, "")
                                    } else {
                                        (x.head.0, grapheme_step(&text, x.head.0, false), "")
                                    }
                                })
                                .collect(),
                        )
                    }
                    4 => {
                        cut(&mut b, &mut v);
                        None
                    }
                    _ => {
                        paste(&mut b, &mut v, "z\n", true, &[]);
                        None
                    }
                };
                if let Some(expect) = expect {
                    let (mut prev_end, mut delta, mut cursors) = (0, 0isize, Vec::new());
                    let mut new = String::new();
                    for (start, end, t) in expect {
                        let start = start.max(prev_end);
                        let end = end.max(start);
                        new.push_str(&text[prev_end..start]);
                        new.push_str(t);
                        prev_end = end;
                        delta += t.len() as isize - (end - start) as isize;
                        let c = (end as isize + delta) as usize;
                        if cursors.last() != Some(&c) {
                            cursors.push(c);
                        }
                    }
                    new.push_str(&text[prev_end..]);
                    assert_eq!(b.rope.to_string(), new, "round {round} step {step} op {op}");
                    let got: Vec<usize> = v.selections.iter().map(|x| x.head.0).collect();
                    assert!(v.selections.iter().all(|x| x.is_empty()));
                    assert_eq!(got, cursors, "round {round} step {step} op {op}: cursors");
                }
                text = b.rope.to_string();
                other.map_through(&b.take_applied(), &b.rope, b.revision());
                for x in other.selections.iter() {
                    for p in [x.anchor.0, x.head.0] {
                        assert!(text.is_char_boundary(p), "round {round} step {step}: other view at {p}");
                    }
                }
                if snapshots.last() != Some(&text) {
                    snapshots.push(text.clone());
                }
            }
            // Undo to the start: every text undo passes through was a snapshot, in reverse order.
            let mut undone = vec![b.rope.to_string()];
            while b.undo().is_some() {
                undone.push(b.rope.to_string());
                other.map_through(&b.take_applied(), &b.rope, b.revision());
            }
            assert_eq!(undone.last(), snapshots.first(), "round {round}: undo to the original");
            let mut rest = snapshots.iter().rev();
            for t in &undone {
                assert!(
                    rest.any(|snap| snap == t),
                    "round {round}: undo produced a text never seen"
                );
            }
            // Redo to the end through the same texts, forward.
            let mut redone = vec![b.rope.to_string()];
            while b.redo().is_some() {
                redone.push(b.rope.to_string());
                other.map_through(&b.take_applied(), &b.rope, b.revision());
            }
            undone.reverse();
            assert_eq!(redone, undone, "round {round}: redo retraces undo");
            let len = b.rope.len();
            assert!(other.selections.iter().all(|x| x.head.0 <= len && x.anchor.0 <= len));
        }
    }
}
