//! The tab tooltip: the pointer resting on a tab for `DELAY` shows the tab's file path under it
//! (`ui::tab_tip`). `path_text` is the one place a file's displayed path is made; the tooltip and
//! Copy Relative Path both use it, so they agree.

use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{MouseEvent, MouseEventKind};

use super::App;
use crate::layout::{GroupId, Hit};

/// How long the pointer rests on a tab before the tooltip shows.
pub const DELAY: Duration = Duration::from_millis(500);

/// A file's path as shown: below `root`, `/`-separated; the full path when it isn't inside the
/// workspace (or is the root itself).
pub fn path_text(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.components().next().is_some() => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.display().to_string(),
    }
}

/// The tooltip shown for a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub group: GroupId,
    pub index: usize,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct TabTip {
    /// The tab the pointer rests on and when the tooltip is due.
    rest: Option<(GroupId, usize, Instant)>,
    pub shown: Option<Shown>,
}

impl TabTip {
    pub fn deadline(&self) -> Option<Instant> {
        self.rest.map(|(_, _, at)| at)
    }
}

impl App {
    /// The file path text of tab `index` in group `g`: `None` for an untitled buffer, a log or
    /// commit buffer, and a shell tab.
    pub(super) fn tab_path_text(&self, g: GroupId, index: usize) -> Option<String> {
        use crate::group::Tab;
        let path = match self.any_group(g)?.tabs.get(index)? {
            Tab::Text(t) => {
                let b = self.buffers.get(t.buffer)?;
                b.has_file().then(|| b.path.clone())?
            }
            Tab::Diff(d) => d.path.clone(),
            Tab::Hex(h) => h.path.clone(),
            Tab::Shell(_) => return None,
        };
        Some(path_text(&self.workspace.root, &path))
    }

    /// Drops the tooltip and the resting timer.
    pub(super) fn clear_tab_tip(&mut self) {
        self.tab_tip.rest = None;
        if self.tab_tip.shown.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// Every mouse event, before it is handled: a move over a tab starts the timer (or keeps it
    /// and the tooltip while on the same tab); anything else clears.
    pub(super) fn tab_tip_mouse(&mut self, m: &MouseEvent, hit: Hit) {
        let on = match hit {
            Hit::Tab(g, i) | Hit::TabClose(g, i) => Some((g, i)),
            _ => None,
        };
        let idle =
            self.context_menu.is_some() || self.prompt.is_some() || self.palette.is_some() || self.drag.is_some();
        match (m.kind, on) {
            (MouseEventKind::Moved, Some((g, i))) if !idle => {
                if self
                    .tab_tip
                    .shown
                    .as_ref()
                    .is_some_and(|s| (s.group, s.index) == (g, i))
                {
                    return;
                }
                if self.tab_tip.rest.is_some_and(|(rg, ri, _)| (rg, ri) == (g, i)) {
                    return;
                }
                self.clear_tab_tip();
                self.tab_tip.rest = Some((g, i, Instant::now() + DELAY));
            }
            _ => self.clear_tab_tip(),
        }
    }

    /// The timer's deadline passed (`now`): show the tooltip for the tab under the pointer.
    pub(super) fn tab_tip_tick(&mut self, now: Instant) {
        let Some((group, index, at)) = self.tab_tip.rest else {
            return;
        };
        if at > now {
            return;
        }
        self.tab_tip.rest = None;
        if let Some(text) = self.tab_path_text(group, index) {
            self.tab_tip.shown = Some(Shown { group, index, text });
            self.needs_redraw = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn path_text_is_relative_inside_the_workspace_and_full_outside() {
        let root = PathBuf::from("/w/proj");
        assert_eq!(path_text(&root, Path::new("/w/proj/src/a.rs")), "src/a.rs");
        assert_eq!(path_text(&root, Path::new("/w/proj/a.rs")), "a.rs");
        assert_eq!(path_text(&root, Path::new("/etc/hosts")), "/etc/hosts");
        assert_eq!(path_text(&root, Path::new("/w/project/a.rs")), "/w/project/a.rs");
        assert_eq!(path_text(&root, &root), "/w/proj");
    }
}
