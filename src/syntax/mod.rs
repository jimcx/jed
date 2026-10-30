//! Language detection and tree-sitter parse trees, one `Syntax` per buffer (`Buffer::syntax`).
//!
//! The first parse of a buffer, and re-parses after big edits, run off the UI thread (`ParseJob`,
//! reported as `AppEvent::Parsed`); a small edit is re-parsed incrementally on the UI thread by
//! `Syntax::update`, which the event loop calls once per turn. Every change set the buffer applies
//! reaches `Syntax::edit` (from `Buffer::edit` / `undo` / `redo`), which turns it into tree-sitter
//! `InputEdit`s. `tree()` is only `Some` when the tree matches the text exactly.
//!
//! `spans(rope, lines)` gives the highlight spans for any lines, from a cache that is only
//! re-queried when the tree changes (see `highlight`).

pub mod authoring;
pub mod build;
mod config;
mod highlight;
mod language;
pub mod plugin;
mod tokens;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ropey::Rope;
use tree_sitter::{InputEdit, ParseOptions, Parser, Point, Tree};

pub use config::{AutoPair, LanguageConfig, is_quote};
pub use highlight::{KEYS, Span};
pub use language::{Associations, Language, detect, detect_path, detect_shebang};
pub use tokens::Tokens;

use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx};

/// Default for `[settings] syntax_max_bytes`: no tree for bigger buffers.
pub const DEFAULT_MAX_BYTES: usize = 8 << 20;

/// Pending edits up to this many changes, touching up to `INLINE_BYTES`, are re-parsed on the UI
/// thread; more go to a background parse (the tree is withheld meanwhile).
const INLINE_EDITS: usize = 2048;
const INLINE_BYTES: usize = 64 << 10;
/// An incremental re-parse on the UI thread that takes longer than this stops and continues in
/// the background (1 MB of C++ on a slow x86: 1-4 ms a keystroke, ~11 ms for an unbalanced
/// bracket; a full re-parse is ~600 ms). With the highlight query and the frame (~3 ms) on top,
/// a key's worst frame stays near 10 ms; a re-parse that stops keeps drawing the last colours.
const INLINE_TIME: Duration = Duration::from_millis(8);
/// A change set with more changes than this becomes one `InputEdit` spanning them all, or, when
/// they span more than `INLINE_BYTES`, drops the tree for a fresh background parse (63k cursors
/// in 1 MB: an edit each costs ~100 ms on the UI thread, one wide edit ~20 ms, dropping it ~0).
const COALESCE_AT: usize = 256;
/// While a background parse runs, edits for its result queue up to this many; past that it is
/// cancelled and restarted on the current text. Its result takes them on the UI thread, ~5 µs
/// each in 1 MB of C++ (`Tree::edit` scans the children of every node it passes).
const MAX_QUEUED: usize = 4096;
const SLOW_TASK: Duration = Duration::from_millis(500);

/// See `Syntax::scope_before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Code,
    String,
    Comment,
}

/// One buffer's language and parse tree.
#[derive(Default)]
pub struct Syntax {
    language: Language,
    state: State,
    /// For re-parses on the UI thread (created on first use).
    parser: Option<Parser>,
    /// Numbers background parses, so a superseded result is dropped.
    generation: u64,
    /// A tree thrown away by `edit`, freed by the next `ParseJob` (off the UI thread: freeing
    /// 1 MB of C++'s tree takes ~20 ms).
    discard: Option<Tree>,
    /// Bumped whenever a new current tree is in place: what cached highlights are keyed on.
    tree_version: u64,
    highlights: highlight::Highlights,
}

#[derive(Default)]
enum State {
    /// No grammar.
    #[default]
    Off,
    /// The text is over the size limit: no tree.
    TooLarge,
    /// Needs a full parse; `update` starts one.
    Wanted,
    /// A background parse of the text as it was; `edits` came since and are applied to its result.
    Parsing {
        generation: u64,
        edits: Vec<InputEdit>,
        bytes: usize,
        _cancel: Cancel,
    },
    /// `tree` with `edits` (touching `bytes`) applied since it was parsed: current when 0.
    Ready { tree: Tree, edits: usize, bytes: usize },
}

