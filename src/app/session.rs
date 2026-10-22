//! Sessions (`crate::session`): the layout restored at start, saved at quit.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::{App, AppEvent, Focus};
use crate::buffer::pos::ByteIdx;
use crate::buffer::{Buffer, BufferId};
use crate::diff_view::DiffTab;
use crate::group::{self, Group, Groups, Tab, TextTab};
use crate::hex::{HexDoc, HexTab};
use crate::layout::{MIN_SIDE_WIDTH, PanelTab};
use crate::selection::{Selection, Selections};
use crate::session::{self, GroupState, PanelState, Restore, Session, Side, SideState, TabState};
use crate::view::View;

/// The session's restore and save (`app.session`).
#[derive(Debug, Default)]
pub struct SessionState {
    /// The restore's read (the session file and its files) is running: quitting meanwhile
    /// saves nothing, so the session isn't lost.
    loading: bool,
    /// The file given on the command line came with a line (`jed a.rs:12`): its cursor wins
    /// over the one the session has for it.
    pub(super) initial_goto: bool,
    /// Restores applied (tests wait for them).
    pub restored: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
impl SessionState {
    pub fn loading(&self) -> bool {
        self.loading
    }
}

impl App {
    /// Reads the workspace's session off-thread (`start`), with every text tab's file, →
    /// `AppEvent::SessionRead` → `session_read`. Nothing without a state dir (tests).
    pub(super) fn restore_session(&mut self) {
        let Some(dir) = self.state_dir.clone() else { return };
        let root = self.workspace.root.clone();
        let open: HashSet<PathBuf> = self.buffers.iter().map(|(_, b)| b.path.clone()).collect();
        let open_hex: HashSet<PathBuf> = self.groups.hex_tabs().map(|h| h.path.clone()).collect();
        self.session.loading = true;
        let tx = self.tx.clone();
        let indentation = self.settings.indentation();
        tokio::task::spawn_blocking(move || {
            let result = session::read(&dir, &root, &open, &open_hex, indentation);
            let _ = tx.send(AppEvent::SessionRead(Box::new(result)));
        });
    }

    /// The session's groups, split tree, tabs, selections, scroll and panels replace the start
    /// layout; tabs already open (the command line's file) go on top in the restored active
    /// group, activated. A bad session is logged and changes nothing.
    /// Then offers the crash backups found meanwhile (their tabs now exist).
    pub(super) fn session_read(&mut self, result: Result<Option<Restore>, String>) {
        self.apply_session(result);
        self.offer_backups();
    }

