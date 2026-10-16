//! Completion (no drawing: `ui::completion` draws the list; `app/completion.rs` drives it): a
//! `textDocument/completion` answer read into `Item`s, filtering and sorting them by the word
//! typed so far, LSP snippets turned into plain text, and inserting an item at every cursor.
//!
//! - `parse` takes `CompletionItem[]` or a `CompletionList` (`isIncomplete`, `itemDefaults`:
//!   `editRange`, `insertTextFormat`, `commitCharacters`, `data`). What an item inserts: its
//!   `textEdit` (an `InsertReplaceEdit` uses its `insert` range: VS Code's default
//!   `editor.suggest.insertMode: insert`), else `insertText`, else `label`, replacing the word
//!   before the cursor. A snippet (`insertTextFormat: 2`) is stripped to plain text by
//!   `strip_snippet`.
//! - An `Anchor` is where a request was asked: the text then (a cheap rope clone), the primary
//!   cursor in it, and two places followed through the edits since: the start of the word before
//!   the cursor (`start`, where filtering begins) and the cursor (`at`, mapped after text typed
//!   there). `place` turns each item's range (server positions in the text when asked) into byte
//!   counts relative to those: `lead` (bytes of the range before the word start: a range that
//!   also replaces a `.`) and `after` (bytes after the cursor).
//! - `Popup::refilter` filters by the text from each item's range start to the cursor against
//!   `filterText` (else `label`), fuzzy (`Filter`: nucleo, case-insensitive, the first char at a
//!   word start as VS Code's `fuzzyScore`), sorted by score, then `sortText` (else `label`), then
//!   `label`; an empty word keeps every item (by `sortText`) and selects a `preselect`ed one.
//! - `insert` applies an item at every selection as one `ChangeSet` and one undo step
//!   (`EditKind::Other`): VS Code replays the primary's edit relative to each cursor, so each
//!   selection gets the same number of chars replaced before and after it (stopping at its line's
//!   ends, and when the primary replaces only word chars, at a non-word char), and the
//!   `additionalTextEdits` (auto-imports) go into the same change set. Multi-line
//!   text gets each line's indentation after its line breaks and the buffer's line ending.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nucleo::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo::{Matcher, Utf32Str};
use ropey::Rope;
use serde_json::Value;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::file::Eol;
use crate::buffer::pos::{self, ByteIdx, LspPos, PosEncoding};
use crate::buffer::undo::EditKind;
use crate::buffer::{Buffer, BufferId};
use crate::layout::GroupId;
use crate::lsp::ServerId;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// Rows the list shows before it scrolls (VS Code: 12).
pub const MAX_ROWS: usize = 12;
/// The list's widest content (cells).
pub const MAX_WIDTH: usize = 60;
/// How long typing a word char waits before asking (VS Code's `quickSuggestionsDelay`).
pub const QUICK_DELAY: Duration = Duration::from_millis(10);
/// Change sets an `Anchor` follows before giving up (the list closes).
const MAX_SINCE: usize = 512;
/// The longest word filtering looks at (a longer run closes the list).
const MAX_WORD: usize = 256;

/// What asked for completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The command (Ctrl+Space): an empty answer says so.
    Invoked,
    /// Typing a word char (VS Code's quick suggestions).
    Quick,
    /// One of the server's trigger characters was typed.
    Character(char),
    /// Typing on in a list the server said was incomplete.
    Incomplete,
}

impl Trigger {
    /// The request's `context` (`CompletionContext`).
    pub fn context(self) -> Value {
        match self {
            Trigger::Invoked | Trigger::Quick => serde_json::json!({ "triggerKind": 1 }),
            Trigger::Character(c) => serde_json::json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            Trigger::Incomplete => serde_json::json!({ "triggerKind": 3 }),
        }
    }
}

/// One completion item, read.
#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    /// `CompletionItemKind` (1 = Text … 25 = TypeParameter).
    pub kind: Option<u64>,
    pub detail: Option<String>,
    filter: Option<String>,
    sort: Option<String>,
    pub preselect: bool,
    /// What it inserts (a snippet already stripped), line breaks as they came.
    pub text: String,
    /// What to select in `text` after inserting (a snippet's first tab stop; empty = a cursor);
    /// `None`: a cursor after it.
    pub select: Option<(usize, usize)>,
    /// The range it replaces, in the server's positions in the text when asked; `None`: the
    /// word before the cursor.
    range: Option<(LspPos, LspPos)>,
    /// Bytes of its range before the word's start (negative: it starts after it), and after the
    /// cursor (set by `place`).
    pub lead: isize,
    pub after: usize,
    /// `additionalTextEdits`: server positions in the text when asked.
    pub additional: Vec<(LspPos, LspPos, String)>,
    /// `commitCharacters` (or the list's default).
    pub commit: Vec<String>,
    /// The item as sent, for `completionItem/resolve`.
    pub raw: Value,
    /// A resolve answer was merged in.
    pub resolved: bool,
}

impl Item {
    /// What filtering matches against.
    pub fn filter_text(&self) -> &str {
        self.filter.as_deref().unwrap_or(&self.label)
    }

    fn sort_text(&self) -> &str {
        self.sort.as_deref().unwrap_or(&self.label)
    }

    /// Takes in a `completionItem/resolve` answer: its detail, additional edits and commit
    /// characters (the inserted text stays: VS Code doesn't change it after showing the list).
    pub fn resolve(&mut self, value: &Value) {
        self.resolved = true;
        if let Some(d) = value.get("detail").and_then(Value::as_str) {
            self.detail = Some(d.to_string());
        }
        if let Some(edits) = value.get("additionalTextEdits").and_then(Value::as_array) {
            self.additional = edits.iter().filter_map(text_edit).collect();
        }
        if let Some(c) = value.get("commitCharacters").and_then(Value::as_array) {
            self.commit = strings(c);
        }
    }
}

fn strings(v: &[Value]) -> Vec<String> {
    v.iter().filter_map(Value::as_str).map(String::from).collect()
}