impl std::fmt::Debug for Syntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match &self.state {
            State::Off => "off",
            State::TooLarge => "too large",
            State::Wanted => "wanted",
            State::Parsing { .. } => "parsing",
            State::Ready { edits: 0, .. } => "ready",
            State::Ready { .. } => "edited",
        };
        f.debug_struct("Syntax")
            .field("language", &self.language)
            .field("state", &state)
            .finish()
    }
}

/// Sets the flag when dropped: replacing a `Parsing` state stops its parse.
struct Cancel(Arc<AtomicBool>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A parse to run off the UI thread.
pub struct ParseJob {
    language: Language,
    rope: Rope,
    /// The edited previous tree, for an incremental parse.
    old_tree: Option<Tree>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    /// A thrown-away tree, freed here.
    discard: Option<Tree>,
}

/// A finished `ParseJob` (`tree` is `None` if it was cancelled).
pub struct Parsed {
    generation: u64,
    tree: Option<Tree>,
}

impl Syntax {
    pub fn language(&self) -> Language {
        self.language
    }

    /// Switches language (dropping the tree); the next `update` parses with the new grammar.
    pub fn set_language(&mut self, language: Language) {
        if language == self.language {
            return;
        }
        self.language = language;
        self.parser = None;
        self.highlights.clear();
        self.state = if language.grammar().is_some() {
            State::Wanted
        } else {
            State::Off
        };
    }

    /// A plugin's grammar was installed (`plugin::load`): a buffer left `Off` for want of it is
    /// now `Wanted`.
    pub fn grammar_installed(&mut self) {
        if matches!(self.state, State::Off) && self.language.grammar().is_some() {
            self.state = State::Wanted;
        }
    }

    /// The parse tree, only while it matches the text exactly.
    pub fn tree(&self) -> Option<&Tree> {
        current(&self.state)
    }

    /// The current tree when its leaves are the language's tokens (`LanguageConfig::tree_tokens`:
    /// not Markdown's): what bracket matching and auto-indent read brackets from. `None` without
    /// one, and they scan the text instead.
    pub fn token_tree(&self) -> Option<&Tree> {
        self.tree().filter(|_| self.language.config().tree_tokens)
    }

    /// A walk over the `token_tree`'s tokens (auto-indent, bracket matching); `None` without one.
    pub fn tokens(&self) -> Option<Tokens<'_>> {
        self.token_tree().map(Tokens::new)
    }

    /// What the char before `pos` is part of: a string, a comment or code (auto-close's `notIn`,
    /// as VS Code reads the token before the cursor). `None` without a current tree. The
    /// smallest node holding that char and its ancestors are checked (`scope_of`), up to a node
    /// that is code again inside a string (an f-string's `interpolation`, a shell `$(…)`).
    pub fn scope_before(&self, pos: ByteIdx) -> Option<Scope> {
        let tree = current(&self.state)?;
        if pos.0 == 0 {
            return Some(Scope::Code);
        }
        let mut node = tree.root_node().descendant_for_byte_range(pos.0 - 1, pos.0);
        while let Some(n) = node {
            match self.language.scope_of(n.kind()) {
                Some(scope) => return Some(scope),
                None => node = n.parent(),
            }
        }
        Some(Scope::Code)
    }

    /// The highlight spans on lines `lines` of `rope` (the buffer's text): sorted, non-overlapping,
    /// possibly reaching past the lines. Exact while the tree is current (cached per tree, the
    /// lines plus `highlight::MARGIN` queried on a miss; very long lines only for their first
    /// 16 KB); while a re-parse runs, the last spans mapped through the edits since; none before
    /// the first tree, or without one.
    pub fn spans(&mut self, rope: &Rope, lines: std::ops::Range<usize>) -> &[Span] {
        let tree = current(&self.state);
        self.highlights
            .spans(self.language, tree, self.tree_version, rope, lines)
    }

    /// Follows every change set the buffer applies (`Buffer::apply`), so the spans kept for
    /// drawing while a re-parse runs stay on the same text.
    pub fn map_highlights(&mut self, changes: &ChangeSet) {
        self.highlights.edit(changes);
    }

    /// How many highlight queries have run (the cache's misses).
    #[cfg(test)]
    pub fn highlight_queries_run(&self) -> usize {
        self.highlights.computed
    }

    /// The text is over `[settings] syntax_max_bytes`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn too_large(&self) -> bool {
        matches!(self.state, State::TooLarge)
    }

    /// Whether `edit` needs the text from before each change set.
    pub fn wants_edits(&self) -> bool {
        matches!(self.state, State::Parsing { .. } | State::Ready { .. })
    }

    /// Follows a change set the buffer just applied: `old` is the text before it, `new` after.
    pub fn edit(&mut self, changes: &ChangeSet, old: &Rope, new: &Rope) {
        if !self.wants_edits() {
            return;
        }
        if changes.len() > COALESCE_AT && span(changes) > INLINE_BYTES {
            // Editing a tree over a wide range copies every node in it (20 ms for 1 MB of C++
            // with 63k cursors), and the re-parse gains nothing from it: start over. Dropping a
            // `Parsing` state cancels its parse.
            if let State::Ready { tree, .. } = std::mem::replace(&mut self.state, State::Wanted) {
                self.discard = Some(tree);
            }
            return;
        }
        match &mut self.state {
            State::Ready { tree, edits, bytes } => input_edits(changes, old, new, |e| {
                *bytes += edit_bytes(&e);
                tree.edit(&e);
                *edits += 1;
            }),
            State::Parsing { edits, bytes, .. } => {
                input_edits(changes, old, new, |e| {
                    *bytes += edit_bytes(&e);
                    edits.push(e);
                });
                if edits.len() > MAX_QUEUED {
                    self.state = State::Wanted;
                }
            }
            _ => {}
        }
    }

    /// Brings the tree up to date with `rope` (the buffer's text): re-parses small edits here,
    /// or returns the background parse to start (the initial one, or after big edits). No tree
    /// past `max_bytes` (0: never).
    pub fn update(&mut self, rope: &Rope, max_bytes: usize) -> Option<ParseJob> {
        if matches!(self.state, State::Off) {
            return None;
        }
        if max_bytes == 0 || rope.len() > max_bytes {
            if !matches!(self.state, State::TooLarge) {
                tracing::info!(
                    bytes = rope.len(),
                    max_bytes,
                    "over the syntax size limit: no parse tree"
                );
                self.state = State::TooLarge;
                self.highlights.clear();
            }
            return None;
        }
        let old_tree = match std::mem::take(&mut self.state) {
            State::Ready { tree, edits: 0, .. } => {
                self.state = State::Ready {
                    tree,
                    edits: 0,
                    bytes: 0,
                };
                return None;
            }
            State::Ready { tree, edits, bytes } if edits <= INLINE_EDITS && bytes <= INLINE_BYTES => {
                match self.reparse(rope, &tree) {
                    Some(tree) => {
                        self.state = State::Ready {
                            tree,
                            edits: 0,
                            bytes: 0,
                        };
                        self.tree_version += 1;
                        return None;
                    }
                    // Too slow for a frame: finish in the background.
                    None => Some(tree),
                }
            }
            State::Ready { tree, .. } => Some(tree),
            s @ State::Parsing { .. } => {
                self.state = s;
                return None;
            }
            _ => None,
        };
        self.generation += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.state = State::Parsing {
            generation: self.generation,
            edits: Vec::new(),
            bytes: 0,
            _cancel: Cancel(cancel.clone()),
        };
        Some(ParseJob {
            language: self.language,
            rope: rope.clone(),
            old_tree,
            generation: self.generation,
            cancel,
            discard: self.discard.take(),
        })
    }

    /// Takes a background parse's result: unless superseded, the edits made since it started are
    /// applied to its tree (re-parsed by the next `update`).
    pub fn parsed(&mut self, parsed: Parsed) {
        let State::Parsing { generation, .. } = &self.state else {
            return;
        };
        if *generation != parsed.generation {
            return;
        }
        let State::Parsing { edits, bytes, .. } = std::mem::take(&mut self.state) else {
            unreachable!()
        };
        self.state = match parsed.tree {
            Some(mut tree) => {
                for e in &edits {
                    tree.edit(e);
                }
                if edits.is_empty() {
                    self.tree_version += 1;
                }
                State::Ready {
                    tree,
                    edits: edits.len(),
                    bytes,
                }
            }
            None => State::Wanted,
        };
    }

    /// Incremental re-parse of the edited `tree` on this thread; `None` if it would take longer
    /// than `INLINE_TIME` (typing `/*` at the top of a big file re-parses all of it).
    fn reparse(&mut self, rope: &Rope, tree: &Tree) -> Option<Tree> {
        let t = Instant::now();
        let language = self.language;
        let parser = self.parser.get_or_insert_with(|| new_parser(language));
        let new = parse(parser, rope, Some(tree), Stop::After(t + INLINE_TIME));
        let elapsed_us = t.elapsed().as_micros() as u64;
        match &new {
            Some(_) => tracing::debug!(elapsed_us, "incremental parse"),
            None => tracing::debug!(elapsed_us, "incremental parse too slow: continuing in the background"),
        }
        new
    }
}