    fn apply_session(&mut self, result: Result<Option<Restore>, String>) {
        self.session.loading = false;
        let Restore { session, buffers, hex } = match result {
            Ok(Some(r)) => r,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("session not restored: {e}");
                return;
            }
        };
        let Session {
            side,
            panel,
            mut tree,
            groups,
            mut active_group,
            ..
        } = session;
        let ids: Vec<_> = groups.iter().map(|g| g.id).collect();
        if let Err(e) = group::check_restore(&tree, &ids, active_group) {
            tracing::warn!("session not restored: {e}");
            return;
        }
        // Groups left without tabs (their files gone) close, as closing their last tab would.
        for g in groups.iter().filter(|g| g.tabs.is_empty()) {
            if tree.groups().len() > 1
                && let Some(focus) = tree.remove(g.id)
                && active_group == g.id
            {
                active_group = focus;
            }
        }
        let kept = tree.groups();
        let mut loaded: HashMap<PathBuf, Buffer> = buffers.into_iter().collect();
        // Hex tabs' bytes: read for the restore, else those of a hex tab open before it.
        let mut hex: HashMap<PathBuf, HexDoc> = hex.into_iter().collect();
        for h in self.groups.hex_tabs() {
            hex.entry(h.path.clone()).or_insert_with(|| h.doc.clone());
        }
        let root = self.workspace.root.clone();
        let mut restored = Vec::new();
        for g in groups.into_iter().filter(|g| kept.contains(&g.id)) {
            let mut tabs = Vec::new();
            for tab in g.tabs {
                match tab {
                    TabState::Text {
                        path,
                        selections,
                        primary,
                        top_line,
                        left_col,
                    } => {
                        let path = session::resolve(&root, &path);
                        let id = match self.buffers.find_path(&path) {
                            Some(id) => id,
                            None => match loaded.remove(&path) {
                                Some(buffer) => self.register_buffer(buffer, None).0,
                                None => continue,
                            },
                        };
                        let rope = &self.buffers.get(id).expect("just found").rope;
                        let view = restored_view(rope, &selections, primary, top_line, left_col);
                        tabs.push(Tab::Text(TextTab { buffer: id, view }));
                    }
                    TabState::Diff { path, left, right } => {
                        let path = session::resolve(&root, &path);
                        let id = self.next_diff_id();
                        tabs.push(Tab::Diff(Box::new(DiffTab::new(id, path, Side::pair(left, right)))));
                    }
                    TabState::Commit { commit } => {
                        let id = self.commit_buffer(&commit, None);
                        tabs.push(Tab::Text(TextTab {
                            buffer: id,
                            view: View::new(),
                        }));
                    }
                    TabState::Hex {
                        path,
                        cursor,
                        column,
                        top,
                    } => {
                        let path = session::resolve(&root, &path);
                        let Some(doc) = hex.get(&path) else { continue };
                        let mut tab = HexTab::new(path, doc.clone());
                        tab.place(cursor, column, top);
                        tabs.push(Tab::Hex(Box::new(tab)));
                    }
                }
            }
            let active = g.active.min(tabs.len().saturating_sub(1));
            restored.push((
                g.id,
                Group {
                    tabs,
                    active,
                    ..Group::default()
                },
            ));
        }

        let mut fresh = Groups::default();
        if let Err(e) = fresh.restore(tree, restored, active_group) {
            // Checked above, but never lose the open tabs over it.
            tracing::warn!("session not restored: {e}");
            self.drop_unshown_buffers();
            return;
        }
        // The tabs open before the restore, the active one last so it ends active.
        let mut before = std::mem::replace(&mut self.groups, fresh);
        let (old_active, old_tab) = (before.active, before.active_group().active);
        let mut carried = Vec::new();
        let mut last = None;
        for gid in before.ids() {
            let g = before.get_mut(gid).expect("listed");
            for (i, tab) in g.tabs.drain(..).enumerate() {
                if gid == old_active && i == old_tab {
                    last = Some(tab);
                } else {
                    carried.push(tab);
                }
            }
        }
        carried.extend(last);

        let tabs: usize = self
            .groups
            .ids()
            .iter()
            .filter_map(|&g| self.groups.get(g))
            .map(|g| g.tabs.len())
            .sum();
        let had_tabs = !carried.is_empty();
        for tab in carried {
            self.carry_tab(tab);
        }
        self.drop_unshown_buffers();

        self.ui.side_visible = side.visible;
        self.ui.side_width = side.width.max(MIN_SIDE_WIDTH);
        self.ui.side_panel = side.view;
        self.git_history.set_collapsed(side.history_collapsed);
        self.git_history.section_height = side.history_height;
        self.ui.panel_visible = panel.visible;
        self.ui.panel_height = panel.height;
        self.ui.panel_tab = PanelTab::Problems;
        let has_tab = !self.group().tabs.is_empty();
        if had_tabs || has_tab || (self.ui.focus == Focus::SidePanel && !self.ui.side_visible) {
            self.ui.focus = Focus::Editor;
        }
        if self.ui.focus == Focus::Panel && !self.ui.panel_visible {
            self.ui.focus = Focus::Editor;
        }
        self.session.restored += 1;
        tracing::info!(tabs, groups = self.groups.len(), "session restored");
        self.needs_redraw = true;
        self.reveal_active();
        self.sync_diffs();
        self.sync_commit_tabs();
    }

    /// Buffers no tab shows (registered for a restore that didn't place them).
    fn drop_unshown_buffers(&mut self) {
        let ids: Vec<BufferId> = self.buffers.iter().map(|(id, _)| id).collect();
        for id in ids {
            if self.groups.tabs_showing(id) == 0 {
                self.buffers.remove(id);
            }
        }
    }