fn lsp_pos(v: &Value) -> Option<LspPos> {
    Some(LspPos {
        line: v.get("line")?.as_u64()? as u32,
        character: v.get("character")?.as_u64()? as u32,
    })
}

/// A `Range` value.
fn range(v: &Value) -> Option<(LspPos, LspPos)> {
    Some((lsp_pos(v.get("start")?)?, lsp_pos(v.get("end")?)?)).filter(|(s, e)| s <= e)
}

/// A `TextEdit` value.
fn text_edit(v: &Value) -> Option<(LspPos, LspPos, String)> {
    let (s, e) = range(v.get("range")?)?;
    Some((s, e, v.get("newText")?.as_str()?.to_string()))
}

/// A `TextEdit` or `InsertReplaceEdit`'s range to use: `range`, else `insert`.
fn edit_range(v: &Value) -> Option<(LspPos, LspPos)> {
    v.get("range")
        .and_then(range)
        .or_else(|| v.get("insert").and_then(range))
}

/// Reads a `textDocument/completion` result: the items, and whether the list is incomplete.
pub fn parse(result: &Value) -> (Vec<Item>, bool) {
    let (items, incomplete, defaults) = match result {
        Value::Array(items) => (items.as_slice(), false, None),
        Value::Object(o) => (
            o.get("items").and_then(Value::as_array).map_or(&[][..], Vec::as_slice),
            o.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false),
            o.get("itemDefaults"),
        ),
        _ => (&[][..], false, None),
    };
    (items.iter().filter_map(|i| item(i, defaults)).collect(), incomplete)
}

fn item(v: &Value, defaults: Option<&Value>) -> Option<Item> {
    let label = v.get("label")?.as_str()?.to_string();
    let default = |key: &str| defaults.and_then(|d| d.get(key));
    let snippet = v
        .get("insertTextFormat")
        .or_else(|| default("insertTextFormat"))
        .and_then(Value::as_u64)
        == Some(2);
    let insert_text = v.get("insertText").and_then(Value::as_str);
    let (range, raw_text) = match v.get("textEdit") {
        Some(te) => (
            edit_range(te),
            te.get("newText").and_then(Value::as_str).map(String::from),
        ),
        None => match default("editRange") {
            Some(r) => (
                edit_range(r).or_else(|| range(r)),
                v.get("textEditText")
                    .and_then(Value::as_str)
                    .or(insert_text)
                    .map(String::from),
            ),
            None => (None, insert_text.map(String::from)),
        },
    };
    let raw_text = raw_text.unwrap_or_else(|| label.clone());
    let (text, select) = if snippet {
        strip_snippet(&raw_text)
    } else {
        (raw_text, None)
    };
    let commit = v
        .get("commitCharacters")
        .or_else(|| default("commitCharacters"))
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |c| strings(c));
    let mut raw = v.clone();
    if raw.get("data").is_none()
        && let Some(data) = default("data")
    {
        raw["data"] = data.clone();
    }
    Some(Item {
        kind: v.get("kind").and_then(Value::as_u64),
        detail: v.get("detail").and_then(Value::as_str).map(String::from),
        filter: v.get("filterText").and_then(Value::as_str).map(String::from),
        sort: v.get("sortText").and_then(Value::as_str).map(String::from),
        preselect: v.get("preselect").and_then(Value::as_bool).unwrap_or(false),
        text,
        select,
        range,
        lead: 0,
        after: 0,
        additional: v
            .get("additionalTextEdits")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |e| e.iter().filter_map(text_edit).collect()),
        commit,
        raw,
        resolved: false,
        label,
    })
}

/// The kind's icon (one char) and its theme key (VS Code's `symbolIcon.*Foreground` groups).
pub fn icon(kind: Option<u64>) -> (char, &'static str) {
    const FUNCTION: &str = "symbolIcon.functionForeground";
    const VARIABLE: &str = "symbolIcon.variableForeground";
    const CLASS: &str = "symbolIcon.classForeground";
    const OTHER: &str = "symbolIcon.foreground";
    match kind.unwrap_or(1) {
        2 => ('m', FUNCTION),  // Method
        3 => ('f', FUNCTION),  // Function
        4 => ('c', FUNCTION),  // Constructor
        5 => ('d', VARIABLE),  // Field
        6 => ('v', VARIABLE),  // Variable
        7 => ('C', CLASS),     // Class
        8 => ('I', VARIABLE),  // Interface
        9 => ('M', OTHER),     // Module
        10 => ('p', OTHER),    // Property
        11 => ('u', OTHER),    // Unit
        12 => ('v', OTHER),    // Value
        13 => ('E', CLASS),    // Enum
        14 => ('k', OTHER),    // Keyword
        15 => ('s', OTHER),    // Snippet
        16 => ('#', OTHER),    // Color
        17 => ('F', OTHER),    // File
        18 => ('r', VARIABLE), // Reference
        19 => ('D', OTHER),    // Folder
        20 => ('e', VARIABLE), // EnumMember
        21 => ('K', OTHER),    // Constant
        22 => ('S', OTHER),    // Struct
        23 => ('!', CLASS),    // Event
        24 => ('o', OTHER),    // Operator
        25 => ('T', VARIABLE), // TypeParameter
        _ => ('t', OTHER),     // Text
    }
}

/// LSP snippet syntax as plain text: placeholders become their default text (a choice its first
/// option, a variable its default or nothing), and what to select afterwards: the first tab
/// stop (the lowest number from 1; its placeholder text selected), else `$0` (a cursor), else
/// `None`. Tab stops after the first aren't visited (no snippet session).
pub fn strip_snippet(snippet: &str) -> (String, Option<(usize, usize)>) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::new();
    let mut stops = Vec::new();
    let mut i = 0;
    snippet_any(&chars, &mut i, &mut out, &mut stops, false);
    let first = stops
        .iter()
        .filter(|(n, ..)| *n > 0)
        .min_by_key(|(n, ..)| *n)
        .or_else(|| stops.iter().find(|(n, ..)| *n == 0));
    let select = first.map(|&(_, s, e)| (s, e));
    (out, select)
}

