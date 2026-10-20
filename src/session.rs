//! Sessions: a workspace's layout as it was at quit, restored on the next start (no app state;
//! `app/session.rs` takes the snapshot and applies it).
//!
//! One JSON file per workspace, `<state dir>/sessions/<32 hex of the root's SHA-256>.json`, the
//! root stored inside (a file naming another root is refused). It holds the editor groups and
//! their split tree (group ids at the leaves, ratios included), each group's tabs — text tabs by
//! path with every selection (anchor, head; the primary's index) and the scroll, diff tabs by
//! path and [`Pair`], commit tabs by id (task tabs live in the panel, never here) — the active tab
//! and group, and the side and bottom panels' layout (the Explorer's Git History section's
//! collapse and height too). Paths inside the root are stored relative to it.
//!
//! [`save`] writes atomically (temp file, fsync, rename); [`read`] loads the file and every text
//! tab's file, dropping tabs whose file is gone or can't be opened, history diffs whose entry was
//! trimmed and commit tabs / commit diffs whose commit the repository no longer has, each with a
//! log line. Both are blocking.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::buffer::Buffer;
use crate::diff_view::{Pair, Text};
use crate::history::Snapshot;
use crate::layout::{GroupId, SidePanel, SplitTree};

/// The format's version; a file with another is ignored. Version 1 gained commit tabs and the
/// history section's `SideState` fields, all serde-defaulted so older files still parse.
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
    /// The Explorer's "Git History" section collapsed (older sessions: expanded).
    #[serde(default)]
    pub history_collapsed: bool,
    /// Its dragged height in rows (divider, header and commits; older sessions: the default).
    #[serde(default = "default_history_height")]
    pub history_height: u16,
}

fn default_history_height() -> u16 {
    crate::git_history::DEFAULT_SECTION_HEIGHT
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
    /// A commit tab, by its full id (dropped at restore when the commit is gone).
    Commit {
        commit: String,
    },
}

/// A diff side (`diff_view::Text`) as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Head,
    Index,
    WorkTree,
    Disk,
    Buffer,
    History {
        time: u64,
        hash: u64,
    },
    /// A commit's tree (the commit id as hex).
    Commit {
        id: String,
    },
    /// The empty tree (a root commit's diff).
    Empty,
}

/// A replace preview's side isn't stored (its tab isn't kept: `Err`).
impl TryFrom<Text> for Side {
    type Error = Text;

