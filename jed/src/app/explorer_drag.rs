//! Dragging an Explorer row onto a folder moves it there (`Drag::Explorer`): the folder under
//! the pointer (a file's row means its folder, empty space below the tree the workspace root)
//! is shaded (`list.dropBackground`) while it would accept the drop; a release moves the entry
//! through `FileOp::Rename`, so open tabs, file history and listings follow as for a rename.

use std::path::{Path, PathBuf};

use super::mouse::Drag;
use super::{App, FileOp, Level};
use crate::layout::Hit;

/// An Explorer drag in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerDrag {
    /// The pressed row's path.
    pub from: PathBuf,
    /// The folder a release here would move `from` into; `None` when the pointer is not over
    /// a folder that accepts it.
    pub target: Option<PathBuf>,
}

/// Where moving `from` into `dir` is allowed: not into itself or below, not where it already is.
pub fn accepts(from: &Path, dir: &Path) -> bool {
    !dir.starts_with(from) && from.parent() != Some(dir)
}

impl App {
    /// The folder an Explorer drag in flight would drop into (the one to shade).
    pub fn explorer_drop_target(&self) -> Option<PathBuf> {
        match &self.drag {
            Some(Drag::Explorer(d)) => d.target.clone(),
            _ => None,
        }
    }

    /// A press on an Explorer row arms a drag (the root row can't move).
    pub(super) fn explorer_drag_start(&mut self, from: PathBuf) {
        if from != self.workspace.root {
            self.drag = Some(Drag::Explorer(ExplorerDrag { from, target: None }));
        }
    }

    /// The folder a drop at (`x`, `y`) lands in, if any.
    fn explorer_drop_dir(&self, x: u16, y: u16) -> Option<PathBuf> {
        match self.hits.hit(x, y) {
            Hit::ExplorerRow(row) => {
                let rows = self.explorer.rows();
                let entry = rows.get(self.explorer.scroll + row as usize)?.entry;
                if entry.is_dir {
                    Some(entry.path.clone())
                } else {
                    entry.path.parent().map(Path::to_path_buf)
                }
            }
            Hit::SidePanel if self.ui.side_panel == crate::layout::SidePanel::Explorer => {
                Some(self.workspace.root.clone())
            }
            _ => None,
        }
    }

    pub(super) fn explorer_drag_to(&mut self, x: u16, y: u16) {
        let Some(Drag::Explorer(d)) = &self.drag else { return };
        let target = self.explorer_drop_dir(x, y).filter(|dir| accepts(&d.from, dir));
        if let Some(Drag::Explorer(d)) = &mut self.drag {
            d.target = target;
        }
    }

    /// A release: moves the entry into the target folder.
    pub(super) fn finish_explorer_drag(&mut self, x: u16, y: u16) {
        self.explorer_drag_to(x, y);
        let Some(Drag::Explorer(d)) = self.drag.take() else {
            return;
        };
        let Some(dir) = d.target else { return };
        let Some(name) = d.from.file_name() else { return };
        let to = dir.join(name);
        if to.exists() {
            self.notify(
                Level::Error,
                format!("'{}' already exists in '{}'", name.to_string_lossy(), dir.display()),
            );
            return;
        }
        self.run_file_op(FileOp::Rename { from: d.from, to });
    }
}
