//! Explorer tree model: lazily loaded, `.gitignore`-aware. No drawing here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    /// `None` until listed.
    pub children: Option<Vec<Entry>>,
}

/// A visible row: an entry and its depth below the root.
pub struct Row<'a> {
    pub depth: usize,
    pub entry: &'a Entry,
}

#[derive(Debug)]
pub struct Explorer {
    root: Entry,
    pub selected: Option<PathBuf>,
    pub scroll: usize,
    /// Visible rows from the last render, for paging.
    pub height: usize,
    /// Where the explorer is expanding towards, e.g. the active file.
    reveal: Option<PathBuf>,
}

impl crate::scrollbar::Scrollable for Explorer {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.row_count(), self.height, self.scroll)
    }

    fn set_top(&mut self, top: usize) {
        self.scroll = top.min(self.row_count().saturating_sub(self.height));
    }
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
        Self {
            root: Entry {
                name,
                path: root,
                is_dir: true,
                expanded: true,
                children: None,
            },
            selected: None,
            scroll: 0,
            height: 1,
            reveal: None,
        }
    }

    /// Expanded entries in display order; the root itself isn't shown.
    pub fn rows(&self) -> Vec<Row<'_>> {
        fn walk<'a>(e: &'a Entry, depth: usize, out: &mut Vec<Row<'a>>) {
            for c in e.children.iter().flatten() {
                out.push(Row { depth, entry: c });
                if c.expanded {
                    walk(c, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, 0, &mut out);
        out
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        let rel = path.strip_prefix(&self.root.path).ok()?;
        let mut e = &mut self.root;
        for part in rel.components() {
            e = e
                .children
                .as_mut()?
                .iter_mut()
                .find(|c| c.path.file_name() == Some(part.as_os_str()))?;
        }
        Some(e)
    }

    pub fn find(&self, path: &Path) -> Option<&Entry> {
        let rel = path.strip_prefix(&self.root.path).ok()?;
        let mut e = &self.root;
        for part in rel.components() {
            e = e
                .children
                .as_ref()?
                .iter()
                .find(|c| c.path.file_name() == Some(part.as_os_str()))?;
        }
        Some(e)
    }

    /// Whether `dir`'s listing is loaded (so a change inside it needs a refresh).
    pub fn is_loaded(&self, dir: &Path) -> bool {
        self.find(dir).is_some_and(|e| e.children.is_some())
    }

    /// Directories whose listings are loaded, for refreshing after a config change.
    pub fn loaded_dirs(&self) -> Vec<PathBuf> {
        fn walk(e: &Entry, out: &mut Vec<PathBuf>) {
            if let Some(cs) = &e.children {
                out.push(e.path.clone());
                cs.iter().for_each(|c| walk(c, out));
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    /// Installs a fresh listing, keeping expanded state and loaded subtrees of entries that still exist.
    /// Returns directories that should be listed next (to continue a reveal).
    pub fn set_children(&mut self, dir: &Path, mut entries: Vec<Entry>) -> Vec<PathBuf> {
        let Some(e) = self.find_mut(dir) else { return Vec::new() };
        if let Some(old) = e.children.take() {
            let mut old: HashMap<String, Entry> = old
                .into_iter()
                .filter(|o| o.is_dir)
                .map(|o| (o.name.clone(), o))
                .collect();
            for new in entries.iter_mut().filter(|n| n.is_dir) {
                if let Some(o) = old.remove(&new.name) {
                    new.expanded = o.expanded;
                    new.children = o.children;
                }
            }
        }
        e.children = Some(entries);
        if let Some(sel) = &self.selected
            && self.find(sel).is_none()
            && sel.parent() == Some(dir)
        {
            self.selected = None;
        }
        self.continue_reveal()
    }

    /// Expands a directory; returns it if it needs listing.
    pub fn expand(&mut self, dir: &Path) -> Option<PathBuf> {
        let e = self.find_mut(dir)?;
        if !e.is_dir {
            return None;
        }
        e.expanded = true;
        e.children.is_none().then(|| dir.to_path_buf())
    }

    pub fn collapse(&mut self, dir: &Path) {
        if let Some(e) = self.find_mut(dir) {
            e.expanded = false;
        }
    }

    /// Expands the ancestors of `path` and selects it, listing directories as needed.
    /// Returns directories to list.
    pub fn reveal(&mut self, path: &Path) -> Vec<PathBuf> {
        if !path.starts_with(&self.root.path) {
            return Vec::new();
        }
        self.reveal = Some(path.to_path_buf());
        self.continue_reveal()
    }

    fn continue_reveal(&mut self) -> Vec<PathBuf> {
        let Some(target) = self.reveal.clone() else {
            return Vec::new();
        };
        let mut chain: Vec<&Path> = target
            .ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(&self.root.path))
            .collect();
        chain.reverse();
        for dir in chain {
            if let Some(need) = self.expand(dir) {
                return vec![need];
            }
            if self.find(dir).is_none() {
                // Not listed yet (a parent's refresh is pending): wait for it.
                return Vec::new();
            }
        }
        self.reveal = None;
        if self.find(&target).is_some() {
            self.selected = Some(target);
            self.scroll_to_selected();
        }
        Vec::new()
    }

    fn selected_index(&self, rows: &[Row]) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        rows.iter().position(|r| &r.entry.path == sel)
    }

    pub fn select_index(&mut self, index: usize) {
        self.reveal = None;
        let rows = self.rows();
        if let Some(r) = rows.get(index.min(rows.len().saturating_sub(1))) {
            self.selected = Some(r.entry.path.clone());
        }
        self.scroll_to_selected();
    }

    /// Moves the selection by `delta` rows (clamped).
    pub fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let i = match self.selected_index(&rows) {
            Some(i) => i.saturating_add_signed(delta).min(rows.len() - 1),
            None => 0,
        };
        drop(rows);
        self.select_index(i);
    }

    pub fn select_last(&mut self) {
        let n = self.rows().len();
        self.select_index(n.saturating_sub(1));
    }

    /// Left: collapse an expanded directory, otherwise go to the parent.
    pub fn left(&mut self) {
        self.reveal = None;
        let Some(sel) = self.selected.clone() else { return };
        match self.find(&sel) {
            Some(e) if e.is_dir && e.expanded => self.collapse(&sel),
            _ => {
                if let Some(parent) = sel.parent().filter(|p| *p != self.root.path) {
                    self.selected = Some(parent.to_path_buf());
                    self.scroll_to_selected();
                }
            }
        }
    }

    /// Right: expand a collapsed directory, or step into an expanded one. Returns a dir to list.
    pub fn right(&mut self) -> Option<PathBuf> {
        self.reveal = None;
        let sel = self.selected.clone()?;
        let e = self.find(&sel)?;
        if !e.is_dir {
            return None;
        }
        if e.expanded {
            self.move_selection(1);
            None
        } else {
            self.expand(&sel)
        }
    }

    /// How many rows `rows` gives, without collecting them.
    pub fn row_count(&self) -> usize {
        fn count(e: &Entry) -> usize {
            e.children
                .iter()
                .flatten()
                .map(|c| 1 + if c.expanded { count(c) } else { 0 })
                .sum()
        }
        count(&self.root)
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.rows().len().saturating_sub(self.height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    pub fn scroll_to_selected(&mut self) {
        let rows = self.rows();
        if let Some(i) = self.selected_index(&rows) {
            if i < self.scroll {
                self.scroll = i;
            } else if i >= self.scroll + self.height {
                self.scroll = i + 1 - self.height;
            }
        }
    }

    /// The directory new files go in: the selected dir, the selected file's dir, or the root.
    pub fn target_dir(&self) -> PathBuf {
        match &self.selected {
            Some(p) if self.find(p).is_some_and(|e| e.is_dir) => p.clone(),
            Some(p) => p.parent().map_or_else(|| self.root.path.clone(), Path::to_path_buf),
            None => self.root.path.clone(),
        }
    }
}

/// `.workspace` exclude globs as an `ignore` override (negated globs hide matches).
pub fn excludes(root: &Path, globs: &[String]) -> Override {
    let mut b = OverrideBuilder::new(root);
    for g in globs {
        if let Err(e) = b.add(&format!("!{g}")) {
            tracing::warn!(glob = g, "bad exclude glob: {e}");
        }
    }
    b.build().unwrap_or_else(|_| Override::empty())
}

/// Never shown, listed or searched: git's store and jed's own state (file history).
pub const HIDDEN_DIRS: [&str; 2] = [".git", ".jed"];

/// The walk the explorer, quick open and workspace search share: `.gitignore` (inside git
/// repos), the excludes, and never `.git/` or `.jed/`. Dotfiles are shown.
pub fn walker(dir: &Path, excludes: &Override) -> WalkBuilder {
    let mut b = WalkBuilder::new(dir);
    b.hidden(false)
        .overrides(excludes.clone())
        .filter_entry(|e| e.depth() == 0 || !HIDDEN_DIRS.iter().any(|h| e.file_name() == *h));
    b
}

/// Blocking: one directory's entries, dirs first, case-insensitive.
pub fn list_dir(dir: &Path, excludes: &Override) -> std::io::Result<Vec<Entry>> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"));
    }
    let walker = walker(dir, excludes).max_depth(Some(1)).build();
    let mut entries: Vec<Entry> = walker
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let path = e.path().to_path_buf();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: path.is_dir(),
                path,
                expanded: false,
                children: None,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Blocking: every file under `root` the explorer would show, as paths relative to `root`,
/// passed to `found` as the walk goes. Stops early once `cancel` is set.
pub fn walk_files(root: &Path, excludes: &Override, cancel: &AtomicBool, mut found: impl FnMut(PathBuf)) {
    for entry in walker(root, excludes).build() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            found(rel.to_path_buf());
        }
    }
}

