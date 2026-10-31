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
    /// Left out by `.gitignore` / `.git/info/exclude` / `.ignore`, or inside an ignored folder.
    pub ignored: bool,
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
    /// Visible rows from the last render (`set_height`), for paging; 0 until the first draw.
    pub height: usize,
    /// A reveal asked for before the first draw (`height` 0), run by `set_height`.
    reveal_pending: bool,
    /// Where the explorer is expanding towards, e.g. the active file.
    reveal: Option<PathBuf>,
}

impl crate::scrollbar::Scrollable for Explorer {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        crate::scrollbar::Scroll::rows(self.row_count(), self.height, self.scroll)
    }

    fn set_top(&mut self, top: usize) {
        self.scroll = top.min(self.row_count().saturating_sub(self.height.max(1)));
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
                ignored: false,
                children: None,
            },
            selected: None,
            scroll: 0,
            height: 0,
            reveal_pending: false,
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
    /// Returns directories that should be listed next (to continue a reveal, or loaded folders
    /// whose ignored state changed).
    pub fn set_children(&mut self, dir: &Path, mut entries: Vec<Entry>) -> Vec<PathBuf> {
        let Some(e) = self.find_mut(dir) else { return Vec::new() };
        // Loaded folders whose ignored state changed: their listings are re-made with it.
        let mut relist = Vec::new();
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
                    if o.ignored != new.ignored && new.children.is_some() {
                        relist.push(new.path.clone());
                    }
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
        relist.extend(self.continue_reveal());
        relist
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

    /// Closes every folder; the selection moves to its top-level ancestor's row.
    pub fn collapse_all(&mut self) {
        fn close(e: &mut Entry) {
            for c in e.children.iter_mut().flatten() {
                c.expanded = false;
                close(c);
            }
        }
        let top = self.selected.as_ref().and_then(|sel| {
            let rel = sel.strip_prefix(&self.root.path).ok()?;
            Some(self.root.path.join(rel.components().next()?))
        });
        close(&mut self.root);
        self.reveal = None;
        if top.is_some() {
            self.selected = top;
        }
        self.scroll = 0;
        self.scroll_to_selected();
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
        let max = self.rows().len().saturating_sub(self.height.max(1));
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height(&mut self, height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.scroll_to_selected();
        }
    }

    /// Scrolls the least that shows the selected row; before the first draw (no height yet),
    /// waits for `set_height`.
    pub fn scroll_to_selected(&mut self) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
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
    walker_with(dir, excludes, |_| true)
}

/// `walker`, also skipping the entries (below `dir`) `keep` refuses: workspace search's folder
/// filter prunes folders with it.
pub fn walker_with(
    dir: &Path,
    excludes: &Override,
    keep: impl Fn(&ignore::DirEntry) -> bool + Send + Sync + 'static,
) -> WalkBuilder {
    let mut b = WalkBuilder::new(dir);
    b.hidden(false)
        .overrides(excludes.clone())
        .filter_entry(move |e| e.depth() == 0 || (!HIDDEN_DIRS.iter().any(|h| e.file_name() == *h) && keep(e)));
    b
}

/// `walker` without the ignore files (`.gitignore`, `.git/info/exclude`, `.ignore`, global): the
/// walk that also visits what they leave out. `.git/`, `.jed/` and the excludes still apply.
pub fn without_ignore_files(mut b: WalkBuilder) -> WalkBuilder {
    b.git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false);
    b
}

/// Blocking: one directory's entries, dirs first, case-insensitive. Ignored entries are listed
/// too, marked `ignored`.
#[cfg(test)]
pub fn list_dir(dir: &Path, excludes: &Override) -> std::io::Result<Vec<Entry>> {
    list_dir_in(dir, excludes, false)
}

