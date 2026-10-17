//! Sessions: a workspace's layout as it was at quit, restored on the next start (no app state;
//! `app/session.rs` takes the snapshot and applies it).
//!
//! One JSON file per workspace, `<state dir>/sessions/<32 hex of the root's SHA-256>.json`, the
//! root stored inside (a file naming another root is refused). It holds the editor groups and
//! their split tree (group ids at the leaves, ratios included), each group's tabs — text tabs by
//! path with every selection (anchor, head; the primary's index) and the scroll, diff tabs by
//! path and [`Pair`] (task tabs live in the panel, never here) — the active tab and group, and the
//! side and bottom panels' layout. Paths inside the root are stored relative to it.
//!
//! [`save`] writes atomically (temp file, fsync, rename); [`read`] loads the file and every text
//! tab's file, dropping tabs whose file is gone or can't be opened and history diffs whose entry
//! was trimmed, each with a log line. Both are blocking.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::buffer::Buffer;
use crate::diff_view::{Pair, Text};
use crate::history::Snapshot;
use crate::layout::{GroupId, SidePanel, SplitTree};

/// The format's version; a file with another is ignored.
pub const VERSION: u32 = 1;
/// Under the state dir.
pub const DIR: &str = "sessions";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    /// The workspace root (the file name is its hash).
    pub root: PathBuf,
    pub side: SideState,
    pub panel: PanelState,
    pub tree: SplitTree,
    /// Every group in `tree`, by id.
    pub groups: Vec<GroupState>,
    pub active_group: GroupId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideState {
    pub visible: bool,
    pub width: u16,
    pub view: SidePanel,
}

/// The bottom panel (it always comes back on Problems).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelState {
    pub visible: bool,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupState {
    pub id: GroupId,
    pub tabs: Vec<TabState>,
    /// Index into `tabs`.
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabState {
    Text {
        /// Relative to the root when inside it.
        path: PathBuf,
        /// (anchor, head) byte offsets, in the view's order.
        selections: Vec<(usize, usize)>,
        primary: usize,
        top_line: usize,
        left_col: usize,
    },
    Diff {
        path: PathBuf,
        left: Side,
        right: Side,
    },
}

impl TabState {
    pub fn path(&self) -> &Path {
        match self {
            TabState::Text { path, .. } | TabState::Diff { path, .. } => path,
        }
    }
}

/// A diff side (`diff_view::Text`) as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Head,
    Index,
    WorkTree,
    Disk,
    Buffer,
    History { time: u64, hash: u64 },
}

impl From<Text> for Side {
    fn from(t: Text) -> Side {
        match t {
            Text::Head => Side::Head,
            Text::Index => Side::Index,
            Text::WorkTree => Side::WorkTree,
            Text::Disk => Side::Disk,
            Text::Buffer => Side::Buffer,
            Text::History(s) => Side::History {
                time: s.time,
                hash: s.hash,
            },
        }
    }
}

impl From<Side> for Text {
    fn from(s: Side) -> Text {
        match s {
            Side::Head => Text::Head,
            Side::Index => Text::Index,
            Side::WorkTree => Text::WorkTree,
            Side::Disk => Text::Disk,
            Side::Buffer => Text::Buffer,
            Side::History { time, hash } => Text::History(Snapshot { time, hash }),
        }
    }
}

impl Side {
    pub fn pair(left: Side, right: Side) -> Pair {
        Pair {
            left: left.into(),
            right: right.into(),
        }
    }
}

/// `path` as stored: relative to `root` when inside it.
pub fn stored(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}

/// A stored path made absolute again.
pub fn resolve(root: &Path, path: &Path) -> PathBuf {
    root.join(path)
}

/// The session file of the workspace at `root`.
pub fn file(state_dir: &Path, root: &Path) -> PathBuf {
    let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let name: String = hash[..16].iter().map(|b| format!("{b:02x}")).collect();
    state_dir.join(DIR).join(format!("{name}.json"))
}