    fn try_from(t: Text) -> Result<Side, Text> {
        Ok(match t {
            Text::Head => Side::Head,
            Text::Index => Side::Index,
            Text::WorkTree => Side::WorkTree,
            Text::Disk => Side::Disk,
            Text::Buffer => Side::Buffer,
            Text::History(s) => Side::History {
                time: s.time,
                hash: s.hash,
            },
            Text::Commit(id) => Side::Commit { id: id.to_string() },
            Text::Empty => Side::Empty,
            Text::Replaced => return Err(t),
        })
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
            // A file's own commit id is always valid; a corrupt one becomes the zero id, whose
            // diff load then fails (`read` drops commit diffs whose commit is gone).
            Side::Commit { id } => Text::Commit(git2::Oid::from_str(&id).unwrap_or(git2::Oid::ZERO_SHA1)),
            Side::Empty => Text::Empty,
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
        group.tabs.retain(|tab| match tab {
            TabState::Text { path, .. } => {
                let path = resolve(root, path);
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
            TabState::Diff { path, left, right } => {
                let path = resolve(root, path);
                diff_still_there(root, &path, Side::pair(left.clone(), right.clone()))
            }
            TabState::Commit { commit } => commit_still_there(root, commit),
        });
        // The active tab stays active, else its neighbour does.
        group.active = active
            .and_then(|a| group.tabs.iter().position(|t| *t == a))
            .unwrap_or(group.active)
            .min(group.tabs.len().saturating_sub(1));
    }
    Ok(Some(Restore { session, buffers }))
}

/// Whether a commit tab can come back: the commit still exists in the root's repository (a
/// `git reset` may have dropped it).
fn commit_still_there(root: &Path, id: &str) -> bool {
    if commit_exists(root, id) {
        return true;
    }
    tracing::info!(commit = id, "session: commit tab not restored (the commit is gone)");
    false
}

fn commit_exists(root: &Path, id: &str) -> bool {
    let Some(repo) = crate::git::Repo::discover(root) else {
        return false;
    };
    let Ok(r) = repo.open() else { return false };
    let Ok(oid) = git2::Oid::from_str(id) else {
        return false;
    };
    r.find_commit(oid).is_ok()
}

/// Whether a diff tab can come back: a commit's change while the commit exists, a history diff
/// while its entry is kept, a git diff always (a deleted file's changes are a diff too), the
/// banner's Show Diff while the file exists.
fn diff_still_there(root: &Path, path: &Path, pair: Pair) -> bool {
    for side in [pair.left, pair.right] {
        if let crate::diff_view::Text::Commit(id) = side
            && !commit_exists(root, &id.to_string())
        {
            tracing::info!(path = %path.display(), "session: diff tab not restored (its commit is gone)");
            return false;
        }
    }
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
                history_collapsed: false,
                history_height: 12,
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

    /// An older session's side state (no history section fields) parses with their defaults.
    #[test]
    fn older_side_states_parse_with_the_history_defaults() {
        let side: SideState = serde_json::from_str(r#"{"visible":true,"width":30,"view":"explorer"}"#).unwrap();
        assert!(!side.history_collapsed);
        assert_eq!(side.history_height, crate::git_history::DEFAULT_SECTION_HEIGHT);
    }

    /// Commit tabs and commit diff tabs come back while their commit exists, and are dropped
    /// (with a log line) after the commit is gone (a reset).
    #[test]
    fn commit_tabs_round_trip_and_a_gone_commit_is_dropped() {
        use crate::git::test_repo::{commit_all, git, init};

        let dir = tempfile::tempdir().unwrap();
        let (state, root) = (dir.path().join("state"), dir.path().join("proj"));
        std::fs::create_dir_all(&root).unwrap();
        init(&root);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        commit_all(&root, "one");
        let first = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let mut s = session(&root);
        s.side.history_collapsed = true;
        s.side.history_height = 7;
        s.groups[0].tabs = vec![
            TabState::Commit { commit: first.clone() },
            TabState::Diff {
                path: "a.txt".into(),
                left: Side::Empty,
                right: Side::Commit { id: first.clone() },
            },
        ];
        s.groups[0].active = 1;
        s.groups.truncate(1);
        s.tree = SplitTree::new(0);
        s.active_group = 0;
        save(&state, &s).unwrap();
        let r = read(&state, &root, &HashSet::new()).unwrap().unwrap();
        assert_eq!(r.session.groups[0].tabs.len(), 2, "both come back");
        assert_eq!(r.session.groups[0].active, 1);
        assert!(r.session.side.history_collapsed);
        assert_eq!(r.session.side.history_height, 7);
        // The commits' text isn't loaded with the session.
        assert!(r.buffers.is_empty());

        // A commit no longer in the repository (a reset): its tab and its diff go.
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        commit_all(&root, "two");
        let second = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        git(&root, &["reset", "--hard", "HEAD~1"]);
        git(&root, &["reflog", "expire", "--expire=now", "--all"]);
        git(&root, &["gc", "--prune=now", "--quiet"]);
        let mut s2 = s.clone();
        s2.groups[0].tabs[0] = TabState::Commit { commit: second.clone() };
        s2.groups[0].tabs[1] = TabState::Diff {
            path: "a.txt".into(),
            left: Side::Empty,
            right: Side::Commit { id: second },
        };
        save(&state, &s2).unwrap();
        let r = read(&state, &root, &HashSet::new()).unwrap().unwrap();
        assert!(r.session.groups[0].tabs.is_empty(), "{:?}", r.session.groups[0].tabs);
    }

    /// A commit diff tab's sides keep their commit id and the empty-tree side through a session.
    #[test]
    fn commit_and_empty_sides_round_trip() {
        let commit = Text::Commit(git2::Oid::from_str(&"ab".repeat(20)).unwrap());
        let side = Side::try_from(commit).unwrap();
        assert_eq!(side, Side::Commit { id: "ab".repeat(20) });
        assert_eq!(Text::from(side), commit);
        assert_eq!(Text::from(Side::try_from(Text::Empty).unwrap()), Text::Empty);
        assert_eq!(Side::try_from(Text::Replaced), Err(Text::Replaced), "never stored");
        // JSON: the variants serialize and read back.
        let pair = Pair::commit(git2::Oid::from_str(&"cd".repeat(20)).unwrap(), None);
        let sides = (Side::try_from(pair.left).unwrap(), Side::try_from(pair.right).unwrap());
        let json = serde_json::to_string(&sides).unwrap();
        assert!(json.contains("empty") && json.contains("commit"));
        let (left, right): (Side, Side) = serde_json::from_str(&json).unwrap();
        assert_eq!((Text::from(left), Text::from(right)), (pair.left, pair.right));
    }
}
