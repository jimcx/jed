//! Files changed on disk, the model (no app state; `app/disk.rs` drives it): `check` reads an
//! open buffer's file off the UI thread and says whether it changed since the buffer last knew
//! it (`buffer::file::Stamp`s, so our own saves' events find nothing new, however late they
//! arrive), and if so how to turn the buffer's text into the file's (`changes`). Also the
//! dirty-file banner's actions.

use std::io;
use std::path::PathBuf;

use ropey::Rope;

use crate::buffer::change::ChangeSet;
use crate::buffer::file::{self, DiskMeta, LoadError, Stamp};
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::git::diff;
use crate::indentation::{self, Guess, Indentation};

/// One check of an open file, as `check` needs it.
pub struct Check {
    pub path: PathBuf,
    /// What the buffer last knew of the file (`Buffer::disk`).
    pub known: Option<Stamp>,
    /// The buffer's text to diff the file against: given when a change would be reloaded (a
    /// clean buffer, or `force`), else only the stamp is compared.
    pub base: Option<Rope>,
    /// "Reload from disk": read and diff the file even if it holds what the buffer knew.
    pub force: bool,
    /// The settings' indentation, for guessing the file's (`indentation::guess`).
    pub defaults: Indentation,
}

/// What `check` found.
#[derive(Debug)]
pub enum OnDisk {
    /// No file there (deleted, or renamed away).
    Missing,
    /// The file holds what the buffer last knew.
    Unchanged,
    /// The file holds something else (or `force`): `stamp` is what. `reload` is how to take it
    /// in, when the check had a `base`: `Err` when the file is no longer text (binary).
    Changed {
        stamp: Stamp,
        reload: Option<Result<Reload, LoadError>>,
    },
}

/// Turning the buffer's text into the file's: `changes` on the `base` text, and the file's
/// BOM, line ending, read-only state (invalid UTF-8, decoded lossily as loading does) and stamp.
#[derive(Debug)]
pub struct Reload {
    pub changes: ChangeSet,
    pub meta: DiskMeta,
    /// The new text's indentation.
    pub guess: Guess,
}

/// Blocking: call from `spawn_blocking`. Reads the file, compares and (with a base) decodes it as
/// `Buffer::open` would and diffs it against the base.
pub fn check(c: &Check) -> io::Result<OnDisk> {
    let Some(bytes) = file::read(&c.path)? else {
        return Ok(OnDisk::Missing);
    };
    let stamp = Stamp::of(&bytes);
    if Some(stamp) == c.known && !c.force {
        return Ok(OnDisk::Unchanged);
    }
    let reload = c.base.as_ref().map(|base| {
        let loaded = file::decode(bytes)?;
        let meta = loaded.meta().expect("decoded bytes have a stamp");
        Ok(Reload {
            changes: changes(base, &loaded.rope),
            meta,
            guess: indentation::guess(&loaded.rope, c.defaults),
        })
    });
    Ok(OnDisk::Changed { stamp, reload })
}

/// The changes from `old` to `new`: git's line diff (context 0: one change per changed run of
/// lines, so a cursor in an unchanged line stays on it), each change then trimmed to the bytes
/// that differ (`format::minimize`). Positions are in `old`. Falls back to one trimmed
/// whole-text change if the diff fails.
pub fn changes(old: &Rope, new: &Rope) -> ChangeSet {
    let (old_text, new_text) = (old.to_string(), new.to_string());
    let whole = || {
        let mut set = ChangeSet::with_capacity(1, new_text.len());
        set.push(ByteIdx(0), ByteIdx(old_text.len()), &new_text);
        set
    };
    let set = match diff::diff(old_text.as_bytes(), new_text.as_bytes(), 0) {
        Ok(d) if !d.binary => {
            // git's lines: split after each `\n` (a CRLF line keeps its `\r`), so every
            // boundary is a char boundary.
            let (old_starts, new_starts) = (line_starts(&old_text), line_starts(&new_text));
            let at = |starts: &[usize], len: usize, line: usize| starts.get(line).copied().unwrap_or(len);
            let mut set = ChangeSet::with_capacity(d.hunks.len(), 0);
            for h in &d.hunks {
                let start = at(&old_starts, old_text.len(), h.old.start);
                let end = at(&old_starts, old_text.len(), h.old.end);
                let from = at(&new_starts, new_text.len(), h.new.start);
                let to = at(&new_starts, new_text.len(), h.new.end);
                set.push(ByteIdx(start), ByteIdx(end), &new_text[from..to]);
            }
            set
        }
        Ok(_) => whole(),
        Err(e) => {
            tracing::warn!("diff for a reload failed: {e}");
            whole()
        }
    };
    crate::format::minimize(old, &set)
}