impl ParseJob {
    /// Blocking: call from spawn_blocking.
    pub fn run(mut self) -> Parsed {
        drop(self.discard.take());
        let t = Instant::now();
        if self.cancel.load(Ordering::Relaxed) {
            return Parsed {
                generation: self.generation,
                tree: None,
            };
        }
        let mut parser = new_parser(self.language);
        let tree = parse(
            &mut parser,
            &self.rope,
            self.old_tree.as_ref(),
            Stop::Cancel(&self.cancel),
        );
        // Cancelled after it finished: it would be dropped on the UI thread.
        let tree = tree.filter(|_| !self.cancel.load(Ordering::Relaxed));
        // The first frame with the tree draws colours: compile the query here, not there.
        highlight::warm(self.language);
        let elapsed = t.elapsed();
        let (ms, bytes) = (elapsed.as_millis() as u64, self.rope.len());
        match &tree {
            None => tracing::debug!(elapsed_ms = ms, "parse cancelled"),
            Some(_) if elapsed > SLOW_TASK => tracing::warn!(elapsed_ms = ms, bytes, "slow parse"),
            Some(_) => tracing::debug!(elapsed_ms = ms, bytes, incremental = self.old_tree.is_some(), "parsed"),
        }
        Parsed {
            generation: self.generation,
            tree,
        }
    }
}