    /// A tab open before the restore, put in the restored active group and activated: the
    /// group's tab on the same buffer if it has one (taking this tab's cursor when the command
    /// line named a line), else a new tab right of its active one.
    fn carry_tab(&mut self, tab: Tab) {
        let goto = self.session.initial_goto;
        let group = self.group_mut();
        // The command line's binary file: the group's hex tab of it, as the session left it.
        if let Some(h) = tab.hex()
            && let Some(i) = group.find_hex(&h.path)
        {
            group.active = i;
            return;
        }
        if let Some(buffer) = tab.buffer()
            && let Some(i) = group.find(buffer)
        {
            if goto && let (Tab::Text(t), Some(Tab::Text(old))) = (tab, group.tabs.get_mut(i)) {
                old.view = t.view;
            }
            group.active = i;
            return;
        }
        group.open_tab(tab);
    }

    /// What `session::save` writes: the groups in reading order with their tabs (text tabs of
    /// files with a path, diff, commit and hex tabs), the active ones, the panels.
    fn session_snapshot(&self) -> Session {
        let root = &self.workspace.root;
        let groups = self
            .groups
            .ids()
            .into_iter()
            .filter_map(|id| Some((id, self.groups.get(id)?)))
            .map(|(id, g)| {
                let mut tabs = Vec::new();
                let mut active = 0;
                for (i, tab) in g.tabs.iter().enumerate() {
                    if i == g.active {
                        active = tabs.len();
                    }
                    match tab {
                        Tab::Text(t) => {
                            let Some(buf) = self.buffers.get(t.buffer) else {
                                continue;
                            };
                            if let Some(c) = &buf.commit {
                                tabs.push(TabState::Commit {
                                    commit: c.commit.clone(),
                                });
                                continue;
                            }
                            // Log tabs are never saved: a restart shows none.
                            if !buf.has_file() || !buf.path.is_absolute() {
                                continue;
                            }
                            let sels = &t.view.selections;
                            tabs.push(TabState::Text {
                                path: session::stored(root, &buf.path),
                                selections: sels.iter().map(|s| (s.anchor.0, s.head.0)).collect(),
                                primary: sels.primary_index(),
                                top_line: t.view.top_line(),
                                left_col: t.view.left_col,
                            });
                        }
                        Tab::Hex(h) => {
                            if !h.path.is_absolute() {
                                continue;
                            }
                            tabs.push(TabState::Hex {
                                path: session::stored(root, &h.path),
                                cursor: h.cursor,
                                column: h.column,
                                top: h.top,
                            });
                        }
                        Tab::Diff(d) => {
                            // A replace preview isn't kept (its replace is the Search view's).
                            let (Ok(left), Ok(right)) = (d.pair.left.try_into(), d.pair.right.try_into()) else {
                                continue;
                            };
                            tabs.push(TabState::Diff {
                                path: session::stored(root, &d.path),
                                left,
                                right,
                            });
                        }
                    }
                }
                let active = active.min(tabs.len().saturating_sub(1));
                GroupState { id, tabs, active }
            })
            .collect();
        Session {
            version: session::VERSION,
            root: root.clone(),
            side: SideState {
                visible: self.ui.side_visible,
                width: self.ui.side_width,
                view: self.ui.side_panel,
                history_collapsed: self.git_history.collapsed(),
                history_height: self.git_history.section_height,
            },
            panel: PanelState {
                visible: self.ui.panel_visible,
                height: self.ui.panel_height,
            },
            tree: self.groups.tree.clone(),
            groups,
            active_group: self.groups.active,
        }
    }

    /// Writes the session off-thread (`shutdown` awaits it). Nothing without a state dir, or
    /// while the restore is still reading (the old session stays).
    pub(super) fn save_session(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.mode.is_single() {
            return None;
        }
        let dir = self.state_dir.clone()?;
        if self.session.loading {
            tracing::info!("session not saved: its restore hadn't finished");
            return None;
        }
        let session = self.session_snapshot();
        Some(tokio::task::spawn_blocking(move || {
            match session::save(&dir, &session) {
                Ok(()) => tracing::debug!("session saved"),
                Err(e) => tracing::warn!("session not saved: {e}"),
            }
        }))
    }
}