/// The workspace's session: `Ok(None)` when it has none, `Err` when the file can't be read or
/// isn't a session of this root in this version.
/// Blocking: call from spawn_blocking.
pub fn load(state_dir: &Path, root: &Path) -> Result<Option<Session>, String> {
    let path = file(state_dir, root);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let session: Session = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if session.version != VERSION {
        return Err(format!(
            "{}: version {}, not {VERSION}",
            path.display(),
            session.version
        ));
    }
    if session.root != root {
        return Err(format!(
            "{}: the session of {}, not {}",
            path.display(),
            session.root.display(),
            root.display()
        ));
    }
    Ok(Some(session))
}

/// Writes the session atomically (a temp file beside it, fsynced, renamed over it).
/// Blocking: call from spawn_blocking.
pub fn save(state_dir: &Path, session: &Session) -> std::io::Result<()> {
    let path = file(state_dir, &session.root);
    let dir = path.parent().expect("session files are in a folder");
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A session read for restoring, with its text tabs' files loaded.
pub struct Restore {
    /// Tabs whose file is missing or unreadable left out (groups may be left empty).
    pub session: Session,
    /// Each text tab's file (absolute path), once.
    pub buffers: Vec<(PathBuf, Buffer)>,
}

/// Reads the session of `root` and opens its text tabs' files, except those in `open` (already
/// open in the app). Tabs of missing or unopenable files and history diffs of trimmed entries
/// are dropped with a log line. `Ok(None)`: no session.
/// Blocking: call from spawn_blocking.
pub fn read(state_dir: &Path, root: &Path, open: &HashSet<PathBuf>) -> Result<Option<Restore>, String> {
    let Some(mut session) = load(state_dir, root)? else {
        return Ok(None);
    };
    let mut buffers = Vec::new();
    let mut loaded = HashSet::new();
    let mut failed = HashSet::new();
    for group in &mut session.groups {
        let active = group.tabs.get(group.active).cloned();
        group.tabs.retain(|tab| {
            let path = resolve(root, tab.path());
            match tab {
                TabState::Text { .. } => {
                    if open.contains(&path) || loaded.contains(&path) {
                        return true;
                    }
                    if failed.contains(&path) {
                        return false;
                    }
                    if !path.is_file() {
                        tracing::info!(path = %path.display(), "session: file gone, tab not restored");
                        failed.insert(path);
                        return false;
                    }
                    match Buffer::open(&path) {
                        Ok(buffer) => {
                            loaded.insert(path.clone());
                            buffers.push((path, buffer));
                            true
                        }
                        Err(e) => {
                            tracing::warn!(path = %path.display(), "session: tab not restored: {e}");
                            failed.insert(path);
                            false
                        }
                    }
                }
                TabState::Diff { left, right, .. } => diff_still_there(root, &path, Side::pair(*left, *right)),
            }
        });
        // The active tab stays active, else its neighbour does.
        group.active = active
            .and_then(|a| group.tabs.iter().position(|t| *t == a))
            .unwrap_or(group.active)
            .min(group.tabs.len().saturating_sub(1));
    }
    Ok(Some(Restore { session, buffers }))
}

/// Whether a diff tab can come back: a history diff while its entry is kept, a git diff always
/// (a deleted file's changes are a diff too), the banner's Show Diff while the file exists.
fn diff_still_there(root: &Path, path: &Path, pair: Pair) -> bool {
    let keep = match pair.snapshot() {
        Some(snapshot) => crate::history::relative(root, path)
            .and_then(|rel| crate::history::entries(root, &rel).ok())
            .is_some_and(|entries| entries.iter().any(|e| e.snapshot() == snapshot)),
        None if pair.needs_repo() => true,
        None => path.is_file(),
    };
    if !keep {
        tracing::info!(path = %path.display(), "session: diff tab not restored (its text is gone)");
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::SplitDir;

    fn session(root: &Path) -> Session {
        let mut tree = SplitTree::new(0);
        tree.split(0, SplitDir::Right, 3);
        Session {
            version: VERSION,
            root: root.to_path_buf(),
            side: SideState {
                visible: false,
                width: 41,
                view: SidePanel::Search,
            },
            panel: PanelState {
                visible: true,
                height: 14,
            },
            tree,
            groups: vec![
                GroupState {
                    id: 0,
                    tabs: vec![
                        TabState::Text {
                            path: "a.txt".into(),
                            selections: vec![(0, 2), (5, 5)],
                            primary: 1,
                            top_line: 3,
                            left_col: 2,
                        },
                        TabState::Diff {
                            path: "a.txt".into(),
                            left: Side::History {
                                time: 7,
                                hash: u64::MAX,
                            },
                            right: Side::Buffer,
                        },
                    ],
                    active: 1,
                },
                GroupState {
                    id: 3,
                    tabs: vec![TabState::Text {
                        path: "/elsewhere/b.txt".into(),
                        selections: vec![(1, 1)],
                        primary: 0,
                        top_line: 0,
                        left_col: 0,
                    }],
                    active: 0,
                },
            ],
            active_group: 3,
        }
    }

    #[test]
    fn saves_and_loads_by_root() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        assert_eq!(load(&state, &root), Ok(None), "no session yet");
        let s = session(&root);
        save(&state, &s).unwrap();
        assert_eq!(load(&state, &root), Ok(Some(s.clone())));
        assert!(file(&state, &root).starts_with(state.join(DIR)));
        assert_ne!(file(&state, &root), file(&state, &dir.path().join("other")));
        assert_eq!(load(&state, &dir.path().join("other")), Ok(None), "another root's");
        // Saving again replaces it; no temp file is left.
        let mut s2 = s.clone();
        s2.active_group = 0;
        save(&state, &s2).unwrap();
        assert_eq!(load(&state, &root), Ok(Some(s2.clone())));
        assert_eq!(std::fs::read_dir(state.join(DIR)).unwrap().count(), 1);

        // A write that fails part way (here: its temp file can't be made) leaves the old
        // session whole.
        let tmp = file(&state, &root).with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::create_dir(&tmp).unwrap();
        let mut s3 = s.clone();
        s3.active_group = 3;
        s3.groups.truncate(1);
        assert!(save(&state, &s3).is_err());
        assert_eq!(load(&state, &root), Ok(Some(s2)), "the old session kept");
    }

    #[test]
    fn bad_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        let path = file(&state, &root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&state, &root).is_err());
        // Another root's session under this name (a copied state dir), or another version.
        let mut s = session(&dir.path().join("other"));
        std::fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();
        assert!(load(&state, &root).unwrap_err().contains("the session of"));
        s.root = root.clone();
        s.version = VERSION + 1;
        std::fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();
        assert!(load(&state, &root).unwrap_err().contains("version"));
    }

    #[test]
    fn read_drops_tabs_of_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        let mut s = session(&root);
        // The history entry doesn't exist: that diff tab goes; b.txt is missing.
        save(&state, &s).unwrap();
        let r = read(&state, &root, &HashSet::new()).unwrap().unwrap();
        assert_eq!(r.session.groups[0].tabs.len(), 1);
        assert_eq!(r.session.groups[0].active, 0);
        assert!(r.session.groups[1].tabs.is_empty());
        assert_eq!(r.buffers.len(), 1);
        assert_eq!(r.buffers[0].0, root.join("a.txt"));

        // A file already open isn't loaded again; a git diff stays without its file.
        s.groups[1].tabs = vec![TabState::Diff {
            path: "gone.txt".into(),
            left: Side::Index,
            right: Side::WorkTree,
        }];
        save(&state, &s).unwrap();
        let open = HashSet::from([root.join("a.txt")]);
        let r = read(&state, &root, &open).unwrap().unwrap();
        assert!(r.buffers.is_empty());
        assert_eq!(r.session.groups[0].tabs.len(), 1);
        assert_eq!(r.session.groups[1].tabs.len(), 1);
    }

    #[test]
    fn paths_inside_the_root_are_relative() {
        let root = Path::new("/w/proj");
        assert_eq!(stored(root, Path::new("/w/proj/src/a.rs")), Path::new("src/a.rs"));
        assert_eq!(stored(root, Path::new("/etc/hosts")), Path::new("/etc/hosts"));
        assert_eq!(resolve(root, Path::new("src/a.rs")), Path::new("/w/proj/src/a.rs"));
        assert_eq!(resolve(root, Path::new("/etc/hosts")), Path::new("/etc/hosts"));
    }
}