/// Blocking: the highlight spans of a whole `text` in `language` (sorted, non-overlapping byte
/// ranges of `text`), from one full parse; none without a grammar. For texts no buffer holds
/// (a diff tab's sides): the caller applies the size limit.
pub fn highlight_text(language: Language, text: &str) -> Vec<Span> {
    if language.grammar().is_none() || text.is_empty() {
        return Vec::new();
    }
    let rope = Rope::from_str(text);
    let mut parser = new_parser(language);
    let never = AtomicBool::new(false);
    let Some(tree) = parse(&mut parser, &rope, None, Stop::Cancel(&never)) else {
        return Vec::new();
    };
    highlight::compute(language, &tree, &rope, ByteIdx(0), ByteIdx(rope.len()))
}

/// The scope a node of this kind sets for the text in it, across jed's grammars; `None`: its
/// parent's. Comments: any kind naming one (`comment`, Rust's `line_comment`, CMake's
/// `bracket_comment`, XML's `Comment`). Strings: kinds naming one (`string_literal`, `raw_string`,
/// YAML's plain `string_scalar`, …, but not the Dockerfile's `unquoted_string`) and the grammars'
/// other quoted kinds. Code: the parts of a string that are code again.
pub fn scope_of(kind: &str) -> Option<Scope> {
    if kind.contains("comment") || kind == "Comment" {
        return Some(Scope::Comment);
    }
    match kind {
        "interpolation"
        | "interpolated_expression"
        | "raw_str_interpolation"
        | "key_path_string_expression" // Swift's `#keyPath(A.b)`: code, not a string.
        // Swift's `\(` opener is a leaf of the string, not of its `interpolated_expression`:
        // right after it is code (`"\(|` closes a `"` typed there).
        | "\\("
        | "expansion"
        | "simple_expansion"
        | "command_substitution"
        | "variable_ref" => {
            return Some(Scope::Code);
        }
        "char_literal"
        | "system_lib_string"
        | "double_quote_scalar"
        | "single_quote_scalar"
        | "block_scalar"
        | "heredoc_body"
        | "heredoc_block"
        | "quoted_argument"
        | "bracket_argument"
        | "AttValue"
        | "EntityValue"
        | "PseudoAttValue"
        | "SystemLiteral"
        | "PubidLiteral" => return Some(Scope::String),
        _ => {}
    }
    (kind.contains("string") && !kind.starts_with("unquoted")).then_some(Scope::String)
}

