//! Workspace search (M7): starting a search and taking in its results. The Search view (7b)
//! draws `app.search` and calls `start_search` as the query changes.

use super::{App, AppEvent};
use crate::search::SearchError;
use crate::workspace_search::{self, Batch, Job, Query, Results};

impl App {
    /// Starts searching the workspace for `query`, cancelling the running search. Open buffers
    /// with unsaved changes inside the root are searched from their text. An empty pattern
    /// gives finished, empty results without searching; an invalid regex clears the results
    /// and returns the error (nothing runs).
    #[allow(dead_code)] // the Search view (7b) calls it; tests do now
    pub fn start_search(&mut self, query: Query) -> Result<(), SearchError> {
        self.search_task = None; // cancels it
        self.search_generation += 1;
        let generation = self.search_generation;
        let matcher = match workspace_search::matcher(&query) {
            Ok(m) => m,
            Err(e) => {
                self.search = None;
                return Err(e);
            }
        };
        let mut results = Results::new(generation, query);
        if matcher.is_empty() {
            results.done = true;
            self.search = Some(results);
            return Ok(());
        }
        let root = self.workspace.root.clone();
        let dirty = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_dirty())
            .filter_map(|(_, b)| Some((b.path.strip_prefix(&root).ok()?.to_path_buf(), b.rope.clone())))
            .collect();
        let job = Job {
            root,
            excludes: self.excludes.clone(),
            matcher,
            dirty,
            generation,
        };
        let tx = self.tx.clone();
        self.search_task = Some(workspace_search::start(job, move |batch| {
            let _ = tx.send(AppEvent::SearchResults(batch));
        }));
        self.search = Some(results);
        Ok(())
    }

    /// A batch of results: added when it belongs to the current search, else dropped.
    pub(super) fn search_results(&mut self, batch: Batch) {
        let Some(results) = &mut self.search else { return };
        if results.add(batch) && results.done {
            self.search_task = None;
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // one-match lines
mod tests {
    use std::fs;

    use crate::buffer::Buffer;
    use crate::workspace_search::{FileMatches, LineMatch};

    use super::*;

    fn batch(generation: u64, done: bool) -> Batch {
        Batch {
            generation,
            files: vec![FileMatches {
                path: "x.txt".into(),
                lines: vec![LineMatch {
                    line: 0,
                    ranges: vec![0..1],
                    text: "x".into(),
                    text_start: 0,
                }],
            }],
            done,
            truncated: false,
        }
    }

    /// A replaced search's batches (even its last one) change nothing and don't release the
    /// running search; the current one's last batch does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_replaced_searchs_batches_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let mut app = crate::milestone_tests::test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
        let q = |p: &str| Query {
            pattern: p.into(),
            ..Query::default()
        };
        app.start_search(q("needle")).unwrap();
        let old = app.search.as_ref().unwrap().generation;
        app.start_search(q("other")).unwrap();
        let new = app.search.as_ref().unwrap().generation;
        assert!(new > old);
        app.search_results(batch(old, true));
        let r = app.search.as_ref().unwrap();
        assert!(r.files.is_empty() && !r.done);
        assert!(app.search_task.is_some());
        app.search_results(batch(new, false));
        assert!(app.search_task.is_some(), "still running");
        app.search_results(batch(new, true));
        let r = app.search.as_ref().unwrap();
        assert_eq!((r.total, r.done), (1, true), "x.txt replaced, not counted twice");
        assert!(app.search_task.is_none());
    }
}
