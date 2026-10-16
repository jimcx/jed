//! The Locations list's model (no drawing; `ui::panel` draws it in the bottom panel's
//! "LOCATIONS" tab): where go to definition found several places (9g's references go here too),
//! grouped by file as a `grouped` list, each place with a preview of its line.
//!
//! - `parse` reads a `Location`, `Location[]` or `LocationLink[]` answer (a link's
//!   `targetSelectionRange`: the name, where VS Code puts the cursor) into `Location`s with
//!   absolute paths and LSP positions.
//! - Previews: a file open in a buffer is read at once (`Locations::read`); others are read off
//!   the UI thread by the app (`unread`, then `read` when the text arrives). Until then a row
//!   shows its line and column.
//! - Opening one goes through `problems::Target` with `Place::Lsp`, read in the file's text as
//!   loaded (`App::open_file` with `Goto::Problem`).

use std::path::{Path, PathBuf};

use ropey::Rope;
use serde_json::Value;

use crate::buffer::pos::{self, LspPos, PosEncoding};
use crate::grouped::{self, Group, ListState, RowKey};
use crate::lsp::uri;
use crate::problems::{Place, Target};
use crate::workspace_search::{LineMatch, line_match};

/// One place a server answered with.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub path: PathBuf,
    pub start: LspPos,
    pub end: LspPos,
}

/// Reads a `Location | Location[] | LocationLink[] | null` answer; places in anything but a
/// `file:` URI are skipped.
pub fn parse(result: &Value) -> Vec<Location> {
    let items: Vec<&Value> = match result {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|v| {
            let (uri, range) = match v.get("targetUri") {
                Some(u) => (u, v.get("targetSelectionRange").or_else(|| v.get("targetRange"))?),
                None => (v.get("uri")?, v.get("range")?),
            };
            let path = uri::to_path(&uri.as_str()?.parse().ok()?)?;
            let pos = |p: &Value| {
                Some(LspPos {
                    line: p.get("line")?.as_u64()? as u32,
                    character: p.get("character")?.as_u64()? as u32,
                })
            };
            let start = pos(range.get("start")?)?;
            let end = pos(range.get("end")?)?.max(start);
            Some(Location { path, start, end })
        })
        .collect()
}

/// One place in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub start: LspPos,
    pub end: LspPos,
    /// Its line with the place marked, once the file's text was read.
    pub preview: Option<LineMatch>,
}

/// One file of the list: its places in position order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: PathBuf,
    pub items: Vec<Item>,
}

impl Group for File {
    fn path(&self) -> &Path {
        &self.path
    }
    fn items(&self) -> usize {
        self.items.len()
    }
}

/// The list: what it shows (`title`, e.g. "Definitions of foo"), its files, their positions'
/// encoding, scroll and selection.
#[derive(Debug)]
pub struct Locations {
    pub title: String,
    pub encoding: PosEncoding,
    pub files: Vec<File>,
    pub list: ListState,
    /// Numbers lists, so file reads for a replaced one are dropped.
    pub generation: u64,
}

impl Locations {
    /// Groups `places` by file (sorted by path, then position; duplicates dropped).
    pub fn new(title: String, encoding: PosEncoding, mut places: Vec<Location>, generation: u64) -> Self {
        places.sort();
        places.dedup();
        let mut files: Vec<File> = Vec::new();
        for l in places {
            let item = Item {
                start: l.start,
                end: l.end,
                preview: None,
            };
            match files.last_mut() {
                Some(f) if f.path == l.path => f.items.push(item),
                _ => files.push(File {
                    path: l.path,
                    items: vec![item],
                }),
            }
        }
        Self {
            title,
            encoding,
            files,
            list: ListState::default(),
            generation,
        }
    }

    pub fn total(&self) -> usize {
        self.files.iter().map(|f| f.items.len()).sum()
    }

    /// Files whose previews aren't read yet.
    pub fn unread(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|f| f.items.iter().any(|i| i.preview.is_none()))
            .map(|f| f.path.clone())
            .collect()
    }

    /// Fills the previews of `path`'s places from its text.
    pub fn read(&mut self, path: &Path, text: &Rope) {
        let encoding = self.encoding;
        if let Some((i, _)) = grouped::group(&self.files, path) {
            for item in &mut self.files[i].items {
                item.preview = Some(preview(text, item.start, item.end, encoding));
            }
        }
    }

    /// The place Enter opens: the selected one, or a file row's first (which becomes the
    /// selection, as in the Problems list).
    pub fn target(&mut self) -> Option<Target> {
        let (path, offset) = self.list.selected()?.clone();
        let (_, f) = grouped::group(&self.files, &path)?;
        let k = offset.max(1).min(f.items.len()) - 1;
        let item = &f.items[k];
        let place = Place::Lsp {
            start: item.start,
            end: item.end,
            encoding: self.encoding,
        };
        self.list.select_key(&self.files, &(path.clone(), k + 1));
        Some(Target { path, place })
    }

    /// Selects row `key` (a click), if its file is in the list.
    pub fn select_key(&mut self, key: &RowKey) -> bool {
        self.list.select_key(&self.files, key)
    }
}