/// Snippet text up to the end (or an unescaped `}` when `nested`, consumed).
fn snippet_any(c: &[char], i: &mut usize, out: &mut String, stops: &mut Vec<(u32, usize, usize)>, nested: bool) {
    while *i < c.len() {
        match c[*i] {
            '\\' if c.get(*i + 1).is_some_and(|n| matches!(n, '$' | '}' | '\\' | ',' | '|')) => {
                out.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => {
                *i += 1;
                return;
            }
            '$' => snippet_dollar(c, i, out, stops),
            ch => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

/// At a `$`: a tab stop, placeholder, choice or variable; else the `$` itself.
fn snippet_dollar(c: &[char], i: &mut usize, out: &mut String, stops: &mut Vec<(u32, usize, usize)>) {
    let digits = |j: usize| c[j..].iter().take_while(|d| d.is_ascii_digit()).count();
    let name = |j: usize| {
        if c.get(j).is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_') {
            c[j..]
                .iter()
                .take_while(|ch| ch.is_ascii_alphanumeric() || **ch == '_')
                .count()
        } else {
            0
        }
    };
    let number = |from: usize, n: usize| c[from..from + n].iter().collect::<String>().parse::<u32>().unwrap_or(0);
    let j = *i + 1;
    // `$1`, `$VAR`
    let n = digits(j);
    if n > 0 {
        stops.push((number(j, n), out.len(), out.len()));
        *i = j + n;
        return;
    }
    let n = name(j);
    if n > 0 {
        *i = j + n; // an unknown variable: nothing
        return;
    }
    if c.get(j) != Some(&'{') {
        out.push('$');
        *i += 1;
        return;
    }
    let k = j + 1;
    let n = digits(k);
    if n > 0 {
        let stop = number(k, n);
        let start = out.len();
        let after = k + n;
        match c.get(after) {
            Some('}') => {
                stops.push((stop, start, start));
                *i = after + 1;
                return;
            }
            Some(':') => {
                *i = after + 1;
                snippet_any(c, i, out, stops, true);
                stops.push((stop, start, out.len()));
                return;
            }
            Some('|') => {
                // A choice: its first option.
                let mut p = after + 1;
                while p < c.len() && !matches!(c[p], ',' | '|') {
                    if c[p] == '\\' && p + 1 < c.len() {
                        p += 1;
                    }
                    out.push(c[p]);
                    p += 1;
                }
                while p < c.len() && !(c[p] == '|' && c.get(p + 1) == Some(&'}')) {
                    p += if c[p] == '\\' { 2 } else { 1 };
                }
                stops.push((stop, start, out.len()));
                *i = (p + 2).min(c.len());
                return;
            }
            _ => {}
        }
    }
    let n = name(k);
    if n > 0 {
        let after = k + n;
        match c.get(after) {
            Some('}') => {
                *i = after + 1;
                return;
            }
            Some(':') => {
                *i = after + 1;
                snippet_any(c, i, out, stops, true);
                return;
            }
            _ => {}
        }
    }
    out.push('$');
    *i += 1;
}

/// Fuzzy scoring, VS Code-like: case-insensitive, and the query's first char must match at a word
/// start of the text (its start, after a non-alphanumeric, or a camelCase hump).
pub struct Filter {
    matcher: Matcher,
    buf: Vec<char>,
    hits: Vec<u32>,
    atom: Option<(String, Atom)>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            matcher: Matcher::new(nucleo::Config::DEFAULT),
            buf: Vec::new(),
            hits: Vec::new(),
            atom: None,
        }
    }
}

impl Filter {
    /// `text`'s score for `query` (higher is better; an empty query scores 0), or `None`. With
    /// `hits`, the matched char indices of `text` go there (cleared first).
    pub fn score(&mut self, query: &str, text: &str, hits: Option<&mut Vec<u32>>) -> Option<u32> {
        if query.is_empty() {
            if let Some(h) = hits {
                h.clear();
            }
            return Some(0);
        }
        if self.atom.as_ref().is_none_or(|(q, _)| q != query) {
            let atom = Atom::new(
                query,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
                false,
            );
            self.atom = Some((query.to_string(), atom));
        }
        let atom = &self.atom.as_ref().expect("set").1;
        self.hits.clear();
        let score = atom.indices(Utf32Str::new(text, &mut self.buf), &mut self.matcher, &mut self.hits)?;
        let first = *self.hits.iter().min()? as usize;
        if first > 0 {
            let mut chars = text.chars().skip(first - 1);
            let (prev, at) = (chars.next()?, chars.next()?);
            let word_start = !prev.is_alphanumeric() || (prev.is_lowercase() && at.is_uppercase());
            if !word_start {
                return None;
            }
        }
        if let Some(h) = hits {
            h.clear();
            h.extend_from_slice(&self.hits);
            h.sort_unstable();
        }
        Some(score as u32)
    }
}

/// Where a completion request was asked, followed through the edits since.
pub struct Anchor {
    pub group: GroupId,
    pub buffer: BufferId,
    pub server: ServerId,
    pub encoding: PosEncoding,
    /// The text when asked (a cheap clone) and the primary cursor in it.
    pub rope: Rope,
    pub at_snap: ByteIdx,
    /// The bytes of the word before that cursor (filtering starts there).
    pub word: usize,
    /// The word's start now (mapped before text typed there).
    pub start: ByteIdx,
    /// The cursor's place now: mapped after text typed there (the text that followed it).
    pub at: ByteIdx,
    /// The edits since, to read places in `rope` now; `None` past `MAX_SINCE`.
    since: Option<Vec<Arc<ChangeSet>>>,
    /// The buffer's count of applied change sets (`Buffer::sets_applied`) `rope` and the
    /// places are at.
    applied: u64,
}

impl Anchor {
    pub fn new(
        (group, buffer): (GroupId, BufferId),
        (server, encoding): (ServerId, PosEncoding),
        (rope, applied): (Rope, u64),
        at: ByteIdx,
        word: usize,
    ) -> Self {
        Self {
            applied,
            group,
            buffer,
            server,
            encoding,
            rope,
            at_snap: at,
            word,
            start: ByteIdx(at.0 - word),
            at,
            since: Some(Vec::new()),
        }
    }

    /// Follows the buffer's last applied change sets (`applied`: its `sets_applied` after them;
    /// those the anchor's text already has are skipped); false once it can't any more: too many
    /// edits, or text right before the word's start removed.
    pub fn follow(&mut self, sets: &[Arc<ChangeSet>], applied: u64) -> bool {
        let first = applied.saturating_sub(sets.len() as u64);
        let skip = self.applied.saturating_sub(first).min(sets.len() as u64) as usize;
        self.applied = self.applied.max(applied);
        for set in &sets[skip..] {
            // Text before the word's start removed (Backspace past it): the word is gone.
            for (s, e, _) in set.iter() {
                if s >= self.start {
                    break;
                }
                if e >= self.start {
                    return false;
                }
            }
            let m = set.mapper();
            self.start = m.map(self.start, Assoc::Before);
            self.at = m.map(self.at, Assoc::After);
            if let Some(since) = &mut self.since {
                since.push(set.clone());
                if since.len() > MAX_SINCE {
                    self.since = None;
                }
            }
        }
        self.since.is_some()
    }

    /// A position in the text when asked, now (`None` when the edits weren't kept).
    pub fn now(&self, p: ByteIdx) -> Option<ByteIdx> {
        let since = self.since.as_ref()?;
        Some(since.iter().fold(p, |p, set| set.mapper().map(p, Assoc::After)))
    }

    /// Whether the cursor `head` (in `rope`, the text now) is still in the word: at or after its
    /// start, with only word chars between.
    pub fn in_word(&self, rope: &Rope, head: ByteIdx) -> bool {
        if head < self.start || head.0 - self.start.0 > MAX_WORD || head.0 > rope.len() {
            return false;
        }
        rope.slice(self.start.0..head.0)
            .chunks()
            .all(|c| c.chars().all(pos::is_word_char))
    }

    /// The text from `start - lead` to `head` now (`None` when that's no range).
    fn query(&self, rope: &Rope, head: ByteIdx, lead: isize) -> Option<String> {
        let from = self.start.0 as isize - lead;
        if from < 0 || from as usize > head.0 || head.0 > rope.len() {
            return None;
        }
        Some(rope.slice(from as usize..head.0).to_string())
    }
}

/// The bytes of the word (word chars) right before `at`, at most `MAX_WORD`.
pub fn word_before(rope: &Rope, at: ByteIdx) -> usize {
    let mut peek = pos::Peek::new(rope);
    let mut p = at.0;
    while at.0 - p < MAX_WORD
        && let Some(c) = peek.before(ByteIdx(p))
        && pos::is_word_char(c)
    {
        p -= c.len_utf8();
    }
    at.0 - p
}

/// Reads each item's range in the anchor's text: `lead` and `after`. Items whose range doesn't
/// hold the cursor, or spans lines, are dropped (VS Code ignores them too).
pub fn place(items: Vec<Item>, anchor: &Anchor) -> Vec<Item> {
    let rope = &anchor.rope;
    let at = anchor.at_snap.0;
    let start = at - anchor.word;
    let mut last: Option<(LspPos, usize)> = None;
    let mut byte = |p: LspPos| match last {
        Some((q, b)) if q == p => b,
        _ => {
            let b = pos::from_lsp_pos(rope, p, anchor.encoding).0;
            last = Some((p, b));
            b
        }
    };
    items
        .into_iter()
        .filter_map(|mut item| {
            if let Some((s, e)) = item.range {
                if s.line != e.line {
                    return None;
                }
                let (s, e) = (byte(s), byte(e));
                if s > at || e < at {
                    return None;
                }
                item.lead = start as isize - s as isize;
                item.after = e - at;
            } else {
                item.lead = 0;
                item.after = 0;
            }
            Some(item)
        })
        .collect()
}

/// The completion list on screen.
pub struct Popup {
    pub anchor: Anchor,
    /// Which request it came from.
    pub generation: u64,
    pub items: Vec<Item>,
    /// The server said typing on should ask again.
    pub incomplete: bool,
    /// The word when it was asked for (an incomplete list asks again when it differs).
    pub asked: String,
    /// The indices of the items that match, best first.
    pub shown: Vec<usize>,
    /// Index into `shown`.
    pub selected: usize,
    /// The first row shown.
    pub top: usize,
    /// The rows drawn last (set by drawing; the page size).
    pub rows: usize,
    /// The word filtered by last (`None`: not yet).
    pub query: Option<String>,
    /// The server answers `completionItem/resolve`.
    pub resolve: bool,
}

impl Popup {
    pub fn new(anchor: Anchor, generation: u64, items: Vec<Item>, incomplete: bool, asked: String) -> Self {
        Self {
            anchor,
            generation,
            items,
            incomplete,
            asked,
            shown: Vec::new(),
            selected: 0,
            top: 0,
            rows: 0,
            query: None,
            resolve: false,
        }
    }

    /// The word from its start to the cursor `head`.
    pub fn word(&self, rope: &Rope, head: ByteIdx) -> String {
        self.anchor.query(rope, head, 0).unwrap_or_default()
    }

    /// The text an item is matched by, for the cursor at `head`.
    pub fn item_query(&self, rope: &Rope, head: ByteIdx, item: &Item) -> Option<String> {
        self.anchor.query(rope, head, item.lead)
    }

    /// Filters and sorts the items by the word up to `head`, if it changed (or `force`); the
    /// selection goes to the first row (or a preselected item while the word is empty).
    pub fn refilter(&mut self, rope: &Rope, head: ByteIdx, filter: &mut Filter, force: bool) {
        let word = self.word(rope, head);
        if !force && self.query.as_deref() == Some(word.as_str()) {
            return;
        }
        let mut queries: Vec<(isize, Option<String>)> = vec![(0, Some(word.clone()))];
        let mut scored: Vec<(u32, usize)> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            let q = match queries.iter().position(|(l, _)| *l == item.lead) {
                Some(k) => k,
                None => {
                    queries.push((item.lead, self.anchor.query(rope, head, item.lead)));
                    queries.len() - 1
                }
            };
            let Some(query) = &queries[q].1 else { continue };
            if let Some(score) = filter.score(query, item.filter_text(), None) {
                scored.push((score, i));
            }
        }
        let items = &self.items;
        scored.sort_by(|(sa, a), (sb, b)| {
            let (a, b) = (&items[*a], &items[*b]);
            sb.cmp(sa)
                .then_with(|| a.sort_text().cmp(b.sort_text()))
                .then_with(|| a.label.cmp(&b.label))
        });
        self.shown = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = if word.is_empty() {
            self.shown.iter().position(|&i| self.items[i].preselect).unwrap_or(0)
        } else {
            0
        };
        self.top = 0;
        self.reveal(MAX_ROWS);
        self.query = Some(word);
    }

    /// The selected item's index in `items`.
    pub fn selected_index(&self) -> Option<usize> {
        self.shown.get(self.selected).copied()
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected_index().map(|i| &self.items[i])
    }

    /// Moves the selection `delta` rows; `wrap` goes round from either end (Up / Down), else it
    /// stops there (pages).
    pub fn move_by(&mut self, delta: isize, wrap: bool) {
        let n = self.shown.len() as isize;
        if n == 0 {
            return;
        }
        let to = self.selected as isize + delta;
        self.selected = if wrap { to.rem_euclid(n) } else { to.clamp(0, n - 1) } as usize;
        self.reveal(self.page());
    }

    /// The rows a page moves (the rows drawn, else the most).
    pub fn page(&self) -> usize {
        if self.rows > 0 { self.rows } else { MAX_ROWS }
    }

    /// Scrolls the least to show the selection in `rows` rows.
    pub fn reveal(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
        self.top = self.top.min(self.shown.len().saturating_sub(rows));
    }

    /// Scrolls the rows (the wheel) without moving the selection.
    pub fn scroll_by(&mut self, delta: isize) {
        let rows = self.page();
        let max = self.shown.len().saturating_sub(rows);
        self.top = (self.top as isize + delta).clamp(0, max as isize) as usize;
    }
}