/// `list_dir` for a directory that is itself ignored when `parent_ignored`: all its entries are.
pub fn list_dir_in(dir: &Path, excludes: &Override, parent_ignored: bool) -> std::io::Result<Vec<Entry>> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"));
    }
    let visible: std::collections::HashSet<PathBuf> = if parent_ignored {
        Default::default()
    } else {
        walker(dir, excludes)
            .max_depth(Some(1))
            .build()
            .filter_map(Result::ok)
            .map(|e| e.into_path())
            .collect()
    };
    let mut entries: Vec<Entry> = without_ignore_files(walker(dir, excludes))
        .max_depth(Some(1))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let path = e.path().to_path_buf();
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: path.is_dir(),
                expanded: false,
                ignored: parent_ignored || !visible.contains(&path),
                path,
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

    /// Moves `path` to the Trash. Tests never touch the user's real Trash: under `cfg(test)` the
    /// entry is removed instead.
    pub fn delete(path: &Path) -> io::Result<()> {
        if cfg!(test) {
            if fs::symlink_metadata(path)?.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            }
        } else {
            trash::delete(path).map_err(io::Error::other)
        }
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

    #[test]
    fn collapse_all_closes_folders_and_selects_the_top_level_row() {
        let (_d, root) = tree();
        let mut ex = Explorer::new(root.clone());
        ex.set_children(&root, list_dir(&root, &Override::empty()).unwrap());
        ex.expand(&root.join("src"));
        ex.set_children(
            &root.join("src"),
            list_dir(&root.join("src"), &Override::empty()).unwrap(),
        );
        ex.expand(&root.join("src/deep"));
        ex.selected = Some(root.join("src/deep"));
        let before = ex.rows().len();
        ex.collapse_all();
        assert!(ex.rows().len() < before);
        assert!(ex.rows().iter().all(|r| r.depth == 0));
        assert_eq!(ex.selected, Some(root.join("src")));
        assert!(!ex.find(&root.join("src")).unwrap().expanded);
        assert!(!ex.find(&root.join("src/deep")).unwrap().expanded);
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn listing_marks_ignored_entries_and_their_subtree() {
        let (_d, root) = tree();
        let got = list_dir(&root, &Override::empty()).unwrap();
        let flags: Vec<(&str, bool)> = got.iter().map(|e| (e.name.as_str(), e.ignored)).collect();
        assert!(flags.contains(&("target", true)) && flags.contains(&("src", false)));
        assert!(!names(&got).contains(&".git") && !names(&got).contains(&".jed"));
        let inside = list_dir_in(&root.join("src"), &Override::empty(), true).unwrap();
        // No pattern matches `main.rs` / `deep` themselves: ignored only through the parent.
        assert_eq!(names(&inside), vec!["deep", "main.rs"]);
        assert!(inside.iter().all(|e| e.ignored));
        let alone = list_dir_in(&root.join("src"), &Override::empty(), false).unwrap();
        assert!(
            alone.iter().all(|e| !e.ignored),
            "the same folder listed as not ignored"
        );
    }

    /// `.ignore` counts outside a git repo (`.gitignore` only inside one, as for the search);
    /// a nested ignore file marks entries of its own folder; excludes still hide entries.
    #[test]
    fn listing_marks_by_dot_ignore_and_nested_ignore_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::create_dir_all(root.join("sub/gen")).unwrap();
        fs::write(root.join(".ignore"), "*.tmp\n").unwrap();
        fs::write(root.join(".gitignore"), "*.txt\n").unwrap();
        fs::write(root.join("a.tmp"), "").unwrap();
        fs::write(root.join("a.txt"), "").unwrap();
        fs::write(root.join("x.o"), "").unwrap();
        fs::write(root.join("sub/.ignore"), "gen/\n").unwrap();
        fs::write(root.join("sub/keep.rs"), "").unwrap();
        let flags = |dir: &Path| -> Vec<(String, bool)> {
            let ex = excludes(&root, &["*.o".into()]);
            list_dir(dir, &ex)
                .unwrap()
                .into_iter()
                .map(|e| (e.name, e.ignored))
                .collect()
        };
        let top = flags(&root);
        assert!(top.contains(&("a.tmp".into(), true)), "{top:?}");
        assert!(top.contains(&("a.txt".into(), false)), "not a git repo: {top:?}");
        assert!(top.contains(&("sub".into(), false)) && !top.iter().any(|(n, _)| n == "x.o"));
        let sub = flags(&root.join("sub"));
        assert!(
            sub.contains(&("gen".into(), true)) && sub.contains(&("keep.rs".into(), false)),
            "{sub:?}"
        );
    }

    /// A loaded folder whose ignored state changes in a fresh listing is returned for relisting
    /// (its entries carry the flag), keeping it expanded; an unchanged one isn't.
    #[test]
    fn a_folder_turning_ignored_is_listed_again() {
        let (_d, root) = tree();
        let mut x = Explorer::new(root.clone());
        let ex = Override::empty();
        x.set_children(&root, list_dir(&root, &ex).unwrap());
        x.expand(&root.join("src"));
        x.set_children(&root.join("src"), list_dir(&root.join("src"), &ex).unwrap());
        assert!(x.set_children(&root, list_dir(&root, &ex).unwrap()).is_empty());
        fs::write(root.join(".gitignore"), "target/\nsrc/\n").unwrap();
        let again = x.set_children(&root, list_dir(&root, &ex).unwrap());
        assert_eq!(again, vec![root.join("src")]);
        let src = x.find(&root.join("src")).unwrap();
        assert!(src.ignored && src.expanded && src.children.is_some());
    }

    #[test]
    fn listing_sorts_and_filters() {
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let got = list_dir(&root, &ex).unwrap();
        assert_eq!(names(&got), vec!["src", "target", ".gitignore", "A.txt", "b.txt"]);
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
            vec![
                "src",
                "  deep",
                "    f.rs",
                "  main.rs",
                "target",
                ".gitignore",
                "A.txt",
                "b.txt"
            ]
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
        assert_eq!((x.row_count(), x.rows().len()), (8, 8));
        x.collapse(&root.join("src/deep"));
        assert_eq!((x.row_count(), x.rows().len()), (7, 7), "a collapsed folder's children");
        x.height = 4;
        x.set_top(1);
        assert_eq!(x.scroll().top, 1);
        x.set_top(100);
        assert_eq!(x.scroll, 3, "the last row at the bottom");
        assert_eq!(x.selected.as_deref(), Some(root.join("src/deep/f.rs").as_path()));
        x.collapse(&root.join("src"));
        assert_eq!((x.row_count(), x.rows().len()), (5, 5));
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("a.txt", false).is_ok());
        assert!(validate_name("a/b.txt", true).is_ok());
        assert!(validate_name("a/b.txt", false).is_err());
        assert!(validate_name("..", true).is_err());
        assert!(validate_name("  ", true).is_err());
    }

    /// A reveal before the first draw (no height) waits for `set_height`, which scrolls the
    /// least that shows the revealed file.
    #[test]
    fn a_reveal_before_the_first_draw_waits_for_the_height() {
        let (_d, root) = tree();
        let ex = excludes(&root, &["build/".into(), "*.o".into()]);
        let mut x = Explorer::new(root.clone());
        let target = root.join("src/main.rs");
        let mut todo = vec![root.clone()];
        todo.extend(x.reveal(&target));
        while let Some(dir) = todo.pop() {
            let listing = list_dir(&dir, &ex).unwrap();
            todo.extend(x.set_children(&dir, listing));
        }
        let i = x.rows().iter().position(|r| r.entry.path == target).unwrap();
        assert!(i >= 2, "{i}");
        assert_eq!(x.scroll, 0, "nothing scrolled without a height");
        x.set_height(2);
        assert_eq!(x.scroll, i - 1, "the revealed row at the bottom");
        x.scroll = 0;
        x.set_height(2);
        assert_eq!(x.scroll, 0, "only once");
    }
}