fn current(state: &State) -> Option<&Tree> {
    match state {
        State::Ready { tree, edits: 0, .. } => Some(tree),
        _ => None,
    }
}

fn new_parser(language: Language) -> Parser {
    let mut parser = Parser::new();
    if let Some(grammar) = language.grammar() {
        // The grammars are built in and ABI-checked by a test.
        parser.set_language(&grammar).expect("grammar ABI");
    }
    parser
}

/// When a parse gives up.
enum Stop<'a> {
    Cancel(&'a AtomicBool),
    After(Instant),
}

impl Stop<'_> {
    fn now(&self) -> bool {
        match self {
            Stop::Cancel(c) => c.load(Ordering::Relaxed),
            Stop::After(deadline) => Instant::now() >= *deadline,
        }
    }
}

/// Parses the rope chunk by chunk (never copying it); `None` if it stopped early.
///
/// tree-sitter asks the progress callback only every 100 parse actions, and one action can
/// lex far: an unterminated `/*` scans to the end of the file each time. So reading checks too,
/// and once stopped reports the end of the text, which ends the parse soon; its tree is dropped.
fn parse(parser: &mut Parser, rope: &Rope, old: Option<&Tree>, stop: Stop) -> Option<Tree> {
    let stopped = std::cell::Cell::new(false);
    let check = || {
        if !stopped.get() && stop.now() {
            stopped.set(true);
        }
        stopped.get()
    };
    let mut read = |at: usize, _: Point| if check() { &[][..] } else { pos::chunk_from(rope, at) };
    let mut progress = |_: &tree_sitter::ParseState| {
        if check() {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(&mut read, old, Some(options));
    if tree.is_none() || stopped.get() {
        parser.reset();
        return None;
    }
    tree
}

/// From the first change's start to the last one's end, in pre-edit coordinates.
fn span(changes: &ChangeSet) -> usize {
    match (changes.iter().next(), changes.iter().last()) {
        (Some((start, _, _)), Some((_, end, _))) => end.0 - start.0,
        _ => 0,
    }
}

/// Bytes an edit touches (what an incremental re-parse has to look at again).
fn edit_bytes(e: &InputEdit) -> usize {
    e.old_end_byte.max(e.new_end_byte) - e.start_byte
}

/// The `InputEdit`s for a change set, in the order `Tree::edit` needs them: each in the
/// coordinates left by the ones before (change sets are in pre-edit coordinates). A set of more
/// than `COALESCE_AT` changes is one edit from its first start to its last end.
fn input_edits(changes: &ChangeSet, old: &Rope, new: &Rope, mut f: impl FnMut(InputEdit)) {
    if changes.len() > COALESCE_AT {
        let (Some((first, _, _)), Some((_, last_end, _))) = (changes.iter().next(), changes.iter().last()) else {
            return;
        };
        let delta = new.len() as isize - old.len() as isize;
        let new_end = ByteIdx((last_end.0 as isize + delta) as usize);
        f(InputEdit {
            start_byte: first.0,
            old_end_byte: last_end.0,
            new_end_byte: new_end.0,
            start_position: pos::ts_point(old, first),
            old_end_position: pos::ts_point(old, last_end),
            new_end_position: pos::ts_point(new, new_end),
        });
        return;
    }
    let mut delta: isize = 0;
    for (start, end, len) in changes.iter() {
        // The text before this change is the same in `new` (later changes sit after it).
        let at = (start.0 as isize + delta) as usize;
        let start_position = pos::ts_point(new, ByteIdx(at));
        let old_end_position = if end == start {
            start_position
        } else {
            // The removed text's extent, measured in `old`.
            let (a, b) = (pos::ts_point(old, start), pos::ts_point(old, end));
            if a.row == b.row {
                Point::new(start_position.row, start_position.column + (end.0 - start.0))
            } else {
                Point::new(start_position.row + (b.row - a.row), b.column)
            }
        };
        let new_end_position = if len == 0 {
            start_position
        } else {
            pos::ts_point(new, ByteIdx(at + len))
        };
        f(InputEdit {
            start_byte: at,
            old_end_byte: at + (end.0 - start.0),
            new_end_byte: at + len,
            start_position,
            old_end_position,
            new_end_position,
        });
        delta += len as isize - (end.0 - start.0) as isize;
    }
}

#[cfg(test)]
mod tests;