/// `start`'s line in `text` with `start..end` marked (cut at the line's end), as a search result
/// line is (`workspace_search::line_match`: a long line is cut around it).
pub fn preview(text: &Rope, start: LspPos, end: LspPos, encoding: PosEncoding) -> LineMatch {
    let a = pos::from_lsp_pos(text, start, encoding);
    let b = pos::from_lsp_pos(text, end, encoding).max(a);
    let line = pos::line_of(text, a);
    let line_start = pos::line_start(text, line);
    let line_text = pos::line_text(text, line);
    let line_text = line_text.trim_end_matches(['\n', '\r']);
    let from = (a.0 - line_start.0).min(line_text.len());
    let to = (b.0 - line_start.0).clamp(from, line_text.len());
    let range = from..to;
    line_match(line, line_text, vec![range])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn range(l1: u32, c1: u32, l2: u32, c2: u32) -> Value {
        json!({ "start": { "line": l1, "character": c1 }, "end": { "line": l2, "character": c2 } })
    }

    #[test]
    fn parse_reads_locations_and_links() {
        let one = parse(&json!({ "uri": "file:///w/a%20b.cpp", "range": range(1, 2, 1, 5) }));
        assert_eq!(
            one,
            [Location {
                path: PathBuf::from("/w/a b.cpp"),
                start: LspPos { line: 1, character: 2 },
                end: LspPos { line: 1, character: 5 },
            }]
        );
        let links = parse(&json!([
            { "targetUri": "file:///w/b.cpp", "targetRange": range(0, 0, 9, 1), "targetSelectionRange": range(3, 4, 3, 7) },
            { "targetUri": "file:///w/c.cpp", "targetRange": range(2, 0, 2, 1) },
            { "uri": "untitled:x", "range": range(0, 0, 0, 1) },
            { "uri": "file:///w/d.cpp", "range": range(5, 3, 4, 0) },
        ]));
        let got: Vec<_> = links
            .iter()
            .map(|l| (l.path.to_str().unwrap(), l.start, l.end))
            .collect();
        let p = |line, character| LspPos { line, character };
        assert_eq!(
            got,
            [
                ("/w/b.cpp", p(3, 4), p(3, 7)),
                ("/w/c.cpp", p(2, 0), p(2, 1)),
                ("/w/d.cpp", p(5, 3), p(5, 3))
            ]
        );
        assert!(parse(&Value::Null).is_empty());
        assert!(parse(&json!([])).is_empty());
    }

    #[test]
    fn grouped_previewed_and_opened() {
        let p = |line, character| LspPos { line, character };
        let loc = |path: &str, line, from, to| Location {
            path: PathBuf::from(path),
            start: p(line, from),
            end: p(line, to),
        };
        let mut l = Locations::new(
            "Definitions of x".into(),
            PosEncoding::Utf16,
            vec![
                loc("/w/b", 1, 0, 1),
                loc("/w/a", 2, 3, 4),
                loc("/w/b", 0, 2, 4),
                loc("/w/a", 2, 3, 4),
            ],
            1,
        );
        assert_eq!(l.total(), 3, "the duplicate dropped");
        assert_eq!(
            l.files.iter().map(|f| f.path.to_str().unwrap()).collect::<Vec<_>>(),
            ["/w/a", "/w/b"]
        );
        assert_eq!(l.files[1].items[0].start, p(0, 2), "by position");
        assert_eq!(l.unread().len(), 2);
        // UTF-16: the 😀 is two units.
        l.read(Path::new("/w/b"), &Rope::from_str("😀xy z\r\nsecond\n"));
        let pv = l.files[1].items[0].preview.clone().unwrap();
        assert_eq!((pv.line, pv.text.as_str(), pv.ranges.len()), (0, "😀xy z", 1));
        assert_eq!(pv.ranges[0], 4..6);
        assert_eq!(l.files[1].items[1].preview.as_ref().unwrap().text, "second");
        assert_eq!(l.unread(), [PathBuf::from("/w/a")]);

        assert!(l.target().is_none(), "nothing selected");
        assert!(l.select_key(&(PathBuf::from("/w/b"), 0)));
        let t = l.target().unwrap();
        assert_eq!(t.path, PathBuf::from("/w/b"));
        assert_eq!(
            t.place,
            Place::Lsp {
                start: p(0, 2),
                end: p(0, 4),
                encoding: PosEncoding::Utf16
            }
        );
        assert_eq!(
            l.list.selected(),
            Some(&(PathBuf::from("/w/b"), 1)),
            "a file row: its first"
        );
    }
}