/// The completion state: the list, the request it waits for, the quick-suggestion timer.
#[derive(Default)]
pub struct Completion {
    pub popup: Option<Popup>,
    /// The pending request's place (its `Pending` in `App::requests` has the same generation).
    pub ask: Option<Ask>,
    /// Typing a word char asks once this passes, if nothing changed.
    pub quick: Option<Quick>,
    /// Numbers requests.
    pub generation: u64,
    pub filter: Filter,
}

pub struct Ask {
    pub generation: u64,
    pub anchor: Anchor,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quick {
    pub deadline: Instant,
    pub group: GroupId,
    pub buffer: BufferId,
    pub revision: u64,
}

impl Completion {
    /// Follows edits to `buffer` in the list's and the pending request's anchors (each is
    /// dropped when too many edits came between).
    /// `changes` are the buffer's last applied sets (`applied`: its `sets_applied` after them).
    pub fn follow(&mut self, buffer: BufferId, changes: &[Arc<ChangeSet>], applied: u64) {
        if let Some(p) = &mut self.popup
            && p.anchor.buffer == buffer
            && !p.anchor.follow(changes, applied)
        {
            self.popup = None;
        }
        if let Some(a) = &mut self.ask
            && a.anchor.buffer == buffer
            && !a.anchor.follow(changes, applied)
        {
            self.ask = None;
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.quick.map(|q| q.deadline)
    }
}

/// An item to insert at every selection.
pub struct Insertion<'a> {
    /// Line breaks as `\n` or `\r\n`: converted to the buffer's.
    pub text: &'a str,
    pub select: Option<(usize, usize)>,
    /// Bytes replaced before and after the primary cursor; the other selections get as many
    /// chars on each side (within their lines).
    pub before: usize,
    pub after: usize,
    /// Other edits (current positions; any order): dropped where they overlap the insertions.
    pub additional: Vec<(ByteIdx, ByteIdx, String)>,
}

/// Where one change's text comes from.
#[derive(Clone, Copy)]
enum Text {
    /// The insertion's text, as is.
    Shared,
    /// `owned[i]` (adapted to a line's indentation / line endings), with its selection.
    Owned(usize),
    /// `additional[i]`'s text.
    Extra(usize),
}

/// Inserts `ins` at every selection of `view`: one change set (the additional edits included),
/// one undo step (`EditKind::Other`); each selection lands on the text's selection (a cursor
/// after it by default).
pub fn insert(buf: &mut Buffer, view: &mut View, ins: &Insertion) {
    let rope = &buf.rope;
    let before_sels = view.selections.clone();
    let primary = before_sels.primary_index();
    let head = before_sels.primary().head;
    let p_start = ByteIdx(head.0.saturating_sub(ins.before));
    let p_end = ByteIdx((head.0 + ins.after).min(rope.len()));
    let back = rope.slice(p_start.0..head.0).to_string();
    let fwd = rope.slice(head.0..p_end.0).to_string();
    let (back_chars, fwd_chars) = (back.chars().count(), fwd.chars().count());
    // A word replaced at the primary replaces only word chars at the others (not a space or
    // bracket before a shorter word there).
    let words = back.chars().chain(fwd.chars()).all(pos::is_word_char);
    let takes = |c: char| c != '\n' && c != '\r' && (!words || pos::is_word_char(c));
    let crlf = buf.eol == Eol::Crlf;
    let lf = ins.text.replace("\r\n", "\n");
    let multi_line = lf.contains('\n');
    // The text as is, unless it needs a line's indentation or CRLF.
    let (shared, shared_select) = if multi_line {
        (String::new(), None)
    } else {
        (lf.clone(), ins.select)
    };

    let mut peek = pos::Peek::new(rope);
    let mut owned: Vec<(String, Option<(usize, usize)>)> = Vec::new();
    let mut mains: Vec<(ByteIdx, ByteIdx, Text)> = Vec::with_capacity(before_sels.len());
    for (i, sel) in before_sels.iter().enumerate() {
        let (start, end) = if i == primary {
            (sel.start().min(p_start), sel.end().max(p_end))
        } else {
            // Counted from the head, as at the primary (a selection's own text is part of the
            // word before its head), and taking in the whole selection.
            let mut s = sel.head;
            for _ in 0..back_chars {
                match peek.before(s) {
                    Some(c) if takes(c) => s = ByteIdx(s.0 - c.len_utf8()),
                    _ => break,
                }
            }
            let mut e = sel.head;
            for _ in 0..fwd_chars {
                match peek.after(e) {
                    Some(c) if takes(c) => e = ByteIdx(e.0 + c.len_utf8()),
                    _ => break,
                }
            }
            (s.min(sel.start()), e.max(sel.end()))
        };
        let text = if multi_line {
            let indent = peek.line_indent(start, 4096).unwrap_or_default();
            owned.push(adapt(&lf, &indent, crlf, ins.select));
            Text::Owned(owned.len() - 1)
        } else {
            Text::Shared
        };
        mains.push((start, end, text));
    }
    let mut extras: Vec<(ByteIdx, ByteIdx, String)> = ins
        .additional
        .iter()
        .map(|(s, e, t)| {
            let t = t.replace("\r\n", "\n");
            (*s, *e, if crlf { t.replace('\n', "\r\n") } else { t })
        })
        .collect();
    extras.sort_by_key(|(s, e, _)| (*s, *e));

    let text_of = |t: Text| -> (&str, Option<(usize, usize)>) {
        match t {
            Text::Shared => (shared.as_str(), shared_select),
            Text::Owned(i) => (owned[i].0.as_str(), owned[i].1),
            Text::Extra(i) => (extras[i].2.as_str(), None),
        }
    };
    let mut changes = ChangeSet::with_capacity(mains.len() + extras.len(), mains.len() * shared.len().max(1));
    let mut lands: Vec<Selection> = Vec::with_capacity(mains.len());
    let mut prev_end = ByteIdx(0);
    let mut delta: isize = 0;
    let mut extra = 0;
    let mut push = |start: ByteIdx, end: ByteIdx, t: Text, changes: &mut ChangeSet| {
        let (text, select) = text_of(t);
        changes.push(start, end, text);
        let new_start = (start.0 as isize + delta) as usize;
        delta += text.len() as isize - (end.0 - start.0) as isize;
        let (a, h) = select.unwrap_or((text.len(), text.len()));
        Selection::new(ByteIdx(new_start + a), ByteIdx(new_start + h))
    };
    for (start, end, t) in mains {
        // Additional edits before this one (dropped if they overlap what's already there).
        while extra < extras.len() && extras[extra].1 <= start {
            let (s, e, _) = extras[extra];
            if s >= prev_end {
                push(s, e, Text::Extra(extra), &mut changes);
                prev_end = e;
            } else {
                tracing::debug!(
                    start = s.0,
                    end = e.0,
                    "additional completion edit overlaps another: dropped"
                );
            }
            extra += 1;
        }
        while extra < extras.len() && extras[extra].0 < end {
            tracing::debug!(
                start = extras[extra].0.0,
                "additional completion edit overlaps the insertion: dropped"
            );
            extra += 1;
        }
        let start = start.max(prev_end);
        let end = end.max(start);
        prev_end = end;
        lands.push(push(start, end, t, &mut changes));
    }
    while extra < extras.len() {
        let (s, e, _) = extras[extra];
        if s >= prev_end {
            push(s, e, Text::Extra(extra), &mut changes);
            prev_end = e;
        }
        extra += 1;
    }
    let after = Selections::with_newest(lands, primary, before_sels.newest_index());
    view.auto_closed.follow(buf.revision(), &changes);
    buf.edit(changes, before_sels, after.clone(), EditKind::Other);
    view.auto_closed.edited(buf.revision(), &after);
    view.selections = after;
    view.reveal_cursor = true;
}

/// `text` (line breaks `\n`) for a line indented `indent`: each break followed by the indent, in
/// CRLF if asked; the selection moved to match.
fn adapt(text: &str, indent: &str, crlf: bool, select: Option<(usize, usize)>) -> (String, Option<(usize, usize)>) {
    let mut out = String::with_capacity(text.len() + 16);
    let (mut a, mut h) = (None, None);
    for (i, c) in text.char_indices() {
        if let Some((sa, sh)) = select {
            if sa == i {
                a = Some(out.len());
            }
            if sh == i {
                h = Some(out.len());
            }
        }
        if c == '\n' {
            out.push_str(if crlf { "\r\n" } else { "\n" });
            out.push_str(indent);
        } else {
            out.push(c);
        }
    }
    let end = out.len();
    (out, select.map(|_| (a.unwrap_or(end), h.unwrap_or(end))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snippets_strip_to_their_defaults() {
        assert_eq!(strip_snippet("foo($0)"), ("foo()".into(), Some((4, 4))));
        assert_eq!(
            strip_snippet("foo(${1:int x}, ${2:y})$0"),
            ("foo(int x, y)".into(), Some((4, 9)))
        );
        assert_eq!(
            strip_snippet("a${2}b$1c"),
            ("abc".into(), Some((2, 2))),
            "the lowest stop"
        );
        assert_eq!(
            strip_snippet("${1:outer ${2:inner}}"),
            ("outer inner".into(), Some((0, 11)))
        );
        assert_eq!(strip_snippet("${1|one,two|} x"), ("one x".into(), Some((0, 3))));
        assert_eq!(
            strip_snippet("\\$1 \\} \\\\ $ ${TM_FILENAME} ${X:def}"),
            ("$1 } \\ $  def".into(), None)
        );
        assert_eq!(strip_snippet("plain"), ("plain".into(), None));
        assert_eq!(strip_snippet("end${0}"), ("end".into(), Some((3, 3))));
        assert_eq!(strip_snippet("é${1:ü}"), ("éü".into(), Some((2, 4))), "byte offsets");
    }

    fn labels(p: &Popup) -> Vec<&str> {
        p.shown.iter().map(|&i| p.items[i].label.as_str()).collect()
    }

    fn anchor(text: &str, at: usize) -> Anchor {
        let rope = Rope::from_str(text);
        let word = word_before(&rope, ByteIdx(at));
        Anchor::new(
            (GroupId::default(), BufferId::default()),
            (ServerId::test(1), PosEncoding::Utf8),
            (rope, 0),
            ByteIdx(at),
            word,
        )
    }

    #[test]
    fn parse_reads_lists_items_and_defaults() {
        let (items, incomplete) = parse(&json!([{ "label": "a" }, { "nolabel": 1 }]));
        assert_eq!((items.len(), incomplete), (1, false));
        assert_eq!(items[0].text, "a");
        let (items, incomplete) = parse(&json!({
            "isIncomplete": true,
            "itemDefaults": {
                "editRange": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 3 } },
                "insertTextFormat": 2,
                "commitCharacters": ["("],
                "data": { "d": 1 },
            },
            "items": [
                { "label": "x", "insertText": "x($0)" },
                { "label": "y", "textEditText": "yy", "insertTextFormat": 1 },
                { "label": "z", "kind": 3, "detail": "fn z", "sortText": "0", "filterText": "zed", "preselect": true,
                  "textEdit": { "insert": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 2 } },
                                "replace": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 4 } },
                                "newText": "zz" },
                  "additionalTextEdits": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "import z\n" }],
                  "data": 7 },
            ],
        }));
        assert!(incomplete);
        assert_eq!((items[0].text.as_str(), items[0].select), ("x()", Some((2, 2))));
        assert_eq!(items[0].commit, ["("]);
        assert_eq!(items[0].raw["data"], json!({ "d": 1 }));
        assert_eq!(items[1].text, "yy");
        let p = |c| LspPos { line: 0, character: c };
        assert_eq!(items[1].range, Some((p(1), p(3))));
        let z = &items[2];
        assert_eq!(
            (z.text.as_str(), z.range),
            ("zz", Some((p(0), p(2)))),
            "the insert range"
        );
        assert_eq!((z.filter_text(), z.sort_text(), z.preselect), ("zed", "0", true));
        assert_eq!(z.additional, [(p(0), p(0), "import z\n".to_string())]);
        assert_eq!(z.raw["data"], json!(7));
        assert_eq!(icon(z.kind).0, 'f');
    }

    #[test]
    fn filtering_is_fuzzy_from_word_starts_and_sorted() {
        let mut f = Filter::default();
        assert!(f.score("ab", "about", None).is_some());
        assert!(f.score("AB", "about", None).is_some(), "case-insensitive");
        assert!(f.score("gi", "getItem", None).is_some());
        assert!(f.score("it", "getItem", None).is_some(), "a camelCase hump");
        assert!(f.score("it", "split", None).is_none(), "not at a word start");
        assert!(f.score("x", "abc", None).is_none());
        assert!(f.score("in", "__init__", None).is_some(), "after underscores");
        let mut hits = Vec::new();
        f.score("gt", "getItem", Some(&mut hits));
        assert_eq!(hits, [0, 2]);

        let items: Vec<Item> = parse(&json!([
            { "label": "format", "sortText": "b" },
            { "label": "for", "sortText": "c" },
            { "label": "from", "sortText": "a" },
            { "label": "zap", "sortText": "0" },
            { "label": "fooBar", "sortText": "d", "preselect": true },
        ]))
        .0;
        let a = anchor("x = fo", 6);
        let items = place(items, &a);
        let mut p = Popup::new(a, 1, items, false, "fo".into());
        let rope = p.anchor.rope.clone();
        p.refilter(&rope, ByteIdx(6), &mut f, false);
        assert_eq!(
            labels(&p)[..2],
            ["format", "for"],
            "prefixes first, equal scores by sortText"
        );
        assert_eq!(labels(&p)[2], "fooBar");
        assert!(labels(&p).contains(&"fooBar"));
        assert!(!labels(&p).contains(&"zap"));
        assert_eq!(p.selected, 0);
        // The empty word: every item by sortText, the preselected one selected.
        p.refilter(&rope, ByteIdx(4), &mut f, false);
        assert_eq!(labels(&p), ["zap", "from", "format", "for", "fooBar"]);
        assert_eq!(p.selected, 4);
        p.move_by(1, true);
        assert_eq!(p.selected, 0, "wraps");
        p.move_by(-1, true);
        assert_eq!(p.selected, 4);
        p.move_by(-10, false);
        assert_eq!(p.selected, 0, "a page stops at the top");
    }

    #[test]
    fn an_item_range_with_a_lead_matches_from_its_start() {
        // `x.fo|`: the server's range takes the dot too (`.foo` → `->foo` style), filterText `.foo`.
        let a = anchor("x.fo", 4);
        assert_eq!(a.word, 2);
        let items = parse(&json!([
            { "label": "foo", "filterText": ".foo", "textEdit": { "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 4 } }, "newText": "->foo" } },
            { "label": "bad", "textEdit": { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 2 } }, "newText": "bad" } },
        ]))
        .0;
        let items = place(items, &a);
        assert_eq!(items.len(), 1, "a range that misses the cursor is dropped");
        assert_eq!((items[0].lead, items[0].after), (1, 0));
        let rope = a.rope.clone();
        let mut p = Popup::new(a, 1, items, false, String::new());
        p.refilter(&rope, ByteIdx(4), &mut Filter::default(), false);
        assert_eq!(labels(&p), ["foo"]);
        assert_eq!(p.item_query(&rope, ByteIdx(4), &p.items[0]).as_deref(), Some(".fo"));
    }

    fn buffer(text: &str, cursors: &[usize]) -> (Buffer, View) {
        let buf = Buffer::from_str(text);
        let mut view = View::new();
        view.selections = Selections::new(cursors.iter().map(|&c| Selection::cursor(ByteIdx(c))).collect(), 0);
        (buf, view)
    }

    fn heads(view: &View) -> Vec<(usize, usize)> {
        view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect()
    }

    #[test]
    fn insert_replays_the_edit_at_every_cursor_in_one_step() {
        let (mut buf, mut view) = buffer("fo x\nfo y\nf\n", &[2, 7, 11]);
        let (text, select) = strip_snippet("foo($0)");
        let ins = Insertion {
            text: &text,
            select,
            before: 2,
            after: 0,
            additional: vec![(ByteIdx(0), ByteIdx(0), "#include <foo>\n".into())],
        };
        insert(&mut buf, &mut view, &ins);
        let inc = "#include <foo>\n".len();
        assert_eq!(buf.rope.to_string(), "#include <foo>\nfoo() x\nfoo() y\nfoo()\n");
        assert_eq!(
            heads(&view),
            [(inc + 4, inc + 4), (inc + 12, inc + 12), (inc + 20, inc + 20)],
            "inside the parens; the third had only one char on its line to replace"
        );
        buf.undo();
        assert_eq!(buf.rope.to_string(), "fo x\nfo y\nf\n", "one undo step");
    }

    #[test]
    fn insert_selects_a_placeholder_and_indents_new_lines() {
        let (mut buf, mut view) = buffer("  if\n\tif", &[4, 8]);
        let (text, select) = strip_snippet("if ${1:cond}:\n\t$0");
        insert(
            &mut buf,
            &mut view,
            &Insertion {
                text: &text,
                select,
                before: 2,
                after: 0,
                additional: vec![],
            },
        );
        assert_eq!(buf.rope.to_string(), "  if cond:\n  \t\n\tif cond:\n\t\t");
        assert_eq!(heads(&view), [(5, 9), (19, 23)], "`cond` selected at both");
        // CRLF buffers get CRLF.
        let (mut buf, mut view) = buffer("a\r\nb", &[1]);
        buf.eol = Eol::Crlf;
        insert(
            &mut buf,
            &mut view,
            &Insertion {
                text: "x\ny",
                select: None,
                before: 1,
                after: 0,
                additional: vec![],
            },
        );
        assert_eq!(buf.rope.to_string(), "x\r\ny\r\nb");
        assert_eq!(heads(&view), [(4, 4)]);
    }

    #[test]
    fn insert_drops_additional_edits_that_overlap() {
        let (mut buf, mut view) = buffer("abc def", &[3]);
        insert(
            &mut buf,
            &mut view,
            &Insertion {
                text: "xyz",
                select: None,
                before: 3,
                after: 0,
                additional: vec![
                    (ByteIdx(1), ByteIdx(2), "!".into()),
                    (ByteIdx(4), ByteIdx(7), "DEF".into()),
                ],
            },
        );
        assert_eq!(buf.rope.to_string(), "xyz DEF");
        assert_eq!(heads(&view), [(3, 3)]);
    }

    #[test]
    fn anchors_follow_typing_and_backspace() {
        let mut a = anchor("x fo", 4);
        assert_eq!((a.start.0, a.at.0), (2, 4));
        let typed = ChangeSet::new(vec![crate::buffer::change::Change {
            start: ByteIdx(4),
            end: ByteIdx(4),
            text: "o".into(),
        }]);
        assert!(a.follow(&[Arc::new(typed.clone())], 1));
        assert!(a.follow(&[Arc::new(typed)], 1), "a set it has seen: skipped");
        assert_eq!((a.start.0, a.at.0), (2, 5));
        let rope = Rope::from_str("x foo");
        assert!(a.in_word(&rope, ByteIdx(5)));
        assert!(!a.in_word(&rope, ByteIdx(1)));
        assert!(!a.in_word(&Rope::from_str("x f.o"), ByteIdx(5)));
        let deleted = ChangeSet::new(vec![crate::buffer::change::Change {
            start: ByteIdx(3),
            end: ByteIdx(5),
            text: String::new(),
        }]);
        assert!(a.follow(&[Arc::new(deleted)], 2));
        assert_eq!((a.start.0, a.at.0), (2, 3));
        assert_eq!(a.now(ByteIdx(1)), Some(ByteIdx(1)));
        let past = ChangeSet::new(vec![crate::buffer::change::Change {
            start: ByteIdx(1),
            end: ByteIdx(2),
            text: String::new(),
        }]);
        assert!(!a.follow(&[Arc::new(past)], 3), "Backspace past the word's start");
    }

    /// A selection (Cmd+D's) is part of the word before its head: the others replace as many
    /// chars back from their heads, not from their starts (`xfoo` keeps its `x`).
    #[test]
    fn insert_counts_from_each_selections_head() {
        let mut buf = Buffer::from_str("foo xfoo\nfoo bar");
        let mut view = View::new();
        view.selections = Selections::new(
            vec![
                Selection::new(ByteIdx(0), ByteIdx(3)),
                Selection::new(ByteIdx(5), ByteIdx(8)),
                Selection::new(ByteIdx(12), ByteIdx(9)), // head first: replaced whole all the same
            ],
            0,
        );
        let ins = Insertion {
            text: "foobar",
            select: None,
            before: 3,
            after: 0,
            additional: vec![],
        };
        insert(&mut buf, &mut view, &ins);
        assert_eq!(buf.rope.to_string(), "foobar xfoobar\nfoobar bar");
        assert_eq!(heads(&view), [(6, 6), (14, 14), (21, 21)]);
    }
}