/// Each line's start offset in `text` (lines end after `\n`; the text after the last one is a
/// line if not empty).
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .filter(|&i| i < text.len() || i == 0)
        .collect()
}

/// A button on the banner shown over a dirty buffer whose file changed on disk (and over a diff
/// tab of that file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    Reload,
    KeepMine,
    /// The diff view of the file on disk against the buffer.
    ShowDiff,
}

impl BannerAction {
    /// In the order the banner shows them.
    pub const ALL: [BannerAction; 3] = [BannerAction::Reload, BannerAction::KeepMine, BannerAction::ShowDiff];

    pub fn label(self) -> &'static str {
        match self {
            BannerAction::Reload => "Reload",
            BannerAction::KeepMine => "Keep Mine",
            BannerAction::ShowDiff => "Show Diff",
        }
    }

    /// The command the button runs (on its group's active tab).
    pub fn command(self) -> Command {
        match self {
            BannerAction::Reload => Command::ReloadFromDisk,
            BannerAction::KeepMine => Command::KeepMine,
            BannerAction::ShowDiff => Command::CompareWithSaved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, new: &str) -> (String, ChangeSet) {
        let mut rope = Rope::from_str(old);
        let set = changes(&rope, &Rope::from_str(new));
        set.apply(&mut rope);
        (rope.to_string(), set)
    }

    #[test]
    fn changes_turn_old_into_new_minimally() {
        for (old, new) in [
            ("", ""),
            ("", "a\nb"),
            ("a\nb\n", ""),
            ("one\ntwo\nthree\n", "one\nTWO\nthree\n"),
            ("one\ntwo\nthree", "zero\none\ntwo\nthree!"),
            ("a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\nd\r\n"),
            ("é\nü\n", "é\nüx\n"),
            ("same", "same"),
        ] {
            assert_eq!(apply(old, new).0, new, "{old:?} -> {new:?}");
        }
        // Two separate lines changed: two small changes, the lines between untouched.
        let old = "fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n";
        let new = "fn a() {\n    10\n}\n\nfn b() {\n    20\n}\n";
        let (_, set) = apply(old, new);
        let spans: Vec<(usize, usize, &str)> = set.changes().map(|(s, e, t)| (s.0, e.0, t)).collect();
        assert_eq!(spans, [(14, 14, "0"), (32, 32, "0")]);
        assert!(apply("x", "x").1.is_empty(), "nothing to change");
    }

    #[test]
    fn check_compares_stamps_and_diffs_with_a_base() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let check = |known, base: Option<&str>, force| Check {
            path: path.clone(),
            known,
            base: base.map(Rope::from_str),
            force,
            defaults: crate::edit::Settings::default().indentation(),
        };
        assert!(matches!(check(None, None, false).run(), OnDisk::Missing));
        std::fs::write(&path, "one\n").unwrap();
        let stamp = Stamp::of(b"one\n");
        assert!(matches!(check(Some(stamp), None, false).run(), OnDisk::Unchanged));
        match check(None, None, false).run() {
            OnDisk::Changed { stamp: s, reload: None } => assert_eq!(s, stamp),
            other => panic!("{other:?}"),
        }
        // Forced: diffed even though unchanged.
        match check(Some(stamp), Some("one\ntwo\n"), true).run() {
            OnDisk::Changed {
                reload: Some(Ok(r)), ..
            } => {
                assert_eq!(r.meta.stamp, stamp);
                assert_eq!(r.changes.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        std::fs::write(&path, b"a\0b").unwrap();
        assert!(matches!(
            check(Some(stamp), Some("one\n"), false).run(),
            OnDisk::Changed {
                reload: Some(Err(LoadError::Binary(_))),
                ..
            }
        ));
        std::fs::write(&path, b"a\xFFb").unwrap();
        match check(Some(stamp), Some("one\n"), false).run() {
            OnDisk::Changed {
                reload: Some(Ok(r)), ..
            } => assert!(r.meta.read_only.is_some(), "not UTF-8: read-only, as loading does"),
            other => panic!("{other:?}"),
        }
    }

    impl Check {
        fn run(&self) -> OnDisk {
            check(self).unwrap()
        }
    }
}