/// File operations, all blocking: call from `spawn_blocking`.
pub mod ops {
    use std::fs;
    use std::io;
    use std::path::Path;

    pub fn new_file(path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ())
    }

    pub fn new_folder(path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        // Same name with different case must still work on case-insensitive filesystems.
        let same = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
        if to.exists() && !same {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a file or folder with that name exists",
            ));
        }
        fs::rename(from, to)
    }

    pub fn delete(path: &Path) -> io::Result<()> {
        trash::delete(path).map_err(io::Error::other)
    }
}

/// Checks a name typed for a new or renamed entry. New entries may contain `/` to create nested folders.
pub fn validate_name(name: &str, allow_nested: bool) -> Result<(), &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name is required");
    }
    if name.contains('/') && !allow_nested {
        return Err("Name can't contain /");
    }
    if name.starts_with('/') || name.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err("Not a valid name");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".jed/history")).unwrap();
        fs::write(root.join(".jed/history/entries.toml"), "").unwrap();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("b.txt"), "").unwrap();
        fs::write(root.join("A.txt"), "").unwrap();
        fs::write(root.join("x.o"), "").unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/deep/f.rs"), "").unwrap();
        (dir, root)
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn listing_sorts_and_filters() {
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let got = list_dir(&root, &ex).unwrap();
        assert_eq!(names(&got), vec!["src", ".gitignore", "A.txt", "b.txt"]);
    }

    #[test]
    fn file_walk_skips_what_the_explorer_skips() {
        let (_d, root) = tree();
        fs::write(root.join("target/out.bin"), "").unwrap();
        fs::write(root.join("build/gen.c"), "").unwrap();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let mut files = Vec::new();
        walk_files(&root, &ex, &AtomicBool::new(false), |p| {
            files.push(p.display().to_string())
        });
        files.sort();
        assert_eq!(
            files,
            vec![".gitignore", "A.txt", "b.txt", "src/deep/f.rs", "src/main.rs"]
        );

        let mut none = Vec::new();
        walk_files(&root, &ex, &AtomicBool::new(true), |p| none.push(p));
        assert!(none.is_empty(), "cancelled before the first file");
    }

    #[test]
    fn reveal_lists_and_expands_ancestors() {
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let mut x = Explorer::new(root.clone());
        let target = root.join("src/deep/f.rs");

        let mut todo = vec![root.clone()];
        todo.extend(x.reveal(&target));
        while let Some(dir) = todo.pop() {
            let listing = list_dir(&dir, &ex).unwrap();
            todo.extend(x.set_children(&dir, listing));
        }
        assert_eq!(x.selected.as_deref(), Some(target.as_path()));
        let rows: Vec<String> = x
            .rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.entry.name))
            .collect();
        assert_eq!(
            rows,
            vec!["src", "  deep", "    f.rs", "  main.rs", ".gitignore", "A.txt", "b.txt"]
        );
    }

    #[test]
    fn keyboard_navigation_and_refresh_keeps_state() {
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let mut x = Explorer::new(root.clone());
        x.height = 10;
        x.set_children(&root, list_dir(&root, &ex).unwrap());
        x.move_selection(1);
        assert_eq!(x.selected.as_deref(), Some(root.join("src").as_path()));
        let need = x.right().unwrap();
        x.set_children(&need, list_dir(&need, &ex).unwrap());
        x.right();
        assert_eq!(x.selected.as_deref(), Some(root.join("src/deep").as_path()));
        x.left();
        assert_eq!(x.selected.as_deref(), Some(root.join("src").as_path()));

        // A refresh of the root keeps src expanded.
        fs::write(root.join("c.txt"), "").unwrap();
        x.set_children(&root, list_dir(&root, &ex).unwrap());
        assert!(x.find(&root.join("src")).unwrap().expanded);
        assert!(x.find(&root.join("c.txt")).is_some());
        x.left();
        assert!(!x.find(&root.join("src")).unwrap().expanded);
    }

    /// `row_count` counts what `rows` gives (expanded folders' children only) without
    /// collecting; the scroll bar's `set_top` clamps as the wheel does, the selection stays.
    #[test]
    fn row_count_and_the_scroll_bars_top() {
        use crate::scrollbar::Scrollable;
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let mut x = Explorer::new(root.clone());
        let mut todo = vec![root.clone()];
        todo.extend(x.reveal(&root.join("src/deep/f.rs")));
        while let Some(dir) = todo.pop() {
            let listing = list_dir(&dir, &ex).unwrap();
            todo.extend(x.set_children(&dir, listing));
        }
        assert_eq!((x.row_count(), x.rows().len()), (7, 7));
        x.collapse(&root.join("src/deep"));
        assert_eq!((x.row_count(), x.rows().len()), (6, 6), "a collapsed folder's children");
        x.height = 4;
        x.set_top(1);
        assert_eq!(x.scroll().top, 1);
        x.set_top(100);
        assert_eq!(x.scroll, 2, "the last row at the bottom");
        assert_eq!(x.selected.as_deref(), Some(root.join("src/deep/f.rs").as_path()));
        x.collapse(&root.join("src"));
        assert_eq!((x.row_count(), x.rows().len()), (4, 4));
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("a.txt", false).is_ok());
        assert!(validate_name("a/b.txt", true).is_ok());
        assert!(validate_name("a/b.txt", false).is_err());
        assert!(validate_name("..", true).is_err());
        assert!(validate_name("  ", true).is_err());
    }
}