/// A text tab's view from the session: its selections (clamped to the text, which may have
/// changed on disk since) and scroll, the cursor not revealed (the scroll is as it was).
fn restored_view(
    rope: &ropey::Rope,
    selections: &[(usize, usize)],
    primary: usize,
    top_line: usize,
    left_col: usize,
) -> View {
    let clamp = |b: usize| ByteIdx(rope.floor_char_boundary(b.min(rope.len())));
    let mut view = View::new();
    let list: Vec<Selection> = selections
        .iter()
        .map(|&(a, h)| Selection::new(clamp(a), clamp(h)))
        .collect();
    if !list.is_empty() {
        let primary = primary.min(list.len() - 1);
        view.selections = Selections::new(list, primary);
    }
    view.set_top_line(rope, top_line);
    view.left_col = left_col;
    view.reveal_cursor = false;
    view.cursor_history.reset(&view.selections);
    view
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::layout::{SidePanel, SplitTree};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    #[test]
    fn restored_selections_are_clamped_to_char_boundaries() {
        let rope = ropey::Rope::from_str("aé\nb\n");
        // Inside 'é' (bytes 1..3), and past the end.
        let view = restored_view(&rope, &[(2, 2), (0, 99)], 7, 50, 4);
        let sels: Vec<_> = view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(sels, [(0, 6)], "merged once clamped");
        let view = restored_view(&rope, &[(2, 2), (5, 99)], 1, 50, 4);
        let sels: Vec<_> = view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
        assert_eq!(sels, [(1, 1), (5, 6)]);
        assert_eq!(view.selections.primary_index(), 1);
        assert_eq!(
            (view.top_line(), view.left_col),
            (2, 4),
            "the scroll clamped to the last line"
        );
        assert!(!view.reveal_cursor);
        // No selections stored: the view's default cursor.
        let view = restored_view(&rope, &[], 0, 0, 0);
        assert_eq!(view.selections.primary().head, ByteIdx(0));
    }

    /// A file opened (quick open) while the restore reads the session, whose load ends after
    /// the restore registered the same file: one buffer, the session's tab activated.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_opened_while_the_restore_reads_is_not_loaded_twice() {
        let dir = tempfile::tempdir().unwrap();
        let base = workspace::normalize(dir.path());
        let (root, state) = (base.join("proj"), base.join("state"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "alpha\nbeta\n").unwrap();
        let s = Session {
            version: session::VERSION,
            root: root.clone(),
            side: SideState {
                visible: true,
                width: 30,
                view: SidePanel::Explorer,
                history_collapsed: false,
                history_height: crate::git_history::DEFAULT_SECTION_HEIGHT,
            },
            panel: PanelState {
                visible: false,
                height: 10,
            },
            tree: SplitTree::new(0),
            groups: vec![GroupState {
                id: 0,
                tabs: vec![TabState::Text {
                    path: "a.txt".into(),
                    selections: vec![(7, 7)],
                    primary: 0,
                    top_line: 0,
                    left_col: 0,
                }],
                active: 0,
            }],
            active_group: 0,
        };
        session::save(&state, &s).unwrap();

        let mut app = App::new(
            Workspace::load(root.clone()),
            TrustStore::load(state.join("trust.toml")),
            None,
            true,
        );
        app.test_start();
        // The open's load is in flight; the restore's read (made before it) lands first.
        app.open_file(root.join("a.txt"), true, None);
        let restore = session::read(
            &state,
            &root,
            &HashSet::new(),
            &HashSet::new(),
            app.settings.indentation(),
        );
        app.session_read(restore);
        assert_eq!(app.session.restored, 1);
        assert!(app.settle(|a| a.opening.is_empty()).await, "the open ends");
        assert_eq!(app.buffers.iter().count(), 1, "one buffer for a.txt");
        assert_eq!(app.group().tabs.len(), 1);
        let view = &app.group().active_tab().unwrap().view;
        assert_eq!(view.selections.primary().head, ByteIdx(7), "the session's cursor");
        assert_eq!(app.ui.focus, Focus::Editor);
    }
}
